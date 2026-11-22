//! A bounded ext2 reader and writer over a sector device (spec 0019, W1–W2,
//! W4, W7; ADR 0040).
//!
//! It talks to a [`SectorDevice`] instead of an identity-mapped byte range, so
//! the same validation and lookup code runs over the kernel's ATA PIO driver
//! and over an in-memory device in the host tests. The writer covers create and
//! write (M6.2) plus remove, mkdir and rename (M6.3), with synchronous,
//! write-through block updates (H4).
//!
//! The supported subset is deliberately the same one the read-only application
//! image uses: 1 KiB blocks, revision 1, no journal and no extents, direct plus
//! single and double indirect blocks, and at most
//! [`abi::fs::MAX_COMPONENTS`] path components. Anything outside the subset
//! refuses the mount rather than returning a best-effort read.
//!
//! Nothing allocates. A fixed block cache turns a filesystem block into two
//! 512-byte sector reads; every inode, descriptor and directory block is
//! bounds-checked before it is used, so a corrupt image is a [`Status`], never
//! a panic.
//!
//! # Write ordering
//!
//! There is no journal, so the writer orders its blocks the way ext2 recovery
//! expects (spec 0019, "Consistency and failure"):
//!
//! 1. an allocation sets the bitmap bit before anything refers to the block or
//!    inode, then the group free count, then the superblock free count;
//! 2. new data is written before the pointer that makes it reachable (the
//!    indirect block, then the inode);
//! 3. a new inode is initialized before the directory entry names it, and a
//!    new directory block is written before the parent inode points at it.
//!
//! Every one of those writes goes to the device before the call returns; the
//! cache is never a write-back queue (H4). A capacity check runs before the
//! first mutation, so a full filesystem is [`Status::CapacityExceeded`] with no
//! partial change; an I/O failure between two writes is not recoverable inside
//! this slice and may leave an orphaned block or inode for `e2fsck`.

#![no_std]
#![forbid(unsafe_code)]

use core::cell::RefCell;

use abi::Status;

/// Byte offset of the superblock (block 1 of a 1 KiB-block filesystem).
const SUPERBLOCK_OFFSET: usize = 1024;
/// Size of the superblock record.
const SUPERBLOCK_BYTES: usize = 1024;
/// ext2 superblock magic (`s_magic`).
const EXT2_MAGIC: u16 = 0xEF53;
/// Root directory inode number.
const ROOT_INODE: u32 = 2;
/// `INCOMPAT_FILETYPE`: directory entries carry a file-type byte. The only
/// incompatible feature this implementation accepts.
const INCOMPAT_FILETYPE: u32 = 0x0002;
/// Size of a group descriptor (the 32-bit variant).
const GROUP_DESC_BYTES: usize = 32;
/// Inode mode mask and the regular-file and directory kinds.
const S_IFMT: u16 = 0xF000;
const S_IFREG: u16 = 0x8000;
const S_IFDIR: u16 = 0x4000;
/// `S_IFREG | 0644`: the mode a created regular file gets (spec 0019, W3).
const REGULAR_FILE_MODE: u16 = S_IFREG | 0o644;
/// `S_IFDIR | 0755`: the mode a created directory gets (spec 0019, W5).
const DIRECTORY_MODE: u16 = S_IFDIR | 0o755;
/// The mandatory inode area every ext2 revision shares.
const INODE_BYTES: usize = 128;
/// `i_links_count` offset inside an inode.
const INODE_LINKS: usize = 26;
/// `i_blocks` (512-byte units) offset inside an inode.
const INODE_BLOCKS512: usize = 28;
/// `i_block[0]` offset inside an inode.
const INODE_BLOCK_WORDS: usize = 40;
/// `i_size_high` offset inside an inode (the high half of a regular file size).
const INODE_SIZE_HIGH: usize = 108;
/// `EXT2_FT_REG_FILE`: the directory-entry type of a regular file.
const EXT2_FT_REG_FILE: u8 = 1;
/// `EXT2_FT_DIR`: the directory-entry type of a directory.
const EXT2_FT_DIR: u8 = 2;
/// `s_state` value for a cleanly mounted filesystem (`EXT2_VALID_FS`).
const SUPERBLOCK_VALID_FS: u16 = 1;
/// Superblock offsets the writer touches.
const SB_FREE_BLOCKS: usize = 12;
const SB_FREE_INODES: usize = 16;
const SB_STATE: usize = 58;
const SB_ERRORS: usize = 60;
/// Group-descriptor offsets of the two free counts.
const GD_FREE_BLOCKS: usize = 12;
const GD_FREE_INODES: usize = 14;
/// Group-descriptor offset of the used-directory count (`bg_used_dirs_count`).
const GD_USED_DIRS: usize = 16;
/// Words in `i_block`: 12 direct, 1 single, 1 double, 1 triple indirect.
const INODE_WORDS: usize = 15;
/// Fixed filesystem block size in bytes (spec 0019, W2: 1 KiB blocks).
pub const BLOCK_SIZE: usize = 1024;
/// One sector, the ATA PIO unit.
pub const SECTOR_SIZE: usize = 512;
/// Sectors per filesystem block.
const SECTORS_PER_BLOCK: usize = BLOCK_SIZE / SECTOR_SIZE;
/// Number of cached blocks. Eight covers an inode table block, a bitmap, a
/// directory block and the indirect block of a write without thrashing.
const CACHE_BLOCKS: usize = 8;

/// A random-access block device measured in 512-byte sectors.
///
/// The kernel's ATA PIO driver and the host tests' in-memory image both
/// implement this. `write_sector` is part of the contract from M6.1 on so the
/// writer can use the same device; the read-only mount never calls it.
pub trait SectorDevice {
    /// The device's sector size, in bytes.
    const SECTOR_SIZE: usize = SECTOR_SIZE;

    /// Number of 512-byte sectors on the device.
    fn sector_count(&self) -> u64;

    /// Reads sector `index` into `out`.
    fn read_sector(&self, index: u64, out: &mut [u8; SECTOR_SIZE]) -> Result<(), Status>;

    /// Writes `data` to sector `index`.
    fn write_sector(&mut self, index: u64, data: &[u8; SECTOR_SIZE]) -> Result<(), Status>;
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, Status> {
    let end = offset.checked_add(2).ok_or(Status::Invalid)?;
    let slice = bytes.get(offset..end).ok_or(Status::Invalid)?;
    Ok(u16::from_le_bytes([slice[0], slice[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, Status> {
    let end = offset.checked_add(4).ok_or(Status::Invalid)?;
    let slice = bytes.get(offset..end).ok_or(Status::Invalid)?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn put_u16(bytes: &mut [u8], offset: usize, value: u16) -> Result<(), Status> {
    let end = offset.checked_add(2).ok_or(Status::Invalid)?;
    let slice = bytes.get_mut(offset..end).ok_or(Status::Invalid)?;
    slice.copy_from_slice(&value.to_le_bytes());
    Ok(())
}

fn put_u32(bytes: &mut [u8], offset: usize, value: u32) -> Result<(), Status> {
    let end = offset.checked_add(4).ok_or(Status::Invalid)?;
    let slice = bytes.get_mut(offset..end).ok_or(Status::Invalid)?;
    slice.copy_from_slice(&value.to_le_bytes());
    Ok(())
}

/// Rounds a directory-record length up to the ext2 four-byte alignment.
fn align4(value: usize) -> usize {
    (value + 3) & !3
}

/// Returns the first clear bit below `capacity`, or `None` when all are set.
fn first_free_bit(bitmap: &[u8; BLOCK_SIZE], capacity: u32) -> Option<u32> {
    for index in 0..capacity {
        let byte = bitmap[(index / 8) as usize];
        if byte & (1 << (index % 8)) == 0 {
            return Some(index);
        }
    }
    None
}

/// Sets one bit in a bitmap block. `bit` is already known to be in range.
fn set_bit(bitmap: &mut [u8; BLOCK_SIZE], bit: u32) {
    let index = (bit / 8) as usize;
    bitmap[index] |= 1 << (bit % 8);
}

fn nonzero(pointer: u32) -> Result<u32, Status> {
    if pointer == 0 {
        Err(Status::Invalid)
    } else {
        Ok(pointer)
    }
}

/// One inode as far as the filesystem needs it.
#[derive(Clone, Copy)]
pub struct Inode {
    mode: u16,
    size: u64,
    blocks: [u32; INODE_WORDS],
    /// `i_blocks`, the allocation in 512-byte units (including indirect
    /// blocks). The reader ignores it; the writer keeps it current because
    /// `e2fsck` checks it.
    blocks512: u32,
    /// `i_links_count`. Regular files keep one link; a directory has two (its
    /// own `.` plus its parent's entry) and one more per subdirectory.
    links: u16,
}

impl Inode {
    /// The inode's byte size.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// True for a regular file.
    pub fn is_file(&self) -> bool {
        self.mode & S_IFMT == S_IFREG
    }

    /// True for a directory.
    pub fn is_dir(&self) -> bool {
        self.mode & S_IFMT == S_IFDIR
    }
}

/// One directory entry as the file API reports it (spec 0013).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Used bytes of the name the caller asked for.
    pub name_len: usize,
    /// `abi::fs::KIND_FILE`, `KIND_DIR` or `KIND_OTHER`.
    pub kind: u64,
    /// The entry's inode size in bytes.
    pub size: u64,
}

fn entry_kind(mode: u16) -> u64 {
    match mode & S_IFMT {
        S_IFREG => abi::fs::KIND_FILE,
        S_IFDIR => abi::fs::KIND_DIR,
        _ => abi::fs::KIND_OTHER,
    }
}

#[derive(Clone, Copy)]
struct GroupDescriptor {
    block_bitmap: u32,
    inode_bitmap: u32,
    inode_table: u32,
    free_blocks: u32,
    free_inodes: u32,
    used_dirs: u32,
}

#[derive(Clone, Copy)]
struct CacheEntry {
    block: u32,
    data: [u8; BLOCK_SIZE],
}

struct Cache {
    entries: [Option<CacheEntry>; CACHE_BLOCKS],
    next: usize,
}

impl Cache {
    const fn new() -> Self {
        Cache {
            entries: [None; CACHE_BLOCKS],
            next: 0,
        }
    }

    fn find(&self, block: u32) -> Option<&CacheEntry> {
        self.entries
            .iter()
            .flatten()
            .find(|entry| entry.block == block)
    }

    fn insert(&mut self, block: u32, data: [u8; BLOCK_SIZE]) {
        // Replace the block's own entry when it is cached, so a write never
        // leaves a stale copy that `find` could return first.
        if let Some(slot) = self
            .entries
            .iter()
            .position(|entry| entry.is_some_and(|entry| entry.block == block))
        {
            self.entries[slot] = Some(CacheEntry { block, data });
            return;
        }
        let slot = match self.entries.iter().position(|entry| entry.is_none()) {
            Some(slot) => slot,
            None => {
                let slot = self.next % CACHE_BLOCKS;
                self.next = (self.next + 1) % CACHE_BLOCKS;
                slot
            }
        };
        self.entries[slot] = Some(CacheEntry { block, data });
    }
}

/// A mounted ext2 filesystem.
pub struct Fs<D: SectorDevice> {
    device: D,
    cache: RefCell<Cache>,
    inodes_count: u32,
    blocks_count: u32,
    inodes_per_group: u32,
    blocks_per_group: u32,
    first_data_block: u32,
    inode_size: usize,
    group_count: u32,
    descriptor_table_block: u32,
    has_filetype: bool,
    /// The whole superblock record, kept so a write can patch the free counts,
    /// `s_state` and `s_errors` and write the other fields back unchanged.
    superblock: [u8; SUPERBLOCK_BYTES],
    /// True only for a [`mount_write`] mount; every mutating call refuses a
    /// read-only mount instead of writing through it.
    writable: bool,
}

/// Reads one 1 KiB block worth of sectors starting at `first_sector`.
fn read_sectors<D: SectorDevice>(
    device: &D,
    first_sector: u64,
    out: &mut [u8; BLOCK_SIZE],
) -> Result<(), Status> {
    let (first, second) = out.split_at_mut(SECTOR_SIZE);
    let first: &mut [u8; SECTOR_SIZE] = first.try_into().map_err(|_| Status::Invalid)?;
    let second: &mut [u8; SECTOR_SIZE] = second.try_into().map_err(|_| Status::Invalid)?;
    device.read_sector(first_sector, first)?;
    device.read_sector(first_sector + 1, second)?;
    Ok(())
}

/// Validates and mounts an ext2 filesystem read-only (the M6.1 subset).
///
/// This is the free-function form of [`Fs::mount`], so a caller names only the
/// device type.
pub fn mount<D: SectorDevice>(device: D) -> Result<Fs<D>, Status> {
    Fs::mount(device)
}

/// Validates and mounts an ext2 filesystem for writing (spec 0019, H5).
///
/// The mount performs the same validation as [`Fs::mount`], reads every
/// group's bitmaps and free counts, then marks the filesystem clean
/// (`s_state = 1`, `s_errors = 0`) and writes the superblock once. The mount
/// count is deliberately not incremented: the home disk is mounted on every
/// boot and a growing count would make `e2fsck` check it after a while.
pub fn mount_write<D: SectorDevice>(device: D) -> Result<Fs<D>, Status> {
    Fs::mount_write(device)
}

impl<D: SectorDevice> Fs<D> {
    /// Validates and mounts an ext2 filesystem, read-only.
    ///
    /// Only the M6 subset is accepted: 1 KiB blocks, revision 1, no
    /// incompatible feature beyond directory file types, and a group
    /// descriptor table, inode table and root directory that all lie inside
    /// the device. A bad structure refuses the mount; nothing is mounted
    /// partially.
    pub fn mount(device: D) -> Result<Self, Status> {
        Self::open(device, false)
    }

    /// Validates and mounts an ext2 filesystem for writing (spec 0019, H5).
    ///
    /// Besides the read-only validation this reads each group's bitmaps and
    /// free counts, marks the filesystem clean and writes the superblock once.
    /// A structure the writer could not safely extend refuses the mount, so a
    /// corrupt image is never written.
    pub fn mount_write(device: D) -> Result<Self, Status> {
        Self::open(device, true)
    }

    fn open(device: D, writable: bool) -> Result<Self, Status> {
        let sectors = device.sector_count();
        if sectors < 4 {
            return Err(Status::Invalid);
        }

        let mut superblock = [0u8; SUPERBLOCK_BYTES];
        read_sectors(
            &device,
            (SUPERBLOCK_OFFSET / SECTOR_SIZE) as u64,
            &mut superblock,
        )?;

        if read_u16(&superblock, 56)? != EXT2_MAGIC {
            return Err(Status::Invalid);
        }
        // Revision 1 (dynamic inodes); revision 0 has fixed 128-byte inodes and
        // is outside the supported subset.
        if read_u32(&superblock, 76)? != 1 {
            return Err(Status::Invalid);
        }

        // 1024 << log_block_size: this implementation supports 1 KiB blocks.
        if read_u32(&superblock, 24)? != 0 {
            return Err(Status::Invalid);
        }

        let inode_size = read_u16(&superblock, 88)? as usize;
        if inode_size < INODE_BYTES
            || inode_size > BLOCK_SIZE
            || !inode_size.is_power_of_two()
            || BLOCK_SIZE % inode_size != 0
        {
            return Err(Status::Invalid);
        }

        let inodes_count = read_u32(&superblock, 0)?;
        let blocks_count = read_u32(&superblock, 4)?;
        let free_blocks = read_u32(&superblock, SB_FREE_BLOCKS)?;
        let free_inodes = read_u32(&superblock, SB_FREE_INODES)?;
        let first_data_block = read_u32(&superblock, 20)?;
        let blocks_per_group = read_u32(&superblock, 32)?;
        let inodes_per_group = read_u32(&superblock, 40)?;
        let feature_incompat = read_u32(&superblock, 96)?;

        if inodes_count == 0 || blocks_count <= 1 || blocks_per_group == 0 || inodes_per_group == 0
        {
            return Err(Status::Invalid);
        }
        if first_data_block != 1 || blocks_count <= first_data_block {
            return Err(Status::Invalid);
        }
        // A free count above the total is a superblock the writer must not
        // trust (spec 0019, W4).
        if free_blocks > blocks_count || free_inodes > inodes_count {
            return Err(Status::Invalid);
        }
        if feature_incompat & !INCOMPAT_FILETYPE != 0 {
            return Err(Status::Invalid);
        }
        // A bitmap has exactly one bit per block or inode in its group.
        if blocks_per_group as usize > BLOCK_SIZE * 8 || inodes_per_group as usize > BLOCK_SIZE * 8
        {
            return Err(Status::Invalid);
        }

        // The device must hold every block the superblock claims.
        let device_bytes = sectors
            .checked_mul(SECTOR_SIZE as u64)
            .ok_or(Status::Invalid)?;
        let fs_bytes = (blocks_count as u64)
            .checked_mul(BLOCK_SIZE as u64)
            .ok_or(Status::Invalid)?;
        if fs_bytes > device_bytes {
            return Err(Status::Invalid);
        }

        let data_blocks = blocks_count - first_data_block;
        let group_count = data_blocks.div_ceil(blocks_per_group);
        if inodes_count as u64 > group_count as u64 * inodes_per_group as u64 {
            return Err(Status::Invalid);
        }
        let descriptor_table_block = first_data_block + 1;

        let descriptor_bytes = (group_count as usize)
            .checked_mul(GROUP_DESC_BYTES)
            .ok_or(Status::Invalid)?;
        let descriptor_blocks = descriptor_bytes.div_ceil(BLOCK_SIZE);
        if descriptor_table_block
            .checked_add(descriptor_blocks as u32)
            .is_none_or(|end| end > blocks_count)
        {
            return Err(Status::Invalid);
        }

        let mut fs = Fs {
            device,
            cache: RefCell::new(Cache::new()),
            inodes_count,
            blocks_count,
            inodes_per_group,
            blocks_per_group,
            first_data_block,
            inode_size,
            group_count,
            descriptor_table_block,
            has_filetype: feature_incompat & INCOMPAT_FILETYPE != 0,
            superblock,
            writable,
        };

        // Validate every group's bitmaps, inode-table extent and free counts.
        let inode_table_blocks = ((inodes_per_group as usize)
            .checked_mul(inode_size)
            .ok_or(Status::Invalid)?)
        .div_ceil(BLOCK_SIZE);
        let mut counted_blocks = 0u32;
        let mut counted_inodes = 0u32;
        for group in 0..group_count {
            let descriptor = fs.group_descriptor(group)?;
            for block in [
                descriptor.block_bitmap,
                descriptor.inode_bitmap,
                descriptor.inode_table,
            ] {
                if block < first_data_block || block >= blocks_count {
                    return Err(Status::Invalid);
                }
            }
            if (descriptor.inode_table as usize)
                .checked_add(inode_table_blocks)
                .is_none_or(|end| end > blocks_count as usize)
            {
                return Err(Status::Invalid);
            }
            if descriptor.free_blocks > fs.group_block_capacity(group)
                || descriptor.free_inodes > fs.group_inode_capacity(group)
            {
                return Err(Status::Invalid);
            }
            counted_blocks = counted_blocks.saturating_add(descriptor.free_blocks);
            counted_inodes = counted_inodes.saturating_add(descriptor.free_inodes);

            // A bitmap must be readable before the writer may set a bit in it.
            let mut bitmap = [0u8; BLOCK_SIZE];
            fs.block(descriptor.block_bitmap, &mut bitmap)?;
            fs.block(descriptor.inode_bitmap, &mut bitmap)?;
        }
        // The group free counts must agree with the superblock, or the writer
        // would hand out a block the superblock still counts as used.
        if counted_blocks != free_blocks || counted_inodes != free_inodes {
            return Err(Status::Invalid);
        }

        // The root directory must be a directory and every entry must parse.
        let root = fs.inode(ROOT_INODE)?;
        if !root.is_dir() {
            return Err(Status::Invalid);
        }
        fs.each_dir_entry(&root, &mut |_, _, _| Ok(true))?;

        if writable {
            fs.mark_clean()?;
        }
        Ok(fs)
    }

    /// Filesystem block size in bytes (always 1 KiB in this subset).
    pub fn block_size(&self) -> usize {
        BLOCK_SIZE
    }

    /// Size of the filesystem in bytes as claimed by the superblock.
    pub fn size_bytes(&self) -> u64 {
        self.blocks_count as u64 * BLOCK_SIZE as u64
    }

    /// Reads a whole file by path.
    ///
    /// Returns the byte count written. A file larger than `out` is
    /// [`Status::CapacityExceeded`]; a path that is not a regular file is
    /// [`Status::Invalid`]; a missing component is [`Status::NotFound`]; a
    /// malformed path is [`Status::Invalid`].
    pub fn read_file(&self, path: &str, out: &mut [u8]) -> Result<usize, Status> {
        let inode = self.regular_inode(path)?;
        if inode.size > out.len() as u64 {
            return Err(Status::CapacityExceeded);
        }

        let size = inode.size as usize;
        let mut written = 0usize;
        self.each_data_block(&inode, &mut |index, _valid| {
            let mut block = [0u8; BLOCK_SIZE];
            self.block(index, &mut block)?;
            let remaining = size - written;
            let count = block.len().min(remaining);
            out[written..written + count].copy_from_slice(&block[..count]);
            written += count;
            Ok(written < size)
        })?;
        if written != size {
            return Err(Status::Invalid);
        }
        Ok(written)
    }

    /// Reads up to `out.len()` bytes of a regular file at byte `offset`
    /// (spec 0013, `core.fs_read`).
    ///
    /// An offset at or past the file's end returns `Ok(0)`, so end-of-file is
    /// not an error. A missing component is [`Status::NotFound`] and a
    /// non-regular entry is [`Status::Invalid`].
    ///
    /// Only the logical blocks the range touches are resolved (spec 0025 B19),
    /// so the cost does not grow with `offset`; a hole inside the range is
    /// [`Status::Invalid`] and a file that needs the triple-indirect pointer is
    /// [`Status::IoError`], as for the sequential iterator.
    pub fn read_at(&self, path: &str, offset: u64, out: &mut [u8]) -> Result<usize, Status> {
        let inode = self.regular_inode(path)?;
        if offset >= inode.size || out.is_empty() {
            return Ok(0);
        }

        let first = (offset / BLOCK_SIZE as u64) as u32;
        let last_byte = offset
            .saturating_add(out.len() as u64 - 1)
            .min(inode.size - 1);
        let last = (last_byte / BLOCK_SIZE as u64) as u32;

        let mut written = 0usize;
        let mut logical = first;
        while logical <= last {
            let block_number = self.block_at(&inode, logical)?;
            let start = u64::from(logical) * BLOCK_SIZE as u64;
            let from = offset.saturating_sub(start) as usize;
            let end = (start + BLOCK_SIZE as u64).min(inode.size);
            let to = (end - start) as usize;
            let count = (to - from).min(out.len() - written);
            if count > 0 {
                let mut block = [0u8; BLOCK_SIZE];
                self.block(block_number, &mut block)?;
                out[written..written + count].copy_from_slice(&block[from..from + count]);
                written += count;
            }
            logical += 1;
        }
        Ok(written)
    }

    /// Returns the physical block that holds logical block `index` of `inode`,
    /// reading only the pointer blocks on the way (spec 0025 B19).
    ///
    /// Direct pointers are read from the inode, single-indirect from
    /// `blocks[12]` and double-indirect from `blocks[13]` → inner block. A zero
    /// pointer is [`Status::Invalid`] (the subset has no holes). An index at or
    /// past `ceil(size / BLOCK_SIZE)` is [`Status::IoError`], as is a file that
    /// needs the triple-indirect pointer.
    fn block_at(&self, inode: &Inode, index: u32) -> Result<u32, Status> {
        let total = inode.size.div_ceil(BLOCK_SIZE as u64);
        if total > self.max_double_indirect_blocks() as u64 || inode.blocks[14] != 0 {
            // The file needs the triple-indirect pointer, which the subset does
            // not support.
            return Err(Status::IoError);
        }
        if u64::from(index) >= total {
            return Err(Status::IoError);
        }

        let per_block = (BLOCK_SIZE / 4) as u32;
        if index < 12 {
            return nonzero(inode.blocks[index as usize]);
        }
        if index < 12 + per_block {
            let indirect = nonzero(inode.blocks[12])?;
            let mut pointers = [0u8; BLOCK_SIZE];
            self.block(indirect, &mut pointers)?;
            return nonzero(read_u32(&pointers, (index - 12) as usize * 4)?);
        }

        let index = index - 12 - per_block;
        let outer_index = (index / per_block) as usize;
        let inner_index = (index % per_block) as usize;
        let indirect = nonzero(inode.blocks[13])?;
        let mut outer = [0u8; BLOCK_SIZE];
        self.block(indirect, &mut outer)?;
        let inner_number = nonzero(read_u32(&outer, outer_index * 4)?)?;
        let mut inner = [0u8; BLOCK_SIZE];
        self.block(inner_number, &mut inner)?;
        nonzero(read_u32(&inner, inner_index * 4)?)
    }

    /// Reads one entry of the directory at `path` by 0-based index (spec 0013,
    /// `core.fs_list`).
    ///
    /// `""` and `"."` name the filesystem root. `.` and `..` are excluded and
    /// entries are visited in directory order. The name is copied into
    /// `name`; a name longer than the destination is
    /// [`Status::CapacityExceeded`]. An index at or past the last entry is
    /// [`Status::NotFound`]; a path that is not a directory is
    /// [`Status::Invalid`].
    pub fn list(&self, path: &str, index: u32, name: &mut [u8]) -> Result<Entry, Status> {
        let (_, directory) = self.directory(path)?;

        let mut seen = 0u32;
        let mut found = None;
        let mut error = None;
        self.each_dir_entry(&directory, &mut |number, _, entry| {
            if found.is_some() || error.is_some() {
                return Ok(false);
            }
            if entry == b"." || entry == b".." {
                return Ok(true);
            }
            if seen != index {
                seen += 1;
                return Ok(true);
            }
            match self.inode(number) {
                Ok(inode) => {
                    if entry.len() > name.len() {
                        error = Some(Status::CapacityExceeded);
                    } else {
                        name[..entry.len()].copy_from_slice(entry);
                        found = Some(Entry {
                            name_len: entry.len(),
                            kind: entry_kind(inode.mode),
                            size: inode.size,
                        });
                    }
                }
                Err(status) => error = Some(status),
            }
            Ok(false)
        })?;

        match (found, error) {
            (_, Some(status)) => Err(status),
            (Some(entry), None) => Ok(entry),
            (None, None) => Err(Status::NotFound),
        }
    }

    /// Size in bytes of a regular file.
    pub fn file_size(&self, path: &str) -> Result<u64, Status> {
        Ok(self.regular_inode(path)?.size)
    }

    /// Finds a path, returning its inode number and inode.
    ///
    /// The root is `""` or `"."`. Every other path is split on `/` and walked
    /// one directory at a time; the walker refuses an empty, `.` or `..`
    /// component, a component longer than [`abi::fs::NAME_CAP`], more than
    /// [`abi::fs::MAX_COMPONENTS`] components, and a component that resolves
    /// through a non-directory. Symbolic links are never followed.
    pub fn find(&self, path: &str) -> Result<(u32, Inode), Status> {
        if path.is_empty() || path == "." {
            let inode = self.inode(ROOT_INODE)?;
            if !inode.is_dir() {
                return Err(Status::Invalid);
            }
            return Ok((ROOT_INODE, inode));
        }
        check_path(path)?;

        let mut number = ROOT_INODE;
        let mut inode = self.inode(number)?;
        for part in path.split('/') {
            if !inode.is_dir() {
                return Err(Status::Invalid);
            }
            number = self
                .entry_inode(&inode, part.as_bytes())?
                .ok_or(Status::NotFound)?;
            inode = self.inode(number)?;
        }
        Ok((number, inode))
    }

    fn regular_inode(&self, path: &str) -> Result<Inode, Status> {
        let (_, inode) = self.find(path)?;
        if !inode.is_file() {
            return Err(Status::Invalid);
        }
        Ok(inode)
    }

    fn directory(&self, path: &str) -> Result<(u32, Inode), Status> {
        let (number, inode) = self.find(path)?;
        if !inode.is_dir() {
            return Err(Status::Invalid);
        }
        Ok((number, inode))
    }

    /// Finds one named entry inside `directory`, if present.
    fn entry_inode(&self, directory: &Inode, name: &[u8]) -> Result<Option<u32>, Status> {
        let mut found = None;
        self.each_dir_entry(directory, &mut |number, _, entry| {
            if entry == name {
                found = Some(number);
                Ok(false)
            } else {
                Ok(true)
            }
        })?;
        Ok(found)
    }

    /// Reads an inode by number.
    fn inode(&self, number: u32) -> Result<Inode, Status> {
        let (block_number, start) = self.inode_location(number)?;
        let mut block = [0u8; BLOCK_SIZE];
        self.block(block_number, &mut block)?;
        let end = start.checked_add(self.inode_size).ok_or(Status::Invalid)?;
        let inode = block.get(start..end).ok_or(Status::Invalid)?;

        let mode = read_u16(inode, 0)?;
        let size_lo = read_u32(inode, 4)? as u64;
        // `i_size_high` lives at byte 108, inside the mandatory 128-byte area.
        let size_hi = read_u32(inode, INODE_SIZE_HIGH)? as u64;

        let mut blocks = [0u32; INODE_WORDS];
        for (index, word) in blocks.iter_mut().enumerate() {
            *word = read_u32(inode, INODE_BLOCK_WORDS + index * 4)?;
        }

        Ok(Inode {
            mode,
            size: size_lo | (size_hi << 32),
            blocks,
            blocks512: read_u32(inode, INODE_BLOCKS512)?,
            links: read_u16(inode, INODE_LINKS)?,
        })
    }

    /// The block and byte offset of an inode inside its group's inode table.
    fn inode_location(&self, number: u32) -> Result<(u32, usize), Status> {
        if number == 0 || number > self.inodes_count {
            return Err(Status::Invalid);
        }
        let group = (number - 1) / self.inodes_per_group;
        let index = (number - 1) % self.inodes_per_group;
        let descriptor = self.group_descriptor(group)?;

        let offset = (index as usize)
            .checked_mul(self.inode_size)
            .ok_or(Status::Invalid)?;
        let block_number = descriptor
            .inode_table
            .checked_add((offset / BLOCK_SIZE) as u32)
            .ok_or(Status::Invalid)?;
        Ok((block_number, offset % BLOCK_SIZE))
    }

    fn group_descriptor(&self, group: u32) -> Result<GroupDescriptor, Status> {
        if group >= self.group_count {
            return Err(Status::Invalid);
        }
        let offset = (group as usize)
            .checked_mul(GROUP_DESC_BYTES)
            .ok_or(Status::Invalid)?;
        let block_number = self
            .descriptor_table_block
            .checked_add((offset / BLOCK_SIZE) as u32)
            .ok_or(Status::Invalid)?;
        let mut block = [0u8; BLOCK_SIZE];
        self.block(block_number, &mut block)?;
        let start = offset % BLOCK_SIZE;
        let descriptor = block
            .get(start..start + GROUP_DESC_BYTES)
            .ok_or(Status::Invalid)?;
        Ok(GroupDescriptor {
            block_bitmap: read_u32(descriptor, 0)?,
            inode_bitmap: read_u32(descriptor, 4)?,
            inode_table: read_u32(descriptor, 8)?,
            free_blocks: read_u16(descriptor, GD_FREE_BLOCKS)? as u32,
            free_inodes: read_u16(descriptor, GD_FREE_INODES)? as u32,
            used_dirs: read_u16(descriptor, GD_USED_DIRS)? as u32,
        })
    }

    /// The block and byte offset of a group descriptor.
    fn group_descriptor_location(&self, group: u32) -> Result<(u32, usize), Status> {
        if group >= self.group_count {
            return Err(Status::Invalid);
        }
        let offset = (group as usize)
            .checked_mul(GROUP_DESC_BYTES)
            .ok_or(Status::Invalid)?;
        let block_number = self
            .descriptor_table_block
            .checked_add((offset / BLOCK_SIZE) as u32)
            .ok_or(Status::Invalid)?;
        Ok((block_number, offset % BLOCK_SIZE))
    }

    /// Reads one filesystem block into `out`, through the fixed cache.
    fn block(&self, index: u32, out: &mut [u8; BLOCK_SIZE]) -> Result<(), Status> {
        if index >= self.blocks_count {
            return Err(Status::IoError);
        }
        {
            let cache = self.cache.borrow();
            if let Some(entry) = cache.find(index) {
                out.copy_from_slice(&entry.data);
                return Ok(());
            }
        }

        let first_sector = u64::from(index) * SECTORS_PER_BLOCK as u64;
        let mut data = [0u8; BLOCK_SIZE];
        read_sectors(&self.device, first_sector, &mut data)?;
        self.cache.borrow_mut().insert(index, data);
        out.copy_from_slice(&data);
        Ok(())
    }

    // ---- the writer (spec 0019, W4/W7) --------------------------------

    /// Writes one filesystem block through the cache and the device.
    ///
    /// The device write happens first, then the cache is updated, so a block
    /// read afterwards sees what the disk holds. This is the only write path:
    /// nothing is deferred (H4).
    fn write_block(&mut self, index: u32, data: &[u8; BLOCK_SIZE]) -> Result<(), Status> {
        if index >= self.blocks_count {
            return Err(Status::IoError);
        }
        let first_sector = u64::from(index) * SECTORS_PER_BLOCK as u64;
        let first: &[u8; SECTOR_SIZE] = data[..SECTOR_SIZE]
            .try_into()
            .map_err(|_| Status::Invalid)?;
        let second: &[u8; SECTOR_SIZE] = data[SECTOR_SIZE..]
            .try_into()
            .map_err(|_| Status::Invalid)?;
        self.device.write_sector(first_sector, first)?;
        self.device.write_sector(first_sector + 1, second)?;
        self.cache.borrow_mut().insert(index, *data);
        Ok(())
    }

    /// Number of blocks the group's bitmap covers (the last group is short).
    fn group_block_capacity(&self, group: u32) -> u32 {
        let first = group.saturating_mul(self.blocks_per_group);
        let data_blocks = self.blocks_count - self.first_data_block;
        data_blocks.saturating_sub(first).min(self.blocks_per_group)
    }

    /// Number of inodes the group's bitmap covers (the last group is short).
    fn group_inode_capacity(&self, group: u32) -> u32 {
        let first = group.saturating_mul(self.inodes_per_group);
        self.inodes_count
            .saturating_sub(first)
            .min(self.inodes_per_group)
    }

    fn superblock_u32(&self, offset: usize) -> u32 {
        u32::from_le_bytes([
            self.superblock[offset],
            self.superblock[offset + 1],
            self.superblock[offset + 2],
            self.superblock[offset + 3],
        ])
    }

    /// Free blocks as the superblock counts them.
    pub fn free_blocks(&self) -> u32 {
        self.superblock_u32(SB_FREE_BLOCKS)
    }

    /// Free inodes as the superblock counts them.
    pub fn free_inodes(&self) -> u32 {
        self.superblock_u32(SB_FREE_INODES)
    }

    fn set_superblock_u32(&mut self, offset: usize, value: u32) -> Result<(), Status> {
        put_u32(&mut self.superblock, offset, value)
    }

    /// Writes the in-memory superblock record back to block 1.
    fn write_superblock(&mut self) -> Result<(), Status> {
        let data = self.superblock;
        self.write_block(self.first_data_block, &data)
    }

    /// Marks the filesystem clean and writes the superblock once (H5).
    fn mark_clean(&mut self) -> Result<(), Status> {
        put_u16(&mut self.superblock, SB_STATE, SUPERBLOCK_VALID_FS)?;
        put_u16(&mut self.superblock, SB_ERRORS, 0)?;
        // The mount count and check time are left exactly as found.
        self.write_superblock()
    }

    fn set_superblock_free_blocks(&mut self, value: u32) -> Result<(), Status> {
        self.set_superblock_u32(SB_FREE_BLOCKS, value)?;
        self.write_superblock()
    }

    fn set_superblock_free_inodes(&mut self, value: u32) -> Result<(), Status> {
        self.set_superblock_u32(SB_FREE_INODES, value)?;
        self.write_superblock()
    }

    fn write_group_free(&mut self, group: u32, field: usize, value: u32) -> Result<(), Status> {
        let value = u16::try_from(value).map_err(|_| Status::IoError)?;
        let (block_number, start) = self.group_descriptor_location(group)?;
        let mut block = [0u8; BLOCK_SIZE];
        self.block(block_number, &mut block)?;
        put_u16(&mut block, start + field, value)?;
        self.write_block(block_number, &block)
    }

    /// Adjusts a group's used-directory count (`bg_used_dirs_count`).
    fn adjust_used_dirs(&mut self, group: u32, add: bool) -> Result<(), Status> {
        let descriptor = self.group_descriptor(group)?;
        let value = if add {
            descriptor.used_dirs.saturating_add(1)
        } else {
            descriptor.used_dirs.saturating_sub(1)
        };
        self.write_group_free(group, GD_USED_DIRS, value)
    }

    /// Allocates one data block: sets its bitmap bit, then the group free
    /// count, then the superblock free count (W4).
    fn alloc_block(&mut self) -> Result<u32, Status> {
        if !self.writable {
            return Err(Status::Invalid);
        }
        for group in 0..self.group_count {
            if self.group_block_capacity(group) == 0 {
                continue;
            }
            let descriptor = self.group_descriptor(group)?;
            let mut bitmap = [0u8; BLOCK_SIZE];
            self.block(descriptor.block_bitmap, &mut bitmap)?;
            let Some(bit) = first_free_bit(&bitmap, self.group_block_capacity(group)) else {
                continue;
            };
            if descriptor.free_blocks == 0 || self.free_blocks() == 0 {
                // A free bit the counts do not know about is not safe to use.
                return Err(Status::IoError);
            }
            set_bit(&mut bitmap, bit);
            self.write_block(descriptor.block_bitmap, &bitmap)?;
            self.write_group_free(group, GD_FREE_BLOCKS, descriptor.free_blocks - 1)?;
            self.set_superblock_free_blocks(self.free_blocks() - 1)?;
            let number = self
                .first_data_block
                .checked_add(group.saturating_mul(self.blocks_per_group))
                .and_then(|base| base.checked_add(bit))
                .ok_or(Status::IoError)?;
            if number >= self.blocks_count {
                return Err(Status::IoError);
            }
            return Ok(number);
        }
        if self.free_blocks() == 0 {
            Err(Status::CapacityExceeded)
        } else {
            // The superblock claims free blocks the bitmaps do not have.
            Err(Status::IoError)
        }
    }

    /// Allocates one inode number the same way [`Self::alloc_block`] allocates
    /// a block.
    fn alloc_inode(&mut self) -> Result<u32, Status> {
        if !self.writable {
            return Err(Status::Invalid);
        }
        for group in 0..self.group_count {
            if self.group_inode_capacity(group) == 0 {
                continue;
            }
            let descriptor = self.group_descriptor(group)?;
            let mut bitmap = [0u8; BLOCK_SIZE];
            self.block(descriptor.inode_bitmap, &mut bitmap)?;
            let Some(bit) = first_free_bit(&bitmap, self.group_inode_capacity(group)) else {
                continue;
            };
            if descriptor.free_inodes == 0 || self.free_inodes() == 0 {
                return Err(Status::IoError);
            }
            set_bit(&mut bitmap, bit);
            self.write_block(descriptor.inode_bitmap, &bitmap)?;
            self.write_group_free(group, GD_FREE_INODES, descriptor.free_inodes - 1)?;
            self.set_superblock_free_inodes(self.free_inodes() - 1)?;
            let number = group
                .saturating_mul(self.inodes_per_group)
                .checked_add(bit)
                .and_then(|index| index.checked_add(1))
                .ok_or(Status::IoError)?;
            if number > self.inodes_count {
                return Err(Status::IoError);
            }
            return Ok(number);
        }
        if self.free_inodes() == 0 {
            Err(Status::CapacityExceeded)
        } else {
            Err(Status::IoError)
        }
    }

    /// Initializes a fresh regular-file inode: mode `S_IFREG | 0644`, one link,
    /// size zero, every block pointer and time zero.
    fn init_regular_inode(&mut self, number: u32) -> Result<(), Status> {
        let (block_number, start) = self.inode_location(number)?;
        let mut block = [0u8; BLOCK_SIZE];
        self.block(block_number, &mut block)?;
        let end = start.checked_add(self.inode_size).ok_or(Status::Invalid)?;
        let bytes = block.get_mut(start..end).ok_or(Status::Invalid)?;
        bytes.fill(0);
        put_u16(bytes, 0, REGULAR_FILE_MODE)?;
        put_u16(bytes, INODE_LINKS, 1)?;
        self.write_block(block_number, &block)
    }

    /// Writes an inode's size, 512-byte block count and pointers back, leaving
    /// the fields this slice does not model (uid, times, flags) untouched.
    fn store_inode(&mut self, number: u32, inode: &Inode) -> Result<(), Status> {
        let (block_number, start) = self.inode_location(number)?;
        let mut block = [0u8; BLOCK_SIZE];
        self.block(block_number, &mut block)?;
        let end = start.checked_add(INODE_BYTES).ok_or(Status::Invalid)?;
        let bytes = block.get_mut(start..end).ok_or(Status::Invalid)?;
        put_u32(bytes, 4, inode.size as u32)?;
        put_u32(bytes, INODE_SIZE_HIGH, (inode.size >> 32) as u32)?;
        put_u16(bytes, INODE_LINKS, inode.links)?;
        put_u32(bytes, INODE_BLOCKS512, inode.blocks512)?;
        for (index, pointer) in inode.blocks.iter().enumerate() {
            put_u32(bytes, INODE_BLOCK_WORDS + index * 4, *pointer)?;
        }
        self.write_block(block_number, &block)
    }

    /// The data-block pointer for a logical block, without allocating.
    fn pointer(&self, inode: &Inode, logical: u32) -> Result<Option<u32>, Status> {
        if logical as usize >= self.max_double_indirect_blocks() {
            return Err(Status::CapacityExceeded);
        }
        let per_block = (BLOCK_SIZE / 4) as u32;
        if logical < 12 {
            let pointer = inode.blocks[logical as usize];
            return Ok(if pointer == 0 { None } else { Some(pointer) });
        }
        if logical < 12 + per_block {
            if inode.blocks[12] == 0 {
                return Ok(None);
            }
            let mut indirect = [0u8; BLOCK_SIZE];
            self.block(inode.blocks[12], &mut indirect)?;
            let pointer = read_u32(&indirect, (logical - 12) as usize * 4)?;
            return Ok(if pointer == 0 { None } else { Some(pointer) });
        }
        if inode.blocks[13] == 0 {
            return Ok(None);
        }
        let index = logical - 12 - per_block;
        let outer_index = (index / per_block) as usize;
        let inner_index = (index % per_block) as usize;
        let mut outer = [0u8; BLOCK_SIZE];
        self.block(inode.blocks[13], &mut outer)?;
        let inner = read_u32(&outer, outer_index * 4)?;
        if inner == 0 {
            return Ok(None);
        }
        let mut block = [0u8; BLOCK_SIZE];
        self.block(inner, &mut block)?;
        let pointer = read_u32(&block, inner_index * 4)?;
        Ok(if pointer == 0 { None } else { Some(pointer) })
    }

    /// Counts the blocks and indirect blocks a write would have to allocate.
    ///
    /// The capacity check runs this before the first mutation, so a full
    /// filesystem refuses the write with the tree unchanged (W4).
    fn plan_blocks(&self, inode: &Inode, first: u32, last: u32) -> Result<u32, Status> {
        if last < first {
            return Ok(0);
        }
        let per_block = (BLOCK_SIZE / 4) as u32;
        let mut needed = 0u32;
        for logical in first..=last {
            if self.pointer(inode, logical)?.is_none() {
                needed = needed.saturating_add(1);
            }
        }
        // The single-indirect block itself.
        if first < 12 + per_block && last >= 12 && inode.blocks[12] == 0 {
            needed = needed.saturating_add(1);
        }
        // The double-indirect block and one block per double-indirect index.
        if last >= 12 + per_block {
            let start = first.max(12 + per_block) - 12 - per_block;
            let end = last - 12 - per_block;
            let outer_start = start / per_block;
            let outer_end = end / per_block;
            if inode.blocks[13] == 0 {
                needed = needed.saturating_add(1 + (outer_end - outer_start + 1));
            } else {
                let mut outer = [0u8; BLOCK_SIZE];
                self.block(inode.blocks[13], &mut outer)?;
                for outer_index in outer_start..=outer_end {
                    if read_u32(&outer, outer_index as usize * 4)? == 0 {
                        needed = needed.saturating_add(1);
                    }
                }
            }
        }
        Ok(needed)
    }

    /// Returns the data block for a logical block, allocating it (and any
    /// indirect block on the way) when the pointer is still zero.
    ///
    /// The second half of the result is true when the data block is new, so the
    /// caller starts from a zeroed buffer instead of reading undefined content.
    fn data_block_for(&mut self, inode: &mut Inode, logical: u32) -> Result<(u32, bool), Status> {
        if logical as usize >= self.max_double_indirect_blocks() {
            return Err(Status::CapacityExceeded);
        }
        let per_block = (BLOCK_SIZE / 4) as u32;
        if logical < 12 {
            let pointer = inode.blocks[logical as usize];
            if pointer != 0 {
                return Ok((pointer, false));
            }
            let block = self.alloc_block()?;
            inode.blocks[logical as usize] = block;
            inode.blocks512 = inode.blocks512.saturating_add(2);
            return Ok((block, true));
        }
        if logical < 12 + per_block {
            let index = (logical - 12) as usize;
            let mut indirect = [0u8; BLOCK_SIZE];
            if inode.blocks[12] == 0 {
                let block = self.alloc_block()?;
                inode.blocks[12] = block;
                inode.blocks512 = inode.blocks512.saturating_add(2);
            } else {
                self.block(inode.blocks[12], &mut indirect)?;
            }
            let pointer = read_u32(&indirect, index * 4)?;
            if pointer != 0 {
                return Ok((pointer, false));
            }
            let block = self.alloc_block()?;
            put_u32(&mut indirect, index * 4, block)?;
            self.write_block(inode.blocks[12], &indirect)?;
            inode.blocks512 = inode.blocks512.saturating_add(2);
            return Ok((block, true));
        }

        let index = logical - 12 - per_block;
        let outer_index = (index / per_block) as usize;
        let inner_index = (index % per_block) as usize;
        let mut outer = [0u8; BLOCK_SIZE];
        if inode.blocks[13] == 0 {
            let block = self.alloc_block()?;
            inode.blocks[13] = block;
            inode.blocks512 = inode.blocks512.saturating_add(2);
        } else {
            self.block(inode.blocks[13], &mut outer)?;
        }
        let mut inner_number = read_u32(&outer, outer_index * 4)?;
        let mut inner = [0u8; BLOCK_SIZE];
        if inner_number == 0 {
            inner_number = self.alloc_block()?;
            put_u32(&mut outer, outer_index * 4, inner_number)?;
            self.write_block(inode.blocks[13], &outer)?;
            inode.blocks512 = inode.blocks512.saturating_add(2);
        } else {
            self.block(inner_number, &mut inner)?;
        }
        let pointer = read_u32(&inner, inner_index * 4)?;
        if pointer != 0 {
            return Ok((pointer, false));
        }
        let block = self.alloc_block()?;
        put_u32(&mut inner, inner_index * 4, block)?;
        self.write_block(inner_number, &inner)?;
        inode.blocks512 = inode.blocks512.saturating_add(2);
        Ok((block, true))
    }

    /// The largest byte size a file can reach without the triple-indirect
    /// pointer (W2).
    fn max_file_bytes(&self) -> u64 {
        self.max_double_indirect_blocks() as u64 * BLOCK_SIZE as u64
    }

    // ---- create and write (spec 0019, W3/W4/W7) -----------------------

    /// Creates a regular file (`S_IFREG | 0644`, one link, empty).
    ///
    /// `path` names the new file inside a directory that must already exist; a
    /// name that is already present is [`Status::Invalid`]. The inode is
    /// allocated and initialized before the directory entry that names it, and
    /// the entry goes into existing free space when possible and a newly
    /// allocated directory block otherwise. Capacity is checked before the
    /// first allocation, so a full filesystem is [`Status::CapacityExceeded`]
    /// with no change.
    pub fn create(&mut self, path: &str) -> Result<u32, Status> {
        if !self.writable {
            return Err(Status::Invalid);
        }
        let (parent_path, name) = split_parent(path)?;
        let (parent_number, parent) = self.directory(parent_path)?;
        let name = name.as_bytes();
        if self.entry_inode(&parent, name)?.is_some() {
            return Err(Status::Invalid);
        }

        let needed = align4(8 + name.len());
        let slot = self.find_dir_slot(&parent, needed)?;
        if self.free_inodes() == 0 {
            return Err(Status::CapacityExceeded);
        }
        if slot.is_none() {
            let logical = (parent.size.div_ceil(BLOCK_SIZE as u64)) as u32;
            if self.plan_blocks(&parent, logical, logical)? > self.free_blocks() {
                return Err(Status::CapacityExceeded);
            }
        }

        let number = self.alloc_inode()?;
        self.init_regular_inode(number)?;
        let mut parent = parent;
        self.add_entry(parent_number, &mut parent, name, number, EXT2_FT_REG_FILE)?;
        Ok(number)
    }

    /// Writes `data` at byte `offset` of an existing regular file.
    ///
    /// The bytes outside the written range are preserved, a partial first or
    /// last block is read-modify-written, the file grows when the write reaches
    /// past its end, and a gap between the old end and `offset` is zero-filled
    /// (W7). Nothing shrinks: this slice has no truncate. An empty `data` is an
    /// `Ok(0)` no-op. A write past the double-indirect reach, or one that needs
    /// more blocks than are free, is [`Status::CapacityExceeded`] before any
    /// change.
    pub fn write_at(&mut self, path: &str, offset: u64, data: &[u8]) -> Result<usize, Status> {
        if !self.writable {
            return Err(Status::Invalid);
        }
        let (inode_number, mut inode) = self.find(path)?;
        if !inode.is_file() {
            return Err(Status::Invalid);
        }
        if data.is_empty() {
            return Ok(0);
        }
        let end = offset
            .checked_add(data.len() as u64)
            .ok_or(Status::CapacityExceeded)?;
        if end > self.max_file_bytes() {
            return Err(Status::CapacityExceeded);
        }
        let first = (offset / BLOCK_SIZE as u64) as u32;
        let last = ((end - 1) / BLOCK_SIZE as u64) as u32;
        // A gap between the old end and `offset` is materialized with zeroed
        // blocks: this subset has no holes, so every block before the write
        // must exist too.
        let gap_first = (inode.size / BLOCK_SIZE as u64) as u32;
        let plan_first = if offset > inode.size {
            gap_first
        } else {
            first
        };
        if self.plan_blocks(&inode, plan_first, last)? > self.free_blocks() {
            return Err(Status::CapacityExceeded);
        }

        let old_size = inode.size;
        if offset > old_size {
            let gap_end = ((offset - 1) / BLOCK_SIZE as u64) as u32;
            let mut logical = gap_first;
            while logical <= gap_end && logical < first {
                let (block_number, fresh) = self.data_block_for(&mut inode, logical)?;
                let mut block = [0u8; BLOCK_SIZE];
                if !fresh {
                    self.block(block_number, &mut block)?;
                    let zero_from =
                        old_size.saturating_sub(u64::from(logical) * BLOCK_SIZE as u64) as usize;
                    block[zero_from.min(BLOCK_SIZE)..].fill(0);
                }
                self.write_block(block_number, &block)?;
                logical += 1;
            }
        }

        let mut consumed = 0usize;
        let mut logical = first;
        while consumed < data.len() {
            let position = offset + consumed as u64;
            let block_start = u64::from(logical) * BLOCK_SIZE as u64;
            let block_offset = (position - block_start) as usize;
            let count = (BLOCK_SIZE - block_offset).min(data.len() - consumed);
            let (block_number, fresh) = self.data_block_for(&mut inode, logical)?;
            let mut block = [0u8; BLOCK_SIZE];
            if !fresh {
                self.block(block_number, &mut block)?;
            }
            // A write past the old end zero-fills the gap, including the
            // remaining bytes of the block that held the old end.
            if old_size < position {
                let zero_from = old_size.saturating_sub(block_start) as usize;
                let zero_to = block_offset.min(BLOCK_SIZE);
                if zero_from < zero_to {
                    block[zero_from..zero_to].fill(0);
                }
            }
            block[block_offset..block_offset + count]
                .copy_from_slice(&data[consumed..consumed + count]);
            self.write_block(block_number, &block)?;
            consumed += count;
            logical += 1;
        }
        if end > inode.size {
            inode.size = end;
        }
        self.store_inode(inode_number, &inode)?;
        Ok(data.len())
    }

    /// Number of data blocks a file of `size` bytes occupies.
    fn blocks_for(size: u64) -> u32 {
        size.div_ceil(BLOCK_SIZE as u64) as u32
    }

    /// `i_blocks` (512-byte units) for a hole-free file of `blocks` data
    /// blocks: every data block plus every single/double indirect block.
    fn blocks512_for(blocks: u32) -> u32 {
        let per_block = (BLOCK_SIZE / 4) as u32;
        let mut count = blocks;
        if blocks > 12 {
            count = count.saturating_add(1);
        }
        if blocks > 12 + per_block {
            let extra = blocks - 12 - per_block;
            count = count
                .saturating_add(1)
                .saturating_add(extra.div_ceil(per_block));
        }
        count.saturating_mul(2)
    }

    /// Sets a regular file's length (spec 0019, A7; ADR 0041).
    ///
    /// Shrinking zeroes the tail of the last kept block, clears the inode's
    /// pointers at and past the new end (including empty indirect blocks),
    /// writes the inode with the new size, and only then releases the block
    /// bitmap bits and counters: a crash between the two leaks blocks, which
    /// `e2fsck` reclaims, while freeing first could hand out a block the inode
    /// still points at. Extending plans capacity first, then allocates and
    /// zero-fills every block up to `size`, including the partial first block's
    /// tail; a full filesystem is [`Status::CapacityExceeded`] with no change.
    /// The current size is an `Ok` no-op, a directory is [`Status::Invalid`]
    /// and a missing path is [`Status::NotFound`].
    pub fn truncate(&mut self, path: &str, size: u64) -> Result<(), Status> {
        if !self.writable {
            return Err(Status::Invalid);
        }
        let (number, inode) = self.find(path)?;
        if !inode.is_file() {
            return Err(Status::Invalid);
        }
        if size == inode.size {
            return Ok(());
        }
        if size > self.max_file_bytes() {
            return Err(Status::CapacityExceeded);
        }
        if size < inode.size {
            self.shrink_inode(number, inode, size)
        } else {
            self.extend_inode(number, inode, size)
        }
    }

    /// The shrink half of [`Self::truncate`] (ADR 0041).
    fn shrink_inode(&mut self, number: u32, old_inode: Inode, size: u64) -> Result<(), Status> {
        let old_size = old_inode.size;
        let old_blocks = Self::blocks_for(old_size);
        let new_blocks = Self::blocks_for(size);
        let per_block = (BLOCK_SIZE / 4) as u32;
        if old_inode.blocks[14] != 0 {
            return Err(Status::IoError);
        }
        if old_blocks as usize > self.max_double_indirect_blocks() {
            return Err(Status::IoError);
        }

        // A later extension must read zeros, so the tail of the last kept
        // partial block is cleared before the size changes.
        let tail = (size % BLOCK_SIZE as u64) as usize;
        if new_blocks > 0 && tail != 0 {
            let logical = new_blocks - 1;
            let block_number = self.pointer(&old_inode, logical)?.ok_or(Status::IoError)?;
            let mut block = [0u8; BLOCK_SIZE];
            self.block(block_number, &mut block)?;
            block[tail..].fill(0);
            self.write_block(block_number, &block)?;
        }

        // Clear the inode-level pointers, then write the new size and
        // `i_blocks` before any bitmap bit is freed.
        let mut new_inode = old_inode;
        new_inode.size = size;
        for index in 0..12 {
            if index as u32 >= new_blocks {
                new_inode.blocks[index] = 0;
            }
        }
        if new_blocks <= 12 {
            new_inode.blocks[12] = 0;
            new_inode.blocks[13] = 0;
        } else if new_blocks <= 12 + per_block {
            new_inode.blocks[13] = 0;
        }
        new_inode.blocks[14] = 0;
        new_inode.blocks512 = Self::blocks512_for(new_blocks);
        self.store_inode(number, &new_inode)?;

        // The old structure is still intact on disk (only the inode was
        // rewritten), so every removed data block can be found through it.
        let mut logical = new_blocks;
        while logical < old_blocks {
            let block_number = self.pointer(&old_inode, logical)?.ok_or(Status::IoError)?;
            self.free_block(block_number)?;
            logical += 1;
        }

        // A single-indirect block that survives keeps only the entries below
        // the new end; one that is entirely past it is released.
        if old_blocks > 12 {
            if new_blocks <= 12 {
                self.free_block(old_inode.blocks[12])?;
            } else if new_blocks < 12 + per_block {
                let indirect = old_inode.blocks[12];
                let mut pointers = [0u8; BLOCK_SIZE];
                self.block(indirect, &mut pointers)?;
                for index in (new_blocks - 12)..per_block {
                    put_u32(&mut pointers, index as usize * 4, 0)?;
                }
                self.write_block(indirect, &pointers)?;
            }
        }

        // The double-indirect tree: release inner blocks entirely past the new
        // end (their data blocks are already free), trim the straddling one and
        // clear the outer pointers that named a released block.
        if old_blocks > 12 + per_block {
            let outer_number = old_inode.blocks[13];
            let mut outer = [0u8; BLOCK_SIZE];
            self.block(outer_number, &mut outer)?;
            let mut outer_changed = false;
            for outer_index in 0..per_block {
                let inner = read_u32(&outer, outer_index as usize * 4)?;
                if inner == 0 {
                    continue;
                }
                let base = 12 + per_block + outer_index * per_block;
                let inner_end = base + per_block - 1;
                if base >= new_blocks {
                    self.free_block(inner)?;
                    put_u32(&mut outer, outer_index as usize * 4, 0)?;
                    outer_changed = true;
                } else if inner_end >= new_blocks {
                    let mut pointers = [0u8; BLOCK_SIZE];
                    self.block(inner, &mut pointers)?;
                    for index in (new_blocks - base)..per_block {
                        put_u32(&mut pointers, index as usize * 4, 0)?;
                    }
                    self.write_block(inner, &pointers)?;
                }
            }
            if new_blocks <= 12 + per_block {
                self.free_block(outer_number)?;
            } else if outer_changed {
                self.write_block(outer_number, &outer)?;
            }
        }
        Ok(())
    }

    /// The extend half of [`Self::truncate`] (ADR 0041).
    fn extend_inode(&mut self, number: u32, mut inode: Inode, size: u64) -> Result<(), Status> {
        let old_size = inode.size;
        let old_blocks = Self::blocks_for(old_size);
        let new_blocks = Self::blocks_for(size);
        if new_blocks as usize > self.max_double_indirect_blocks() {
            return Err(Status::CapacityExceeded);
        }
        // Capacity is planned before the first allocation.
        if self.plan_blocks(&inode, old_blocks, new_blocks - 1)? > self.free_blocks() {
            return Err(Status::CapacityExceeded);
        }

        // The bytes between the old end and the block boundary read as zeros;
        // the new blocks are zero-filled from end to end.
        let tail = (old_size % BLOCK_SIZE as u64) as usize;
        if old_blocks > 0 && tail != 0 {
            let logical = old_blocks - 1;
            let block_number = self.pointer(&inode, logical)?.ok_or(Status::IoError)?;
            let mut block = [0u8; BLOCK_SIZE];
            self.block(block_number, &mut block)?;
            block[tail..].fill(0);
            self.write_block(block_number, &block)?;
        }
        for logical in old_blocks..new_blocks {
            let (block_number, _fresh) = self.data_block_for(&mut inode, logical)?;
            let block = [0u8; BLOCK_SIZE];
            self.write_block(block_number, &block)?;
        }
        inode.size = size;
        self.store_inode(number, &inode)
    }

    /// Finds the first directory record that can hold a `needed`-byte entry.
    ///
    /// A free record (inode 0) that is large enough is reused whole; a live
    /// record is split when its own aligned length plus `needed` still fits
    /// its `rec_len`. `None` means the directory needs a new data block.
    fn find_dir_slot(&self, directory: &Inode, needed: usize) -> Result<Option<DirSlot>, Status> {
        let mut found = None;
        let mut error = None;
        self.each_data_block(directory, &mut |index, _| {
            let mut block = [0u8; BLOCK_SIZE];
            self.block(index, &mut block)?;
            let result = walk_dir_block(&block, self.has_filetype, &mut |entry| {
                if entry.inode == 0 {
                    if entry.rec_len >= needed {
                        found = Some(DirSlot {
                            block: index,
                            offset: entry.offset,
                            record: entry.rec_len,
                            used: 0,
                            free: true,
                        });
                        return Ok(false);
                    }
                    return Ok(true);
                }
                let used = align4(8 + entry.name_len);
                if used <= entry.rec_len && entry.rec_len >= used + needed {
                    found = Some(DirSlot {
                        block: index,
                        offset: entry.offset,
                        record: entry.rec_len,
                        used,
                        free: false,
                    });
                    return Ok(false);
                }
                Ok(true)
            });
            match result {
                Ok(stopped) => Ok(stopped),
                Err(status) => {
                    error = Some(status);
                    Err(status)
                }
            }
        })?;
        match error {
            Some(status) => Err(status),
            None => Ok(found),
        }
    }

    /// Adds a directory entry for `inode_number`, growing the directory by one
    /// block when no record has room.
    ///
    /// The data block is written before the parent inode that points at it, and
    /// the created file's inode was written before this call, so the entry is
    /// never visible without its inode (W7 ordering).
    fn add_entry(
        &mut self,
        dir_number: u32,
        dir: &mut Inode,
        name: &[u8],
        inode_number: u32,
        file_type: u8,
    ) -> Result<(), Status> {
        let needed = align4(8 + name.len());
        if let Some(slot) = self.find_dir_slot(dir, needed)? {
            let mut block = [0u8; BLOCK_SIZE];
            self.block(slot.block, &mut block)?;
            if slot.free {
                write_dir_entry(
                    &mut block,
                    slot.offset,
                    slot.record,
                    inode_number,
                    name,
                    file_type,
                    self.has_filetype,
                )?;
            } else {
                put_u16(&mut block, slot.offset + 4, slot.used as u16)?;
                write_dir_entry(
                    &mut block,
                    slot.offset + slot.used,
                    slot.record - slot.used,
                    inode_number,
                    name,
                    file_type,
                    self.has_filetype,
                )?;
            }
            return self.write_block(slot.block, &block);
        }

        let logical = (dir.size.div_ceil(BLOCK_SIZE as u64)) as u32;
        let (block_number, fresh) = self.data_block_for(dir, logical)?;
        if !fresh {
            // A directory must not have a hole; refuse rather than overwrite
            // a block that is already in use.
            return Err(Status::IoError);
        }
        let mut block = [0u8; BLOCK_SIZE];
        write_dir_entry(
            &mut block,
            0,
            BLOCK_SIZE,
            inode_number,
            name,
            file_type,
            self.has_filetype,
        )?;
        self.write_block(block_number, &block)?;
        dir.size = u64::from(logical + 1) * BLOCK_SIZE as u64;
        self.store_inode(dir_number, dir)
    }

    // ---- removal, directories and rename (spec 0019, W3–W6) ----------

    /// Frees one data block: clears its bitmap bit, then raises the group and
    /// superblock free counts (the reverse of [`Self::alloc_block`]).
    ///
    /// A block that the bitmap already marks free is `IoError`: a double free
    /// means the tree and the bitmap disagree, and the writer must not invent a
    /// count for it.
    fn free_block(&mut self, number: u32) -> Result<(), Status> {
        if number <= self.first_data_block || number >= self.blocks_count {
            return Err(Status::IoError);
        }
        let relative = number - self.first_data_block;
        let group = relative / self.blocks_per_group;
        let bit = relative % self.blocks_per_group;
        let descriptor = self.group_descriptor(group)?;
        let mut bitmap = [0u8; BLOCK_SIZE];
        self.block(descriptor.block_bitmap, &mut bitmap)?;
        let index = (bit / 8) as usize;
        let mask = 1u8 << (bit % 8);
        if bitmap[index] & mask == 0 {
            return Err(Status::IoError);
        }
        bitmap[index] &= !mask;
        self.write_block(descriptor.block_bitmap, &bitmap)?;
        self.write_group_free(group, GD_FREE_BLOCKS, descriptor.free_blocks + 1)?;
        self.set_superblock_free_blocks(self.free_blocks() + 1)
    }

    /// Frees one inode: clears its bitmap bit, then raises the group and
    /// superblock free counts (the reverse of [`Self::alloc_inode`]).
    ///
    /// The inode-table entry is zeroed so a scan of the table cannot count a
    /// freed inode as live, and a directory also lowers the group's used-count.
    fn free_inode(&mut self, number: u32, is_dir: bool) -> Result<(), Status> {
        if number == 0 || number > self.inodes_count {
            return Err(Status::IoError);
        }
        let relative = number - 1;
        let group = relative / self.inodes_per_group;
        let bit = relative % self.inodes_per_group;
        let descriptor = self.group_descriptor(group)?;
        let mut bitmap = [0u8; BLOCK_SIZE];
        self.block(descriptor.inode_bitmap, &mut bitmap)?;
        let index = (bit / 8) as usize;
        let mask = 1u8 << (bit % 8);
        if bitmap[index] & mask == 0 {
            return Err(Status::IoError);
        }
        bitmap[index] &= !mask;
        self.write_block(descriptor.inode_bitmap, &bitmap)?;
        self.write_group_free(group, GD_FREE_INODES, descriptor.free_inodes + 1)?;
        self.set_superblock_free_inodes(self.free_inodes() + 1)?;

        let (block_number, start) = self.inode_location(number)?;
        let mut block = [0u8; BLOCK_SIZE];
        self.block(block_number, &mut block)?;
        let end = start.checked_add(self.inode_size).ok_or(Status::Invalid)?;
        block.get_mut(start..end).ok_or(Status::Invalid)?.fill(0);
        self.write_block(block_number, &block)?;
        if is_dir {
            self.adjust_used_dirs(group, false)?;
        }
        Ok(())
    }

    /// Releases every data and indirect block an inode still points at.
    ///
    /// The triple-indirect pointer is outside the subset and is refused rather
    /// than leaked. The caller has already removed the directory entry, so a
    /// block cannot stay reachable once this runs (W5 ordering).
    fn free_inode_blocks(&mut self, inode: &Inode) -> Result<(), Status> {
        if inode.blocks[14] != 0 {
            return Err(Status::IoError);
        }
        for index in 0..12 {
            let pointer = inode.blocks[index];
            if pointer != 0 {
                self.free_block(pointer)?;
            }
        }
        if inode.blocks[12] != 0 {
            let mut pointers = [0u8; BLOCK_SIZE];
            self.block(inode.blocks[12], &mut pointers)?;
            for chunk in pointers.chunks_exact(4) {
                let pointer = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                if pointer != 0 {
                    self.free_block(pointer)?;
                }
            }
            self.free_block(inode.blocks[12])?;
        }
        if inode.blocks[13] != 0 {
            let mut outer = [0u8; BLOCK_SIZE];
            self.block(inode.blocks[13], &mut outer)?;
            for chunk in outer.chunks_exact(4) {
                let inner = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                if inner == 0 {
                    continue;
                }
                let mut pointers = [0u8; BLOCK_SIZE];
                self.block(inner, &mut pointers)?;
                for entry in pointers.chunks_exact(4) {
                    let pointer = u32::from_le_bytes([entry[0], entry[1], entry[2], entry[3]]);
                    if pointer != 0 {
                        self.free_block(pointer)?;
                    }
                }
                self.free_block(inner)?;
            }
            self.free_block(inode.blocks[13])?;
        }
        Ok(())
    }

    /// Finds one record of `name` inside a directory, with the offset of the
    /// record before it (if any) so a removal can merge the two.
    fn entry_record(
        &self,
        directory: &Inode,
        name: &[u8],
    ) -> Result<Option<(u32, DirEntryRef, Option<usize>)>, Status> {
        let mut found = None;
        let mut error = None;
        self.each_data_block(directory, &mut |index, _| {
            let mut block = [0u8; BLOCK_SIZE];
            self.block(index, &mut block)?;
            let mut previous: Option<usize> = None;
            let mut hit: Option<(DirEntryRef, Option<usize>)> = None;
            let result = walk_dir_block(&block, self.has_filetype, &mut |entry| {
                if entry.inode != 0 {
                    let end = entry.name_start + entry.name_len;
                    if entry.name_len == name.len() && block.get(entry.name_start..end) == Some(name)
                    {
                        hit = Some((entry, previous));
                        return Ok(false);
                    }
                }
                previous = Some(entry.offset);
                Ok(true)
            });
            if let Err(status) = result {
                error = Some(status);
                return Err(status);
            }
            if let Some((entry, previous)) = hit {
                found = Some((index, entry, previous));
                return Ok(false);
            }
            Ok(true)
        })?;
        match error {
            Some(status) => Err(status),
            None => Ok(found),
        }
    }

    /// Removes one entry from a directory, merging its record into the
    /// predecessor or leaving it as the block's free record (W5).
    ///
    /// A trailing directory block that becomes entirely free is released and
    /// the directory shrinks by one block; an interior free block stays (holes
    /// are not allowed). The directory inode is stored here when the tail is
    /// compacted.
    fn unlink_entry(
        &mut self,
        dir_number: u32,
        dir: &mut Inode,
        name: &[u8],
    ) -> Result<(), Status> {
        let Some((block_number, entry, previous)) = self.entry_record(dir, name)? else {
            return Err(Status::NotFound);
        };
        let mut block = [0u8; BLOCK_SIZE];
        self.block(block_number, &mut block)?;
        let record = u16::try_from(entry.rec_len).map_err(|_| Status::IoError)?;
        match previous {
            Some(offset) => {
                let prior = read_u16(&block, offset + 4)? as usize;
                let merged = prior.checked_add(entry.rec_len).ok_or(Status::IoError)?;
                put_u16(
                    &mut block,
                    offset + 4,
                    u16::try_from(merged).map_err(|_| Status::IoError)?,
                )?;
            }
            None => {
                put_u32(&mut block, entry.offset, 0)?;
                put_u16(&mut block, entry.offset + 4, record)?;
                if self.has_filetype {
                    *block.get_mut(entry.offset + 6).ok_or(Status::Invalid)? = 0;
                    *block.get_mut(entry.offset + 7).ok_or(Status::Invalid)? = 0;
                } else {
                    put_u16(&mut block, entry.offset + 6, 0)?;
                }
                let start = entry.offset + 8;
                let end = entry.offset + entry.rec_len;
                block.get_mut(start..end).ok_or(Status::Invalid)?.fill(0);
            }
        }
        self.write_block(block_number, &block)?;
        self.compact_directory_tail(dir_number, dir)
    }

    /// Releases trailing directory blocks that no longer hold a live entry.
    ///
    /// Only direct and single-indirect blocks are compacted; a deeper directory
    /// keeps its free blocks (still a legal ext2 directory) rather than risking
    /// a double-indirect update in this slice.
    fn compact_directory_tail(
        &mut self,
        dir_number: u32,
        dir: &mut Inode,
    ) -> Result<(), Status> {
        while self.shrink_last_directory_block(dir_number, dir)? {}
        Ok(())
    }

    fn shrink_last_directory_block(
        &mut self,
        dir_number: u32,
        dir: &mut Inode,
    ) -> Result<bool, Status> {
        let total = (dir.size / BLOCK_SIZE as u64) as u32;
        if total <= 1 {
            return Ok(false);
        }
        let logical = total - 1;
        let Some(block_number) = self.pointer(dir, logical)? else {
            return Ok(false);
        };
        let mut block = [0u8; BLOCK_SIZE];
        self.block(block_number, &mut block)?;
        if !block_all_free(&block, self.has_filetype)? {
            return Ok(false);
        }

        let per_block = (BLOCK_SIZE / 4) as u32;
        if logical < 12 {
            dir.blocks[logical as usize] = 0;
        } else if logical < 12 + per_block {
            let indirect = nonzero(dir.blocks[12])?;
            let mut pointers = [0u8; BLOCK_SIZE];
            self.block(indirect, &mut pointers)?;
            put_u32(&mut pointers, (logical - 12) as usize * 4, 0)?;
            self.write_block(indirect, &pointers)?;
            let empty = pointers.chunks_exact(4).all(|chunk| {
                u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]) == 0
            });
            if empty {
                self.free_block(indirect)?;
                dir.blocks[12] = 0;
                dir.blocks512 = dir.blocks512.saturating_sub(2);
            }
        } else {
            return Ok(false);
        }

        self.free_block(block_number)?;
        dir.blocks512 = dir.blocks512.saturating_sub(2);
        dir.size -= BLOCK_SIZE as u64;
        self.store_inode(dir_number, dir)?;
        Ok(true)
    }

    /// True for a directory whose only entries are `.` and `..`.
    fn directory_is_empty(&self, directory: &Inode) -> Result<bool, Status> {
        let mut live = false;
        self.each_dir_entry(directory, &mut |_, _, name| {
            if name == b"." || name == b".." {
                Ok(true)
            } else {
                live = true;
                Ok(false)
            }
        })?;
        Ok(!live)
    }

    /// Initializes a fresh directory inode: mode `S_IFDIR | 0755`, two links,
    /// one 1 KiB block holding `.` and `..`.
    fn init_directory_inode(&mut self, number: u32, block_number: u32, parent: u32)
        -> Result<(), Status>
    {
        let data = self.new_directory_block(number, parent);
        let (inode_block, start) = self.inode_location(number)?;
        // The block's contents land before the inode points at it.
        self.write_block(block_number, &data)?;
        let mut block = [0u8; BLOCK_SIZE];
        self.block(inode_block, &mut block)?;
        let end = start.checked_add(self.inode_size).ok_or(Status::Invalid)?;
        let bytes = block.get_mut(start..end).ok_or(Status::Invalid)?;
        bytes.fill(0);
        put_u16(bytes, 0, DIRECTORY_MODE)?;
        put_u32(bytes, 4, BLOCK_SIZE as u32)?;
        put_u16(bytes, INODE_LINKS, 2)?;
        put_u32(bytes, INODE_BLOCKS512, 2)?;
        put_u32(bytes, INODE_BLOCK_WORDS, block_number)?;
        self.write_block(inode_block, &block)
    }

    fn new_directory_block(&self, number: u32, parent: u32) -> [u8; BLOCK_SIZE] {
        let mut block = [0u8; BLOCK_SIZE];
        // The two mandatory entries always fit: 12 bytes for `.` and the rest
        // for `..`. The writes cannot fail, so the result is ignored.
        let _ = write_dir_entry(
            &mut block,
            0,
            12,
            number,
            b".",
            EXT2_FT_DIR,
            self.has_filetype,
        );
        let _ = write_dir_entry(
            &mut block,
            12,
            BLOCK_SIZE - 12,
            parent,
            b"..",
            EXT2_FT_DIR,
            self.has_filetype,
        );
        block
    }

    /// Removes a regular file or an empty directory (spec 0019, W3/W5).
    ///
    /// The parent entry goes first, then the file's data and indirect blocks,
    /// then the inode and the free counts; a directory also drops its parent's
    /// link count. A non-empty directory, the root and a missing name are
    /// refused without changing anything.
    pub fn remove(&mut self, path: &str) -> Result<(), Status> {
        if !self.writable {
            return Err(Status::Invalid);
        }
        let (parent_path, name) = split_parent(path)?;
        let (parent_number, mut parent) = self.directory(parent_path)?;
        let Some(number) = self.entry_inode(&parent, name.as_bytes())? else {
            return Err(Status::NotFound);
        };
        let target = self.inode(number)?;
        if !target.is_file() && !target.is_dir() {
            return Err(Status::Invalid);
        }
        if target.is_dir() && !self.directory_is_empty(&target)? {
            return Err(Status::Invalid);
        }

        self.unlink_entry(parent_number, &mut parent, name.as_bytes())?;
        self.free_inode_blocks(&target)?;
        self.free_inode(number, target.is_dir())?;
        if target.is_dir() {
            parent.links = parent.links.saturating_sub(1);
            self.store_inode(parent_number, &parent)?;
        }
        Ok(())
    }

    /// Creates an empty directory (spec 0019, W5).
    ///
    /// The inode and its one block are written before the parent entry that
    /// names it; the parent's link count rises by one. A duplicate name, a
    /// missing or non-directory parent and a full filesystem are refused before
    /// the first allocation.
    pub fn mkdir(&mut self, path: &str) -> Result<u32, Status> {
        if !self.writable {
            return Err(Status::Invalid);
        }
        let (parent_path, name) = split_parent(path)?;
        let (parent_number, mut parent) = self.directory(parent_path)?;
        let name = name.as_bytes();
        if self.entry_inode(&parent, name)?.is_some() {
            return Err(Status::Invalid);
        }
        if self.free_inodes() == 0 || self.free_blocks() == 0 {
            return Err(Status::CapacityExceeded);
        }

        let needed = align4(8 + name.len());
        let slot = self.find_dir_slot(&parent, needed)?;
        let mut blocks_needed = 1u32;
        if slot.is_none() {
            let logical = (parent.size.div_ceil(BLOCK_SIZE as u64)) as u32;
            blocks_needed =
                blocks_needed.saturating_add(self.plan_blocks(&parent, logical, logical)?);
        }
        if blocks_needed > self.free_blocks() {
            return Err(Status::CapacityExceeded);
        }

        let number = self.alloc_inode()?;
        let block = self.alloc_block()?;
        self.init_directory_inode(number, block, parent_number)?;
        let group = (number - 1) / self.inodes_per_group;
        self.adjust_used_dirs(group, true)?;
        parent.links = parent.links.saturating_add(1);
        self.store_inode(parent_number, &parent)?;
        self.add_entry(parent_number, &mut parent, name, number, EXT2_FT_DIR)?;
        Ok(number)
    }

    /// Moves a file or directory, replacing an existing regular-file target
    /// (spec 0019, W3/W6).
    ///
    /// `to` may be in the same or another directory. An existing regular file
    /// at `to` is unlinked and freed first; an existing directory target, the
    /// root and a missing source are refused. A cross-directory directory move
    /// rewrites the moved directory's `..` and adjusts both parents' link
    /// counts. The same path is an `Ok` no-op.
    pub fn rename(&mut self, from: &str, to: &str) -> Result<(), Status> {
        if !self.writable {
            return Err(Status::Invalid);
        }
        if check_path(from)? == 0 || check_path(to)? == 0 {
            return Err(Status::Invalid);
        }
        if from == to {
            return Ok(());
        }

        let (from_parent_path, from_name) = split_parent(from)?;
        let (to_parent_path, to_name) = split_parent(to)?;
        let (from_parent_number, from_parent) = self.directory(from_parent_path)?;
        let Some(source_number) = self.entry_inode(&from_parent, from_name.as_bytes())? else {
            return Err(Status::NotFound);
        };
        let source = self.inode(source_number)?;
        if source.is_dir() && is_within(to, from) {
            // Moving a directory into its own subtree would make a cycle.
            return Err(Status::Invalid);
        }

        let (to_parent_number, mut to_parent) = self.directory(to_parent_path)?;
        let same_parent = from_parent_number == to_parent_number;

        let target = self.entry_inode(&to_parent, to_name.as_bytes())?;
        if let Some(number) = target {
            if number == source_number {
                // The two names already address the same inode (the same path
                // was handled above; this is a no-op the tree cannot express
                // without hard links, so it is refused).
                return Ok(());
            }
            if self.inode(number)?.is_dir() {
                return Err(Status::Invalid);
            }
        }

        // Plan the target directory growth before the first mutation. A
        // replacement's freed data blocks are not assumed to fund it.
        let needed = align4(8 + to_name.len());
        if self.find_dir_slot(&to_parent, needed)?.is_none() {
            let logical = (to_parent.size.div_ceil(BLOCK_SIZE as u64)) as u32;
            if self.plan_blocks(&to_parent, logical, logical)? > self.free_blocks() {
                return Err(Status::CapacityExceeded);
            }
        }

        if let Some(number) = target {
            let replaced = self.inode(number)?;
            self.unlink_entry(to_parent_number, &mut to_parent, to_name.as_bytes())?;
            self.free_inode_blocks(&replaced)?;
            self.free_inode(number, false)?;
        }

        let file_type = if source.is_dir() {
            EXT2_FT_DIR
        } else {
            EXT2_FT_REG_FILE
        };
        // Removing the target may have changed the source directory too (the
        // two can be the same inode), so read it back before removing the
        // source entry.
        let (_, mut from_parent) = self.directory(from_parent_path)?;
        self.unlink_entry(from_parent_number, &mut from_parent, from_name.as_bytes())?;

        // The removals above may have changed either directory, and the two may
        // be the same inode; re-read the target parent before adding the entry.
        let (_, mut target_parent) = self.directory(to_parent_path)?;
        self.add_entry(to_parent_number, &mut target_parent, to_name.as_bytes(), source_number,
            file_type)?;

        if source.is_dir() && !same_parent {
            let mut moved = self.inode(source_number)?;
            self.set_dotdot(source_number, &mut moved, to_parent_number)?;
            let (_, mut old_parent) = self.directory(from_parent_path)?;
            old_parent.links = old_parent.links.saturating_sub(1);
            self.store_inode(from_parent_number, &old_parent)?;
            let (_, mut new_parent) = self.directory(to_parent_path)?;
            new_parent.links = new_parent.links.saturating_add(1);
            self.store_inode(to_parent_number, &new_parent)?;
        }
        Ok(())
    }

    /// Rewrites the `..` entry of a moved directory to name its new parent.
    fn set_dotdot(&mut self, _number: u32, directory: &mut Inode, parent: u32)
        -> Result<(), Status>
    {
        let Some((block_number, entry, _)) = self.entry_record(directory, b"..")? else {
            return Err(Status::IoError);
        };
        let mut block = [0u8; BLOCK_SIZE];
        self.block(block_number, &mut block)?;
        put_u32(&mut block, entry.offset, parent)?;
        self.write_block(block_number, &block)
    }

    /// Visits the file's data blocks in order, for exactly `size` bytes.
    ///
    /// A zero pointer is a hole, which the subset does not support. The
    /// visitor receives the block number and the number of meaningful bytes in
    /// the last block (a full block otherwise) and returns `false` to stop.
    fn each_data_block(
        &self,
        inode: &Inode,
        visit: &mut impl FnMut(u32, usize) -> Result<bool, Status>,
    ) -> Result<(), Status> {
        let total = inode.size.div_ceil(BLOCK_SIZE as u64) as usize;
        if total == 0 {
            return Ok(());
        }
        if total > self.max_double_indirect_blocks() {
            // The file needs the triple-indirect pointer, which the subset does
            // not support.
            return Err(Status::IoError);
        }
        if inode.blocks[14] != 0 {
            return Err(Status::IoError);
        }

        let mut remaining = total;
        let valid = |remaining: usize| -> usize {
            if remaining == 1 {
                let tail = inode.size as usize % BLOCK_SIZE;
                if tail == 0 {
                    BLOCK_SIZE
                } else {
                    tail
                }
            } else {
                BLOCK_SIZE
            }
        };

        for &pointer in &inode.blocks[..12] {
            if remaining == 0 {
                return Ok(());
            }
            if pointer == 0 {
                return Err(Status::Invalid);
            }
            if !visit(pointer, valid(remaining))? {
                return Ok(());
            }
            remaining -= 1;
        }

        // Single indirect.
        if remaining > 0 {
            let indirect = nonzero(inode.blocks[12])?;
            let mut pointers = [0u8; BLOCK_SIZE];
            self.block(indirect, &mut pointers)?;
            for chunk in pointers.chunks_exact(4) {
                if remaining == 0 {
                    return Ok(());
                }
                let pointer = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                if pointer == 0 {
                    return Err(Status::Invalid);
                }
                if !visit(pointer, valid(remaining))? {
                    return Ok(());
                }
                remaining -= 1;
            }
        }

        // Double indirect: one pointer block of single-indirect pointers.
        if remaining > 0 {
            let indirect = nonzero(inode.blocks[13])?;
            let mut outer = [0u8; BLOCK_SIZE];
            self.block(indirect, &mut outer)?;
            for chunk in outer.chunks_exact(4) {
                if remaining == 0 {
                    return Ok(());
                }
                let pointer = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                if pointer == 0 {
                    return Err(Status::Invalid);
                }
                let mut inner = [0u8; BLOCK_SIZE];
                self.block(pointer, &mut inner)?;
                for entry in inner.chunks_exact(4) {
                    if remaining == 0 {
                        return Ok(());
                    }
                    let data = u32::from_le_bytes([entry[0], entry[1], entry[2], entry[3]]);
                    if data == 0 {
                        return Err(Status::Invalid);
                    }
                    if !visit(data, valid(remaining))? {
                        return Ok(());
                    }
                    remaining -= 1;
                }
            }
        }

        if remaining > 0 {
            return Err(Status::IoError);
        }
        Ok(())
    }

    /// Number of data blocks reachable without the triple-indirect pointer.
    fn max_double_indirect_blocks(&self) -> usize {
        let per_block = BLOCK_SIZE / 4;
        12 + per_block + per_block * per_block
    }

    /// Visits the entries of a directory inode. `.` and `..` are included here;
    /// callers filter them.
    fn each_dir_entry(
        &self,
        inode: &Inode,
        visit: &mut impl FnMut(u32, u8, &[u8]) -> Result<bool, Status>,
    ) -> Result<(), Status> {
        if !inode.is_dir() {
            return Err(Status::Invalid);
        }
        let mut error = None;
        self.each_data_block(inode, &mut |index, _valid| {
            let mut block = [0u8; BLOCK_SIZE];
            self.block(index, &mut block)?;
            match parse_dir_block(&block, self.has_filetype, visit) {
                Ok(stopped) => Ok(stopped),
                Err(status) => {
                    error = Some(status);
                    Err(status)
                }
            }
        })?;
        match error {
            Some(status) => Err(status),
            None => Ok(()),
        }
    }
}

/// One validated record of a directory data block.
#[derive(Clone, Copy)]
struct DirEntryRef {
    offset: usize,
    inode: u32,
    rec_len: usize,
    name_len: usize,
    name_start: usize,
    file_type: u8,
}

/// A directory record that can take a new entry, found by [`Fs::find_dir_slot`].
#[derive(Clone, Copy)]
struct DirSlot {
    /// The filesystem block the record lives in.
    block: u32,
    /// Byte offset of the record inside that block.
    offset: usize,
    /// The record's `rec_len`.
    record: usize,
    /// The bytes the live record really needs (zero for a free record).
    used: usize,
    /// True when the record has no inode and can be reused whole.
    free: bool,
}

/// Validates and visits one directory data block's records.
///
/// Returns `Ok(false)` when the visitor asked to stop. `rec_len` must be a
/// multiple of four, at least 8 and must not cross the block; a live record's
/// `name_len` must fit inside it. Free records (inode 0) are visited too: the
/// writer reuses them.
fn walk_dir_block(
    block: &[u8; BLOCK_SIZE],
    has_filetype: bool,
    visit: &mut impl FnMut(DirEntryRef) -> Result<bool, Status>,
) -> Result<bool, Status> {
    let mut offset = 0usize;
    while offset < block.len() {
        if block.len() - offset < 8 {
            return Err(Status::Invalid);
        }
        let inode = read_u32(block, offset)?;
        let rec_len = read_u16(block, offset + 4)? as usize;
        if rec_len < 8 || rec_len % 4 != 0 {
            return Err(Status::Invalid);
        }
        let next = offset.checked_add(rec_len).ok_or(Status::Invalid)?;
        if next > block.len() {
            return Err(Status::Invalid);
        }

        let (name_len, file_type) = if has_filetype {
            let length = *block.get(offset + 6).ok_or(Status::Invalid)? as usize;
            let kind = *block.get(offset + 7).ok_or(Status::Invalid)?;
            (length, kind)
        } else {
            (read_u16(block, offset + 6)? as usize, 0)
        };
        if inode != 0 && (name_len == 0 || name_len > rec_len - 8) {
            return Err(Status::Invalid);
        }
        let entry = DirEntryRef {
            offset,
            inode,
            rec_len,
            name_len,
            name_start: offset + 8,
            file_type,
        };
        if !visit(entry)? {
            return Ok(false);
        }
        offset = next;
    }
    Ok(true)
}

/// Writes one directory entry into a record of `record` bytes at `offset`.
///
/// The record must leave room for the 8-byte header and the name. The rest of
/// the record is zeroed so the block is deterministic.
fn write_dir_entry(
    block: &mut [u8; BLOCK_SIZE],
    offset: usize,
    record: usize,
    inode: u32,
    name: &[u8],
    file_type: u8,
    has_filetype: bool,
) -> Result<(), Status> {
    let needed = align4(8 + name.len());
    let record_len = u16::try_from(record).map_err(|_| Status::CapacityExceeded)?;
    if name.is_empty() || needed > record || name.len() > u8::MAX as usize {
        return Err(Status::CapacityExceeded);
    }
    let end = offset.checked_add(record).ok_or(Status::Invalid)?;
    let record = block.get_mut(offset..end).ok_or(Status::Invalid)?;
    put_u32(record, 0, inode)?;
    put_u16(record, 4, record_len)?;
    if has_filetype {
        *record.get_mut(6).ok_or(Status::Invalid)? = name.len() as u8;
        *record.get_mut(7).ok_or(Status::Invalid)? = file_type;
    } else {
        put_u16(record, 6, name.len() as u16)?;
    }
    let name_end = 8usize.checked_add(name.len()).ok_or(Status::Invalid)?;
    record
        .get_mut(8..name_end)
        .ok_or(Status::Invalid)?
        .copy_from_slice(name);
    record.get_mut(name_end..).ok_or(Status::Invalid)?.fill(0);
    Ok(())
}

/// True when `path` is `ancestor` itself or lies below it.
fn is_within(path: &str, ancestor: &str) -> bool {
    if path == ancestor {
        return true;
    }
    path.len() > ancestor.len()
        && path.starts_with(ancestor)
        && path.as_bytes().get(ancestor.len()) == Some(&b'/')
}

/// True when no record of a directory data block names a live inode.
fn block_all_free(block: &[u8; BLOCK_SIZE], has_filetype: bool) -> Result<bool, Status> {
    let mut live = false;
    walk_dir_block(block, has_filetype, &mut |entry| {
        if entry.inode != 0 {
            live = true;
            return Ok(false);
        }
        Ok(true)
    })?;
    Ok(!live)
}

/// Validates the syntax of a non-root path and returns its component count
/// (zero for `""` and `"."`).
fn check_path(path: &str) -> Result<usize, Status> {
    if path.is_empty() || path == "." {
        return Ok(0);
    }
    if path.len() > abi::fs::PATH_MAX {
        return Err(Status::CapacityExceeded);
    }
    let mut components = 0usize;
    for part in path.split('/') {
        if part.is_empty() || part == "." || part == ".." || part.len() > abi::fs::NAME_CAP {
            return Err(Status::Invalid);
        }
        components += 1;
    }
    if components > abi::fs::MAX_COMPONENTS {
        return Err(Status::Invalid);
    }
    Ok(components)
}

/// Splits a create path into its parent directory and final component.
fn split_parent(path: &str) -> Result<(&str, &str), Status> {
    if check_path(path)? == 0 {
        return Err(Status::Invalid);
    }
    match path.rfind('/') {
        Some(index) => Ok((&path[..index], &path[index + 1..])),
        None => Ok(("", path)),
    }
}

/// Validates and visits one directory data block.
///
/// Returns `Ok(false)` when the visitor asked to stop.
fn parse_dir_block(
    block: &[u8; BLOCK_SIZE],
    has_filetype: bool,
    visit: &mut impl FnMut(u32, u8, &[u8]) -> Result<bool, Status>,
) -> Result<bool, Status> {
    walk_dir_block(block, has_filetype, &mut |entry| {
        if entry.inode == 0 {
            return Ok(true);
        }
        let name_end = entry
            .name_start
            .checked_add(entry.name_len)
            .ok_or(Status::Invalid)?;
        let name = block
            .get(entry.name_start..name_end)
            .ok_or(Status::Invalid)?;
        visit(entry.inode, entry.file_type, name)
    })
}
