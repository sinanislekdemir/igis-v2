//! Host-side driver for the application language.
//!
//! This is the "language track" from spec 0005: the compiler and interpreter
//! linked together with a host implementation of the console and the clock.
//! The kernel will link the same two crates against its own [`Host`], so
//! nothing here is part of the language itself.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::io::{Read, Seek, Write};
use std::path::PathBuf;
use std::time::Instant;

use abi::{ExitReason, FaultCode, Status};
use interpreter::{FsEntry, Host, Machine, Outcome, Program, StreamError, WaitOutcome};

/// Default instruction budget for one `langhost` run. This is a host-side
/// watchdog, not a scheduler quantum: the machine is resumed between slices
/// until the budget is exhausted.
pub const DEFAULT_BUDGET: u64 = 100_000_000;

/// Maximum nesting of `core.spawn` on the host. The real kernel places tasks
/// statically and has no depth limit beyond the 64-slot task table; the host
/// runs a child synchronously inside its parent, so it needs one.
pub const MAX_SPAWN_DEPTH: u32 = 8;

/// Directory the file API and `core.spawn` resolve image paths against, unless
/// `IGIS_APP_DIR` is set. It is the source tree `xtask image` stages, so the
/// host and the kernel see the same `lib`/`base`/`home`/`system` layout
/// (spec 0018).
const DEFAULT_APP_DIR: &str = "samples/apps";

/// Prefix a bare launch name resolves against (spec 0018, A1).
const LAUNCH_PREFIX: &str = "base/";

/// A package with owned buffers, produced by [`compile`].
///
/// The compiler writes into caller-provided buffers, so the host allocates the
/// arena, code and manifest once per compile (the sizes are the caps exported
/// by `compiler`) and copies the result out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedPackage {
    /// Encoded manifest.
    pub manifest: Vec<u8>,
    /// Encoded code blob.
    pub code: Vec<u8>,
}

/// Compiles one unit into an owned package.
///
/// This is the host wrapper around [`compiler::compile_with_imports`]: it
/// allocates the bounded buffers the kernel will later place statically,
/// compiles, and copies the artifact out so the diagnostic can outlive the
/// scratch. `/lib` modules resolve through a [`TreeReader`] over the same
/// source tree `core.spawn` and the file API use (spec 0018, X5).
#[allow(clippy::result_large_err)]
pub fn compile(source: &[u8], path: &str) -> Result<OwnedPackage, compiler::Diagnostic> {
    compile_in(source, path, &app_dir())
}

/// [`compile`] against an explicit tree root. Tests use it to pin a fixture.
#[allow(clippy::result_large_err)]
fn compile_in(
    source: &[u8],
    path: &str,
    root: &std::path::Path,
) -> Result<OwnedPackage, compiler::Diagnostic> {
    let mut reader = TreeReader {
        root: root.to_path_buf(),
    };
    compile_with_reader(source, path, &mut reader)
}

/// The reader-agnostic core of [`compile`]; tests supply an in-memory reader.
#[allow(clippy::result_large_err)]
fn compile_with_reader(
    source: &[u8],
    path: &str,
    reader: &mut dyn compiler::SourceReader,
) -> Result<OwnedPackage, compiler::Diagnostic> {
    let mut arena = vec![0u8; compiler::MAX_ARENA_BYTES];
    let mut code = vec![0u8; compiler::MAX_CODE_BYTES];
    let mut manifest = vec![0u8; compiler::MAX_MANIFEST_BYTES];
    let package =
        compiler::compile_with_imports(source, path, reader, &mut arena, &mut code, &mut manifest)?;
    Ok(OwnedPackage {
        manifest: package.manifest.to_vec(),
        code: package.code.to_vec(),
    })
}

/// Reads `/lib` module sources from the image tree (spec 0018, X5).
///
/// The rules match the kernel's ext2 reader: only a regular file is readable,
/// a symlink is refused instead of followed, a missing component is
/// `NotFound`, and a path that walks through a file is `Invalid`.
struct TreeReader {
    root: PathBuf,
}

/// Maps a host I/O error onto the file API's statuses.
fn io_status(error: &std::io::Error) -> Status {
    match error.kind() {
        std::io::ErrorKind::NotFound => Status::NotFound,
        std::io::ErrorKind::NotADirectory => Status::Invalid,
        _ => Status::IoError,
    }
}

impl TreeReader {
    fn metadata(&self, path: &str) -> Result<std::fs::Metadata, Status> {
        let target = resolve_in(&self.root, path)?;
        let metadata = std::fs::symlink_metadata(&target).map_err(|e| io_status(&e))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(Status::Invalid);
        }
        Ok(metadata)
    }
}

impl compiler::SourceReader for TreeReader {
    fn size(&self, path: &str) -> Result<usize, Status> {
        let metadata = self.metadata(path)?;
        usize::try_from(metadata.len()).map_err(|_| Status::CapacityExceeded)
    }

    fn read(&self, path: &str, out: &mut [u8]) -> Result<usize, Status> {
        self.metadata(path)?;
        let target = resolve_in(&self.root, path)?;
        let mut file = std::fs::File::open(&target).map_err(|e| io_status(&e))?;
        let mut written = 0usize;
        while written < out.len() {
            match file.read(&mut out[written..]) {
                Ok(0) => break,
                Ok(count) => written += count,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return Err(Status::IoError),
            }
        }
        // `size` promised this many bytes; a growing file is a race, not a
        // truncated read, so refuse it instead of returning a short length.
        let mut extra = [0u8; 1];
        match file.read(&mut extra) {
            Ok(0) => Ok(written),
            Ok(_) => Err(Status::CapacityExceeded),
            Err(_) => Err(Status::IoError),
        }
    }
}

/// Resolves an image path under `root`, component by component, so a file used
/// as a directory reports `Invalid` exactly as the ext2 walker does (spec 0018,
/// L6). `""` and `"."` are the root.
fn resolve_in(root: &std::path::Path, path: &str) -> Result<PathBuf, Status> {
    let mut current = root.to_path_buf();
    if path.is_empty() || path == "." {
        return Ok(current);
    }
    let mut parts = path.split('/').peekable();
    while let Some(part) = parts.next() {
        current.push(part);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if parts.peek().is_some() && !metadata.is_dir() {
                    return Err(Status::Invalid);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(Status::NotFound)
            }
            Err(_) => return Err(Status::IoError),
        }
    }
    Ok(current)
}

/// A finished, or budget-terminated, run.
#[derive(Debug)]
pub struct RunResult {
    /// Bytes the program wrote to the console.
    pub output: Vec<u8>,
    /// Why the run stopped.
    pub reason: ExitReason,
    /// Source line and column of a fault, when the line table can resolve the
    /// faulting instruction.
    pub fault_at: Option<(u32, u32)>,
}

/// The host implementation behind the trait.
///
/// `core.spawn` is implemented the only way a single-threaded host can: it
/// compiles and runs the child **synchronously**, appends its output to the
/// parent's, and stores the exit reason under a fresh id. `core.wait` then
/// returns that record immediately and never blocks; the scheduling difference
/// from the kernel is invisible to a program that spawns and waits.
///
/// The host has no kernel console ring, so `core.read_input` drains a scripted
/// byte queue: [`HOST_INPUT`] is what a person would type at the serial line,
/// including the newlines. A host-side shell therefore runs the same
/// application-side line editing as the kernel's, deterministically and without
/// ever blocking. A program that keeps reading after the script is exhausted
/// reads zero bytes and either exits or blocks (which the host reports as
/// `IllegalOperation`).
const HOST_INPUT: &[u8] = b"help\nrun child world\necho hi there\nexit\n";

struct ConsoleHost {
    output: Vec<u8>,
    depth: u32,
    source_dir: PathBuf,
    /// The temporary directory that stands in for the home disk (spec 0019,
    /// A6). `home` and `home/...` resolve here; the top-level host owns it and
    /// removes it on drop, a child shares the parent's directory.
    home_dir: PathBuf,
    /// True only for the host that created `home_dir`.
    home_owned: bool,
    children: HashMap<u64, ExitReason>,
    next_id: u64,
    /// Bytes the scripted console has not delivered yet (`core.read_input`).
    input: Vec<u8>,
    /// One loopback FIFO for every endpoint (see `stream_send`).
    stream: Vec<u8>,
    /// Record-shaped events queued by `syslib.inject_event`, oldest first.
    events: Vec<Vec<u8>>,
}

impl ConsoleHost {
    /// A child host that shares this run's home directory (a child's write is
    /// visible to its parent, as it is in the kernel).
    fn child(&self) -> Self {
        ConsoleHost {
            output: Vec::new(),
            depth: self.depth + 1,
            source_dir: self.source_dir.clone(),
            home_dir: self.home_dir.clone(),
            home_owned: false,
            children: HashMap::new(),
            next_id: 1,
            input: HOST_INPUT.to_vec(),
            stream: Vec::new(),
            events: Vec::new(),
        }
    }

    /// Resolves an image path under the sample tree (spec 0018, L6) or, for
    /// `home`, under this run's home directory (spec 0019, A6).
    fn resolve_path(&self, path: &str) -> Result<PathBuf, Status> {
        match home_local(path) {
            Some(local) => resolve_in(&self.home_dir, local),
            None => resolve_in(&self.source_dir, path),
        }
    }

    fn with_input(depth: u32, input: &[u8]) -> Self {
        let source_dir = app_dir();
        let home_dir = make_home_dir(&source_dir);
        ConsoleHost {
            output: Vec::new(),
            depth,
            source_dir,
            home_dir,
            home_owned: true,
            children: HashMap::new(),
            next_id: 1,
            input: input.to_vec(),
            stream: Vec::new(),
            events: Vec::new(),
        }
    }
}

impl Drop for ConsoleHost {
    fn drop(&mut self) {
        if self.home_owned {
            let _ = std::fs::remove_dir_all(&self.home_dir);
        }
    }
}

/// Splits `home` / `home/...` into the path local to the home directory. Every
/// other path is `None` and stays on the image tree (spec 0019, A4).
fn home_local(path: &str) -> Option<&str> {
    if path == "home" {
        Some("")
    } else {
        path.strip_prefix("home/")
    }
}

/// Creates this run's home directory under the system temp directory and seeds
/// it from the image's `home` tree, so `home/readme.txt` reads the same file on
/// the host as in the guest (spec 0019, A6).
fn make_home_dir(source_dir: &std::path::Path) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let unique = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "igis-langhost-home-{}-{unique}",
        std::process::id()
    ));
    if std::fs::create_dir_all(&path).is_err() {
        return path;
    }
    let seed = source_dir.join("home");
    if let Ok(entries) = std::fs::read_dir(&seed) {
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if !kind.is_file() {
                continue;
            }
            let _ = std::fs::copy(entry.path(), path.join(entry.file_name()));
        }
    }
    path
}

/// Resolves the directory `core.spawn` reads sources from.
///
/// `IGIS_APP_DIR` wins. Otherwise the repository layout is tried relative to
/// the working directory (`samples/apps` from the root, `../samples/apps` from
/// the `langhost` crate root under `cargo test`).
fn app_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("IGIS_APP_DIR") {
        return PathBuf::from(dir);
    }
    for candidate in [DEFAULT_APP_DIR, "../samples/apps"] {
        let path = PathBuf::from(candidate);
        if path.is_dir() {
            return path;
        }
    }
    PathBuf::from(DEFAULT_APP_DIR)
}

impl Host for ConsoleHost {
    fn console_write(&mut self, bytes: &[u8]) {
        self.output.extend_from_slice(bytes);
    }

    fn read_input(&mut self, dst: &mut [u8]) -> usize {
        let copied = self.input.len().min(dst.len());
        dst[..copied].copy_from_slice(&self.input[..copied]);
        self.input.drain(..copied);
        copied
    }

    fn clock(&mut self) -> u64 {
        // One process-wide baseline, so nested runs observe the same clock.
        static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        START.get_or_init(Instant::now).elapsed().as_nanos() as u64
    }

    // The host has no scheduler, no stream registry and no manifest reader, so
    // it models every endpoint as one loopback FIFO: a send appends and a
    // receive takes from the front. That is enough to exercise the typed views
    // in `stdlib` (length prefixes, delimiters, byte order) deterministically.
    // Capacity is not modelled, and an empty FIFO reports `WouldBlock`, which
    // the host cannot honour: a program that receives before it sends stops
    // with `IllegalOperation` like any other host-side block. The kernel owns
    // the real FIFO (spec 0006).
    fn stream_send(&mut self, _handle: u64, bytes: &[u8]) -> Result<(), StreamError> {
        self.stream.extend_from_slice(bytes);
        Ok(())
    }

    fn stream_recv(&mut self, _handle: u64, dst: &mut [u8]) -> Result<usize, StreamError> {
        if self.stream.is_empty() {
            return Err(StreamError::WouldBlock);
        }
        let copied = self.stream.len().min(dst.len());
        dst[..copied].copy_from_slice(&self.stream[..copied]);
        self.stream.drain(..copied);
        Ok(copied)
    }

    // Multi-source waits (spec 0003, spec 0007; Q39). The host has no
    // scheduler, so a blocking `next_any` on a source that never arrives is
    // still a host-side block (`IllegalOperation`), exactly like an exhausted
    // script or an empty loopback receive. The probes model the three source
    // kinds faithfully: console bytes remain, an injected event is queued, or
    // the loopback FIFO has bytes.
    fn console_byte_ready(&mut self) -> bool {
        !self.input.is_empty()
    }

    fn input_ready(&mut self) -> bool {
        !self.events.is_empty()
    }

    fn stream_readable(&mut self, _handle: u64) -> bool {
        !self.stream.is_empty()
    }

    // A small in-memory event queue so `inject_event`/`poll_event`/`next_event`
    // and the input source of `poll_any` can be exercised on the host. The
    // kernel owns the real 64-event machine-wide queue (spec 0010); this one
    // never overflows in a host test.
    fn poll_event(&mut self, event: &mut [u8]) -> Status {
        let Some(first) = self.events.first().cloned() else {
            return Status::WouldBlock;
        };
        if event.len() < abi::layout::event::SIZE as usize {
            return Status::Invalid;
        }
        event[..abi::layout::event::SIZE as usize].copy_from_slice(&first);
        self.events.remove(0);
        Status::Ok
    }

    fn inject_event(&mut self, event: &[u8]) -> Status {
        if abi::layout::event::read(event).is_none() {
            return Status::Invalid;
        }
        let size = abi::layout::event::SIZE as usize;
        self.events.push(event[..size].to_vec());
        Status::Ok
    }

    fn event_flush(&mut self) -> u64 {
        let flushed = self.events.len() as u64;
        self.events.clear();
        flushed
    }

    fn spawn(&mut self, path: &[u8], args: &[u8], diag: &mut [u8]) -> Result<u64, Status> {
        let describe = |diag: &mut [u8], message: &[u8]| {
            abi::layout::diagnostic::write(diag, b"", message, 0, 0);
        };
        if self.depth >= MAX_SPAWN_DEPTH {
            eprintln!("langhost: spawn refused: nesting deeper than {MAX_SPAWN_DEPTH}");
            describe(diag, b"spawn nesting is too deep");
            return Err(Status::Full);
        }
        let path = match std::str::from_utf8(path) {
            Ok(path) => path,
            Err(_) => {
                eprintln!("langhost: spawn refused: path is not valid UTF-8");
                describe(diag, b"path is not valid UTF-8");
                return Err(Status::Invalid);
            }
        };
        if !valid_path(path) {
            eprintln!("langhost: spawn refused: malformed path `{path}`");
            describe(
                diag,
                b"path must be relative with no empty or `..` component",
            );
            return Err(Status::Invalid);
        }
        if args.len() > 1024 {
            describe(diag, b"argument is longer than 1024 bytes");
            return Err(Status::CapacityExceeded);
        }
        if std::str::from_utf8(args).is_err() {
            describe(diag, b"argument is not valid UTF-8");
            return Err(Status::Invalid);
        }

        // A bare name lives under `base/`; a path with `/` is used as written
        // (spec 0018, A1).
        let resolved = if path.contains('/') {
            path.to_string()
        } else {
            format!("{LAUNCH_PREFIX}{path}")
        };
        let status = abi::fs::validate_path(&resolved);
        if status != Status::Ok {
            eprintln!("langhost: spawn refused: malformed resolved path `{resolved}`");
            describe(diag, b"resolved path is not a valid image path");
            return Err(status);
        }

        let source = match std::fs::read(self.source_dir.join(&resolved)) {
            Ok(source) => source,
            Err(error) => {
                let status = if error.kind() == std::io::ErrorKind::NotFound {
                    Status::NotFound
                } else {
                    Status::IoError
                };
                eprintln!("langhost: spawn `{resolved}`: {error}");
                describe(
                    diag,
                    if status == Status::NotFound {
                        b"No such application"
                    } else {
                        b"application file cannot be read"
                    },
                );
                return Err(status);
            }
        };
        let package = match compile(&source, &resolved) {
            Ok(package) => package,
            Err(diagnostic) => {
                eprintln!("langhost: spawn `{resolved}`: {diagnostic}");
                abi::layout::diagnostic::write(
                    diag,
                    diagnostic.unit.as_str().as_bytes(),
                    diagnostic.message.as_str().as_bytes(),
                    i64::from(diagnostic.line),
                    i64::from(diagnostic.column),
                );
                return Err(Status::Invalid);
            }
        };

        let mut child = self.child();
        match run_with_host(&package.code, args, DEFAULT_BUDGET, &mut child) {
            Ok(result) => {
                self.output.extend_from_slice(&result.output);
                let id = self.next_id;
                self.next_id += 1;
                self.children.insert(id, result.reason);
                abi::layout::diagnostic::clear(diag);
                Ok(id)
            }
            Err(diagnostic) => {
                eprintln!("langhost: spawn `{resolved}`: verifier rejected it: {diagnostic}");
                describe(diag, b"verifier rejected the package");
                Err(Status::Invalid)
            }
        }
    }

    fn wait(&mut self, id: u64) -> WaitOutcome {
        match self.children.remove(&id) {
            Some(reason) => WaitOutcome::Exited(reason),
            None => WaitOutcome::Status(Status::NotFound),
        }
    }

    fn kill(&mut self, _id: u64) -> Status {
        // Children run to completion inside `spawn`, so nothing is ever live.
        Status::NotFound
    }

    // Application files (spec 0013, spec 0018). `langhost` has a real
    // filesystem, so it implements the same tree view over the sample source
    // directory the image is built from: listing is sorted by name (the ext2
    // order is not sorted, so no program may depend on it), reads are chunked
    // at the same 256 bytes, and symlinks and other non-file entries are
    // refused exactly as the kernel subset does.
    fn fs_list(&mut self, path: &[u8], index: u64, name: &mut [u8]) -> Result<FsEntry, Status> {
        let path = std::str::from_utf8(path).map_err(|_| Status::Invalid)?;
        let status = abi::fs::validate_dir(path);
        if status != Status::Ok {
            return Err(status);
        }
        let index = usize::try_from(index).map_err(|_| Status::NotFound)?;
        let directory = match home_local(path) {
            Some(local) => resolve_in(&self.home_dir, local)?,
            None => {
                if path.is_empty() || path == "." {
                    self.source_dir.clone()
                } else {
                    self.source_dir.join(path)
                }
            }
        };
        let mut entries = Vec::new();
        let read_dir = std::fs::read_dir(&directory).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                Status::NotFound
            } else if error.kind() == std::io::ErrorKind::NotADirectory {
                Status::Invalid
            } else {
                Status::IoError
            }
        })?;
        for entry in read_dir {
            let entry = entry.map_err(|_| Status::IoError)?;
            let entry_name = entry
                .file_name()
                .into_string()
                .map_err(|_| Status::Invalid)?;
            // The image stager skips repository bookkeeping (`.gitkeep`), so
            // the host view does too.
            if entry_name.starts_with('.') {
                continue;
            }
            let file_type = entry.file_type().map_err(|_| Status::IoError)?;
            let kind = if file_type.is_file() {
                abi::fs::KIND_FILE
            } else if file_type.is_dir() {
                abi::fs::KIND_DIR
            } else {
                abi::fs::KIND_OTHER
            };
            let size = entry.metadata().map(|meta| meta.len()).unwrap_or(0);
            entries.push((entry_name, kind, size));
        }
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        let Some((entry_name, kind, size)) = entries.get(index) else {
            return Err(Status::NotFound);
        };
        let bytes = entry_name.as_bytes();
        if bytes.len() > name.len() {
            return Err(Status::CapacityExceeded);
        }
        name[..bytes.len()].copy_from_slice(bytes);
        Ok(FsEntry {
            name_len: bytes.len(),
            kind: *kind,
            size: *size,
        })
    }

    fn fs_read(&mut self, path: &[u8], offset: u64, dst: &mut [u8]) -> Result<usize, Status> {
        let path = std::str::from_utf8(path).map_err(|_| Status::Invalid)?;
        let status = abi::fs::validate_path(path);
        if status != Status::Ok {
            return Err(status);
        }
        let target = self.resolve_path(path)?;
        let metadata = std::fs::symlink_metadata(&target).map_err(|_| Status::IoError)?;
        if !metadata.is_file() {
            return Err(Status::Invalid);
        }
        let mut file = std::fs::File::open(&target).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                Status::NotFound
            } else {
                Status::IoError
            }
        })?;
        file.seek(std::io::SeekFrom::Start(offset))
            .map_err(|_| Status::IoError)?;
        let mut written = 0usize;
        while written < dst.len() {
            match file.read(&mut dst[written..]) {
                Ok(0) => break,
                Ok(count) => written += count,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return Err(Status::IoError),
            }
        }
        Ok(written)
    }

    fn fs_size(&mut self, path: &[u8]) -> Result<u64, Status> {
        let path = std::str::from_utf8(path).map_err(|_| Status::Invalid)?;
        let status = abi::fs::validate_path(path);
        if status != Status::Ok {
            return Err(status);
        }
        let target = self.resolve_path(path)?;
        let metadata = std::fs::symlink_metadata(&target).map_err(|_| Status::IoError)?;
        if !metadata.is_file() {
            return Err(Status::Invalid);
        }
        Ok(metadata.len())
    }

    // The writable home directory (spec 0019, A6). Only `home/...` is
    // writable, exactly as in the kernel; a bare host (the default trait
    // implementation) reports `Invalid`. The write is a plain host write: the
    // temporary directory has no ext2 behind it, so it exercises the API, the
    // router and the sample application, not the writer (the `ext2w` host
    // tests own that).
    fn fs_write(&mut self, path: &[u8], offset: u64, data: &[u8]) -> Result<usize, Status> {
        let path = std::str::from_utf8(path).map_err(|_| Status::Invalid)?;
        let status = abi::fs::validate_writable(path);
        if status != Status::Ok {
            return Err(status);
        }
        let local = home_local(path).ok_or(Status::Invalid)?;
        // The final component may not exist yet (implicit create), so only the
        // parent is resolved through the tree walk.
        let (parent, name) = match local.rfind('/') {
            Some(index) => (&local[..index], &local[index + 1..]),
            None => ("", local),
        };
        if name.is_empty() {
            return Err(Status::Invalid);
        }
        let directory = resolve_in(&self.home_dir, parent)?;
        let metadata = std::fs::symlink_metadata(&directory).map_err(|error| io_status(&error))?;
        if !metadata.is_dir() {
            return Err(Status::Invalid);
        }
        let target = directory.join(name);
        if let Ok(metadata) = std::fs::symlink_metadata(&target) {
            if metadata.is_dir() {
                return Err(Status::Invalid);
            }
        }
        let mut options = std::fs::OpenOptions::new();
        options.write(true);
        if offset == 0 {
            // A missing file is created implicitly at offset 0 only (A3).
            options.create(true);
        }
        let mut file = options.open(&target).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                Status::NotFound
            } else if error.kind() == std::io::ErrorKind::IsADirectory {
                Status::Invalid
            } else {
                Status::IoError
            }
        })?;
        file.seek(std::io::SeekFrom::Start(offset))
            .map_err(|_| Status::IoError)?;
        file.write_all(data).map_err(|_| Status::IoError)?;
        Ok(data.len())
    }

    fn fs_remove(&mut self, path: &[u8]) -> Result<(), Status> {
        let path = std::str::from_utf8(path).map_err(|_| Status::Invalid)?;
        let status = abi::fs::validate_writable(path);
        if status != Status::Ok {
            return Err(status);
        }
        let local = home_local(path).ok_or(Status::Invalid)?;
        let target = resolve_in(&self.home_dir, local)?;
        let metadata = std::fs::symlink_metadata(&target).map_err(|error| io_status(&error))?;
        if metadata.is_dir() {
            let mut entries = std::fs::read_dir(&target).map_err(|_| Status::IoError)?;
            if entries.next().is_some() {
                return Err(Status::Invalid);
            }
            std::fs::remove_dir(&target).map_err(|error| io_status(&error))
        } else if metadata.is_file() {
            std::fs::remove_file(&target).map_err(|error| io_status(&error))
        } else {
            Err(Status::Invalid)
        }
    }

    fn fs_mkdir(&mut self, path: &[u8]) -> Result<(), Status> {
        let path = std::str::from_utf8(path).map_err(|_| Status::Invalid)?;
        let status = abi::fs::validate_writable(path);
        if status != Status::Ok {
            return Err(status);
        }
        let local = home_local(path).ok_or(Status::Invalid)?;
        let (parent, name) = match local.rfind('/') {
            Some(index) => (&local[..index], &local[index + 1..]),
            None => ("", local),
        };
        if name.is_empty() {
            return Err(Status::Invalid);
        }
        let directory = resolve_in(&self.home_dir, parent)?;
        let metadata = std::fs::symlink_metadata(&directory).map_err(|error| io_status(&error))?;
        if !metadata.is_dir() {
            return Err(Status::Invalid);
        }
        let target = directory.join(name);
        match std::fs::symlink_metadata(&target) {
            Ok(_) => return Err(Status::Invalid),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(Status::IoError),
        }
        std::fs::create_dir(&target).map_err(|error| match error.kind() {
            std::io::ErrorKind::AlreadyExists => Status::Invalid,
            _ => io_status(&error),
        })
    }

    fn fs_rename(&mut self, from: &[u8], to: &[u8]) -> Result<(), Status> {
        let from = std::str::from_utf8(from).map_err(|_| Status::Invalid)?;
        let to = std::str::from_utf8(to).map_err(|_| Status::Invalid)?;
        if abi::fs::validate_writable(from) != Status::Ok
            || abi::fs::validate_writable(to) != Status::Ok
        {
            return Err(Status::Invalid);
        }
        if from == to {
            return Ok(());
        }
        let from_local = home_local(from).ok_or(Status::Invalid)?;
        let to_local = home_local(to).ok_or(Status::Invalid)?;
        let source = resolve_in(&self.home_dir, from_local)?;
        let source_metadata =
            std::fs::symlink_metadata(&source).map_err(|error| io_status(&error))?;
        if !source_metadata.is_file() && !source_metadata.is_dir() {
            return Err(Status::Invalid);
        }
        let (parent, name) = match to_local.rfind('/') {
            Some(index) => (&to_local[..index], &to_local[index + 1..]),
            None => ("", to_local),
        };
        if name.is_empty() {
            return Err(Status::Invalid);
        }
        let directory = resolve_in(&self.home_dir, parent)?;
        let metadata = std::fs::symlink_metadata(&directory).map_err(|error| io_status(&error))?;
        if !metadata.is_dir() {
            return Err(Status::Invalid);
        }
        let target = directory.join(name);
        if let Ok(metadata) = std::fs::symlink_metadata(&target) {
            if metadata.is_dir() {
                return Err(Status::Invalid);
            }
        }
        std::fs::rename(&source, &target).map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => Status::NotFound,
            std::io::ErrorKind::AlreadyExists
            | std::io::ErrorKind::IsADirectory
            | std::io::ErrorKind::DirectoryNotEmpty => Status::Invalid,
            _ => io_status(&error),
        })
    }
}

/// Host-side path validation, matching the kernel's relative-path rule.
fn valid_path(path: &str) -> bool {
    abi::fs::validate_path(path) == Status::Ok
}

/// Verifies and runs a compiled package.
///
/// `args` is the text delivered to `main(args)`; it may be at most 1024 bytes.
/// `budget` bounds the total number of instructions executed.
pub fn run(code: &[u8], args: &[u8], budget: u64) -> Result<RunResult, interpreter::Diagnostic> {
    run_with_input(code, args, HOST_INPUT, budget)
}

/// Runs a package with a caller-supplied console byte script.
///
/// The default [`run`] uses [`HOST_INPUT`]; a test that needs different typed
/// bytes (a backspace, a different command) passes its own. The script is
/// drained by `core.read_input`, so it is exactly what a person would type,
/// including newlines.
pub fn run_with_input(
    code: &[u8],
    args: &[u8],
    input: &[u8],
    budget: u64,
) -> Result<RunResult, interpreter::Diagnostic> {
    let mut host = ConsoleHost::with_input(0, input);
    run_with_host(code, args, budget, &mut host)
}

fn run_with_host(
    code: &[u8],
    args: &[u8],
    budget: u64,
    host: &mut ConsoleHost,
) -> Result<RunResult, interpreter::Diagnostic> {
    if args.len() > 1024 {
        // The language's `string[1024]` argument is a compile/loader contract;
        // report it as a capacity failure rather than truncating.
        return Err(interpreter::Diagnostic {
            status: abi::Status::CapacityExceeded,
            offset: 0,
            message: "args is longer than 1024 bytes",
        });
    }

    let mut scratch = vec![0u8; code.len().div_ceil(32)];
    let program = Program::parse(code, &mut scratch)?;
    let header = *program.header();
    let mut state = vec![0u8; header.state_size as usize];
    let mut stack = vec![0u8; header.stack_size as usize];
    let mut argument_buffer = vec![0u8; 8 + 1024];
    argument_buffer[..8].copy_from_slice(&(args.len() as u64).to_le_bytes());
    argument_buffer[8..8 + args.len()].copy_from_slice(args);

    let mut machine = Machine::new();
    let mut remaining = budget;
    let reason = loop {
        let slice = remaining.min(1_000_000);
        match machine.run(
            &program,
            host,
            &mut state,
            &mut stack,
            &argument_buffer,
            slice,
        ) {
            Outcome::Exited(reason) => break reason,
            Outcome::Blocked(_) => {
                // `ConsoleHost::wait` never returns `Blocked`; a package that
                // reaches here is broken rather than merely slow.
                break ExitReason::Faulted(FaultCode::IllegalOperation);
            }
            Outcome::Yielded => {
                remaining = remaining.saturating_sub(slice);
                if remaining == 0 {
                    break ExitReason::Faulted(FaultCode::BudgetExceeded);
                }
            }
        }
    };

    let fault_at = if matches!(reason, ExitReason::Faulted(_)) {
        // `pc` is the instruction after the faulting one, so look up the
        // position inside it.
        program.line_for(machine.pc().saturating_sub(1))
    } else {
        None
    };

    Ok(RunResult {
        output: std::mem::take(&mut host.output),
        reason,
        fault_at,
    })
}

/// Writes a package in the `langhost` container format.
///
/// ```text
/// "IGPK" | manifest_len: u32-LE | code_len: u32-LE | manifest | code
/// ```
///
/// The container is a host convenience for inspecting or archiving a package;
/// the image itself never carries bytecode (ADR 0019).
pub fn write_package(path: &std::path::Path, package: &OwnedPackage) -> std::io::Result<()> {
    let mut file = std::fs::File::create(path)?;
    file.write_all(b"IGPK")?;
    file.write_all(&(package.manifest.len() as u32).to_le_bytes())?;
    file.write_all(&(package.code.len() as u32).to_le_bytes())?;
    file.write_all(&package.manifest)?;
    file.write_all(&package.code)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The repository's staged source tree; the same one the image is built
    /// from. `compile_in` is `compile` with an explicit root, so this test does
    /// not depend on the process working directory or `IGIS_APP_DIR`.
    fn sample_tree() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../samples/apps")
    }

    #[test]
    fn an_application_imports_lib_math_through_the_tree_reader() {
        let source = br#"
unit module_host;
uses core, lib.math;

var
  n: int;

procedure main(args: const string[1024]);
begin
  n := math.clamp(42, 0, 9);
  if n = 9
     and math.min_int(2, 5) = 2
     and math.max_int(2, 5) = 5
     and math.abs_int(0 - 3) = 3
     and math.sign(0 - 3) = 0 - 1 then
    core.write("module: ok\n")
  else
    core.write("module: FAILED\n");
end;
"#;
        let package = compile_in(source, "module_host", &sample_tree()).expect("compiles");
        let result = run(&package.code, b"", DEFAULT_BUDGET).expect("verifies and runs");
        assert_eq!(String::from_utf8(result.output).unwrap(), "module: ok\n");
        assert_eq!(result.reason, ExitReason::Ok);
    }

    #[test]
    fn an_in_memory_reader_supplies_modules_to_compile() {
        // spec 0018 X5: the host path is reader-agnostic, so a test can supply
        // module sources without touching the filesystem.
        struct Memory {
            path: &'static str,
            source: &'static [u8],
        }
        impl compiler::SourceReader for Memory {
            fn size(&self, path: &str) -> Result<usize, Status> {
                if path == self.path {
                    Ok(self.source.len())
                } else {
                    Err(Status::NotFound)
                }
            }
            fn read(&self, path: &str, out: &mut [u8]) -> Result<usize, Status> {
                if path != self.path {
                    return Err(Status::NotFound);
                }
                if out.len() < self.source.len() {
                    return Err(Status::CapacityExceeded);
                }
                out[..self.source.len()].copy_from_slice(self.source);
                Ok(self.source.len())
            }
        }
        let source = br#"
unit in_memory;
uses core, lib.math;

var
  n: int;

procedure main(args: const string[1024]);
begin
  n := math.clamp(0 - 5, 0, 10);
  if n = 0 then
    core.write("memory: ok\n")
  else
    core.write("memory: FAILED\n");
end;
"#;
        let math: &'static [u8] = include_bytes!("../../samples/apps/lib/math.ig");
        let mut reader = Memory {
            path: "lib/math.ig",
            source: math,
        };
        let package = compile_with_reader(source, "in_memory", &mut reader).expect("compiles");
        let result = run(&package.code, b"", DEFAULT_BUDGET).expect("verifies and runs");
        assert_eq!(String::from_utf8(result.output).unwrap(), "memory: ok\n");
        assert_eq!(result.reason, ExitReason::Ok);
    }

    #[test]
    fn a_missing_module_is_a_compile_diagnostic() {
        let source = br#"
unit missing_module;
uses lib.no_such_module;

procedure main(args: const string[1024]);
begin
end;
"#;
        let error = compile_in(source, "missing_module", &sample_tree())
            .expect_err("the reader cannot find it");
        assert_eq!(error.path, "lib/no_such_module.ig");
        assert!(
            error.message.contains("cannot read `lib.no_such_module`"),
            "{}",
            error.message
        );
    }

    #[test]
    fn package_container_records_both_sections() {
        let source = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../samples/lang/hello.ig"
        ))
        .expect("sample");
        let package = compile(&source, "hello.ig").expect("compiles");
        let path =
            std::env::temp_dir().join(format!("igis-package-test-{}.bin", std::process::id()));
        write_package(&path, &package).expect("write");
        let bytes = std::fs::read(&path).expect("read");
        let _ = std::fs::remove_file(&path);

        assert_eq!(&bytes[0..4], b"IGPK");
        let manifest_len = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
        let code_len = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;
        assert_eq!(manifest_len, package.manifest.len());
        assert_eq!(code_len, package.code.len());
        assert_eq!(bytes.len(), 12 + manifest_len + code_len);
        assert_eq!(&bytes[12..12 + manifest_len], &package.manifest[..]);
        assert_eq!(&bytes[12 + manifest_len..], &package.code[..]);
    }

    #[test]
    fn oversized_args_are_refused() {
        let error = run(&[], &vec![b'x'; 1025], 10).expect_err("must be refused");
        assert_eq!(error.status, abi::Status::CapacityExceeded);
    }

    /// The `stdlib` framing helpers on top of the host's loopback FIFO
    /// (spec 0006, "Interfaces"). Every case is a `report(name, passed)` line,
    /// so the assertion pins the status, the payload and the "destination
    /// untouched" promise of each error path.
    #[test]
    fn framing_helpers_round_trip_and_reject_bad_lengths() {
        let source = br#"
unit framing;

uses core, stdlib;

var
  out: out_stream[64];
  inp: in_stream[64];
  chunk64: string[64];
  long: string[320];
  max: string[252];
  digits: string[32];
  line: bytes[256];
  frame: bytes[256];
  kept: bytes[256];
  big: bytes[320];
  header: bytes[4];
  value: int;
  n: int;
  i: int;
  st: Status;
  st2: Status;

procedure report(name: const string[0]; passed: bool);
begin
  if passed then
    core.write("ok ")
  else
    core.write("FAILED ");
  core.write(name);
  core.write("\n");
end;

procedure main(args: const string[1024]);
begin
  chunk64 := "0123456789012345678901234567890123456789012345678901234567890123";
  long := chunk64 + chunk64 + chunk64 + chunk64 + chunk64;

  // ---- recv_exact bounds: nothing is read, so nothing is consumed --------
  kept := "keep";
  st := stdlib.recv_exact(inp, kept, -1);
  report("exact negative is Invalid", st = Invalid and kept = "keep");
  st := stdlib.recv_exact(inp, kept, 257);
  report("exact over capacity is CapacityExceeded", st = CapacityExceeded and kept = "keep");

  // ---- newline-delimited text ------------------------------------------
  st := stdlib.send_line(out, "alpha");
  st2 := stdlib.recv_line_into(inp, line);
  report("line round trip", st = Ok and st2 = Ok and line = "alpha");

  st := stdlib.send_bytes(out, "crlf\r\n");
  st2 := stdlib.recv_line_into(inp, line);
  report("crlf is stripped", st2 = Ok and line = "crlf");

  st := stdlib.send_bytes(out, "\n");
  st2 := stdlib.recv_line_into(inp, line);
  report("empty line", st2 = Ok and int(core.len(line)) = 0);

  i := 0;
  while i < 300 do
  begin
    big[i] := byte(65);
    i := i + 1;
  end;
  big[300] := byte(10);
  st := stdlib.send_bytes(out, big);
  kept := "keep";
  st2 := stdlib.recv_line_into(inp, kept);
  report("overlong line is CapacityExceeded", st2 = CapacityExceeded and kept = "keep");

  st := stdlib.send_line(out, "after");
  st2 := stdlib.recv_line_into(inp, line);
  report("overlong line keeps the stream in sync", st2 = Ok and line = "after");

  // ---- length-prefixed frames ------------------------------------------
  st := stdlib.send_frame(out, "hello frame");
  st2 := stdlib.recv_frame_into(inp, frame);
  report("frame round trip", st = Ok and st2 = Ok and frame = "hello frame");

  if core.substr_into(max, long, 0, 252) <> 252 then
    report("frame payload of 252 bytes fits", false)
  else
  begin
    st := stdlib.send_frame(out, max);
    st2 := stdlib.recv_frame_into(inp, frame);
    report("frame payload of 252 bytes fits", st = Ok and st2 = Ok and int(core.len(frame)) = 252);
  end;

  st := stdlib.send_frame(out, long);
  report("frame payload of 320 bytes is CapacityExceeded", st = CapacityExceeded);

  // The failed send queued nothing: the next frame is the next send's.
  st := stdlib.send_frame(out, "tiny");
  st2 := stdlib.recv_frame_into(inp, frame);
  report("failed frame sends nothing", st2 = Ok and frame = "tiny");

  header[0] := byte(44);
  header[1] := byte(1);
  header[2] := byte(0);
  header[3] := byte(0);
  st := stdlib.send_bytes(out, header);
  kept := "keep";
  st2 := stdlib.recv_frame_into(inp, kept);
  report("unsatisfiable frame length is CapacityExceeded",
    st2 = CapacityExceeded and kept = "keep");

  // ---- typed u64 view ---------------------------------------------------
  st := stdlib.send_u64(out, 0x0102030405060708);
  st2 := stdlib.recv_exact(inp, frame, 8);
  report("u64 is little endian", st2 = Ok and int(frame[0]) = 8 and int(frame[7]) = 1);

  st := stdlib.send_u64(out, 42);
  st2 := stdlib.recv_u64(inp, value);
  report("u64 positive round trip", st2 = Ok and value = 42);

  st := stdlib.send_u64(out, -1);
  st2 := stdlib.recv_u64(inp, value);
  report("u64 -1 round trip", st2 = Ok and value = -1);

  st := stdlib.send_u64(out, -9223372036854775807 - 1);
  st2 := stdlib.recv_u64(inp, value);
  report("u64 MinInt round trip", st2 = Ok and value = -9223372036854775807 - 1);

  st := stdlib.send_u64(out, 9223372036854775807);
  st2 := stdlib.recv_u64(inp, value);
  report("u64 MaxInt round trip", st2 = Ok and value = 9223372036854775807);
end;
"#;
        let package = compile(source, "framing.ig").expect("compiles");
        let result = run(&package.code, b"", DEFAULT_BUDGET).expect("verifies and runs");
        let output = String::from_utf8(result.output).expect("UTF-8 output");
        let mut expected = String::new();
        for name in [
            "exact negative is Invalid",
            "exact over capacity is CapacityExceeded",
            "line round trip",
            "crlf is stripped",
            "empty line",
            "overlong line is CapacityExceeded",
            "overlong line keeps the stream in sync",
            "frame round trip",
            "frame payload of 252 bytes fits",
            "frame payload of 320 bytes is CapacityExceeded",
            "failed frame sends nothing",
            "unsatisfiable frame length is CapacityExceeded",
            "u64 is little endian",
            "u64 positive round trip",
            "u64 -1 round trip",
            "u64 MinInt round trip",
            "u64 MaxInt round trip",
        ] {
            expected.push_str("ok ");
            expected.push_str(name);
            expected.push('\n');
        }
        assert_eq!(output, expected);
        assert_eq!(result.reason, ExitReason::Ok);
    }
}
