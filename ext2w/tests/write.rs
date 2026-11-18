//! Host tests for the ext2 writer (spec 0019, W1, W4, W7, H5).
//!
//! Each test builds a real ext2 image with `mke2fs -d`, mounts it writable over
//! an in-memory [`SectorDevice`], mutates it, and — after every test that
//! changes the image — writes the mutated bytes to a temporary file and runs
//! `e2fsck -fn` on it. That host check is the consistency proof (H5). If
//! `mke2fs` or `e2fsck` is not installed the test prints a clear skip message
//! and returns; the harness has no skip primitive.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use abi::Status;
use ext2w::{mount, mount_write, Entry, SectorDevice};

const README: &[u8] = b"igis-v2 home\n\nA host-side ext2 fixture for the ext2w writer.\n";

/// An image in memory, in 512-byte sectors, shared so the test can inspect the
/// bytes the filesystem wrote.
#[derive(Clone)]
struct MemoryDevice {
    bytes: Rc<RefCell<Vec<u8>>>,
}

impl MemoryDevice {
    fn new(bytes: Vec<u8>) -> Self {
        MemoryDevice {
            bytes: Rc::new(RefCell::new(bytes)),
        }
    }

    fn bytes(&self) -> Vec<u8> {
        self.bytes.borrow().clone()
    }
}

impl SectorDevice for MemoryDevice {
    fn sector_count(&self) -> u64 {
        (self.bytes.borrow().len() / 512) as u64
    }

    fn read_sector(&self, index: u64, out: &mut [u8; 512]) -> Result<(), Status> {
        let start = usize::try_from(index).map_err(|_| Status::IoError)? * 512;
        let end = start.checked_add(512).ok_or(Status::IoError)?;
        let bytes = self.bytes.borrow();
        let slice = bytes.get(start..end).ok_or(Status::IoError)?;
        out.copy_from_slice(slice);
        Ok(())
    }

    fn write_sector(&mut self, index: u64, data: &[u8; 512]) -> Result<(), Status> {
        let start = usize::try_from(index).map_err(|_| Status::IoError)? * 512;
        let end = start.checked_add(512).ok_or(Status::IoError)?;
        let mut bytes = self.bytes.borrow_mut();
        let slice = bytes.get_mut(start..end).ok_or(Status::IoError)?;
        slice.copy_from_slice(data);
        Ok(())
    }
}

/// A temporary directory removed on drop.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "igis-ext2w-write-{}-{}-{tag}",
            std::process::id(),
            unique
        ));
        std::fs::create_dir_all(&path).expect("create temp dir");
        TempDir { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn mke2fs_available() -> bool {
    Command::new("mke2fs").arg("-V").output().is_ok()
}

fn e2fsck_available() -> bool {
    Command::new("e2fsck").arg("-V").output().is_ok()
}

/// Builds a fresh 1 MiB ext2 image from a staging tree, or returns `None` when
/// `mke2fs` is unavailable.
fn make_image() -> Option<Vec<u8>> {
    if !mke2fs_available() {
        eprintln!("ext2w: skipping test: `mke2fs` is not installed");
        return None;
    }

    let temp = TempDir::new("image");
    let staging = temp.path().join("root");
    std::fs::create_dir_all(staging.join("sub")).expect("create staging tree");
    std::fs::write(staging.join("readme.txt"), README).expect("write readme");
    std::fs::write(staging.join("sub").join("nested.txt"), b"nested\n").expect("write nested");

    let image = temp.path().join("home.img");
    let output = Command::new("mke2fs")
        .arg("-q")
        .arg("-t")
        .arg("ext2")
        .arg("-b")
        .arg("1024")
        .arg("-d")
        .arg(&staging)
        .arg("-F")
        .arg(&image)
        .arg("1024K")
        .output()
        .expect("run mke2fs");
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!("mke2fs failed ({}): {stderr}", output.status);
    }
    Some(std::fs::read(&image).expect("read image"))
}

/// Runs `e2fsck -fn` on the image bytes and fails the test unless it exits 0.
fn assert_e2fsck_clean(bytes: &[u8], label: &str) {
    if !e2fsck_available() {
        eprintln!("ext2w: skipping e2fsck check `{label}`: `e2fsck` is not installed");
        return;
    }
    let temp = TempDir::new("fsck");
    let image = temp.path().join("home.img");
    std::fs::write(&image, bytes).expect("write image for e2fsck");
    let output = Command::new("e2fsck")
        .arg("-fn")
        .arg(&image)
        .output()
        .expect("run e2fsck");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "e2fsck -fn failed for `{label}` ({}):\n{stdout}{stderr}",
        output.status
    );
}

fn list_names<D: SectorDevice>(fs: &ext2w::Fs<D>, path: &str) -> Vec<(String, Entry)> {
    let mut name = [0u8; 64];
    let mut names = Vec::new();
    let mut index = 0u32;
    loop {
        match fs.list(path, index, &mut name) {
            Ok(entry) => {
                names.push((
                    String::from_utf8(name[..entry.name_len].to_vec()).expect("utf-8 name"),
                    entry,
                ));
                index += 1;
            }
            Err(Status::NotFound) => break,
            Err(status) => panic!("listing `{path}` failed: {status:?}"),
        }
    }
    names
}

fn read_whole<D: SectorDevice>(fs: &ext2w::Fs<D>, path: &str, len: usize) -> Vec<u8> {
    let mut buffer = vec![0u8; len];
    let read = fs.read_file(path, &mut buffer).expect("read whole file");
    assert_eq!(read, len, "read length for `{path}`");
    buffer
}

#[test]
fn mount_write_marks_the_filesystem_clean() {
    let Some(bytes) = make_image() else {
        return;
    };
    let device = MemoryDevice::new(bytes);
    let fs = mount_write(device.clone()).expect("image mounts writable");
    drop(fs);
    // The mount alone is a clean no-op for e2fsck: the superblock is rewritten
    // with `s_state = 1` and no counter changed.
    assert_e2fsck_clean(&device.bytes(), "clean mount");
}

#[test]
fn creates_writes_and_reads_a_file() {
    let Some(bytes) = make_image() else {
        return;
    };
    let device = MemoryDevice::new(bytes);
    let mut fs = mount_write(device.clone()).expect("image mounts writable");

    let free_blocks = fs.free_blocks();
    let free_inodes = fs.free_inodes();
    let inode = fs.create("written.txt").expect("create");
    assert!(inode > 0);

    let first = b"igis home write\n";
    assert_eq!(fs.write_at("written.txt", 0, first), Ok(first.len()));
    assert_eq!(fs.file_size("written.txt"), Ok(first.len() as u64));
    assert_eq!(read_whole(&fs, "written.txt", first.len()), first);
    assert_eq!(fs.free_inodes(), free_inodes - 1);
    assert_eq!(fs.free_blocks(), free_blocks - 1);

    // A second create of the same name is refused and changes nothing.
    assert_eq!(fs.create("written.txt"), Err(Status::Invalid));
    assert_eq!(fs.free_inodes(), free_inodes - 1);

    // A missing parent is NotFound; a directory target is Invalid.
    assert_eq!(fs.create("nope/child.txt"), Err(Status::NotFound));
    assert_eq!(fs.create("sub"), Err(Status::Invalid));
    assert_eq!(
        fs.write_at("sub", 0, b"x"),
        Err(Status::Invalid),
        "a directory is not a writable file"
    );
    assert_eq!(
        fs.write_at("missing.txt", 0, b"x"),
        Err(Status::NotFound),
        "write does not create; the API layer does that at offset 0"
    );

    // The listing sees the new entry with its type and size.
    let entries = list_names(&fs, "");
    let written = entries
        .iter()
        .find(|(name, _)| name == "written.txt")
        .expect("written.txt in root");
    assert_eq!(written.1.kind, abi::fs::KIND_FILE);
    assert_eq!(written.1.size, first.len() as u64);
    drop(fs);
    assert_e2fsck_clean(&device.bytes(), "create and write");
}

#[test]
fn overwrites_and_appends_preserving_the_rest() {
    let Some(bytes) = make_image() else {
        return;
    };
    let device = MemoryDevice::new(bytes);
    let mut fs = mount_write(device.clone()).expect("image mounts writable");

    fs.create("file.txt").expect("create");
    let original = b"0123456789abcdef";
    assert_eq!(fs.write_at("file.txt", 0, original), Ok(16));

    // Overwrite 4 bytes in the middle: the size and the other bytes stay.
    assert_eq!(fs.write_at("file.txt", 6, b"WXYZ"), Ok(4));
    assert_eq!(fs.file_size("file.txt"), Ok(16));
    assert_eq!(read_whole(&fs, "file.txt", 16), b"012345WXYZabcdef");

    // Append at the end.
    assert_eq!(fs.write_at("file.txt", 16, b"+-"), Ok(2));
    assert_eq!(fs.file_size("file.txt"), Ok(18));
    assert_eq!(read_whole(&fs, "file.txt", 18), b"012345WXYZabcdef+-");

    // Extend within the same block: the gap is zero-filled.
    assert_eq!(fs.write_at("file.txt", 22, b"!"), Ok(1));
    assert_eq!(fs.file_size("file.txt"), Ok(23));
    let contents = read_whole(&fs, "file.txt", 23);
    assert_eq!(&contents[..18], b"012345WXYZabcdef+-");
    assert_eq!(&contents[18..22], &[0, 0, 0, 0]);
    assert_eq!(contents[22], b'!');
    drop(fs);
    assert_e2fsck_clean(&device.bytes(), "overwrite, append and gap");
}

#[test]
fn a_write_across_blocks_grows_the_file_and_zero_fills_the_gap() {
    let Some(bytes) = make_image() else {
        return;
    };
    let device = MemoryDevice::new(bytes);
    let mut fs = mount_write(device.clone()).expect("image mounts writable");
    fs.create("sparse.txt").expect("create");

    // 3000 bytes at offset 0 crosses two block boundaries.
    let payload: Vec<u8> = (0..3000).map(|index| (index % 251) as u8).collect();
    assert_eq!(fs.write_at("sparse.txt", 0, &payload), Ok(3000));
    assert_eq!(read_whole(&fs, "sparse.txt", 3000), payload);

    // A write at 5000 leaves 2000 zero bytes between the old end and the write.
    assert_eq!(fs.write_at("sparse.txt", 5000, b"tail"), Ok(4));
    assert_eq!(fs.file_size("sparse.txt"), Ok(5004));
    let contents = read_whole(&fs, "sparse.txt", 5004);
    assert_eq!(&contents[..3000], &payload[..]);
    assert!(contents[3000..5000].iter().all(|byte| *byte == 0));
    assert_eq!(&contents[5000..], b"tail");

    // Overwrite the block that held the old end: the bytes around stay.
    assert_eq!(fs.write_at("sparse.txt", 2900, b"mend"), Ok(4));
    let contents = read_whole(&fs, "sparse.txt", 5004);
    assert_eq!(&contents[2896..2900], &payload[2896..2900]);
    assert_eq!(&contents[2900..2904], b"mend");
    assert_eq!(&contents[2904..3000], &payload[2904..3000]);
    drop(fs);
    assert_e2fsck_clean(&device.bytes(), "multi-block write and gap");
}

#[test]
fn indirect_and_double_indirect_blocks_are_allocated() {
    let Some(bytes) = make_image() else {
        return;
    };
    let device = MemoryDevice::new(bytes);
    let mut fs = mount_write(device.clone()).expect("image mounts writable");
    fs.create("big.bin").expect("create");

    // 300 KiB covers the 12 direct pointers, the 256 single-indirect pointers
    // and the start of the double-indirect tree.
    let payload: Vec<u8> = (0..300 * 1024).map(|index| (index % 253) as u8).collect();
    assert_eq!(fs.write_at("big.bin", 0, &payload), Ok(payload.len()));
    assert_eq!(fs.file_size("big.bin"), Ok(payload.len() as u64));
    assert_eq!(read_whole(&fs, "big.bin", payload.len()), payload);

    // A second write deep inside the double-indirect region is preserved.
    assert_eq!(fs.write_at("big.bin", 280 * 1024, b"deep"), Ok(4));
    let contents = read_whole(&fs, "big.bin", payload.len());
    assert_eq!(&contents[280 * 1024..280 * 1024 + 4], b"deep");
    assert_eq!(&contents[0..1024], &payload[0..1024]);
    drop(fs);
    assert_e2fsck_clean(&device.bytes(), "indirect and double indirect");
}

#[test]
fn a_write_past_the_double_indirect_reach_is_refused() {
    let Some(bytes) = make_image() else {
        return;
    };
    let device = MemoryDevice::new(bytes);
    let mut fs = mount_write(device).expect("image mounts writable");
    fs.create("limit.txt").expect("create");
    let free = fs.free_blocks();

    // 12 + 256 + 65536 blocks at 1 KiB.
    let limit = (12u64 + 256 + 65536) * 1024;
    assert_eq!(
        fs.write_at("limit.txt", limit, b"x"),
        Err(Status::CapacityExceeded)
    );
    assert_eq!(
        fs.write_at("limit.txt", u64::MAX, b"x"),
        Err(Status::CapacityExceeded)
    );
    assert_eq!(fs.free_blocks(), free, "a refused write allocates nothing");
    assert_eq!(fs.file_size("limit.txt"), Ok(0));
}

#[test]
fn a_full_filesystem_is_capacity_exceeded_and_leaves_the_tree_intact() {
    let Some(bytes) = make_image() else {
        return;
    };
    let device = MemoryDevice::new(bytes);
    let mut fs = mount_write(device.clone()).expect("image mounts writable");
    fs.create("filler.bin").expect("create");

    // Fill the image a block at a time until the writer refuses. Each
    // successful write consumes exactly one data block.
    let chunk = [0x5Au8; 1024];
    let mut offset = 0u64;
    let mut writes = 0u32;
    loop {
        match fs.write_at("filler.bin", offset, &chunk) {
            Ok(count) => {
                assert_eq!(count, 1024);
                offset += 1024;
                writes += 1;
            }
            Err(Status::CapacityExceeded) => break,
            Err(other) => panic!("unexpected status while filling: {other:?}"),
        }
        assert!(writes < 4096, "the image never filled");
    }
    assert!(writes > 100, "the fixture is too small to be interesting");
    assert_eq!(fs.free_blocks(), 0);
    assert_eq!(fs.file_size("filler.bin"), Ok(offset));

    // The refused write changed nothing: the size and free count hold, and a
    // new file cannot be created either.
    assert_eq!(
        fs.write_at("filler.bin", offset, &chunk),
        Err(Status::CapacityExceeded)
    );
    assert_eq!(fs.file_size("filler.bin"), Ok(offset));
    assert_eq!(fs.free_blocks(), 0);
    // A new *empty* file may still fit when the directory has a free record and
    // an inode is left (an inode costs no data block); writing to it must not.
    match fs.create("another.txt") {
        Ok(_) => assert_eq!(
            fs.write_at("another.txt", 0, b"x"),
            Err(Status::CapacityExceeded)
        ),
        Err(status) => assert_eq!(status, Status::CapacityExceeded),
    }

    // The bytes that did land are still there.
    let mut head = [0u8; 1024];
    assert_eq!(fs.read_at("filler.bin", 0, &mut head), Ok(1024));
    assert!(head.iter().all(|byte| *byte == 0x5A));
    drop(fs);
    assert_e2fsck_clean(&device.bytes(), "full filesystem");
}

#[test]
fn a_read_only_mount_refuses_every_write() {
    let Some(bytes) = make_image() else {
        return;
    };
    let device = MemoryDevice::new(bytes.clone());
    let mut fs = mount(device.clone()).expect("image mounts read-only");

    assert_eq!(fs.create("new.txt"), Err(Status::Invalid));
    assert_eq!(fs.write_at("readme.txt", 0, b"x"), Err(Status::Invalid));
    // Nothing was written, not even the superblock.
    assert_eq!(device.bytes(), bytes);
}

#[test]
fn an_inconsistent_free_count_is_refused() {
    let Some(mut bytes) = make_image() else {
        return;
    };
    // Claim more free blocks than the filesystem has: the writer must not
    // trust a superblock it cannot reconcile with the bitmaps.
    bytes[1024 + 12..1024 + 16].copy_from_slice(&u32::MAX.to_le_bytes());
    match mount_write(MemoryDevice::new(bytes)) {
        Err(Status::Invalid) => {}
        Err(other) => panic!("inconsistent free count should be Invalid, got {other:?}"),
        Ok(_) => panic!("inconsistent free count mounted"),
    }
}
