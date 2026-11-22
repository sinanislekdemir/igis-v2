//! One path over the read-only application image and the optional writable
//! home filesystem (spec 0019, A1–A5; ADR 0040).
//!
//! The application image is the GRUB module the bootloader leaves in RAM; the
//! home filesystem is the `ext2w` mount over the ATA home disk. Nothing above
//! this module knows there are two filesystems:
//!
//! ```text
//! home          -> the home mount's root (or the app image's `home` when absent)
//! home/...      -> the home mount (or the app image when absent)
//! everything else -> the read-only application image
//! ```
//!
//! `/` is listed from the application image, which contains `home`, and `home`
//! is listed from the home mount when one is mounted. Only `home/...` is
//! writable; every other path refuses a write with `Invalid`.
//!
//! # Locks
//!
//! [`IMAGE`] is taken only to copy the published reference and is released
//! before any read runs. [`HOME`] serialises the one ATA device and its block
//! cache across cores; a filesystem call holds it for its own duration and this
//! module never takes it while another kernel lock is held. The lock order is
//! therefore `HOME → (ATA ports only)`: no console, window, graphics or
//! scheduler lock is taken under it. Read paths outside `home` never touch
//! [`HOME`]. A write holds it from the implicit create through the inode
//! update, and the writer is synchronous, so the lock is never a write-back
//! queue (spec 0019, H4).

use abi::Status;

use crate::ata::AtaDisk;
use crate::ext2::{Entry, Ext2};
use crate::println;
use crate::sync::SpinLock;

/// The read-only application image, published once before the APs start.
static IMAGE: SpinLock<Option<&'static Ext2<'static>>> = SpinLock::new(None);

/// The optional writable home mount, serialised for SMP.
static HOME: SpinLock<Option<ext2w::Fs<AtaDisk>>> = SpinLock::new(None);

/// Publishes the application image. Called once on the boot core after the
/// module has mounted and before any core can service a request.
pub fn init(image: &Ext2<'_>) {
    // SAFETY: the mount borrows the GRUB module's bytes, which the bootloader
    // owns for the lifetime of the system; nothing unmaps or frees them (see
    // `module::bytes`). Extending the borrow to `'static` is what lets the AP
    // loops share the read-only mount without an allocator.
    let image: &'static Ext2<'static> = unsafe { core::mem::transmute(image) };
    *IMAGE.lock() = Some(image);
}

fn image() -> &'static Ext2<'static> {
    match *IMAGE.lock() {
        Some(image) => image,
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

/// The image tree boot check (spec 0018, L1/L6).
pub fn tree_self_check() -> Result<(), u32> {
    image().tree_self_check()
}

/// Splits `home` / `home/...` into the path local to the home mount. Returns
/// `None` for every other path, which stays on the application image.
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
    self::image().read_file(path, out)
}

/// Reads at an offset through the router (spec 0013, `core.fs_read`).
pub fn read_at(path: &str, offset: u64, out: &mut [u8]) -> Result<usize, Status> {
    if let Some(local) = home_local(path) {
        let home = HOME.lock();
        if let Some(home) = home.as_ref() {
            return home.read_at(local, offset, out);
        }
    }
    self::image().read_at(path, offset, out)
}

/// Returns a regular file's size through the router.
pub fn file_size(path: &str) -> Result<u64, Status> {
    if let Some(local) = home_local(path) {
        let home = HOME.lock();
        if let Some(home) = home.as_ref() {
            return home.file_size(local);
        }
    }
    self::image().file_size(path)
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
pub fn list(path: &str, index: u32, name: &mut [u8]) -> Result<Entry, Status> {
    if let Some(local) = home_local(path) {
        let home = HOME.lock();
        if let Some(home) = home.as_ref() {
            let entry = home.list(local, index, name)?;
            return Ok(Entry {
                name_len: entry.name_len,
                kind: entry.kind,
                size: entry.size,
            });
        }
    }
    self::image().list_entry(path, index, name)
}
