//! Build, image and QEMU orchestration.
//!
//! `cargo xtask <command>`; the workspace `.cargo/config.toml` does not set a
//! default target, so this host tool builds with the host toolchain while the
//! kernel is cross-built for `x86_64-unknown-none`.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const TARGET: &str = "x86_64-unknown-none";
const KERNEL_ELF: &str = "kernel";

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives in the workspace")
        .to_path_buf()
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let command = args.first().map(String::as_str).unwrap_or("build");

    match command {
        "build" => build(),
        "image" => image(
            args.iter().any(|arg| arg == "--shell"),
            args.iter().any(|arg| arg == "--reset-home"),
        ),
        "run" => run(&args[1..]),
        "clean" => clean(),
        other => {
            eprintln!("unknown command: {other}");
            eprintln!(
                "usage: cargo xtask <build|image [--shell] [--reset-home]|run [--headless] [--no-tablet] [--no-home] [--reset-home] [--shell] [--cores N]|clean>"
            );
            ExitCode::FAILURE
        }
    }
}

fn build() -> ExitCode {
    let status = Command::new(cargo())
        .current_dir(root())
        .args(["build", "-p", "kernel", "--release", "--target", TARGET])
        .status()
        .expect("failed to run cargo");

    if status.success() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn kernel_elf() -> PathBuf {
    root()
        .join("target")
        .join(TARGET)
        .join("release")
        .join(KERNEL_ELF)
}

fn image(shell: bool, reset_home: bool) -> ExitCode {
    if build() == ExitCode::FAILURE {
        return ExitCode::FAILURE;
    }

    let elf = kernel_elf();
    if !elf.exists() {
        eprintln!("kernel ELF not found at {}", elf.display());
        return ExitCode::FAILURE;
    }

    let iso_root = root()
        .join("build")
        .join(if shell { "iso-shell" } else { "iso" });
    let boot = iso_root.join("boot");
    let grub = boot.join("grub");

    fs::create_dir_all(&grub).expect("create ISO tree");
    fs::copy(&elf, boot.join("kernel.elf")).expect("copy kernel ELF");

    let apps = match apps_image(shell) {
        Ok(image) => image,
        Err(message) => {
            eprintln!("apps image: {message}");
            return ExitCode::FAILURE;
        }
    };
    fs::copy(&apps, boot.join("apps.img")).expect("copy apps image");

    if let Err(message) = home_image(reset_home) {
        eprintln!("home image: {message}");
        return ExitCode::FAILURE;
    }

    let cmdline = "multiboot2 /boot/kernel.elf";
    let module = "module2 /boot/apps.img apps";
    let config = format!(
        "serial --unit=0 --speed=115200\n\
         terminal_input serial\n\
         terminal_output serial\n\
         set timeout=0\n\
         set default=0\n\n\
         menuentry \"igis-v2\" {{\n    {cmdline}\n    {module}\n    boot\n}}\n"
    );
    fs::write(grub.join("grub.cfg"), config).expect("write grub.cfg");

    let iso_name = if shell { "igis-shell.iso" } else { "igis.iso" };
    let iso = root().join("build").join(iso_name);
    let status = Command::new("grub2-mkrescue")
        .arg("-o")
        .arg(&iso)
        .arg(&iso_root)
        .status()
        .expect("failed to run grub2-mkrescue");

    if !status.success() {
        return ExitCode::FAILURE;
    }

    println!("image: {}", iso.display());
    ExitCode::SUCCESS
}

/// Builds the read-only ext2 application image from `samples/apps/`.
///
/// The staging directory and the image live under `build/`; `mke2fs -d` copies
/// the staged tree into a fresh 4 MiB ext2 filesystem with 1 KiB blocks. The
/// image carries source files only (ADR 0019) and preserves the source tree
/// (spec 0018, X1): `lib`, `base`, `home` and `system` become real directories,
/// and no file sits at the image root.
///
/// The boot entry is `system/entrypoint` (spec 0018, A3): `base/desktop_shell`
/// is staged a second time under that name so the boot unit can be the desktop
/// without a duplicate source. The file name is the load path; the
/// `unit desktop_shell;` name is only diagnostic identity. `--shell` omits the
/// alias so the REPL falls back to `base/shell`, exactly as an older image
/// would.
fn apps_image(shell: bool) -> Result<PathBuf, String> {
    let source = root().join("samples").join("apps");
    if !source.is_dir() {
        return Err(format!("{} is not a directory", source.display()));
    }
    // Spec 0018, L1: the source root holds exactly the four directories.
    let top_level = ["lib", "base", "home", "system"];
    let entries =
        fs::read_dir(&source).map_err(|error| format!("{}: {error}", source.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("{}: {error}", source.display()))?;
        let path = entry.path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if name.starts_with('.') {
            continue;
        }
        if !path.is_dir() || !top_level.contains(&name) {
            return Err(format!(
                "{}: the image root holds only lib, base, home and system",
                path.display()
            ));
        }
    }

    let staging = root().join("build").join(if shell {
        "apps-shell-root"
    } else {
        "apps-root"
    });
    if staging.exists() {
        fs::remove_dir_all(&staging).map_err(|error| format!("{}: {error}", staging.display()))?;
    }
    fs::create_dir_all(&staging).map_err(|error| format!("{}: {error}", staging.display()))?;
    stage_tree(&source, &staging)?;
    // X3: the four top-level directories exist even when empty (lib is empty
    // until M3.2).
    for name in top_level {
        fs::create_dir_all(staging.join(name))
            .map_err(|error| format!("{}: {error}", staging.join(name).display()))?;
    }

    if !shell {
        let desktop = staging.join("base").join("desktop_shell");
        if !desktop.is_file() {
            return Err(format!("{}: missing desktop source", desktop.display()));
        }
        let entrypoint = staging.join("system").join("entrypoint");
        fs::copy(&desktop, &entrypoint)
            .map_err(|error| format!("{}: {error}", entrypoint.display()))?;
    }

    let image = root()
        .join("build")
        .join(if shell { "apps-shell.img" } else { "apps.img" });
    if image.exists() {
        fs::remove_file(&image).map_err(|error| format!("{}: {error}", image.display()))?;
    }

    let status = Command::new("mke2fs")
        .arg("-q")
        .arg("-t")
        .arg("ext2")
        .arg("-b")
        .arg("1024")
        .arg("-d")
        .arg(&staging)
        .arg("-F")
        .arg(&image)
        .arg("4M")
        .status()
        .map_err(|error| format!("failed to run mke2fs: {error}"))?;
    if !status.success() {
        return Err(format!("mke2fs failed with {status}"));
    }

    Ok(image)
}

/// Ensures the writable home image exists (spec 0019, H1/H2).
///
/// The image is 8 MiB of 1 KiB-block ext2 with an initial tree from
/// `samples/home/`. It is created only when missing; an existing image is
/// user data and is never overwritten unless `reset` is set (`--reset-home`).
fn home_image(reset: bool) -> Result<PathBuf, String> {
    let image = root().join("build").join("home.img");
    if image.exists() && !reset {
        return Ok(image);
    }
    if image.exists() {
        fs::remove_file(&image).map_err(|error| format!("{}: {error}", image.display()))?;
    }

    let source = root().join("samples").join("home");
    if !source.is_dir() {
        return Err(format!("{} is not a directory", source.display()));
    }
    let staging = root().join("build").join("home-root");
    if staging.exists() {
        fs::remove_dir_all(&staging).map_err(|error| format!("{}: {error}", staging.display()))?;
    }
    fs::create_dir_all(&staging).map_err(|error| format!("{}: {error}", staging.display()))?;
    stage_tree(&source, &staging)?;

    let status = Command::new("mke2fs")
        .arg("-q")
        .arg("-t")
        .arg("ext2")
        .arg("-b")
        .arg("1024")
        .arg("-d")
        .arg(&staging)
        .arg("-F")
        .arg(&image)
        .arg("8M")
        .status()
        .map_err(|error| format!("failed to run mke2fs: {error}"))?;
    if !status.success() {
        return Err(format!("mke2fs failed with {status}"));
    }
    println!("home image: {}", image.display());
    Ok(image)
}

/// Copies the tree under `source` into `destination`, creating real
/// directories (spec 0018, X1).
///
/// Files whose name starts with `.` are repository bookkeeping (`.gitkeep`),
/// not image content, and are skipped.
fn stage_tree(source: &Path, destination: &Path) -> Result<(), String> {
    fs::create_dir_all(destination)
        .map_err(|error| format!("{}: {error}", destination.display()))?;
    let entries = fs::read_dir(source).map_err(|error| format!("{}: {error}", source.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("{}: {error}", source.display()))?;
        let path = entry.path();
        let name = match path.file_name() {
            Some(name) => name,
            None => return Err(format!("{}: no file name", path.display())),
        };
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        let target = destination.join(name);
        if path.is_dir() {
            stage_tree(&path, &target)?;
        } else if path.is_file() {
            fs::copy(&path, &target).map_err(|error| format!("{}: {error}", path.display()))?;
        }
    }
    Ok(())
}

fn run(args: &[String]) -> ExitCode {
    let shell = args.iter().any(|arg| arg == "--shell");
    let reset_home = args.iter().any(|arg| arg == "--reset-home");
    if image(shell, reset_home) == ExitCode::FAILURE {
        return ExitCode::FAILURE;
    }

    let headless = args.iter().any(|arg| arg == "--headless");
    // The absolute-pointer proof needs the USB tablet; `--no-tablet` boots the
    // same image with the UHCI bus but no HID device, so the PS/2 fallback can
    // be checked (spec 0010, M2.2.6).
    let no_tablet = args.iter().any(|arg| arg == "--no-tablet");
    // The home disk is optional (spec 0019, H2/H3); `--no-home` proves the
    // read-only fallback exactly as an image without the drive would.
    let no_home = args.iter().any(|arg| arg == "--no-home");
    // Guest cores; the kernel brings up at most `MAX_CORES`. `-smp 1` is the
    // single-core path, `-smp 2` proves absent APIC ids are tolerated.
    let mut cores = 4u32;
    if let Some(position) = args.iter().position(|arg| arg == "--cores") {
        match args
            .get(position + 1)
            .and_then(|value| value.parse::<u32>().ok())
        {
            Some(value) if value >= 1 => cores = value,
            _ => {
                eprintln!("--cores needs a number of at least 1");
                return ExitCode::FAILURE;
            }
        }
    }

    let iso = root()
        .join("build")
        .join(if shell { "igis-shell.iso" } else { "igis.iso" });
    let mut qemu = Command::new("qemu-system-x86_64");
    qemu.current_dir(root());

    // The home disk is the primary-master IDE disk (spec 0019, H2). It is
    // added before the CD-ROM so the fixed drive order keeps it on ide0 and
    // the boot ISO on the CD-ROM interface.
    if !no_home {
        qemu.arg("-drive")
            .arg("file=build/home.img,format=raw,if=ide");
    }

    qemu.arg("-cdrom")
        .arg(&iso)
        .arg("-m")
        .arg("256M")
        .arg("-smp")
        .arg(cores.to_string())
        .arg("-serial")
        .arg("stdio")
        .arg("-monitor")
        .arg("none")
        .arg("-no-reboot")
        .arg("-no-shutdown")
        .arg("-accel")
        .arg("tcg")
        .arg("-vga")
        .arg("std")
        .arg("-usb");

    if !no_tablet {
        qemu.arg("-device").arg("usb-tablet");
    }

    if headless {
        qemu.arg("-display").arg("none");
    }

    let status = qemu.status().expect("failed to run qemu-system-x86_64");
    ExitCode::from(status.code().unwrap_or(1) as u8)
}

fn clean() -> ExitCode {
    let _ = fs::remove_dir_all(root().join("build"));
    let status = Command::new(cargo())
        .current_dir(root())
        .args(["clean"])
        .status()
        .expect("failed to run cargo clean");

    if status.success() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn cargo() -> String {
    env::var("CARGO").unwrap_or_else(|_| "cargo".to_string())
}

#[allow(dead_code)]
fn is_file(path: &Path) -> bool {
    path.is_file()
}
