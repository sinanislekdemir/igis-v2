//! Interpreter for the igis-v2 application language.
//!
//! The crate is `#![no_std]` and dependency-free beyond `abi`, because the
//! same code runs inside the kernel. It never allocates and never parses
//! source: the compiler produces a package, [`Program::parse`] verifies it
//! structurally, and [`Machine`] executes it against caller-provided regions.
//!
//! ```no_run
//! # use interpreter::{Machine, Outcome, Program};
//! # struct Host;
//! # impl interpreter::Host for Host {
//! #     fn console_write(&mut self, _bytes: &[u8]) {}
//! #     fn clock(&mut self) -> u64 { 0 }
//! # }
//! # fn run(code: &[u8], scratch: &mut [u8], state: &mut [u8], stack: &mut [u8]) {
//! let program = Program::parse(code, scratch).expect("valid package");
//! let mut machine = Machine::new();
//! let mut host = Host;
//! match machine.run(&program, &mut host, state, stack, &[], 100_000) {
//!     Outcome::Exited(reason) => { /* task finished */ }
//!     Outcome::Yielded => { /* resume later */ }
//! }
//! # }
//! ```

#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

#[cfg(test)]
extern crate std;

mod machine;
mod program;

pub use abi::{ExitReason, FaultCode, Status};
pub use machine::{Host, Machine, Outcome};
pub use program::{verify, Diagnostic, Program};
