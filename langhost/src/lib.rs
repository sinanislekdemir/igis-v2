//! Host-side driver for the application language.
//!
//! This is the "language track" from spec 0005: the compiler and interpreter
//! linked together with a host implementation of the console and the clock.
//! The kernel will link the same two crates against its own [`Host`], so
//! nothing here is part of the language itself.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::time::Instant;

use abi::{ExitReason, FaultCode, Status};
use interpreter::{Host, Machine, Outcome, Program, WaitOutcome};

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
}

/// Host-side path validation, matching the kernel's relative-path rule.
fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.ends_with('/')
        && !path.split('/').any(|part| part.is_empty() || part == "..")
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
}
