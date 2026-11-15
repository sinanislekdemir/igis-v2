//! Stream opcode tests with a mock host FIFO (spec 0006).
//!
//! The interpreter does not own streams: it validates references, calls the
//! host, and turns `WouldBlock` into a block for the blocking opcodes. These
//! tests pin that contract without the kernel.

use abi::opcodes;
use abi::{
    CodeHeader, Function, Status, ABI_VERSION, CODE_MAGIC, NO_DIAGNOSTIC, NO_NAME,
    PROFILE_X86_64_LE,
};
use interpreter::{Host, Machine, Outcome, StreamError, WaitKind};

const HANDLE: u64 = 0x1234_5678_9ABC_0007;

fn build(instructions: &[u8], constants: &[u8], state_size: u32) -> Vec<u8> {
    let header = CodeHeader {
        magic: CODE_MAGIC,
        abi_version: ABI_VERSION,
        profile: PROFILE_X86_64_LE,
        function_count: 1,
        entry: 0,
        code_len: instructions.len() as u32,
        const_len: constants.len() as u32,
        line_len: 0,
        state_size,
        stack_size: 128,
        diag_offset: NO_DIAGNOSTIC,
    };
    let mut code = Vec::new();
    code.extend_from_slice(&header.to_bytes());
    code.extend_from_slice(
        &Function {
            code_off: 0,
            frame_size: 80,
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

fn constant(data: &[u8]) -> Vec<u8> {
    let mut out = (data.len() as u64).to_le_bytes().to_vec();
    out.extend_from_slice(data);
    while !out.len().is_multiple_of(8) {
        out.push(0);
    }
    out
}

struct StreamHost {
    queue: Vec<u8>,
    capacity: usize,
    closed: bool,
    output: Vec<u8>,
}

impl StreamHost {
    fn new(capacity: usize) -> Self {
        StreamHost {
            queue: Vec::new(),
            capacity,
            closed: false,
            output: Vec::new(),
        }
    }
}

impl Host for StreamHost {
    fn console_write(&mut self, bytes: &[u8]) {
        self.output.extend_from_slice(bytes);
    }

    fn clock(&mut self) -> u64 {
        0
    }

    fn stream_send(&mut self, _handle: u64, bytes: &[u8]) -> Result<(), StreamError> {
        if self.closed {
            return Err(StreamError::Closed);
        }
        if bytes.len() > self.capacity {
            return Err(StreamError::CapacityExceeded);
        }
        if self.queue.len() + bytes.len() > self.capacity {
            return Err(StreamError::WouldBlock);
        }
        self.queue.extend_from_slice(bytes);
        Ok(())
    }

    fn stream_recv(&mut self, _handle: u64, dst: &mut [u8]) -> Result<usize, StreamError> {
        if self.queue.is_empty() {
            return if self.closed {
                Err(StreamError::Closed)
            } else {
                Err(StreamError::WouldBlock)
            };
        }
        let copied = self.queue.len().min(dst.len());
        dst[..copied].copy_from_slice(&self.queue[..copied]);
        self.queue.drain(..copied);
        Ok(copied)
    }
}

/// Builds `[handle][settings]` state with a destination buffer at 8 and a
/// result word at 40.
fn initial_state() -> Vec<u8> {
    let mut state = vec![0u8; 64];
    state[..8].copy_from_slice(&HANDLE.to_le_bytes());
    state
}

fn run(code: &[u8], state: &mut [u8], host: &mut StreamHost) -> (Outcome, Machine) {
    let mut scratch = vec![0u8; code.len().div_ceil(32)];
    let program = interpreter::Program::parse(code, &mut scratch).expect("package verifies");
    let mut stack = vec![0u8; 128];
    let args = vec![0u8; 8 + 1024];
    let mut machine = Machine::new();
    let outcome = machine.run(&program, host, state, &mut stack, &args, 10_000);
    (outcome, machine)
}

#[test]
fn send_then_receive_round_trips_and_reports_the_count() {
    let data = b"hello stream";
    let mut instructions = Vec::new();
    op_imm(&mut instructions, opcodes::LOAD_STATE, 0);
    op_imm(&mut instructions, opcodes::REF_CONST, 8);
    op(&mut instructions, opcodes::STREAM_SEND);
    op(&mut instructions, opcodes::DROP);
    op_imm(&mut instructions, opcodes::LOAD_STATE, 0);
    op_imm(&mut instructions, opcodes::REF_STATE, 16);
    op_imm(&mut instructions, opcodes::STREAM_RECV, 16);
    op_imm(&mut instructions, opcodes::STORE_STATE, 40);
    op_imm(&mut instructions, opcodes::REF_STATE, 16);
    op_imm(&mut instructions, opcodes::BUILTIN, opcodes::builtin::WRITE);
    op(&mut instructions, opcodes::RET);

    let code = build(&instructions, &constant(data), 64);
    let mut state = initial_state();
    let mut host = StreamHost::new(16);
    let (outcome, _) = run(&code, &mut state, &mut host);
    assert_eq!(outcome, Outcome::Exited(interpreter::ExitReason::Ok));
    assert_eq!(host.output, data);
    assert_eq!(
        u64::from_le_bytes(state[40..48].try_into().unwrap()),
        data.len() as u64
    );
}

#[test]
fn a_full_queue_blocks_the_blocking_send_and_would_blocks_the_try() {
    let data = b"abc";
    let mut instructions = Vec::new();
    op_imm(&mut instructions, opcodes::LOAD_STATE, 0);
    op_imm(&mut instructions, opcodes::REF_CONST, 8);
    op(&mut instructions, opcodes::STREAM_SEND);
    op(&mut instructions, opcodes::RET);
    let code = build(&instructions, &constant(data), 64);
    let mut state = initial_state();
    let mut host = StreamHost::new(3);
    host.queue.extend_from_slice(data);
    let (outcome, machine) = run(&code, &mut state, &mut host);
    assert_eq!(outcome, Outcome::Blocked(WaitKind::Stream(HANDLE)));
    assert_eq!(
        machine.pc(),
        16,
        "the send instruction is retried unchanged"
    );

    let mut instructions = Vec::new();
    op_imm(&mut instructions, opcodes::LOAD_STATE, 0);
    op_imm(&mut instructions, opcodes::REF_CONST, 8);
    op(&mut instructions, opcodes::STREAM_TRY_SEND);
    op_imm(&mut instructions, opcodes::STORE_STATE, 40);
    op(&mut instructions, opcodes::RET);
    let code = build(&instructions, &constant(data), 64);
    let mut state = initial_state();
    let mut host = StreamHost::new(3);
    host.queue.extend_from_slice(data);
    let (outcome, _) = run(&code, &mut state, &mut host);
    assert_eq!(outcome, Outcome::Exited(interpreter::ExitReason::Ok));
    assert_eq!(
        u64::from_le_bytes(state[40..48].try_into().unwrap()),
        Status::WouldBlock as u64
    );
}

#[test]
fn a_larger_than_capacity_send_is_capacity_exceeded() {
    let mut instructions = Vec::new();
    op_imm(&mut instructions, opcodes::LOAD_STATE, 0);
    op_imm(&mut instructions, opcodes::REF_CONST, 8);
    op(&mut instructions, opcodes::STREAM_TRY_SEND);
    op_imm(&mut instructions, opcodes::STORE_STATE, 40);
    op(&mut instructions, opcodes::RET);
    let code = build(&instructions, &constant(b"too many bytes"), 64);
    let mut state = initial_state();
    let mut host = StreamHost::new(4);
    let (outcome, _) = run(&code, &mut state, &mut host);
    assert_eq!(outcome, Outcome::Exited(interpreter::ExitReason::Ok));
    assert_eq!(
        u64::from_le_bytes(state[40..48].try_into().unwrap()),
        Status::CapacityExceeded as u64
    );
    assert!(host.queue.is_empty());
}

#[test]
fn an_empty_queue_blocks_the_blocking_receive() {
    let mut instructions = Vec::new();
    op_imm(&mut instructions, opcodes::LOAD_STATE, 0);
    op_imm(&mut instructions, opcodes::REF_STATE, 16);
    op_imm(&mut instructions, opcodes::STREAM_RECV, 16);
    op_imm(&mut instructions, opcodes::STORE_STATE, 40);
    op(&mut instructions, opcodes::RET);
    let code = build(&instructions, &[], 64);
    let mut state = initial_state();
    let mut host = StreamHost::new(16);
    let (outcome, _) = run(&code, &mut state, &mut host);
    assert_eq!(outcome, Outcome::Blocked(WaitKind::Stream(HANDLE)));
}

#[test]
fn a_closed_empty_stream_receives_minus_two() {
    let mut instructions = Vec::new();
    op_imm(&mut instructions, opcodes::LOAD_STATE, 0);
    op_imm(&mut instructions, opcodes::REF_STATE, 16);
    op_imm(&mut instructions, opcodes::STREAM_TRY_RECV, 16);
    op_imm(&mut instructions, opcodes::STORE_STATE, 40);
    op(&mut instructions, opcodes::RET);
    let code = build(&instructions, &[], 64);
    let mut state = initial_state();
    let mut host = StreamHost::new(16);
    host.closed = true;
    let (outcome, _) = run(&code, &mut state, &mut host);
    assert_eq!(outcome, Outcome::Exited(interpreter::ExitReason::Ok));
    assert_eq!(
        u64::from_le_bytes(state[40..48].try_into().unwrap()),
        (-2i64) as u64
    );
}
