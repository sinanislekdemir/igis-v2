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
    assert_eq!(fs.remove("readme.txt"), Err(Status::Invalid));
    assert_eq!(fs.mkdir("newdir"), Err(Status::Invalid));
    assert_eq!(fs.rename("readme.txt", "moved.txt"), Err(Status::Invalid));
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

#[test]
fn mkdir_creates_an_empty_directory_with_dot_and_dotdot() {
    let Some(bytes) = make_image() else {
        return;
    };
    let device = MemoryDevice::new(bytes);
    let mut fs = mount_write(device.clone()).expect("image mounts writable");
    let free_blocks = fs.free_blocks();
    let free_inodes = fs.free_inodes();

    let number = fs.mkdir("newdir").expect("mkdir");
    assert!(number > 0);
    let entries = list_names(&fs, "");
    let created = entries
        .iter()
        .find(|(name, _)| name == "newdir")
        .expect("newdir in root");
    assert_eq!(created.1.kind, abi::fs::KIND_DIR);
    // A fresh directory has one block and no live entry beyond `.`/`..`.
    assert_eq!(fs.list("newdir", 0, &mut [0u8; 64]), Err(Status::NotFound));
    assert_eq!(fs.free_inodes(), free_inodes - 1);
    assert_eq!(fs.free_blocks(), free_blocks - 1);

    // A file inside the new directory is created and read normally.
    fs.create("newdir/file.txt").expect("create in newdir");
    fs.write_at("newdir/file.txt", 0, b"inside\n").expect("write");
    assert_eq!(read_whole(&fs, "newdir/file.txt", 7), b"inside\n");

    // A duplicate name and a file parent are refused without changing counts.
    assert_eq!(fs.mkdir("newdir"), Err(Status::Invalid));
    assert_eq!(fs.mkdir("readme.txt/child"), Err(Status::Invalid));
    assert_eq!(fs.mkdir("missing/child"), Err(Status::NotFound));
    assert_eq!(fs.free_inodes(), free_inodes - 2);
    drop(fs);
    assert_e2fsck_clean(&device.bytes(), "mkdir");
}

#[test]
fn remove_frees_files_and_empty_directories_and_restores_the_counts() {
    let Some(bytes) = make_image() else {
        return;
    };
    let device = MemoryDevice::new(bytes);
    let mut fs = mount_write(device.clone()).expect("image mounts writable");
    let free_blocks = fs.free_blocks();
    let free_inodes = fs.free_inodes();

    // A file with data in a direct block.
    fs.create("gone.txt").expect("create");
    fs.write_at("gone.txt", 0, b"bye\n").expect("write");
    assert_eq!(fs.free_inodes(), free_inodes - 1);
    assert_eq!(fs.free_blocks(), free_blocks - 1);
    assert_eq!(fs.remove("gone.txt"), Ok(()));
    assert_eq!(fs.free_inodes(), free_inodes);
    assert_eq!(fs.free_blocks(), free_blocks);
    assert_eq!(fs.remove("gone.txt"), Err(Status::NotFound));
    assert!(list_names(&fs, "").iter().all(|(name, _)| name != "gone.txt"));

    // An empty directory goes; a non-empty one is refused until it is emptied.
    fs.mkdir("empty").expect("mkdir empty");
    assert_eq!(fs.remove("empty"), Ok(()));
    fs.mkdir("full").expect("mkdir full");
    fs.create("full/file").expect("create full/file");
    fs.write_at("full/file", 0, b"x").expect("write full/file");
    assert_eq!(fs.remove("full"), Err(Status::Invalid));
    assert_eq!(read_whole(&fs, "full/file", 1), b"x");
    assert_eq!(fs.remove("full/file"), Ok(()));
    assert_eq!(fs.remove("full"), Ok(()));
    assert_eq!(fs.free_blocks(), free_blocks);
    assert_eq!(fs.free_inodes(), free_inodes);

    // The root, `.` and `..` are never removable.
    assert_eq!(fs.remove(""), Err(Status::Invalid));
    assert_eq!(fs.remove("."), Err(Status::Invalid));
    assert_eq!(fs.remove(".."), Err(Status::Invalid));

    // Removing a fixture file also merges its record into the predecessor.
    assert_eq!(fs.remove("readme.txt"), Ok(()));
    assert!(list_names(&fs, "").iter().all(|(name, _)| name != "readme.txt"));
    drop(fs);
    assert_e2fsck_clean(&device.bytes(), "remove");
}

#[test]
fn rename_moves_files_replaces_regular_targets_and_refuses_directories() {
    let Some(bytes) = make_image() else {
        return;
    };
    let device = MemoryDevice::new(bytes);
    let mut fs = mount_write(device.clone()).expect("image mounts writable");

    fs.create("a.txt").expect("create a");
    fs.write_at("a.txt", 0, b"first\n").expect("write a");
    assert_eq!(fs.rename("a.txt", "b.txt"), Ok(()));
    assert_eq!(fs.file_size("a.txt"), Err(Status::NotFound));
    assert_eq!(read_whole(&fs, "b.txt", 6), b"first\n");
    assert_eq!(fs.rename("b.txt", "b.txt"), Ok(()), "same path is a no-op");

    // Cross-directory move keeps the inode and its data.
    assert_eq!(fs.rename("b.txt", "sub/c.txt"), Ok(()));
    assert_eq!(read_whole(&fs, "sub/c.txt", 6), b"first\n");

    // Replacing an existing regular file unlinks the target first.
    fs.create("d.txt").expect("create d");
    fs.write_at("d.txt", 0, b"old target\n").expect("write d");
    assert_eq!(fs.rename("sub/c.txt", "d.txt"), Ok(()));
    assert_eq!(read_whole(&fs, "d.txt", 6), b"first\n");
    assert_eq!(fs.file_size("d.txt"), Ok(6));
    assert_eq!(fs.file_size("sub/c.txt"), Err(Status::NotFound));

    // A replacement whose source and target share one directory: the target
    // unlink may compact that directory, so the source lookup must re-read it.
    fs.create("same_target").expect("create same_target");
    fs.write_at("same_target", 0, b"old\n").expect("write target");
    fs.create("same_source").expect("create same_source");
    fs.write_at("same_source", 0, b"new\n").expect("write source");
    assert_eq!(fs.rename("same_source", "same_target"), Ok(()));
    assert_eq!(read_whole(&fs, "same_target", 4), b"new\n");
    assert_eq!(fs.file_size("same_source"), Err(Status::NotFound));

    // Error paths: missing source, missing parent and a directory target.
    assert_eq!(fs.rename("missing.txt", "x.txt"), Err(Status::NotFound));
    assert_eq!(fs.rename("d.txt", "nope/x.txt"), Err(Status::NotFound));
    fs.mkdir("dir").expect("mkdir dir");
    assert_eq!(fs.rename("d.txt", "dir"), Err(Status::Invalid));
    assert_eq!(fs.rename("d.txt", ""), Err(Status::Invalid));
    assert_eq!(read_whole(&fs, "d.txt", 6), b"first\n");
    drop(fs);
    assert_e2fsck_clean(&device.bytes(), "rename files");
}

#[test]
fn truncate_shrinks_extends_and_zeroes_the_new_tail() {
    let Some(bytes) = make_image() else {
        return;
    };
    let device = MemoryDevice::new(bytes);
    let mut fs = mount_write(device.clone()).expect("image mounts writable");
    fs.create("cut.bin").expect("create");
    let payload: Vec<u8> = (0..3000).map(|index| (index % 251) as u8).collect();
    assert_eq!(fs.write_at("cut.bin", 0, &payload), Ok(3000));
    let after_write = fs.free_blocks();

    // Shrink to 1000 bytes keeps exactly one block.
    assert_eq!(fs.truncate("cut.bin", 1000), Ok(()));
    assert_eq!(fs.file_size("cut.bin"), Ok(1000));
    assert_eq!(read_whole(&fs, "cut.bin", 1000), payload[..1000]);
    assert_eq!(fs.free_blocks(), after_write + 2, "two blocks returned");

    // The current size is an `Ok` no-op.
    assert_eq!(fs.truncate("cut.bin", 1000), Ok(()));
    assert_eq!(fs.file_size("cut.bin"), Ok(1000));
    assert_eq!(fs.free_blocks(), after_write + 2);

    // Extend to 4000: the kept bytes stay and the new range is all zeros.
    assert_eq!(fs.truncate("cut.bin", 4000), Ok(()));
    assert_eq!(fs.file_size("cut.bin"), Ok(4000));
    let grown = read_whole(&fs, "cut.bin", 4000);
    assert_eq!(&grown[..1000], &payload[..1000]);
    assert!(grown[1000..].iter().all(|byte| *byte == 0));

    // A partial-block shrink must zero the tail of the kept block so the
    // extension reads zeros (not the old pattern) from the same block.
    assert_eq!(fs.truncate("cut.bin", 500), Ok(()));
    assert_eq!(fs.truncate("cut.bin", 900), Ok(()));
    let regrown = read_whole(&fs, "cut.bin", 900);
    assert_eq!(&regrown[..500], &payload[..500]);
    assert!(regrown[500..].iter().all(|byte| *byte == 0));

    // Error classes: a directory and a missing name.
    assert_eq!(fs.truncate("sub", 0), Err(Status::Invalid));
    assert_eq!(fs.truncate("missing.bin", 0), Err(Status::NotFound));
    assert_eq!(fs.file_size("missing.bin"), Err(Status::NotFound));

    // Truncating to zero empties the file and returns every block.
    assert_eq!(fs.truncate("cut.bin", 0), Ok(()));
    assert_eq!(fs.file_size("cut.bin"), Ok(0));
    let mut buffer = [0u8; 8];
    assert_eq!(fs.read_at("cut.bin", 0, &mut buffer), Ok(0));
    assert_eq!(fs.truncate("cut.bin", 0), Ok(()), "idempotent at zero");
    drop(fs);
    assert_e2fsck_clean(&device.bytes(), "truncate small file");
}

#[test]
fn truncate_crosses_the_single_and_double_indirect_boundaries() {
    let Some(bytes) = make_image() else {
        return;
    };
    let device = MemoryDevice::new(bytes);
    let mut fs = mount_write(device.clone()).expect("image mounts writable");
    fs.create("big.bin").expect("create");
    let before_write = fs.free_blocks();
    // 300 KiB: 12 direct, 256 single-indirect and 32 double-indirect blocks.
    let payload: Vec<u8> = (0..300 * 1024).map(|index| (index % 253) as u8).collect();
    assert_eq!(fs.write_at("big.bin", 0, &payload), Ok(payload.len()));
    let after_write = fs.free_blocks();

    // 280 KiB still uses the double-indirect tree but drops 20 data blocks.
    assert_eq!(fs.truncate("big.bin", 280 * 1024), Ok(()));
    assert_eq!(fs.file_size("big.bin"), Ok(280 * 1024));
    assert_eq!(
        read_whole(&fs, "big.bin", 280 * 1024),
        payload[..280 * 1024]
    );
    assert_eq!(fs.free_blocks(), after_write + 20);

    // 100 KiB removes the whole double-indirect tree (its outer and inner
    // pointer blocks are freed too).
    assert_eq!(fs.truncate("big.bin", 100 * 1024), Ok(()));
    assert_eq!(fs.file_size("big.bin"), Ok(100 * 1024));
    assert_eq!(
        read_whole(&fs, "big.bin", 100 * 1024),
        payload[..100 * 1024]
    );
    assert!(
        fs.free_blocks() >= after_write + 202,
        "200 data blocks and the double-indirect pointers return"
    );

    // 1 KiB then removes the single-indirect pointer block as well.
    assert_eq!(fs.truncate("big.bin", 1024), Ok(()));
    assert_eq!(read_whole(&fs, "big.bin", 1024), payload[..1024]);
    assert!(
        fs.free_blocks() >= after_write + 300,
        "all non-kept data and pointer blocks return"
    );

    // Extend back across both boundaries: every new byte is zero.
    assert_eq!(fs.truncate("big.bin", 300 * 1024), Ok(()));
    assert_eq!(fs.file_size("big.bin"), Ok(300 * 1024));
    let grown = read_whole(&fs, "big.bin", 300 * 1024);
    assert_eq!(&grown[..1024], &payload[..1024]);
    assert!(grown[1024..].iter().all(|byte| *byte == 0));

    assert_eq!(fs.truncate("big.bin", 0), Ok(()));
    assert_eq!(fs.free_blocks(), before_write, "every block returned");
    drop(fs);
    assert_e2fsck_clean(&device.bytes(), "truncate indirect boundaries");
}

#[test]
fn an_extension_beyond_the_free_blocks_is_capacity_exceeded_and_changes_nothing() {
    let Some(bytes) = make_image() else {
        return;
    };
    let device = MemoryDevice::new(bytes);
    let mut fs = mount_write(device.clone()).expect("image mounts writable");
    fs.create("too_big.bin").expect("create");
    let free = fs.free_blocks();
    // The 1 MiB fixture cannot hold 900 KiB of data blocks; capacity is
    // planned before the first allocation.
    let size = 900 * 1024u64;
    if size / 1024 <= fs.free_blocks() as u64 {
        eprintln!("ext2w: skipping capacity test: the fixture has room");
        return;
    }
    assert_eq!(
        fs.truncate("too_big.bin", size),
        Err(Status::CapacityExceeded)
    );
    assert_eq!(fs.file_size("too_big.bin"), Ok(0));
    assert_eq!(
        fs.free_blocks(),
        free,
        "a refused truncate allocates nothing"
    );
    drop(fs);
    assert_e2fsck_clean(&device.bytes(), "truncate capacity refusal");
}

#[test]
fn rename_moves_directories_updates_dotdot_and_link_counts() {
    let Some(bytes) = make_image() else {
        return;
    };
    let device = MemoryDevice::new(bytes);
    let mut fs = mount_write(device.clone()).expect("image mounts writable");

    // Same-directory directory rename.
    fs.mkdir("first").expect("mkdir first");
    fs.create("first/file.txt").expect("create inside");
    fs.write_at("first/file.txt", 0, b"kept\n").expect("write inside");
    assert_eq!(fs.rename("first", "second"), Ok(()));
    assert_eq!(read_whole(&fs, "second/file.txt", 5), b"kept\n");
    assert_eq!(fs.file_size("first/file.txt"), Err(Status::NotFound));

    // Cross-directory move: the moved directory's `..` and both parents' link
    // counts must stay consistent, which `e2fsck` checks.
    fs.mkdir("left").expect("mkdir left");
    fs.mkdir("right").expect("mkdir right");
    fs.mkdir("left/child").expect("mkdir left/child");
    fs.create("left/child/marker").expect("create marker");
    fs.write_at("left/child/marker", 0, b"m\n").expect("write marker");
    assert_eq!(fs.rename("left/child", "right/child"), Ok(()));
    assert_eq!(read_whole(&fs, "right/child/marker", 2), b"m\n");
    assert_eq!(fs.file_size("left/child/marker"), Err(Status::NotFound));
    // `..` of the moved directory now names `right`: creating in it works and
    // removing the moved tree leaves both parents consistent.
    assert_eq!(fs.remove("right/child"), Err(Status::Invalid));
    assert_eq!(fs.remove("right/child/marker"), Ok(()));
    assert_eq!(fs.remove("right/child"), Ok(()));
    assert_eq!(fs.remove("right"), Ok(()));
    assert_eq!(fs.remove("left"), Ok(()));

    // A directory cannot move into its own subtree, and a non-empty directory
    // cannot replace anything.
    fs.mkdir("outer").expect("mkdir outer");
    fs.mkdir("outer/inner").expect("mkdir outer/inner");
    assert_eq!(fs.rename("outer", "outer/inner/self"), Err(Status::Invalid));
    assert_eq!(fs.rename("second", "outer/inner"), Err(Status::Invalid));
    assert_eq!(fs.remove("outer/inner"), Ok(()));
    assert_eq!(fs.remove("outer"), Ok(()));
    drop(fs);
    assert_e2fsck_clean(&device.bytes(), "rename directories");
}
