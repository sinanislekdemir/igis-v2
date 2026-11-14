//! Compiler-level tests: packages, determinism and diagnostics.

use abi::{CodeHeader, ABI_VERSION, CODE_MAGIC, PROFILE_X86_64_LE};

const HELLO: &[u8] = include_bytes!("../../samples/lang/hello.ig");
const LOOP: &[u8] = include_bytes!("../../samples/lang/loop.ig");

/// The compiler is allocation-free, so this integration test plays the kernel's
/// role: allocate the bounded buffers, call `compile_into`, copy the result.
#[derive(Debug, PartialEq, Eq)]
struct Owned {
    manifest: Vec<u8>,
    code: Vec<u8>,
}

#[allow(clippy::result_large_err)]
fn compile(source: &[u8], path: &str) -> Result<Owned, compiler::Diagnostic> {
    let mut arena = vec![0u8; compiler::MAX_ARENA_BYTES];
    let mut code = vec![0u8; compiler::MAX_CODE_BYTES];
    let mut manifest = vec![0u8; compiler::MAX_MANIFEST_BYTES];
    let package = compiler::compile_into(source, path, &mut arena, &mut code, &mut manifest)?;
    Ok(Owned {
        manifest: package.manifest.to_vec(),
        code: package.code.to_vec(),
    })
}

#[test]
fn hello_emits_a_well_formed_header() {
    let package = compile(HELLO, "hello.ig").expect("compiles");
    let header = CodeHeader::from_bytes(&package.code[..40]).expect("header");
    assert_eq!(header.magic, CODE_MAGIC);
    assert_eq!(header.abi_version, ABI_VERSION);
    assert_eq!(header.profile, PROFILE_X86_64_LE);
    assert_eq!(header.function_count, 1);
    assert_eq!(header.entry, 0);
    assert!(header.code_len > 0);
    assert!(header.const_len > 0, "the string literal lives in the pool");
    assert!(header.line_len > 0);
    assert!(header.stack_size >= 40);
    assert!(!package.manifest.is_empty());
}

#[test]
fn compiling_twice_is_byte_identical() {
    let first = compile(LOOP, "loop.ig").expect("compiles");
    let second = compile(LOOP, "loop.ig").expect("compiles");
    assert_eq!(first, second);
}

#[test]
fn undefined_procedure_is_reported() {
    let source = br#"
unit t;

procedure main(args: const string[1024]);
begin
  missing();
end;
"#;
    let error = compile(source, "t.ig").expect_err("must not compile");
    assert_eq!(error.line, 6);
    assert_eq!(error.column, 3);
    assert!(
        error.message.contains("undefined procedure `missing`"),
        "{}",
        error.message
    );
}

#[test]
fn wrong_argument_count_is_reported() {
    let source = br#"
unit t;

procedure greet(name: const string[0]);
begin
end;

procedure main(args: const string[1024]);
begin
  greet();
end;
"#;
    let error = compile(source, "t.ig").expect_err("must not compile");
    assert_eq!(error.line, 10);
    assert_eq!(error.column, 3);
    assert!(
        error.message.contains("expects 1 argument(s), got 0"),
        "{}",
        error.message
    );
}

#[test]
fn capacity_mismatch_between_variables_is_reported() {
    let source = br#"
unit t;

var
  wide: string[16];
  narrow: string[8];

procedure main(args: const string[1024]);
begin
  wide := "x";
  narrow := wide;
end;
"#;
    let error = compile(source, "t.ig").expect_err("must not compile");
    assert_eq!(error.line, 11);
    assert!(
        error.message.contains("capacity mismatch"),
        "{}",
        error.message
    );
}

#[test]
fn self_concatenation_is_rejected() {
    let source = br#"
unit t;

var
  text: string[16];

procedure main(args: const string[1024]);
begin
  text := text + "x";
end;
"#;
    let error = compile(source, "t.ig").expect_err("must not compile");
    assert!(
        error.message.contains("aliases an operand"),
        "{}",
        error.message
    );
}

#[test]
fn unknown_unit_in_uses_is_reported() {
    let source = br#"
unit t;

uses stdlib;

procedure main(args: const string[1024]);
begin
end;
"#;
    let error = compile(source, "t.ig").expect_err("must not compile");
    assert!(error.message.contains("unknown unit `stdlib`"));
}

#[test]
fn boolean_operators_require_bools() {
    let source = br#"
unit t;

procedure main(args: const string[1024]);
var
  n: int;
begin
  n := 1 and 2;
end;
"#;
    let error = compile(source, "t.ig").expect_err("must not compile");
    assert!(
        error.message.contains("needs `bool` operands"),
        "{}",
        error.message
    );
}

#[test]
fn main_signature_is_checked() {
    let source = br#"
unit t;

procedure main();
begin
end;
"#;
    let error = compile(source, "t.ig").expect_err("must not compile");
    assert!(
        error.message.contains("main must be declared"),
        "{}",
        error.message
    );
}

#[test]
fn read_line_accepts_a_var_string_and_rejects_bytes() {
    let good = br#"
unit t;

var
  line: string[16];

procedure main(args: const string[1024]);
begin
  core.read_line(line);
end;
"#;
    compile(good, "t.ig").expect("a var string buffer is accepted");

    let bad = br#"
unit t;

var
  raw: bytes[16];

procedure main(args: const string[1024]);
begin
  core.read_line(raw);
end;
"#;
    let error = compile(bad, "t.ig").expect_err("bytes is not a string buffer");
    assert!(error.message.contains("var string"), "{}", error.message);
}

#[test]
fn a_raw_byte_in_a_string_literal_is_rejected() {
    let source = br#"
unit t;

var
  text: string[4];

procedure main(args: const string[1024]);
begin
  text := "\xFF";
end;
"#;
    let error = compile(source, "t.ig").expect_err("must not compile");
    assert!(
        error.message.contains("not valid UTF-8"),
        "{}",
        error.message
    );
}

#[test]
fn a_tiny_arena_is_a_capacity_error() {
    let mut arena = [0u8; 64];
    let mut code = vec![0u8; compiler::MAX_CODE_BYTES];
    let mut manifest = vec![0u8; compiler::MAX_MANIFEST_BYTES];
    let error = compiler::compile_into(HELLO, "hello.ig", &mut arena, &mut code, &mut manifest)
        .expect_err("must be refused");
    assert_eq!(error.status, abi::Status::CapacityExceeded);
    assert!(error.message.contains("scratch arena"), "{}", error.message);
}

#[test]
fn a_tiny_code_buffer_is_a_capacity_error() {
    let mut arena = vec![0u8; compiler::MAX_ARENA_BYTES];
    let mut code = [0u8; 8];
    let mut manifest = vec![0u8; compiler::MAX_MANIFEST_BYTES];
    let error = compiler::compile_into(HELLO, "hello.ig", &mut arena, &mut code, &mut manifest)
        .expect_err("must be refused");
    assert_eq!(error.status, abi::Status::CapacityExceeded);
    assert!(error.message.contains("code buffer"), "{}", error.message);
}

#[test]
fn a_tiny_manifest_buffer_is_a_capacity_error() {
    let mut arena = vec![0u8; compiler::MAX_ARENA_BYTES];
    let mut code = vec![0u8; compiler::MAX_CODE_BYTES];
    let mut manifest = [0u8; 8];
    let error = compiler::compile_into(HELLO, "hello.ig", &mut arena, &mut code, &mut manifest)
        .expect_err("must be refused");
    assert_eq!(error.status, abi::Status::CapacityExceeded);
    assert!(
        error.message.contains("manifest buffer"),
        "{}",
        error.message
    );
}

#[test]
fn compile_into_reports_its_caps() {
    assert_eq!(compiler::MAX_SOURCE_BYTES, 64 * 1024);
    assert_eq!(compiler::MAX_ARENA_BYTES, 1024 * 1024);
    assert_eq!(compiler::MAX_CODE_BYTES, 256 * 1024);
    assert_eq!(compiler::MAX_MANIFEST_BYTES, 4 * 1024);
}

#[test]
fn spawn_wait_and_kill_emit_their_builtin_ids() {
    use abi::opcodes::{self, builtin};

    let source = br#"
unit t;

uses core;

var
  id: int;
  code: int;

procedure main(args: const string[1024]);
begin
  id := core.spawn("child", "hello");
  code := core.wait(id);
  code := core.kill(id);
end;
"#;
    let package = compile(source, "t.ig").expect("compiles");
    let header = CodeHeader::from_bytes(&package.code[..40]).expect("header");
    let start = 40 + 24 * header.function_count as usize;
    let instructions = &package.code[start..start + header.code_len as usize];

    let mut seen = Vec::new();
    let mut pc = 0;
    while pc + 4 <= instructions.len() {
        let op = instructions[pc];
        let words = opcodes::instruction_words(op).expect("known opcode") as usize;
        if op == opcodes::BUILTIN {
            let id = u32::from_le_bytes(instructions[pc + 4..pc + 8].try_into().unwrap());
            seen.push(id);
        }
        pc += words * 4;
    }
    assert_eq!(seen, vec![builtin::SPAWN, builtin::WAIT, builtin::KILL]);
}

#[test]
fn spawn_argument_count_and_task_id_type_are_checked() {
    let wrong_count = br#"
unit t;

uses core;

procedure main(args: const string[1024]);
begin
  core.spawn("child");
end;
"#;
    let error = compile(wrong_count, "t.ig").expect_err("must not compile");
    assert!(
        error.message.contains("`core.spawn` expects 2 arguments"),
        "{}",
        error.message
    );

    let wrong_type = br#"
unit t;

uses core;

var
  u: uint;

procedure main(args: const string[1024]);
begin
  u := uint(1);
  core.wait(u);
end;
"#;
    let error = compile(wrong_type, "t.ig").expect_err("must not compile");
    assert!(error.message.contains("task id"), "{}", error.message);
}

/// Walks the instruction section and returns every opcode in order.
fn opcodes_of(package: &Owned) -> Vec<u8> {
    use abi::opcodes;
    let header = CodeHeader::from_bytes(&package.code[..40]).expect("header");
    let start = 40 + 24 * header.function_count as usize;
    let instructions = &package.code[start..start + header.code_len as usize];
    let mut seen = Vec::new();
    let mut pc = 0;
    while pc + 4 <= instructions.len() {
        let op = instructions[pc];
        seen.push(op);
        let words = opcodes::instruction_words(op).expect("known opcode") as usize;
        pc += words * 4;
    }
    seen
}

#[test]
fn indexing_emits_byte_load_and_store() {
    let source = br#"
unit t;

var
  raw: bytes[8];
  b: byte;
  i: int;

procedure main(args: const string[1024]);
begin
  raw := "ab";
  i := 1;
  b := raw[i];
  raw[i + 1] := b;
end;
"#;
    let package = compile(source, "t.ig").expect("compiles");
    let ops = opcodes_of(&package);
    assert!(
        ops.contains(&abi::opcodes::LOAD_U8),
        "no load_u8 in {ops:?}"
    );
    assert!(
        ops.contains(&abi::opcodes::STORE_U8),
        "no store_u8 in {ops:?}"
    );
}

#[test]
fn reading_a_byte_from_a_string_is_allowed() {
    let source = br#"
unit t;

var
  text: string[8];
  b: byte;

procedure main(args: const string[1024]);
begin
  text := "abc";
  b := text[1];
end;
"#;
    compile(source, "t.ig").expect("a string is readable by index");
}

#[test]
fn writing_into_a_string_by_index_is_rejected() {
    let source = br#"
unit t;

var
  text: string[8];

procedure main(args: const string[1024]);
begin
  text := "abc";
  text[1] := byte(66);
end;
"#;
    let error = compile(source, "t.ig").expect_err("must not compile");
    assert_eq!(error.line, 10);
    assert_eq!(error.column, 3);
    assert!(
        error.message.contains("read-only by index"),
        "{}",
        error.message
    );
    assert!(error.message.contains("UTF-8"), "{}", error.message);
}

#[test]
fn an_index_must_be_an_integer() {
    let source = br#"
unit t;

var
  raw: bytes[8];
  b: byte;
  f: f64;

procedure main(args: const string[1024]);
begin
  raw := "ab";
  f := 1.0;
  b := raw[f];
end;
"#;
    let error = compile(source, "t.ig").expect_err("must not compile");
    assert!(
        error
            .message
            .contains("buffer index must be `int`, `uint` or `byte`"),
        "{}",
        error.message
    );
}

#[test]
fn an_indexed_write_needs_a_byte_value() {
    let source = br#"
unit t;

var
  raw: bytes[8];
  n: int;

procedure main(args: const string[1024]);
begin
  raw := "ab";
  n := 1;
  raw[0] := n;
end;
"#;
    let error = compile(source, "t.ig").expect_err("must not compile");
    assert!(
        error.message.contains("needs a `byte`"),
        "{}",
        error.message
    );
}

#[test]
fn indexed_assignment_to_a_const_parameter_is_rejected() {
    let source = br#"
unit t;

procedure poke(raw: const bytes[8]);
begin
  raw[0] := byte(1);
end;

procedure main(args: const string[1024]);
var
  buf: bytes[8];
begin
  poke(buf);
end;
"#;
    let error = compile(source, "t.ig").expect_err("must not compile");
    assert!(error.message.contains("const"), "{}", error.message);
}

#[test]
fn buffers_compare_with_equality_across_capacities() {
    let source = br#"
unit t;

var
  line: string[64];
  other: string[8];
  raw: bytes[4];
  done: bool;

procedure main(args: const string[1024]);
begin
  line := "help";
  other := "help";
  raw := "he";
  done := line = "help";
  done := line = other;
  done := line <> "bye";
  done := raw = "he";
  done := raw <> "help";
end;
"#;
    let package = compile(source, "t.ig").expect("compiles");
    let ops = opcodes_of(&package);
    assert!(
        ops.contains(&abi::opcodes::CMP_BUFFERS),
        "no cmp_buffers in {ops:?}"
    );
    assert!(ops.contains(&abi::opcodes::NOT), "`<>` negates the compare");
}

#[test]
fn comparing_a_string_with_bytes_is_rejected() {
    let source = br#"
unit t;

var
  text: string[8];
  raw: bytes[8];
  done: bool;

procedure main(args: const string[1024]);
begin
  text := "ab";
  raw := "ab";
  done := text = raw;
end;
"#;
    let error = compile(source, "t.ig").expect_err("must not compile");
    assert!(
        error.message.contains("distinct types"),
        "{}",
        error.message
    );
}

#[test]
fn comparing_a_buffer_with_a_scalar_is_rejected() {
    let source = br#"
unit t;

var
  text: string[8];
  done: bool;

procedure main(args: const string[1024]);
begin
  text := "ab";
  done := text = 1;
end;
"#;
    let error = compile(source, "t.ig").expect_err("must not compile");
    assert!(
        error.message.contains("both operands must be buffers"),
        "{}",
        error.message
    );
}

#[test]
fn buffer_ordering_is_rejected() {
    let source = br#"
unit t;

var
  text: string[8];
  done: bool;

procedure main(args: const string[1024]);
begin
  text := "ab";
  done := text < "b";
end;
"#;
    let error = compile(source, "t.ig").expect_err("must not compile");
    assert!(
        error.message.contains("buffer ordering"),
        "{}",
        error.message
    );
}

#[test]
fn core_len_returns_uint_for_both_buffer_kinds() {
    let good = br#"
unit t;

uses core;

var
  text: string[8];
  raw: bytes[4];
  n: uint;

procedure main(args: const string[1024]);
begin
  text := "ab";
  raw := "abc";
  n := core.len(text);
  n := core.len(raw);
  n := core.len("abcd");
end;
"#;
    let package = compile(good, "t.ig").expect("compiles");
    let header = CodeHeader::from_bytes(&package.code[..40]).expect("header");
    let start = 40 + 24 * header.function_count as usize;
    let instructions = &package.code[start..start + header.code_len as usize];
    let mut seen = Vec::new();
    let mut pc = 0;
    while pc + 4 <= instructions.len() {
        let op = instructions[pc];
        let words = abi::opcodes::instruction_words(op).expect("known opcode") as usize;
        if op == abi::opcodes::BUILTIN {
            seen.push(u32::from_le_bytes(
                instructions[pc + 4..pc + 8].try_into().unwrap(),
            ));
        }
        pc += words * 4;
    }
    assert_eq!(
        seen,
        vec![
            abi::opcodes::builtin::LEN,
            abi::opcodes::builtin::LEN,
            abi::opcodes::builtin::LEN
        ]
    );

    let bad = br#"
unit t;

uses core;

var
  text: string[8];
  n: int;

procedure main(args: const string[1024]);
begin
  text := "ab";
  n := core.len(text);
end;
"#;
    let error = compile(bad, "t.ig").expect_err("uint is not int");
    assert!(error.message.contains("type mismatch"), "{}", error.message);
}

/// Returns `(kind, capacity)` for every `substr` instruction in order.
fn substr_encodings(package: &Owned) -> Vec<(u8, u32)> {
    use abi::opcodes;
    let header = CodeHeader::from_bytes(&package.code[..40]).expect("header");
    let start = 40 + 24 * header.function_count as usize;
    let instructions = &package.code[start..start + header.code_len as usize];
    let mut seen = Vec::new();
    let mut pc = 0;
    while pc + 4 <= instructions.len() {
        let op = instructions[pc];
        let words = opcodes::instruction_words(op).expect("known opcode") as usize;
        if op == opcodes::SUBSTR {
            let kind = instructions[pc + 1];
            let capacity = u32::from_le_bytes(instructions[pc + 4..pc + 8].try_into().unwrap());
            seen.push((kind, capacity));
        }
        pc += words * 4;
    }
    seen
}

#[test]
fn find_emits_the_byte_scan_and_returns_int() {
    let source = br#"
unit t;

uses core;

var
  text: string[16];
  at: int;

procedure main(args: const string[1024]);
begin
  text := "hello";
  at := core.find(text, byte(108));
end;
"#;
    let package = compile(source, "t.ig").expect("compiles");
    let ops = opcodes_of(&package);
    assert!(ops.contains(&abi::opcodes::FIND), "no find in {ops:?}");

    let returned = br#"
unit t;

uses core;

var
  text: string[16];
  n: uint;

procedure main(args: const string[1024]);
begin
  text := "hello";
  n := core.find(text, byte(108));
end;
"#;
    let error = compile(returned, "t.ig").expect_err("find returns int");
    assert!(error.message.contains("type mismatch"), "{}", error.message);

    let wrong_value = br#"
unit t;

uses core;

var
  text: string[16];
  at: int;

procedure main(args: const string[1024]);
begin
  text := "hello";
  at := core.find(text, 108);
end;
"#;
    let error = compile(wrong_value, "t.ig").expect_err("the needle is a byte");
    assert!(
        error.message.contains("expects a `byte` value"),
        "{}",
        error.message
    );
}

#[test]
fn substr_encodes_kind_and_destination_capacity() {
    let source = br#"
unit t;

uses core;

var
  text: string[16];
  part: string[8];
  raw: bytes[16];
  piece: bytes[4];
  n: int;

procedure main(args: const string[1024]);
begin
  text := "hello";
  raw := "abcdef";
  n := core.substr_into(part, text, 0, 2);
  n := core.substr_into(piece, raw, 1, 2);
end;
"#;
    let package = compile(source, "t.ig").expect("compiles");
    assert_eq!(
        substr_encodings(&package),
        vec![
            (abi::opcodes::buffer_kind::STRING, 8),
            (abi::opcodes::buffer_kind::BYTES, 4)
        ]
    );
}

#[test]
fn substr_rejects_mixed_kinds() {
    let source = br#"
unit t;

uses core;

var
  part: string[8];
  raw: bytes[8];
  n: int;

procedure main(args: const string[1024]);
begin
  raw := "ab";
  n := core.substr_into(part, raw, 0, 1);
end;
"#;
    let error = compile(source, "t.ig").expect_err("string and bytes are distinct");
    assert!(error.message.contains("same kind"), "{}", error.message);
    assert!(error.message.contains("differ"), "{}", error.message);
}

#[test]
fn substr_needs_a_writable_destination() {
    let literal = br#"
unit t;

uses core;

var
  text: string[8];
  n: int;

procedure main(args: const string[1024]);
begin
  text := "abc";
  n := core.substr_into("abc", text, 0, 1);
end;
"#;
    let error = compile(literal, "t.ig").expect_err("the destination must be var");
    assert!(
        error.message.contains("must be a variable or parameter"),
        "{}",
        error.message
    );

    let constant = br#"
unit t;

uses core;

procedure poke(dst: const string[8]);
begin
  core.substr_into(dst, "abc", 0, 1);
end;

procedure main(args: const string[1024]);
var
  text: string[8];
begin
  poke(text);
end;
"#;
    let error = compile(constant, "t.ig").expect_err("const is not writable");
    assert!(error.message.contains("const"), "{}", error.message);
}

/// Returns `(kind, capacity)` for every `format_int` instruction in order.
fn format_int_encodings(package: &Owned) -> Vec<(u8, u32)> {
    use abi::opcodes;
    let header = CodeHeader::from_bytes(&package.code[..40]).expect("header");
    let start = 40 + 24 * header.function_count as usize;
    let instructions = &package.code[start..start + header.code_len as usize];
    let mut seen = Vec::new();
    let mut pc = 0;
    while pc + 4 <= instructions.len() {
        let op = instructions[pc];
        let words = opcodes::instruction_words(op).expect("known opcode") as usize;
        if op == opcodes::FORMAT_INT {
            let kind = instructions[pc + 1];
            let capacity = u32::from_le_bytes(instructions[pc + 4..pc + 8].try_into().unwrap());
            seen.push((kind, capacity));
        }
        pc += words * 4;
    }
    seen
}

#[test]
fn format_int_into_checks_types_and_encodes_the_destination() {
    let source = br#"
unit t;

uses core;

var
  text: string[8];
  raw: bytes[8];
  n: int;

procedure main(args: const string[1024]);
begin
  n := core.format_int_into(text, -42, uint(10), uint(4));
  n := core.format_int_into(raw, 255, uint(16), uint(0));
end;
"#;
    let package = compile(source, "t.ig").expect("compiles");
    assert_eq!(
        format_int_encodings(&package),
        vec![
            (abi::opcodes::buffer_kind::STRING, 8),
            (abi::opcodes::buffer_kind::BYTES, 8)
        ]
    );

    let wrong_value = br#"
unit t;

uses core;

var
  text: string[8];
  n: int;

procedure main(args: const string[1024]);
begin
  n := core.format_int_into(text, uint(1), uint(10), uint(2));
end;
"#;
    let error = compile(wrong_value, "t.ig").expect_err("the value is an int");
    assert!(error.message.contains("`int` value"), "{}", error.message);

    let wrong_base = br#"
unit t;

uses core;

var
  text: string[8];
  n: int;

procedure main(args: const string[1024]);
begin
  n := core.format_int_into(text, 1, 10, uint(2));
end;
"#;
    let error = compile(wrong_base, "t.ig").expect_err("the base is a uint");
    assert!(error.message.contains("`uint` base"), "{}", error.message);

    let wrong_width = br#"
unit t;

uses core;

var
  text: string[8];
  n: int;

procedure main(args: const string[1024]);
begin
  n := core.format_int_into(text, 1, uint(10), 2);
end;
"#;
    let error = compile(wrong_width, "t.ig").expect_err("the width is a uint");
    assert!(error.message.contains("`uint` width"), "{}", error.message);

    let wrong_destination = br#"
unit t;

uses core;

var
  n: int;

procedure main(args: const string[1024]);
begin
  n := core.format_int_into("abc", 1, uint(10), uint(2));
end;
"#;
    let error = compile(wrong_destination, "t.ig").expect_err("the destination is var");
    assert!(
        error.message.contains("must be a variable or parameter"),
        "{}",
        error.message
    );
}

#[test]
fn syslib_ports_are_accepted_and_emit_their_builtins() {
    use abi::opcodes::{self, builtin};

    let source = br#"
unit t;

uses core, syslib;

var
  byte_in: uint;
  status: int;

procedure main(args: const string[1024]);
begin
  syslib.port_out8(uint(0x70), uint(0x0A));
  byte_in := syslib.port_in8(uint(0x71));
  status := syslib.port_out8(uint(0x70), uint(1));
end;
"#;
    let package = compile(source, "t.ig").expect("syslib is a known unit");
    let header = CodeHeader::from_bytes(&package.code[..40]).expect("header");
    let start = 40 + 24 * header.function_count as usize;
    let instructions = &package.code[start..start + header.code_len as usize];
    let mut seen = Vec::new();
    let mut pc = 0;
    while pc + 4 <= instructions.len() {
        let op = instructions[pc];
        let words = opcodes::instruction_words(op).expect("known opcode") as usize;
        if op == opcodes::BUILTIN {
            seen.push(u32::from_le_bytes(
                instructions[pc + 4..pc + 8].try_into().unwrap(),
            ));
        }
        pc += words * 4;
    }
    assert_eq!(
        seen,
        vec![builtin::PORT_OUT8, builtin::PORT_IN8, builtin::PORT_OUT8]
    );
}

#[test]
fn port_namespaces_and_types_are_checked() {
    let core_port = br#"
unit t;

uses core;

var
  b: uint;

procedure main(args: const string[1024]);
begin
  b := core.port_in8(uint(0x71));
end;
"#;
    let error = compile(core_port, "t.ig").expect_err("ports live in syslib");
    assert!(
        error
            .message
            .contains("undefined procedure `core.port_in8`"),
        "{}",
        error.message
    );

    let syslib_len = br#"
unit t;

uses core, syslib;

var
  text: string[8];
  n: uint;

procedure main(args: const string[1024]);
begin
  text := "a";
  n := syslib.len(text);
end;
"#;
    let error = compile(syslib_len, "t.ig").expect_err("len lives in core");
    assert!(
        error.message.contains("undefined procedure `syslib.len`"),
        "{}",
        error.message
    );

    let wrong_port = br#"
unit t;

uses core, syslib;

var
  b: uint;

procedure main(args: const string[1024]);
begin
  b := syslib.port_in8(0x71);
end;
"#;
    let error = compile(wrong_port, "t.ig").expect_err("a port is a uint");
    assert!(error.message.contains("`uint` port"), "{}", error.message);

    let wrong_value = br#"
unit t;

uses core, syslib;

var
  r: int;

procedure main(args: const string[1024]);
begin
  r := syslib.port_out8(uint(0x70), 300);
end;
"#;
    let error = compile(wrong_value, "t.ig").expect_err("a port value is a uint");
    assert!(error.message.contains("`uint` value"), "{}", error.message);
}

/// The exact diagnostic text expected when the nesting limit is reached.
fn nesting_message() -> String {
    format!("nesting too deep (limit {})", compiler::MAX_NESTING)
}

/// Runs a nesting test on a thread with enough stack for a debug build.
///
/// An unoptimised frame is much larger than a release frame (the kernel builds
/// release), so the default 2 MiB test-thread stack is not the stack these
/// bounds describe. 64 MiB is test scaffolding only; the kernel bound is the
/// 256 KiB guarded stack in `arch/x86_64/src/stack.rs`.
fn with_test_stack<T: Send + 'static>(body: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(body)
        .expect("spawn test thread")
        .join()
        .expect("test thread panicked")
}

/// `depth` nested `begin`/`end` statements around an empty body.
fn nested_blocks(depth: usize) -> String {
    let mut source = String::from("unit t;\n\nprocedure main(args: const string[1024]);\nbegin\n");
    for _ in 0..depth {
        source.push_str("begin\n");
    }
    for _ in 0..depth {
        source.push_str("end;\n");
    }
    source.push_str("end;\n");
    source
}

/// `depth` nested `if true then` statements around an empty block.
fn nested_ifs(depth: usize) -> String {
    let mut source = String::from("unit t;\n\nprocedure main(args: const string[1024]);\nbegin\n");
    for _ in 0..depth {
        source.push_str("if true then\n");
    }
    source.push_str("begin end;\n");
    for _ in 0..depth {
        source.push_str(";\n");
    }
    source.push_str("end;\n");
    source
}

/// `depth` nested `while true do` statements around an empty block.
fn nested_whiles(depth: usize) -> String {
    let mut source = String::from("unit t;\n\nprocedure main(args: const string[1024]);\nbegin\n");
    for _ in 0..depth {
        source.push_str("while true do\n");
    }
    source.push_str("begin end;\n");
    for _ in 0..depth {
        source.push_str(";\n");
    }
    source.push_str("end;\n");
    source
}

#[test]
fn blocks_nested_to_the_limit_compile_and_one_more_is_rejected() {
    with_test_stack(|| {
        let limit = compiler::MAX_NESTING as usize;
        compile(nested_blocks(limit).as_bytes(), "deep.ig").expect("the limit itself is accepted");

        let error = compile(nested_blocks(limit + 1).as_bytes(), "deep.ig")
            .expect_err("one level past the limit is rejected");
        assert!(
            error.message.contains(&nesting_message()),
            "{}",
            error.message
        );
        assert!(error.line > 0 && error.column > 0);
    });
}

#[test]
fn parentheses_nested_to_the_limit_compile_and_one_more_is_rejected() {
    // A `const` value is parsed by one `expression` call, so `n` parentheses
    // reach nesting `n + 1`.
    with_test_stack(|| {
        let limit = compiler::MAX_NESTING as usize;
        let source = |depth: usize| {
            format!(
                "unit t;\nconst C = {open}1{close};\n\nprocedure main(args: const string[1024]);\nbegin\nend;\n",
                open = "(".repeat(depth),
                close = ")".repeat(depth),
            )
        };

        compile(source(limit - 1).as_bytes(), "deep.ig").expect("the limit itself is accepted");

        let error = compile(source(limit).as_bytes(), "deep.ig")
            .expect_err("one level past the limit is rejected");
        assert!(
            error.message.contains(&nesting_message()),
            "{}",
            error.message
        );
    });
}

#[test]
fn nested_if_and_while_statements_are_bounded() {
    with_test_stack(|| {
        let limit = compiler::MAX_NESTING as usize;
        // A condition is parsed one level below its `if`/`while`, so one fewer
        // statement level fits; deeper programs are rejected, never crashed on.
        compile(nested_ifs(limit - 1).as_bytes(), "deep.ig").expect("deep `if` is accepted");
        compile(nested_whiles(limit - 1).as_bytes(), "deep.ig").expect("deep `while` is accepted");

        let error = compile(nested_ifs(limit).as_bytes(), "deep.ig").expect_err("bounded `if`");
        assert!(
            error.message.contains(&nesting_message()),
            "{}",
            error.message
        );
        let error =
            compile(nested_whiles(limit).as_bytes(), "deep.ig").expect_err("bounded `while`");
        assert!(
            error.message.contains(&nesting_message()),
            "{}",
            error.message
        );
    });
}

#[test]
fn left_deep_operator_chains_are_bounded_in_codegen() {
    // `1 + 1 + ...` is parsed by a loop, so the AST is deep even though the
    // parser never recurses; codegen bounds its own walk.
    with_test_stack(|| {
        let limit = compiler::MAX_NESTING as usize;
        let source = |terms: usize| {
            let chain = vec!["1"; terms].join(" + ");
            format!(
                "unit t;\nvar x: int;\n\nprocedure main(args: const string[1024]);\nbegin\n  x := {chain};\nend;\n"
            )
        };

        compile(source(limit).as_bytes(), "deep.ig").expect("the limit itself is accepted");

        let error = compile(source(limit + 1).as_bytes(), "deep.ig")
            .expect_err("one term past the limit is rejected");
        assert!(
            error.message.contains(&nesting_message()),
            "{}",
            error.message
        );
    });
}

#[test]
fn left_deep_const_and_buffer_chains_are_bounded() {
    with_test_stack(|| {
        let limit = compiler::MAX_NESTING as usize;

        let chain = vec!["1"; limit + 1].join(" + ");
        let constant = format!(
            "unit t;\nconst C = {chain};\n\nprocedure main(args: const string[1024]);\nbegin\nend;\n"
        );
        let error = compile(constant.as_bytes(), "deep.ig").expect_err("bounded const walk");
        assert!(
            error.message.contains(&nesting_message()),
            "{}",
            error.message
        );

        let parts = vec!["\"a\""; limit + 1].join(" + ");
        let buffer = format!(
            "unit t;\nvar s: string[256];\n\nprocedure main(args: const string[1024]);\nbegin\n  s := {parts};\nend;\n"
        );
        let error = compile(buffer.as_bytes(), "deep.ig").expect_err("bounded concat walk");
        assert!(
            error.message.contains(&nesting_message()),
            "{}",
            error.message
        );
    });
}
