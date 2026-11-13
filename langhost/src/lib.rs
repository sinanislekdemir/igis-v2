//! Host-side driver for the application language.
//!
//! This is the "language track" from spec 0005: the compiler and interpreter
//! linked together with a host implementation of the console and the clock.
//! The kernel will link the same two crates against its own [`Host`], so
//! nothing here is part of the language itself.

#![forbid(unsafe_code)]

use std::io::Write;
use std::time::Instant;

use abi::{ExitReason, FaultCode};
use interpreter::{Host, Machine, Outcome, Program};

/// Default instruction budget for one `langhost` run. This is a host-side
/// watchdog, not a scheduler quantum: the machine is resumed between slices
/// until the budget is exhausted.
pub const DEFAULT_BUDGET: u64 = 100_000_000;

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
struct ConsoleHost {
    started: Instant,
    output: Vec<u8>,
}

impl ConsoleHost {
    fn new() -> Self {
        ConsoleHost {
            started: Instant::now(),
            output: Vec::new(),
        }
    }
}

impl Host for ConsoleHost {
    fn console_write(&mut self, bytes: &[u8]) {
        self.output.extend_from_slice(bytes);
    }

    fn clock(&mut self) -> u64 {
        self.started.elapsed().as_nanos() as u64
    }
}

/// Verifies and runs a compiled package.
///
/// `args` is the text delivered to `main(args)`; it may be at most 1024 bytes.
/// `budget` bounds the total number of instructions executed.
pub fn run(code: &[u8], args: &[u8], budget: u64) -> Result<RunResult, interpreter::Diagnostic> {
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
    let mut host = ConsoleHost::new();
    let mut remaining = budget;
    let reason = loop {
        let slice = remaining.min(1_000_000);
        match machine.run(
            &program,
            &mut host,
            &mut state,
            &mut stack,
            &argument_buffer,
            slice,
        ) {
            Outcome::Exited(reason) => break reason,
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
        output: host.output,
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
pub fn write_package(path: &std::path::Path, package: &compiler::Package) -> std::io::Result<()> {
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
        let package = compiler::compile(&source, "hello.ig").expect("compiles");
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
