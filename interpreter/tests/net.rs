//! Network builtin tests with a mock host (spec 0020, N12–N17; ADR 0048–0050).
//!
//! The interpreter does not own the wire stack: it range-checks the operands
//! with `abi::net`, resolves the caller's buffers, calls the host and publishes
//! every out-parameter only on `Ok`. These tests pin that contract — including
//! the "untouched on failure" rule that makes a retried non-blocking send or
//! receive safe — without the kernel. They also pin the disabled-NIC defaults
//! every other host inherits.

use abi::opcodes;
use abi::{
    CodeHeader, FaultCode, Function, Status, ABI_VERSION, CODE_MAGIC, NO_DIAGNOSTIC, NO_NAME,
    PROFILE_X86_64_LE,
};
use interpreter::{ExitReason, Host, Machine, NetInfo, NetStats, Outcome};

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

fn builtin(out: &mut Vec<u8>, id: u32) {
    op_imm(out, opcodes::BUILTIN, id);
}

/// The mock network stack: records every call and returns scripted results.
struct NetHost {
    config: Status,
    info: Result<NetInfo, Status>,
    stats: Result<NetStats, Status>,
    mac: u64,
    open: Result<u64, Status>,
    send: Result<usize, Status>,
    recv: Result<usize, Status>,
    recv_bytes: Vec<u8>,
    close: Status,
    calls: usize,
    last_config: Option<(u32, u32, u32, u32)>,
    last_open: Option<(u64, u32, u16, u16)>,
    last_handle: u64,
    last_data: Vec<u8>,
    last_dst_len: usize,
}

impl NetHost {
    fn new() -> Self {
        NetHost {
            config: Status::Ok,
            info: Ok(NetInfo {
                ip: 0x0A00_020F,
                mask: 0xFFFF_FF00,
                gateway: 0x0A00_0202,
                dns: 0x0A00_0203,
            }),
            mac: 0x5254_0012_3456,
            stats: Ok(NetStats {
                rx_frames: 11,
                tx_frames: 7,
                drops: 2,
                errors: 1,
            }),
            open: Ok(0x0101),
            send: Ok(0),
            recv: Ok(0),
            recv_bytes: Vec::new(),
            close: Status::Ok,
            calls: 0,
            last_config: None,
            last_open: None,
            last_handle: 0,
            last_data: Vec::new(),
            last_dst_len: 0,
        }
    }
}

impl Host for NetHost {
    fn console_write(&mut self, _bytes: &[u8]) {}

    fn clock(&mut self) -> u64 {
        0
    }

    fn net_config(&mut self, ip: u32, mask: u32, gateway: u32, dns: u32) -> Status {
        self.calls += 1;
        self.last_config = Some((ip, mask, gateway, dns));
        self.config
    }

    fn net_info(&mut self) -> Result<NetInfo, Status> {
        self.calls += 1;
        self.info
    }

    fn net_mac(&mut self) -> u64 {
        self.calls += 1;
        self.mac
    }

    fn net_stats(&mut self) -> Result<NetStats, Status> {
        self.calls += 1;
        self.stats
    }

    fn socket_open(&mut self, kind: u64, ip: u32, port: u16, local: u16) -> Result<u64, Status> {
        self.calls += 1;
        self.last_open = Some((kind, ip, port, local));
        self.open
    }

    fn socket_send(&mut self, handle: u64, data: &[u8]) -> Result<usize, Status> {
        self.calls += 1;
        self.last_handle = handle;
        self.last_data = data.to_vec();
        self.send
    }

    fn socket_recv(&mut self, handle: u64, dst: &mut [u8]) -> Result<usize, Status> {
        self.calls += 1;
        self.last_handle = handle;
        self.last_dst_len = dst.len();
        match self.recv {
            Ok(count) => {
                // Deliberately not clamped: the machine owns the "count <= the
                // staged limit" rule, and a host that over-reports must fault.
                let copy = count.min(dst.len());
                dst[..copy].copy_from_slice(&self.recv_bytes[..copy]);
                Ok(count)
            }
            Err(status) => Err(status),
        }
    }

    fn socket_close(&mut self, handle: u64) -> Status {
        self.calls += 1;
        self.last_handle = handle;
        self.close
    }
}

/// A host with no network overrides at all: the trait defaults.
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

// ---- configuration ---------------------------------------------------

fn config_program(ip: i64, mask: i64, gateway: i64, dns: i64) -> Vec<u8> {
    let mut instructions = Vec::new();
    op_const(&mut instructions, ip);
    op_const(&mut instructions, mask);
    op_const(&mut instructions, gateway);
    op_const(&mut instructions, dns);
    builtin(&mut instructions, opcodes::builtin::NET_CONFIG);
    op_imm(&mut instructions, opcodes::STORE_STATE, 0);
    op(&mut instructions, opcodes::RET);
    build(&instructions, &[], 8)
}

#[test]
fn net_config_passes_the_four_host_order_values() {
    let mut state = vec![0u8; 8];
    let mut host = NetHost::new();
    let program = config_program(0x0A00_020F, 0xFFFF_FF00, 0x0A00_0202, 0x0A00_0203);
    assert_eq!(
        run(&program, &mut state, &mut host),
        Outcome::Exited(ExitReason::Ok)
    );
    assert_eq!(
        host.last_config,
        Some((0x0A00_020F, 0xFFFF_FF00, 0x0A00_0202, 0x0A00_0203))
    );
    assert_eq!(state_word(&state, 0), Status::Ok as u64);
}

#[test]
fn net_config_pushes_the_host_status() {
    let mut state = vec![0u8; 8];
    let mut host = NetHost::new();
    host.config = Status::IoError;
    let program = config_program(1, 2, 3, 4);
    assert_eq!(
        run(&program, &mut state, &mut host),
        Outcome::Exited(ExitReason::Ok)
    );
    assert_eq!(state_word(&state, 0), Status::IoError as u64);
}

#[test]
fn net_config_rejects_a_value_above_u32_before_the_host_call() {
    let mut state = vec![0u8; 8];
    let mut host = NetHost::new();
    let program = config_program(0x1_0000_0000, 0, 0, 0);
    assert_eq!(
        run(&program, &mut state, &mut host),
        Outcome::Exited(ExitReason::Ok)
    );
    assert_eq!(state_word(&state, 0), Status::Invalid as u64);
    assert_eq!(host.calls, 0, "the interpreter owns the range rule");
}

/// `[ip][mask][gateway][dns][status]`.
fn info_program() -> Vec<u8> {
    let mut instructions = Vec::new();
    op_imm(&mut instructions, opcodes::REF_STATE, 0);
    op_imm(&mut instructions, opcodes::REF_STATE, 8);
    op_imm(&mut instructions, opcodes::REF_STATE, 16);
    op_imm(&mut instructions, opcodes::REF_STATE, 24);
    builtin(&mut instructions, opcodes::builtin::NET_INFO);
    op_imm(&mut instructions, opcodes::STORE_STATE, 32);
    op(&mut instructions, opcodes::RET);
    build(&instructions, &[], 40)
}

#[test]
fn net_info_writes_all_four_values_on_ok() {
    let mut state = vec![0u8; 40];
    let mut host = NetHost::new();
    assert_eq!(
        run(&info_program(), &mut state, &mut host),
        Outcome::Exited(ExitReason::Ok)
    );
    assert_eq!(state_word(&state, 0), 0x0A00_020F);
    assert_eq!(state_word(&state, 8), 0xFFFF_FF00);
    assert_eq!(state_word(&state, 16), 0x0A00_0202);
    assert_eq!(state_word(&state, 24), 0x0A00_0203);
    assert_eq!(state_word(&state, 32), Status::Ok as u64);
}

#[test]
fn net_info_failure_leaves_all_four_values_untouched() {
    let mut state = vec![0xAAu8; 40];
    let mut host = NetHost::new();
    host.info = Err(Status::IoError);
    assert_eq!(
        run(&info_program(), &mut state, &mut host),
        Outcome::Exited(ExitReason::Ok)
    );
    for offset in (0..32).step_by(8) {
        assert_eq!(
            state_word(&state, offset),
            0xAAAA_AAAA_AAAA_AAAA,
            "destination {offset} untouched"
        );
    }
    assert_eq!(state_word(&state, 32), Status::IoError as u64);
}

#[test]
fn net_mac_pushes_the_host_value() {
    let mut instructions = Vec::new();
    builtin(&mut instructions, opcodes::builtin::NET_MAC);
    op_imm(&mut instructions, opcodes::STORE_STATE, 0);
    op(&mut instructions, opcodes::RET);
    let mut state = vec![0u8; 8];
    let mut host = NetHost::new();
    assert_eq!(
        run(&build(&instructions, &[], 8), &mut state, &mut host),
        Outcome::Exited(ExitReason::Ok)
    );
    assert_eq!(state_word(&state, 0), 0x5254_0012_3456);

    let mut absent = NetHost::new();
    absent.mac = 0;
    assert_eq!(
        run(&build(&instructions, &[], 8), &mut state, &mut absent),
        Outcome::Exited(ExitReason::Ok)
    );
    assert_eq!(state_word(&state, 0), 0, "no device reports zero");
}

/// `[rx][tx][drops][errors][status]`.
fn stats_program() -> Vec<u8> {
    let mut instructions = Vec::new();
    op_imm(&mut instructions, opcodes::REF_STATE, 0);
    op_imm(&mut instructions, opcodes::REF_STATE, 8);
    op_imm(&mut instructions, opcodes::REF_STATE, 16);
    op_imm(&mut instructions, opcodes::REF_STATE, 24);
    builtin(&mut instructions, opcodes::builtin::NET_STATS);
    op_imm(&mut instructions, opcodes::STORE_STATE, 32);
    op(&mut instructions, opcodes::RET);
    build(&instructions, &[], 40)
}

#[test]
fn net_stats_writes_all_four_counters_on_ok() {
    let mut state = vec![0u8; 40];
    let mut host = NetHost::new();
    assert_eq!(
        run(&stats_program(), &mut state, &mut host),
        Outcome::Exited(ExitReason::Ok)
    );
    assert_eq!(state_word(&state, 0), 11);
    assert_eq!(state_word(&state, 8), 7);
    assert_eq!(state_word(&state, 16), 2);
    assert_eq!(state_word(&state, 24), 1);
    assert_eq!(state_word(&state, 32), Status::Ok as u64);
    assert_eq!(host.calls, 1);
}

#[test]
fn net_stats_failure_leaves_all_four_counters_untouched() {
    let mut state = vec![0xAAu8; 40];
    let mut host = NetHost::new();
    host.stats = Err(Status::IoError);
    assert_eq!(
        run(&stats_program(), &mut state, &mut host),
        Outcome::Exited(ExitReason::Ok)
    );
    for offset in (0..32).step_by(8) {
        assert_eq!(
            state_word(&state, offset),
            0xAAAA_AAAA_AAAA_AAAA,
            "counter {offset} untouched"
        );
    }
    assert_eq!(state_word(&state, 32), Status::IoError as u64);
}

#[test]
fn the_default_net_stats_is_the_disabled_nic() {
    // `DisabledHost` overrides nothing, so `net_stats` is the trait default.
    let mut state = vec![0x11u8; 40];
    let mut host = DisabledHost;
    assert_eq!(
        run(&stats_program(), &mut state, &mut host),
        Outcome::Exited(ExitReason::Ok)
    );
    for offset in (0..32).step_by(8) {
        assert_eq!(state_word(&state, offset), 0x1111_1111_1111_1111);
    }
    assert_eq!(state_word(&state, 32), Status::Invalid as u64);
}

// ---- sockets ---------------------------------------------------------

/// `[id][status]`.
fn open_program(kind: i64, ip: i64, port: i64, local: i64) -> Vec<u8> {
    let mut instructions = Vec::new();
    op_const(&mut instructions, kind);
    op_const(&mut instructions, ip);
    op_const(&mut instructions, port);
    op_const(&mut instructions, local);
    op_imm(&mut instructions, opcodes::REF_STATE, 0);
    builtin(&mut instructions, opcodes::builtin::SOCKET_OPEN);
    op_imm(&mut instructions, opcodes::STORE_STATE, 8);
    op(&mut instructions, opcodes::RET);
    build(&instructions, &[], 16)
}

#[test]
fn socket_open_writes_the_handle_on_ok() {
    let mut state = vec![0u8; 16];
    let mut host = NetHost::new();
    let program = open_program(abi::net::KIND_TCP as i64, 0x0A00_0202, 6666, 0);
    assert_eq!(
        run(&program, &mut state, &mut host),
        Outcome::Exited(ExitReason::Ok)
    );
    assert_eq!(
        host.last_open,
        Some((abi::net::KIND_TCP, 0x0A00_0202, 6666, 0))
    );
    assert_eq!(state_word(&state, 0), 0x0101);
    assert_eq!(state_word(&state, 8), Status::Ok as u64);
}

#[test]
fn socket_open_failure_leaves_the_id_untouched() {
    let mut state = vec![0u8; 16];
    let mut host = NetHost::new();
    host.open = Err(Status::Full);
    let program = open_program(abi::net::KIND_UDP as i64, 1, 0, 5353);
    assert_eq!(
        run(&program, &mut state, &mut host),
        Outcome::Exited(ExitReason::Ok)
    );
    assert_eq!(state_word(&state, 0), 0, "the id is untouched");
    assert_eq!(state_word(&state, 8), Status::Full as u64);
}

#[test]
fn socket_open_rejects_a_bad_kind_or_port_before_the_host_call() {
    for program in [
        open_program(3, 0x0A00_0202, 80, 0),
        open_program(0, 0x0A00_0202, 80, 0),
        open_program(abi::net::KIND_TCP as i64, 0x0A00_0202, 65536, 0),
        open_program(abi::net::KIND_TCP as i64, 0x0A00_0202, 80, 65536),
        open_program(abi::net::KIND_TCP as i64, 0x1_0000_0000, 80, 0),
    ] {
        let mut state = vec![0xAAu8; 16];
        let mut host = NetHost::new();
        assert_eq!(
            run(&program, &mut state, &mut host),
            Outcome::Exited(ExitReason::Ok)
        );
        assert_eq!(state_word(&state, 8), Status::Invalid as u64);
        assert_eq!(state_word(&state, 0), 0xAAAA_AAAA_AAAA_AAAA);
        assert_eq!(host.calls, 0, "a malformed open never reaches the stack");
    }
}

#[test]
fn socket_open_faults_on_a_zero_handle() {
    let mut state = vec![0u8; 16];
    let mut host = NetHost::new();
    host.open = Ok(0);
    let program = open_program(abi::net::KIND_TCP as i64, 1, 80, 0);
    assert_eq!(
        run(&program, &mut state, &mut host),
        Outcome::Exited(ExitReason::Faulted(FaultCode::IllegalOperation))
    );
    assert_eq!(state_word(&state, 0), 0);
}

/// `[data len][data 5][sent][status]`.
fn send_program(data_ref: u32) -> Vec<u8> {
    let mut instructions = Vec::new();
    op_const(&mut instructions, 0x0101);
    op_imm(&mut instructions, opcodes::REF_STATE, data_ref);
    op_imm(&mut instructions, opcodes::REF_STATE, 16);
    builtin(&mut instructions, opcodes::builtin::SOCKET_SEND);
    op_imm(&mut instructions, opcodes::STORE_STATE, 24);
    op(&mut instructions, opcodes::RET);
    build(&instructions, &[], 32)
}

fn send_state() -> Vec<u8> {
    let mut state = vec![0u8; 32];
    set_word(&mut state, 0, 5);
    state[8..13].copy_from_slice(b"HELLO");
    state
}

#[test]
fn socket_send_writes_the_copied_count_and_passes_the_bytes() {
    let mut state = send_state();
    let mut host = NetHost::new();
    host.send = Ok(5);
    assert_eq!(
        run(&send_program(8), &mut state, &mut host),
        Outcome::Exited(ExitReason::Ok)
    );
    assert_eq!(host.last_handle, 0x0101);
    assert_eq!(host.last_data, b"HELLO");
    assert_eq!(state_word(&state, 16), 5);
    assert_eq!(state_word(&state, 24), Status::Ok as u64);
}

#[test]
fn socket_send_would_block_leaves_the_count_untouched() {
    let mut state = send_state();
    set_word(&mut state, 16, 99);
    let mut host = NetHost::new();
    host.send = Err(Status::WouldBlock);
    assert_eq!(
        run(&send_program(8), &mut state, &mut host),
        Outcome::Exited(ExitReason::Ok)
    );
    assert_eq!(state_word(&state, 16), 99, "`sent` is untouched");
    assert_eq!(state_word(&state, 24), Status::WouldBlock as u64);
}

#[test]
fn socket_send_copies_a_prefix_of_the_buffer() {
    let mut state = send_state();
    let mut host = NetHost::new();
    host.send = Ok(2);
    assert_eq!(
        run(&send_program(8), &mut state, &mut host),
        Outcome::Exited(ExitReason::Ok)
    );
    assert_eq!(state_word(&state, 16), 2, "a partial send reports a prefix");
    assert_eq!(host.last_data, b"HELLO");
}

#[test]
fn socket_send_faults_when_the_host_over_reports() {
    let mut state = send_state();
    let mut host = NetHost::new();
    host.send = Ok(6);
    assert_eq!(
        run(&send_program(8), &mut state, &mut host),
        Outcome::Exited(ExitReason::Faulted(FaultCode::IllegalOperation))
    );
}

/// `[dst len][dst 8][read][status]`.
fn recv_program(capacity: i64) -> Vec<u8> {
    let mut instructions = Vec::new();
    op_const(&mut instructions, 0x0101);
    op_const(&mut instructions, capacity);
    op_imm(&mut instructions, opcodes::REF_STATE, 8);
    op_imm(&mut instructions, opcodes::REF_STATE, 16);
    builtin(&mut instructions, opcodes::builtin::SOCKET_RECV);
    op_imm(&mut instructions, opcodes::STORE_STATE, 24);
    op(&mut instructions, opcodes::RET);
    build(&instructions, &[], 32)
}

#[test]
fn socket_recv_publishes_the_length_and_count_on_ok() {
    let mut state = vec![0u8; 32];
    let mut host = NetHost::new();
    host.recv = Ok(4);
    host.recv_bytes = b"PONG".to_vec();
    assert_eq!(
        run(&recv_program(8), &mut state, &mut host),
        Outcome::Exited(ExitReason::Ok)
    );
    assert_eq!(host.last_handle, 0x0101);
    assert_eq!(
        host.last_dst_len, 8,
        "the declared capacity bounds the copy"
    );
    assert_eq!(state_word(&state, 0), 4, "the runtime length is set");
    assert_eq!(&state[8..12], b"PONG");
    assert_eq!(state_word(&state, 16), 4);
    assert_eq!(state_word(&state, 24), Status::Ok as u64);
}

#[test]
fn socket_recv_end_of_file_is_an_ok_with_zero_bytes() {
    let mut state = vec![0u8; 32];
    let mut host = NetHost::new();
    host.recv = Ok(0);
    assert_eq!(
        run(&recv_program(8), &mut state, &mut host),
        Outcome::Exited(ExitReason::Ok)
    );
    assert_eq!(state_word(&state, 0), 0, "the length becomes zero");
    assert_eq!(state_word(&state, 16), 0);
    assert_eq!(state_word(&state, 24), Status::Ok as u64);
}

#[test]
fn socket_recv_would_block_leaves_the_destination_and_count_untouched() {
    let mut state = vec![0u8; 32];
    set_word(&mut state, 0, 3);
    state[8..11].copy_from_slice(b"old");
    set_word(&mut state, 16, 77);
    let mut host = NetHost::new();
    host.recv = Err(Status::WouldBlock);
    assert_eq!(
        run(&recv_program(8), &mut state, &mut host),
        Outcome::Exited(ExitReason::Ok)
    );
    assert_eq!(state_word(&state, 0), 3);
    assert_eq!(&state[8..11], b"old");
    assert_eq!(state_word(&state, 16), 77);
    assert_eq!(state_word(&state, 24), Status::WouldBlock as u64);
}

#[test]
fn socket_recv_clamps_a_large_destination_to_one_chunk() {
    // A 4096-byte buffer: the interpreter still stages at most RECV_CHUNK.
    let state_size = 8 + 4096 + 8 + 8;
    let mut instructions = Vec::new();
    op_const(&mut instructions, 0x0101);
    op_const(&mut instructions, 4096);
    op_imm(&mut instructions, opcodes::REF_STATE, 8);
    op_imm(&mut instructions, opcodes::REF_STATE, 8 + 4096);
    builtin(&mut instructions, opcodes::builtin::SOCKET_RECV);
    op_imm(&mut instructions, opcodes::STORE_STATE, 8 + 4096 + 8);
    op(&mut instructions, opcodes::RET);
    let mut state = vec![0u8; state_size as usize];
    let mut host = NetHost::new();
    host.recv = Ok(0);
    assert_eq!(
        run(
            &build(&instructions, &[], state_size),
            &mut state,
            &mut host
        ),
        Outcome::Exited(ExitReason::Ok)
    );
    assert_eq!(host.last_dst_len, interpreter::NET_RECV_CHUNK);
    assert_eq!(host.last_dst_len, abi::net::RECV_CHUNK);
}

#[test]
fn socket_recv_faults_when_the_host_over_reports() {
    let mut state = vec![0u8; 32];
    let mut host = NetHost::new();
    host.recv = Ok(9);
    host.recv_bytes = b"OVERREPORT".to_vec();
    assert_eq!(
        run(&recv_program(8), &mut state, &mut host),
        Outcome::Exited(ExitReason::Faulted(FaultCode::IllegalOperation))
    );
}

#[test]
fn socket_recv_faults_when_the_destination_is_smaller_than_its_capacity() {
    // The reference names state[24] but the region ends at 32: the eight-byte
    // copy the declared capacity promises does not fit.
    let mut instructions = Vec::new();
    op_const(&mut instructions, 0x0101);
    op_const(&mut instructions, 16);
    op_imm(&mut instructions, opcodes::REF_STATE, 24);
    op_imm(&mut instructions, opcodes::REF_STATE, 16);
    builtin(&mut instructions, opcodes::builtin::SOCKET_RECV);
    op_imm(&mut instructions, opcodes::STORE_STATE, 24);
    op(&mut instructions, opcodes::RET);
    let mut state = vec![0u8; 32];
    let mut host = NetHost::new();
    assert_eq!(
        run(&build(&instructions, &[], 32), &mut state, &mut host),
        Outcome::Exited(ExitReason::Faulted(FaultCode::OutOfBounds))
    );
    assert_eq!(host.calls, 0, "nothing reaches the stack");
}

#[test]
fn socket_close_returns_the_host_status() {
    let mut instructions = Vec::new();
    op_const(&mut instructions, 0x0101);
    builtin(&mut instructions, opcodes::builtin::SOCKET_CLOSE);
    op_imm(&mut instructions, opcodes::STORE_STATE, 0);
    op(&mut instructions, opcodes::RET);
    let program = build(&instructions, &[], 8);

    let mut state = vec![0u8; 8];
    let mut host = NetHost::new();
    assert_eq!(
        run(&program, &mut state, &mut host),
        Outcome::Exited(ExitReason::Ok)
    );
    assert_eq!(host.last_handle, 0x0101);
    assert_eq!(state_word(&state, 0), Status::Ok as u64);

    let mut gone = NetHost::new();
    gone.close = Status::NotFound;
    assert_eq!(
        run(&program, &mut state, &mut gone),
        Outcome::Exited(ExitReason::Ok)
    );
    assert_eq!(state_word(&state, 0), Status::NotFound as u64);
}

#[test]
fn the_disabled_nic_defaults_leave_everything_untouched() {
    // `net_config` is `Invalid`, `net_info` is `Invalid`, `net_mac` is 0.
    let mut host = DisabledHost;

    let mut config_state = vec![0xAAu8; 8];
    assert_eq!(
        run(&config_program(1, 2, 3, 4), &mut config_state, &mut host),
        Outcome::Exited(ExitReason::Ok)
    );
    assert_eq!(state_word(&config_state, 0), Status::Invalid as u64);

    let mut info_state = vec![0xAAu8; 40];
    assert_eq!(
        run(&info_program(), &mut info_state, &mut host),
        Outcome::Exited(ExitReason::Ok)
    );
    for offset in (0..32).step_by(8) {
        assert_eq!(state_word(&info_state, offset), 0xAAAA_AAAA_AAAA_AAAA);
    }
    assert_eq!(state_word(&info_state, 32), Status::Invalid as u64);

    let mut instructions = Vec::new();
    builtin(&mut instructions, opcodes::builtin::NET_MAC);
    op_imm(&mut instructions, opcodes::STORE_STATE, 0);
    op(&mut instructions, opcodes::RET);
    let mut mac_state = vec![0u8; 8];
    assert_eq!(
        run(&build(&instructions, &[], 8), &mut mac_state, &mut host),
        Outcome::Exited(ExitReason::Ok)
    );
    assert_eq!(state_word(&mac_state, 0), 0);

    // A socket call without a device is `Invalid` and writes nothing.
    let mut socket_state = vec![0xAAu8; 32];
    assert_eq!(
        run(
            &open_program(abi::net::KIND_TCP as i64, 1, 80, 0),
            &mut socket_state,
            &mut host
        ),
        Outcome::Exited(ExitReason::Ok)
    );
    assert_eq!(state_word(&socket_state, 0), 0xAAAA_AAAA_AAAA_AAAA);
    assert_eq!(state_word(&socket_state, 8), Status::Invalid as u64);
}
