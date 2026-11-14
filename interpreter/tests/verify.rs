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
    port_value: u8,
    port_reads: Vec<u16>,
    port_writes: Vec<(u16, u8)>,
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

    fn port_in8(&mut self, port: u16) -> u8 {
        self.port_reads.push(port);
        self.port_value
    }

    fn port_out8(&mut self, port: u16, value: u8) {
        self.port_writes.push((port, value));
    }

    fn spawn(&mut self, path: &[u8], args: &[u8], diag: &mut [u8]) -> Result<u64, Status> {
        self.spawn_args = Some((path.to_vec(), args.to_vec()));
        let result = self.spawn_result.unwrap_or(Err(Status::Invalid));
        match result {
            Ok(_) => abi::layout::diagnostic::clear(diag),
            Err(_) => abi::layout::diagnostic::write(diag, b"unit", b"bad source", 7, 3),
        }
        result
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
    // `Diagnostic` occupies state[16..208]; the id slot is state[0..8].
    op_imm(&mut instructions, opcodes::REF_STATE, 16);
    op_imm(&mut instructions, opcodes::REF_STATE, 0);
    op_imm(&mut instructions, opcodes::BUILTIN, opcodes::builtin::SPAWN);
    op_imm(&mut instructions, opcodes::STORE_STATE, 8);
    op(&mut instructions, opcodes::RET);
    let mut constants = constant(b"child");
    constants.extend_from_slice(&constant(b"hello"));
    let spec = FnSpec {
        frame_size: 72,
        ..MAIN
    };
    let code = build(&[spec], &instructions, &constants, 208, 128, 0);
    let mut host = MockHost {
        spawn_result: Some(spawn_result),
        ..Default::default()
    };
    let mut state = vec![0u8; 208];
    let mut stack = vec![0u8; 128];
    let (outcome, _) = run_with_host(&code, &mut state, &mut stack, &mut host, 100);
    assert_eq!(outcome, Outcome::Exited(ExitReason::Ok));
    let (path, args) = host.spawn_args.unwrap_or_default();
    (state, path, args)
}

fn state_u64(state: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(state[offset..offset + 8].try_into().unwrap())
}

#[test]
fn spawn_writes_the_reserved_id_and_pushes_ok() {
    let (state, path, args) = spawn_package(Ok(7));
    assert_eq!(state_u64(&state, 0), 7, "the id goes through the var slot");
    assert_eq!(state_u64(&state, 8), Status::Ok as u64);
    assert_eq!(path, b"child");
    assert_eq!(args, b"hello");
    assert_eq!(
        state_u64(&state, 16),
        0,
        "success clears the diagnostic's unit_name length"
    );
}

#[test]
fn spawn_failure_pushes_the_status_and_fills_the_diagnostic() {
    let (state, _, _) = spawn_package(Err(Status::Full));
    assert_eq!(state_u64(&state, 0), 0, "the id out-parameter is untouched");
    assert_eq!(state_u64(&state, 8), Status::Full as u64);
    // `Diagnostic.message` is at record offset 40, so state[56..64] is its
    // length word and state[64..] its data.
    let message_len = state_u64(
        &state,
        16 + abi::layout::diagnostic::MESSAGE_OFFSET as usize,
    );
    assert_eq!(message_len, b"bad source".len() as u64);
    assert_eq!(
        &state[16 + abi::layout::diagnostic::MESSAGE_OFFSET as usize + 8..][..b"bad source".len()],
        b"bad source"
    );
    assert_eq!(
        state_u64(&state, 16 + abi::layout::diagnostic::LINE_OFFSET as usize),
        7
    );
}

#[test]
fn wait_blocks_without_advancing_and_retries_the_same_instruction() {
    let mut instructions = Vec::new();
    op_const(&mut instructions, 3);
    op_imm(&mut instructions, opcodes::REF_STATE, 8);
    op_imm(&mut instructions, opcodes::BUILTIN, opcodes::builtin::WAIT);
    op_imm(&mut instructions, opcodes::STORE_STATE, 0);
    op(&mut instructions, opcodes::RET);
    let spec = FnSpec {
        frame_size: 56,
        ..MAIN
    };
    let code = build(&[spec], &instructions, &[], 24, 64, 0);

    let mut host = MockHost {
        wait_results: vec![
            WaitOutcome::Blocked,
            WaitOutcome::Exited(ExitReason::Faulted(FaultCode::Overflow)),
        ],
        ..Default::default()
    };
    let mut state = vec![0u8; 24];
    let mut stack = vec![0u8; 64];

    let (outcome, machine) = run_with_host(&code, &mut state, &mut stack, &mut host, 100);
    assert_eq!(outcome, Outcome::Blocked(WaitKind::ChildExit(3)));
    assert_eq!(machine.pc(), 20, "pc stays on the wait instruction");
    assert_eq!(state, vec![0u8; 24], "nothing is consumed or stored");

    let (outcome, _) = run_with_host(&code, &mut state, &mut stack, &mut host, 100);
    assert_eq!(outcome, Outcome::Exited(ExitReason::Ok));
    assert_eq!(host.wait_calls, 2);
    assert_eq!(state_u64(&state, 0), Status::Ok as u64);
    assert_eq!(
        state_u64(&state, 8),
        abi::layout::exit_tag::FAULTED as u64,
        "the reason's tag is Faulted"
    );
    assert_eq!(
        state_u64(&state, 16),
        FaultCode::Overflow as u64,
        "the reason record's code is the fault code"
    );
}

#[test]
fn wait_maps_exit_reasons_and_status_errors() {
    for (outcome, status, kind, fault_code) in [
        (WaitOutcome::Exited(ExitReason::Ok), 0u64, 0u64, 0u64),
        (WaitOutcome::Exited(ExitReason::Killed), 0, 1, 0),
        (WaitOutcome::Status(Status::NotFound), 3, 0, 0),
    ] {
        let mut instructions = Vec::new();
        op_const(&mut instructions, 9);
        op_imm(&mut instructions, opcodes::REF_STATE, 8);
        op_imm(&mut instructions, opcodes::BUILTIN, opcodes::builtin::WAIT);
        op_imm(&mut instructions, opcodes::STORE_STATE, 0);
        op(&mut instructions, opcodes::RET);
        let spec = FnSpec {
            frame_size: 56,
            ..MAIN
        };
        let code = build(&[spec], &instructions, &[], 24, 64, 0);
        let mut host = MockHost {
            wait_results: vec![outcome],
            ..Default::default()
        };
        let mut state = vec![0u8; 24];
        let mut stack = vec![0u8; 64];
        let (result, _) = run_with_host(&code, &mut state, &mut stack, &mut host, 100);
        assert_eq!(result, Outcome::Exited(ExitReason::Ok));
        assert_eq!(state_u64(&state, 0), status, "{outcome:?}");
        assert_eq!(state_u64(&state, 8), kind, "{outcome:?}");
        assert_eq!(state_u64(&state, 16), fault_code, "{outcome:?}");
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

#[test]
fn enum_cast_keeps_a_valid_ordinal_and_faults_overflow_past_it() {
    // A three-variant enum: ordinals 0, 1 and 2 are valid; 3 and -1 are not.
    for (ordinal, expected) in [
        (0u64, ExitReason::Ok),
        (2, ExitReason::Ok),
        (3, ExitReason::Faulted(FaultCode::Overflow)),
        (u64::MAX, ExitReason::Faulted(FaultCode::Overflow)),
    ] {
        let mut instructions = Vec::new();
        op_const(&mut instructions, ordinal as i64);
        op_imm(&mut instructions, opcodes::ENUM_CAST, 3);
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
        assert_eq!(outcome, Outcome::Exited(expected), "ordinal {ordinal}");
        if expected == ExitReason::Ok {
            assert_eq!(u64::from_le_bytes(state[..8].try_into().unwrap()), ordinal);
        }
    }
}

// ---- buffer indexing, comparison and length -----------------------------
//
// The byte-oriented packages below lay `state` out by hand:
//
//   state[0..8]    a scalar result slot
//   state[8..16]   the buffer's runtime length
//   state[16..24]  eight bytes of data
//
// so a test can preload a length and bytes and read them back afterwards.

const BYTE_FRAME: FnSpec = FnSpec {
    code_off: 0,
    frame_size: 64,
    operand_off: 40,
    param_count: 1,
    has_destination: false,
};

#[test]
fn store_u8_extends_the_length_and_zero_fills_the_gap() {
    let mut instructions = Vec::new();
    op_imm(&mut instructions, opcodes::REF_STATE, 16);
    op_const(&mut instructions, 4);
    op_const(&mut instructions, 0xAB);
    op_imm(&mut instructions, opcodes::STORE_U8, 8);
    op(&mut instructions, opcodes::RET);
    let code = build(&[BYTE_FRAME], &instructions, &[], 24, 64, 0);

    let mut host = MockHost::default();
    let mut state = vec![0u8; 24];
    state[8..16].copy_from_slice(&2u64.to_le_bytes());
    state[16..18].copy_from_slice(b"hi");
    let mut stack = vec![0u8; 64];
    let (outcome, _) = run_with_host(&code, &mut state, &mut stack, &mut host, 100);
    assert_eq!(outcome, Outcome::Exited(ExitReason::Ok));
    assert_eq!(u64::from_le_bytes(state[8..16].try_into().unwrap()), 5);
    assert_eq!(&state[16..21], b"hi\0\0\xab");
}

#[test]
fn store_u8_inside_the_length_keeps_it() {
    let mut instructions = Vec::new();
    op_imm(&mut instructions, opcodes::REF_STATE, 16);
    op_const(&mut instructions, 1);
    op_const(&mut instructions, b'X' as i64);
    op_imm(&mut instructions, opcodes::STORE_U8, 8);
    op(&mut instructions, opcodes::RET);
    let code = build(&[BYTE_FRAME], &instructions, &[], 24, 64, 0);

    let mut host = MockHost::default();
    let mut state = vec![0u8; 24];
    state[8..16].copy_from_slice(&3u64.to_le_bytes());
    state[16..19].copy_from_slice(b"abc");
    let mut stack = vec![0u8; 64];
    let (outcome, _) = run_with_host(&code, &mut state, &mut stack, &mut host, 100);
    assert_eq!(outcome, Outcome::Exited(ExitReason::Ok));
    assert_eq!(u64::from_le_bytes(state[8..16].try_into().unwrap()), 3);
    assert_eq!(&state[16..19], b"aXc");
}

#[test]
fn load_u8_reads_inside_the_length_and_faults_past_it() {
    let mut inside = Vec::new();
    op_imm(&mut inside, opcodes::REF_STATE, 16);
    op_const(&mut inside, 2);
    op(&mut inside, opcodes::LOAD_U8);
    op_imm(&mut inside, opcodes::STORE_STATE, 0);
    op(&mut inside, opcodes::RET);
    let code = build(&[BYTE_FRAME], &inside, &[], 24, 64, 0);
    let mut host = MockHost::default();
    let mut state = vec![0u8; 24];
    state[8..16].copy_from_slice(&3u64.to_le_bytes());
    state[16..19].copy_from_slice(b"abc");
    let mut stack = vec![0u8; 64];
    let (outcome, _) = run_with_host(&code, &mut state, &mut stack, &mut host, 100);
    assert_eq!(outcome, Outcome::Exited(ExitReason::Ok));
    assert_eq!(u64::from_le_bytes(state[0..8].try_into().unwrap()), 99);

    let mut past = Vec::new();
    op_imm(&mut past, opcodes::REF_STATE, 16);
    op_const(&mut past, 3);
    op(&mut past, opcodes::LOAD_U8);
    op(&mut past, opcodes::DROP);
    op(&mut past, opcodes::RET);
    let code = build(&[BYTE_FRAME], &past, &[], 24, 64, 0);
    let mut state = vec![0u8; 24];
    state[8..16].copy_from_slice(&3u64.to_le_bytes());
    let mut stack = vec![0u8; 64];
    let (outcome, _) = run_with_host(&code, &mut state, &mut stack, &mut host, 100);
    assert_eq!(
        outcome,
        Outcome::Exited(ExitReason::Faulted(FaultCode::OutOfBounds))
    );
}

#[test]
fn store_u8_past_the_capacity_faults_capacity_exceeded() {
    let mut instructions = Vec::new();
    op_imm(&mut instructions, opcodes::REF_STATE, 16);
    op_const(&mut instructions, 8);
    op_const(&mut instructions, 1);
    op_imm(&mut instructions, opcodes::STORE_U8, 8);
    op(&mut instructions, opcodes::RET);
    let code = build(&[BYTE_FRAME], &instructions, &[], 24, 64, 0);
    let mut host = MockHost::default();
    let mut state = vec![0u8; 24];
    let mut stack = vec![0u8; 64];
    let (outcome, _) = run_with_host(&code, &mut state, &mut stack, &mut host, 100);
    assert_eq!(
        outcome,
        Outcome::Exited(ExitReason::Faulted(FaultCode::CapacityExceeded))
    );
    assert_eq!(state, vec![0u8; 24], "nothing is written on a failed store");
}

#[test]
fn cmp_buffers_compares_length_then_bytes() {
    for (length, data, expected) in [
        (3u64, &b"abc"[..], 1u64),
        (3, &b"abd"[..], 0),
        (2, &b"ab"[..], 0),
    ] {
        let mut instructions = Vec::new();
        op_imm(&mut instructions, opcodes::REF_STATE, 16);
        op_imm(&mut instructions, opcodes::REF_CONST, 8);
        op(&mut instructions, opcodes::CMP_BUFFERS);
        op_imm(&mut instructions, opcodes::STORE_STATE, 0);
        op(&mut instructions, opcodes::RET);
        let constants = constant(b"abc");
        let code = build(&[BYTE_FRAME], &instructions, &constants, 24, 64, 0);

        let mut host = MockHost::default();
        let mut state = vec![0u8; 24];
        state[8..16].copy_from_slice(&length.to_le_bytes());
        state[16..16 + data.len()].copy_from_slice(data);
        let mut stack = vec![0u8; 64];
        let (outcome, _) = run_with_host(&code, &mut state, &mut stack, &mut host, 100);
        assert_eq!(outcome, Outcome::Exited(ExitReason::Ok));
        assert_eq!(
            u64::from_le_bytes(state[0..8].try_into().unwrap()),
            expected,
            "{length} bytes of {data:?}"
        );
    }
}

#[test]
fn len_builtin_pushes_the_runtime_length() {
    let mut instructions = Vec::new();
    op_imm(&mut instructions, opcodes::REF_STATE, 16);
    op_imm(&mut instructions, opcodes::BUILTIN, opcodes::builtin::LEN);
    op_imm(&mut instructions, opcodes::STORE_STATE, 0);
    op(&mut instructions, opcodes::RET);
    let code = build(&[BYTE_FRAME], &instructions, &[], 24, 64, 0);
    let mut host = MockHost::default();
    let mut state = vec![0u8; 24];
    state[8..16].copy_from_slice(&5u64.to_le_bytes());
    let mut stack = vec![0u8; 64];
    let (outcome, _) = run_with_host(&code, &mut state, &mut stack, &mut host, 100);
    assert_eq!(outcome, Outcome::Exited(ExitReason::Ok));
    assert_eq!(u64::from_le_bytes(state[0..8].try_into().unwrap()), 5);
}

// ---- find and substr ------------------------------------------------------
//
// These packages lay `state` out by hand:
//
//   state[0..8]    a scalar result slot
//   state[8..16]   the destination buffer's runtime length
//   state[16..24]  eight bytes of destination data
//   state[24..32]  the source buffer's runtime length
//   state[32..40]  eight bytes of source data
//
// `substr` copies into state[16..] and `find` scans the source at state[16..].

fn op_word(out: &mut Vec<u8>, opcode: u8, a: u8, b: u8, c: u8) {
    out.extend_from_slice(&[opcode, a, b, c]);
}

fn substr_package(kind: u8, capacity: u32, start: i64, length: i64) -> Vec<u8> {
    let mut instructions = Vec::new();
    op_imm(&mut instructions, opcodes::REF_STATE, 16);
    op_imm(&mut instructions, opcodes::REF_STATE, 32);
    op_const(&mut instructions, start);
    op_const(&mut instructions, length);
    op_word(&mut instructions, opcodes::SUBSTR, kind, 0, 0);
    instructions.extend_from_slice(&capacity.to_le_bytes());
    op_imm(&mut instructions, opcodes::STORE_STATE, 0);
    op(&mut instructions, opcodes::RET);
    let frame = FnSpec {
        frame_size: 96,
        ..BYTE_FRAME
    };
    build(&[frame], &instructions, &[], 40, 128, 0)
}

fn run_substr(
    kind: u8,
    capacity: u32,
    start: i64,
    length: i64,
    destination: &[u8],
    source: &[u8],
) -> (i64, Vec<u8>) {
    let code = substr_package(kind, capacity, start, length);
    let mut host = MockHost::default();
    let mut state = vec![0u8; 40];
    state[8..16].copy_from_slice(&(destination.len() as u64).to_le_bytes());
    state[16..16 + destination.len()].copy_from_slice(destination);
    state[24..32].copy_from_slice(&(source.len() as u64).to_le_bytes());
    state[32..32 + source.len()].copy_from_slice(source);
    let mut stack = vec![0u8; 128];
    let (outcome, _) = run_with_host(&code, &mut state, &mut stack, &mut host, 100);
    assert_eq!(outcome, Outcome::Exited(ExitReason::Ok));
    let result = i64::from_le_bytes(state[0..8].try_into().unwrap());
    let length = u64::from_le_bytes(state[8..16].try_into().unwrap()) as usize;
    (result, state[16..16 + length].to_vec())
}

#[test]
fn substr_copies_and_clamps_to_the_source_tail() {
    let (result, copied) = run_substr(opcodes::buffer_kind::BYTES, 8, 2, 99, b"XYZ", b"abcdef");
    assert_eq!(result, 4);
    assert_eq!(copied, b"cdef");
}

#[test]
fn substr_clamps_to_the_destination_capacity() {
    let (result, copied) = run_substr(opcodes::buffer_kind::BYTES, 3, 0, 99, b"XYZ", b"abcdef");
    assert_eq!(result, 3);
    assert_eq!(copied, b"abc");
}

#[test]
fn substr_past_the_end_copies_nothing() {
    let (result, copied) = run_substr(opcodes::buffer_kind::BYTES, 8, 9, 2, b"XYZ", b"abc");
    assert_eq!(result, 0);
    assert_eq!(copied, b"");
}

#[test]
fn substr_zero_length_clears_the_destination() {
    let (result, copied) = run_substr(opcodes::buffer_kind::BYTES, 8, 1, 0, b"XYZ", b"abc");
    assert_eq!(result, 0);
    assert_eq!(copied, b"");
}

#[test]
fn substr_negative_arguments_are_invalid_and_write_nothing() {
    for (start, length) in [(-1i64, 2i64), (0, -1)] {
        let (result, copied) = run_substr(
            opcodes::buffer_kind::BYTES,
            8,
            start,
            length,
            b"XYZ",
            b"abc",
        );
        assert_eq!(result, -4, "start={start} length={length}");
        assert_eq!(copied, b"XYZ", "the destination is untouched");
    }
}

#[test]
fn substr_string_rejects_a_split_code_point() {
    // "é" is two bytes (0xC3 0xA9); starting at 1 splits it.
    let (result, copied) = run_substr(opcodes::buffer_kind::STRING, 8, 1, 1, b"XYZ", b"\xC3\xA9");
    assert_eq!(result, -4);
    assert_eq!(copied, b"XYZ", "the destination is untouched");
}

#[test]
fn substr_string_accepts_code_point_boundaries() {
    let (result, copied) = run_substr(opcodes::buffer_kind::STRING, 8, 1, 2, b"", b"a\xC3\xA9");
    assert_eq!(result, 2);
    assert_eq!(copied, b"\xC3\xA9");
}

fn find_package(needle: u8) -> Vec<u8> {
    let mut instructions = Vec::new();
    op_imm(&mut instructions, opcodes::REF_STATE, 16);
    op_const(&mut instructions, i64::from(needle));
    op(&mut instructions, opcodes::FIND);
    op_imm(&mut instructions, opcodes::STORE_STATE, 0);
    op(&mut instructions, opcodes::RET);
    build(&[BYTE_FRAME], &instructions, &[], 24, 64, 0)
}

fn run_find(needle: u8, length: u64, data: &[u8]) -> i64 {
    let code = find_package(needle);
    let mut host = MockHost::default();
    let mut state = vec![0u8; 24];
    state[8..16].copy_from_slice(&length.to_le_bytes());
    state[16..16 + data.len()].copy_from_slice(data);
    let mut stack = vec![0u8; 64];
    let (outcome, _) = run_with_host(&code, &mut state, &mut stack, &mut host, 100);
    assert_eq!(outcome, Outcome::Exited(ExitReason::Ok));
    i64::from_le_bytes(state[0..8].try_into().unwrap())
}

#[test]
fn find_returns_the_first_byte_index_or_minus_one() {
    assert_eq!(run_find(b'l', 5, b"hello"), 2);
    assert_eq!(run_find(b'z', 5, b"hello"), -1);
    // Only `0..length` is scanned; the rest of the region is invisible.
    assert_eq!(run_find(b'l', 2, b"hello"), -1);
    // `string` is scanned byte-wise, not by code point.
    assert_eq!(run_find(0xA9, 2, b"\xC3\xA9"), 1);
}

// ---- integer formatting ---------------------------------------------------
//
// `state[0..8]` is the scalar result, `state[8..16]` the destination length and
// `state[16..24]` eight bytes of destination data, as in the byte tests above.

fn format_package(kind: u8, capacity: u32, value: i64, base: u64, width: u64) -> Vec<u8> {
    let mut instructions = Vec::new();
    op_imm(&mut instructions, opcodes::REF_STATE, 16);
    op_const(&mut instructions, value);
    op_const(&mut instructions, base as i64);
    op_const(&mut instructions, width as i64);
    op_word(&mut instructions, opcodes::FORMAT_INT, kind, 0, 0);
    instructions.extend_from_slice(&capacity.to_le_bytes());
    op_imm(&mut instructions, opcodes::STORE_STATE, 0);
    op(&mut instructions, opcodes::RET);
    let frame = FnSpec {
        frame_size: 80,
        ..BYTE_FRAME
    };
    build(&[frame], &instructions, &[], 24, 96, 0)
}

/// Runs one `format_int` and returns `(result, bytes, length)`. The destination
/// starts with `initial`, so an untouched buffer is observable.
fn run_format(
    kind: u8,
    capacity: u32,
    value: i64,
    base: u64,
    width: u64,
    initial: &[u8],
) -> (i64, Vec<u8>, u64) {
    let code = format_package(kind, capacity, value, base, width);
    let mut host = MockHost::default();
    let mut state = vec![0u8; 24];
    state[8..16].copy_from_slice(&(initial.len() as u64).to_le_bytes());
    state[16..16 + initial.len()].copy_from_slice(initial);
    let mut stack = vec![0u8; 96];
    let (outcome, _) = run_with_host(&code, &mut state, &mut stack, &mut host, 100);
    assert_eq!(outcome, Outcome::Exited(ExitReason::Ok));
    let result = i64::from_le_bytes(state[0..8].try_into().unwrap());
    let length = u64::from_le_bytes(state[8..16].try_into().unwrap());
    let bytes = state[16..16 + length as usize].to_vec();
    (result, bytes, length)
}

#[test]
fn format_int_is_decimal_or_lowercase_hex() {
    assert_eq!(
        run_format(opcodes::buffer_kind::STRING, 8, 42, 10, 0, b""),
        (2, b"42".to_vec(), 2)
    );
    assert_eq!(
        run_format(opcodes::buffer_kind::STRING, 8, 0xBEEF, 16, 0, b""),
        (4, b"beef".to_vec(), 4)
    );
    assert_eq!(
        run_format(opcodes::buffer_kind::BYTES, 8, 0xDEAD, 16, 0, b""),
        (4, b"dead".to_vec(), 4)
    );
}

#[test]
fn format_int_handles_negatives_and_zero_padding() {
    assert_eq!(
        run_format(opcodes::buffer_kind::STRING, 8, -42, 10, 0, b""),
        (3, b"-42".to_vec(), 3)
    );
    assert_eq!(
        run_format(opcodes::buffer_kind::STRING, 8, 7, 10, 4, b""),
        (4, b"0007".to_vec(), 4)
    );
    assert_eq!(
        run_format(opcodes::buffer_kind::STRING, 8, 0, 10, 2, b""),
        (2, b"00".to_vec(), 2)
    );
    // The sign is additional: width 4 pads the digits, not the `-`.
    assert_eq!(
        run_format(opcodes::buffer_kind::STRING, 8, -5, 10, 4, b""),
        (5, b"-0005".to_vec(), 5)
    );
    // A value that needs more digits than the width keeps all of them.
    assert_eq!(
        run_format(opcodes::buffer_kind::STRING, 8, 12345, 10, 2, b""),
        (5, b"12345".to_vec(), 5)
    );
}

#[test]
fn format_int_fits_exactly_or_reports_capacity() {
    assert_eq!(
        run_format(opcodes::buffer_kind::STRING, 2, 42, 10, 0, b"XYZ"),
        (2, b"42".to_vec(), 2)
    );
    assert_eq!(
        run_format(opcodes::buffer_kind::STRING, 3, -42, 10, 0, b"XYZ"),
        (3, b"-42".to_vec(), 3)
    );
    // One byte short: `-6`, nothing written, length untouched.
    assert_eq!(
        run_format(opcodes::buffer_kind::STRING, 1, 42, 10, 0, b"XYZ"),
        (-6, b"XYZ".to_vec(), 3)
    );
    assert_eq!(
        run_format(opcodes::buffer_kind::STRING, 2, -42, 10, 0, b"XYZ"),
        (-6, b"XYZ".to_vec(), 3)
    );
    // Padding counts against the capacity too.
    assert_eq!(
        run_format(opcodes::buffer_kind::STRING, 3, 7, 10, 4, b"XYZ"),
        (-6, b"XYZ".to_vec(), 3)
    );
    // A width so large that the total overflows `u64` is also a capacity error.
    assert_eq!(
        run_format(opcodes::buffer_kind::STRING, 8, 7, 10, u64::MAX, b"XYZ"),
        (-6, b"XYZ".to_vec(), 3)
    );
}

#[test]
fn format_int_rejects_unsupported_bases() {
    for base in [0u64, 2, 8, 36] {
        assert_eq!(
            run_format(opcodes::buffer_kind::STRING, 8, 42, base, 0, b"XYZ"),
            (-4, b"XYZ".to_vec(), 3),
            "base {base}"
        );
    }
}

// ---- syslib port builtins -------------------------------------------------

#[test]
fn port_in8_reads_through_the_host_and_uses_the_low_16_bits() {
    let mut instructions = Vec::new();
    op_const(&mut instructions, 0x1_0071);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::PORT_IN8,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 0);
    op(&mut instructions, opcodes::RET);
    let spec = FnSpec {
        frame_size: 48,
        ..MAIN
    };
    let code = build(&[spec], &instructions, &[], 8, 64, 0);
    let mut host = MockHost {
        port_value: 0x42,
        ..Default::default()
    };
    let mut state = vec![0u8; 8];
    let mut stack = vec![0u8; 64];
    let (outcome, _) = run_with_host(&code, &mut state, &mut stack, &mut host, 100);
    assert_eq!(outcome, Outcome::Exited(ExitReason::Ok));
    assert_eq!(u64::from_le_bytes(state[..8].try_into().unwrap()), 0x42);
    assert_eq!(host.port_reads, vec![0x0071]);
}

#[test]
fn port_out8_writes_and_rejects_values_over_255() {
    let mut valid = Vec::new();
    op_const(&mut valid, 0x70);
    op_const(&mut valid, 0xAB);
    op_imm(&mut valid, opcodes::BUILTIN, opcodes::builtin::PORT_OUT8);
    op_imm(&mut valid, opcodes::STORE_STATE, 0);
    op(&mut valid, opcodes::RET);
    let spec = FnSpec {
        frame_size: 64,
        operand_off: 48,
        ..MAIN
    };
    let code = build(&[spec], &valid, &[], 8, 64, 0);
    let mut host = MockHost::default();
    let mut state = vec![0u8; 8];
    let mut stack = vec![0u8; 64];
    let (outcome, _) = run_with_host(&code, &mut state, &mut stack, &mut host, 100);
    assert_eq!(outcome, Outcome::Exited(ExitReason::Ok));
    assert_eq!(u64::from_le_bytes(state[..8].try_into().unwrap()), 0);
    assert_eq!(host.port_writes, vec![(0x70, 0xAB)]);

    let mut invalid = Vec::new();
    op_const(&mut invalid, 0x70);
    op_const(&mut invalid, 300);
    op_imm(&mut invalid, opcodes::BUILTIN, opcodes::builtin::PORT_OUT8);
    op_imm(&mut invalid, opcodes::STORE_STATE, 0);
    op(&mut invalid, opcodes::RET);
    let code = build(&[spec], &invalid, &[], 8, 64, 0);
    let mut host = MockHost::default();
    let mut state = vec![0u8; 8];
    let mut stack = vec![0u8; 64];
    let (outcome, _) = run_with_host(&code, &mut state, &mut stack, &mut host, 100);
    assert_eq!(outcome, Outcome::Exited(ExitReason::Ok));
    assert_eq!(
        i64::from_le_bytes(state[..8].try_into().unwrap()),
        -4,
        "a value over 255 is Status::Invalid"
    );
    assert!(host.port_writes.is_empty(), "the port is not touched");
}

#[test]
fn ref_offset_forms_a_field_reference() {
    let mut instructions = Vec::new();
    op_imm(&mut instructions, opcodes::REF_STATE, 0);
    op_imm(&mut instructions, opcodes::REF_OFFSET, 8);
    op(&mut instructions, opcodes::LOAD_REF);
    op_imm(&mut instructions, opcodes::STORE_STATE, 24);
    op(&mut instructions, opcodes::RET);
    let spec = FnSpec {
        frame_size: 48,
        ..MAIN
    };
    let code = build(&[spec], &instructions, &[], 32, 64, 0);
    let mut scratch = scratch_for(&code);
    let program = Program::parse(&code, &mut scratch).expect("verifies");
    let mut state = vec![0u8; 32];
    state[8..16].copy_from_slice(&0x42u64.to_le_bytes());
    let mut stack = vec![0u8; 64];
    let args = vec![0u8; 8 + 1024];
    let mut machine = Machine::new();
    let mut host = Recorder { output: Vec::new() };
    assert_eq!(
        machine.run(&program, &mut host, &mut state, &mut stack, &args, 100),
        Outcome::Exited(ExitReason::Ok)
    );
    assert_eq!(state_u64(&state, 24), 0x42, "the field at +8 was loaded");
}

#[test]
fn ref_offset_past_the_region_faults_out_of_bounds() {
    let mut instructions = Vec::new();
    op_imm(&mut instructions, opcodes::REF_STATE, 0);
    op_imm(&mut instructions, opcodes::REF_OFFSET, 32);
    op(&mut instructions, opcodes::LOAD_REF);
    op(&mut instructions, opcodes::DROP);
    op(&mut instructions, opcodes::RET);
    let spec = FnSpec {
        frame_size: 48,
        ..MAIN
    };
    let code = build(&[spec], &instructions, &[], 32, 64, 0);
    let (reason, _) = execute(&code, 32, 64);
    assert_eq!(reason, ExitReason::Faulted(FaultCode::OutOfBounds));
}

#[test]
fn copy_refs_moves_a_record_and_handles_overlap() {
    let mut instructions = Vec::new();
    op_imm(&mut instructions, opcodes::REF_STATE, 8);
    op_imm(&mut instructions, opcodes::REF_STATE, 0);
    op_const(&mut instructions, 16);
    op(&mut instructions, opcodes::COPY_REFS);
    op(&mut instructions, opcodes::RET);
    let spec = FnSpec {
        frame_size: 64,
        ..MAIN
    };
    let code = build(&[spec], &instructions, &[], 32, 64, 0);
    let mut scratch = scratch_for(&code);
    let program = Program::parse(&code, &mut scratch).expect("verifies");
    let mut state = vec![0u8; 32];
    for (index, byte) in state[..16].iter_mut().enumerate() {
        *byte = index as u8 + 1;
    }
    let original = state[..16].to_vec();
    let mut stack = vec![0u8; 64];
    let args = vec![0u8; 8 + 1024];
    let mut machine = Machine::new();
    let mut host = Recorder { output: Vec::new() };
    assert_eq!(
        machine.run(&program, &mut host, &mut state, &mut stack, &args, 100),
        Outcome::Exited(ExitReason::Ok)
    );
    assert_eq!(
        &state[8..24],
        &original[..16],
        "the overlapping move copied the original bytes"
    );
    assert_eq!(&state[24..], &[0u8; 8], "nothing past the copy changed");
}

#[test]
fn copy_refs_past_the_region_faults_out_of_bounds() {
    let mut instructions = Vec::new();
    op_imm(&mut instructions, opcodes::REF_STATE, 16);
    op_imm(&mut instructions, opcodes::REF_STATE, 0);
    op_const(&mut instructions, 32);
    op(&mut instructions, opcodes::COPY_REFS);
    op(&mut instructions, opcodes::RET);
    let spec = FnSpec {
        frame_size: 64,
        ..MAIN
    };
    let code = build(&[spec], &instructions, &[], 32, 64, 0);
    let (reason, _) = execute(&code, 32, 64);
    assert_eq!(reason, ExitReason::Faulted(FaultCode::OutOfBounds));
}

#[test]
fn the_verifier_rejects_a_misaligned_field_offset() {
    let mut instructions = Vec::new();
    op_imm(&mut instructions, opcodes::REF_STATE, 0);
    op_imm(&mut instructions, opcodes::REF_OFFSET, 4);
    op(&mut instructions, opcodes::RET);
    let spec = FnSpec {
        frame_size: 48,
        ..MAIN
    };
    let code = build(&[spec], &instructions, &[], 32, 64, 0);
    let mut scratch = scratch_for(&code);
    let error = Program::parse(&code, &mut scratch).expect_err("must be rejected");
    assert!(error.message.contains("field offset"), "{}", error.message);
}
