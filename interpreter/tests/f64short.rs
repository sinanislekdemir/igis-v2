//! Shortest round-trip `f64` text against the host's own formatter.
//!
//! Rust's `{:e}` produces the shortest significant-digit string that
//! round-trips, so parsing it into `(digits, e10)` and re-applying the shape
//! rule of spec 0003 is an independent oracle for both halves of
//! `core.format_f64_short_into`: the digit sequence and the positional /
//! scientific choice. The packages are hand-assembled, like `verify.rs`.

use abi::opcodes;
use abi::{
    CodeHeader, Function, ABI_VERSION, CODE_MAGIC, NO_DIAGNOSTIC, NO_NAME, PROFILE_X86_64_LE,
};
use interpreter::{ExitReason, Host, Machine, Outcome, Program};

struct FnSpec {
    frame_size: u32,
    operand_off: u32,
}

const MAIN: FnSpec = FnSpec {
    frame_size: 80,
    operand_off: 40,
};

const CAPACITY: u32 = 32;

fn build(spec: FnSpec, instructions: &[u8], state_size: u32, stack_size: u32) -> Vec<u8> {
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
        stack_size,
        diag_offset: NO_DIAGNOSTIC,
    };
    let mut code = Vec::new();
    code.extend_from_slice(&header.to_bytes());
    code.extend_from_slice(
        &Function {
            code_off: 0,
            frame_size: spec.frame_size,
            operand_off: spec.operand_off,
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

fn op_const(out: &mut Vec<u8>, value: i64) {
    op(out, opcodes::CONST);
    out.extend_from_slice(&(value as u32).to_le_bytes());
    out.extend_from_slice(&((value >> 32) as u32).to_le_bytes());
}

fn op_word(out: &mut Vec<u8>, opcode: u8, a: u8, b: u8, c: u8) {
    out.extend_from_slice(&[opcode, a, b, c]);
}

struct Sink;

impl Host for Sink {
    fn console_write(&mut self, _bytes: &[u8]) {}

    fn clock(&mut self) -> u64 {
        0
    }
}

/// Formats `value` through the interpreter and returns the produced text.
fn shortest_text(value: f64, kind: u8) -> String {
    let mut instructions = Vec::new();
    op_imm(&mut instructions, opcodes::REF_STATE, 16);
    op_const(&mut instructions, value.to_bits() as i64);
    op_word(&mut instructions, opcodes::FORMAT_F64_SHORT, kind, 0, 0);
    instructions.extend_from_slice(&CAPACITY.to_le_bytes());
    op_imm(&mut instructions, opcodes::STORE_STATE, 0);
    op(&mut instructions, opcodes::RET);

    let state_size = 16 + CAPACITY as usize;
    let code = build(MAIN, &instructions, state_size as u32, 96);
    let mut scratch = vec![0u8; code.len().div_ceil(32)];
    let program = Program::parse(&code, &mut scratch).expect("package verifies");
    let mut state = vec![0u8; state_size];
    let mut stack = vec![0u8; 96];
    let args = vec![0u8; 8 + 1024];
    let mut machine = Machine::new();
    let mut host = Sink;
    let outcome = machine.run(&program, &mut host, &mut state, &mut stack, &args, 10_000);
    assert_eq!(outcome, Outcome::Exited(ExitReason::Ok), "{value:?}");
    let result = i64::from_le_bytes(state[0..8].try_into().unwrap());
    let length = u64::from_le_bytes(state[8..16].try_into().unwrap());
    assert_eq!(result, length as i64, "the result is the byte count");
    String::from_utf8(state[16..16 + length as usize].to_vec()).expect("ASCII")
}

/// The spec 0003 shape rule over a `(digits, e10)` pair.
fn render(negative: bool, digits: &str, exponent10: i32) -> String {
    let mut text = String::new();
    if negative {
        text.push('-');
    }
    let count = digits.len() as i32;
    let point = exponent10 + 1;
    if point > -6 && point <= 21 {
        if point <= 0 {
            text.push_str("0.");
            for _ in point..0 {
                text.push('0');
            }
            text.push_str(digits);
        } else if point < count {
            text.push_str(&digits[..point as usize]);
            text.push('.');
            text.push_str(&digits[point as usize..]);
        } else {
            text.push_str(digits);
            for _ in count..point {
                text.push('0');
            }
        }
    } else {
        text.push_str(&digits[..1]);
        if digits.len() > 1 {
            text.push('.');
            text.push_str(&digits[1..]);
        }
        text.push('e');
        if exponent10 < 0 {
            text.push('-');
        }
        text.push_str(&format!("{}", exponent10.abs()));
    }
    text
}

/// The expected text, derived from Rust's shortest digits and the shape rule.
fn reference(value: f64) -> String {
    if value.is_nan() {
        return "nan".to_string();
    }
    if value.is_infinite() {
        return if value.is_sign_negative() {
            "-inf".to_string()
        } else {
            "inf".to_string()
        };
    }
    let negative = value.is_sign_negative();
    if value == 0.0 {
        return if negative { "-0" } else { "0" }.to_string();
    }
    let scientific = format!("{:e}", value.abs());
    let (mantissa, exponent) = scientific.split_once('e').expect("`{:e}` has an exponent");
    let exponent10: i32 = exponent.parse().expect("integer exponent");
    let digits = mantissa.replace('.', "");
    render(negative, &digits, exponent10)
}

fn check(value: f64, context: &str) {
    if value.is_nan() || value.is_infinite() {
        return;
    }
    let expected = reference(value);
    let actual = shortest_text(value, opcodes::buffer_kind::STRING);
    assert_eq!(
        actual,
        expected,
        "{context}: bits {:#018x} formatted as {actual} but the oracle says {expected}",
        value.to_bits()
    );
    // Independent of the oracle: the text must parse back to the same bits.
    let parsed: f64 = actual.parse().expect("our own text parses");
    assert_eq!(
        parsed.to_bits(),
        value.to_bits(),
        "{context}: {actual} does not round-trip"
    );
}

#[test]
fn random_bits_match_the_host_formatter() {
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut checked = 0;
    for round in 0..25_000 {
        // xorshift64; the bits are the value, so every exponent range and
        // every significand pattern is reachable.
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let value = f64::from_bits(state);
        if value.is_nan() || value.is_infinite() {
            continue;
        }
        check(value, &format!("random round {round}"));
        checked += 1;
    }
    assert!(checked >= 20_000, "only {checked} finite samples");
}

#[test]
fn powers_and_their_neighbours_match_the_host_formatter() {
    // Powers of ten from the smallest subnormal decade to the largest.
    for exponent in -320..=308i32 {
        let value: f64 = format!("1e{exponent}").parse().expect("power of ten");
        let bits = value.to_bits();
        check(value, &format!("1e{exponent}"));
        check(f64::from_bits(bits + 1), &format!("1e{exponent} up"));
        check(f64::from_bits(bits - 1), &format!("1e{exponent} down"));
    }
    // Every power of two, including the subnormal tail.
    for exponent in -1074..=-1023i32 {
        let value = f64::from_bits(1u64 << (exponent + 1074));
        let bits = value.to_bits();
        check(value, &format!("2^{exponent}"));
        check(f64::from_bits(bits + 1), &format!("2^{exponent} up"));
        check(f64::from_bits(bits - 1), &format!("2^{exponent} down"));
    }
    for exponent in -1022..=1023i32 {
        let value = f64::from_bits(((exponent + 1023) as u64) << 52);
        let bits = value.to_bits();
        check(value, &format!("2^{exponent}"));
        check(f64::from_bits(bits + 1), &format!("2^{exponent} up"));
        check(f64::from_bits(bits - 1), &format!("2^{exponent} down"));
    }
}

#[test]
fn exponent_and_mantissa_sweep_matches_the_host_formatter() {
    // Every biased exponent, at the mantissa boundaries that change the layout
    // (zero, the smallest step, the implicit-bit boundary and the top of the
    // field) plus a moving interior pattern.
    let fractions = [
        0u64,
        1,
        2,
        3,
        0x0007_FFFF_FFFF_FFFF,
        0x0008_0000_0000_0000,
        0x000F_FFFF_FFFF_FFFE,
        0x000F_FFFF_FFFF_FFFF,
    ];
    let mut state = 0x0123_4567_89AB_CDEFu64;
    for biased in 0..0x7FFu64 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        for fraction in fractions
            .into_iter()
            .chain(core::iter::once(state & 0x000F_FFFF_FFFF_FFFF))
        {
            for sign in [0u64, 1 << 63] {
                let value = f64::from_bits(sign | (biased << 52) | fraction);
                check(value, "sweep");
            }
        }
    }
}

#[test]
fn hard_values_match_the_host_formatter() {
    let values = [
        0.0,
        -0.0,
        f64::MIN_POSITIVE,
        f64::MAX,
        f64::MIN,
        5.0e-324,
        1.0e-323,
        0.1,
        0.2,
        0.3,
        1.0 / 3.0,
        2.0 / 3.0,
        1.0e15,
        1.0e16,
        1.0e17,
        1.0e21,
        1.0e22,
        1.0e-6,
        1.0e-7,
        9.999_999_999_999_999e15,
        9_007_199_254_740_992.0,
        9_007_199_254_740_994.0,
        9_007_199_254_740_993.0,
        0.300_000_000_000_000_04,
        1.797_693_134_862_315_7e308,
        2.225_073_858_507_201_4e-308,
        4.9e-324,
        1.234_567_890_123_456_7e-300,
        1.234_567_890_123_456_7e300,
        123_456_789.123_456_79,
    ];
    for value in values {
        check(value, &format!("hard value {value:?}"));
        check(-value, &format!("hard value -{value:?}"));
    }
}

#[test]
fn bytes_destinations_match_the_host_formatter() {
    for value in [1.0, 0.1, 1.0e21, 5.0e-324, f64::MAX, -2.5, 1.0 / 3.0] {
        let expected = reference(value);
        let actual = shortest_text(value, opcodes::buffer_kind::BYTES);
        assert_eq!(actual, expected, "{value:?}");
    }
}
