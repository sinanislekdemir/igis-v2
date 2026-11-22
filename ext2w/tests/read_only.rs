//! Host tests for the read-only ext2 mount (spec 0019, W1/W2).
//!
//! Each test builds a real 1 MiB ext2 image with `mke2fs -d` from a temporary
//! staging tree, loads it into a `Vec`-backed [`SectorDevice`], and exercises
//! the reader against the filesystem the host wrote. If `mke2fs` is not
//! installed the test prints a clear skip message and returns; the harness has
//! no skip primitive.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use abi::Status;
use ext2w::{mount, Entry, SectorDevice};

const README: &[u8] = b"igis-v2 home\n\nA host-side ext2 fixture for the ext2w reader.\n";

/// An image in memory, in 512-byte sectors.
struct MemoryDevice {
    bytes: Vec<u8>,
}

impl MemoryDevice {
    fn new(bytes: Vec<u8>) -> Self {
        MemoryDevice { bytes }
    }
}

impl SectorDevice for MemoryDevice {
    fn sector_count(&self) -> u64 {
        (self.bytes.len() / 512) as u64
    }

    fn read_sector(&self, index: u64, out: &mut [u8; 512]) -> Result<(), Status> {
        let start = usize::try_from(index).map_err(|_| Status::IoError)? * 512;
        let end = start.checked_add(512).ok_or(Status::IoError)?;
        let slice = self.bytes.get(start..end).ok_or(Status::IoError)?;
        out.copy_from_slice(slice);
        Ok(())
    }

    fn write_sector(&mut self, index: u64, data: &[u8; 512]) -> Result<(), Status> {
        let start = usize::try_from(index).map_err(|_| Status::IoError)? * 512;
        let end = start.checked_add(512).ok_or(Status::IoError)?;
        let slice = self.bytes.get_mut(start..end).ok_or(Status::IoError)?;
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
            "igis-ext2w-{}-{}-{tag}",
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

/// Builds a fresh 1 MiB ext2 image from a staging tree, or returns `None` when
/// `mke2fs` is unavailable (with a message, so a skipped run is visible).
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

/// Builds an ext2 image of `size_kib` KiB holding one regular file, or returns
/// `None` when `mke2fs` is unavailable.
fn make_image_with_file(size_kib: u32, name: &str, contents: &[u8]) -> Option<Vec<u8>> {
    if !mke2fs_available() {
        eprintln!("ext2w: skipping test: `mke2fs` is not installed");
        return None;
    }

    let temp = TempDir::new("large-image");
    let staging = temp.path().join("root");
    std::fs::create_dir_all(&staging).expect("create staging tree");
    std::fs::write(staging.join(name), contents).expect("write large file");

    let image = temp.path().join("large.img");
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
        .arg(format!("{size_kib}K"))
        .output()
        .expect("run mke2fs");
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!("mke2fs failed ({}): {stderr}", output.status);
    }
    Some(std::fs::read(&image).expect("read image"))
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

#[test]
fn mounts_and_lists_the_root() {
    let Some(bytes) = make_image() else {
        return;
    };
    let fs = mount(MemoryDevice::new(bytes)).expect("image mounts");

    assert_eq!(fs.block_size(), 1024);
    assert_eq!(fs.size_bytes(), 1024 * 1024);

    let entries = list_names(&fs, "");
    let readme = entries
        .iter()
        .find(|(name, _)| name == "readme.txt")
        .expect("readme.txt in root");
    assert_eq!(readme.1.kind, abi::fs::KIND_FILE);
    assert_eq!(readme.1.size, README.len() as u64);
    let sub = entries
        .iter()
        .find(|(name, _)| name == "sub")
        .expect("sub in root");
    assert_eq!(sub.1.kind, abi::fs::KIND_DIR);

    // The last index is NotFound, not a panic or a silent stop.
    let mut name = [0u8; 64];
    assert_eq!(
        fs.list("", entries.len() as u32, &mut name),
        Err(Status::NotFound)
    );
}

#[test]
fn reads_a_file_by_offset_and_whole() {
    let Some(bytes) = make_image() else {
        return;
    };
    let fs = mount(MemoryDevice::new(bytes)).expect("image mounts");

    assert_eq!(fs.file_size("readme.txt"), Ok(README.len() as u64));

    let mut whole = [0u8; 256];
    let read = fs.read_file("readme.txt", &mut whole).expect("whole read");
    assert_eq!(read, README.len());
    assert_eq!(&whole[..read], README);

    let mut chunk = [0u8; 16];
    assert_eq!(fs.read_at("readme.txt", 0, &mut chunk), Ok(16));
    assert_eq!(&chunk[..], &README[..16]);

    let mut one = [0u8; 1];
    assert_eq!(fs.read_at("readme.txt", 9, &mut one), Ok(1));
    assert_eq!(one[0], README[9]);

    // At and past the end is Ok(0), not an error.
    assert_eq!(
        fs.read_at("readme.txt", README.len() as u64, &mut chunk),
        Ok(0)
    );
    assert_eq!(
        fs.read_at("readme.txt", README.len() as u64 + 100, &mut chunk),
        Ok(0)
    );

    // A destination smaller than the file is CapacityExceeded.
    let mut small = [0u8; 4];
    assert_eq!(
        fs.read_file("readme.txt", &mut small),
        Err(Status::CapacityExceeded)
    );
}

fn large_payload() -> Vec<u8> {
    (0..2 * 1024 * 1024 + 37)
        .map(|index| ((index / 3) ^ (index * 7)) as u8)
        .collect()
}

/// A 2 MiB file needs the double-indirect pointer (direct covers 12 blocks,
/// single indirect 256), so offsets in all three ranges must resolve without
/// walking the blocks before them (spec 0025 B19).
#[test]
fn reads_past_the_direct_blocks_by_offset() {
    let payload = large_payload();
    let Some(bytes) = make_image_with_file(8 * 1024, "large.bin", &payload) else {
        return;
    };
    let fs = mount(MemoryDevice::new(bytes)).expect("image mounts");
    assert_eq!(fs.file_size("large.bin"), Ok(payload.len() as u64));

    // Sequential baselines: the whole-file reader and a chunked read_at loop.
    let mut whole = vec![0u8; payload.len()];
    assert_eq!(fs.read_file("large.bin", &mut whole), Ok(payload.len()));
    assert_eq!(whole, payload, "read_file differs from the staged bytes");

    let mut sequential = Vec::with_capacity(payload.len());
    let mut offset = 0u64;
    let mut chunk = [0u8; 777];
    while offset < payload.len() as u64 {
        let read = fs
            .read_at("large.bin", offset, &mut chunk)
            .expect("chunked read");
        assert!(read > 0 && read <= chunk.len());
        sequential.extend_from_slice(&chunk[..read]);
        offset += read as u64;
    }
    assert_eq!(sequential, payload, "chunked read_at differs from read_file");

    // Direct (blocks 0..12), single-indirect (12..268) and double-indirect
    // (268..) offsets return the sequential bytes.
    const DIRECT: u64 = 5 * 1024 + 11;
    const SINGLE: u64 = (12 + 200) * 1024 + 7;
    const DOUBLE: u64 = (12 + 256 + 1000) * 1024 + 3;
    for start in [
        0,
        1,
        1023,
        1024,
        DIRECT,
        SINGLE,
        DOUBLE,
        268 * 1024 - 8,
        payload.len() as u64 - 17,
    ] {
        let end = (start as usize + 300).min(payload.len());
        let mut buffer = [0u8; 300];
        let read = fs
            .read_at("large.bin", start, &mut buffer)
            .expect("offset read");
        assert_eq!(read, end - start as usize);
        assert_eq!(
            &buffer[..read],
            &payload[start as usize..end],
            "read at {start}"
        );
    }

    // One range crossing a block boundary in the double-indirect tree.
    let crossing = 268 * 1024 - 8;
    let mut buffer = [0u8; 32];
    assert_eq!(
        fs.read_at("large.bin", crossing as u64, &mut buffer),
        Ok(32)
    );
    assert_eq!(&buffer, &payload[crossing..crossing + 32]);

    // Three bytes from a large offset are exactly those bytes.
    let tail = payload.len() - 11;
    let mut three = [0u8; 3];
    assert_eq!(fs.read_at("large.bin", tail as u64, &mut three), Ok(3));
    assert_eq!(&three, &payload[tail..tail + 3]);

    // EOF and past EOF stay Ok(0); an empty destination does too.
    let mut at_eof = [0u8; 8];
    assert_eq!(
        fs.read_at("large.bin", payload.len() as u64, &mut at_eof),
        Ok(0)
    );
    assert_eq!(
        fs.read_at("large.bin", payload.len() as u64 + 100, &mut at_eof),
        Ok(0)
    );
    assert_eq!(fs.read_at("large.bin", 100, &mut []), Ok(0));
}

#[test]
fn missing_components_are_not_found() {
    let Some(bytes) = make_image() else {
        return;
    };
    let fs = mount(MemoryDevice::new(bytes)).expect("image mounts");

    let mut buffer = [0u8; 32];
    assert_eq!(fs.read_file("nope.txt", &mut buffer), Err(Status::NotFound));
    assert_eq!(fs.file_size("nope.txt"), Err(Status::NotFound));
    let mut name = [0u8; 64];
    assert_eq!(fs.list("nope", 0, &mut name), Err(Status::NotFound));
    assert_eq!(
        fs.find("sub/nope.txt").map(|(_, inode)| inode.size()),
        Err(Status::NotFound)
    );

    // Path syntax is the same rule the application API uses.
    assert_eq!(fs.file_size("../readme.txt"), Err(Status::Invalid));
    assert_eq!(fs.file_size("sub//nested.txt"), Err(Status::Invalid));
}

#[test]
fn a_directory_is_not_a_regular_file() {
    let Some(bytes) = make_image() else {
        return;
    };
    let fs = mount(MemoryDevice::new(bytes)).expect("image mounts");

    let mut buffer = [0u8; 256];
    assert_eq!(fs.read_file("sub", &mut buffer), Err(Status::Invalid));
    assert_eq!(fs.file_size("sub"), Err(Status::Invalid));
    assert_eq!(fs.read_file("", &mut buffer), Err(Status::Invalid));

    // A regular file is not a directory.
    let mut name = [0u8; 64];
    assert_eq!(fs.list("readme.txt", 0, &mut name), Err(Status::Invalid));

    let (_, nested) = fs.find("sub/nested.txt").expect("nested file");
    assert!(nested.is_file());
    let mut tiny = [0u8; 8];
    assert_eq!(fs.read_file("sub/nested.txt", &mut tiny), Ok(7));
    assert_eq!(&tiny[..7], b"nested\n");
}

#[test]
fn a_corrupt_magic_is_refused() {
    let Some(mut bytes) = make_image() else {
        return;
    };
    // `s_magic` is at byte 56 of the superblock, which starts at byte 1024.
    bytes[1024 + 56] = 0;
    bytes[1024 + 57] = 0;

    match mount(MemoryDevice::new(bytes)) {
        Err(Status::Invalid) => {}
        Err(other) => panic!("corrupt magic should be Invalid, got {other:?}"),
        Ok(_) => panic!("corrupt magic mounted"),
    }
}

#[test]
fn a_truncated_device_is_refused() {
    let Some(bytes) = make_image() else {
        return;
    };
    // Claim the whole image but hand the mount half of it.
    let truncated = bytes[..bytes.len() / 2].to_vec();
    match mount(MemoryDevice::new(truncated)) {
        Err(Status::Invalid) => {}
        Err(other) => panic!("truncated device should be Invalid, got {other:?}"),
        Ok(_) => panic!("truncated device mounted"),
    }
}
