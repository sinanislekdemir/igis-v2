//! Compiler-level tests: packages, determinism and diagnostics.

use abi::{
    CodeHeader, ManifestHeader, ABI_VERSION, CODE_MAGIC, LINE_ENTRY_SIZE, PROFILE_X86_64_LE,
};

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

uses nosuchunit;

procedure main(args: const string[1024]);
begin
end;
"#;
    let error = compile(source, "t.ig").expect_err("must not compile");
    assert!(
        error.message.contains("unknown unit `nosuchunit`"),
        "{}",
        error.message
    );
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
  st: Status;
  id: int;
  diag: Diagnostic;
  reason: ExitReason;

procedure main(args: const string[1024]);
begin
  st := core.spawn("child", "hello", diag, id);
  st := core.wait(id, reason);
  st := core.kill(id);
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
fn run_emits_its_builtin_id() {
    use abi::opcodes::{self, builtin};

    let source = br#"
unit t;

uses core;

var
  st: Status;
  diag: Diagnostic;
  reason: ExitReason;

procedure main(args: const string[1024]);
begin
  st := core.run("child", "hello", diag, reason);
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
    assert_eq!(seen, vec![builtin::RUN]);
}

#[test]
fn run_argument_count_and_out_parameter_types_are_checked() {
    let wrong_count = br#"
unit t;

uses core;

var
  diag: Diagnostic;

procedure main(args: const string[1024]);
begin
  core.run("child", "hello", diag);
end;
"#;
    let error = compile(wrong_count, "t.ig").expect_err("must not compile");
    assert!(
        error.message.contains("`core.run` expects 4 arguments"),
        "{}",
        error.message
    );

    let wrong_diag = br#"
unit t;

uses core;

var
  diag: Diagnostic;
  reason: ExitReason;

procedure main(args: const string[1024]);
begin
  core.run("child", "hello", reason, reason);
end;
"#;
    let error = compile(wrong_diag, "t.ig").expect_err("must not compile");
    assert!(
        error
            .message
            .contains("`core.run` expects `var diag: Diagnostic`"),
        "{}",
        error.message
    );

    let wrong_reason = br#"
unit t;

uses core;

var
  diag: Diagnostic;

procedure main(args: const string[1024]);
begin
  core.run("child", "hello", diag, diag);
end;
"#;
    let error = compile(wrong_reason, "t.ig").expect_err("must not compile");
    assert!(
        error
            .message
            .contains("`core.run` expects `var reason: ExitReason`"),
        "{}",
        error.message
    );
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
        error.message.contains("`core.spawn` expects 4 arguments"),
        "{}",
        error.message
    );

    let wrong_type = br#"
unit t;

uses core;

var
  u: uint;
  reason: ExitReason;

procedure main(args: const string[1024]);
begin
  u := uint(1);
  core.wait(u, reason);
end;
"#;
    let error = compile(wrong_type, "t.ig").expect_err("must not compile");
    assert!(error.message.contains("task id"), "{}", error.message);

    let wrong_out_parameter = br#"
unit t;

uses core;

var
  st: Status;
  id: int;
  diag: Diagnostic;
  code: uint;

procedure main(args: const string[1024]);
begin
  st := core.spawn("child", "hello", diag, id);
  st := core.wait(id, code);
end;
"#;
    let error = compile(wrong_out_parameter, "t.ig").expect_err("reason is an ExitReason");
    assert!(
        error.message.contains("`var reason` expects a record"),
        "{}",
        error.message
    );
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

/// Builds a minimal unit that constructs `ExitReason` and matches it with
/// `case`, proving the predeclared sum type is usable like any other.
const EXIT_REASON_CASE: &[u8] = br#"
unit t;

var
  reason: ExitReason;
  st: Status;
  ordinal: FaultCode;

procedure main(args: const string[1024]);
begin
  st := Ok;
  reason := Ok;
  ordinal := FaultCode(4);
  if ordinal = FaultCode.DivideByZero then
    st := Ok;
  reason := Faulted(ordinal);
  case reason of
    Ok: st := Ok
    Killed: st := NotFound
    Faulted(code):
    begin
      if code = FaultCode.DivideByZero then
        st := Ok
      else
        st := Invalid;
    end
  end;
  reason := Killed;
end;
"#;

#[test]
fn sums_construct_copy_and_match() {
    let source = br#"
unit t;

uses core;

type Token = Ident(name: string[8]) | Number(value: int) | Eof;

var
  token: Token;
  copy: Token;

procedure show(t: const Token);
begin
  case t of
    Ident(name):
    begin
      core.write(name);
      core.write("\n");
    end
    Number(value): core.write("number\n")
    Eof: core.write("eof\n")
  end;
end;

procedure main(args: const string[1024]);
begin
  token := Number(7);
  copy := token;
  token := Ident("x" + "y");
  show(copy);
  show(token);
  token := Eof;
  show(token);
end;
"#;
    let package = compile(source, "t.ig").expect("sum types compile");
    let ops = opcodes_of(&package);
    for (op, name) in [
        (abi::opcodes::CONST, "const"),
        (abi::opcodes::STORE_REF, "store_ref"),
        (abi::opcodes::REF_OFFSET, "ref_offset"),
        (abi::opcodes::COPY_REFS, "copy_refs"),
        (abi::opcodes::LOAD_REF, "load_ref"),
        (abi::opcodes::EQ, "eq"),
        (abi::opcodes::JZ, "jz"),
        (abi::opcodes::JMP, "jmp"),
    ] {
        assert!(ops.contains(&op), "no {name} in {ops:?}");
    }
}

#[test]
fn predeclared_exit_reason_is_a_usable_sum_type() {
    let package = compile(EXIT_REASON_CASE, "t.ig").expect("ExitReason compiles");
    let ops = opcodes_of(&package);
    // A foreign `FaultCode` variant is a plain ordinal constant.
    assert!(ops.contains(&abi::opcodes::EQ), "no tag/payload comparison");
}

#[test]
fn case_must_be_exhaustive() {
    let source = br#"
unit t;

type Token = A | B;

var
  t: Token;

procedure main(args: const string[1024]);
begin
  t := A;
  case t of
    A: t := B
  end;
end;
"#;
    let error = compile(source, "t.ig").expect_err("a missing arm must not compile");
    assert!(
        error.message.contains("non-exhaustive `case`") && error.message.contains("`B`"),
        "{}",
        error.message
    );
}

#[test]
fn a_duplicate_case_arm_is_rejected() {
    let source = br#"
unit t;

type Token = A | B;

var
  t: Token;

procedure main(args: const string[1024]);
begin
  t := A;
  case t of
    A: t := A
    A: t := B
    B: t := A
  end;
end;
"#;
    let error = compile(source, "t.ig").expect_err("a duplicate arm must not compile");
    assert!(
        error.message.contains("duplicate arm for variant `A`"),
        "{}",
        error.message
    );
}

#[test]
fn an_unknown_case_variant_is_rejected() {
    let source = br#"
unit t;

type Token = A | B;

var
  t: Token;

procedure main(args: const string[1024]);
begin
  t := A;
  case t of
    A: t := A
    B: t := B
    Missing: t := A
  end;
end;
"#;
    let error = compile(source, "t.ig").expect_err("an unknown variant must not compile");
    assert!(
        error.message.contains("has no variant `Missing`"),
        "{}",
        error.message
    );
}

#[test]
fn a_payload_type_mismatch_is_rejected() {
    let source = br#"
unit t;

type Token = Ident(name: string[8]) | Number(value: int) | Eof;

var
  token: Token;

procedure main(args: const string[1024]);
begin
  token := Number(true);
end;
"#;
    let error = compile(source, "t.ig").expect_err("the payload must match exactly");
    assert!(
        error.message.contains("expected `int`, found `bool`"),
        "{}",
        error.message
    );
}

#[test]
fn a_nullary_variant_with_a_payload_is_rejected() {
    let source = br#"
unit t;

type Token = A | B;

var
  token: Token;

procedure main(args: const string[1024]);
begin
  token := A(1);
end;
"#;
    let error = compile(source, "t.ig").expect_err("A takes no payload");
    assert!(
        error.message.contains("carries no payload"),
        "{}",
        error.message
    );
}

#[test]
fn a_payload_variant_without_a_payload_is_rejected() {
    let source = br#"
unit t;

type Token = Ident(name: string[8]) | Eof;

var
  token: Token;

procedure main(args: const string[1024]);
begin
  token := Ident;
end;
"#;
    let error = compile(source, "t.ig").expect_err("Ident needs a payload");
    assert!(
        error.message.contains("carries a payload"),
        "{}",
        error.message
    );
}

#[test]
fn a_variant_of_another_sum_type_is_rejected() {
    let source = br#"
unit t;

type Token = Ident(name: string[8]) | Eof;
type Expr = Literal(value: int) | Empty;

var
  token: Token;

procedure main(args: const string[1024]);
begin
  token := Literal(1);
end;
"#;
    let error = compile(source, "t.ig").expect_err("Literal belongs to Expr");
    assert!(
        error
            .message
            .contains("`Literal` is a variant of `Expr`, not of `Token`"),
        "{}",
        error.message
    );
}

#[test]
fn a_sum_value_cannot_be_copied_between_sum_types() {
    let source = br#"
unit t;

type Token = Ident(name: string[8]) | Eof;
type Expr = Literal(value: int) | Empty;

var
  token: Token;
  expr: Expr;

procedure main(args: const string[1024]);
begin
  token := Ident("x");
  expr := Literal(1);
  token := expr;
end;
"#;
    let error = compile(source, "t.ig").expect_err("sum types are nominal");
    assert!(
        error
            .message
            .contains("type mismatch: expected `Token`, found `Expr`"),
        "{}",
        error.message
    );
}

#[test]
fn a_sum_value_is_not_a_scalar() {
    let source = br#"
unit t;

type Token = A | B;

var
  token: Token;
  n: int;

procedure main(args: const string[1024]);
begin
  token := A;
  n := token;
end;
"#;
    let error = compile(source, "t.ig").expect_err("a sum is not a scalar");
    assert!(
        error.message.contains("cannot be used as a scalar value")
            && error.message.contains("match it with `case`"),
        "{}",
        error.message
    );
}

#[test]
fn case_needs_a_sum_subject() {
    let source = br#"
unit t;

var
  n: int;

procedure main(args: const string[1024]);
begin
  n := 1;
  case n of
    A: n := 2
  end;
end;
"#;
    let error = compile(source, "t.ig").expect_err("`case` needs a sum");
    assert!(
        error.message.contains("`case` needs a sum value"),
        "{}",
        error.message
    );
}

#[test]
fn sum_variants_share_one_namespace() {
    let source = br#"
unit t;

type One = Foo | Bar;
type Two = Foo | Baz;

procedure main(args: const string[1024]);
begin
end;
"#;
    let error = compile(source, "t.ig").expect_err("variants are globally unique");
    assert!(
        error.message.contains("duplicate variant `Foo`"),
        "{}",
        error.message
    );
}

#[test]
fn a_payloadless_arm_needs_no_binding() {
    let source = br#"
unit t;

type Token = A | B(value: int);

var
  token: Token;

procedure main(args: const string[1024]);
begin
  token := A;
  case token of
    A(x): token := A
    B(value): token := B(value)
  end;
end;
"#;
    let error = compile(source, "t.ig").expect_err("A has no payload to bind");
    assert!(
        error.message.contains("carries no payload"),
        "{}",
        error.message
    );
}

#[test]
fn a_payload_arm_must_bind_its_payload() {
    let source = br#"
unit t;

type Token = A | B(value: int);

var
  token: Token;

procedure main(args: const string[1024]);
begin
  token := A;
  case token of
    A: token := A
    B: token := A
  end;
end;
"#;
    let error = compile(source, "t.ig").expect_err("B carries a payload");
    assert!(
        error.message.contains("carries a payload"),
        "{}",
        error.message
    );
}

#[test]
fn predeclared_sum_and_enum_names_cannot_be_declared() {
    for source in [
        b"unit t;\n\ntype ExitReason = A | B;\n\nprocedure main(args: const string[1024]);\nbegin\nend;\n" as &[u8],
        b"unit t;\n\ntype FaultCode = (A, B);\n\nprocedure main(args: const string[1024]);\nbegin\nend;\n",
    ] {
        let error = compile(source, "t.ig").expect_err("predeclared names are reserved");
        assert!(
            error.message.contains("is predeclared by the language"),
            "{}",
            error.message
        );
    }
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

/// Returns the variant-count immediate of every `enum_cast` in order.
fn enum_cast_counts(package: &Owned) -> Vec<u32> {
    use abi::opcodes;
    let header = CodeHeader::from_bytes(&package.code[..40]).expect("header");
    let start = 40 + 24 * header.function_count as usize;
    let instructions = &package.code[start..start + header.code_len as usize];
    let mut seen = Vec::new();
    let mut pc = 0;
    while pc + 4 <= instructions.len() {
        let op = instructions[pc];
        let words = opcodes::instruction_words(op).expect("known opcode") as usize;
        if op == opcodes::ENUM_CAST {
            seen.push(u32::from_le_bytes(
                instructions[pc + 4..pc + 8].try_into().unwrap(),
            ));
        }
        pc += words * 4;
    }
    seen
}

#[test]
fn enums_compile_with_ordinals_comparisons_and_casts() {
    let source = br#"
unit t;

type Color = (Red, Green, Blue);

var
  c: Color;
  other: Color;
  n: int;
  same: bool;

procedure main(args: const string[1024]);
begin
  c := Green;
  n := int(c);
  c := Color(2);
  same := c = Blue;
  same := c <> Red;
  same := Red < Blue;
  same := Red <= Green;
  same := Blue > Red;
  same := Blue >= Green;
end;
"#;
    let package = compile(source, "t.ig").expect("enums compile");
    assert_eq!(enum_cast_counts(&package), vec![3]);
}

#[test]
fn status_is_predeclared_and_its_variants_are_globals() {
    let source = br#"
unit t;

var
  st: Status;
  ok: bool;
  n: int;

procedure main(args: const string[1024]);
begin
  st := Ok;
  ok := st = Ok;
  n := int(WouldBlock);
  st := Status(7);
  ok := st = IoError;
  ok := st <> Ok;
  ok := Ok < Full;
end;
"#;
    let package = compile(source, "t.ig").expect("Status is predeclared");
    // One cast into the predeclared type, whose variant count is eight.
    assert_eq!(enum_cast_counts(&package), vec![8]);
}

#[test]
fn a_duplicate_variant_is_rejected() {
    let source = br#"
unit t;

type Color = (Red, Green, Red);

procedure main(args: const string[1024]);
begin
end;
"#;
    let error = compile(source, "t.ig").expect_err("must not compile");
    assert!(
        error.message.contains("duplicate variant `Red`"),
        "{}",
        error.message
    );
    assert_eq!(error.line, 4);
}

#[test]
fn a_variant_collides_with_another_enums_variant() {
    let source = br#"
unit t;

type Left = (A, B);
type Right = (B, C);

procedure main(args: const string[1024]);
begin
end;
"#;
    let error = compile(source, "t.ig").expect_err("must not compile");
    assert!(
        error.message.contains("duplicate variant `B`"),
        "{}",
        error.message
    );
}

#[test]
fn a_second_type_with_the_same_name_is_rejected() {
    let source = br#"
unit t;

type Color = (Red);
type Color = (Blue);

procedure main(args: const string[1024]);
begin
end;
"#;
    let error = compile(source, "t.ig").expect_err("must not compile");
    assert!(
        error
            .message
            .contains("duplicate declaration of type `Color`"),
        "{}",
        error.message
    );
}

#[test]
fn user_declared_status_is_rejected() {
    let source = br#"
unit t;

type Status = (Ok, Bad);

procedure main(args: const string[1024]);
begin
end;
"#;
    let error = compile(source, "t.ig").expect_err("must not compile");
    assert!(
        error.message.contains("`Status` is predeclared"),
        "{}",
        error.message
    );
}

#[test]
fn enums_and_ints_do_not_mix_without_a_cast() {
    let write_int = br#"
unit t;

type Color = (Red, Green);

var
  n: int;

procedure main(args: const string[1024]);
begin
  n := Red;
end;
"#;
    let error = compile(write_int, "t.ig").expect_err("enum is not int");
    assert!(
        error
            .message
            .contains("type mismatch: expected `int`, found `Color`"),
        "{}",
        error.message
    );

    let compare_int = br#"
unit t;

type Color = (Red, Green);

var
  same: bool;

procedure main(args: const string[1024]);
begin
  same := 1 = Red;
end;
"#;
    let error = compile(compare_int, "t.ig").expect_err("enum is not int");
    assert!(
        error.message.contains("same enum type"),
        "{}",
        error.message
    );

    let mixed_enums = br#"
unit t;

type Color = (Red, Green);
type Shape = (Circle, Square);

var
  same: bool;

procedure main(args: const string[1024]);
begin
  same := Red = Circle;
end;
"#;
    let error = compile(mixed_enums, "t.ig").expect_err("nominal enums do not mix");
    assert!(
        error.message.contains("same enum type"),
        "{}",
        error.message
    );

    let arithmetic = br#"
unit t;

type Color = (Red, Green);

var
  c: Color;

procedure main(args: const string[1024]);
begin
  c := Red + Green;
end;
"#;
    let error = compile(arithmetic, "t.ig").expect_err("enums have no arithmetic");
    assert!(
        error.message.contains("ordinal ordering"),
        "{}",
        error.message
    );
}

#[test]
fn enum_casts_are_checked() {
    let from_uint = br#"
unit t;

type Color = (Red, Green);

var
  c: Color;
  u: uint;

procedure main(args: const string[1024]);
begin
  u := uint(0);
  c := Color(u);
end;
"#;
    let error = compile(from_uint, "t.ig").expect_err("the ordinal is an int");
    assert!(
        error.message.contains("needs an `int` ordinal"),
        "{}",
        error.message
    );

    let to_float = br#"
unit t;

type Color = (Red, Green);

var
  f: f64;
  c: Color;

procedure main(args: const string[1024]);
begin
  c := Red;
  f := f64(c);
end;
"#;
    let error = compile(to_float, "t.ig").expect_err("cast through int first");
    assert!(
        error.message.contains("convert it with `int(x)` first"),
        "{}",
        error.message
    );

    let to_int = br#"
unit t;

type Color = (Red, Green);

var
  n: int;
  c: Color;

procedure main(args: const string[1024]);
begin
  c := Blue;
  n := int(c);
end;
"#;
    let error = compile(to_int, "t.ig").expect_err("Blue is not a Color variant");
    assert!(
        error.message.contains("unknown identifier `Blue`"),
        "{}",
        error.message
    );
}

#[test]
fn a_variant_is_not_assignable() {
    let source = br#"
unit t;

procedure main(args: const string[1024]);
begin
  Ok := Full;
end;
"#;
    let error = compile(source, "t.ig").expect_err("must not compile");
    assert!(
        error.message.contains("cannot assign to enum variant `Ok`"),
        "{}",
        error.message
    );
}

#[test]
fn records_layout_in_declaration_order() {
    use abi::{ManifestHeader, MANIFEST_HEADER_SIZE};

    // `name: string[8]` is 16 bytes (`8 + 8`); `at: Point` is 16; `count` is 8.
    let source = br#"
unit t;

type Point = record x, y: int; end;
type Mixed = record name: string[8]; at: Point; count: int; end;

var
  m: Mixed;

procedure main(args: const string[1024]);
begin
  m.count := 1;
  m.at.x := 2;
  m.at.y := 3;
  m.name := "n";
end;
"#;
    let package = compile(source, "t.ig").expect("records compile");
    let header = ManifestHeader::from_bytes(&package.manifest[..MANIFEST_HEADER_SIZE])
        .expect("manifest header");
    assert_eq!(header.state_size, 40, "fields in order, each aligned to 8");
}

#[test]
fn records_accept_nested_fields_buffer_fields_and_copies() {
    let source = br#"
unit t;

type Point = record x, y: int; end;
type Named = record label: string[8]; at: Point; end;

var
  n: Named;
  p: Point;
  copy: Named;

procedure set(q: var Point; x: int; y: int);
begin
  q.x := x;
  q.y := y;
end;

procedure main(args: const string[1024]);
begin
  set(p, 3, 4);
  n.at := p;
  n.label := "n";
  copy := n;
  copy.at := n.at;
  p := copy.at;
end;
"#;
    compile(source, "t.ig").expect("records compile");
}

#[test]
fn unknown_record_field_is_reported() {
    let source = br#"
unit t;

type Point = record x, y: int; end;

var
  p: Point;

procedure main(args: const string[1024]);
begin
  p.z := 1;
end;
"#;
    let error = compile(source, "t.ig").expect_err("must not compile");
    assert!(
        error.message.contains("has no field `z`"),
        "{}",
        error.message
    );
}

#[test]
fn record_type_mismatch_is_reported() {
    let source = br#"
unit t;

type A = record x: int; end;
type B = record x: int; end;

var
  a: A;
  b: B;

procedure main(args: const string[1024]);
begin
  a := b;
end;
"#;
    let error = compile(source, "t.ig").expect_err("records are nominal");
    assert!(error.message.contains("type mismatch"), "{}", error.message);
}

#[test]
fn writing_a_const_record_field_is_reported() {
    let source = br#"
unit t;

type Point = record x, y: int; end;

procedure move(q: const Point);
begin
  q.x := 1;
end;

procedure main(args: const string[1024]);
begin
end;
"#;
    let error = compile(source, "t.ig").expect_err("const records are read-only");
    assert!(
        error
            .message
            .contains("cannot assign to a field of `const` parameter"),
        "{}",
        error.message
    );
}

#[test]
fn a_record_type_must_be_declared_before_use() {
    let source = br#"
unit t;

type Outer = record inner: Inner; end;
type Inner = record x: int; end;

procedure main(args: const string[1024]);
begin
end;
"#;
    let error = compile(source, "t.ig").expect_err("no forward references");
    assert!(
        error.message.contains("unknown type `Inner`"),
        "{}",
        error.message
    );
}

#[test]
fn defining_a_predeclared_record_is_reported() {
    let diagnostic = br#"
unit t;

type Diagnostic = record x: int; end;

procedure main(args: const string[1024]);
begin
end;
"#;
    let error = compile(diagnostic, "t.ig").expect_err("Diagnostic is predeclared");
    assert!(
        error.message.contains("`Diagnostic` is predeclared"),
        "{}",
        error.message
    );

    let exit_reason = br#"
unit t;

type ExitReason = record x: int; end;

procedure main(args: const string[1024]);
begin
end;
"#;
    let error = compile(exit_reason, "t.ig").expect_err("ExitReason is predeclared");
    assert!(
        error.message.contains("`ExitReason` is predeclared"),
        "{}",
        error.message
    );
}

#[test]
fn a_record_name_is_not_a_constructor() {
    let source = br#"
unit t;

type Point = record x, y: int; end;

var
  p: Point;

procedure main(args: const string[1024]);
begin
  p := Point(1);
end;
"#;
    let error = compile(source, "t.ig").expect_err("records have no constructors");
    assert!(
        error.message.contains("is not a record constructor"),
        "{}",
        error.message
    );
}

#[test]
fn a_records_buffer_field_is_usable_as_a_buffer() {
    let source = br#"
unit t;

type Named = record label: string[8]; end;

var
  n: Named;
  size: uint;

procedure main(args: const string[1024]);
begin
  n.label := "hi";
  core.write(n.label);
  size := core.len(n.label);
end;
"#;
    compile(source, "t.ig").expect("buffer fields work");
}

// ---- multi-unit compilation and the embedded stdlib ----------------------

/// Compiles a unit with the "library only" entry point, which does not require
/// `main`.
#[allow(clippy::result_large_err)]
fn compile_library(source: &[u8], path: &str) -> Result<Owned, compiler::Diagnostic> {
    let mut arena = vec![0u8; compiler::MAX_ARENA_BYTES];
    let mut code = vec![0u8; compiler::MAX_CODE_BYTES];
    let mut manifest = vec![0u8; compiler::MAX_MANIFEST_BYTES];
    let package =
        compiler::compile_library_into(source, path, &mut arena, &mut code, &mut manifest)?;
    Ok(Owned {
        manifest: package.manifest.to_vec(),
        code: package.code.to_vec(),
    })
}

/// Every line-table unit id in a package.
fn line_units(code: &[u8]) -> Vec<u32> {
    let header = CodeHeader::from_bytes(&code[..abi::CODE_HEADER_SIZE]).expect("header");
    let mut offset = abi::CODE_HEADER_SIZE + header.function_count as usize * abi::FUNCTION_SIZE;
    offset = offset.next_multiple_of(8);
    offset += header.code_len as usize;
    offset = offset.next_multiple_of(8);
    offset += header.const_len as usize;
    offset = offset.next_multiple_of(8);
    let count = header.line_len as usize / LINE_ENTRY_SIZE;
    (0..count)
        .map(|index| {
            let at = offset + index * LINE_ENTRY_SIZE + 4;
            u32::from_le_bytes([code[at], code[at + 1], code[at + 2], code[at + 3]])
        })
        .collect()
}

fn manifest_unit(package: &Owned) -> String {
    let header = ManifestHeader::from_bytes(&package.manifest[..abi::MANIFEST_HEADER_SIZE])
        .expect("manifest header");
    let start = abi::MANIFEST_HEADER_SIZE;
    let end = start + header.unit_name_len as usize;
    String::from_utf8(package.manifest[start..end].to_vec()).expect("unit name")
}

const STDAPP: &[u8] = include_bytes!("../../samples/lang/stdlib.ig");

#[test]
fn the_embedded_stdlib_compiles_on_its_own() {
    let package = compile_library(compiler::STDLIB_SOURCE.as_bytes(), compiler::STDLIB_PATH)
        .expect("the stdlib source is valid on its own");
    let header = CodeHeader::from_bytes(&package.code[..abi::CODE_HEADER_SIZE]).expect("header");
    assert_eq!(manifest_unit(&package), "stdlib");
    assert_eq!(
        header.function_count, 7,
        "trim_into, split_once, parse_int_into, starts_with, contains_byte, split_all, drain_into"
    );
    assert_eq!(header.entry, 0);
    assert_eq!(header.state_size, 0, "the stdlib declares no state");
}

#[test]
fn a_broken_library_is_a_normal_diagnostic_naming_stdlib() {
    let broken = br#"
unit stdlib;
uses core;

function oops(src: const string[0]): int;
begin
  return src[0];
end;
"#;
    let error = compile_library(broken, compiler::STDLIB_PATH).expect_err("must fail");
    assert_eq!(error.unit, "stdlib");
    assert_eq!(error.path, compiler::STDLIB_PATH);
    assert!(
        error.message.contains("returns `int`, found `byte`"),
        "{}",
        error.message
    );
}

#[test]
fn an_application_importing_stdlib_shares_one_package() {
    let package = compile(STDAPP, "stdlib.ig").expect("compiles");
    let header = CodeHeader::from_bytes(&package.code[..abi::CODE_HEADER_SIZE]).expect("header");
    assert!(
        header.function_count >= 5 + 3,
        "library and application functions"
    );
    assert!(header.entry >= 5, "entry is the application's main");
    let units = line_units(&package.code);
    assert!(units.contains(&0), "application lines");
    assert!(units.contains(&1), "stdlib lines");
    assert_eq!(manifest_unit(&package), "stdlib_demo");
}

#[test]
fn calling_stdlib_without_importing_it_is_rejected() {
    let source = br#"
unit t;

var
  s: string[16];
  n: int;

procedure main(args: const string[1024]);
begin
  n := stdlib.trim_into(s, args);
end;
"#;
    let error = compile(source, "t.ig").expect_err("stdlib is not imported");
    assert_eq!(error.unit, "t");
    assert!(
        error.message.contains("`stdlib` is not imported"),
        "{}",
        error.message
    );
}

#[test]
fn an_unknown_stdlib_name_is_rejected() {
    let source = br#"
unit t;
uses stdlib;

procedure main(args: const string[1024]);
begin
  stdlib.no_such_helper();
end;
"#;
    let error = compile(source, "t.ig").expect_err("unknown helper");
    assert!(
        error
            .message
            .contains("undefined procedure `stdlib.no_such_helper`"),
        "{}",
        error.message
    );
}

#[test]
fn compile_cost_of_the_stdlib_is_bounded_and_reported() {
    // The same application with and without `uses stdlib` isolates the
    // library's code and scratch cost.
    let without = br#"
unit cost;
uses core;
procedure main(args: const string[1024]);
begin
  core.write("cost\n");
end;
"#;
    let with = br#"
unit cost;
uses core, stdlib;
var
  s: string[256];
  n: int;
procedure main(args: const string[1024]);
begin
  n := stdlib.trim_into(s, args);
  core.write(s);
end;
"#;
    let plain = compile(without, "cost.ig").expect("compiles");
    let rich = compile(with, "cost.ig").expect("compiles");
    let library = compile_library(compiler::STDLIB_SOURCE.as_bytes(), compiler::STDLIB_PATH)
        .expect("compiles");

    // Smallest arena each build accepts, found by bisection. This is the real
    // scratch cost because `compile_into` caps the arena to its slice length.
    let minimum_arena = |source: &[u8]| -> usize {
        let mut low = 4 * 1024usize;
        let mut high = compiler::MAX_ARENA_BYTES;
        while low < high {
            let mid = (low + high) / 2;
            let mut arena = vec![0u8; mid];
            let mut code = vec![0u8; compiler::MAX_CODE_BYTES];
            let mut manifest = vec![0u8; compiler::MAX_MANIFEST_BYTES];
            match compiler::compile_into(source, "cost.ig", &mut arena, &mut code, &mut manifest) {
                Ok(_) => high = mid,
                Err(error) if error.status == abi::Status::CapacityExceeded => low = mid + 1,
                Err(error) => panic!("unexpected diagnostic: {error}"),
            }
        }
        low
    };
    let plain_arena = minimum_arena(without);
    let rich_arena = minimum_arena(with);
    let library_header =
        CodeHeader::from_bytes(&library.code[..abi::CODE_HEADER_SIZE]).expect("library header");
    let rich_header = CodeHeader::from_bytes(&rich.code[..abi::CODE_HEADER_SIZE]).expect("header");
    println!(
        "stdlib cost: source {} B, code {} B (package {} B), stack {} B, state {} B, arena {} B; app alone code {} B, arena {} B; pair stack {} B",
        compiler::STDLIB_SOURCE.len(),
        library.code.len(),
        rich.code.len(),
        library_header.stack_size,
        library_header.state_size,
        rich_arena,
        plain.code.len(),
        plain_arena,
        rich_header.stack_size,
    );
    assert!(
        rich_arena <= compiler::MAX_ARENA_BYTES,
        "the pair fits the published arena cap"
    );
    assert!(rich.code.len() > plain.code.len());
    assert!(header_function_count(&rich) > header_function_count(&plain));
}

fn header_function_count(package: &Owned) -> u32 {
    CodeHeader::from_bytes(&package.code[..abi::CODE_HEADER_SIZE])
        .expect("header")
        .function_count
}

#[test]
fn arrays_of_every_element_kind_compile() {
    let source = br#"
unit t;

type
  Point = record x: int; end;
  Sum = A | B(value: int);
  Color = (Red, Green);

var
  ints: array[int, 2];
  floats: array[f64, 2];
  raws: array[bytes[4], 2];
  words: array[string[4], 2];
  colors: array[Color, 2];
  points: array[Point, 2];
  sums: array[Sum, 2];
  nested: array[array[int, 2], 2];
  total: int;
  b: byte;

procedure main(args: const string[1024]);
begin
  ints[0] := 1;
  floats[0] := 1.5;
  raws[0] := "ab";
  words[0] := "cd";
  colors[0] := Red;
  points[0].x := 3;
  sums[0] := A;
  nested[1][1] := 5;
  nested[0] := nested[1];
  points[1] := points[0];
  total := ints[0] + nested[1][1] + points[0].x;
  b := raws[0][0];
end;
"#;
    let package = compile(source, "t.ig").expect("arrays compile");
    let ops = opcodes_of(&package);
    assert!(
        ops.contains(&abi::opcodes::REF_ELEMENT),
        "no ref_element in {ops:?}"
    );
    assert!(ops.contains(&abi::opcodes::COPY_REFS));
}

#[test]
fn an_array_count_comes_from_a_positive_constant() {
    let good = br#"
unit t;

const
  Rows = 2;
  Cols = Rows + 1;

var
  cells: array[int, Cols];

procedure main(args: const string[1024]);
begin
  cells[Cols - 1] := 7;
end;
"#;
    compile(good, "t.ig").expect("a const expression sizes an array");

    for count in ["0", "-1"] {
        let source = format!(
            r#"
unit t;

var
  cells: array[int, {count}];

procedure main(args: const string[1024]);
begin
end;
"#
        );
        let error = compile(source.as_bytes(), "t.ig").expect_err("count must be positive");
        assert!(
            error.message.contains("positive constant"),
            "{}",
            error.message
        );
    }
}

#[test]
fn array_types_are_nominal_in_element_and_count() {
    let source = br#"
unit t;

var
  a: array[int, 2];
  b: array[int, 3];

procedure main(args: const string[1024]);
begin
  a := b;
end;
"#;
    let error = compile(source, "t.ig").expect_err("counts differ");
    assert!(
        error
            .message
            .contains("type mismatch: expected `array[int, 2]`, found `array[int, 3]`"),
        "{}",
        error.message
    );

    let source = br#"
unit t;

var
  a: array[int, 2];
  b: array[byte, 2];

procedure main(args: const string[1024]);
begin
  a := b;
end;
"#;
    let error = compile(source, "t.ig").expect_err("elements differ");
    assert!(
        error
            .message
            .contains("expected `array[int, 2]`, found `array[byte, 2]`"),
        "{}",
        error.message
    );
}

#[test]
fn array_parameters_and_record_fields_work() {
    let source = br#"
unit t;

type
  Row = record cells: array[int, 2]; end;

var
  row: Row;

procedure fill(dst: var array[int, 2]);
begin
  dst[0] := 1;
  dst[1] := 2;
end;

procedure read(src: const array[int, 2]; total: var int);
begin
  total := src[0] + src[1];
end;

function first(items: array[int, 2]): int;
begin
  return items[0];
end;

procedure main(args: const string[1024]);
var
  local: array[int, 2];
  total: int;
begin
  fill(local);
  read(local, total);
  row.cells := local;
  total := total + first(row.cells);
end;
"#;
    compile(source, "t.ig").expect("arrays in parameters and records compile");
}

#[test]
fn indexing_an_array_needs_an_integer() {
    let source = br#"
unit t;

var
  a: array[int, 2];
  f: f64;

procedure main(args: const string[1024]);
begin
  a[0] := 1;
  f := 1.0;
  a[f] := 2;
end;
"#;
    let error = compile(source, "t.ig").expect_err("an f64 is not an index");
    assert!(
        error
            .message
            .contains("index must be `int`, `uint` or `byte`"),
        "{}",
        error.message
    );
}

#[test]
fn writing_an_element_of_a_const_array_is_rejected() {
    let source = br#"
unit t;

procedure touch(src: const array[int, 2]);
begin
  src[0] := 1;
end;

procedure main(args: const string[1024]);
begin
end;
"#;
    let error = compile(source, "t.ig").expect_err("const arrays are read-only");
    assert!(
        error.message.contains("`const` parameter"),
        "{}",
        error.message
    );
}

#[test]
fn the_array_sample_compiles() {
    let source = include_bytes!("../../samples/lang/array.ig");
    compile(source, "array.ig").expect("the array sample compiles");
}

#[test]
fn rings_of_every_element_kind_compile() {
    let source = br#"
unit t;

type
  Point = record x: int; end;
  Sum = A | B(value: int);
  Color = (Red, Green);

var
  ints: ring[int, 2];
  floats: ring[f64, 2];
  raws: ring[bytes[4], 2];
  words: ring[string[4], 2];
  colors: ring[Color, 2];
  points: ring[Point, 2];
  sums: ring[Sum, 2];
  arrays: ring[array[int, 2], 2];
  nested: ring[ring[int, 2], 2];
  st: Status;
  i: int;
  f: f64;
  raw: bytes[4];
  word: string[4];
  c: Color;
  p: Point;
  s: Sum;
  a: array[int, 2];
  r: ring[int, 2];
  b: byte;

procedure main(args: const string[1024]);
begin
  st := core.ring_push(ints, 1);
  st := core.ring_pop(ints, i);
  st := core.ring_push(floats, 1.5);
  st := core.ring_pop(floats, f);
  st := core.ring_push(raws, "ab");
  st := core.ring_pop(raws, raw);
  st := core.ring_push(words, "cd");
  st := core.ring_pop(words, word);
  c := Red;
  st := core.ring_push(colors, c);
  st := core.ring_pop(colors, c);
  p.x := 3;
  st := core.ring_push(points, p);
  st := core.ring_peek(points, p);
  s := A;
  st := core.ring_push(sums, s);
  st := core.ring_pop(sums, s);
  a[0] := 1;
  st := core.ring_push(arrays, a);
  st := core.ring_pop(arrays, a);
  st := core.ring_push(nested, r);
  st := core.ring_pop(nested, r);
  b := raw[0];
end;
"#;
    let package = compile(source, "t.ig").expect("rings compile");
    let ops = opcodes_of(&package);
    assert!(
        ops.contains(&abi::opcodes::REF_ELEMENT),
        "no ref_element in {ops:?}"
    );
    assert!(ops.contains(&abi::opcodes::COPY_REFS));
    assert!(ops.contains(&abi::opcodes::MOD_U));
}

#[test]
fn a_ring_count_comes_from_a_positive_constant() {
    let good = br#"
unit t;

const
  Capacity = 1 + 1;

var
  cells: ring[int, Capacity];

procedure main(args: const string[1024]);
var
  value: int;
begin
  if core.ring_push(cells, 7) = Ok then
    if core.ring_pop(cells, value) = Ok then
      core.write("ok\n");
end;
"#;
    compile(good, "t.ig").expect("a const expression sizes a ring");

    for count in ["0", "-1"] {
        let source = format!(
            r#"
unit t;

var
  cells: ring[int, {count}];

procedure main(args: const string[1024]);
begin
end;
"#
        );
        let error = compile(source.as_bytes(), "t.ig").expect_err("count must be positive");
        assert!(
            error.message.contains("positive constant"),
            "{}",
            error.message
        );
    }
}

#[test]
fn ring_types_are_nominal_in_element_and_count() {
    let source = br#"
unit t;

var
  a: ring[int, 2];
  b: ring[int, 3];

procedure main(args: const string[1024]);
begin
  a := b;
end;
"#;
    let error = compile(source, "t.ig").expect_err("counts differ");
    assert!(
        error
            .message
            .contains("type mismatch: expected `ring[int, 2]`, found `ring[int, 3]`"),
        "{}",
        error.message
    );

    let source = br#"
unit t;

var
  a: ring[int, 2];
  b: ring[byte, 2];

procedure main(args: const string[1024]);
begin
  a := b;
end;
"#;
    let error = compile(source, "t.ig").expect_err("elements differ");
    assert!(
        error
            .message
            .contains("expected `ring[int, 2]`, found `ring[byte, 2]`"),
        "{}",
        error.message
    );
}

#[test]
fn ring_positions_and_parameters_work() {
    let source = br#"
unit t;

type
  Row = record queue: ring[int, 2]; end;
  Box = Empty | Holds(value: ring[int, 2]);

var
  row: Row;
  box: Box;
  rows: array[ring[int, 2], 2];

procedure fill(dst: var ring[int, 2]);
begin
  if core.ring_push(dst, 1) <> Ok then
    core.write("full\n");
end;

procedure read(src: const ring[int, 2]; total: var int);
var
  value: int;
begin
  total := -1;
  if core.ring_peek(src, value) = Ok then
    total := value;
end;

function first(items: ring[int, 2]): int;
var
  value: int;
begin
  value := -1;
  if core.ring_peek(items, value) = Ok then
    return value;
  return -1;
end;

procedure main(args: const string[1024]);
var
  local: ring[int, 2];
  total: int;
  value: int;
begin
  fill(local);
  read(local, total);
  total := total + first(local);
  row.queue := local;
  rows[0] := local;
  box := Holds(local);
  case box of
    Empty:
      value := 0;
    Holds(inner):
      if core.ring_pop(inner, value) <> Ok then
        value := -1;
  end;
end;
"#;
    compile(source, "t.ig").expect("rings in every position compile");
}

#[test]
fn a_ring_call_on_a_non_ring_is_reported() {
    let source = br#"
unit t;

var
  value: int;

procedure main(args: const string[1024]);
begin
  if core.ring_push(value, 1) = Ok then
    core.write("ok\n");
end;
"#;
    let error = compile(source, "t.ig").expect_err("an int is not a ring");
    assert!(
        error
            .message
            .contains("expects a `ring[T, N]` first argument, found `int`"),
        "{}",
        error.message
    );
}

#[test]
fn mutating_a_const_ring_parameter_is_rejected() {
    let source = br#"
unit t;

procedure touch(src: const ring[int, 2]);
begin
  if core.ring_push(src, 1) = Ok then
    core.write("ok\n");
end;

procedure main(args: const string[1024]);
begin
end;
"#;
    let error = compile(source, "t.ig").expect_err("const rings are read-only");
    assert!(
        error.message.contains("cannot mutate `const` parameter"),
        "{}",
        error.message
    );
}

#[test]
fn a_value_ring_parameter_is_an_ordinary_writable_copy() {
    // A `value` parameter is a writable local copy everywhere in the language,
    // so push/pop through one compile; the callee mutates its own image.
    let source = br#"
unit t;

procedure touch(src: ring[int, 2]; total: var int);
var
  value: int;
begin
  if core.ring_push(src, 1) <> Ok then
    total := -1;
  if core.ring_pop(src, value) = Ok then
    total := value;
end;

procedure main(args: const string[1024]);
var
  queue: ring[int, 2];
  total: int;
begin
  touch(queue, total);
end;
"#;
    compile(source, "t.ig").expect("a value ring parameter is a writable copy");
}

#[test]
fn a_ring_field_of_a_value_record_parameter_is_writable() {
    // The copy rule follows the whole value, not just the designator base: a
    // ring reached through a `value` record parameter is writable too.
    let source = br#"
unit t;

type
  Box = record queue: ring[int, 2]; end;

procedure fill(box: Box; seen: var int);
var
  value: int;
begin
  seen := -1;
  if core.ring_push(box.queue, 7) = Ok then
    if core.ring_pop(box.queue, value) = Ok then
      seen := value;
end;

procedure main(args: const string[1024]);
var
  box: Box;
  seen: int;
begin
  fill(box, seen);
end;
"#;
    compile(source, "t.ig").expect("a ring field of a value record is writable");
}

#[test]
fn ring_operations_check_their_argument_count_and_value_types() {
    let cases: [(&[u8], &str); 5] = [
        (
            br#"
unit t;
var r: ring[int, 2];
procedure main(args: const string[1024]);
begin
  if core.ring_push(r) = Ok then core.write("x\n");
end;
"#,
            "`core.ring_push` expects 2 arguments, got 1",
        ),
        (
            br#"
unit t;
var r: ring[int, 2];
var a, b: int;
procedure main(args: const string[1024]);
begin
  if core.ring_pop(r, a, b) = Ok then core.write("x\n");
end;
"#,
            "`core.ring_pop` expects 2 arguments, got 3",
        ),
        (
            br#"
unit t;
var r: ring[byte, 2];
procedure main(args: const string[1024]);
begin
  if core.ring_push(r, 1) = Ok then core.write("x\n");
end;
"#,
            "`core.ring_push` expects `byte`, found `int`",
        ),
        (
            br#"
unit t;
var r: ring[string[4], 2];
var value: int;
procedure main(args: const string[1024]);
begin
  if core.ring_pop(r, value) = Ok then core.write("x\n");
end;
"#,
            "`core.ring_pop` expects `var value: string[4]`, found `int`",
        ),
        (
            br#"
unit t;
var r: ring[int, 2];
procedure main(args: const string[1024]);
begin
  if core.ring_full(r, r) then core.write("x\n");
end;
"#,
            "`core.ring_full` expects 1 argument, got 2",
        ),
    ];
    for (source, expected) in cases {
        let error = compile(source, "t.ig").expect_err("the call is invalid");
        assert!(
            error.message.contains(expected),
            "expected `{expected}` in: {}",
            error.message
        );
    }
}

#[test]
fn a_ring_is_not_a_scalar_value_or_comparable() {
    let source = br#"
unit t;

var
  a: ring[int, 2];
  b: ring[int, 2];

procedure main(args: const string[1024]);
begin
  if a = b then
    core.write("same\n");
end;
"#;
    let error = compile(source, "t.ig").expect_err("rings do not compare");
    assert!(error.message.contains("ring"), "{}", error.message);
    assert!(error.message.contains("scalar"), "{}", error.message);
}

#[test]
fn ring_calls_emit_no_builtin_opcode() {
    let source = br#"
unit t;

type
  Point = record x: int; end;

var
  r: ring[int, 2];
  points: ring[Point, 2];
  point: Point;

procedure main(args: const string[1024]);
var
  value: int;
begin
  if core.ring_push(r, 1) = Ok then
    value := 0;
  if core.ring_pop(r, value) = Ok then
    value := value + 1;
  if core.ring_peek(r, value) = Ok then
    value := value + 1;
  if core.ring_full(r) then
    value := value + 1;
  if core.ring_empty(r) then
    value := value + 1;
  point.x := 3;
  if core.ring_push(points, point) = Ok then
    point := point;
  if core.ring_pop(points, point) = Ok then
    value := point.x;
end;
"#;
    let package = compile(source, "t.ig").expect("ring operations compile");
    let ops = opcodes_of(&package);
    assert!(
        !ops.contains(&abi::opcodes::BUILTIN),
        "ring calls must not use the BUILTIN opcode: {ops:?}"
    );
    for required in [
        abi::opcodes::REF_ELEMENT,
        abi::opcodes::COPY_REFS,
        abi::opcodes::MOD_U,
        abi::opcodes::JNZ,
    ] {
        assert!(ops.contains(&required), "missing opcode {required:#x}");
    }
}

#[test]
fn the_ring_sample_compiles() {
    let source = include_bytes!("../../samples/lang/ring.ig");
    compile(source, "ring.ig").expect("the ring sample compiles");
}

#[test]
fn compile_cost_of_a_ring_is_bounded_and_reported() {
    // The same application with and without one ring isolates the cost of the
    // type and its operations.
    let without = br#"
unit cost;
uses core;
procedure main(args: const string[1024]);
begin
  core.write("ring\n");
end;
"#;
    let with = br#"
unit cost;
uses core;
var
  r: ring[int, 4];
procedure main(args: const string[1024]);
var
  value: int;
begin
  if core.ring_push(r, 1) = Ok then
    value := 0;
  if core.ring_pop(r, value) = Ok then
    core.write("ring\n");
end;
"#;
    let plain = compile(without, "cost.ig").expect("compiles");
    let ring = compile(with, "cost.ig").expect("compiles");

    // Smallest arena each build accepts, found by bisection. This is the real
    // scratch cost because `compile_into` caps the arena to its slice length.
    let minimum_arena = |source: &[u8]| -> usize {
        let mut low = 4 * 1024usize;
        let mut high = compiler::MAX_ARENA_BYTES;
        while low < high {
            let mid = (low + high) / 2;
            let mut arena = vec![0u8; mid];
            let mut code = vec![0u8; compiler::MAX_CODE_BYTES];
            let mut manifest = vec![0u8; compiler::MAX_MANIFEST_BYTES];
            match compiler::compile_into(source, "cost.ig", &mut arena, &mut code, &mut manifest) {
                Ok(_) => high = mid,
                Err(error) if error.status == abi::Status::CapacityExceeded => low = mid + 1,
                Err(error) => panic!("unexpected diagnostic: {error}"),
            }
        }
        low
    };
    let plain_arena = minimum_arena(without);
    let ring_arena = minimum_arena(with);
    let ring_header = CodeHeader::from_bytes(&ring.code[..abi::CODE_HEADER_SIZE]).expect("header");
    println!(
        "ring cost: code {} B (app alone {} B), state {} B, stack {} B, arena {} B (app alone {} B)",
        ring.code.len(),
        plain.code.len(),
        ring_header.state_size,
        ring_header.stack_size,
        ring_arena,
        plain_arena,
    );
    assert!(
        ring_arena <= compiler::MAX_ARENA_BYTES,
        "the ring app fits the published arena cap"
    );
    assert!(ring.code.len() > plain.code.len());
    assert_eq!(ring_header.state_size, 48, "ring[int, 4] is 4*8 + 16");
}

#[test]
fn a_ring_element_type_must_be_known_and_at_least_one() {
    let source = br#"
unit t;

var
  cells: ring[Nope, 2];

procedure main(args: const string[1024]);
begin
end;
"#;
    let error = compile(source, "t.ig").expect_err("Nope is not a type");
    assert!(
        error.message.contains("unknown type `Nope`"),
        "{}",
        error.message
    );

    let source = br#"
unit t;

var
  cells: ring[int, 0];

procedure main(args: const string[1024]);
begin
end;
"#;
    let error = compile(source, "t.ig").expect_err("N = 0 is rejected");
    assert!(
        error
            .message
            .contains("ring count must be a positive constant, found 0"),
        "{}",
        error.message
    );

    let source = br#"
unit t;

type ring = (A, B);

procedure main(args: const string[1024]);
begin
end;
"#;
    let error = compile(source, "t.ig").expect_err("a built-in type name cannot be shadowed");
    assert!(error.message.contains("built-in type"), "{}", error.message);
}
