//! Structural-verification and execution tests.
//!
//! Packages are hand-assembled here on purpose: the interpreter must be
//! testable without the compiler, because the two crates never depend on each
//! other (spec 0008, B12).

use abi::opcodes;
use abi::refs;
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
    spawn_calls: usize,
    wait_results: Vec<WaitOutcome>,
    wait_calls: usize,
    diag_for: Option<(u64, Vec<u8>)>,
    input: Vec<u8>,
    read_calls: usize,
    killed: Vec<u64>,
    port_value: u8,
    port_reads: Vec<u16>,
    port_writes: Vec<(u16, u8)>,
    realtime_status: u64,
    realtime_calls: usize,
    heartbeat_calls: usize,
    screen_width_value: u64,
    screen_height_value: u64,
    gfx_ok: bool,
    rects: Vec<(i64, i64, i64, i64, u32)>,
    dithers: Vec<(i64, i64, i64, i64, u32)>,
    outlines: Vec<(i64, i64, i64, i64, u32)>,
    texts: Vec<(i64, i64, Vec<u8>, u32)>,
    prop_widths: Vec<Vec<u8>>,
    prop_texts: Vec<(i64, i64, Vec<u8>, u32)>,
    prop_width_value: u64,
    prop_height_value: u64,
    pixels: Vec<(i64, i64)>,
    pixel_value: u32,
    presents: Vec<(i64, i64, i64, i64)>,
    clears: Vec<u32>,
    pointer_x_value: i64,
    pointer_y_value: i64,
    pointer_buttons_value: u64,
    pointer_source_value: u64,
    injections: Vec<(i64, i64, u32)>,
    events: std::collections::VecDeque<Vec<u8>>,
    event_injections: Vec<Vec<u8>>,
    event_drops_value: u64,
}

impl Host for MockHost {
    fn console_write(&mut self, bytes: &[u8]) {
        self.output.extend_from_slice(bytes);
    }

    fn read_input(&mut self, dst: &mut [u8]) -> usize {
        self.read_calls += 1;
        let copied = self.input.len().min(dst.len());
        dst[..copied].copy_from_slice(&self.input[..copied]);
        self.input.drain(..copied);
        copied
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
        self.spawn_calls += 1;
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

    fn diagnostic_for(&mut self, id: u64, diag: &mut [u8]) -> bool {
        match &self.diag_for {
            Some((want, bytes)) if *want == id => {
                let copied = diag.len().min(bytes.len());
                diag[..copied].copy_from_slice(&bytes[..copied]);
                true
            }
            _ => false,
        }
    }

    fn kill(&mut self, id: u64) -> Status {
        self.killed.push(id);
        Status::Ok
    }

    fn realtime_on(&mut self) -> u64 {
        self.realtime_calls += 1;
        self.realtime_status
    }

    fn heartbeat(&mut self) {
        self.heartbeat_calls += 1;
    }

    // `syslib` graphics (spec 0009): record the calls and return the scripted
    // status. `MockHost::default()` leaves the screen 0x0 and every call
    // invalid, which is the same shape as the trait defaults.

    fn screen_width(&mut self) -> u64 {
        self.screen_width_value
    }

    fn screen_height(&mut self) -> u64 {
        self.screen_height_value
    }

    fn fill_rect(&mut self, x: i64, y: i64, w: i64, h: i64, color: u32) -> Status {
        self.rects.push((x, y, w, h, color));
        if self.gfx_ok {
            Status::Ok
        } else {
            Status::Invalid
        }
    }

    fn fill_dither(&mut self, x: i64, y: i64, w: i64, h: i64, color: u32) -> Status {
        self.dithers.push((x, y, w, h, color));
        if self.gfx_ok {
            Status::Ok
        } else {
            Status::Invalid
        }
    }

    fn draw_rect(&mut self, x: i64, y: i64, w: i64, h: i64, color: u32) -> Status {
        self.outlines.push((x, y, w, h, color));
        if self.gfx_ok {
            Status::Ok
        } else {
            Status::Invalid
        }
    }

    fn draw_text(&mut self, x: i64, y: i64, text: &[u8], color: u32) -> Status {
        self.texts.push((x, y, text.to_vec(), color));
        if self.gfx_ok {
            Status::Ok
        } else {
            Status::Invalid
        }
    }

    fn text_width_prop(&mut self, text: &[u8]) -> u64 {
        self.prop_widths.push(text.to_vec());
        self.prop_width_value
    }

    fn text_height_prop(&mut self) -> u64 {
        self.prop_height_value
    }

    fn draw_text_prop(&mut self, x: i64, y: i64, text: &[u8], color: u32) -> Status {
        self.prop_texts.push((x, y, text.to_vec(), color));
        if self.gfx_ok {
            Status::Ok
        } else {
            Status::Invalid
        }
    }

    fn get_pixel(&mut self, x: i64, y: i64) -> u32 {
        self.pixels.push((x, y));
        self.pixel_value
    }

    fn present(&mut self, x: i64, y: i64, w: i64, h: i64) -> Status {
        self.presents.push((x, y, w, h));
        if self.gfx_ok {
            Status::Ok
        } else {
            Status::Invalid
        }
    }

    fn clear(&mut self, color: u32) -> Status {
        self.clears.push(color);
        if self.gfx_ok {
            Status::Ok
        } else {
            Status::Invalid
        }
    }

    // Input (spec 0010): scripted pointer values, recorded injections. A host
    // with `gfx_ok` false models graphics disabled, where injection is refused.

    fn pointer_x(&mut self) -> i64 {
        self.pointer_x_value
    }

    fn pointer_y(&mut self) -> i64 {
        self.pointer_y_value
    }

    fn pointer_buttons(&mut self) -> u64 {
        self.pointer_buttons_value
    }

    fn inject_pointer(&mut self, dx: i64, dy: i64, buttons: u32) -> Status {
        self.injections.push((dx, dy, buttons));
        if self.gfx_ok {
            Status::Ok
        } else {
            Status::Invalid
        }
    }

    fn pointer_source(&mut self) -> u64 {
        self.pointer_source_value
    }

    // Event queue (spec 0010, I19): a scriptable FIFO. `inject_event` also
    // records the image so a test can assert what crossed the host boundary.
    fn poll_event(&mut self, event: &mut [u8]) -> Status {
        match self.events.pop_front() {
            Some(image) => {
                let copied = image.len().min(event.len());
                event[..copied].copy_from_slice(&image[..copied]);
                Status::Ok
            }
            None => Status::WouldBlock,
        }
    }

    fn inject_event(&mut self, event: &[u8]) -> Status {
        self.event_injections.push(event.to_vec());
        self.events.push_back(event.to_vec());
        Status::Ok
    }

    fn event_drops(&mut self) -> u64 {
        self.event_drops_value
    }

    fn event_flush(&mut self) -> u64 {
        let flushed = self.events.len() as u64;
        self.events.clear();
        flushed
    }
}

fn run_with_host(
    code: &[u8],
    state: &mut [u8],
    stack: &mut [u8],
    host: &mut MockHost,
    budget: u64,
) -> (Outcome, Machine) {
    let mut machine = Machine::new();
    let outcome = resume_with_host(code, state, stack, host, budget, &mut machine);
    (outcome, machine)
}

/// Resumes an existing machine, as the scheduler does across quanta. A machine
/// carries the pending `core.run` id, so this is what proves a blocked run does
/// not spawn twice.
fn resume_with_host(
    code: &[u8],
    state: &mut [u8],
    stack: &mut [u8],
    host: &mut MockHost,
    budget: u64,
    machine: &mut Machine,
) -> Outcome {
    let mut scratch = scratch_for(code);
    let program = Program::parse(code, &mut scratch).expect("verifies");
    let args = vec![0u8; 8 + 1024];
    machine.run(&program, host, state, stack, &args, budget)
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

fn run_package() -> Vec<u8> {
    let mut instructions = Vec::new();
    op_imm(&mut instructions, opcodes::REF_CONST, 8);
    op_imm(&mut instructions, opcodes::REF_CONST, 24);
    // `Diagnostic` occupies state[16..208]; the `ExitReason` is state[208..224]
    // and the returned `Status` slot is state[0..8].
    op_imm(&mut instructions, opcodes::REF_STATE, 16);
    op_imm(&mut instructions, opcodes::REF_STATE, 208);
    op_imm(&mut instructions, opcodes::BUILTIN, opcodes::builtin::RUN);
    op_imm(&mut instructions, opcodes::STORE_STATE, 0);
    op(&mut instructions, opcodes::RET);
    let mut constants = constant(b"child");
    constants.extend_from_slice(&constant(b"hello"));
    let spec = FnSpec {
        frame_size: 72,
        ..MAIN
    };
    build(&[spec], &instructions, &constants, 224, 128, 0)
}

fn diag_message(state: &[u8]) -> (u64, Vec<u8>) {
    let base = 16 + abi::layout::diagnostic::MESSAGE_OFFSET as usize;
    let length = state_u64(state, base);
    (length, state[base + 8..base + 8 + length as usize].to_vec())
}

#[test]
fn run_spawns_once_and_reports_an_immediate_completion() {
    let code = run_package();
    let mut host = MockHost {
        spawn_result: Some(Ok(7)),
        wait_results: vec![WaitOutcome::Exited(ExitReason::Ok)],
        ..Default::default()
    };
    let mut state = vec![0u8; 224];
    let mut stack = vec![0u8; 128];
    let (outcome, _) = run_with_host(&code, &mut state, &mut stack, &mut host, 200);

    assert_eq!(outcome, Outcome::Exited(ExitReason::Ok));
    assert_eq!(host.spawn_calls, 1);
    assert_eq!(host.wait_calls, 1);
    assert_eq!(state_u64(&state, 0), Status::Ok as u64);
    assert_eq!(
        state_u64(&state, 208),
        abi::layout::exit_tag::OK as u64,
        "the reason's tag is Ok"
    );
    assert_eq!(diag_message(&state).0, 0, "success clears the diagnostic");
}

#[test]
fn run_blocks_then_resumes_without_spawning_twice() {
    let code = run_package();
    let mut host = MockHost {
        spawn_result: Some(Ok(7)),
        wait_results: vec![
            WaitOutcome::Blocked,
            WaitOutcome::Exited(ExitReason::Faulted(FaultCode::Overflow)),
        ],
        ..Default::default()
    };
    let mut state = vec![0u8; 224];
    let mut stack = vec![0u8; 128];
    let mut machine = Machine::new();

    let outcome = resume_with_host(&code, &mut state, &mut stack, &mut host, 200, &mut machine);
    assert_eq!(outcome, Outcome::Blocked(WaitKind::ChildExit(7)));
    assert_eq!(host.spawn_calls, 1);
    assert_eq!(machine.pc(), 32, "pc stays on the run instruction");
    assert_eq!(state_u64(&state, 0), 0, "no status is reported yet");
    assert_eq!(
        state_u64(&state, 208),
        0,
        "the reason is untouched until completion"
    );

    let outcome = resume_with_host(&code, &mut state, &mut stack, &mut host, 200, &mut machine);
    assert_eq!(outcome, Outcome::Exited(ExitReason::Ok));
    assert_eq!(
        host.spawn_calls, 1,
        "the retry waits; it never spawns again"
    );
    assert_eq!(host.wait_calls, 2);
    assert_eq!(state_u64(&state, 0), Status::Ok as u64);
    assert_eq!(
        state_u64(&state, 208),
        abi::layout::exit_tag::FAULTED as u64
    );
    assert_eq!(state_u64(&state, 216), FaultCode::Overflow as u64);
}

#[test]
fn run_spawn_error_pushes_the_status_and_fills_the_diagnostic() {
    let code = run_package();
    let mut host = MockHost {
        spawn_result: Some(Err(Status::NotFound)),
        ..Default::default()
    };
    let mut state = vec![0u8; 224];
    let mut stack = vec![0u8; 128];
    let (outcome, _) = run_with_host(&code, &mut state, &mut stack, &mut host, 200);

    assert_eq!(outcome, Outcome::Exited(ExitReason::Ok));
    assert_eq!(host.spawn_calls, 1);
    assert_eq!(host.wait_calls, 0, "a spawn failure does not wait");
    assert_eq!(state_u64(&state, 0), Status::NotFound as u64);
    assert_eq!(diag_message(&state), (10, b"bad source".to_vec()));
    assert_eq!(
        state_u64(&state, 208),
        0,
        "the reason out-parameter is untouched"
    );
}

#[test]
fn run_wait_error_pushes_the_status_and_leaves_the_reason() {
    let code = run_package();
    let mut host = MockHost {
        spawn_result: Some(Ok(7)),
        wait_results: vec![WaitOutcome::Status(Status::NotFound)],
        ..Default::default()
    };
    let mut state = vec![0u8; 224];
    let mut stack = vec![0u8; 128];
    let (outcome, _) = run_with_host(&code, &mut state, &mut stack, &mut host, 200);

    assert_eq!(outcome, Outcome::Exited(ExitReason::Ok));
    assert_eq!(host.spawn_calls, 1);
    assert_eq!(host.wait_calls, 1);
    assert_eq!(state_u64(&state, 0), Status::NotFound as u64);
    assert_eq!(
        state_u64(&state, 208),
        0,
        "the reason out-parameter is untouched"
    );
}

#[test]
fn run_copies_the_request_completion_diagnostic_after_a_fault() {
    let code = run_package();
    let mut detail = [0u8; abi::layout::diagnostic::SIZE as usize];
    abi::layout::diagnostic::write(&mut detail, b"broken", b"expected `;`", 3, 1);
    let mut host = MockHost {
        spawn_result: Some(Ok(7)),
        wait_results: vec![WaitOutcome::Exited(ExitReason::Faulted(FaultCode::IoError))],
        diag_for: Some((7, detail.to_vec())),
        ..Default::default()
    };
    let mut state = vec![0u8; 224];
    let mut stack = vec![0u8; 128];
    let (outcome, _) = run_with_host(&code, &mut state, &mut stack, &mut host, 200);

    assert_eq!(outcome, Outcome::Exited(ExitReason::Ok));
    assert_eq!(state_u64(&state, 0), Status::Ok as u64);
    assert_eq!(
        state_u64(&state, 208),
        abi::layout::exit_tag::FAULTED as u64
    );
    assert_eq!(state_u64(&state, 216), FaultCode::IoError as u64);
    assert_eq!(diag_message(&state), (12, b"expected `;`".to_vec()));
    assert_eq!(
        state_u64(&state, 16 + abi::layout::diagnostic::LINE_OFFSET as usize),
        3
    );
}

#[test]
fn read_input_copies_the_available_bytes_without_blocking() {
    let mut instructions = Vec::new();
    op_imm(&mut instructions, opcodes::REF_STATE, 8);
    op_imm(&mut instructions, opcodes::READ_INPUT, 8);
    op_imm(&mut instructions, opcodes::STORE_STATE, 16);
    op(&mut instructions, opcodes::RET);
    let spec = FnSpec {
        frame_size: 64,
        ..MAIN
    };
    let code = build(&[spec], &instructions, &[], 24, 64, 0);

    let mut host = MockHost {
        input: b"alpha".to_vec(),
        ..Default::default()
    };
    let mut state = vec![0u8; 24];
    let mut stack = vec![0u8; 64];
    let (outcome, _) = run_with_host(&code, &mut state, &mut stack, &mut host, 100);
    assert_eq!(outcome, Outcome::Exited(ExitReason::Ok));
    assert_eq!(host.read_calls, 1);
    assert_eq!(u64::from_le_bytes(state[0..8].try_into().unwrap()), 5);
    assert_eq!(&state[8..13], b"alpha");
    assert_eq!(u64::from_le_bytes(state[16..24].try_into().unwrap()), 5);
}

#[test]
fn read_input_returns_zero_and_publishes_an_empty_buffer_when_nothing_is_waiting() {
    let mut instructions = Vec::new();
    op_imm(&mut instructions, opcodes::REF_STATE, 8);
    op_imm(&mut instructions, opcodes::READ_INPUT, 8);
    op_imm(&mut instructions, opcodes::STORE_STATE, 16);
    op(&mut instructions, opcodes::RET);
    let spec = FnSpec {
        frame_size: 64,
        ..MAIN
    };
    let code = build(&[spec], &instructions, &[], 24, 64, 0);

    let mut host = MockHost::default();
    let mut state = vec![0xFFu8; 24];
    let mut stack = vec![0u8; 64];
    let (outcome, _) = run_with_host(&code, &mut state, &mut stack, &mut host, 100);
    assert_eq!(outcome, Outcome::Exited(ExitReason::Ok));
    assert_eq!(u64::from_le_bytes(state[0..8].try_into().unwrap()), 0);
    assert_eq!(u64::from_le_bytes(state[16..24].try_into().unwrap()), 0);
}

#[test]
fn read_input_truncates_to_the_buffer_capacity() {
    let mut instructions = Vec::new();
    op_imm(&mut instructions, opcodes::REF_STATE, 8);
    op_imm(&mut instructions, opcodes::READ_INPUT, 4);
    op_imm(&mut instructions, opcodes::STORE_STATE, 16);
    op(&mut instructions, opcodes::RET);
    let spec = FnSpec {
        frame_size: 64,
        ..MAIN
    };
    let code = build(&[spec], &instructions, &[], 24, 64, 0);

    let mut host = MockHost {
        input: b"alphabet".to_vec(),
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

fn substr_package(kind: u8, source_kind: u8, capacity: u32, start: i64, length: i64) -> Vec<u8> {
    let mut instructions = Vec::new();
    op_imm(&mut instructions, opcodes::REF_STATE, 16);
    op_imm(&mut instructions, opcodes::REF_STATE, 32);
    op_const(&mut instructions, start);
    op_const(&mut instructions, length);
    op_word(&mut instructions, opcodes::SUBSTR, kind, source_kind, 0);
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
    source_kind: u8,
    capacity: u32,
    start: i64,
    length: i64,
    destination: &[u8],
    source: &[u8],
) -> (i64, Vec<u8>) {
    let code = substr_package(kind, source_kind, capacity, start, length);
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
    let (result, copied) = run_substr(
        opcodes::buffer_kind::BYTES,
        opcodes::buffer_kind::BYTES,
        8,
        2,
        99,
        b"XYZ",
        b"abcdef",
    );
    assert_eq!(result, 4);
    assert_eq!(copied, b"cdef");
}

#[test]
fn substr_clamps_to_the_destination_capacity() {
    let (result, copied) = run_substr(
        opcodes::buffer_kind::BYTES,
        opcodes::buffer_kind::BYTES,
        3,
        0,
        99,
        b"XYZ",
        b"abcdef",
    );
    assert_eq!(result, 3);
    assert_eq!(copied, b"abc");
}

#[test]
fn substr_past_the_end_copies_nothing() {
    let (result, copied) = run_substr(
        opcodes::buffer_kind::BYTES,
        opcodes::buffer_kind::BYTES,
        8,
        9,
        2,
        b"XYZ",
        b"abc",
    );
    assert_eq!(result, 0);
    assert_eq!(copied, b"");
}

#[test]
fn substr_zero_length_clears_the_destination() {
    let (result, copied) = run_substr(
        opcodes::buffer_kind::BYTES,
        opcodes::buffer_kind::BYTES,
        8,
        1,
        0,
        b"XYZ",
        b"abc",
    );
    assert_eq!(result, 0);
    assert_eq!(copied, b"");
}

#[test]
fn substr_negative_arguments_are_invalid_and_write_nothing() {
    for (start, length) in [(-1i64, 2i64), (0, -1)] {
        let (result, copied) = run_substr(
            opcodes::buffer_kind::BYTES,
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
fn substr_string_source_keeps_the_boundary_rule() {
    // "é" is two bytes (0xC3 0xA9); starting at 1 splits it.
    let (result, copied) = run_substr(
        opcodes::buffer_kind::STRING,
        opcodes::buffer_kind::STRING,
        8,
        1,
        1,
        b"XYZ",
        b"\xC3\xA9",
    );
    assert_eq!(result, -4);
    assert_eq!(copied, b"XYZ", "the destination is untouched");

    // An empty range that starts inside a code point is still not a boundary.
    let (result, copied) = run_substr(
        opcodes::buffer_kind::STRING,
        opcodes::buffer_kind::STRING,
        8,
        1,
        0,
        b"XYZ",
        b"\xC3\xA9",
    );
    assert_eq!(result, -4);
    assert_eq!(copied, b"XYZ", "the destination is untouched");

    // Both boundaries fall on code points, so the sequence copies.
    let (result, copied) = run_substr(
        opcodes::buffer_kind::STRING,
        opcodes::buffer_kind::STRING,
        8,
        1,
        2,
        b"",
        b"a\xC3\xA9",
    );
    assert_eq!(result, 2);
    assert_eq!(copied, b"\xC3\xA9");
}

#[test]
fn substr_bytes_source_into_a_string_is_strictly_validated() {
    let string = opcodes::buffer_kind::STRING;
    let bytes = opcodes::buffer_kind::BYTES;

    // A complete, well-formed sequence converts.
    let (result, copied) = run_substr(string, bytes, 8, 0, 2, b"", b"\xC3\xA9");
    assert_eq!(result, 2);
    assert_eq!(copied, b"\xC3\xA9");

    // Every malformed class is `-4` and leaves the destination alone.
    for bad in [
        &b"\x80"[..],        // stray continuation
        b"\xC0\xAF",         // overlong 2-byte
        b"\xC1\xBF",         // overlong 2-byte
        b"\xE0\x80\x80",     // overlong 3-byte
        b"\xED\xA0\x80",     // surrogate
        b"\xF0\x80\x80\x80", // overlong 4-byte
        b"\xF4\x90\x80\x80", // above U+10FFFF
        b"\xF5\x80\x80\x80", // invalid lead
        b"\xE2\x82",         // truncated
        b"\xC3",             // truncated
        b"\xC3\xA9\xFF",     // invalid byte after a good sequence
    ] {
        let (result, copied) = run_substr(string, bytes, 8, 0, bad.len() as i64, b"keep", bad);
        assert_eq!(result, -4, "accepted {bad:02X?}");
        assert_eq!(copied, b"keep", "the destination is untouched");
    }

    // A destination capacity that cuts a code point is a truncated sequence.
    let (result, copied) = run_substr(string, bytes, 1, 0, 2, b"", b"\xC3\xA9");
    assert_eq!(result, -4);
    assert_eq!(copied, b"");
}

#[test]
fn substr_bytes_destination_copies_raw_from_either_kind() {
    let bytes = opcodes::buffer_kind::BYTES;

    // A mid-code-point cut of a `string` is raw data (spec 0003, T4).
    let (result, copied) = run_substr(
        bytes,
        opcodes::buffer_kind::STRING,
        8,
        2,
        1,
        b"",
        b"a\xC3\xA9",
    );
    assert_eq!(result, 1);
    assert_eq!(copied, b"\xA9");

    // Raw bytes are copied without validation.
    let (result, copied) = run_substr(bytes, bytes, 8, 0, 2, b"", b"\xC0\xAF");
    assert_eq!(result, 2);
    assert_eq!(copied, b"\xC0\xAF");
}

#[test]
fn substr_rejects_a_source_kind_out_of_range() {
    let code = substr_package(opcodes::buffer_kind::STRING, 2, 8, 0, 1);
    let mut scratch = scratch_for(&code);
    let error = interpreter::verify(&code, &mut scratch).expect_err("must be refused");
    assert_eq!(error.status, Status::Invalid);
    assert!(
        error.message.contains("substr kind is out of range"),
        "{}",
        error.message
    );
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

// ---- floating-point text --------------------------------------------------
//
// Same `state` layout as the integer formatter: `state[0..8]` result,
// `state[8..16]` length, `state[16..]` destination data.

fn format_f64_package(
    kind: u8,
    capacity: u32,
    bits: u64,
    frac_digits: u64,
    state_size: u32,
) -> Vec<u8> {
    let mut instructions = Vec::new();
    op_imm(&mut instructions, opcodes::REF_STATE, 16);
    op_const(&mut instructions, bits as i64);
    op_const(&mut instructions, frac_digits as i64);
    op_word(&mut instructions, opcodes::FORMAT_F64, kind, 0, 0);
    instructions.extend_from_slice(&capacity.to_le_bytes());
    op_imm(&mut instructions, opcodes::STORE_STATE, 0);
    op(&mut instructions, opcodes::RET);
    let frame = FnSpec {
        frame_size: 80,
        ..BYTE_FRAME
    };
    build(&[frame], &instructions, &[], state_size, 96, 0)
}

/// Runs one `format_f64` and returns `(result, bytes, length)`. The destination
/// starts with `initial`, so an untouched buffer is observable.
fn run_format_f64(
    kind: u8,
    capacity: u32,
    bits: u64,
    frac_digits: u64,
    initial: &[u8],
) -> (i64, Vec<u8>, u64) {
    let state_size = (16 + capacity as usize).max(16 + initial.len()).max(24);
    let code = format_f64_package(kind, capacity, bits, frac_digits, state_size as u32);
    let mut host = MockHost::default();
    let mut state = vec![0u8; state_size];
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

fn format_f64(
    kind: u8,
    capacity: u32,
    value: f64,
    frac_digits: u64,
    initial: &[u8],
) -> (i64, Vec<u8>, u64) {
    run_format_f64(kind, capacity, value.to_bits(), frac_digits, initial)
}

#[test]
fn format_f64_writes_fixed_point_decimal() {
    let string = opcodes::buffer_kind::STRING;
    let bytes = opcodes::buffer_kind::BYTES;
    assert_eq!(
        format_f64(string, 16, 0.0, 2, b""),
        (4, b"0.00".to_vec(), 4)
    );
    assert_eq!(format_f64(string, 16, 0.0, 0, b""), (1, b"0".to_vec(), 1));
    assert_eq!(format_f64(string, 16, 1.5, 1, b""), (3, b"1.5".to_vec(), 3));
    assert_eq!(
        format_f64(string, 16, -1.5, 1, b""),
        (4, b"-1.5".to_vec(), 4)
    );
    assert_eq!(
        format_f64(string, 16, 42.25, 3, b""),
        (6, b"42.250".to_vec(), 6)
    );
    // Both kinds accept the ASCII output, and the count is the length.
    assert_eq!(
        format_f64(bytes, 16, 2.5, 0, b""),
        (1, b"3".to_vec(), 1),
        "0 fraction digits rounds 2.5 up"
    );
    assert_eq!(
        format_f64(bytes, 16, 1.0 / 3.0, 9, b""),
        (11, b"0.333333333".to_vec(), 11)
    );
    // The integer part never carries a leading zero.
    assert_eq!(
        format_f64(string, 16, 7.0, 2, b""),
        (4, b"7.00".to_vec(), 4)
    );
}

#[test]
fn format_f64_keeps_the_sign_of_negative_zero() {
    assert_eq!(
        format_f64(opcodes::buffer_kind::STRING, 16, -0.0, 3, b""),
        (6, b"-0.000".to_vec(), 6)
    );
    assert_eq!(
        format_f64(opcodes::buffer_kind::STRING, 16, -0.0, 0, b""),
        (2, b"-0".to_vec(), 2)
    );
}

#[test]
fn format_f64_rounds_half_away_from_zero_and_carries() {
    let string = opcodes::buffer_kind::STRING;
    // Half away from zero, in both directions: 0.75 is exact in binary.
    assert_eq!(
        format_f64(string, 16, 0.75, 1, b""),
        (3, b"0.8".to_vec(), 3)
    );
    assert_eq!(
        format_f64(string, 16, -0.75, 1, b""),
        (4, b"-0.8".to_vec(), 4)
    );
    assert_eq!(
        format_f64(string, 16, 0.25, 1, b""),
        (3, b"0.3".to_vec(), 3)
    );
    // Carry into the integer part.
    assert_eq!(
        format_f64(string, 16, 9.999, 2, b""),
        (5, b"10.00".to_vec(), 5)
    );
    assert_eq!(
        format_f64(string, 16, -9.999, 2, b""),
        (6, b"-10.00".to_vec(), 6)
    );
    // Just below half rounds down.
    assert_eq!(
        format_f64(string, 32, 0.74, 1, b""),
        (3, b"0.7".to_vec(), 3)
    );
    // Rounding to zero fraction digits can carry too.
    assert_eq!(format_f64(string, 16, 0.5, 0, b""), (1, b"1".to_vec(), 1));
    assert_eq!(format_f64(string, 16, 0.49, 0, b""), (1, b"0".to_vec(), 1));
}

#[test]
fn format_f64_writes_specials_literally() {
    for digits in [0u64, 3, 9] {
        assert_eq!(
            format_f64(opcodes::buffer_kind::STRING, 16, f64::NAN, digits, b""),
            (3, b"nan".to_vec(), 3),
            "{digits} digits"
        );
        assert_eq!(
            format_f64(opcodes::buffer_kind::STRING, 16, f64::INFINITY, digits, b""),
            (3, b"inf".to_vec(), 3)
        );
        assert_eq!(
            format_f64(
                opcodes::buffer_kind::STRING,
                16,
                f64::NEG_INFINITY,
                digits,
                b""
            ),
            (4, b"-inf".to_vec(), 4)
        );
    }
}

#[test]
fn format_f64_rejects_an_unrepresentable_magnitude() {
    let string = opcodes::buffer_kind::STRING;
    // Exactly 2^63 and anything above it has no fixed-point integer part.
    assert_eq!(
        format_f64(string, 32, 9_223_372_036_854_775_808.0, 0, b"XYZ"),
        (-4, b"XYZ".to_vec(), 3)
    );
    assert_eq!(
        format_f64(string, 32, -9_223_372_036_854_775_808.0, 0, b"XYZ"),
        (-4, b"XYZ".to_vec(), 3)
    );
    assert_eq!(
        format_f64(string, 32, 1.0e300, 3, b"XYZ"),
        (-4, b"XYZ".to_vec(), 3)
    );
    // The largest integer part below the bound still formats.
    assert_eq!(
        format_f64(string, 32, 9_223_372_036_854_774_784.0, 0, b""),
        (19, b"9223372036854774784".to_vec(), 19)
    );
}

#[test]
fn format_f64_rejects_more_than_nine_fraction_digits() {
    for digits in [10u64, 100, u64::MAX] {
        assert_eq!(
            format_f64(opcodes::buffer_kind::STRING, 16, 1.0, digits, b"XYZ"),
            (-4, b"XYZ".to_vec(), 3),
            "{digits} digits"
        );
    }
    // The width bound applies before the specials: a 10-digit field is invalid
    // even though `nan` would ignore the width.
    assert_eq!(
        format_f64(opcodes::buffer_kind::STRING, 16, f64::NAN, 10, b"XYZ"),
        (-4, b"XYZ".to_vec(), 3)
    );
}

#[test]
fn format_f64_reports_capacity_and_leaves_the_length_alone() {
    let string = opcodes::buffer_kind::STRING;
    // "42.5" needs 4 bytes; 3 is one short.
    assert_eq!(
        format_f64(string, 3, 42.5, 1, b"XYZ"),
        (-6, b"XYZ".to_vec(), 3)
    );
    assert_eq!(
        format_f64(string, 4, 42.5, 1, b""),
        (4, b"42.5".to_vec(), 4)
    );
    // The sign and the fraction digits count against the capacity.
    assert_eq!(
        format_f64(string, 3, -1.5, 1, b"XYZ"),
        (-6, b"XYZ".to_vec(), 3)
    );
    assert_eq!(
        format_f64(
            opcodes::buffer_kind::BYTES,
            10,
            f64::NEG_INFINITY,
            0,
            b"XYZ"
        ),
        (4, b"-inf".to_vec(), 4)
    );
    assert_eq!(
        format_f64(opcodes::buffer_kind::BYTES, 3, f64::NEG_INFINITY, 0, b"XYZ"),
        (-6, b"XYZ".to_vec(), 3)
    );
}

#[test]
fn format_f64_rejects_a_kind_or_capacity_out_of_range() {
    let code = format_f64_package(2, 8, 0, 0, 24);
    let mut scratch = scratch_for(&code);
    let error = interpreter::verify(&code, &mut scratch).expect_err("must be refused");
    assert_eq!(error.status, Status::Invalid);
    assert!(
        error.message.contains("format_f64 kind is out of range"),
        "{}",
        error.message
    );

    // A zero-capacity destination can never receive a digit.
    let code = format_f64_package(opcodes::buffer_kind::STRING, 0, 0, 0, 24);
    let mut scratch = scratch_for(&code);
    let error = interpreter::verify(&code, &mut scratch).expect_err("must be refused");
    assert_eq!(error.status, Status::Invalid);
    assert!(
        error
            .message
            .contains("format_f64 destination capacity is out of range"),
        "{}",
        error.message
    );
}

// ---- shortest round-trip floating-point text ------------------------------
//
// Same `state` layout as the fixed-point formatter: `state[0..8]` result,
// `state[8..16]` length, `state[16..]` destination data.

fn format_f64_short_package(kind: u8, capacity: u32, bits: u64, state_size: u32) -> Vec<u8> {
    let mut instructions = Vec::new();
    op_imm(&mut instructions, opcodes::REF_STATE, 16);
    op_const(&mut instructions, bits as i64);
    op_word(&mut instructions, opcodes::FORMAT_F64_SHORT, kind, 0, 0);
    instructions.extend_from_slice(&capacity.to_le_bytes());
    op_imm(&mut instructions, opcodes::STORE_STATE, 0);
    op(&mut instructions, opcodes::RET);
    let frame = FnSpec {
        frame_size: 80,
        ..BYTE_FRAME
    };
    build(&[frame], &instructions, &[], state_size, 96, 0)
}

/// Runs one `format_f64_short` and returns `(result, bytes, length)`. The
/// destination starts with `initial`, so an untouched buffer is observable.
fn run_format_f64_short(kind: u8, capacity: u32, bits: u64, initial: &[u8]) -> (i64, Vec<u8>, u64) {
    let state_size = (16 + capacity as usize).max(16 + initial.len()).max(24);
    let code = format_f64_short_package(kind, capacity, bits, state_size as u32);
    let mut host = MockHost::default();
    let mut state = vec![0u8; state_size];
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

fn format_f64_short(kind: u8, capacity: u32, value: f64) -> (i64, Vec<u8>, u64) {
    run_format_f64_short(kind, capacity, value.to_bits(), b"")
}

#[test]
fn format_f64_short_writes_the_shortest_round_trip() {
    let string = opcodes::buffer_kind::STRING;
    let bytes = opcodes::buffer_kind::BYTES;
    // (value, text) pairs; the text is the spec's shape rule applied to the
    // shortest digits, which the property test checks against Rust.
    let cases: &[(f64, &[u8])] = &[
        (0.0, b"0"),
        (-0.0, b"-0"),
        (1.0, b"1"),
        (-1.0, b"-1"),
        (0.1, b"0.1"),
        (1.0 / 3.0, b"0.3333333333333333"),
        (1.0e21, b"1e21"),
        (1.0e20, b"100000000000000000000"),
        (1.0e-6, b"0.000001"),
        (1.0e-7, b"1e-7"),
        (5.0e-324, b"5e-324"),
        (f64::MAX, b"1.7976931348623157e308"),
        (f64::MIN_POSITIVE, b"2.2250738585072014e-308"),
        (9_007_199_254_740_992.0, b"9007199254740992"),
        (9_007_199_254_740_994.0, b"9007199254740994"),
        // 2^53 + 1 is not representable, so it formats as 2^53.
        (9_007_199_254_740_993.0, b"9007199254740992"),
        (0.30000000000000004, b"0.30000000000000004"),
        (f64::NAN, b"nan"),
        (f64::INFINITY, b"inf"),
        (f64::NEG_INFINITY, b"-inf"),
    ];
    for (value, want) in cases {
        assert_eq!(
            format_f64_short(string, 64, *value),
            (want.len() as i64, want.to_vec(), want.len() as u64),
            "{value:?}"
        );
    }
    // Both destination kinds accept the ASCII text.
    assert_eq!(
        format_f64_short(bytes, 64, 1.0e21),
        (4, b"1e21".to_vec(), 4)
    );
    assert_eq!(
        format_f64_short(bytes, 64, f64::NEG_INFINITY),
        (4, b"-inf".to_vec(), 4)
    );
}

#[test]
fn format_f64_short_reports_capacity_and_leaves_the_length_alone() {
    let string = opcodes::buffer_kind::STRING;
    // "0.30000000000000004" is 19 bytes.
    assert_eq!(
        run_format_f64_short(string, 18, 0.30000000000000004f64.to_bits(), b"XYZ"),
        (-6, b"XYZ".to_vec(), 3)
    );
    // "1.7976931348623157e308" is 22 bytes.
    assert_eq!(
        run_format_f64_short(string, 21, f64::MAX.to_bits(), b"XYZ"),
        (-6, b"XYZ".to_vec(), 3)
    );
    // Even the shortest form needs room for its exponent.
    assert_eq!(
        run_format_f64_short(string, 3, 1.0e21f64.to_bits(), b"XYZ"),
        (-6, b"XYZ".to_vec(), 3)
    );
    // A one-byte-short `-inf` fails too, in either kind.
    assert_eq!(
        run_format_f64_short(
            opcodes::buffer_kind::BYTES,
            3,
            f64::NEG_INFINITY.to_bits(),
            b"XYZ"
        ),
        (-6, b"XYZ".to_vec(), 3)
    );
    // Exactly enough room succeeds.
    assert_eq!(
        format_f64_short(string, 4, 1.0e21),
        (4, b"1e21".to_vec(), 4)
    );
}

#[test]
fn format_f64_short_rejects_a_kind_capacity_or_reserved_field() {
    let code = format_f64_short_package(2, 8, 0, 24);
    let mut scratch = scratch_for(&code);
    let error = interpreter::verify(&code, &mut scratch).expect_err("must be refused");
    assert_eq!(error.status, Status::Invalid);
    assert!(
        error
            .message
            .contains("format_f64_short kind is out of range"),
        "{}",
        error.message
    );

    // A zero-capacity destination can never receive a digit.
    let code = format_f64_short_package(opcodes::buffer_kind::STRING, 0, 0, 24);
    let mut scratch = scratch_for(&code);
    let error = interpreter::verify(&code, &mut scratch).expect_err("must be refused");
    assert_eq!(error.status, Status::Invalid);
    assert!(
        error
            .message
            .contains("format_f64_short destination capacity is out of range"),
        "{}",
        error.message
    );

    // `b` and `c` are reserved and fold into the same kind check, as they do
    // for `format_f64`.
    for (b, c) in [(1u8, 0u8), (0, 1)] {
        let mut instructions = Vec::new();
        op_imm(&mut instructions, opcodes::REF_STATE, 16);
        op_const(&mut instructions, 0);
        op_word(&mut instructions, opcodes::FORMAT_F64_SHORT, 0, b, c);
        instructions.extend_from_slice(&8u32.to_le_bytes());
        op_imm(&mut instructions, opcodes::STORE_STATE, 0);
        op(&mut instructions, opcodes::RET);
        let frame = FnSpec {
            frame_size: 80,
            ..BYTE_FRAME
        };
        let code = build(&[frame], &instructions, &[], 24, 96, 0);
        let mut scratch = scratch_for(&code);
        let error = interpreter::verify(&code, &mut scratch).expect_err("must be refused");
        assert_eq!(error.status, Status::Invalid);
        assert!(
            error
                .message
                .contains("format_f64_short kind is out of range"),
            "{}",
            error.message
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

fn op_ref_element(out: &mut Vec<u8>, stride: u32, count: u32) {
    op(out, opcodes::REF_ELEMENT);
    out.extend_from_slice(&stride.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
}

#[test]
fn ref_element_reads_and_writes_array_elements() {
    let mut instructions = Vec::new();
    // state[16] := state[8] (element 1 of an array at 0, stride 8, count 4).
    op_imm(&mut instructions, opcodes::REF_STATE, 0);
    op_const(&mut instructions, 1);
    op_ref_element(&mut instructions, 8, 4);
    op(&mut instructions, opcodes::LOAD_REF);
    op_imm(&mut instructions, opcodes::STORE_STATE, 24);
    // state[16] := 0x99 through the same element reference.
    op_imm(&mut instructions, opcodes::REF_STATE, 0);
    op_const(&mut instructions, 2);
    op_ref_element(&mut instructions, 8, 4);
    op_const(&mut instructions, 0x99);
    op(&mut instructions, opcodes::STORE_REF);
    op(&mut instructions, opcodes::RET);
    let spec = FnSpec {
        frame_size: 64,
        ..MAIN
    };
    let code = build(&[spec], &instructions, &[], 32, 64, 0);
    let mut scratch = scratch_for(&code);
    let program = Program::parse(&code, &mut scratch).expect("verifies");
    let mut state = vec![0u8; 32];
    state[0..8].copy_from_slice(&0x11u64.to_le_bytes());
    state[8..16].copy_from_slice(&0x22u64.to_le_bytes());
    state[16..24].copy_from_slice(&0x33u64.to_le_bytes());
    let mut stack = vec![0u8; 64];
    let args = vec![0u8; 8 + 1024];
    let mut machine = Machine::new();
    let mut host = Recorder { output: Vec::new() };
    assert_eq!(
        machine.run(&program, &mut host, &mut state, &mut stack, &args, 100),
        Outcome::Exited(ExitReason::Ok)
    );
    assert_eq!(state_u64(&state, 24), 0x22, "element 1 was read");
    assert_eq!(state_u64(&state, 16), 0x99, "element 2 was written");
}

#[test]
fn ref_element_nests_and_copies_a_whole_element() {
    let mut instructions = Vec::new();
    // state[24] := 0x77 through element [1][1] of an array of arrays
    // (outer stride 16 count 2, inner stride 8 count 2).
    op_imm(&mut instructions, opcodes::REF_STATE, 0);
    op_const(&mut instructions, 1);
    op_ref_element(&mut instructions, 16, 2);
    op_const(&mut instructions, 1);
    op_ref_element(&mut instructions, 8, 2);
    op_const(&mut instructions, 0x77);
    op(&mut instructions, opcodes::STORE_REF);
    // state[0..16] := state[16..32] (one whole outer element).
    op_imm(&mut instructions, opcodes::REF_STATE, 0);
    op_const(&mut instructions, 0);
    op_ref_element(&mut instructions, 16, 2);
    op_imm(&mut instructions, opcodes::REF_STATE, 0);
    op_const(&mut instructions, 1);
    op_ref_element(&mut instructions, 16, 2);
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
    let mut stack = vec![0u8; 64];
    let args = vec![0u8; 8 + 1024];
    let mut machine = Machine::new();
    let mut host = Recorder { output: Vec::new() };
    assert_eq!(
        machine.run(&program, &mut host, &mut state, &mut stack, &args, 100),
        Outcome::Exited(ExitReason::Ok)
    );
    assert_eq!(state_u64(&state, 24), 0x77, "nested element [1][1] written");
    assert_eq!(
        &state[0..16],
        &state[16..32],
        "the whole outer element was copied"
    );
}

#[test]
fn ref_element_out_of_range_faults_out_of_bounds() {
    for index in [4i64, -1] {
        let mut instructions = Vec::new();
        op_imm(&mut instructions, opcodes::REF_STATE, 0);
        op_const(&mut instructions, index);
        op_ref_element(&mut instructions, 8, 4);
        op(&mut instructions, opcodes::LOAD_REF);
        op(&mut instructions, opcodes::DROP);
        op(&mut instructions, opcodes::RET);
        let spec = FnSpec {
            frame_size: 64,
            ..MAIN
        };
        let code = build(&[spec], &instructions, &[], 32, 64, 0);
        let (reason, _) = execute(&code, 32, 64);
        assert_eq!(
            reason,
            ExitReason::Faulted(FaultCode::OutOfBounds),
            "index {index} must fault, never clamp"
        );
    }
}

#[test]
fn the_verifier_rejects_a_bad_element_stride_or_count() {
    for (stride, count) in [(4u32, 4u32), (8, 0), (8, 1 << 30)] {
        let mut instructions = Vec::new();
        op_imm(&mut instructions, opcodes::REF_STATE, 0);
        op_const(&mut instructions, 0);
        op_ref_element(&mut instructions, stride, count);
        op(&mut instructions, opcodes::DROP);
        op(&mut instructions, opcodes::RET);
        let spec = FnSpec {
            frame_size: 64,
            ..MAIN
        };
        let code = build(&[spec], &instructions, &[], 32, 64, 0);
        let mut scratch = scratch_for(&code);
        let error = Program::parse(&code, &mut scratch).expect_err("must be rejected");
        assert!(
            error.message.contains("array"),
            "stride {stride}, count {count}: {}",
            error.message
        );
    }
}

// ---- rings ----------------------------------------------------------------
//
// A ring is not an opcode: the compiler lowers `push`/`pop`/`peek` to the same
// `ref_offset`/`ref_element`/`copy_refs`/branch instructions an array uses.
// These tests hand-assemble that lowering over a known layout (storage first,
// then `head` and `count`) and prove its semantics in the machine itself.
//
// Layout used here: element stride 8, storage `N * 8` at state offset `base`,
// `head` at `base + N*8`, `count` at `base + N*8 + 8`.

struct Asm {
    code: Vec<u8>,
    labels: Vec<Option<usize>>,
    pending: Vec<(usize, usize)>,
}

impl Asm {
    fn new() -> Self {
        Asm {
            code: Vec::new(),
            labels: Vec::new(),
            pending: Vec::new(),
        }
    }

    fn word(&mut self, opcode: u8) {
        self.code.extend_from_slice(&[opcode, 0, 0, 0]);
    }

    fn word_imm(&mut self, opcode: u8, immediate: u32) {
        self.word(opcode);
        self.code.extend_from_slice(&immediate.to_le_bytes());
    }

    fn const_i64(&mut self, value: i64) {
        self.word(opcodes::CONST);
        self.code.extend_from_slice(&(value as u32).to_le_bytes());
        self.code
            .extend_from_slice(&((value >> 32) as u32).to_le_bytes());
    }

    fn label(&mut self) -> usize {
        self.labels.push(None);
        self.labels.len() - 1
    }

    fn bind(&mut self, label: usize) {
        self.labels[label] = Some(self.code.len());
    }

    fn jump(&mut self, opcode: u8, label: usize) {
        self.word(opcode);
        let imm_pos = self.code.len();
        self.code.extend_from_slice(&0i32.to_le_bytes());
        self.pending.push((imm_pos, label));
    }

    fn finish(mut self) -> Vec<u8> {
        for (imm_pos, label) in self.pending.clone() {
            let target = self.labels[label].expect("bound label");
            let next = imm_pos + 4;
            let delta = (target as i64 - next as i64) / 4;
            self.code[imm_pos..next].copy_from_slice(&(delta as i32).to_le_bytes());
        }
        self.word(opcodes::RET);
        self.code
    }

    /// Pushes the `head` or `count` header word.
    fn header(&mut self, base: u32, off: u32) {
        self.word_imm(opcodes::REF_STATE, base);
        self.word_imm(opcodes::REF_OFFSET, off);
        self.word(opcodes::LOAD_REF);
    }

    /// Pushes a reference to the next push slot or the oldest element.
    fn element(&mut self, base: u32, count: u32, off: u32, at_head: bool) {
        self.word_imm(opcodes::REF_STATE, base);
        if at_head {
            self.header(base, off);
        } else {
            self.header(base, off);
            self.header(base, off + 8);
            self.word(opcodes::ADD_U);
            self.const_i64(count as i64);
            self.word(opcodes::MOD_U);
        }
        self.word(opcodes::REF_ELEMENT);
        self.code.extend_from_slice(&8u32.to_le_bytes());
        self.code.extend_from_slice(&count.to_le_bytes());
    }

    fn adjust_count(&mut self, base: u32, off: u32, delta: i64) {
        self.word_imm(opcodes::REF_STATE, base);
        self.word_imm(opcodes::REF_OFFSET, off + 8);
        self.header(base, off + 8);
        self.const_i64(delta.abs());
        if delta > 0 {
            self.word(opcodes::ADD_U);
        } else {
            self.word(opcodes::SUB_U);
        }
        self.word(opcodes::STORE_REF);
    }

    fn advance_head(&mut self, base: u32, count: u32, off: u32) {
        self.word_imm(opcodes::REF_STATE, base);
        self.word_imm(opcodes::REF_OFFSET, off);
        self.header(base, off);
        self.const_i64(1);
        self.word(opcodes::ADD_U);
        self.const_i64(count as i64);
        self.word(opcodes::MOD_U);
        self.word(opcodes::STORE_REF);
    }

    /// `core.ring_push`: leaves the `Status` operand on the stack.
    fn push(&mut self, base: u32, count: u32, off: u32, value: i64) {
        let full = self.label();
        let end = self.label();
        self.header(base, off + 8);
        self.const_i64(count as i64);
        self.word(opcodes::EQ);
        self.jump(opcodes::JNZ, full);
        self.element(base, count, off, false);
        self.const_i64(value);
        self.word(opcodes::STORE_REF);
        self.adjust_count(base, off, 1);
        self.const_i64(0);
        self.jump(opcodes::JMP, end);
        self.bind(full);
        self.const_i64(5);
        self.bind(end);
    }

    /// `core.ring_pop` (or `peek` when `remove` is false): copies the oldest
    /// element into `out` and leaves the `Status` operand on the stack.
    fn pop(&mut self, base: u32, count: u32, off: u32, out: u32, remove: bool) {
        let empty = self.label();
        let end = self.label();
        self.header(base, off + 8);
        self.const_i64(0);
        self.word(opcodes::EQ);
        self.jump(opcodes::JNZ, empty);
        self.word_imm(opcodes::REF_STATE, out);
        self.element(base, count, off, true);
        self.word(opcodes::LOAD_REF);
        self.word(opcodes::STORE_REF);
        if remove {
            self.advance_head(base, count, off);
            self.adjust_count(base, off, -1);
        }
        self.const_i64(0);
        self.jump(opcodes::JMP, end);
        self.bind(empty);
        self.const_i64(3);
        self.bind(end);
    }
}

/// Runs a hand-assembled body against a fresh state and returns the state and
/// the outcome.
fn run_ring(code: &[u8], state: &mut [u8]) -> (Outcome, Vec<u8>) {
    let spec = FnSpec {
        frame_size: 128,
        ..MAIN
    };
    let package = build(&[spec], code, &[], state.len() as u32, 128, 0);
    let mut scratch = scratch_for(&package);
    let program = Program::parse(&package, &mut scratch).expect("verifies");
    let mut stack = vec![0u8; 128];
    let args = vec![0u8; 8 + 1024];
    let mut machine = Machine::new();
    let mut host = Recorder { output: Vec::new() };
    let outcome = machine.run(&program, &mut host, state, &mut stack, &args, 10_000);
    (outcome, stack)
}

#[test]
fn a_lowered_ring_push_and_pop_are_fifo() {
    // ring[int, 3] at state 0: storage 0..24, head 24, count 32; out 40..64.
    let (base, count, head_off, out) = (0u32, 3u32, 24u32, 40u32);
    let mut asm = Asm::new();
    for value in [10, 20, 30] {
        asm.push(base, count, head_off, value);
        asm.word(opcodes::DROP);
    }
    for slot in [out, out + 8, out + 16] {
        asm.pop(base, count, head_off, slot, true);
        asm.word(opcodes::DROP);
    }
    let code = asm.finish();
    let mut state = vec![0u8; 64];
    let (outcome, _) = run_ring(&code, &mut state);
    assert_eq!(outcome, Outcome::Exited(ExitReason::Ok));
    assert_eq!(state_u64(&state, out as usize), 10);
    assert_eq!(state_u64(&state, out as usize + 8), 20);
    assert_eq!(state_u64(&state, out as usize + 16), 30, "FIFO order");
    assert_eq!(state_u64(&state, head_off as usize + 8), 0, "empty again");
}

#[test]
fn a_lowered_ring_peek_does_not_remove() {
    // ring[int, 2] at 0: storage 0..16, head 16, count 24, size 32.
    let (base, count, head_off, out) = (0u32, 2u32, 16u32, 32u32);
    let mut asm = Asm::new();
    asm.push(base, count, head_off, 7);
    asm.word(opcodes::DROP);
    asm.pop(base, count, head_off, out, false);
    asm.word(opcodes::DROP);
    asm.pop(base, count, head_off, out + 8, false);
    asm.word(opcodes::DROP);
    let code = asm.finish();
    let mut state = vec![0u8; 48];
    let (outcome, _) = run_ring(&code, &mut state);
    assert_eq!(outcome, Outcome::Exited(ExitReason::Ok));
    assert_eq!(state_u64(&state, out as usize), 7);
    assert_eq!(state_u64(&state, out as usize + 8), 7, "peek is repeatable");
    assert_eq!(
        state_u64(&state, head_off as usize + 8),
        1,
        "count is unchanged"
    );
}

#[test]
fn a_lowered_push_on_a_full_ring_changes_nothing() {
    let (base, count, head_off) = (0u32, 2u32, 16u32);
    let mut asm = Asm::new();
    asm.push(base, count, head_off, 1);
    asm.word(opcodes::DROP);
    asm.push(base, count, head_off, 2);
    asm.word(opcodes::DROP);
    asm.push(base, count, head_off, 3); // Full: writes nothing
    asm.word_imm(opcodes::STORE_STATE, 40);
    let code = asm.finish();
    let mut state = vec![0u8; 48];
    let (outcome, _) = run_ring(&code, &mut state);
    assert_eq!(outcome, Outcome::Exited(ExitReason::Ok));
    assert_eq!(state_u64(&state, 40), 5, "Status.Full");
    assert_eq!(state_u64(&state, 0), 1, "the oldest element is untouched");
    assert_eq!(state_u64(&state, 8), 2);
    assert_eq!(state_u64(&state, head_off as usize + 8), 2, "count is 2");
    assert_eq!(
        state_u64(&state, 16),
        0,
        "the failed push wrote no element at the next slot"
    );
}

#[test]
fn a_lowered_pop_or_peek_on_an_empty_ring_leaves_the_out_parameter() {
    let (base, count, head_off, out) = (0u32, 2u32, 16u32, 32u32);
    for remove in [true, false] {
        let mut asm = Asm::new();
        asm.pop(base, count, head_off, out, remove);
        asm.word_imm(opcodes::STORE_STATE, 40);
        let code = asm.finish();
        let mut state = vec![0u8; 48];
        state[out as usize..out as usize + 8].copy_from_slice(&99u64.to_le_bytes());
        let (outcome, _) = run_ring(&code, &mut state);
        assert_eq!(outcome, Outcome::Exited(ExitReason::Ok));
        assert_eq!(state_u64(&state, 40), 3, "Status.NotFound");
        assert_eq!(state_u64(&state, out as usize), 99, "untouched");
        assert_eq!(state_u64(&state, head_off as usize + 8), 0, "still empty");
    }
}

#[test]
fn a_whole_ring_copy_duplicates_head_and_count() {
    // ring[int, 2] at 0 (size 32); a second copy at 32; out slots at 64, 72.
    let (count, head_off) = (2u32, 16u32);
    let mut asm = Asm::new();
    asm.push(0, count, head_off, 1);
    asm.word(opcodes::DROP);
    asm.push(0, count, head_off, 2);
    asm.word(opcodes::DROP);
    // Remove the oldest: head becomes 1, count 1.
    asm.pop(0, count, head_off, 64, true);
    asm.word(opcodes::DROP);
    // copy := small: 32 bytes, header included.
    asm.word_imm(opcodes::REF_STATE, 32);
    asm.word_imm(opcodes::REF_STATE, 0);
    asm.const_i64(32);
    asm.word(opcodes::COPY_REFS);
    // The copy's oldest element is still 2, from its own head/count; peeking
    // it leaves both rings untouched.
    asm.pop(32, count, head_off, 72, false);
    asm.word(opcodes::DROP);
    let code = asm.finish();
    let mut state = vec![0u8; 88];
    let (outcome, _) = run_ring(&code, &mut state);
    assert_eq!(outcome, Outcome::Exited(ExitReason::Ok));
    assert_eq!(state_u64(&state, 64), 1);
    assert_eq!(state_u64(&state, 72), 2, "the copy carries head/count");
    assert_eq!(state_u64(&state, 16), 1, "head of the original");
    assert_eq!(state_u64(&state, 24), 1, "count of the original");
    assert_eq!(state_u64(&state, 48), 1, "head of the copy");
    assert_eq!(state_u64(&state, 56), 1, "count of the copy");
    assert_eq!(
        &state[0..16],
        &state[32..48],
        "the storage is copied byte for byte"
    );
}

#[test]
fn a_lowered_ring_wraps_around_several_times() {
    let (base, count, head_off) = (0u32, 3u32, 24u32);
    let mut asm = Asm::new();
    for value in [1, 2, 3] {
        asm.push(base, count, head_off, value);
        asm.word(opcodes::DROP);
    }
    asm.push(base, count, head_off, 4); // full: returns Full, writes nothing
    asm.word_imm(opcodes::STORE_STATE, 80);
    asm.pop(base, count, head_off, 40, true); // 1
    asm.word(opcodes::DROP);
    asm.push(base, count, head_off, 4); // wraps to physical 0
    asm.word(opcodes::DROP);
    asm.pop(base, count, head_off, 48, true); // 2
    asm.word(opcodes::DROP);
    asm.push(base, count, head_off, 5); // wraps to physical 1
    asm.word(opcodes::DROP);
    asm.pop(base, count, head_off, 56, true); // 3
    asm.word(opcodes::DROP);
    asm.pop(base, count, head_off, 64, true); // 4
    asm.word(opcodes::DROP);
    asm.pop(base, count, head_off, 72, true); // 5
    asm.word(opcodes::DROP);
    let code = asm.finish();
    let mut state = vec![0u8; 88];
    let (outcome, _) = run_ring(&code, &mut state);
    assert_eq!(outcome, Outcome::Exited(ExitReason::Ok));
    assert_eq!(state_u64(&state, 80), 5, "the full push reported Full");
    assert_eq!(state_u64(&state, 40), 1);
    assert_eq!(state_u64(&state, 48), 2);
    assert_eq!(state_u64(&state, 56), 3);
    assert_eq!(state_u64(&state, 64), 4, "pushed across the wrap");
    assert_eq!(state_u64(&state, 72), 5);
    assert_eq!(state_u64(&state, head_off as usize + 8), 0, "empty");
}

// ---- maps -----------------------------------------------------------------
//
// A map is `count: u64` at offset 0, then `capacity` slots of
// `16 + key_size + value_size` bytes: state (0 empty, 1 occupied), the stored
// FNV-1a hash, the key image and the value image. These tests hand-assemble
// the `map_*` opcodes over a known layout and prove insert/replace/get/has/
// remove/count, probing past colliding entries, backward-shift deletion, key 0,
// buffer keys, aggregate values, whole-map copies and the verifier's checks.
//
// `map[int, int, 3]`: capacity 8, limit 3, key 8, value 8, stride 32,
// total 8 + 8*32 = 264. Keys 5, 13 and 21 all hash to home slot 0.

#[allow(clippy::too_many_arguments)]
fn op_map(
    out: &mut Vec<u8>,
    opcode: u8,
    key_kind: u8,
    value_kind: u8,
    capacity: u32,
    limit: u32,
    key_size: u32,
    value_size: u32,
) {
    op_word(out, opcode, key_kind, value_kind, 0);
    for immediate in [capacity, limit, key_size, value_size] {
        out.extend_from_slice(&immediate.to_le_bytes());
    }
}

fn op_int_map_put(out: &mut Vec<u8>, base: u32, key: i64, value: i64) {
    op_imm(out, opcodes::REF_STATE, base);
    op_const(out, key);
    op_const(out, value);
    op_map(
        out,
        opcodes::MAP_PUT,
        opcodes::map_key_kind::INT,
        opcodes::map_value_kind::SCALAR,
        8,
        3,
        8,
        8,
    );
}

fn op_int_map_key(out: &mut Vec<u8>, opcode: u8, base: u32, key: i64, destination: u32) {
    op_imm(out, opcodes::REF_STATE, base);
    op_const(out, key);
    if opcode == opcodes::MAP_GET {
        op_imm(out, opcodes::REF_STATE, destination);
    }
    op_map(
        out,
        opcode,
        opcodes::map_key_kind::INT,
        opcodes::map_value_kind::SCALAR,
        8,
        3,
        8,
        8,
    );
}

/// Runs a hand-assembled body against a fresh state and returns the state, the
/// outcome and the operand stack.
fn run_map(code: &[u8], state: &mut [u8], state_size: u32) -> (Outcome, Vec<u8>) {
    let _ = state_size;
    let spec = FnSpec {
        frame_size: 128,
        ..MAIN
    };
    let package = build(&[spec], code, &[], state.len() as u32, 128, 0);
    let mut scratch = scratch_for(&package);
    let program = Program::parse(&package, &mut scratch).expect("verifies");
    let mut stack = vec![0u8; 128];
    let args = vec![0u8; 8 + 1024];
    let mut machine = Machine::new();
    let mut host = Recorder { output: Vec::new() };
    let outcome = machine.run(&program, &mut host, state, &mut stack, &args, 10_000);
    (outcome, stack)
}

/// Runs a body that must fault and returns the fault code.
fn map_fault(code: &[u8], state_size: u32) -> FaultCode {
    let spec = FnSpec {
        frame_size: 128,
        ..MAIN
    };
    let package = build(&[spec], code, &[], state_size, 128, 0);
    let mut scratch = scratch_for(&package);
    let program = Program::parse(&package, &mut scratch).expect("verifies");
    let mut state = vec![0u8; state_size as usize];
    let mut stack = vec![0u8; 128];
    let args = vec![0u8; 8 + 1024];
    let mut machine = Machine::new();
    let mut host = Recorder { output: Vec::new() };
    match machine.run(&program, &mut host, &mut state, &mut stack, &args, 10_000) {
        Outcome::Exited(ExitReason::Faulted(code)) => code,
        other => panic!("expected a fault, got {other:?}"),
    }
}

#[test]
fn map_put_get_has_remove_and_count() {
    // map at 0 (264 bytes); statuses and out-parameters after it.
    let mut instructions = Vec::new();
    op_int_map_put(&mut instructions, 0, 5, 50);
    op_imm(&mut instructions, opcodes::STORE_STATE, 264);
    op_int_map_put(&mut instructions, 0, 13, 130);
    op_imm(&mut instructions, opcodes::STORE_STATE, 272);
    op_int_map_put(&mut instructions, 0, 21, 210);
    op_imm(&mut instructions, opcodes::STORE_STATE, 280);
    // get 21 -> 288
    op_int_map_key(&mut instructions, opcodes::MAP_GET, 0, 21, 288);
    op_imm(&mut instructions, opcodes::STORE_STATE, 296);
    // has 13 -> 304
    op_int_map_key(&mut instructions, opcodes::MAP_HAS, 0, 13, 0);
    op_imm(&mut instructions, opcodes::STORE_STATE, 304);
    // remove 13 -> 312
    op_int_map_key(&mut instructions, opcodes::MAP_REMOVE, 0, 13, 0);
    op_imm(&mut instructions, opcodes::STORE_STATE, 312);
    // get 13 -> 320 (preloaded, must stay untouched)
    op_int_map_key(&mut instructions, opcodes::MAP_GET, 0, 13, 320);
    op_imm(&mut instructions, opcodes::STORE_STATE, 328);
    // get 5 -> 336, get 21 -> 344 (both must survive the shift)
    op_int_map_key(&mut instructions, opcodes::MAP_GET, 0, 5, 336);
    op_imm(&mut instructions, opcodes::STORE_STATE, 352);
    op_int_map_key(&mut instructions, opcodes::MAP_GET, 0, 21, 344);
    op_imm(&mut instructions, opcodes::STORE_STATE, 360);
    // count -> 368
    op_imm(&mut instructions, opcodes::REF_STATE, 0);
    op(&mut instructions, opcodes::MAP_COUNT);
    op_imm(&mut instructions, opcodes::STORE_STATE, 368);
    op(&mut instructions, opcodes::RET);

    let mut state = vec![0u8; 512];
    state[320..328].copy_from_slice(&0xDEADu64.to_le_bytes());
    let (outcome, _) = run_map(&instructions, &mut state, 512);
    assert_eq!(outcome, Outcome::Exited(ExitReason::Ok));
    for status in [264, 272, 280, 296, 312, 352, 360] {
        assert_eq!(state_u64(&state, status), 0, "operation {status} was Ok");
    }
    assert_eq!(state_u64(&state, 288), 210, "colliding key found");
    assert_eq!(state_u64(&state, 304), 1, "has a live key");
    assert_eq!(state_u64(&state, 320), 0xDEAD, "missing get left the value");
    assert_eq!(state_u64(&state, 328), 3, "the removed key is NotFound");
    assert_eq!(state_u64(&state, 336), 50);
    assert_eq!(state_u64(&state, 344), 210, "survivor after the shift");
    assert_eq!(state_u64(&state, 368), 2, "count after removal");
    assert_eq!(state_u64(&state, 0), 2, "the header count agrees");
}

#[test]
fn map_put_replaces_and_full_stops_growth() {
    let mut instructions = Vec::new();
    op_int_map_put(&mut instructions, 0, 5, 50);
    op_imm(&mut instructions, opcodes::STORE_STATE, 264);
    // Replace 5 in place.
    op_int_map_put(&mut instructions, 0, 5, 51);
    op_imm(&mut instructions, opcodes::STORE_STATE, 272);
    op_int_map_key(&mut instructions, opcodes::MAP_GET, 0, 5, 280);
    op_imm(&mut instructions, opcodes::STORE_STATE, 288);
    // Fill to the limit.
    op_int_map_put(&mut instructions, 0, 13, 130);
    op(&mut instructions, opcodes::DROP);
    op_int_map_put(&mut instructions, 0, 21, 210);
    op(&mut instructions, opcodes::DROP);
    // A fourth distinct key is Full and changes nothing.
    op_int_map_put(&mut instructions, 0, 29, 290);
    op_imm(&mut instructions, opcodes::STORE_STATE, 296);
    op_int_map_key(&mut instructions, opcodes::MAP_GET, 0, 29, 304);
    op_imm(&mut instructions, opcodes::STORE_STATE, 312);
    op_int_map_key(&mut instructions, opcodes::MAP_HAS, 0, 29, 0);
    op_imm(&mut instructions, opcodes::STORE_STATE, 320);
    op_imm(&mut instructions, opcodes::REF_STATE, 0);
    op(&mut instructions, opcodes::MAP_COUNT);
    op_imm(&mut instructions, opcodes::STORE_STATE, 328);
    // An existing key still replaces at the limit.
    op_int_map_put(&mut instructions, 0, 13, 131);
    op(&mut instructions, opcodes::DROP);
    op_int_map_key(&mut instructions, opcodes::MAP_GET, 0, 13, 336);
    op_imm(&mut instructions, opcodes::STORE_STATE, 344);
    op(&mut instructions, opcodes::RET);

    let mut state = vec![0u8; 512];
    state[304..312].copy_from_slice(&0xBEEFu64.to_le_bytes());
    let (outcome, _) = run_map(&instructions, &mut state, 512);
    assert_eq!(outcome, Outcome::Exited(ExitReason::Ok));
    assert_eq!(state_u64(&state, 264), 0);
    assert_eq!(state_u64(&state, 272), 0);
    assert_eq!(state_u64(&state, 280), 51, "value replaced in place");
    assert_eq!(state_u64(&state, 288), 0);
    assert_eq!(state_u64(&state, 296), 5, "Full is reported");
    assert_eq!(state_u64(&state, 304), 0xBEEF, "failed get left the value");
    assert_eq!(state_u64(&state, 312), 3, "NotFound");
    assert_eq!(state_u64(&state, 320), 0, "the new key was not inserted");
    assert_eq!(state_u64(&state, 328), 3, "count stayed at the limit");
    assert_eq!(state_u64(&state, 336), 131, "existing key replaced");
    assert_eq!(state_u64(&state, 344), 0);
}

#[test]
fn map_backward_shift_keeps_every_survivor_findable() {
    // 5, 13 and 21 home to slot 0, 1 and 2 when inserted in that order? No:
    // all three hash to slot 0, so they land in slots 0, 1 and 2. Removing 13
    // (slot 1) moves 21 back to slot 1 and clears slot 2.
    let mut instructions = Vec::new();
    for (key, value) in [(5, 50), (13, 130), (21, 210)] {
        op_int_map_put(&mut instructions, 0, key, value);
        op(&mut instructions, opcodes::DROP);
    }
    op_int_map_key(&mut instructions, opcodes::MAP_REMOVE, 0, 13, 0);
    op_imm(&mut instructions, opcodes::STORE_STATE, 264);
    // Capture the slot 1 key and the slot 1/2 occupancy now, before the later
    // removals empty the map again.
    op_imm(&mut instructions, opcodes::REF_STATE, 56);
    op(&mut instructions, opcodes::LOAD_REF);
    op_imm(&mut instructions, opcodes::STORE_STATE, 400);
    op_imm(&mut instructions, opcodes::REF_STATE, 40);
    op(&mut instructions, opcodes::LOAD_REF);
    op_imm(&mut instructions, opcodes::STORE_STATE, 408);
    op_imm(&mut instructions, opcodes::REF_STATE, 72);
    op(&mut instructions, opcodes::LOAD_REF);
    op_imm(&mut instructions, opcodes::STORE_STATE, 416);
    op_int_map_key(&mut instructions, opcodes::MAP_GET, 0, 21, 272);
    op_imm(&mut instructions, opcodes::STORE_STATE, 280);
    op_int_map_key(&mut instructions, opcodes::MAP_REMOVE, 0, 5, 0);
    op_imm(&mut instructions, opcodes::STORE_STATE, 288);
    op_int_map_key(&mut instructions, opcodes::MAP_GET, 0, 21, 296);
    op_imm(&mut instructions, opcodes::STORE_STATE, 304);
    op_int_map_key(&mut instructions, opcodes::MAP_REMOVE, 0, 21, 0);
    op_imm(&mut instructions, opcodes::STORE_STATE, 312);
    op_int_map_key(&mut instructions, opcodes::MAP_GET, 0, 5, 320);
    op_imm(&mut instructions, opcodes::STORE_STATE, 328);
    op(&mut instructions, opcodes::RET);

    let mut state = vec![0u8; 512];
    state[320..328].copy_from_slice(&0xCAFEu64.to_le_bytes());
    let (outcome, _) = run_map(&instructions, &mut state, 512);
    assert_eq!(outcome, Outcome::Exited(ExitReason::Ok));
    assert_eq!(state_u64(&state, 264), 0, "middle of the cluster removed");
    assert_eq!(state_u64(&state, 272), 210, "the next entry was found");
    assert_eq!(state_u64(&state, 280), 0);
    // Slot 1 is still occupied (21 moved back into it), slot 2 was cleared.
    assert_eq!(state_u64(&state, 400), 21, "21 sits in slot 1");
    assert_eq!(state_u64(&state, 408), 1, "slot 1 is occupied");
    assert_eq!(state_u64(&state, 416), 0, "slot 2 was cleared");
    assert_eq!(state_u64(&state, 288), 0);
    assert_eq!(
        state_u64(&state, 296),
        210,
        "still findable after the shift"
    );
    assert_eq!(state_u64(&state, 304), 0);
    assert_eq!(state_u64(&state, 312), 0, "last entry removed");
    assert_eq!(state_u64(&state, 320), 0xCAFE, "empty map left the value");
    assert_eq!(state_u64(&state, 328), 3, "NotFound");
    assert_eq!(state_u64(&state, 0), 0, "count back to zero");
}

#[test]
fn map_key_zero_is_an_ordinary_key() {
    let mut instructions = Vec::new();
    op_int_map_put(&mut instructions, 0, 0, 7);
    op_imm(&mut instructions, opcodes::STORE_STATE, 264);
    op_int_map_key(&mut instructions, opcodes::MAP_GET, 0, 0, 272);
    op_imm(&mut instructions, opcodes::STORE_STATE, 280);
    op_int_map_key(&mut instructions, opcodes::MAP_HAS, 0, 0, 0);
    op_imm(&mut instructions, opcodes::STORE_STATE, 288);
    op_int_map_key(&mut instructions, opcodes::MAP_HAS, 0, 1, 0);
    op_imm(&mut instructions, opcodes::STORE_STATE, 296);
    op_int_map_key(&mut instructions, opcodes::MAP_REMOVE, 0, 0, 0);
    op_imm(&mut instructions, opcodes::STORE_STATE, 304);
    op(&mut instructions, opcodes::RET);

    let mut state = vec![0u8; 512];
    let (outcome, _) = run_map(&instructions, &mut state, 512);
    assert_eq!(outcome, Outcome::Exited(ExitReason::Ok));
    assert_eq!(state_u64(&state, 264), 0);
    assert_eq!(state_u64(&state, 272), 7);
    assert_eq!(state_u64(&state, 288), 1);
    assert_eq!(state_u64(&state, 296), 0);
    assert_eq!(state_u64(&state, 304), 0);
    assert_eq!(state_u64(&state, 0), 0);
}

#[test]
fn map_buffer_keys_include_the_empty_key() {
    // map[string[8], int, 2]: capacity 4, limit 2, key_size 16, value_size 8,
    // stride 40, total 8 + 4*40 = 168. The constant blob holds an empty image
    // at 0..8 (data ref 8) and "ab" at 8..24 (data ref 16).
    let constants = {
        let mut blob = constant(b"");
        blob.extend_from_slice(&constant(b"ab"));
        blob
    };
    let mut instructions = Vec::new();
    let put = |instructions: &mut Vec<u8>, key_ref: u32, value: i64| {
        op_imm(instructions, opcodes::REF_STATE, 0);
        op_imm(instructions, opcodes::REF_CONST, key_ref);
        op_const(instructions, value);
        op_map(
            instructions,
            opcodes::MAP_PUT,
            opcodes::map_key_kind::STRING,
            opcodes::map_value_kind::SCALAR,
            4,
            2,
            16,
            8,
        );
    };
    put(&mut instructions, 8, 11);
    op_imm(&mut instructions, opcodes::STORE_STATE, 168);
    put(&mut instructions, 16, 22);
    op_imm(&mut instructions, opcodes::STORE_STATE, 176);
    // get "" -> 184 (empty key)
    op_imm(&mut instructions, opcodes::REF_STATE, 0);
    op_imm(&mut instructions, opcodes::REF_CONST, 8);
    op_imm(&mut instructions, opcodes::REF_STATE, 184);
    op_map(
        &mut instructions,
        opcodes::MAP_GET,
        opcodes::map_key_kind::STRING,
        opcodes::map_value_kind::SCALAR,
        4,
        2,
        16,
        8,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 192);
    // get "ab" -> 200
    op_imm(&mut instructions, opcodes::REF_STATE, 0);
    op_imm(&mut instructions, opcodes::REF_CONST, 16);
    op_imm(&mut instructions, opcodes::REF_STATE, 200);
    op_map(
        &mut instructions,
        opcodes::MAP_GET,
        opcodes::map_key_kind::STRING,
        opcodes::map_value_kind::SCALAR,
        4,
        2,
        16,
        8,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 208);
    // remove "" and look it up again
    op_imm(&mut instructions, opcodes::REF_STATE, 0);
    op_imm(&mut instructions, opcodes::REF_CONST, 8);
    op_map(
        &mut instructions,
        opcodes::MAP_REMOVE,
        opcodes::map_key_kind::STRING,
        opcodes::map_value_kind::SCALAR,
        4,
        2,
        16,
        8,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 216);
    op_imm(&mut instructions, opcodes::REF_STATE, 0);
    op_imm(&mut instructions, opcodes::REF_CONST, 8);
    op_imm(&mut instructions, opcodes::REF_STATE, 224);
    op_map(
        &mut instructions,
        opcodes::MAP_GET,
        opcodes::map_key_kind::STRING,
        opcodes::map_value_kind::SCALAR,
        4,
        2,
        16,
        8,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 232);
    op_imm(&mut instructions, opcodes::REF_STATE, 0);
    op(&mut instructions, opcodes::MAP_COUNT);
    op_imm(&mut instructions, opcodes::STORE_STATE, 240);
    op(&mut instructions, opcodes::RET);

    let mut state = vec![0u8; 512];
    state[224..232].copy_from_slice(&0xFEEDu64.to_le_bytes());
    let spec = FnSpec {
        frame_size: 128,
        ..MAIN
    };
    let package = build(&[spec], &instructions, &constants, 512, 128, 0);
    let mut scratch = scratch_for(&package);
    let program = Program::parse(&package, &mut scratch).expect("verifies");
    let mut stack = vec![0u8; 128];
    let args = vec![0u8; 8 + 1024];
    let mut machine = Machine::new();
    let mut host = Recorder { output: Vec::new() };
    let outcome = machine.run(&program, &mut host, &mut state, &mut stack, &args, 10_000);
    assert_eq!(outcome, Outcome::Exited(ExitReason::Ok));
    assert_eq!(state_u64(&state, 168), 0);
    assert_eq!(state_u64(&state, 176), 0);
    assert_eq!(state_u64(&state, 184), 11, "empty key found");
    assert_eq!(state_u64(&state, 192), 0);
    assert_eq!(state_u64(&state, 200), 22);
    assert_eq!(state_u64(&state, 208), 0);
    assert_eq!(state_u64(&state, 216), 0);
    assert_eq!(state_u64(&state, 224), 0xFEED, "removed key left the value");
    assert_eq!(state_u64(&state, 232), 3, "NotFound");
    assert_eq!(state_u64(&state, 240), 1);
}

#[test]
fn map_aggregate_and_buffer_values_are_copied_whole() {
    let mut instructions = Vec::new();
    // A fixed 16-byte value image at state 168 is copied into the slot.
    for (at, word) in [(168u32, 0x1111u64), (176, 0x2222)] {
        op_const(&mut instructions, word as i64);
        op_imm(&mut instructions, opcodes::STORE_STATE, at);
    }
    op_imm(&mut instructions, opcodes::REF_STATE, 0);
    op_const(&mut instructions, 7);
    op_imm(&mut instructions, opcodes::REF_STATE, 168);
    op_map(
        &mut instructions,
        opcodes::MAP_PUT,
        opcodes::map_key_kind::INT,
        opcodes::map_value_kind::FIXED,
        4,
        2,
        8,
        16,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 200);
    op_imm(&mut instructions, opcodes::REF_STATE, 0);
    op_const(&mut instructions, 7);
    op_imm(&mut instructions, opcodes::REF_STATE, 184);
    op_map(
        &mut instructions,
        opcodes::MAP_GET,
        opcodes::map_key_kind::INT,
        opcodes::map_value_kind::FIXED,
        4,
        2,
        8,
        16,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 208);
    // A buffer value: map at 216 holds [len][data] in a 16-byte image.
    op_imm(&mut instructions, opcodes::REF_STATE, 216);
    op_const(&mut instructions, 1);
    op_imm(&mut instructions, opcodes::REF_CONST, 8);
    op_map(
        &mut instructions,
        opcodes::MAP_PUT,
        opcodes::map_key_kind::INT,
        opcodes::map_value_kind::BUFFER,
        4,
        2,
        8,
        16,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 392);
    op_imm(&mut instructions, opcodes::REF_STATE, 216);
    op_const(&mut instructions, 1);
    op_imm(&mut instructions, opcodes::REF_STATE, 424);
    op_map(
        &mut instructions,
        opcodes::MAP_GET,
        opcodes::map_key_kind::INT,
        opcodes::map_value_kind::BUFFER,
        4,
        2,
        8,
        16,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 400);
    op(&mut instructions, opcodes::RET);

    let constants = constant(b"hi");
    let mut state = vec![0u8; 512];
    let spec = FnSpec {
        frame_size: 160,
        ..MAIN
    };
    let package = build(&[spec], &instructions, &constants, 512, 160, 0);
    let mut scratch = scratch_for(&package);
    let program = Program::parse(&package, &mut scratch).expect("verifies");
    let mut stack = vec![0u8; 160];
    let args = vec![0u8; 8 + 1024];
    let mut machine = Machine::new();
    let mut host = Recorder { output: Vec::new() };
    let outcome = machine.run(&program, &mut host, &mut state, &mut stack, &args, 10_000);
    assert_eq!(outcome, Outcome::Exited(ExitReason::Ok));
    assert_eq!(state_u64(&state, 200), 0);
    assert_eq!(state_u64(&state, 208), 0);
    assert_eq!(state_u64(&state, 184), 0x1111);
    assert_eq!(state_u64(&state, 192), 0x2222);
    assert_eq!(state_u64(&state, 392), 0);
    assert_eq!(state_u64(&state, 400), 0);
    assert_eq!(state_u64(&state, 416), 2, "copied buffer length");
    assert_eq!(&state[424..426], b"hi", "copied buffer data");
}

#[test]
fn map_whole_copy_duplicates_header_and_slots() {
    // map[int, int, 2] is 8 + 4*32 = 136 bytes; A at 0, B at 144.
    let mut instructions = Vec::new();
    for (key, value) in [(5, 50), (13, 130)] {
        op_imm(&mut instructions, opcodes::REF_STATE, 0);
        op_const(&mut instructions, key);
        op_const(&mut instructions, value);
        op_map(
            &mut instructions,
            opcodes::MAP_PUT,
            opcodes::map_key_kind::INT,
            opcodes::map_value_kind::SCALAR,
            4,
            2,
            8,
            8,
        );
        op(&mut instructions, opcodes::DROP);
    }
    // B := A (whole image, header and slots).
    op_imm(&mut instructions, opcodes::REF_STATE, 144);
    op_imm(&mut instructions, opcodes::REF_STATE, 0);
    op_const(&mut instructions, 136);
    op(&mut instructions, opcodes::COPY_REFS);
    // remove 5 from B only
    op_imm(&mut instructions, opcodes::REF_STATE, 144);
    op_const(&mut instructions, 5);
    op_map(
        &mut instructions,
        opcodes::MAP_REMOVE,
        opcodes::map_key_kind::INT,
        opcodes::map_value_kind::SCALAR,
        4,
        2,
        8,
        8,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 288);
    // A still has 5; B does not.
    op_imm(&mut instructions, opcodes::REF_STATE, 0);
    op_const(&mut instructions, 5);
    op_imm(&mut instructions, opcodes::REF_STATE, 296);
    op_map(
        &mut instructions,
        opcodes::MAP_GET,
        opcodes::map_key_kind::INT,
        opcodes::map_value_kind::SCALAR,
        4,
        2,
        8,
        8,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 304);
    op_imm(&mut instructions, opcodes::REF_STATE, 144);
    op_const(&mut instructions, 5);
    op_map(
        &mut instructions,
        opcodes::MAP_HAS,
        opcodes::map_key_kind::INT,
        opcodes::map_value_kind::SCALAR,
        4,
        2,
        8,
        8,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 312);
    op_imm(&mut instructions, opcodes::REF_STATE, 0);
    op(&mut instructions, opcodes::MAP_COUNT);
    op_imm(&mut instructions, opcodes::STORE_STATE, 320);
    op_imm(&mut instructions, opcodes::REF_STATE, 144);
    op(&mut instructions, opcodes::MAP_COUNT);
    op_imm(&mut instructions, opcodes::STORE_STATE, 328);
    op(&mut instructions, opcodes::RET);

    let mut state = vec![0u8; 512];
    let (outcome, _) = run_map(&instructions, &mut state, 512);
    assert_eq!(outcome, Outcome::Exited(ExitReason::Ok));
    assert_eq!(state_u64(&state, 288), 0);
    assert_eq!(state_u64(&state, 296), 50, "the original still has 5");
    assert_eq!(state_u64(&state, 304), 0);
    assert_eq!(state_u64(&state, 312), 0, "the copy lost 5");
    assert_eq!(state_u64(&state, 320), 2);
    assert_eq!(state_u64(&state, 328), 1);
}

#[test]
fn map_operations_bounds_check_the_image() {
    // The map image is 264 bytes but the state region is only 128: the first
    // operation must fault before it touches anything.
    let mut instructions = Vec::new();
    op_int_map_put(&mut instructions, 0, 5, 50);
    op(&mut instructions, opcodes::RET);
    assert_eq!(map_fault(&instructions, 128), FaultCode::OutOfBounds);
}

#[test]
fn the_verifier_rejects_bad_map_immediates() {
    let invalid = [
        (0u8, 0u8, 3u32, 1u32, 8u32, 8u32),
        (0, 0, 1, 1, 8, 8),
        (0, 0, 8, 0, 8, 8),
        (0, 0, 8, 5, 8, 8),
        (0, 0, 8, 3, 4, 8),
        (0, 0, 8, 3, 8, 4),
        (3, 0, 8, 3, 8, 8),
        (0, 3, 8, 3, 8, 8),
        (0, 0, 1 << 20, 1, 8, 8),
    ];
    for (key_kind, value_kind, capacity, limit, key_size, value_size) in invalid {
        let mut instructions = Vec::new();
        op_imm(&mut instructions, opcodes::REF_STATE, 0);
        op_const(&mut instructions, 0);
        op_map(
            &mut instructions,
            opcodes::MAP_HAS,
            key_kind,
            value_kind,
            capacity,
            limit,
            key_size,
            value_size,
        );
        op(&mut instructions, opcodes::DROP);
        op(&mut instructions, opcodes::RET);
        let spec = FnSpec {
            frame_size: 64,
            ..MAIN
        };
        let code = build(&[spec], &instructions, &[], 64, 64, 0);
        let mut scratch = scratch_for(&code);
        let error = Program::parse(&code, &mut scratch).expect_err("must be rejected");
        assert!(
            error.message.contains("map"),
            "{key_kind}/{value_kind}/{capacity}/{limit}/{key_size}/{value_size}: {}",
            error.message
        );
    }
}

// ---- realtime promotion and heartbeat -------------------------------------

/// `state[0..8]` receives the `Status` pushed by `realtime_on`; `heartbeat`
/// takes no operand and pushes none, so it is a bare statement between the two.
fn realtime_package() -> Vec<u8> {
    let mut instructions = Vec::new();
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::REALTIME_ON,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 0);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::HEARTBEAT,
    );
    op(&mut instructions, opcodes::RET);
    let spec = FnSpec {
        frame_size: 48,
        ..MAIN
    };
    build(&[spec], &instructions, &[], 8, 64, 0)
}

#[test]
fn realtime_on_pushes_the_host_status_and_heartbeat_is_a_no_op_result() {
    let code = realtime_package();
    let mut host = MockHost::default();
    let mut state = vec![0u8; 8];
    let mut stack = vec![0u8; 64];
    let (outcome, _) = run_with_host(&code, &mut state, &mut stack, &mut host, 100);
    assert_eq!(outcome, Outcome::Exited(ExitReason::Ok));
    assert_eq!(u64::from_le_bytes(state[..8].try_into().unwrap()), 0);
    assert_eq!(host.realtime_calls, 1);
    assert_eq!(host.heartbeat_calls, 1);
}

#[test]
fn realtime_on_reports_full_without_changing_the_heartbeat_path() {
    let code = realtime_package();
    let mut host = MockHost {
        realtime_status: Status::Full as u64,
        ..Default::default()
    };
    let mut state = vec![0u8; 8];
    let mut stack = vec![0u8; 64];
    let (outcome, _) = run_with_host(&code, &mut state, &mut stack, &mut host, 100);
    assert_eq!(outcome, Outcome::Exited(ExitReason::Ok));
    assert_eq!(
        u64::from_le_bytes(state[..8].try_into().unwrap()),
        Status::Full as u64
    );
    assert_eq!(host.realtime_calls, 1);
    assert_eq!(host.heartbeat_calls, 1);
}

// ---- syslib graphics (spec 0009) ------------------------------------------

fn store_at(out: &mut Vec<u8>, offset: u32) {
    op_imm(out, opcodes::STORE_STATE, offset);
}

#[test]
fn graphics_primitives_route_through_the_host_in_declaration_order() {
    let mut instructions = Vec::new();
    op_const(&mut instructions, 10); // x
    op_const(&mut instructions, 20); // y
    op_const(&mut instructions, 30); // w
    op_const(&mut instructions, 40); // h
    op_const(&mut instructions, 0x12_3456); // color
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::FILL_RECT,
    );
    store_at(&mut instructions, 0);

    op_const(&mut instructions, 1);
    op_const(&mut instructions, 2);
    op_const(&mut instructions, 3);
    op_const(&mut instructions, 4);
    op_const(&mut instructions, 0x00_00FF);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::FILL_DITHER,
    );
    store_at(&mut instructions, 8);

    op_const(&mut instructions, 5);
    op_const(&mut instructions, 6);
    op_const(&mut instructions, 7);
    op_const(&mut instructions, 8);
    op_const(&mut instructions, 9);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::DRAW_RECT,
    );
    store_at(&mut instructions, 16);

    op_const(&mut instructions, 1);
    op_const(&mut instructions, 2);
    op_imm(&mut instructions, opcodes::REF_CONST, 8);
    op_const(&mut instructions, 0x00_FF00);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::DRAW_TEXT,
    );
    store_at(&mut instructions, 24);

    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::SCREEN_WIDTH,
    );
    store_at(&mut instructions, 32);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::SCREEN_HEIGHT,
    );
    store_at(&mut instructions, 40);

    op_const(&mut instructions, 7);
    op_const(&mut instructions, 8);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::GET_PIXEL,
    );
    store_at(&mut instructions, 48);

    op_const(&mut instructions, 0);
    op_const(&mut instructions, 0);
    op_const(&mut instructions, 1024);
    op_const(&mut instructions, 768);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::PRESENT,
    );
    store_at(&mut instructions, 56);

    op_const(&mut instructions, 0x00_FFFF);
    op_imm(&mut instructions, opcodes::BUILTIN, opcodes::builtin::CLEAR);
    store_at(&mut instructions, 64);
    op(&mut instructions, opcodes::RET);

    let constants = constant(b"Hi");
    let spec = FnSpec {
        frame_size: 128,
        operand_off: 64,
        ..MAIN
    };
    let code = build(&[spec], &instructions, &constants, 72, 128, 0);

    let mut host = MockHost {
        screen_width_value: 1024,
        screen_height_value: 768,
        gfx_ok: true,
        pixel_value: 0x12_3456,
        ..Default::default()
    };
    let mut state = vec![0u8; 72];
    let mut stack = vec![0u8; 128];
    let (outcome, _) = run_with_host(&code, &mut state, &mut stack, &mut host, 500);
    assert_eq!(outcome, Outcome::Exited(ExitReason::Ok));

    assert_eq!(host.rects, vec![(10, 20, 30, 40, 0x12_3456)]);
    assert_eq!(host.dithers, vec![(1, 2, 3, 4, 0x0000_00FF)]);
    assert_eq!(host.outlines, vec![(5, 6, 7, 8, 9)]);
    assert_eq!(host.texts, vec![(1, 2, b"Hi".to_vec(), 0x00_FF00)]);
    assert_eq!(host.pixels, vec![(7, 8)]);
    assert_eq!(host.presents, vec![(0, 0, 1024, 768)]);
    assert_eq!(host.clears, vec![0x00_FFFF]);

    let word = |offset: usize| u64::from_le_bytes(state[offset..offset + 8].try_into().unwrap());
    for offset in [0, 8, 16, 24, 56, 64] {
        assert_eq!(word(offset), Status::Ok as u64, "status at {offset}");
    }
    assert_eq!(word(32), 1024);
    assert_eq!(word(40), 768);
    assert_eq!(word(48), 0x12_3456);
}

#[test]
fn graphics_reject_non_positive_extents_and_over_wide_colours_without_calling_the_host() {
    let mut instructions = Vec::new();
    // w = 0
    for value in [1, 2, 0, 4] {
        op_const(&mut instructions, value);
    }
    op_const(&mut instructions, 0);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::FILL_RECT,
    );
    store_at(&mut instructions, 0);
    // h = -1
    for value in [1, 2, 3, -1] {
        op_const(&mut instructions, value);
    }
    op_const(&mut instructions, 0);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::FILL_DITHER,
    );
    store_at(&mut instructions, 8);
    // A colour above 0x00FF_FFFF.
    for value in [1, 2, 3, 4] {
        op_const(&mut instructions, value);
    }
    op_const(&mut instructions, 0x0100_0000);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::DRAW_RECT,
    );
    store_at(&mut instructions, 16);
    // A present with h = 0.
    op_const(&mut instructions, 0);
    op_const(&mut instructions, 0);
    op_const(&mut instructions, 100);
    op_const(&mut instructions, 0);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::PRESENT,
    );
    store_at(&mut instructions, 24);
    // clear with an over-wide colour.
    op_const(&mut instructions, 0xFFFF_FFFF);
    op_imm(&mut instructions, opcodes::BUILTIN, opcodes::builtin::CLEAR);
    store_at(&mut instructions, 32);
    op(&mut instructions, opcodes::RET);

    let spec = FnSpec {
        frame_size: 96,
        operand_off: 56,
        ..MAIN
    };
    let code = build(&[spec], &instructions, &[], 40, 96, 0);
    let mut host = MockHost {
        gfx_ok: true,
        ..Default::default()
    };
    let mut state = vec![0u8; 40];
    let mut stack = vec![0u8; 96];
    let (outcome, _) = run_with_host(&code, &mut state, &mut stack, &mut host, 500);
    assert_eq!(outcome, Outcome::Exited(ExitReason::Ok));

    for offset in [0, 8, 16, 24, 32] {
        let status = u64::from_le_bytes(state[offset..offset + 8].try_into().unwrap());
        assert_eq!(status, Status::Invalid as u64, "status at {offset}");
    }
    assert!(host.rects.is_empty());
    assert!(host.dithers.is_empty());
    assert!(host.outlines.is_empty());
    assert!(host.presents.is_empty());
    assert!(host.clears.is_empty());
}

#[test]
fn graphics_defaults_describe_a_disabled_screen() {
    // No graphics overrides: the trait defaults are a 0x0 screen, `Invalid`
    // for drawing and 0 for `get_pixel` (spec 0009, G15/G16).
    let mut instructions = Vec::new();
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::SCREEN_WIDTH,
    );
    store_at(&mut instructions, 0);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::SCREEN_HEIGHT,
    );
    store_at(&mut instructions, 8);
    for value in [0, 0, 10, 10] {
        op_const(&mut instructions, value);
    }
    op_const(&mut instructions, 0x00_0000);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::FILL_RECT,
    );
    store_at(&mut instructions, 16);
    op_const(&mut instructions, 4);
    op_const(&mut instructions, 4);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::GET_PIXEL,
    );
    store_at(&mut instructions, 24);
    op(&mut instructions, opcodes::RET);

    let spec = FnSpec {
        frame_size: 96,
        operand_off: 56,
        ..MAIN
    };
    let code = build(&[spec], &instructions, &[], 32, 96, 0);
    let mut host = MockHost::default();
    let mut state = vec![0u8; 32];
    let mut stack = vec![0u8; 96];
    let (outcome, _) = run_with_host(&code, &mut state, &mut stack, &mut host, 100);
    assert_eq!(outcome, Outcome::Exited(ExitReason::Ok));

    let word = |offset: usize| u64::from_le_bytes(state[offset..offset + 8].try_into().unwrap());
    assert_eq!(word(0), 0);
    assert_eq!(word(8), 0);
    assert_eq!(word(16), Status::Invalid as u64);
    assert_eq!(word(24), 0);
}

#[test]
fn proportional_font_builtins_route_through_the_host_in_declaration_order() {
    let mut instructions = Vec::new();
    op_imm(&mut instructions, opcodes::REF_CONST, 8);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::TEXT_WIDTH_PROP,
    );
    store_at(&mut instructions, 0);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::TEXT_HEIGHT_PROP,
    );
    store_at(&mut instructions, 8);
    op_const(&mut instructions, 3);
    op_const(&mut instructions, 4);
    op_imm(&mut instructions, opcodes::REF_CONST, 8);
    op_const(&mut instructions, 0x10_2030);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::DRAW_TEXT_PROP,
    );
    store_at(&mut instructions, 16);
    op(&mut instructions, opcodes::RET);

    let constants = constant(b"ii");
    let spec = FnSpec {
        frame_size: 96,
        operand_off: 56,
        ..MAIN
    };
    let code = build(&[spec], &instructions, &constants, 24, 96, 0);
    let mut host = MockHost {
        gfx_ok: true,
        prop_width_value: 6,
        prop_height_value: 12,
        ..Default::default()
    };
    let mut state = vec![0u8; 24];
    let mut stack = vec![0u8; 96];
    let (outcome, _) = run_with_host(&code, &mut state, &mut stack, &mut host, 500);
    assert_eq!(outcome, Outcome::Exited(ExitReason::Ok));

    let word = |offset: usize| u64::from_le_bytes(state[offset..offset + 8].try_into().unwrap());
    assert_eq!(word(0), 6);
    assert_eq!(word(8), 12);
    assert_eq!(word(16), Status::Ok as u64);
    assert_eq!(host.prop_widths, vec![b"ii".to_vec()]);
    assert_eq!(host.prop_texts, vec![(3, 4, b"ii".to_vec(), 0x10_2030)]);
}

#[test]
fn proportional_font_defaults_and_over_wide_colours_do_not_reach_the_host() {
    // The trait defaults are a disabled screen: 0 metrics and `Invalid` drawing
    // (spec 0015). An over-wide colour is rejected before the host is called.
    let mut instructions = Vec::new();
    op_imm(&mut instructions, opcodes::REF_CONST, 8);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::TEXT_WIDTH_PROP,
    );
    store_at(&mut instructions, 0);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::TEXT_HEIGHT_PROP,
    );
    store_at(&mut instructions, 8);
    op_const(&mut instructions, 0);
    op_const(&mut instructions, 0);
    op_imm(&mut instructions, opcodes::REF_CONST, 8);
    op_const(&mut instructions, 0x0100_0000);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::DRAW_TEXT_PROP,
    );
    store_at(&mut instructions, 16);
    op(&mut instructions, opcodes::RET);

    let constants = constant(b"x");
    let spec = FnSpec {
        frame_size: 96,
        operand_off: 56,
        ..MAIN
    };
    let code = build(&[spec], &instructions, &constants, 24, 96, 0);
    let mut host = MockHost::default();
    let mut state = vec![0u8; 24];
    let mut stack = vec![0u8; 96];
    let (outcome, _) = run_with_host(&code, &mut state, &mut stack, &mut host, 500);
    assert_eq!(outcome, Outcome::Exited(ExitReason::Ok));

    let word = |offset: usize| u64::from_le_bytes(state[offset..offset + 8].try_into().unwrap());
    assert_eq!(word(0), 0);
    assert_eq!(word(8), 0);
    assert_eq!(word(16), Status::Invalid as u64);
    assert_eq!(host.prop_widths, vec![b"x".to_vec()]);
    assert!(host.prop_texts.is_empty());
}

#[test]
fn pointer_builtins_read_the_pointer_and_injection_rejects_wide_masks() {
    let mut instructions = Vec::new();
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::POINTER_X,
    );
    store_at(&mut instructions, 0);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::POINTER_Y,
    );
    store_at(&mut instructions, 8);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::POINTER_BUTTONS,
    );
    store_at(&mut instructions, 16);
    // A valid injection: dx, dy, buttons, in declaration order.
    op_const(&mut instructions, 10);
    op_const(&mut instructions, -20);
    op_const(&mut instructions, 5);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::INJECT_POINTER,
    );
    store_at(&mut instructions, 24);
    // buttons = 8 is above 0b111: `Invalid` and the host is not called.
    op_const(&mut instructions, 0);
    op_const(&mut instructions, 0);
    op_const(&mut instructions, 8);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::INJECT_POINTER,
    );
    store_at(&mut instructions, 32);
    // The source is a read-only machine value (0 none, 1 PS/2, 2 tablet).
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::POINTER_SOURCE,
    );
    store_at(&mut instructions, 40);
    op(&mut instructions, opcodes::RET);

    let spec = FnSpec {
        frame_size: 96,
        operand_off: 56,
        ..MAIN
    };
    let code = build(&[spec], &instructions, &[], 48, 96, 0);
    let mut host = MockHost {
        pointer_x_value: 640,
        pointer_y_value: 480,
        pointer_buttons_value: 5,
        pointer_source_value: 2,
        gfx_ok: true,
        ..Default::default()
    };
    let mut state = vec![0u8; 48];
    let mut stack = vec![0u8; 96];
    let (outcome, _) = run_with_host(&code, &mut state, &mut stack, &mut host, 500);
    assert_eq!(outcome, Outcome::Exited(ExitReason::Ok));

    let word = |offset: usize| u64::from_le_bytes(state[offset..offset + 8].try_into().unwrap());
    assert_eq!(word(0), 640);
    assert_eq!(word(8), 480);
    assert_eq!(word(16), 5);
    assert_eq!(word(24), Status::Ok as u64);
    assert_eq!(word(32), Status::Invalid as u64);
    assert_eq!(word(40), 2);
    assert_eq!(host.injections, vec![(10, -20, 5)]);
}

#[test]
fn pointer_defaults_describe_no_device() {
    // No overrides: the pointer is at (0, 0), no button is down and injection
    // is refused, which is also the kernel's disabled-graphics state
    // (spec 0010, I11).
    let mut instructions = Vec::new();
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::POINTER_X,
    );
    store_at(&mut instructions, 0);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::POINTER_Y,
    );
    store_at(&mut instructions, 8);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::POINTER_BUTTONS,
    );
    store_at(&mut instructions, 16);
    op_const(&mut instructions, 1);
    op_const(&mut instructions, 1);
    op_const(&mut instructions, 1);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::INJECT_POINTER,
    );
    store_at(&mut instructions, 24);
    op(&mut instructions, opcodes::RET);

    let spec = FnSpec {
        frame_size: 96,
        operand_off: 56,
        ..MAIN
    };
    let code = build(&[spec], &instructions, &[], 32, 96, 0);
    let mut host = MockHost::default();
    let mut state = vec![0u8; 32];
    let mut stack = vec![0u8; 96];
    let (outcome, _) = run_with_host(&code, &mut state, &mut stack, &mut host, 100);
    assert_eq!(outcome, Outcome::Exited(ExitReason::Ok));

    let word = |offset: usize| u64::from_le_bytes(state[offset..offset + 8].try_into().unwrap());
    assert_eq!(word(0), 0);
    assert_eq!(word(8), 0);
    assert_eq!(word(16), 0);
    assert_eq!(word(24), Status::Invalid as u64);
}

// ---- event queue (spec 0010, I19) -----------------------------------------

#[test]
fn event_builtins_poll_inject_count_drops_and_flush() {
    let mut instructions = Vec::new();
    // poll_event into slot 0 while the queue is empty.
    op_const(
        &mut instructions,
        refs::encode(refs::REGION_STATE, 0) as i64,
    );
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::POLL_EVENT,
    );
    store_at(&mut instructions, 96);
    // inject_event from the record at 48.
    op_const(
        &mut instructions,
        refs::encode(refs::REGION_STATE, 48) as i64,
    );
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::INJECT_EVENT,
    );
    store_at(&mut instructions, 104);
    // poll_event now returns the injected record.
    op_const(
        &mut instructions,
        refs::encode(refs::REGION_STATE, 0) as i64,
    );
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::POLL_EVENT,
    );
    store_at(&mut instructions, 112);
    // Inject a second event, then flush it and flush the empty queue again.
    op_const(
        &mut instructions,
        refs::encode(refs::REGION_STATE, 48) as i64,
    );
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::INJECT_EVENT,
    );
    store_at(&mut instructions, 120);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::EVENT_FLUSH,
    );
    store_at(&mut instructions, 128);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::EVENT_FLUSH,
    );
    store_at(&mut instructions, 136);
    // The drop counter is a host value.
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::EVENT_DROPS,
    );
    store_at(&mut instructions, 144);
    op(&mut instructions, opcodes::RET);

    let spec = FnSpec {
        frame_size: 160,
        operand_off: 40,
        ..MAIN
    };
    let code = build(&[spec], &instructions, &[], 152, 160, 0);

    let event = abi::Event {
        kind: abi::EventKind::PointerMove,
        x: 12,
        y: 34,
        code: 0b101,
        window: 0,
    };
    let mut state = vec![0u8; 152];
    let mut record = [0u8; abi::layout::event::SIZE as usize];
    abi::layout::event::write(&mut record, event);
    state[48..88].copy_from_slice(&record);
    // A sentinel proves an empty poll leaves the destination untouched.
    state[0..40].fill(0xAA);

    let mut host = MockHost {
        event_drops_value: 7,
        ..Default::default()
    };
    let mut stack = vec![0u8; 160];
    let (outcome, _) = run_with_host(&code, &mut state, &mut stack, &mut host, 300);
    assert_eq!(outcome, Outcome::Exited(ExitReason::Ok));

    let word = |offset: usize| u64::from_le_bytes(state[offset..offset + 8].try_into().unwrap());
    assert_eq!(word(96), Status::WouldBlock as u64);
    assert_eq!(word(104), Status::Ok as u64);
    assert_eq!(word(112), Status::Ok as u64);
    assert_eq!(&state[0..40], &record);
    assert_eq!(word(120), Status::Ok as u64);
    assert_eq!(word(128), 1);
    assert_eq!(word(136), 0);
    assert_eq!(word(144), 7);
    assert_eq!(
        host.event_injections,
        vec![record.to_vec(), record.to_vec()]
    );
}

#[test]
fn next_event_blocks_on_input_and_retries_after_a_wake() {
    let mut instructions = Vec::new();
    op_const(
        &mut instructions,
        refs::encode(refs::REGION_STATE, 0) as i64,
    );
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::NEXT_EVENT,
    );
    store_at(&mut instructions, 40);
    op(&mut instructions, opcodes::RET);

    let spec = FnSpec {
        frame_size: 56,
        operand_off: 40,
        ..MAIN
    };
    let code = build(&[spec], &instructions, &[], 48, 64, 0);
    let mut state = vec![0u8; 48];
    let mut stack = vec![0u8; 64];
    let mut host = MockHost::default();

    let (outcome, mut machine) = run_with_host(&code, &mut state, &mut stack, &mut host, 100);
    assert_eq!(outcome, Outcome::Blocked(WaitKind::Input));

    let event = abi::Event {
        kind: abi::EventKind::KeyDown,
        x: 0,
        y: 0,
        code: abi::key::pack(u16::from(b'Z'), b'Z', abi::key::MOD_CTRL),
        window: 0,
    };
    let mut record = [0u8; abi::layout::event::SIZE as usize];
    abi::layout::event::write(&mut record, event);
    host.events.push_back(record.to_vec());

    let outcome = resume_with_host(&code, &mut state, &mut stack, &mut host, 100, &mut machine);
    assert_eq!(outcome, Outcome::Exited(ExitReason::Ok));
    assert_eq!(&state[0..40], &record);
    assert_eq!(
        u64::from_le_bytes(state[40..48].try_into().unwrap()),
        Status::Ok as u64
    );
}

#[test]
fn event_defaults_refuse_an_injection_and_report_an_empty_queue() {
    // No overrides: the trait defaults model a machine with no event queue.
    let mut host = Recorder { output: Vec::new() };
    let mut event = [0u8; 40];
    assert_eq!(host.poll_event(&mut event), Status::WouldBlock);
    assert_eq!(host.inject_event(&event), Status::Invalid);
    assert_eq!(host.event_drops(), 0);
    assert_eq!(host.event_flush(), 0);
}
