//! Structural-verification and execution tests.
//!
//! Packages are hand-assembled here on purpose: the interpreter must be
//! testable without the compiler, because the two crates never depend on each
//! other (spec 0008, B12).

use abi::opcodes;
use abi::{
    CodeHeader, Function, Status, ABI_VERSION, CODE_MAGIC, NO_DIAGNOSTIC, NO_NAME,
    PROFILE_X86_64_LE,
};
use interpreter::{ExitReason, FaultCode, Host, Machine, Outcome, Program, WaitKind, WaitOutcome};

#[derive(Clone, Copy)]
struct FnSpec {
    code_off: u32,
    frame_size: u32,
    operand_off: u32,
    param_count: u8,
    has_destination: bool,
}

const MAIN: FnSpec = FnSpec {
    code_off: 0,
    frame_size: 40,
    operand_off: 40,
    param_count: 1,
    has_destination: false,
};

fn build(
    functions: &[FnSpec],
    instructions: &[u8],
    constants: &[u8],
    state_size: u32,
    stack_size: u32,
    entry: u32,
) -> Vec<u8> {
    let header = CodeHeader {
        magic: CODE_MAGIC,
        abi_version: ABI_VERSION,
        profile: PROFILE_X86_64_LE,
        function_count: functions.len() as u32,
        entry,
        code_len: instructions.len() as u32,
        const_len: constants.len() as u32,
        line_len: 0,
        state_size,
        stack_size,
        diag_offset: NO_DIAGNOSTIC,
    };
    let mut code = Vec::new();
    code.extend_from_slice(&header.to_bytes());
    for function in functions {
        code.extend_from_slice(
            &Function {
                code_off: function.code_off,
                frame_size: function.frame_size,
                operand_off: function.operand_off,
                line_base: 0,
                name_off: NO_NAME,
                param_count: function.param_count,
                has_destination: function.has_destination,
                _reserved: [0, 0],
            }
            .to_bytes(),
        );
    }
    while code.len() % 8 != 0 {
        code.push(0);
    }
    code.extend_from_slice(instructions);
    while code.len() % 8 != 0 {
        code.push(0);
    }
    code.extend_from_slice(constants);
    while code.len() % 8 != 0 {
        code.push(0);
    }
    code
}

fn op(out: &mut Vec<u8>, opcode: u8) {
    out.extend_from_slice(&[opcode, 0, 0, 0]);
}

fn op_imm(out: &mut Vec<u8>, opcode: u8, immediate: u32) {
    op(out, opcode);
    out.extend_from_slice(&immediate.to_le_bytes());
}

fn op_const(out: &mut Vec<u8>, value: i64) {
    op(out, opcodes::CONST);
    out.extend_from_slice(&(value as u32).to_le_bytes());
    out.extend_from_slice(&((value >> 32) as u32).to_le_bytes());
}

fn constant(data: &[u8]) -> Vec<u8> {
    let mut out = (data.len() as u64).to_le_bytes().to_vec();
    out.extend_from_slice(data);
    while !out.len().is_multiple_of(8) {
        out.push(0);
    }
    out
}

fn scratch_for(code: &[u8]) -> Vec<u8> {
    vec![0u8; code.len().div_ceil(32)]
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

fn execute(code: &[u8], state_size: usize, stack_size: usize) -> (ExitReason, Vec<u8>) {
    let mut scratch = scratch_for(code);
    let program = Program::parse(code, &mut scratch).expect("package must verify");
    let mut state = vec![0u8; state_size];
    let mut stack = vec![0u8; stack_size];
    let args = vec![0u8; 8 + 1024];
    let mut machine = Machine::new();
    let mut host = Recorder { output: Vec::new() };
    let outcome = machine.run(&program, &mut host, &mut state, &mut stack, &args, 10_000);
    match outcome {
        Outcome::Exited(reason) => (reason, host.output),
        Outcome::Blocked(kind) => panic!("machine blocked on {kind:?}"),
        Outcome::Yielded => panic!("machine yielded without a budget limit"),
    }
}

#[test]
fn minimal_package_runs_to_ok() {
    let mut instructions = Vec::new();
    op(&mut instructions, opcodes::RET);
    let code = build(&[MAIN], &instructions, &[], 0, 64, 0);
    let (reason, output) = execute(&code, 0, 64);
    assert_eq!(reason, ExitReason::Ok);
    assert!(output.is_empty());
}

#[test]
fn unknown_opcode_is_rejected() {
    let mut instructions = Vec::new();
    op(&mut instructions, 0xEE);
    let code = build(&[MAIN], &instructions, &[], 0, 64, 0);
    let mut scratch = scratch_for(&code);
    let error = interpreter::verify(&code, &mut scratch).expect_err("must be refused");
    assert_eq!(error.status, Status::Invalid);
    assert!(
        error.message.contains("unknown opcode"),
        "{}",
        error.message
    );
}

#[test]
fn jump_into_the_middle_of_an_instruction_is_rejected() {
    let mut instructions = Vec::new();
    op_const(&mut instructions, 0);
    op_imm(&mut instructions, opcodes::JMP, u32::MAX); // rel = -1 -> mid-word
    op_imm(&mut instructions, opcodes::LOAD_STATE, 0);
    op(&mut instructions, opcodes::RET);
    let code = build(&[MAIN], &instructions, &[], 8, 64, 0);
    let mut scratch = scratch_for(&code);
    let error = interpreter::verify(&code, &mut scratch).expect_err("must be refused");
    assert_eq!(error.status, Status::Invalid);
    assert!(error.message.contains("jump target"), "{}", error.message);
}

#[test]
fn constant_length_past_the_blob_is_rejected() {
    let mut instructions = Vec::new();
    op_imm(&mut instructions, opcodes::REF_CONST, 8);
    op(&mut instructions, opcodes::RET);
    // A header that claims 100 bytes of payload in an 8-byte blob.
    let constants = 100u64.to_le_bytes().to_vec();
    let code = build(&[MAIN], &instructions, &constants, 0, 64, 0);
    let mut scratch = scratch_for(&code);
    let error = interpreter::verify(&code, &mut scratch).expect_err("must be refused");
    assert_eq!(error.status, Status::Invalid);
    assert!(
        error.message.contains("constant range"),
        "{}",
        error.message
    );
}

#[test]
fn abi_version_mismatch_is_rejected() {
    let mut instructions = Vec::new();
    op(&mut instructions, opcodes::RET);
    let mut code = build(&[MAIN], &instructions, &[], 0, 64, 0);
    code[4..6].copy_from_slice(&2u16.to_le_bytes());
    let mut scratch = scratch_for(&code);
    let error = interpreter::verify(&code, &mut scratch).expect_err("must be refused");
    assert_eq!(error.status, Status::Invalid);
    assert!(error.message.contains("ABI version"), "{}", error.message);
}

#[test]
fn truncated_blob_is_rejected() {
    let mut instructions = Vec::new();
    op(&mut instructions, opcodes::RET);
    let mut code = build(&[MAIN], &instructions, &[], 0, 64, 0);
    code.pop();
    let mut scratch = vec![0u8; 8];
    let error = interpreter::verify(&code, &mut scratch).expect_err("must be refused");
    assert_eq!(error.status, Status::Invalid);
}

#[test]
fn too_small_scratch_is_a_capacity_error() {
    let mut instructions = Vec::new();
    op(&mut instructions, opcodes::RET);
    let code = build(&[MAIN], &instructions, &[], 0, 64, 0);
    let mut scratch = [0u8; 0];
    let error = Program::parse(&code, &mut scratch).expect_err("must be refused");
    assert_eq!(error.status, Status::CapacityExceeded);
}

/// `Append` with a capacity of 4 and a five-byte literal: runtime
/// `CapacityExceeded`, not a panic.
fn append_package(capacity: u32) -> Vec<u8> {
    let mut instructions = Vec::new();
    op_imm(&mut instructions, opcodes::REF_STATE, 8);
    op(&mut instructions, opcodes::CLEAR);
    op_imm(&mut instructions, opcodes::REF_STATE, 8);
    op_imm(&mut instructions, opcodes::REF_CONST, 8);
    op_imm(&mut instructions, opcodes::APPEND, capacity);
    op(&mut instructions, opcodes::RET);
    let constants = constant(b"abcde");
    let spec = FnSpec {
        frame_size: 56,
        operand_off: 40,
        ..MAIN
    };
    build(&[spec], &instructions, &constants, 16, 64, 0)
}

#[test]
fn capacity_overflow_faults() {
    let code = append_package(4);
    let (reason, _) = execute(&code, 16, 64);
    assert_eq!(reason, ExitReason::Faulted(FaultCode::CapacityExceeded));
}

#[test]
fn destination_past_the_region_faults_out_of_bounds() {
    let code = append_package(100);
    let (reason, _) = execute(&code, 16, 64);
    assert_eq!(reason, ExitReason::Faulted(FaultCode::OutOfBounds));
}

#[test]
fn operand_underflow_faults_instead_of_panicking() {
    let mut instructions = Vec::new();
    op(&mut instructions, opcodes::DROP);
    op(&mut instructions, opcodes::RET);
    let code = build(&[MAIN], &instructions, &[], 0, 64, 0);
    let (reason, _) = execute(&code, 0, 64);
    assert_eq!(reason, ExitReason::Faulted(FaultCode::IllegalOperation));
}

#[test]
fn a_budget_yields_and_resumes_at_the_same_boundary() {
    let mut instructions = Vec::new();
    op_const(&mut instructions, 0);
    op(&mut instructions, opcodes::DROP);
    op(&mut instructions, opcodes::RET);
    let spec = FnSpec {
        frame_size: 48,
        ..MAIN
    };
    let code = build(&[spec], &instructions, &[], 0, 64, 0);

    let mut scratch = scratch_for(&code);
    let program = Program::parse(&code, &mut scratch).expect("verifies");
    let mut state = Vec::new();
    let mut stack = vec![0u8; 64];
    let args = vec![0u8; 8 + 1024];
    let mut machine = Machine::new();
    let mut host = Recorder { output: Vec::new() };

    assert_eq!(
        machine.run(&program, &mut host, &mut state, &mut stack, &args, 1),
        Outcome::Yielded
    );
    assert_eq!(machine.pc(), 12); // after `const`, before `drop`
    assert_eq!(
        machine.run(&program, &mut host, &mut state, &mut stack, &args, 100),
        Outcome::Exited(ExitReason::Ok)
    );
}

#[test]
fn console_write_reaches_the_host() {
    let mut instructions = Vec::new();
    op_imm(&mut instructions, opcodes::REF_CONST, 8);
    op_imm(&mut instructions, opcodes::BUILTIN, opcodes::builtin::WRITE);
    op(&mut instructions, opcodes::RET);
    let constants = constant(b"hi");
    let spec = FnSpec {
        frame_size: 48,
        ..MAIN
    };
    let code = build(&[spec], &instructions, &constants, 0, 64, 0);
    let (reason, output) = execute(&code, 0, 64);
    assert_eq!(reason, ExitReason::Ok);
    assert_eq!(output, b"hi");
}

#[test]
fn clock_builtin_pushes_a_value() {
    let mut instructions = Vec::new();
    op_imm(&mut instructions, opcodes::BUILTIN, opcodes::builtin::CLOCK);
    op(&mut instructions, opcodes::DROP);
    op(&mut instructions, opcodes::RET);
    let spec = FnSpec {
        frame_size: 48,
        ..MAIN
    };
    let code = build(&[spec], &instructions, &[], 0, 64, 0);
    let (reason, _) = execute(&code, 0, 64);
    assert_eq!(reason, ExitReason::Ok);
}

#[test]
fn a_frame_that_does_not_fit_faults_stack_overflow() {
    let mut instructions = Vec::new();
    op(&mut instructions, opcodes::RET);
    let code = build(&[MAIN], &instructions, &[], 0, 16, 0);
    let (reason, _) = execute(&code, 0, 16);
    assert_eq!(reason, ExitReason::Faulted(FaultCode::StackOverflow));
}

/// A host that scripts the spawn/wait/kill primitives. `wait` returns its
/// scripted outcomes in order, so a blocking path can be exercised without a
/// scheduler.
#[derive(Default)]
struct MockHost {
    output: Vec<u8>,
    spawn_result: Option<Result<u64, Status>>,
    spawn_args: Option<(Vec<u8>, Vec<u8>)>,
    wait_results: Vec<WaitOutcome>,
    wait_calls: usize,
    read_lines: Vec<Option<Vec<u8>>>,
    read_calls: usize,
    killed: Vec<u64>,
}

impl Host for MockHost {
    fn console_write(&mut self, bytes: &[u8]) {
        self.output.extend_from_slice(bytes);
    }

    fn read_line(&mut self, buf: &mut [u8]) -> Option<usize> {
        let index = self.read_calls;
        self.read_calls += 1;
        match self.read_lines.get(index) {
            Some(Some(line)) => {
                let copied = line.len().min(buf.len());
                buf[..copied].copy_from_slice(&line[..copied]);
                Some(copied)
            }
            _ => None,
        }
    }

    fn clock(&mut self) -> u64 {
        42
    }

    fn spawn(&mut self, path: &[u8], args: &[u8]) -> Result<u64, Status> {
        self.spawn_args = Some((path.to_vec(), args.to_vec()));
        self.spawn_result.unwrap_or(Err(Status::Invalid))
    }

    fn wait(&mut self, _id: u64) -> WaitOutcome {
        let index = self.wait_calls;
        self.wait_calls += 1;
        self.wait_results
            .get(index)
            .copied()
            .unwrap_or(WaitOutcome::Status(Status::NotFound))
    }

    fn kill(&mut self, id: u64) -> Status {
        self.killed.push(id);
        Status::Ok
    }
}

fn run_with_host(
    code: &[u8],
    state: &mut [u8],
    stack: &mut [u8],
    host: &mut MockHost,
    budget: u64,
) -> (Outcome, Machine) {
    let mut scratch = scratch_for(code);
    let program = Program::parse(code, &mut scratch).expect("verifies");
    let args = vec![0u8; 8 + 1024];
    let mut machine = Machine::new();
    let outcome = machine.run(&program, host, state, stack, &args, budget);
    (outcome, machine)
}

fn spawn_package(spawn_result: Result<u64, Status>) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let mut instructions = Vec::new();
    op_imm(&mut instructions, opcodes::REF_CONST, 8);
    op_imm(&mut instructions, opcodes::REF_CONST, 24);
    op_imm(&mut instructions, opcodes::BUILTIN, opcodes::builtin::SPAWN);
    op_imm(&mut instructions, opcodes::STORE_STATE, 0);
    op(&mut instructions, opcodes::RET);
    let mut constants = constant(b"child");
    constants.extend_from_slice(&constant(b"hello"));
    let spec = FnSpec {
        frame_size: 64,
        ..MAIN
    };
    let code = build(&[spec], &instructions, &constants, 8, 64, 0);
    let mut host = MockHost {
        spawn_result: Some(spawn_result),
        ..Default::default()
    };
    let mut state = vec![0u8; 8];
    let mut stack = vec![0u8; 64];
    let (outcome, _) = run_with_host(&code, &mut state, &mut stack, &mut host, 100);
    assert_eq!(outcome, Outcome::Exited(ExitReason::Ok));
    let (path, args) = host.spawn_args.unwrap_or_default();
    (state, path, args)
}

#[test]
fn spawn_pushes_the_reserved_id_and_forwards_both_buffers() {
    let (state, path, args) = spawn_package(Ok(7));
    assert_eq!(u64::from_le_bytes(state[..8].try_into().unwrap()), 7);
    assert_eq!(path, b"child");
    assert_eq!(args, b"hello");
}

#[test]
fn spawn_failure_pushes_the_negated_status() {
    let (state, _, _) = spawn_package(Err(Status::Full));
    assert_eq!(
        u64::from_le_bytes(state[..8].try_into().unwrap()),
        (-5i64) as u64
    );
}

#[test]
fn wait_blocks_without_advancing_and_retries_the_same_instruction() {
    let mut instructions = Vec::new();
    op_const(&mut instructions, 3);
    op_imm(&mut instructions, opcodes::BUILTIN, opcodes::builtin::WAIT);
    op_imm(&mut instructions, opcodes::STORE_STATE, 0);
    op(&mut instructions, opcodes::RET);
    let spec = FnSpec {
        frame_size: 48,
        ..MAIN
    };
    let code = build(&[spec], &instructions, &[], 8, 64, 0);

    let mut host = MockHost {
        wait_results: vec![
            WaitOutcome::Blocked,
            WaitOutcome::Exited(ExitReason::Faulted(FaultCode::Overflow)),
        ],
        ..Default::default()
    };
    let mut state = vec![0u8; 8];
    let mut stack = vec![0u8; 64];

    let (outcome, machine) = run_with_host(&code, &mut state, &mut stack, &mut host, 100);
    assert_eq!(outcome, Outcome::Blocked(WaitKind::ChildExit(3)));
    assert_eq!(machine.pc(), 12, "pc stays on the wait instruction");
    assert_eq!(state, vec![0u8; 8], "the id is not consumed or stored");

    let (outcome, _) = run_with_host(&code, &mut state, &mut stack, &mut host, 100);
    assert_eq!(outcome, Outcome::Exited(ExitReason::Ok));
    assert_eq!(host.wait_calls, 2);
    assert_eq!(
        u64::from_le_bytes(state[..8].try_into().unwrap()),
        2 + FaultCode::Overflow as u64
    );
}

#[test]
fn wait_maps_exit_reasons_and_status_errors() {
    for (outcome, expected) in [
        (WaitOutcome::Exited(ExitReason::Ok), 0i64),
        (WaitOutcome::Exited(ExitReason::Killed), 1),
        (WaitOutcome::Status(Status::NotFound), -3),
    ] {
        let mut instructions = Vec::new();
        op_const(&mut instructions, 9);
        op_imm(&mut instructions, opcodes::BUILTIN, opcodes::builtin::WAIT);
        op_imm(&mut instructions, opcodes::STORE_STATE, 0);
        op(&mut instructions, opcodes::RET);
        let spec = FnSpec {
            frame_size: 48,
            ..MAIN
        };
        let code = build(&[spec], &instructions, &[], 8, 64, 0);
        let mut host = MockHost {
            wait_results: vec![outcome],
            ..Default::default()
        };
        let mut state = vec![0u8; 8];
        let mut stack = vec![0u8; 64];
        let (result, _) = run_with_host(&code, &mut state, &mut stack, &mut host, 100);
        assert_eq!(result, Outcome::Exited(ExitReason::Ok));
        assert_eq!(
            i64::from_le_bytes(state[..8].try_into().unwrap()),
            expected,
            "{outcome:?}"
        );
    }
}

#[test]
fn read_line_blocks_then_copies_the_line_into_the_buffer() {
    let mut instructions = Vec::new();
    op_imm(&mut instructions, opcodes::REF_STATE, 8);
    op_imm(&mut instructions, opcodes::READ_LINE, 8);
    op_imm(&mut instructions, opcodes::STORE_STATE, 16);
    op(&mut instructions, opcodes::RET);
    let spec = FnSpec {
        frame_size: 64,
        ..MAIN
    };
    let code = build(&[spec], &instructions, &[], 24, 64, 0);

    let mut host = MockHost {
        read_lines: vec![None, Some(b"alpha".to_vec())],
        ..Default::default()
    };
    let mut state = vec![0u8; 24];
    let mut stack = vec![0u8; 64];

    let (outcome, machine) = run_with_host(&code, &mut state, &mut stack, &mut host, 100);
    assert_eq!(outcome, Outcome::Blocked(WaitKind::Console));
    assert_eq!(machine.pc(), 8, "pc stays on the read_line instruction");
    assert_eq!(
        state,
        vec![0u8; 24],
        "nothing is written before a line exists"
    );

    let (outcome, _) = run_with_host(&code, &mut state, &mut stack, &mut host, 100);
    assert_eq!(outcome, Outcome::Exited(ExitReason::Ok));
    assert_eq!(host.read_calls, 2);
    assert_eq!(u64::from_le_bytes(state[0..8].try_into().unwrap()), 5);
    assert_eq!(&state[8..13], b"alpha");
    assert_eq!(u64::from_le_bytes(state[16..24].try_into().unwrap()), 5);
}

#[test]
fn read_line_truncates_to_the_buffer_capacity() {
    let mut instructions = Vec::new();
    op_imm(&mut instructions, opcodes::REF_STATE, 8);
    op_imm(&mut instructions, opcodes::READ_LINE, 4);
    op_imm(&mut instructions, opcodes::STORE_STATE, 16);
    op(&mut instructions, opcodes::RET);
    let spec = FnSpec {
        frame_size: 64,
        ..MAIN
    };
    let code = build(&[spec], &instructions, &[], 24, 64, 0);

    let mut host = MockHost {
        read_lines: vec![Some(b"alphabet".to_vec())],
        ..Default::default()
    };
    let mut state = vec![0u8; 24];
    let mut stack = vec![0u8; 64];
    let (outcome, _) = run_with_host(&code, &mut state, &mut stack, &mut host, 100);
    assert_eq!(outcome, Outcome::Exited(ExitReason::Ok));
    assert_eq!(u64::from_le_bytes(state[0..8].try_into().unwrap()), 4);
    assert_eq!(&state[8..12], b"alph");
    assert_eq!(u64::from_le_bytes(state[16..24].try_into().unwrap()), 4);
}

#[test]
fn kill_forwards_the_id_and_pushes_zero() {
    let mut instructions = Vec::new();
    op_const(&mut instructions, 4);
    op_imm(&mut instructions, opcodes::BUILTIN, opcodes::builtin::KILL);
    op_imm(&mut instructions, opcodes::STORE_STATE, 0);
    op(&mut instructions, opcodes::RET);
    let spec = FnSpec {
        frame_size: 48,
        ..MAIN
    };
    let code = build(&[spec], &instructions, &[], 8, 64, 0);
    let mut host = MockHost::default();
    let mut state = vec![0u8; 8];
    let mut stack = vec![0u8; 64];
    let (outcome, _) = run_with_host(&code, &mut state, &mut stack, &mut host, 100);
    assert_eq!(outcome, Outcome::Exited(ExitReason::Ok));
    assert_eq!(host.killed, vec![4]);
    assert_eq!(u64::from_le_bytes(state[..8].try_into().unwrap()), 0);
}
