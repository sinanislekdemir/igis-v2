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
        "image" => match net_irq(&args) {
            Ok(mode) => image(
                args.iter().any(|arg| arg == "--shell"),
                args.iter().any(|arg| arg == "--reset-home"),
                mode,
            ),
            Err(code) => code,
        },
        "run" => run(&args[1..]),
        "clean" => clean(),
        other => {
            eprintln!("unknown command: {other}");
            eprintln!(
                "usage: cargo xtask <build|image [--shell] [--reset-home] [--net-irq intx|poll]|run [--headless] [--no-tablet] [--no-net] [--net-device virtio|e1000|e1000e] [--net-irq intx|poll] [--no-home] [--reset-home] [--shell] [--cores N]|clean>"
            );
            ExitCode::FAILURE
        }
    }
}

/// The NIC interrupt mode selected by `--net-irq` (spec 0020, N4).
///
/// `intx` is the default: boot routes the PCI INTx line. `poll` puts
/// `igis.net_irq=poll` on the multiboot2 command line so the same kernel skips
/// the route and keeps the polling drain, which is how the fallback is proved.
fn net_irq(args: &[String]) -> Result<&'static str, ExitCode> {
    match args.iter().position(|arg| arg == "--net-irq") {
        None => Ok("intx"),
        Some(position) => match args.get(position + 1).map(String::as_str) {
            Some("intx") => Ok("intx"),
            Some("poll") => Ok("poll"),
            _ => {
                eprintln!("--net-irq needs one of intx, poll");
                Err(ExitCode::FAILURE)
            }
        },
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

fn image(shell: bool, reset_home: bool, net_irq: &str) -> ExitCode {
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

    let cmdline = if net_irq == "poll" {
        "multiboot2 /boot/kernel.elf igis.net_irq=poll"
    } else {
        "multiboot2 /boot/kernel.elf"
    };
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

/// Builds the read-only ext2 application image.
///
/// The shipped corpus is `samples/apps/`. The `home` fallback comes from
/// `samples/home/`, the same seed the writable disk uses, so it has one source.
/// The staging directory and the image live under `build/`; `mke2fs -d` copies
/// the staged tree into a fresh 4 MiB ext2 filesystem with 1 KiB blocks. The
/// image carries source files only (ADR 0019) and preserves the source tree
/// (spec 0018, X1): `lib`, `base`, `home` and `system` become real directories,
/// and no file sits at the image root.
///
/// The boot entry is `system/entrypoint` (spec 0018, A3): `base/desktop_shell`
/// is staged a second time under that name so the boot unit can be the desktop
/// without a duplicate source. The file name is the load path; the
/// `unit desktop_shell;` name is only diagnostic identity.
///
/// `--shell` selects the test image (spec 0018, X2): it overlays
/// `samples/tests/apps/` into the same directories so proofs and negative
/// fixtures are reachable with their usual names, and it omits the
/// `system/entrypoint` alias so the REPL falls back to `base/shell`, exactly as
/// an older image would. The product image never carries the test corpus.
fn apps_image(shell: bool) -> Result<PathBuf, String> {
    let source = root().join("samples").join("apps");
    if !source.is_dir() {
        return Err(format!("{} is not a directory", source.display()));
    }
    // Spec 0018, L1: the source root holds only image directories.
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
    stage_apps(&source, &staging, false)?;

    // X3: `home` is the read-only fallback and shares one seed with the
    // writable disk image (spec 0018, X1; spec 0019, H2).
    let home_seed = root().join("samples").join("home");
    if !home_seed.is_dir() {
        return Err(format!("{} is not a directory", home_seed.display()));
    }
    stage_tree(&home_seed, &staging.join("home"), false)?;

    // X2: the test corpus is present only in the `--shell` image.
    if shell {
        let tests = root().join("samples").join("tests").join("apps");
        if !tests.is_dir() {
            return Err(format!("{} is not a directory", tests.display()));
        }
        for entry in fs::read_dir(&tests).map_err(|error| format!("{}: {error}", tests.display()))? {
            let path = entry
                .map_err(|error| format!("{}: {error}", tests.display()))?
                .path();
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            if name.starts_with('.') {
                continue;
            }
            if !path.is_dir() || !["base", "system"].contains(&name) {
                return Err(format!(
                    "{}: the test root holds only base and system",
                    path.display()
                ));
            }
        }
        stage_apps(&tests, &staging, true)?;
    }

    // The four top-level directories exist even when empty.
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
    stage_tree(&source, &staging, false)?;

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

/// Stages an application tree (spec 0018, L3/X1): `lib` and `home` keep their
/// names (`lib/<name>.ig`), while `base` and `system` sources are `<name>.ig`
/// and are staged as `<name>` so the image command name has no extension.
fn stage_apps(source: &Path, destination: &Path, overlay: bool) -> Result<(), String> {
    fs::create_dir_all(destination)
        .map_err(|error| format!("{}: {error}", destination.display()))?;
    let entries = fs::read_dir(source).map_err(|error| format!("{}: {error}", source.display()))?;
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
        if !path.is_dir() {
            return Err(format!(
                "{}: an image root entry must be a directory",
                path.display()
            ));
        }
        let target = destination.join(name);
        if name == "lib" || name == "home" {
            stage_tree(&path, &target, overlay)?;
        } else {
            stage_app_dir(&path, &target, overlay)?;
        }
    }
    Ok(())
}

/// Stages one application directory, dropping the `.ig` suffix from file names
/// (spec 0018, L3). A file without the suffix is copied unchanged.
fn stage_app_dir(source: &Path, destination: &Path, overlay: bool) -> Result<(), String> {
    fs::create_dir_all(destination)
        .map_err(|error| format!("{}: {error}", destination.display()))?;
    let entries = fs::read_dir(source).map_err(|error| format!("{}: {error}", source.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("{}: {error}", source.display()))?;
        let path = entry.path();
        let raw_name = match path.file_name().and_then(|name| name.to_str()) {
            Some(name) => name,
            None => return Err(format!("{}: no file name", path.display())),
        };
        if raw_name.starts_with('.') {
            continue;
        }
        let staged_name = match raw_name.strip_suffix(".ig") {
            Some(stem) if path.is_file() => stem,
            _ => raw_name,
        };
        let target = destination.join(staged_name);
        if path.is_dir() {
            stage_app_dir(&path, &target, overlay)?;
        } else if path.is_file() {
            if overlay && target.exists() {
                return Err(format!(
                    "{}: test source collides with an image file",
                    target.display()
                ));
            }
            fs::copy(&path, &target).map_err(|error| format!("{}: {error}", path.display()))?;
        }
    }
    Ok(())
}

/// Copies the tree under `source` into `destination`, creating real
/// directories (spec 0018, X1).
///
/// With `overlay` false a later file replaces an earlier one; the shipped
/// corpus and the home seed are staged that way. With `overlay` true (the test
/// corpus) a file may not replace an existing image file, so a test name that
/// collides with a shipped name is a staging error instead of silently
/// shadowing it. Files whose name starts with `.` are repository bookkeeping
/// (`.gitkeep`), not image content, and are skipped.
fn stage_tree(source: &Path, destination: &Path, overlay: bool) -> Result<(), String> {
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
            stage_tree(&path, &target, overlay)?;
        } else if path.is_file() {
            if overlay && target.exists() {
                return Err(format!(
                    "{}: test source collides with an image file",
                    target.display()
                ));
            }
            fs::copy(&path, &target).map_err(|error| format!("{}: {error}", path.display()))?;
        }
    }
    Ok(())
}

fn run(args: &[String]) -> ExitCode {
    let shell = args.iter().any(|arg| arg == "--shell");
    let reset_home = args.iter().any(|arg| arg == "--reset-home");
    let net_irq = match net_irq(args) {
        Ok(mode) => mode,
        Err(code) => return code,
    };
    if image(shell, reset_home, net_irq) == ExitCode::FAILURE {
        return ExitCode::FAILURE;
    }

    let headless = args.iter().any(|arg| arg == "--headless");
    // The absolute-pointer proof needs the USB tablet; `--no-tablet` boots the
    // same image with the UHCI bus but no HID device, so the PS/2 fallback can
    // be checked (spec 0010, M2.2.6).
    let no_tablet = args.iter().any(|arg| arg == "--no-tablet");
    // One user-mode NIC is the default test device (spec 0020, N19/ADR 0049);
    // `--no-net` proves the graceful-absence path and `--net-device` selects the
    // driver under test (virtio-net is the default, e1000/e1000e is VMware's
    // standard virtual NIC and the second `NetDevice` implementation).
    let no_net = args.iter().any(|arg| arg == "--no-net");
    let mut net_device = "virtio";
    if let Some(position) = args.iter().position(|arg| arg == "--net-device") {
        match args.get(position + 1).map(String::as_str) {
            Some(value @ ("virtio" | "e1000" | "e1000e")) => net_device = value,
            _ => {
                eprintln!("--net-device needs one of virtio, e1000, e1000e");
                return ExitCode::FAILURE;
            }
        }
    }
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

    // One deterministic NIC on QEMU user networking (spec 0020, N19; ADR 0049).
    // `disable-modern=on` keeps the legacy I/O BAR0 register layout the virtio
    // driver implements; `e1000`/`e1000e` select the MMIO driver instead.
    if !no_net {
        qemu.arg("-netdev").arg("user,id=n0").arg("-device");
        match net_device {
            "e1000" => qemu.arg("e1000,netdev=n0"),
            "e1000e" => qemu.arg("e1000e,netdev=n0"),
            _ => qemu.arg("virtio-net-pci,netdev=n0,disable-modern=on"),
        };
    } else {
        // QEMU adds a default e1000 when no network option is present;
        // `-nic none` makes `--no-net` a real absence (spec 0020, N19).
        qemu.arg("-nic").arg("none");
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
