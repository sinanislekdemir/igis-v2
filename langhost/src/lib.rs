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

/// Directory `core.spawn` resolves paths against, unless `IGIS_APP_DIR` is
/// set. It matches the source directory `xtask image` stages (ADR 0019).
const DEFAULT_APP_DIR: &str = "samples/apps";

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
/// This is the host wrapper around [`compiler::compile_into`]: it allocates the
/// boundeds buffers the kernel will later place statically, compiles, and copies
/// the artifact out so the diagnostic can outlive the scratch.
#[allow(clippy::result_large_err)]
pub fn compile(source: &[u8], path: &str) -> Result<OwnedPackage, compiler::Diagnostic> {
    let mut arena = vec![0u8; compiler::MAX_ARENA_BYTES];
    let mut code = vec![0u8; compiler::MAX_CODE_BYTES];
    let mut manifest = vec![0u8; compiler::MAX_MANIFEST_BYTES];
    let package = compiler::compile_into(source, path, &mut arena, &mut code, &mut manifest)?;
    Ok(OwnedPackage {
        manifest: package.manifest.to_vec(),
        code: package.code.to_vec(),
    })
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
/// The host has no kernel line ring, so `core.read_line` is a scripted stub:
/// it hands out one complete line per call from [`HOST_LINES`], then an empty
/// line forever. A host-side shell therefore runs the same code path as the
/// kernel's, deterministically, without ever blocking. A program that keeps
/// reading after the script is exhausted sees empty lines and either exits or
/// is stopped by the budget watchdog.
const HOST_LINES: [&[u8]; 4] = [b"help", b"run child world", b"echo hi there", b"exit"];

struct ConsoleHost {
    output: Vec<u8>,
    depth: u32,
    source_dir: PathBuf,
    children: HashMap<u64, ExitReason>,
    next_id: u64,
    next_line: usize,
    /// One loopback FIFO for every endpoint (see `stream_send`).
    stream: Vec<u8>,
    /// Record-shaped events queued by `syslib.inject_event`, oldest first.
    events: Vec<Vec<u8>>,
}

impl ConsoleHost {
    fn new(depth: u32) -> Self {
        ConsoleHost {
            output: Vec::new(),
            depth,
            source_dir: app_dir(),
            children: HashMap::new(),
            next_id: 1,
            next_line: 0,
            stream: Vec::new(),
            events: Vec::new(),
        }
    }
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

    fn read_line(&mut self, buf: &mut [u8]) -> Option<usize> {
        let line = HOST_LINES.get(self.next_line).copied().unwrap_or(b"");
        self.next_line += 1;
        let copied = line.len().min(buf.len());
        buf[..copied].copy_from_slice(&line[..copied]);
        Some(copied)
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
    // still a host-side block (`IllegalOperation`), exactly like `read_line`'s
    // stalled script or an empty loopback receive. The probes model the three
    // source kinds faithfully: a scripted line remains, an injected event is
    // queued, or the loopback FIFO has bytes.
    fn console_line_ready(&mut self) -> bool {
        self.next_line < HOST_LINES.len()
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

        let source = match std::fs::read(self.source_dir.join(path)) {
            Ok(source) => source,
            Err(error) => {
                let status = if error.kind() == std::io::ErrorKind::NotFound {
                    Status::NotFound
                } else {
                    Status::IoError
                };
                eprintln!("langhost: spawn `{path}`: {error}");
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
        let package = match compile(&source, path) {
            Ok(package) => package,
            Err(diagnostic) => {
                eprintln!("langhost: spawn `{path}`: {diagnostic}");
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

        let mut child = ConsoleHost::new(self.depth + 1);
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
                eprintln!("langhost: spawn `{path}`: verifier rejected it: {diagnostic}");
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

    // Application files (spec 0013). `langhost` has a real filesystem, so it
    // implements the same root-only view over the sample source directory the
    // image is built from: listing is sorted by name (the ext2 order is not
    // sorted, so no program may depend on it), reads are chunked at the same
    // 256 bytes, and subdirectories/symlinks are refused exactly as the kernel
    // subset does.
    fn fs_list(&mut self, path: &[u8], index: u64, name: &mut [u8]) -> Result<FsEntry, Status> {
        let path = std::str::from_utf8(path).map_err(|_| Status::Invalid)?;
        let status = abi::fs::validate_root(path);
        if status != Status::Ok {
            return Err(status);
        }
        let index = usize::try_from(index).map_err(|_| Status::NotFound)?;
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(&self.source_dir).map_err(|_| Status::IoError)? {
            let entry = entry.map_err(|_| Status::IoError)?;
            let entry_name = entry
                .file_name()
                .into_string()
                .map_err(|_| Status::Invalid)?;
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
        let status = abi::fs::validate_root_name(path);
        if status != Status::Ok {
            return Err(status);
        }
        let target = self.source_dir.join(path);
        let metadata = match std::fs::symlink_metadata(&target) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(Status::NotFound)
            }
            Err(_) => return Err(Status::IoError),
        };
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
        let status = abi::fs::validate_root_name(path);
        if status != Status::Ok {
            return Err(status);
        }
        let target = self.source_dir.join(path);
        let metadata = match std::fs::symlink_metadata(&target) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(Status::NotFound)
            }
            Err(_) => return Err(Status::IoError),
        };
        if !metadata.is_file() {
            return Err(Status::Invalid);
        }
        Ok(metadata.len())
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
    let mut host = ConsoleHost::new(0);
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
