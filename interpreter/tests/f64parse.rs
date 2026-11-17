//! `core.parse_f64_into` builtin tests (spec 0003, "Floating-point text"; Q41).
//!
//! The conversion itself is exercised in `interpreter/src/f64parse.rs` against
//! hard-coded bit patterns and the exact `f64short` formatter. These tests pin
//! the *builtin* contract: the operand order, the `Status` result, the
//! `f64` out-parameter written only on `Ok`, and the fact that a malformed
//! string never touches it. Packages are hand-assembled, like `verify.rs`.

use abi::opcodes;
use abi::{
    CodeHeader, Function, Status, ABI_VERSION, CODE_MAGIC, NO_DIAGNOSTIC, NO_NAME,
    PROFILE_X86_64_LE,
};
use interpreter::{ExitReason, Host, Machine, Outcome, Program};

const SRC_CONST: u32 = 8;
const VALUE_OFF: u32 = 8;
const STATUS_OFF: u32 = 0;

fn build(instructions: &[u8], constants: &[u8]) -> Vec<u8> {
    let header = CodeHeader {
        magic: CODE_MAGIC,
        abi_version: ABI_VERSION,
        profile: PROFILE_X86_64_LE,
        function_count: 1,
        entry: 0,
        code_len: instructions.len() as u32,
        const_len: constants.len() as u32,
        line_len: 0,
        state_size: 24,
        stack_size: 128,
        diag_offset: NO_DIAGNOSTIC,
    };
    let mut code = Vec::new();
    code.extend_from_slice(&header.to_bytes());
    code.extend_from_slice(
        &Function {
            code_off: 0,
            frame_size: 96,
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

/// Builds `parse(src, value); status := Ok`.
fn parse_package(src: &[u8]) -> Vec<u8> {
    let mut instructions = Vec::new();
    op_imm(&mut instructions, opcodes::REF_CONST, SRC_CONST);
    op_imm(&mut instructions, opcodes::REF_STATE, VALUE_OFF);
    op_imm(
        &mut instructions,
        opcodes::BUILTIN,
        opcodes::builtin::PARSE_F64,
    );
    op_imm(&mut instructions, opcodes::STORE_STATE, STATUS_OFF);
    op(&mut instructions, opcodes::RET);
    build(&instructions, &constant(src))
}

struct Silent;

impl Host for Silent {
    fn console_write(&mut self, _bytes: &[u8]) {}
    fn clock(&mut self) -> u64 {
        0
    }
}

const SENTINEL: u64 = 0x401e_0000_0000_0000; // 7.5

fn run(src: &[u8]) -> (Status, u64) {
    let code = parse_package(src);
    let mut scratch = vec![0u8; code.len().div_ceil(32)];
    let program = Program::parse(&code, &mut scratch).expect("package verifies");
    let mut state = vec![0u8; 24];
    state[VALUE_OFF as usize..VALUE_OFF as usize + 8].copy_from_slice(&SENTINEL.to_le_bytes());
    let mut stack = vec![0u8; 128];
    let args = vec![0u8; 8 + 1024];
    let mut machine = Machine::new();
    let mut host = Silent;
    match machine.run(&program, &mut host, &mut state, &mut stack, &args, 10_000) {
        Outcome::Exited(ExitReason::Ok) => {}
        other => panic!("unexpected outcome {other:?}"),
    }
    let status = u64::from_le_bytes(
        state[STATUS_OFF as usize..STATUS_OFF as usize + 8]
            .try_into()
            .unwrap(),
    );
    let bits = u64::from_le_bytes(
        state[VALUE_OFF as usize..VALUE_OFF as usize + 8]
            .try_into()
            .unwrap(),
    );
    let status = match status {
        0 => Status::Ok,
        4 => Status::Invalid,
        other => panic!("unexpected status {other}"),
    };
    (status, bits)
}

#[test]
fn parses_a_decimal_and_writes_the_bits() {
    let (status, bits) = run(b"3.25");
    assert_eq!(status, Status::Ok);
    assert_eq!(bits, 3.25f64.to_bits());

    let (status, bits) = run(b"-0");
    assert_eq!(status, Status::Ok);
    assert_eq!(bits, 0x8000_0000_0000_0000);

    let (status, bits) = run(b"nan");
    assert_eq!(status, Status::Ok);
    assert_eq!(bits, 0x7ff8_0000_0000_0000, "canonical quiet NaN");
}

#[test]
fn a_malformed_string_is_invalid_and_leaves_the_value_untouched() {
    for src in [
        &b""[..],
        b"+",
        b".",
        b"1e",
        b"1e+",
        b"1.2.3",
        b" 1",
        b"1 ",
        b"--1",
        b"infinity",
        b"+nan",
        b"INF",
        b"1\x00",
        b"1\xff",
    ] {
        let (status, bits) = run(src);
        assert_eq!(status, Status::Invalid, "{src:?}");
        assert_eq!(bits, SENTINEL, "{src:?} wrote through the out-parameter");
    }
}
