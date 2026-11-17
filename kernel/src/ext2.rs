//! A read-only ext2 reader over an in-memory block device (spec 0002, ADR 0015).
//!
//! The supported subset is deliberately small: 1 KiB and 4 KiB blocks, revision
//! 1 filesystems without incompatible features beyond directory file types,
//! regular files in the image root, and direct plus single and double indirect
//! blocks. Anything outside the subset refuses the mount rather than returning
//! a best-effort read.
//!
//! The device is a byte range in identity-mapped physical memory, so reads are
//! subslices: no block buffer, no copying and no allocation. Every block
//! number, table extent and directory entry length is bounds-checked before it
//! is used.

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
/// incompatible feature this reader accepts.
const INCOMPAT_FILETYPE: u32 = 0x0002;
/// Size of a group descriptor (the 32-bit variant).
const GROUP_DESC_BYTES: usize = 32;
/// Inode mode mask and the regular-file and directory kinds.
const S_IFMT: u16 = 0xF000;
const S_IFREG: u16 = 0x8000;
const S_IFDIR: u16 = 0x4000;
/// Words in `i_block`: 12 direct, 1 single, 1 double, 1 triple indirect.
const INODE_WORDS: usize = 15;

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

/// The in-memory block device: a byte range owned by the bootloader.
#[derive(Clone, Copy)]
pub struct BlockDevice<'a> {
    bytes: &'a [u8],
}

impl<'a> BlockDevice<'a> {
    /// Wraps a byte range. The caller guarantees it is identity-mapped.
    pub const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    /// Length of the device in bytes.
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Returns `len` bytes at `offset`, bounds-checked.
    pub fn slice(&self, offset: usize, len: usize) -> Result<&'a [u8], Status> {
        let end = offset.checked_add(len).ok_or(Status::IoError)?;
        self.bytes.get(offset..end).ok_or(Status::IoError)
    }
}

/// One inode as far as the loader needs it.
#[derive(Clone, Copy)]
struct Inode {
    mode: u16,
    size: u64,
    blocks: [u32; INODE_WORDS],
}

/// One root-directory entry as the application file API reports it (spec
/// 0013). `kind` is an `abi::fs::KIND_*` value; `size` is the inode byte size.
#[derive(Clone, Copy)]
pub struct RootEntry {
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

/// A mounted read-only ext2 filesystem.
pub struct Ext2<'a> {
    device: BlockDevice<'a>,
    block_size: usize,
    inodes_count: u32,
    blocks_count: u32,
    inodes_per_group: u32,
    inode_size: usize,
    group_count: u32,
    descriptor_table_block: u32,
    has_filetype: bool,
}

impl<'a> Ext2<'a> {
    /// Validates and mounts an ext2 filesystem.
    ///
    /// Nothing is mounted partially: a bad superblock, an unsupported feature
    /// or an inconsistent group descriptor, inode table or root directory is
    /// an error.
    pub fn mount(device: BlockDevice<'a>) -> Result<Self, Status> {
        if device.len() < SUPERBLOCK_OFFSET + SUPERBLOCK_BYTES {
            return Err(Status::Invalid);
        }
        let superblock = device.slice(SUPERBLOCK_OFFSET, SUPERBLOCK_BYTES)?;

        if read_u16(superblock, 56)? != EXT2_MAGIC {
            return Err(Status::Invalid);
        }
        // Revision 1 (dynamic inodes); revision 0 has fixed 128-byte inodes and
        // is outside the supported subset.
        if read_u32(superblock, 76)? != 1 {
            return Err(Status::Invalid);
        }

        // 1024 << log_block_size: only 1 KiB and 4 KiB are supported.
        let log_block_size = read_u32(superblock, 24)?;
        if log_block_size != 0 && log_block_size != 2 {
            return Err(Status::Invalid);
        }
        let block_size = 1024usize << log_block_size;

        let inode_size = read_u16(superblock, 88)? as usize;
        if inode_size < 128
            || inode_size > block_size
            || !inode_size.is_power_of_two()
            || block_size % inode_size != 0
        {
            return Err(Status::Invalid);
        }

        let inodes_count = read_u32(superblock, 0)?;
        let blocks_count = read_u32(superblock, 4)?;
        let first_data_block = read_u32(superblock, 20)?;
        let blocks_per_group = read_u32(superblock, 32)?;
        let inodes_per_group = read_u32(superblock, 40)?;
        let feature_incompat = read_u32(superblock, 96)?;

        if inodes_count == 0 || blocks_count == 0 || blocks_per_group == 0 || inodes_per_group == 0
        {
            return Err(Status::Invalid);
        }
        let expected_first = if block_size == 1024 { 1 } else { 0 };
        if first_data_block != expected_first || blocks_count <= first_data_block {
            return Err(Status::Invalid);
        }
        if feature_incompat & !INCOMPAT_FILETYPE != 0 {
            return Err(Status::Invalid);
        }

        // The device must hold every block the superblock claims.
        let device_bytes = (blocks_count as u64)
            .checked_mul(block_size as u64)
            .ok_or(Status::Invalid)?;
        if device_bytes > device.len() as u64 {
            return Err(Status::Invalid);
        }

        let data_blocks = blocks_count - first_data_block;
        let group_count = data_blocks.div_ceil(blocks_per_group);
        let descriptor_table_block = first_data_block + 1;

        // The descriptor table must fit before the end of the filesystem.
        let descriptor_bytes = (group_count as usize)
            .checked_mul(GROUP_DESC_BYTES)
            .ok_or(Status::Invalid)?;
        let descriptor_blocks = descriptor_bytes.div_ceil(block_size);
        if descriptor_table_block
            .checked_add(descriptor_blocks as u32)
            .is_none_or(|end| end > blocks_count)
        {
            return Err(Status::Invalid);
        }

        let fs = Ext2 {
            device,
            block_size,
            inodes_count,
            blocks_count,
            inodes_per_group,
            inode_size,
            group_count,
            descriptor_table_block,
            has_filetype: feature_incompat & INCOMPAT_FILETYPE != 0,
        };

        // Validate every group's bitmap and inode-table extent. A single bad
        // descriptor refuses the mount.
        let inode_table_blocks = ((inodes_per_group as usize)
            .checked_mul(inode_size)
            .ok_or(Status::Invalid)?)
        .div_ceil(block_size);
        for group in 0..group_count {
            let descriptor = fs.group_descriptor(group)?;
            let block_bitmap = read_u32(descriptor, 0)?;
            let inode_bitmap = read_u32(descriptor, 4)?;
            let inode_table = read_u32(descriptor, 8)?;
            for block in [block_bitmap, inode_bitmap, inode_table] {
                if block < first_data_block || block >= blocks_count {
                    return Err(Status::Invalid);
                }
            }
            if (inode_table as usize)
                .checked_add(inode_table_blocks)
                .is_none_or(|end| end > blocks_count as usize)
            {
                return Err(Status::Invalid);
            }
        }

        // The root directory must be a directory and every entry must parse.
        let root = fs.inode(ROOT_INODE)?;
        if root.mode & S_IFMT != S_IFDIR {
            return Err(Status::Invalid);
        }
        fs.each_dir_entry(&root, &mut |_, _, _| Ok(true))?;

        Ok(fs)
    }

    /// Block size in bytes.
    pub fn block_size(&self) -> usize {
        self.block_size
    }

    /// Size of the filesystem in bytes as claimed by the superblock.
    pub fn size_bytes(&self) -> u64 {
        self.blocks_count as u64 * self.block_size as u64
    }

    /// Reads a whole file by exact, case-sensitive name.
    ///
    /// Returns the byte count written. A file larger than `out` is
    /// [`Status::CapacityExceeded`]; a name that is not a regular file is
    /// [`Status::Invalid`]; a missing name is [`Status::NotFound`].
    pub fn read_file(&self, name: &str, out: &mut [u8]) -> Result<usize, Status> {
        let inode = self.regular_inode(name)?;
        if inode.size > out.len() as u64 {
            return Err(Status::CapacityExceeded);
        }

        let mut written = 0usize;
        let size = inode.size as usize;
        self.each_data_block(&inode, &mut |data| {
            let remaining = size - written;
            let count = data.len().min(remaining);
            out[written..written + count].copy_from_slice(&data[..count]);
            written += count;
            Ok(())
        })?;
        if written != size {
            return Err(Status::Invalid);
        }
        Ok(written)
    }

    /// Reads up to `out.len()` bytes of a regular file at byte `offset`
    /// (spec 0013, `core.fs_read`).
    ///
    /// The data blocks are streamed in order; nothing is buffered or
    /// allocated. An offset at or past the file's end returns `Ok(0)`, so
    /// end-of-file is not an error. A missing name is [`Status::NotFound`] and
    /// a non-regular entry is [`Status::Invalid`], exactly as in
    /// [`Ext2::read_file`].
    pub fn read_at(&self, name: &str, offset: u64, out: &mut [u8]) -> Result<usize, Status> {
        let inode = self.regular_inode(name)?;
        if offset >= inode.size || out.is_empty() {
            return Ok(0);
        }

        let block_size = self.block_size as u64;
        let mut block_start = 0u64;
        let mut written = 0usize;
        self.each_data_block(&inode, &mut |data| {
            let start = block_start;
            block_start += block_size;
            if written == out.len() {
                return Ok(());
            }
            // Blocks are contiguous in the subset, so the block that holds
            // `offset` is the first one whose end is past it.
            let end = (start + block_size).min(inode.size);
            if end <= offset {
                return Ok(());
            }
            let from = offset.saturating_sub(start) as usize;
            let to = (end - start) as usize;
            if from >= to {
                return Ok(());
            }
            let count = (to - from).min(out.len() - written);
            out[written..written + count].copy_from_slice(&data[from..from + count]);
            written += count;
            Ok(())
        })?;
        Ok(written)
    }

    /// Reads one named root-directory entry by 0-based index (spec 0013,
    /// `core.fs_list`).
    ///
    /// `.` and `..` are excluded and entries are visited in directory order
    /// (which is not sorted). The name is copied into `name`; a name longer
    /// than the destination is [`Status::CapacityExceeded`]. An index at or
    /// past the last entry is [`Status::NotFound`].
    pub fn list_entry(&self, index: u32, name: &mut [u8]) -> Result<RootEntry, Status> {
        let root = self.inode(ROOT_INODE)?;
        if root.mode & S_IFMT != S_IFDIR {
            return Err(Status::Invalid);
        }

        let mut seen = 0u32;
        let mut found = None;
        let mut error = None;
        self.each_dir_entry(&root, &mut |number, _, entry| {
            // `each_dir_entry` cannot stop the block walk, so a `done` check
            // keeps a later directory block from overwriting the result.
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
                        found = Some(RootEntry {
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
    pub fn file_size(&self, name: &str) -> Result<u64, Status> {
        Ok(self.regular_inode(name)?.size)
    }

    /// Visits every named entry in the root directory.
    pub fn list(
        &self,
        visit: &mut impl FnMut(&[u8], u32, u64) -> Result<(), Status>,
    ) -> Result<(), Status> {
        let root = self.inode(ROOT_INODE)?;
        if root.mode & S_IFMT != S_IFDIR {
            return Err(Status::Invalid);
        }
        let mut error = None;
        self.each_dir_entry(&root, &mut |number, _, name| {
            if name == b"." || name == b".." {
                return Ok(true);
            }
            let inode = match self.inode(number) {
                Ok(inode) => inode,
                Err(status) => {
                    error = Some(status);
                    return Ok(false);
                }
            };
            if let Err(status) = visit(name, number, inode.size) {
                error = Some(status);
                return Ok(false);
            }
            Ok(true)
        })?;
        match error {
            Some(status) => Err(status),
            None => Ok(()),
        }
    }

    fn regular_inode(&self, name: &str) -> Result<Inode, Status> {
        let (_, inode) = self.find(name)?;
        if inode.mode & S_IFMT != S_IFREG {
            return Err(Status::Invalid);
        }
        Ok(inode)
    }

    /// Finds a root-directory entry, returning its inode number and inode.
    fn find(&self, name: &str) -> Result<(u32, Inode), Status> {
        let root = self.inode(ROOT_INODE)?;
        if root.mode & S_IFMT != S_IFDIR {
            return Err(Status::Invalid);
        }

        let mut found = None;
        self.each_dir_entry(&root, &mut |number, _, entry| {
            if entry == name.as_bytes() {
                found = Some(number);
                Ok(false)
            } else {
                Ok(true)
            }
        })?;

        let number = found.ok_or(Status::NotFound)?;
        let inode = self.inode(number)?;
        Ok((number, inode))
    }

    /// Reads an inode by number.
    fn inode(&self, number: u32) -> Result<Inode, Status> {
        if number == 0 || number > self.inodes_count {
            return Err(Status::Invalid);
        }
        let group = (number - 1) / self.inodes_per_group;
        let index = (number - 1) % self.inodes_per_group;
        let descriptor = self.group_descriptor(group)?;
        let table_block = read_u32(descriptor, 8)?;

        let offset = (index as usize)
            .checked_mul(self.inode_size)
            .ok_or(Status::Invalid)?;
        let block = table_block
            .checked_add((offset / self.block_size) as u32)
            .ok_or(Status::Invalid)?;
        let block = self.block(block)?;
        let start = offset % self.block_size;
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

    fn group_descriptor(&self, group: u32) -> Result<&'a [u8], Status> {
        if group >= self.group_count {
            return Err(Status::Invalid);
        }
        let offset = (group as usize)
            .checked_mul(GROUP_DESC_BYTES)
            .ok_or(Status::Invalid)?;
        let block = self
            .descriptor_table_block
            .checked_add((offset / self.block_size) as u32)
            .ok_or(Status::Invalid)?;
        let block = self.block(block)?;
        let start = offset % self.block_size;
        let end = start.checked_add(GROUP_DESC_BYTES).ok_or(Status::Invalid)?;
        block.get(start..end).ok_or(Status::Invalid)
    }

    /// Reads one filesystem block, bounds-checked against the superblock and
    /// the device.
    fn block(&self, index: u32) -> Result<&'a [u8], Status> {
        if index >= self.blocks_count {
            return Err(Status::IoError);
        }
        let offset = (index as usize)
            .checked_mul(self.block_size)
            .ok_or(Status::IoError)?;
        self.device.slice(offset, self.block_size)
    }

    /// Reads the pointer block at `index` as 4-byte block numbers.
    fn pointer_block(&self, index: u32) -> Result<&'a [u8], Status> {
        let block = self.block(index)?;
        Ok(block)
    }

    /// Reads one data block. A zero pointer is a hole, which the subset does
    /// not support.
    fn data_block(&self, index: u32) -> Result<&'a [u8], Status> {
        if index == 0 {
            return Err(Status::Invalid);
        }
        self.block(index)
    }

    /// Visits the file's data blocks in order, for exactly `size` bytes.
    fn each_data_block(
        &self,
        inode: &Inode,
        visit: &mut impl FnMut(&'a [u8]) -> Result<(), Status>,
    ) -> Result<(), Status> {
        let total = inode.size.div_ceil(self.block_size as u64) as usize;
        if total == 0 {
            return Ok(());
        }
        if total > self.max_double_indirect_blocks() {
            // The file needs the triple-indirect pointer, which the subset
            // does not support (spec 0002).
            return Err(Status::IoError);
        }
        if inode.blocks[14] != 0 {
            return Err(Status::IoError);
        }

        let mut remaining = total;

        for &pointer in &inode.blocks[..12] {
            if remaining == 0 {
                return Ok(());
            }
            visit(self.data_block(pointer)?)?;
            remaining -= 1;
        }

        // Single indirect.
        if remaining > 0 {
            let indirect = self.pointer_block(nonzero(inode.blocks[12])?)?;
            for chunk in indirect.chunks_exact(4) {
                if remaining == 0 {
                    return Ok(());
                }
                let pointer = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                visit(self.data_block(pointer)?)?;
                remaining -= 1;
            }
        }

        // Double indirect: one pointer block of single-indirect pointers.
        if remaining > 0 {
            let indirect = self.pointer_block(nonzero(inode.blocks[13])?)?;
            for chunk in indirect.chunks_exact(4) {
                if remaining == 0 {
                    return Ok(());
                }
                let pointer = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                let single = self.pointer_block(nonzero(pointer)?)?;
                for entry in single.chunks_exact(4) {
                    if remaining == 0 {
                        return Ok(());
                    }
                    let data = u32::from_le_bytes([entry[0], entry[1], entry[2], entry[3]]);
                    visit(self.data_block(data)?)?;
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
        let per_block = self.block_size / 4;
        12 + per_block + per_block * per_block
    }

    /// Visits the root directory's entries of one inode.
    fn each_dir_entry(
        &self,
        inode: &Inode,
        visit: &mut impl FnMut(u32, u8, &[u8]) -> Result<bool, Status>,
    ) -> Result<(), Status> {
        if inode.mode & S_IFMT != S_IFDIR {
            return Err(Status::Invalid);
        }
        let mut error = None;
        self.each_data_block(inode, &mut |block| match parse_dir_block(
            block,
            self.has_filetype,
            visit,
        ) {
            Ok(true) => Ok(()),
            Ok(false) => Ok(()),
            Err(status) => {
                error = Some(status);
                Err(status)
            }
        })?;
        match error {
            Some(status) => Err(status),
            None => Ok(()),
        }
    }
}

fn nonzero(pointer: u32) -> Result<u32, Status> {
    if pointer == 0 {
        Err(Status::Invalid)
    } else {
        Ok(pointer)
    }
}

/// Validates and visits one directory data block.
///
/// Returns `Ok(false)` when the visitor asked to stop. `rec_len` must be a
/// multiple of four, at least 8 and must not cross the block; `name_len` must
/// fit inside the record.
fn parse_dir_block(
    block: &[u8],
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
