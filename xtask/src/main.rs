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
        "image" => image(),
        "run" => run(&args[1..]),
        "clean" => clean(),
        other => {
            eprintln!("unknown command: {other}");
            eprintln!("usage: cargo xtask <build|image|run [--headless] [--cores N]|clean>");
            ExitCode::FAILURE
        }
    }
}

fn build() -> ExitCode {
    let status = Command::new(cargo())
        .current_dir(root())
        .args([
            "build",
            "-p",
            "kernel",
            "--release",
            "--target",
            TARGET,
        ])
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

fn image() -> ExitCode {
    if build() == ExitCode::FAILURE {
        return ExitCode::FAILURE;
    }

    let elf = kernel_elf();
    if !elf.exists() {
        eprintln!("kernel ELF not found at {}", elf.display());
        return ExitCode::FAILURE;
    }

    let iso_root = root().join("build").join("iso");
    let boot = iso_root.join("boot");
    let grub = boot.join("grub");

    fs::create_dir_all(&grub).expect("create ISO tree");
    fs::copy(&elf, boot.join("kernel.elf")).expect("copy kernel ELF");

    let apps = match apps_image() {
        Ok(image) => image,
        Err(message) => {
            eprintln!("apps image: {message}");
            return ExitCode::FAILURE;
        }
    };
    fs::copy(&apps, boot.join("apps.img")).expect("copy apps image");

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

    let iso = root().join("build").join("igis.iso");
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
/// image carries source files only (ADR 0019).
fn apps_image() -> Result<PathBuf, String> {
    let source = root().join("samples").join("apps");
    if !source.is_dir() {
        return Err(format!("{} is not a directory", source.display()));
    }

    let staging = root().join("build").join("apps-root");
    if staging.exists() {
        fs::remove_dir_all(&staging).map_err(|error| format!("{}: {error}", staging.display()))?;
    }
    fs::create_dir_all(&staging).map_err(|error| format!("{}: {error}", staging.display()))?;
    stage_files(&source, &staging)?;

    let image = root().join("build").join("apps.img");
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

/// Copies the regular files of `source` into `destination` (recursively).
///
/// The ext2 reader only serves the image root, so a nested directory would be
/// unreachable from an application. Refusing it here keeps the image honest.
fn stage_files(source: &Path, destination: &Path) -> Result<(), String> {
    let entries = fs::read_dir(source).map_err(|error| format!("{}: {error}", source.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("{}: {error}", source.display()))?;
        let path = entry.path();
        if path.is_dir() {
            return Err(format!(
                "{}: directories below the image root are not supported",
                path.display()
            ));
        }
        if !path.is_file() {
            continue;
        }
        let name = match path.file_name() {
            Some(name) => name,
            None => return Err(format!("{}: no file name", path.display())),
        };
        fs::copy(&path, destination.join(name))
            .map_err(|error| format!("{}: {error}", path.display()))?;
    }
    Ok(())
}

fn run(args: &[String]) -> ExitCode {
    if image() == ExitCode::FAILURE {
        return ExitCode::FAILURE;
    }

    let headless = args.iter().any(|arg| arg == "--headless");
    // Guest cores; the kernel brings up at most `MAX_CORES`. `-smp 1` is the
    // single-core path, `-smp 2` proves absent APIC ids are tolerated.
    let mut cores = 4u32;
    if let Some(position) = args.iter().position(|arg| arg == "--cores") {
        match args.get(position + 1).and_then(|value| value.parse::<u32>().ok()) {
            Some(value) if value >= 1 => cores = value,
            _ => {
                eprintln!("--cores needs a number of at least 1");
                return ExitCode::FAILURE;
            }
        }
    }

    let iso = root().join("build").join("igis.iso");
    let mut qemu = Command::new("qemu-system-x86_64");

    qemu.current_dir(root())
        .arg("-cdrom")
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
        .arg("std");

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
