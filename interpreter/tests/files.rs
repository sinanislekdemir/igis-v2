//! Application file builtin tests with a mock filesystem (spec 0013).
//!
//! The interpreter does not own the image: it validates the path syntax and
//! the destination regions, decodes the operands, calls the host and publishes
//! results only on `Ok`. These tests pin that contract, the root-only M2.5.1
//! limits and the "out-parameters untouched on failure" rule without the
//! kernel. The disabled defaults every other host inherits are covered too.

use abi::opcodes;
use abi::{
    CodeHeader, FaultCode, Function, Status, ABI_VERSION, CODE_MAGIC, NO_DIAGNOSTIC, NO_NAME,
    PROFILE_X86_64_LE,
};
use interpreter::{ExitReason, FsEntry, Host, Machine, Outcome};

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
        stack_size: 256,
        diag_offset: NO_DIAGNOSTIC,
    };
    let mut code = Vec::new();
    code.extend_from_slice(&header.to_bytes());
    code.extend_from_slice(
        &Function {
            code_off: 0,
            frame_size: 192,
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

fn op_const(out: &mut Vec<u8>, value: i64) {
    op(out, opcodes::CONST);
    out.extend_from_slice(&(value as u32).to_le_bytes());
    out.extend_from_slice(&((value >> 32) as u32).to_le_bytes());
}

/// A mock filesystem that records calls and returns scripted results.
struct FileHost {
    list: Result<FsEntry, Status>,
    list_name: Vec<u8>,
    read: Result<usize, Status>,
    read_bytes: Vec<u8>,
    size: Result<u64, Status>,
    calls: usize,
    last_path: Vec<u8>,
    last_index: u64,
    last_offset: u64,
    last_dst_len: usize,
}

impl FileHost {
    fn new() -> Self {
        FileHost {
            list: Ok(FsEntry {
                name_len: 5,
                kind: abi::fs::KIND_FILE,
                size: 1234,
            }),
            list_name: b"child".to_vec(),
            read: Ok(0),
            read_bytes: Vec::new(),
            size: Ok(1234),
            calls: 0,
            last_path: Vec::new(),
            last_index: 0,
            last_offset: 0,
            last_dst_len: 0,
        }
    }
}

impl Host for FileHost {
    fn console_write(&mut self, _bytes: &[u8]) {}

    fn clock(&mut self) -> u64 {
        0
    }

    fn fs_list(&mut self, path: &[u8], index: u64, name: &mut [u8]) -> Result<FsEntry, Status> {
        self.calls += 1;
        self.last_path = path.to_vec();
        self.last_index = index;
        match self.list {
            Ok(entry) => {
                if entry.name_len > name.len() {
                    return Err(Status::CapacityExceeded);
                }
                name[..entry.name_len].copy_from_slice(&self.list_name[..entry.name_len]);
                Ok(entry)
            }
            Err(status) => Err(status),
        }
    }

    fn fs_read(&mut self, path: &[u8], offset: u64, dst: &mut [u8]) -> Result<usize, Status> {
        self.calls += 1;
        self.last_path = path.to_vec();
        self.last_offset = offset;
        self.last_dst_len = dst.len();
        match self.read {
            Ok(count) => {
                if count > dst.len() {
                    return Err(Status::Invalid);
                }
                dst[..count].copy_from_slice(&self.read_bytes[..count]);
                Ok(count)
            }
            Err(status) => Err(status),
        }
    }

    fn fs_size(&mut self, path: &[u8]) -> Result<u64, Status> {
        self.calls += 1;
        self.last_path = path.to_vec();
        self.size
    }
}

/// A host with no filesystem overrides at all: the trait defaults.
struct DisabledHost;

impl Host for DisabledHost {
    fn console_write(&mut self, _bytes: &[u8]) {}

    fn clock(&mut self) -> u64 {
        0
    }
}

fn run(code: &[u8], state: &mut [u8], host: &mut impl Host) -> Outcome {
    let mut scratch = vec![0u8; code.len().div_ceil(32)];
    let program = interpreter::Program::parse(code, &mut scratch).expect("package verifies");
    let mut stack = vec![0u8; 256];
    let args = vec![0u8; 8 + 1024];
    let mut machine = Machine::new();
    machine.run(&program, host, state, &mut stack, &args, 10_000)
}

fn state_word(state: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(state[offset..offset + 8].try_into().unwrap())
}

fn set_word(state: &mut [u8], offset: usize, value: u64) {
    state[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

/// State layout used by the `fs_list` tests:
/// `[path len][path data 8][name len][name data 64][kind][size][status]`.
fn list_state(path: &[u8]) -> Vec<u8> {
    let mut state = vec![0u8; 128];
    set_word(&mut state, 0, path.len() as u64);
    state[8..8 + path.len()].copy_from_slice(path);
    state
}

fn list_program() -> Vec<u8> {
    // Operands: path data reference, index, name data reference, kind
    // reference, size reference.
    let mut instructions = Vec::new();
    op_imm(&mut instructions, opcodes::REF_STATE, 8);
    op_const(&mut instructions, 1);
    op_imm(&mut instructions, opcodes::REF_STATE, 24);
    op_imm(&mut instructions, opcodes::REF_STATE, 88);
    op_imm(&mut instructions, opcodes::REF_STATE, 96);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::FS_LIST,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 104);
    op(&mut instructions, opcodes::RET);
    build(&instructions, &[], 128)
}

#[test]
fn fs_list_writes_the_name_kind_and_size_on_ok() {
    let mut state = list_state(b".");
    let mut host = FileHost::new();
    assert_eq!(
        run(&list_program(), &mut state, &mut host),
        Outcome::Exited(ExitReason::Ok)
    );
    assert_eq!(host.calls, 1);
    assert_eq!(host.last_path, b".");
    assert_eq!(host.last_index, 1);
    assert_eq!(state_word(&state, 104), Status::Ok as u64);
    assert_eq!(state_word(&state, 16), 5, "the name length is published");
    assert_eq!(&state[24..29], b"child");
    assert_eq!(state_word(&state, 88), abi::fs::KIND_FILE);
    assert_eq!(state_word(&state, 96), 1234);
}

#[test]
fn fs_list_not_found_leaves_every_destination_untouched() {
    let mut state = list_state(b"");
    let mut host = FileHost::new();
    host.list = Err(Status::NotFound);
    assert_eq!(
        run(&list_program(), &mut state, &mut host),
        Outcome::Exited(ExitReason::Ok)
    );
    assert_eq!(state_word(&state, 104), Status::NotFound as u64);
    for offset in (16..104).step_by(8) {
        assert_eq!(
            state_word(&state, offset),
            0,
            "destination {offset} untouched"
        );
    }
}

#[test]
fn fs_list_rejects_a_non_root_path_before_the_host_call() {
    let mut state = list_state(b"child");
    let mut host = FileHost::new();
    assert_eq!(
        run(&list_program(), &mut state, &mut host),
        Outcome::Exited(ExitReason::Ok)
    );
    assert_eq!(state_word(&state, 104), Status::Invalid as u64);
    assert_eq!(host.calls, 0, "the interpreter owns the root-only rule");
    assert_eq!(state_word(&state, 16), 0);
}

/// State layout used by the `fs_read` tests:
/// `[path len][path data 8][dst len][dst data 256][read][status]`.
fn read_state(path: &[u8]) -> Vec<u8> {
    let mut state = vec![0u8; 304];
    set_word(&mut state, 0, path.len() as u64);
    state[8..8 + path.len()].copy_from_slice(path);
    state
}

fn read_program() -> Vec<u8> {
    let mut instructions = Vec::new();
    op_imm(&mut instructions, opcodes::REF_STATE, 8);
    op_const(&mut instructions, 100);
    op_imm(&mut instructions, opcodes::REF_STATE, 24);
    op_imm(&mut instructions, opcodes::REF_STATE, 280);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::FS_READ,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 288);
    op(&mut instructions, opcodes::RET);
    build(&instructions, &[], 304)
}

#[test]
fn fs_read_publishes_a_chunk_and_sets_the_length_and_count() {
    let mut state = read_state(b"child");
    let mut host = FileHost::new();
    host.read = Ok(11);
    host.read_bytes = b"unit child;".to_vec();
    assert_eq!(
        run(&read_program(), &mut state, &mut host),
        Outcome::Exited(ExitReason::Ok)
    );
    assert_eq!(host.last_path, b"child");
    assert_eq!(host.last_offset, 100);
    assert_eq!(host.last_dst_len, interpreter::FS_READ_CHUNK);
    assert_eq!(state_word(&state, 288), Status::Ok as u64);
    assert_eq!(state_word(&state, 16), 11, "the runtime length is set");
    assert_eq!(&state[24..35], b"unit child;");
    assert_eq!(state_word(&state, 280), 11, "`read` mirrors the count");
}

#[test]
fn fs_read_at_end_is_an_ok_with_zero_bytes() {
    let mut state = read_state(b"child");
    let mut host = FileHost::new();
    host.read = Ok(0);
    set_word(&mut state, 16, 7);
    state[24..28].copy_from_slice(b"keep");
    assert_eq!(
        run(&read_program(), &mut state, &mut host),
        Outcome::Exited(ExitReason::Ok)
    );
    assert_eq!(state_word(&state, 288), Status::Ok as u64);
    assert_eq!(state_word(&state, 16), 0, "the length becomes zero");
    assert_eq!(state_word(&state, 280), 0, "`read` is zero");
}

#[test]
fn fs_read_failure_leaves_the_destination_and_count_untouched() {
    let mut state = read_state(b"child");
    let mut host = FileHost::new();
    host.read = Err(Status::NotFound);
    set_word(&mut state, 16, 4);
    state[24..28].copy_from_slice(b"keep");
    set_word(&mut state, 280, 99);
    assert_eq!(
        run(&read_program(), &mut state, &mut host),
        Outcome::Exited(ExitReason::Ok)
    );
    assert_eq!(state_word(&state, 288), Status::NotFound as u64);
    assert_eq!(state_word(&state, 16), 4);
    assert_eq!(&state[24..28], b"keep");
    assert_eq!(state_word(&state, 280), 99);
}

#[test]
fn fs_read_rejects_a_subdirectory_path_before_the_host_call() {
    let mut state = read_state(b"child/x");
    let mut host = FileHost::new();
    assert_eq!(
        run(&read_program(), &mut state, &mut host),
        Outcome::Exited(ExitReason::Ok)
    );
    assert_eq!(state_word(&state, 288), Status::Invalid as u64);
    assert_eq!(host.calls, 0);
    assert_eq!(state_word(&state, 16), 0);
}

#[test]
fn fs_read_faults_when_the_destination_has_no_room_for_the_chunk() {
    let mut state = read_state(b"child");
    let mut host = FileHost::new();
    // A 256-byte destination at state[24] needs 280 bytes of region; this
    // program claims 300 but points the reference near the end.
    let mut instructions = Vec::new();
    op_imm(&mut instructions, opcodes::REF_STATE, 8);
    op_const(&mut instructions, 0);
    op_imm(&mut instructions, opcodes::REF_STATE, 200);
    op_imm(&mut instructions, opcodes::REF_STATE, 280);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::FS_READ,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 288);
    op(&mut instructions, opcodes::RET);
    let code = build(&instructions, &[], 304);
    assert_eq!(
        run(&code, &mut state, &mut host),
        Outcome::Exited(ExitReason::Faulted(FaultCode::OutOfBounds))
    );
    assert_eq!(host.calls, 0);
}

#[test]
fn fs_size_writes_on_ok_and_leaves_the_destination_on_failure() {
    // `[path len][path data 8][size][status]`.
    let mut state = vec![0u8; 32];
    set_word(&mut state, 0, 5);
    state[8..13].copy_from_slice(b"child");
    let mut instructions = Vec::new();
    op_imm(&mut instructions, opcodes::REF_STATE, 8);
    op_imm(&mut instructions, opcodes::REF_STATE, 16);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::FS_SIZE,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 24);
    op(&mut instructions, opcodes::RET);
    let code = build(&instructions, &[], 32);

    let mut host = FileHost::new();
    assert_eq!(
        run(&code, &mut state, &mut host),
        Outcome::Exited(ExitReason::Ok)
    );
    assert_eq!(host.last_path, b"child");
    assert_eq!(state_word(&state, 24), Status::Ok as u64);
    assert_eq!(state_word(&state, 16), 1234);

    set_word(&mut state, 16, 77);
    host.size = Err(Status::Invalid);
    assert_eq!(
        run(&code, &mut state, &mut host),
        Outcome::Exited(ExitReason::Ok)
    );
    assert_eq!(state_word(&state, 24), Status::Invalid as u64);
    assert_eq!(state_word(&state, 16), 77, "a failure writes nothing");
}

#[test]
fn the_default_file_host_is_not_found() {
    let mut state = list_state(b"");
    let mut host = DisabledHost;
    assert_eq!(
        run(&list_program(), &mut state, &mut host),
        Outcome::Exited(ExitReason::Ok)
    );
    assert_eq!(state_word(&state, 104), Status::NotFound as u64);
    assert_eq!(state_word(&state, 16), 0);
}
