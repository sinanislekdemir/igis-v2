//! System-control builtin tests (spec 0017).
//!
//! The interpreter stages no operands for `core.halt`/`core.reboot` and pushes
//! the host's status. The live kernel never returns from either call, so these
//! tests pin the only observable surface: the builtin ids, the (0, 1) effect,
//! the host dispatch and the disabled-service defaults every bare host
//! (including `langhost`) inherits.

use abi::opcodes;
use abi::{
    CodeHeader, Function, Status, ABI_VERSION, CODE_MAGIC, NO_DIAGNOSTIC, NO_NAME,
    PROFILE_X86_64_LE,
};
use interpreter::{Host, Machine, Outcome};

fn build(instructions: &[u8], state_size: u32) -> Vec<u8> {
    let header = CodeHeader {
        magic: CODE_MAGIC,
        abi_version: ABI_VERSION,
        profile: PROFILE_X86_64_LE,
        function_count: 1,
        entry: 0,
        code_len: instructions.len() as u32,
        const_len: 0,
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
    code
}

fn op(out: &mut Vec<u8>, opcode: u8) {
    out.extend_from_slice(&[opcode, 0, 0, 0]);
}

fn op_imm(out: &mut Vec<u8>, opcode: u8, immediate: u32) {
    op(out, opcode);
    out.extend_from_slice(&immediate.to_le_bytes());
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

/// A host with system-control authority; records the calls.
struct SystemHost {
    halts: u32,
    reboots: u32,
}

impl Host for SystemHost {
    fn console_write(&mut self, _bytes: &[u8]) {}
    fn clock(&mut self) -> u64 {
        0
    }
    fn halt(&mut self) -> Status {
        self.halts += 1;
        Status::Ok
    }

    fn reboot(&mut self) -> Status {
        self.reboots += 1;
        Status::Ok
    }
}

#[test]
fn halt_and_reboot_are_builtins_48_and_49_with_a_status_result() {
    assert_eq!(opcodes::builtin::HALT, 48);
    assert_eq!(opcodes::builtin::REBOOT, 49);
    assert_eq!(
        opcodes::builtin::effect(opcodes::builtin::HALT),
        Some((0, 1))
    );
    assert_eq!(
        opcodes::builtin::effect(opcodes::builtin::REBOOT),
        Some((0, 1))
    );
}

#[test]
fn the_system_builtins_dispatch_to_the_host_and_store_the_status() {
    let mut instructions = Vec::new();
    op_imm(&mut instructions, opcodes::BUILTIN, opcodes::builtin::HALT);
    op_imm(&mut instructions, opcodes::STORE_STATE, 0);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::REBOOT,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 8);
    op(&mut instructions, opcodes::RET);

    let code = build(&instructions, 64);
    let mut state = vec![0u8; 64];
    let mut host = SystemHost {
        halts: 0,
        reboots: 0,
    };
    assert_eq!(
        run(&code, &mut state, &mut host),
        Outcome::Exited(abi::ExitReason::Ok)
    );
    assert_eq!(host.halts, 1);
    assert_eq!(host.reboots, 1);
    assert_eq!(state_word(&state, 0), Status::Ok as u64);
    assert_eq!(state_word(&state, 8), Status::Ok as u64);
}

#[test]
fn the_default_system_builtins_are_the_disabled_service() {
    let mut instructions = Vec::new();
    op_imm(&mut instructions, opcodes::BUILTIN, opcodes::builtin::HALT);
    op_imm(&mut instructions, opcodes::STORE_STATE, 0);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::REBOOT,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, 8);
    op(&mut instructions, opcodes::RET);

    let code = build(&instructions, 64);
    let mut state = vec![0u8; 64];
    // A record-only host keeps every default; this is the `langhost` contract.
    struct BareHost;
    impl Host for BareHost {
        fn console_write(&mut self, _bytes: &[u8]) {}
        fn clock(&mut self) -> u64 {
            0
        }
    }
    let mut bare = BareHost;
    assert_eq!(
        run(&code, &mut state, &mut bare),
        Outcome::Exited(abi::ExitReason::Ok)
    );
    assert_eq!(state_word(&state, 0), Status::Invalid as u64);
    assert_eq!(state_word(&state, 8), Status::Invalid as u64);
}
