//! One path over the read-only application volume and the optional writable
//! home filesystem (spec 0019, A1–A5; spec 0025, B6–B9; ADR 0040, ADR 0059).
//!
//! The application image is a read-only `ext2w` mount over the ATA volume on
//! the primary slave; the home filesystem is a writable `ext2w` mount over the
//! ATA home disk on the primary master. Nothing above this module knows there
//! are two filesystems:
//!
//! ```text
//! home          -> the home mount's root (or the app volume's `home` when absent)
//! home/...      -> the home mount (or the app volume when absent)
//! everything else -> the read-only application volume
//! ```
//!
//! `/` is listed from the application volume, which contains `home`, and
//! `home` is listed from the home mount when one is mounted. Only `home/...`
//! is writable; every other path refuses a write with `Invalid`.
//!
//! # Locks
//!
//! [`APPS`] and [`HOME`] each serialise one `ext2w` mount and its block cache
//! across cores. A call takes exactly one of them, holds it for its own
//! duration and never takes the other; the lock order is therefore
//! `APPS|HOME -> ATA channel`, and no console, window, graphics or scheduler
//! lock is taken under a mount lock. Both mounts share the ATA driver's channel
//! lock inside `read_sector`/`write_sector`, so register sequences from the two
//! units cannot interleave (spec 0025, B5/B9). A write holds [`HOME`] from the
//! implicit create through the inode update, and the writer is synchronous, so
//! the lock is never a write-back queue (spec 0019, H4).

use abi::Status;

use crate::ata::AtaDisk;
use crate::println;
use crate::sync::SpinLock;

/// The read-only application volume, published once before the APs start.
static APPS: SpinLock<Option<ext2w::Fs<AtaDisk>>> = SpinLock::new(None);

/// The optional writable home mount, serialised for SMP.
static HOME: SpinLock<Option<ext2w::Fs<AtaDisk>>> = SpinLock::new(None);

/// Publishes the read-only application volume. Called once on the boot core
/// after the mount and before any core can service a request.
pub fn init(apps: ext2w::Fs<AtaDisk>) {
    *APPS.lock() = Some(apps);
}

/// Runs `read` under the application mount lock.
///
/// A call locks, borrows the published mount and finishes inside the guard, so
/// exactly one mount lock is held (spec 0025, B9). A call before the boot core
/// publishes the mount is a boot bug: it says so and halts rather than reading
/// through a missing mount.
fn with_apps<T>(read: impl FnOnce(&ext2w::Fs<AtaDisk>) -> Result<T, Status>) -> Result<T, Status> {
    let apps = APPS.lock();
    match apps.as_ref() {
        Some(apps) => read(apps),
        None => {
            println!("fs: no application image published; this core halts");
            arch_x86_64::halt();
        }
    }
}

/// Tries to mount the home disk and runs the boot self-check.
///
/// An absent drive prints `ext2w: no home disk`; a present but invalid image
/// prints `ext2w: home failed` and leaves the mount read-only. A failed
/// self-check also leaves the mount read-only, so a boot never trusts a tree it
/// could not read. None of these cases halts: the system boots read-only, which
/// is exactly the no-disk behaviour (spec 0019, H3).
///
/// A successful mount is the writable one (spec 0019, H5): the filesystem is
/// marked clean when it opens. The self-check itself never writes, so a boot
/// leaves no trace on the disk.
pub fn mount_home() {
    let Some(disk) = crate::ata::init(crate::ata::Unit::Master) else {
        println!("ext2w: no home disk");
        return;
    };
    let home = match ext2w::mount_write(disk) {
        Ok(home) => home,
        Err(status) => {
            println!("ext2w: home failed ({})", status.name());
            return;
        }
    };
    println!(
        "ext2w: mounted, {} byte blocks, {} KiB",
        home.block_size(),
        home.size_bytes() / 1024
    );
    match home_self_check(&home) {
        Ok(()) => {
            *HOME.lock() = Some(home);
            println!("ext2w: home ok");
        }
        Err(step) => println!("ext2w: home FAIL at step {step}"),
    }
}

/// Reads the initial file and lists the root of a freshly mounted home image.
///
/// `readme.txt` from `samples/home/` must be present. The check runs on the
/// local mount before it is published, so a bad tree is never served; it
/// returns the failed step number. It is deliberately read-only: a scratch
/// write here would either persist or need a removal primitive (M6.3), and
/// `base/home_write` is the write proof.
fn home_self_check(home: &ext2w::Fs<AtaDisk>) -> Result<(), u32> {
    let size = match home.file_size("readme.txt") {
        Ok(size) if size > 0 => size,
        _ => return Err(1),
    };
    if size > 1024 {
        return Err(2);
    }
    let mut contents = [0u8; 1024];
    let read = home
        .read_file("readme.txt", &mut contents)
        .map_err(|_| 2u32)?;
    if read != size as usize {
        return Err(2);
    }
    // The staged file starts with the system name; this catches a mount that
    // reads sector data from the wrong place.
    if !contents[..read].starts_with(b"igis-v2 home") {
        return Err(3);
    }

    let mut name = [0u8; abi::fs::NAME_CAP];
    let mut found_readme = false;
    let mut entries = 0u32;
    let mut index = 0u32;
    loop {
        match home.list("", index, &mut name) {
            Ok(entry) => {
                if entry.name_len == 10
                    && &name[..entry.name_len] == b"readme.txt"
                    && entry.kind == abi::fs::KIND_FILE
                {
                    found_readme = true;
                }
                entries += 1;
                index += 1;
                if index > 64 {
                    return Err(4);
                }
            }
            Err(Status::NotFound) => break,
            Err(_) => return Err(4),
        }
    }
    if entries == 0 || !found_readme {
        return Err(4);
    }
    Ok(())
}

/// The application volume tree boot check (spec 0018, L1/L6; spec 0025, B10).
///
/// Proves the path walk before the router serves anything: `base/shell` exists
/// and is a regular non-empty file, its first byte streams, and the root lists
/// the six staged directories (`base`, `apps`, `home`, `lib`, `system`,
/// `image`). Returns the failed step number; the caller halts.
pub fn tree_self_check(apps: &ext2w::Fs<AtaDisk>) -> Result<(), u32> {
    let mut name = [0u8; abi::fs::NAME_CAP];

    // 1: base/shell exists and is a regular file.
    match apps.file_size("base/shell") {
        Ok(size) if size > 0 => {}
        _ => return Err(1),
    }

    // 2: the first byte of it can be streamed.
    let mut byte = [0u8; 1];
    match apps.read_at("base/shell", 0, &mut byte) {
        Ok(1) => {}
        _ => return Err(2),
    }

    // 3: the root lists the six directories (and mke2fs's lost+found).
    let mut seen = 0u8;
    let mut index = 0u32;
    loop {
        match apps.list("", index, &mut name) {
            Ok(entry) => {
                if entry.kind != abi::fs::KIND_DIR {
                    return Err(3);
                }
                for (bit, directory) in ["base", "apps", "home", "lib", "system", "image"]
                    .iter()
                    .enumerate()
                {
                    if &name[..entry.name_len] == directory.as_bytes() {
                        seen |= 1 << bit;
                    }
                }
                index += 1;
                if index > 16 {
                    return Err(3);
                }
            }
            Err(Status::NotFound) => break,
            Err(_) => return Err(3),
        }
    }
    if seen != 0b11_1111 {
        return Err(3);
    }

    Ok(())
}

/// Splits `home` / `home/...` into the path local to the home mount. Returns
/// `None` for every other path, which stays on the application volume.
fn home_local(path: &str) -> Option<&str> {
    if path == "home" {
        Some("")
    } else {
        path.strip_prefix("home/")
    }
}

/// Reads a whole file through the router.
pub fn read_file(path: &str, out: &mut [u8]) -> Result<usize, Status> {
    if let Some(local) = home_local(path) {
        let home = HOME.lock();
        if let Some(home) = home.as_ref() {
            return home.read_file(local, out);
        }
    }
    self::with_apps(|apps| apps.read_file(path, out))
}

/// Reads at an offset through the router (spec 0013, `core.fs_read`).
pub fn read_at(path: &str, offset: u64, out: &mut [u8]) -> Result<usize, Status> {
    if let Some(local) = home_local(path) {
        let home = HOME.lock();
        if let Some(home) = home.as_ref() {
            return home.read_at(local, offset, out);
        }
    }
    self::with_apps(|apps| apps.read_at(path, offset, out))
}

/// Returns a regular file's size through the router.
pub fn file_size(path: &str) -> Result<u64, Status> {
    if let Some(local) = home_local(path) {
        let home = HOME.lock();
        if let Some(home) = home.as_ref() {
            return home.file_size(local);
        }
    }
    self::with_apps(|apps| apps.file_size(path))
}

/// Writes at an offset through the router (spec 0019, A2/A3).
///
/// Only a `home/...` path is writable, and only when the home disk is mounted;
/// every other path is [`Status::Invalid`]. A missing file is created
/// implicitly when `offset` is `0` and only then (A3), which is why the create
/// and the write are two writer calls here rather than two API primitives. The
/// returned count is the number of bytes written.
pub fn write_at(path: &str, offset: u64, data: &[u8]) -> Result<usize, Status> {
    let Some(local) = home_local(path) else {
        return Err(Status::Invalid);
    };
    let mut home = HOME.lock();
    let Some(home) = home.as_mut() else {
        return Err(Status::Invalid);
    };
    match home.write_at(local, offset, data) {
        Err(Status::NotFound) if offset == 0 => {
            home.create(local)?;
            home.write_at(local, offset, data)
        }
        result => result,
    }
}

/// Removes a regular file or empty directory through the router (spec 0019,
/// A2/W3/W5). Only a mounted `home/...` path is accepted.
pub fn remove(path: &str) -> Result<(), Status> {
    let Some(local) = home_local(path) else {
        return Err(Status::Invalid);
    };
    let mut home = HOME.lock();
    let Some(home) = home.as_mut() else {
        return Err(Status::Invalid);
    };
    home.remove(local)
}

/// Creates a directory through the router (spec 0019, A2/W5). Only a mounted
/// `home/...` path is accepted.
pub fn mkdir(path: &str) -> Result<(), Status> {
    let Some(local) = home_local(path) else {
        return Err(Status::Invalid);
    };
    let mut home = HOME.lock();
    let Some(home) = home.as_mut() else {
        return Err(Status::Invalid);
    };
    home.mkdir(local).map(|_| ())
}

/// Renames a file or directory through the router (spec 0019, A2/W3/W6). Both
/// paths must name the mounted home filesystem.
pub fn rename(from: &str, to: &str) -> Result<(), Status> {
    let (Some(from_local), Some(to_local)) = (home_local(from), home_local(to)) else {
        return Err(Status::Invalid);
    };
    let mut home = HOME.lock();
    let Some(home) = home.as_mut() else {
        return Err(Status::Invalid);
    };
    home.rename(from_local, to_local)
}

/// Sets a regular file's length through the router (spec 0019, A7; ADR 0041).
/// Only a mounted `home/...` path is accepted.
pub fn truncate(path: &str, size: u64) -> Result<(), Status> {
    let Some(local) = home_local(path) else {
        return Err(Status::Invalid);
    };
    let mut home = HOME.lock();
    let Some(home) = home.as_mut() else {
        return Err(Status::Invalid);
    };
    home.truncate(local, size)
}

/// Returns one directory entry through the router (spec 0013, `core.fs_list`).
pub fn list(path: &str, index: u32, name: &mut [u8]) -> Result<ext2w::Entry, Status> {
    if let Some(local) = home_local(path) {
        let home = HOME.lock();
        if let Some(home) = home.as_ref() {
            return home.list(local, index, name);
        }
    }
    self::with_apps(|apps| apps.list(path, index, name))
}
