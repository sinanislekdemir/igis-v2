//! A bounded ext2 reader over a sector device (spec 0019, W1/W2; ADR 0040).
//!
//! This crate is the read half of the writable home filesystem. It talks to a
//! [`SectorDevice`] instead of an identity-mapped byte range, so the same
//! validation and lookup code runs over the kernel's ATA PIO driver and over an
//! in-memory device in the host tests. The writer joins it in M6.2.
//!
//! The supported subset is deliberately the same one the read-only application
//! image uses: 1 KiB blocks, revision 1, no journal and no extents, direct plus
//! single and double indirect blocks, and at most
//! [`abi::fs::MAX_COMPONENTS`] path components. Anything outside the subset
//! refuses the mount rather than returning a best-effort read.
//!
//! Nothing allocates. A fixed four-block cache turns a filesystem block into
//! two 512-byte sector reads; every inode, descriptor and directory block is
//! bounds-checked before it is used, so a corrupt image is a [`Status`], never
//! a panic.

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
/// Words in `i_block`: 12 direct, 1 single, 1 double, 1 triple indirect.
const INODE_WORDS: usize = 15;
/// Fixed filesystem block size in bytes (spec 0019, W2: 1 KiB blocks).
pub const BLOCK_SIZE: usize = 1024;
/// One sector, the ATA PIO unit.
pub const SECTOR_SIZE: usize = 512;
/// Sectors per filesystem block.
const SECTORS_PER_BLOCK: usize = BLOCK_SIZE / SECTOR_SIZE;
/// Number of cached blocks.
const CACHE_BLOCKS: usize = 4;

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

fn nonzero(pointer: u32) -> Result<u32, Status> {
    if pointer == 0 {
        Err(Status::Invalid)
    } else {
        Ok(pointer)
    }
}

/// One inode as far as the reader needs it.
#[derive(Clone, Copy)]
pub struct Inode {
    mode: u16,
    size: u64,
    blocks: [u32; INODE_WORDS],
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
    inode_size: usize,
    group_count: u32,
    descriptor_table_block: u32,
    has_filetype: bool,
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

/// Validates and mounts an ext2 filesystem (the M6 read-only subset).
///
/// This is the free-function form of [`Fs::mount`], so a caller names only the
/// device type.
pub fn mount<D: SectorDevice>(device: D) -> Result<Fs<D>, Status> {
    Fs::mount(device)
}

impl<D: SectorDevice> Fs<D> {
    /// Validates and mounts an ext2 filesystem.
    ///
    /// Only the M6 subset is accepted: 1 KiB blocks, revision 1, no
    /// incompatible feature beyond directory file types, and a group
    /// descriptor table, inode table and root directory that all lie inside
    /// the device. A bad structure refuses the mount; nothing is mounted
    /// partially.
    pub fn mount(device: D) -> Result<Self, Status> {
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
        if inode_size < 128
            || inode_size > BLOCK_SIZE
            || !inode_size.is_power_of_two()
            || BLOCK_SIZE % inode_size != 0
        {
            return Err(Status::Invalid);
        }

        let inodes_count = read_u32(&superblock, 0)?;
        let blocks_count = read_u32(&superblock, 4)?;
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

        let fs = Fs {
            device,
            cache: RefCell::new(Cache::new()),
            inodes_count,
            blocks_count,
            inodes_per_group,
            inode_size,
            group_count,
            descriptor_table_block,
            has_filetype: feature_incompat & INCOMPAT_FILETYPE != 0,
        };

        // Validate every group's bitmaps and inode-table extent.
        let inode_table_blocks = ((inodes_per_group as usize)
            .checked_mul(inode_size)
            .ok_or(Status::Invalid)?)
        .div_ceil(BLOCK_SIZE);
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
        }

        // The root directory must be a directory and every entry must parse.
        let root = fs.inode(ROOT_INODE)?;
        if !root.is_dir() {
            return Err(Status::Invalid);
        }
        fs.each_dir_entry(&root, &mut |_, _, _| Ok(true))?;

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
    pub fn read_at(&self, path: &str, offset: u64, out: &mut [u8]) -> Result<usize, Status> {
        let inode = self.regular_inode(path)?;
        if offset >= inode.size || out.is_empty() {
            return Ok(0);
        }

        let mut block_start = 0u64;
        let mut written = 0usize;
        self.each_data_block(&inode, &mut |index, _valid| {
            let start = block_start;
            block_start += BLOCK_SIZE as u64;
            if written == out.len() {
                return Ok(false);
            }
            let end = (start + BLOCK_SIZE as u64).min(inode.size);
            if end <= offset {
                return Ok(true);
            }
            let from = offset.saturating_sub(start) as usize;
            let to = (end - start) as usize;
            if from >= to {
                return Ok(true);
            }
            let mut block = [0u8; BLOCK_SIZE];
            self.block(index, &mut block)?;
            let count = (to - from).min(out.len() - written);
            out[written..written + count].copy_from_slice(&block[from..from + count]);
            written += count;
            Ok(written < out.len())
        })?;
        Ok(written)
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
        let mut block = [0u8; BLOCK_SIZE];
        self.block(block_number, &mut block)?;
        let start = offset % BLOCK_SIZE;
        let end = start.checked_add(self.inode_size).ok_or(Status::Invalid)?;
        let inode = block.get(start..end).ok_or(Status::Invalid)?;

        let mode = read_u16(inode, 0)?;
        let size_lo = read_u32(inode, 4)? as u64;
        // `i_size_high` lives at byte 108, inside the mandatory 128-byte area.
        let size_hi = read_u32(inode, 108)? as u64;

        let mut blocks = [0u32; INODE_WORDS];
        for (index, word) in blocks.iter_mut().enumerate() {
            *word = read_u32(inode, 40 + index * 4)?;
        }

        Ok(Inode {
            mode,
            size: size_lo | (size_hi << 32),
            blocks,
        })
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
        })
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

/// Validates and visits one directory data block.
///
/// Returns `Ok(false)` when the visitor asked to stop. `rec_len` must be a
/// multiple of four, at least 8 and must not cross the block; `name_len` must
/// fit inside the record.
fn parse_dir_block(
    block: &[u8; BLOCK_SIZE],
    has_filetype: bool,
    visit: &mut impl FnMut(u32, u8, &[u8]) -> Result<bool, Status>,
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
            (block[offset + 6] as usize, block[offset + 7])
        } else {
            (read_u16(block, offset + 6)? as usize, 0)
        };
        if inode != 0 {
            if name_len == 0 || name_len > rec_len - 8 {
                return Err(Status::Invalid);
            }
            let name_start = offset + 8;
            let name_end = name_start.checked_add(name_len).ok_or(Status::Invalid)?;
            let name = block.get(name_start..name_end).ok_or(Status::Invalid)?;
            if !visit(inode, file_type, name)? {
                return Ok(false);
            }
        }
        offset = next;
    }
    Ok(true)
}
