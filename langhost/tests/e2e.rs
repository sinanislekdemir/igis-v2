//! End-to-end tests: source → package → verified program → output.

use abi::{ExitReason, FaultCode, Status};
use interpreter::Host;

fn sample(name: &str) -> Vec<u8> {
    let path = format!("{}/../samples/lang/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|error| panic!("cannot read {path}: {error}"))
}

fn run_sample(name: &str, args: &[u8]) -> langhost::RunResult {
    let source = sample(name);
    let package = compiler::compile(&source, name).expect("compiles");
    langhost::run(&package.code, args, langhost::DEFAULT_BUDGET).expect("verifies")
}

struct Recorder {
    output: Vec<u8>,
}

impl Host for Recorder {
    fn console_write(&mut self, bytes: &[u8]) {
        self.output.extend_from_slice(bytes);
    }

    fn clock(&mut self) -> u64 {
        42
    }
}

#[test]
fn hello_world_compiles_and_runs() {
    let result = run_sample("hello.ig", b"");
    assert_eq!(result.output, b"hello, igis\n");
    assert_eq!(result.reason, ExitReason::Ok);
}

#[test]
fn loop_and_branches_produce_the_expected_text() {
    let result = run_sample("loop.ig", b"");
    assert_eq!(
        result.output,
        b"even\nodd\neven\nodd\neven\ntotal=abc\n".to_vec()
    );
    assert_eq!(result.reason, ExitReason::Ok);
}

#[test]
fn concatenation_and_parameters_work() {
    let result = run_sample("concat.ig", b"");
    assert_eq!(result.output, b"greeting: hello, igis\n");
    assert_eq!(result.reason, ExitReason::Ok);
}

#[test]
fn short_circuit_logic_works() {
    let result = run_sample("logic.ig", b"");
    assert_eq!(result.output, b"and\nor\nnot\n");
    assert_eq!(result.reason, ExitReason::Ok);
}

#[test]
fn float_arithmetic_and_casts_work() {
    let result = run_sample("float.ig", b"");
    assert_eq!(result.output, b"float ok\nclock ok\n");
    assert_eq!(result.reason, ExitReason::Ok);
}

#[test]
fn var_scalar_parameters_alias_the_caller() {
    let result = run_sample("params.ig", b"");
    assert_eq!(result.output, b"params ok\n");
    assert_eq!(result.reason, ExitReason::Ok);
}

#[test]
fn bytes_accept_raw_data_that_string_rejects() {
    let result = run_sample("bytes.ig", b"");
    assert_eq!(result.output, b"a\xffb\n");
    assert_eq!(result.reason, ExitReason::Ok);
}

#[test]
fn main_receives_the_argument_buffer() {
    let result = run_sample("args.ig", b"one two");
    assert_eq!(result.output, b"args=[one two]\n");
    assert_eq!(result.reason, ExitReason::Ok);

    let empty = run_sample("args.ig", b"");
    assert_eq!(empty.output, b"args=[]\n");
}

#[test]
fn a_runtime_capacity_overflow_faults() {
    let result = run_sample("runtime_fault.ig", b"");
    assert_eq!(result.output, b"before\n");
    assert_eq!(
        result.reason,
        ExitReason::Faulted(FaultCode::CapacityExceeded)
    );
    assert_eq!(result.fault_at, Some((15, 3)));
}

#[test]
fn compiling_twice_is_byte_identical() {
    let source = sample("loop.ig");
    let first = compiler::compile(&source, "loop.ig").expect("compiles");
    let second = compiler::compile(&source, "loop.ig").expect("compiles");
    assert_eq!(first.code, second.code);
    assert_eq!(first.manifest, second.manifest);
}

#[test]
fn oversized_literal_is_a_compile_error_with_a_position() {
    let error = compiler::compile(&sample("bad_capacity.ig"), "bad_capacity.ig")
        .expect_err("must not compile");
    assert_eq!(error.line, 8);
    assert_eq!(error.column, 11);
    assert!(error.message.contains("does not fit"), "{}", error.message);
    assert_eq!(error.unit, "bad_capacity");
}

#[test]
fn unknown_identifier_is_a_compile_error_with_a_position() {
    let error = compiler::compile(&sample("bad_unknown.ig"), "bad_unknown.ig")
        .expect_err("must not compile");
    assert_eq!(error.line, 5);
    assert_eq!(error.column, 3);
    assert!(
        error.message.contains("unknown identifier `total`"),
        "{}",
        error.message
    );
}

#[test]
fn missing_main_is_a_diagnostic() {
    let error = compiler::compile(&sample("bad_no_main.ig"), "bad_no_main.ig")
        .expect_err("must not compile");
    assert_eq!(error.line, 1);
    assert!(
        error.message.contains("missing `main`"),
        "{}",
        error.message
    );
}

#[test]
fn a_malformed_package_is_refused_by_the_verifier() {
    let source = sample("hello.ig");
    let package = compiler::compile(&source, "hello.ig").expect("compiles");

    let mut scratch = vec![0u8; package.code.len().div_ceil(32)];
    interpreter::verify(&package.code, &mut scratch).expect("the compiler output verifies");

    // Flip the first instruction byte to an undefined opcode.
    let mut corrupted = package.code.clone();
    let instructions = 40 + 24; // padded to 64 by the compiler
    let start = (instructions + 7) & !7;
    corrupted[start] = 0xEE;
    let error = interpreter::verify(&corrupted, &mut scratch).expect_err("must be refused");
    assert_eq!(error.status, Status::Invalid);
}

#[test]
fn host_trait_implementation_sees_application_bytes() {
    // `run` already uses its own host; this checks the trait is usable the way
    // the kernel will use it: a caller-supplied Host over the same machine.
    let source = sample("hello.ig");
    let package = compiler::compile(&source, "hello.ig").expect("compiles");
    let mut scratch = vec![0u8; package.code.len().div_ceil(32)];
    let program = interpreter::Program::parse(&package.code, &mut scratch).expect("verifies");
    let header = *program.header();
    let mut state = vec![0u8; header.state_size as usize];
    let mut stack = vec![0u8; header.stack_size as usize];
    let args = vec![0u8; 8 + 1024];
    let mut machine = interpreter::Machine::new();
    let mut host = Recorder { output: Vec::new() };
    let outcome = machine.run(
        &program,
        &mut host,
        &mut state,
        &mut stack,
        &args,
        langhost::DEFAULT_BUDGET,
    );
    assert_eq!(outcome, interpreter::Outcome::Exited(ExitReason::Ok));
    assert_eq!(host.output, b"hello, igis\n");
    assert_eq!(args[..8], 0u64.to_le_bytes());
}
