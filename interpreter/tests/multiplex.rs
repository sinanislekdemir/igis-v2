//! `core.poll_any`/`core.next_any` builtin tests (spec 0003, spec 0007; Q39).
//!
//! These tests pin the interpreter contract only: the operand order, the
//! per-source readiness probes in caller order, the `ready` index written only
//! on `Ok`, the `WouldBlock`/`Invalid` cases, and the blocked `WaitKind::Any`
//! retry. The kernel's wake-up and readiness rules are proved by
//! `samples/apps/multiplex`; packages are hand-assembled like `f64parse.rs`.

use abi::opcodes;
use abi::wait::{WaitSet, MAX_SOURCES, SOURCE_CONSOLE, SOURCE_INPUT};
use abi::{
    CodeHeader, Function, Status, ABI_VERSION, CODE_MAGIC, NO_DIAGNOSTIC, NO_NAME,
    PROFILE_X86_64_LE,
};
use interpreter::{ExitReason, Host, Machine, Outcome, WaitKind};

const SOURCES_OFF: u32 = 0;
const READY_OFF: u32 = 72;
const STATUS_OFF: u32 = 80;
const STATE_BYTES: usize = 96;
const READY_SENTINEL: u64 = 0xDEAD_BEEF_0000_0001;

fn build(instructions: &[u8]) -> Vec<u8> {
    let header = CodeHeader {
        magic: CODE_MAGIC,
        abi_version: ABI_VERSION,
        profile: PROFILE_X86_64_LE,
        function_count: 1,
        entry: 0,
        code_len: instructions.len() as u32,
        const_len: 0,
        line_len: 0,
        state_size: STATE_BYTES as u32,
        stack_size: 128,
        diag_offset: NO_DIAGNOSTIC,
    };
    let mut code = Vec::new();
    code.extend_from_slice(&header.to_bytes());
    code.extend_from_slice(
        &Function {
            code_off: 0,
            frame_size: 128,
            operand_off: 40,
            line_base: 0,
            name_off: NO_NAME,
            param_count: 1,
            has_destination: false,
            _reserved: [0, 0],
        }
        .to_bytes(),
    );
    while code.len() % 8 != 0 {
        code.push(0);
    }
    code.extend_from_slice(instructions);
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

fn const_u64(out: &mut Vec<u8>, value: u64) {
    op(out, opcodes::CONST);
    out.extend_from_slice(&(value as u32).to_le_bytes());
    out.extend_from_slice(&((value >> 32) as u32).to_le_bytes());
}

/// Builds `status := poll_any(sources, count, ready)` (or `next_any`).
fn package(id: u32, count: u64) -> Vec<u8> {
    let mut instructions = Vec::new();
    op_imm(&mut instructions, opcodes::REF_STATE, SOURCES_OFF);
    const_u64(&mut instructions, count);
    op_imm(&mut instructions, opcodes::REF_STATE, READY_OFF);
    op_imm(&mut instructions, opcodes::BUILTIN, id);
    op_imm(&mut instructions, opcodes::STORE_STATE, STATUS_OFF);
    op(&mut instructions, opcodes::RET);
    build(&instructions)
}

#[derive(Debug)]
struct ProbeHost {
    console: bool,
    input: bool,
    stream_ready: bool,
    stream_probes: Vec<u64>,
}

impl ProbeHost {
    fn new() -> Self {
        ProbeHost {
            console: false,
            input: false,
            stream_ready: false,
            stream_probes: Vec::new(),
        }
    }
}

impl Host for ProbeHost {
    fn console_write(&mut self, _bytes: &[u8]) {}
    fn clock(&mut self) -> u64 {
        0
    }
    fn console_line_ready(&mut self) -> bool {
        self.console
    }
    fn input_ready(&mut self) -> bool {
        self.input
    }
    fn stream_readable(&mut self, handle: u64) -> bool {
        self.stream_probes.push(handle);
        self.stream_ready
    }
}

fn run(code: &[u8], sources: &[u64], host: &mut ProbeHost) -> (Outcome, u64, u64, Vec<u8>) {
    let mut scratch = vec![0u8; code.len().div_ceil(32)];
    let program = interpreter::Program::parse(code, &mut scratch).expect("package verifies");
    let mut state = vec![0u8; STATE_BYTES];
    for (index, source) in sources.iter().enumerate() {
        let at = SOURCES_OFF as usize + index * 8;
        state[at..at + 8].copy_from_slice(&source.to_le_bytes());
    }
    state[READY_OFF as usize..READY_OFF as usize + 8]
        .copy_from_slice(&READY_SENTINEL.to_le_bytes());
    let mut stack = vec![0u8; 128];
    let args = vec![0u8; 8 + 1024];
    let mut machine = Machine::new();
    let outcome = machine.run(&program, host, &mut state, &mut stack, &args, 10_000);
    let ready = read_word(&state, READY_OFF);
    let status = read_word(&state, STATUS_OFF);
    (outcome, ready, status, state)
}

fn read_word(state: &[u8], offset: u32) -> u64 {
    u64::from_le_bytes(
        state[offset as usize..offset as usize + 8]
            .try_into()
            .unwrap(),
    )
}

fn assert_exited(outcome: &Outcome) {
    assert!(
        matches!(outcome, Outcome::Exited(ExitReason::Ok)),
        "unexpected outcome {outcome:?}"
    );
}

#[test]
fn poll_any_reports_would_block_and_leaves_ready_untouched() {
    let code = package(opcodes::builtin::POLL_ANY, 1);
    let mut host = ProbeHost::new();
    let (outcome, ready, status, state) = run(&code, &[300], &mut host);
    assert_exited(&outcome);
    assert_eq!(status, Status::WouldBlock as u64);
    assert_eq!(ready, READY_SENTINEL);
    assert_eq!(state.len(), STATE_BYTES);
    assert_eq!(host.stream_probes, vec![300]);
}

#[test]
fn poll_any_reports_the_first_ready_index_in_caller_order() {
    let code = package(opcodes::builtin::POLL_ANY, 3);
    let mut host = ProbeHost::new();
    host.console = true;
    host.input = true;
    host.stream_ready = true;
    let (outcome, ready, status, _) = run(&code, &[SOURCE_CONSOLE, SOURCE_INPUT, 300], &mut host);
    assert_exited(&outcome);
    assert_eq!(status, Status::Ok as u64);
    assert_eq!(ready, 0, "the console source comes first");
    assert!(
        host.stream_probes.is_empty(),
        "the scan stops at the first ready source"
    );

    let mut host = ProbeHost::new();
    host.input = true;
    host.stream_ready = true;
    let (outcome, ready, status, _) = run(&code, &[SOURCE_CONSOLE, SOURCE_INPUT, 300], &mut host);
    assert_exited(&outcome);
    assert_eq!(status, Status::Ok as u64);
    assert_eq!(ready, 1);
    assert!(host.stream_probes.is_empty());

    let mut host = ProbeHost::new();
    host.stream_ready = true;
    let (outcome, ready, status, _) = run(&code, &[SOURCE_CONSOLE, SOURCE_INPUT, 300], &mut host);
    assert_exited(&outcome);
    assert_eq!(status, Status::Ok as u64);
    assert_eq!(ready, 2);
    assert_eq!(host.stream_probes, vec![300]);
}

#[test]
fn poll_any_reads_every_element_of_a_full_set() {
    let code = package(opcodes::builtin::POLL_ANY, MAX_SOURCES as u64);
    let mut host = ProbeHost::new();
    host.input = true;
    let mut sources = [300; MAX_SOURCES];
    sources[MAX_SOURCES - 1] = SOURCE_INPUT;
    let (outcome, ready, status, _) = run(&code, &sources, &mut host);
    assert_exited(&outcome);
    assert_eq!(status, Status::Ok as u64);
    assert_eq!(ready, (MAX_SOURCES - 1) as u64);
}

#[test]
fn a_count_outside_one_to_eight_is_invalid_and_probes_nothing() {
    for count in [0, 9] {
        let code = package(opcodes::builtin::POLL_ANY, count);
        let mut host = ProbeHost::new();
        host.console = true;
        host.input = true;
        host.stream_ready = true;
        let (outcome, ready, status, _) = run(&code, &[SOURCE_CONSOLE], &mut host);
        assert_exited(&outcome);
        assert_eq!(status, Status::Invalid as u64, "count {count}");
        assert_eq!(ready, READY_SENTINEL);
        assert!(host.stream_probes.is_empty());
    }
}

#[test]
fn next_any_blocks_on_a_wait_set_and_retries_after_a_wake() {
    let code = package(opcodes::builtin::NEXT_ANY, 2);
    let mut host = ProbeHost::new();
    let sources = [SOURCE_INPUT, 300];

    let mut scratch = vec![0u8; code.len().div_ceil(32)];
    let program = interpreter::Program::parse(&code, &mut scratch).expect("package verifies");
    let mut state = vec![0u8; STATE_BYTES];
    for (index, source) in sources.iter().enumerate() {
        let at = SOURCES_OFF as usize + index * 8;
        state[at..at + 8].copy_from_slice(&source.to_le_bytes());
    }
    state[READY_OFF as usize..READY_OFF as usize + 8]
        .copy_from_slice(&READY_SENTINEL.to_le_bytes());
    let mut stack = vec![0u8; 128];
    let args = vec![0u8; 8 + 1024];
    let mut machine = Machine::new();

    let first = machine.run(&program, &mut host, &mut state, &mut stack, &args, 10_000);
    let expected = WaitSet::from_slice(&sources).expect("two sources");
    assert_eq!(first, Outcome::Blocked(WaitKind::Any(expected)));
    assert_eq!(read_word(&state, READY_OFF), READY_SENTINEL);
    // Nothing was written to the destination and the operand stack is intact:
    // a second call resumes the same instruction.
    let second = machine.run(&program, &mut host, &mut state, &mut stack, &args, 10_000);
    assert_eq!(second, Outcome::Blocked(WaitKind::Any(expected)));

    host.stream_ready = true;
    let third = machine.run(&program, &mut host, &mut state, &mut stack, &args, 10_000);
    assert_exited(&third);
    assert_eq!(read_word(&state, STATUS_OFF), Status::Ok as u64);
    assert_eq!(read_word(&state, READY_OFF), 1);
    assert_eq!(host.stream_probes, vec![300, 300, 300]);
}

#[test]
fn next_any_returns_a_ready_source_without_blocking() {
    let code = package(opcodes::builtin::NEXT_ANY, 1);
    let mut host = ProbeHost::new();
    host.console = true;
    let (outcome, ready, status, _) = run(&code, &[SOURCE_CONSOLE], &mut host);
    assert_exited(&outcome);
    assert_eq!(status, Status::Ok as u64);
    assert_eq!(ready, 0);
}

#[test]
fn the_new_builtins_keep_their_declared_stack_effects() {
    assert_eq!(
        opcodes::builtin::effect(opcodes::builtin::POLL_ANY),
        Some((3, 1))
    );
    assert_eq!(
        opcodes::builtin::effect(opcodes::builtin::NEXT_ANY),
        Some((3, 1))
    );
}
