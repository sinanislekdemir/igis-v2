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
    assert_eq!(compiler::MAX_SOURCE_BYTES, 128 * 1024);
    assert_eq!(compiler::MAX_ARENA_BYTES, 8 * 1024 * 1024);
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

/// Returns `(destination kind, source kind, capacity)` for every `substr`
/// instruction in order.
fn substr_encodings(package: &Owned) -> Vec<(u8, u8, u32)> {
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
            let source_kind = instructions[pc + 2];
            let capacity = u32::from_le_bytes(instructions[pc + 4..pc + 8].try_into().unwrap());
            seen.push((kind, source_kind, capacity));
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
fn substr_encodes_both_kinds_and_destination_capacity() {
    let source = br#"
unit t;

uses core;

var
  text: string[16];
  part: string[8];
  raw: bytes[16];
  piece: bytes[4];
  tail: bytes[8];
  wide: string[12];
  n: int;

procedure main(args: const string[1024]);
begin
  text := "hello";
  raw := "abcdef";
  n := core.substr_into(part, text, 0, 2);
  n := core.substr_into(piece, raw, 1, 2);
  n := core.substr_into(part, raw, 0, 2);
  n := core.substr_into(tail, text, 0, 2);
  n := core.substr_into(wide, "abc", 0, 1);
end;
"#;
    let package = compile(source, "t.ig").expect("compiles");
    assert_eq!(
        substr_encodings(&package),
        vec![
            (
                abi::opcodes::buffer_kind::STRING,
                abi::opcodes::buffer_kind::STRING,
                8
            ),
            (
                abi::opcodes::buffer_kind::BYTES,
                abi::opcodes::buffer_kind::BYTES,
                4
            ),
            // `bytes` source into a `string` destination is the checked
            // conversion; the destination kind selects the run-time check.
            (
                abi::opcodes::buffer_kind::STRING,
                abi::opcodes::buffer_kind::BYTES,
                8
            ),
            (
                abi::opcodes::buffer_kind::BYTES,
                abi::opcodes::buffer_kind::STRING,
                8
            ),
            // A literal takes its kind from the destination.
            (
                abi::opcodes::buffer_kind::STRING,
                abi::opcodes::buffer_kind::STRING,
                12
            ),
        ]
    );
}

#[test]
fn substr_rejects_non_writable_destinations_and_a_wrong_arity() {
    // A literal is not a writable place.
    let literal = br#"
unit t;

uses core;

var
  n: int;

procedure main(args: const string[1024]);
begin
  n := core.substr_into("abc", "abc", 0, 1);
end;
"#;
    let error = compile(literal, "t.ig").expect_err("a literal is not a destination");
    assert!(
        error.message.contains("must be a variable or parameter"),
        "{}",
        error.message
    );

    // A `const` parameter is read-only.
    let constant = br#"
unit t;

uses core;

var
  n: int;

procedure poke(dst: const string[8]);
begin
  n := core.substr_into(dst, "abc", 0, 1);
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

    // Buffer parameters are `const` or `var`; there is no `value` mode.
    let value = br#"
unit t;

uses core;

procedure poke(dst: string[8]);
begin
end;

procedure main(args: const string[1024]);
begin
end;
"#;
    let error = compile(value, "t.ig").expect_err("a buffer parameter needs a mode");
    assert!(
        error.message.contains("must be declared `const` or `var`"),
        "{}",
        error.message
    );

    let arity = br#"
unit t;

uses core;

var
  text: string[8];
  n: int;

procedure main(args: const string[1024]);
begin
  text := "abc";
  n := core.substr_into(text, text, 0);
end;
"#;
    let error = compile(arity, "t.ig").expect_err("four arguments are required");
    assert!(
        error.message.contains("expects 4 arguments, got 3"),
        "{}",
        error.message
    );
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

/// Returns `(kind, capacity)` for every `format_f64` instruction in order.
fn format_f64_encodings(package: &Owned) -> Vec<(u8, u32)> {
    use abi::opcodes;
    let header = CodeHeader::from_bytes(&package.code[..40]).expect("header");
    let start = 40 + 24 * header.function_count as usize;
    let instructions = &package.code[start..start + header.code_len as usize];
    let mut seen = Vec::new();
    let mut pc = 0;
    while pc + 4 <= instructions.len() {
        let op = instructions[pc];
        let words = opcodes::instruction_words(op).expect("known opcode") as usize;
        if op == opcodes::FORMAT_F64 {
            let kind = instructions[pc + 1];
            let capacity = u32::from_le_bytes(instructions[pc + 4..pc + 8].try_into().unwrap());
            seen.push((kind, capacity));
        }
        pc += words * 4;
    }
    seen
}

#[test]
fn format_f64_into_checks_types_and_encodes_the_destination() {
    let source = br#"
unit t;

uses core;

var
  text: string[32];
  raw: bytes[64];
  n: int;
  x: f64;

procedure main(args: const string[1024]);
begin
  x := -1.5;
  n := core.format_f64_into(text, x, uint(2));
  n := core.format_f64_into(raw, 2.5, uint(0));
end;
"#;
    let package = compile(source, "t.ig").expect("compiles");
    assert_eq!(
        format_f64_encodings(&package),
        vec![
            (abi::opcodes::buffer_kind::STRING, 32),
            (abi::opcodes::buffer_kind::BYTES, 64)
        ]
    );

    let wrong_arity = br#"
unit t;

uses core;

var
  text: string[32];
  n: int;

procedure main(args: const string[1024]);
begin
  n := core.format_f64_into(text, 1.5);
end;
"#;
    let error = compile(wrong_arity, "t.ig").expect_err("three arguments are required");
    assert!(
        error.message.contains("expects 3 arguments, got 2"),
        "{}",
        error.message
    );

    let wrong_value = br#"
unit t;

uses core;

var
  text: string[32];
  n: int;

procedure main(args: const string[1024]);
begin
  n := core.format_f64_into(text, 1, uint(2));
end;
"#;
    let error = compile(wrong_value, "t.ig").expect_err("the value is an f64");
    assert!(error.message.contains("`f64` value"), "{}", error.message);

    let wrong_digits = br#"
unit t;

uses core;

var
  text: string[32];
  n: int;

procedure main(args: const string[1024]);
begin
  n := core.format_f64_into(text, 1.5, 2);
end;
"#;
    let error = compile(wrong_digits, "t.ig").expect_err("the width is a uint");
    assert!(
        error.message.contains("`uint` fraction width"),
        "{}",
        error.message
    );

    let wrong_destination = br#"
unit t;

uses core;

var
  n: int;
  x: f64;

procedure main(args: const string[1024]);
begin
  x := 1.5;
  n := core.format_f64_into("abc", x, uint(2));
end;
"#;
    let error = compile(wrong_destination, "t.ig").expect_err("the destination is var");
    assert!(
        error.message.contains("must be a variable or parameter"),
        "{}",
        error.message
    );

    let scalar_destination = br#"
unit t;

uses core;

var
  value: int;
  n: int;
  x: f64;

procedure main(args: const string[1024]);
begin
  x := 1.5;
  n := core.format_f64_into(value, x, uint(2));
end;
"#;
    let error = compile(scalar_destination, "t.ig").expect_err("an int is not a buffer");
    assert!(
        error.message.contains("not a buffer") || error.message.contains("buffer"),
        "{}",
        error.message
    );
}

/// Returns `(kind, capacity)` for every `format_f64_short` instruction in
/// order.
fn format_f64_short_encodings(package: &Owned) -> Vec<(u8, u32)> {
    use abi::opcodes;
    let header = CodeHeader::from_bytes(&package.code[..40]).expect("header");
    let start = 40 + 24 * header.function_count as usize;
    let instructions = &package.code[start..start + header.code_len as usize];
    let mut seen = Vec::new();
    let mut pc = 0;
    while pc + 4 <= instructions.len() {
        let op = instructions[pc];
        let words = opcodes::instruction_words(op).expect("known opcode") as usize;
        if op == opcodes::FORMAT_F64_SHORT {
            let kind = instructions[pc + 1];
            let capacity = u32::from_le_bytes(instructions[pc + 4..pc + 8].try_into().unwrap());
            seen.push((kind, capacity));
        }
        pc += words * 4;
    }
    seen
}

#[test]
fn format_f64_short_into_checks_types_and_encodes_the_destination() {
    let source = br#"
unit t;

uses core;

var
  text: string[32];
  raw: bytes[48];
  n: int;
  x: f64;

procedure main(args: const string[1024]);
begin
  x := -1.5;
  n := core.format_f64_short_into(text, x);
  n := core.format_f64_short_into(raw, 1.0e300);
  n := core.format_f64_short_into(text, 1.0 / 3.0);
end;
"#;
    let package = compile(source, "t.ig").expect("compiles");
    assert_eq!(
        format_f64_short_encodings(&package),
        vec![
            (abi::opcodes::buffer_kind::STRING, 32),
            (abi::opcodes::buffer_kind::BYTES, 48),
            (abi::opcodes::buffer_kind::STRING, 32),
        ]
    );

    let wrong_arity = br#"
unit t;

uses core;

var
  text: string[32];
  n: int;

procedure main(args: const string[1024]);
begin
  n := core.format_f64_short_into(text, 1.5, uint(2));
end;
"#;
    let error = compile(wrong_arity, "t.ig").expect_err("two arguments are required");
    assert!(
        error.message.contains("expects 2 arguments, got 3"),
        "{}",
        error.message
    );

    let wrong_value = br#"
unit t;

uses core;

var
  text: string[32];
  n: int;

procedure main(args: const string[1024]);
begin
  n := core.format_f64_short_into(text, 1);
end;
"#;
    let error = compile(wrong_value, "t.ig").expect_err("the value is an f64");
    assert!(error.message.contains("`f64` value"), "{}", error.message);

    let wrong_destination = br#"
unit t;

uses core;

var
  n: int;

procedure main(args: const string[1024]);
begin
  n := core.format_f64_short_into("abc", 1.5);
end;
"#;
    let error = compile(wrong_destination, "t.ig").expect_err("the destination is var");
    assert!(
        error.message.contains("must be a variable or parameter"),
        "{}",
        error.message
    );

    let scalar_destination = br#"
unit t;

uses core;

var
  value: int;
  n: int;

procedure main(args: const string[1024]);
begin
  n := core.format_f64_short_into(value, 1.5);
end;
"#;
    let error = compile(scalar_destination, "t.ig").expect_err("an int is not a buffer");
    assert!(
        error.message.contains("not a buffer") || error.message.contains("buffer"),
        "{}",
        error.message
    );
}

/// Counts the `BUILTIN` instructions carrying `id`, in order.
fn builtin_count(package: &Owned, id: u32) -> usize {
    use abi::opcodes;
    let header = CodeHeader::from_bytes(&package.code[..40]).expect("header");
    let start = 40 + 24 * header.function_count as usize;
    let instructions = &package.code[start..start + header.code_len as usize];
    let mut seen = 0;
    let mut pc = 0;
    while pc + 4 <= instructions.len() {
        let op = instructions[pc];
        let words = opcodes::instruction_words(op).expect("known opcode") as usize;
        if op == opcodes::BUILTIN {
            let immediate = u32::from_le_bytes(instructions[pc + 4..pc + 8].try_into().unwrap());
            if immediate == id {
                seen += 1;
            }
        }
        pc += words * 4;
    }
    seen
}

#[test]
fn parse_f64_into_checks_types_and_emits_the_builtin() {
    let source = br#"
unit t;

uses core;

var
  text: string[32];
  value: f64;
  st: Status;

procedure main(args: const string[1024]);
begin
  st := core.parse_f64_into(text, value);
  st := core.parse_f64_into("1.5", value);
end;
"#;
    let package = compile(source, "t.ig").expect("compiles");
    assert_eq!(
        builtin_count(&package, abi::opcodes::builtin::PARSE_F64),
        2,
        "one builtin per call, two words each"
    );

    let wrong_arity = br#"
unit t;

uses core;

var
  value: f64;
  st: Status;

procedure main(args: const string[1024]);
begin
  st := core.parse_f64_into("1", value, value);
end;
"#;
    let error = compile(wrong_arity, "t.ig").expect_err("two arguments are required");
    assert!(
        error.message.contains("expects 2 arguments, got 3"),
        "{}",
        error.message
    );

    let wrong_source = br#"
unit t;

uses core;

var
  raw: bytes[16];
  value: f64;
  st: Status;

procedure main(args: const string[1024]);
begin
  st := core.parse_f64_into(raw, value);
end;
"#;
    let error = compile(wrong_source, "t.ig").expect_err("the source must be a string");
    assert!(
        error.message.contains("expects a `string` source"),
        "{}",
        error.message
    );

    let wrong_value = br#"
unit t;

uses core;

var
  n: int;
  st: Status;

procedure main(args: const string[1024]);
begin
  st := core.parse_f64_into("1", n);
end;
"#;
    let error = compile(wrong_value, "t.ig").expect_err("the value must be an f64");
    assert!(
        error.message.contains("`var value: f64`"),
        "{}",
        error.message
    );

    let const_value = br#"
unit t;

uses core;

function f(x: const f64): Status;
var
  st: Status;
begin
  st := core.parse_f64_into("1", x);
  return st;
end;

procedure main(args: const string[1024]);
begin
end;
"#;
    let error = compile(const_value, "t.ig").expect_err("a const value is not writable");
    assert!(
        error.message.contains("cannot pass `const` parameter"),
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
fn syslib_cpu_id_is_accepted_and_emits_its_builtin() {
    use abi::opcodes::{self, builtin};

    let source = br#"
unit t;

uses core, syslib;

var
  core_id: uint;

procedure main(args: const string[1024]);
begin
  core_id := syslib.cpu_id();
end;
"#;
    let package = compile(source, "t.ig").expect("cpu_id is a known primitive");
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
    assert_eq!(seen, vec![builtin::CPU_ID]);

    let with_argument = br#"
unit t;

uses core, syslib;

var
  r: uint;

procedure main(args: const string[1024]);
begin
  r := syslib.cpu_id(uint(1));
end;
"#;
    let error = compile(with_argument, "t.ig").expect_err("cpu_id takes no argument");
    assert!(
        error
            .message
            .contains("`syslib.cpu_id` expects 0 arguments"),
        "{}",
        error.message
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
        header.function_count, 125,
        "trim_into, split_once, parse_int_into, parse_f64_into, starts_with, contains_byte, \
         split_all, drain_into, map_get_or, utf8_decode_into, utf8_len, utf8_decode, \
         utf8_valid, recv_exact, send_frame, recv_frame_into, send_line, recv_line_into, \
         send_u64, recv_u64, send_bytes, the M2.4.0 UI layer: ui_text_width, \
         ui_text_height, ui_line_height, ui_content_inset, ui_title_bar_height, \
         ui_close_box_size, ui_black, ui_white, ui_gray, ui_focus, ui_accent, \
         ui_text_center_x, ui_text_center_y, ui_frame, ui_title_bar, ui_close_box, \
         ui_separator, ui_center_text, the M2.4.1 editing layer: str_boundary, \
         str_cursor_clamp, str_cursor_left, str_cursor_right, str_cursor_home, \
         str_cursor_end, str_insert_into, str_delete_into, str_backspace, \
         str_selection_normalize, str_selection_all, str_selection_collapse, \
         str_selection_replace, the M2.4.2 controls layer: ui_rect_hit, \
         ui_focus_ring, ui_focus_next, ui_key_text, ui_key_modifiers, ui_key_code, \
         ui_key_is_backspace, ui_key_is_tab, ui_key_is_enter, ui_key_is_left, \
         ui_key_is_right, ui_key_is_up, ui_key_is_down, ui_key_is_home, ui_key_is_end, \
         ui_key_is_delete, ui_button, \
         ui_button_hit, ui_button_pressed, ui_checkbox, ui_checkbox_hit, \
         ui_checkbox_toggle, ui_text_field, ui_text_field_reveal, ui_scrollbar_clamp, \
         ui_scrollbar, ui_scrollbar_hit, ui_list_rows, ui_list, ui_list_hit, \
         ui_list_ensure_visible, ui_list_move, and the M2.4.3 menus layer: \
         ui_menu_bar_height, ui_menu_is_separator, ui_menu_bit, ui_menu_has, \
         ui_menu_set, ui_menu_selectable, ui_menu_first, ui_menu_open, ui_menu_close, \
         ui_menu_move, ui_menu_key, ui_menu_bar, ui_menu_bar_hit, ui_menu_dropdown, \
         ui_menu_hit, and the M2.4.4 dialogs layer: ui_message, \
         ui_dialog_button_width, ui_dialog_button_left, ui_dialog_button_top, \
         ui_dialog_button, ui_alert, ui_alert_hit, ui_input_dialog, \
         ui_input_dialog_hit, ui_dialog_key, and the M2.4.5 icons layer: \
         ui_icon_size, ui_icon_row_bit, ui_icon, ui_icon_label, ui_icon_cell_x, \
         ui_icon_cell_y, ui_icon_label_y, ui_icon_grid_hit, ui_double_click, \
         ui_icon_grab, ui_icon_follow, ui_selection_normalize, \
         ui_selection_rect, ui_rect_intersects, ui_icon_drop, ui_trash_hit"
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
fn an_application_calls_the_ui_layout_helpers() {
    let source = br#"
unit ui;
uses core, stdlib, syslib;

var
  st: Status;
  n: int;

procedure main(args: const string[1024]);
begin
  n := stdlib.ui_text_width(args);
  n := stdlib.ui_text_center_x(0, 100, "ok");
  n := stdlib.ui_text_center_y(0, stdlib.ui_line_height());
  if stdlib.ui_text_height() = 8 then
    n := n + 1;
  if stdlib.ui_content_inset() = 8 then
    n := n + 1;
  if stdlib.ui_title_bar_height() = 16 then
    n := n + 1;
  if stdlib.ui_close_box_size() = 11 then
    n := n + 1;
  if stdlib.ui_black() = uint(0) and stdlib.ui_white() = uint(0x00FFFFFF) then
    n := n + 1;
  if stdlib.ui_gray() = uint(0x00CCCCCC) and stdlib.ui_focus() = stdlib.ui_white() then
    n := n + 1;
  if stdlib.ui_accent() = uint(0x000080) then
    n := n + 1;
  st := stdlib.ui_frame(0, 0, 10, 10);
  st := stdlib.ui_title_bar(0, 0, 100, "title", true);
  st := stdlib.ui_close_box(1, 1);
  st := stdlib.ui_separator(0, 20, 100);
  st := stdlib.ui_center_text(0, 0, 100, 16, "ok", stdlib.ui_black());
  if st = Invalid then
    n := n + 1;
end;
"#;
    let package = compile(source, "ui.ig").expect("the UI helpers compile");
    let header = CodeHeader::from_bytes(&package.code[..abi::CODE_HEADER_SIZE]).expect("header");
    assert_eq!(
        header.entry, 125,
        "the entry follows the 125 library functions"
    );
    assert!(header_function_count(&package) > 84);
}

#[test]
fn an_application_calls_the_string_editing_helpers() {
    let source = br#"
unit edit;
uses core, stdlib;

var
  text: string[256];
  cursor: int;
  next: int;
  start: int;
  stop: int;
  st: Status;
  ok: bool;

procedure main(args: const string[1024]);
begin
  text := "abc";
  ok := stdlib.str_boundary(text, 0);
  next := stdlib.str_cursor_left(text, cursor);
  next := stdlib.str_cursor_right(text, cursor);
  next := stdlib.str_cursor_home(cursor);
  next := stdlib.str_cursor_end(text, cursor);
  next := stdlib.str_cursor_clamp(text, cursor);
  st := stdlib.str_insert_into(text, next, "x");
  st := stdlib.str_delete_into(text, 0, 1);
  st := stdlib.str_backspace(text, cursor);
  next := stdlib.str_selection_normalize(text, start, stop);
  next := stdlib.str_selection_all(text, start, stop);
  next := stdlib.str_selection_collapse(text, start, stop, true);
  st := stdlib.str_selection_replace(text, start, stop, "y");
end;
"#;
    let package = compile(source, "edit.ig").expect("the editing helpers compile");
    let header = CodeHeader::from_bytes(&package.code[..abi::CODE_HEADER_SIZE]).expect("header");
    assert_eq!(
        header.entry, 125,
        "the entry follows the 125 library functions"
    );
    assert!(header_function_count(&package) > 84);
}

#[test]
fn an_application_calls_the_menu_helpers() {
    let source = br#"
unit menus;
uses core, stdlib;

const
  MenuCount = 3;

var
  titles: array[string[32], 8];
  items: array[string[64], 16];
  shortcuts: array[string[32], 16];
  count: int;
  selected: int;
  open: int;
  action: int;
  mask: uint;
  st: Status;
  on: bool;

procedure main(args: const string[1024]);
begin
  titles[0] := "File";
  items[0] := "New";
  items[1] := "-";
  items[2] := "Open";
  shortcuts[0] := "Ctrl+N";
  count := 3;
  selected := 0;
  mask := stdlib.ui_menu_bit(0);
  on := stdlib.ui_menu_has(mask, 0);
  mask := stdlib.ui_menu_set(mask, 1, true);
  on := stdlib.ui_menu_selectable(items[2], mask, 2);
  count := stdlib.ui_menu_first(count, items, mask);
  open := stdlib.ui_menu_open(0, 3, items, mask, selected);
  selected := stdlib.ui_menu_move(selected, 3, 1, items, mask);
  action := stdlib.ui_menu_key(uint(0x100 * 65536), open >= 0);
  open := stdlib.ui_menu_close();
  on := stdlib.ui_menu_is_separator(items[1]);
  count := stdlib.ui_menu_bar_height();
  st := stdlib.ui_menu_bar(0, 0, 200, titles, MenuCount, open);
  count := stdlib.ui_menu_bar_hit(0, 0, 200, titles, MenuCount, 10, 10);
  st := stdlib.ui_menu_dropdown(0, 22, 160, items, shortcuts, 3, selected, mask, mask, true);
  count := stdlib.ui_menu_hit(0, 22, 160, items, 3, 10, 30);
end;
"#;
    let package = compile(source, "menus.ig").expect("the menu helpers compile");
    let header = CodeHeader::from_bytes(&package.code[..abi::CODE_HEADER_SIZE]).expect("header");
    assert_eq!(
        header.entry, 125,
        "the entry follows the 125 library functions"
    );
    assert!(header_function_count(&package) > 125);
}

#[test]
fn an_application_calls_the_dialog_helpers() {
    let source = br#"
unit dlg;
uses core, stdlib, syslib;

var
  st: Status;
  n: int;
  focus: int;
  action: int;
  text: string[256];

procedure main(args: const string[1024]);
begin
  text := "abc";
  focus := 0;
  st := stdlib.ui_message(0, 0, 200, 64, "hello");
  n := stdlib.ui_dialog_button_width("OK");
  n := stdlib.ui_dialog_button_left(0, 300, "OK", "Cancel", focus);
  n := stdlib.ui_dialog_button_top(0, 120);
  st := stdlib.ui_dialog_button(0, 0, 72, 24, "OK", true, true);
  st := stdlib.ui_alert(0, 0, 300, 120, "t", text, "OK", "Cancel", focus);
  n := stdlib.ui_alert_hit(0, 0, 300, 120, "OK", "Cancel", 1, 1);
  st := stdlib.ui_input_dialog(
    0, 0, 300, 200, "t", "p", "OK", "Cancel", text, 3, 0, 0, 0, focus);
  n := stdlib.ui_input_dialog_hit(0, 0, 300, 200, "OK", "Cancel", 1, 1);
  action := stdlib.ui_dialog_key(uint(0x103 * 65536), true);
end;
"#;
    let package = compile(source, "dlg.ig").expect("the dialog helpers compile");
    let header = CodeHeader::from_bytes(&package.code[..abi::CODE_HEADER_SIZE]).expect("header");
    assert_eq!(
        header.entry, 125,
        "the entry follows the 125 library functions"
    );
    assert!(header_function_count(&package) > 125);
}

#[test]
fn ui_dialogs_reject_a_record_or_sum_argument() {
    // The M2.4.4 dialogs follow the same cross-unit rule: a record or sum has
    // no identity across the unit boundary, so it is a scalar/buffer mismatch.
    let record = br#"
unit t;
uses core, stdlib;

type P = record x: int; end;

var
  p: P;
  st: Status;

procedure main(args: const string[1024]);
begin
  st := stdlib.ui_alert(0, 0, 300, 120, "t", p, "OK", "Cancel", 0);
end;
"#;
    let error = compile(record, "t.ig").expect_err("a record is not a string");
    assert!(error.message.contains("not a buffer"), "{}", error.message);

    let sum = br#"
unit t;
uses core, stdlib;

type V = A | B;

var
  v: V;
  n: int;

procedure main(args: const string[1024]);
begin
  n := stdlib.ui_dialog_key(v, true);
end;
"#;
    let error = compile(sum, "t.ig").expect_err("a sum is not a uint");
    assert!(
        error.message.contains("cannot be used as a scalar"),
        "{}",
        error.message
    );
}

#[test]
fn an_application_calls_the_icon_helpers() {
    let source = br#"
unit icons;
uses core, stdlib, syslib;

var
  rows: array[uint, 32];
  x: int;
  y: int;
  dx: int;
  dy: int;
  w: int;
  h: int;
  st: Status;
  on: bool;

procedure main(args: const string[1024]);
begin
  rows[0] := uint(3);
  st := stdlib.ui_icon(0, 0, rows, stdlib.ui_black());
  st := stdlib.ui_icon_label(0, 34, "Doc");
  x := stdlib.ui_icon_size();
  x := stdlib.ui_icon_cell_x(1, 0, 100);
  x := stdlib.ui_icon_cell_y(1, 0, 80);
  x := stdlib.ui_icon_label_y(0);
  x := stdlib.ui_icon_grid_hit(0, 0, 2, 4, 100, 80, 10, 10);
  on := stdlib.ui_double_click(uint(0), uint(1), true);
  on := stdlib.ui_icon_row_bit(rows[0], 1);
  on := stdlib.ui_icon_grab(0, 0, 5, 5, dx, dy);
  st := stdlib.ui_icon_follow(100, 100, dx, dy, x, y);
  st := stdlib.ui_selection_normalize(10, 10, 0, 0, x, y, w, h);
  st := stdlib.ui_selection_rect(0, 0, 10, 10);
  on := stdlib.ui_rect_intersects(0, 0, 10, 10, 5, 5, 10, 10);
  x := stdlib.ui_icon_drop(0, 0, 2, 4, 100, 80, 300, 300, 50, 50, 10, 10);
  on := stdlib.ui_trash_hit(300, 300, 50, 50, 310, 310);
end;
"#;
    let package = compile(source, "icons.ig").expect("the icon helpers compile");
    let header = CodeHeader::from_bytes(&package.code[..abi::CODE_HEADER_SIZE]).expect("header");
    assert_eq!(
        header.entry, 125,
        "the entry follows the 125 library functions"
    );
    assert!(header_function_count(&package) > 125);
}

#[test]
fn ui_icons_reject_a_record_or_sum_argument() {
    // The M2.4.5 icons follow the same cross-unit rule: the bitmap rows are
    // `array[uint, 32]` and the bit index is a scalar, so a record and a sum
    // are rejected at the unit boundary.
    let record = br#"
unit t;
uses core, stdlib;

type P = record x: int; end;

var
  p: P;
  st: Status;

procedure main(args: const string[1024]);
begin
  st := stdlib.ui_icon(0, 0, p, uint(0));
end;
"#;
    let error = compile(record, "t.ig").expect_err("a record is not an array");
    assert!(
        error.message.contains("expects `array[uint, 32]`"),
        "{}",
        error.message
    );

    let sum = br#"
unit t;
uses core, stdlib;

type V = A | B;

var
  v: V;
  on: bool;

procedure main(args: const string[1024]);
begin
  on := stdlib.ui_icon_row_bit(v, 0);
end;
"#;
    let error = compile(sum, "t.ig").expect_err("a sum is not a uint");
    assert!(
        error.message.contains("cannot be used as a scalar"),
        "{}",
        error.message
    );
}

#[test]
fn ui_menus_reject_a_record_or_sum_argument() {
    // The M2.4.3 menus follow the same cross-unit rule: a record or sum has no
    // identity across the unit boundary, so it is a scalar/buffer mismatch.
    let record = br#"
unit t;
uses core, stdlib;

type P = record x: int; end;

var
  p: P;
  n: int;

procedure main(args: const string[1024]);
begin
  n := stdlib.ui_menu_bit(p);
end;
"#;
    let error = compile(record, "t.ig").expect_err("a record is not an int");
    assert!(
        error.message.contains("cannot be used as a scalar"),
        "{}",
        error.message
    );

    let sum = br#"
unit t;
uses core, stdlib;

type V = A | B;

var
  v: V;
  on: bool;

procedure main(args: const string[1024]);
begin
  on := stdlib.ui_menu_is_separator(v);
end;
"#;
    let error = compile(sum, "t.ig").expect_err("a sum is not a string");
    assert!(error.message.contains("not a buffer"), "{}", error.message);
}

#[test]
fn ui_helpers_reject_a_record_or_sum_argument() {
    // Type ids are per unit, so a record or sum still has no identity across
    // the boundary; the argument is a type mismatch rather than a shared type.
    let record = br#"
unit t;
uses core, stdlib;

type P = record x: int; end;

var
  p: P;
  st: Status;

procedure main(args: const string[1024]);
begin
  st := stdlib.ui_frame(p, 0, 10, 10);
end;
"#;
    let error = compile(record, "t.ig").expect_err("a record is not an int");
    assert!(
        error.message.contains("cannot be used as a scalar"),
        "{}",
        error.message
    );

    let sum = br#"
unit t;
uses core, stdlib;

type V = A | B;

var
  v: V;
  st: Status;

procedure main(args: const string[1024]);
begin
  st := stdlib.ui_frame(0, 0, 10, 10);
  st := stdlib.ui_center_text(0, 0, 10, 10, v, stdlib.ui_black());
end;
"#;
    let error = compile(sum, "t.ig").expect_err("a sum is not a string");
    assert!(error.message.contains("not a buffer"), "{}", error.message);
}

#[test]
fn ui_controls_reject_a_record_or_sum_argument() {
    // The M2.4.2 controls follow the same rule: a record or sum has no identity
    // across the unit boundary, so it is a scalar/buffer mismatch.
    let record = br#"
unit t;
uses core, stdlib;

type P = record x: int; end;

var
  p: P;
  st: Status;

procedure main(args: const string[1024]);
begin
  st := stdlib.ui_rect_hit(p, 0, 10, 10, 1, 1);
end;
"#;
    let error = compile(record, "t.ig").expect_err("a record is not an int");
    assert!(
        error.message.contains("cannot be used as a scalar"),
        "{}",
        error.message
    );

    let sum = br#"
unit t;
uses core, stdlib;

type V = A | B;

var
  v: V;
  st: Status;

procedure main(args: const string[1024]);
begin
  st := stdlib.ui_button(0, 0, 100, 20, v, false, true);
end;
"#;
    let error = compile(sum, "t.ig").expect_err("a sum is not a string");
    assert!(error.message.contains("not a buffer"), "{}", error.message);
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

// ---- maps -----------------------------------------------------------------

fn state_size(package: &Owned) -> u32 {
    CodeHeader::from_bytes(&package.code[..abi::CODE_HEADER_SIZE])
        .expect("header")
        .state_size
}

#[test]
fn maps_of_every_key_and_value_kind_compile() {
    let source = br#"
unit t;

type
  Point = record x, y: int; end;
  Sum = A | B(value: int);

var
  ints: map[int, int, 2];
  words: map[string[4], string[4], 2];
  raws: map[bytes[4], int, 2];
  records: map[int, Point, 2];
  sums: map[int, Sum, 2];
  arrays: map[int, array[int, 2], 2];
  rings: map[int, ring[int, 2], 2];
  nested: map[int, map[int, int, 2], 2];
  inner: map[int, int, 2];
  other: map[int, int, 2];
  st: Status;
  i: int;
  word: string[4];
  raw: bytes[4];
  p: Point;
  s: Sum;
  a: array[int, 2];
  r: ring[int, 2];

procedure main(args: const string[1024]);
begin
  st := core.map_put(ints, 1, 10);
  st := core.map_get(ints, 1, i);
  st := core.map_put(words, "ab", "cd");
  st := core.map_get(words, "ab", word);
  st := core.map_put(raws, "ef", 20);
  st := core.map_get(raws, "ef", i);
  p.x := 3;
  p.y := 4;
  st := core.map_put(records, 2, p);
  st := core.map_get(records, 2, p);
  s := A;
  st := core.map_put(sums, 3, s);
  st := core.map_get(sums, 3, s);
  a[0] := 5;
  st := core.map_put(arrays, 4, a);
  st := core.map_get(arrays, 4, a);
  st := core.ring_push(r, 6);
  st := core.map_put(rings, 5, r);
  st := core.map_get(rings, 5, r);
  st := core.map_put(inner, 7, 8);
  st := core.map_put(nested, 6, inner);
  st := core.map_get(nested, 6, other);
  other := ints;
  if core.map_has(ints, 1) then
    i := i + 1;
  st := core.map_remove(ints, 1);
  i := core.map_count(ints);
end;
"#;
    let package = compile(source, "t.ig").expect("maps compile");
    let ops = opcodes_of(&package);
    assert!(ops.contains(&abi::opcodes::MAP_PUT), "{ops:?}");
    assert!(ops.contains(&abi::opcodes::MAP_GET), "{ops:?}");
    assert!(ops.contains(&abi::opcodes::MAP_HAS), "{ops:?}");
    assert!(ops.contains(&abi::opcodes::MAP_REMOVE), "{ops:?}");
    assert!(ops.contains(&abi::opcodes::MAP_COUNT), "{ops:?}");
}

#[test]
fn a_map_entry_count_comes_from_a_positive_constant() {
    let good = br#"
unit t;

const
  Capacity = 1 + 1;

var
  cells: map[int, int, Capacity];

procedure main(args: const string[1024]);
begin
end;
"#;
    compile(good, "t.ig").expect("a const expression sizes a map");

    for count in ["0", "-1"] {
        let source = format!(
            r#"
unit t;

var
  cells: map[int, int, {count}];

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
fn map_table_capacity_is_the_next_power_of_two_at_least_twice_the_limit() {
    // Layout: 8-byte count plus capacity * (16 + 8 + 8). N=1 -> 2 slots (72),
    // N=3 and N=4 -> 8 slots (264), N=5 -> 16 slots (520), N=9 -> 32 (1032).
    for (count, expected) in [
        ("1", 72u32),
        ("3", 264),
        ("4", 264),
        ("5", 520),
        ("9", 1032),
    ] {
        let source = format!(
            r#"
unit t;

var
  cells: map[int, int, {count}];

procedure main(args: const string[1024]);
begin
end;
"#
        );
        let package = compile(source.as_bytes(), "t.ig").expect("compiles");
        assert_eq!(
            state_size(&package),
            expected,
            "map[int, int, {count}] size"
        );
    }
}

#[test]
fn map_key_types_are_restricted() {
    for (declared, key) in [
        ("", "f64"),
        ("", "byte"),
        ("", "bool"),
        ("Color = (Red, Green);", "Color"),
        ("Point = record x: int; end;", "Point"),
        ("Sum = A | B(value: int);", "Sum"),
        ("", "array[int, 2]"),
        ("", "ring[int, 2]"),
    ] {
        let type_section = if declared.is_empty() {
            String::new()
        } else {
            format!("type\n  {declared}\n")
        };
        let source = format!(
            r#"
unit t;

{type_section}
var
  cells: map[{key}, int, 2];

procedure main(args: const string[1024]);
begin
end;
"#
        );
        let error = compile(source.as_bytes(), "t.ig").expect_err("bad key type");
        assert!(
            error
                .message
                .contains("a map key must be `int`, `string[M]` or `bytes[M]`"),
            "{key}: {}",
            error.message
        );
    }
}

#[test]
fn map_types_are_nominal_in_key_value_and_count() {
    let cases = [
        (
            "a: map[int, string[4], 2];\n  b: map[int, string[8], 2];",
            "map[int, string[4], 2]",
            "map[int, string[8], 2]",
        ),
        (
            "a: map[int, int, 2];\n  b: map[int, uint, 2];",
            "map[int, int, 2]",
            "map[int, uint, 2]",
        ),
        (
            "a: map[int, int, 2];\n  b: map[int, int, 3];",
            "map[int, int, 2]",
            "map[int, int, 3]",
        ),
    ];
    for (vars, expected, found) in cases {
        let source = format!(
            r#"
unit t;

var
  {vars}

procedure main(args: const string[1024]);
begin
  a := b;
end;
"#
        );
        let error = compile(source.as_bytes(), "t.ig").expect_err("types differ");
        assert!(
            error
                .message
                .contains(&format!("expected `{expected}`, found `{found}`")),
            "{}",
            error.message
        );
    }
}

#[test]
fn map_buffer_keys_follow_at_least_n_and_keep_their_kind() {
    let good = br#"
unit t;

var
  m: map[string[8], int, 2];

function store(target: var map[string[8], int, 2]; key: const string[16]): Status;
begin
  return core.map_put(target, key, 1);
end;

procedure main(args: const string[1024]);
var
  name: string[8];
  wide: string[16];
begin
  name := "x";
  wide := "z";
  if core.map_put(m, "y", 2) = Ok then
    if store(m, wide) = Ok then
      core.write("ok\n");
end;
"#;
    compile(good, "t.ig").expect("a wider buffer key is accepted");

    // A narrower key parameter is rejected.
    let source = br#"
unit t;

var
  m: map[string[8], int, 2];

procedure main(args: const string[1024]);
var
  key: string[4];
begin
  key := "x";
  core.map_put(m, key, 1);
end;
"#;
    let error = compile(source, "t.ig").expect_err("too narrow");
    assert!(
        error.message.contains("at least 8 bytes"),
        "{}",
        error.message
    );

    // A literal longer than the key capacity is rejected.
    let source = br#"
unit t;

var
  m: map[string[8], int, 2];

procedure main(args: const string[1024]);
begin
  core.map_put(m, "abcdefghij", 1);
end;
"#;
    let error = compile(source, "t.ig").expect_err("literal too long");
    assert!(
        error.message.contains("does not fit map key"),
        "{}",
        error.message
    );

    // `string` and `bytes` do not mix, in either direction.
    let source = br#"
unit t;

var
  m: map[string[8], int, 2];
  raw: bytes[8];

procedure main(args: const string[1024]);
begin
  core.map_put(m, raw, 1);
end;
"#;
    let error = compile(source, "t.ig").expect_err("bytes key into a string map");
    assert!(
        error
            .message
            .contains("expected `string`, found `bytes[8]`"),
        "{}",
        error.message
    );

    let source = br#"
unit t;

var
  m: map[bytes[8], int, 2];
  text: string[8];

procedure main(args: const string[1024]);
begin
  core.map_put(m, text, 1);
end;
"#;
    let error = compile(source, "t.ig").expect_err("string key into a bytes map");
    assert!(
        error
            .message
            .contains("expected `bytes`, found `string[8]`"),
        "{}",
        error.message
    );
}

#[test]
fn map_calls_check_receiver_arity_and_value_types() {
    // Non-map receiver.
    let source = br#"
unit t;

var
  x: int;

procedure main(args: const string[1024]);
begin
  x := 1;
  core.map_put(x, 1, 1);
end;
"#;
    let error = compile(source, "t.ig").expect_err("x is not a map");
    assert!(
        error
            .message
            .contains("`core.map_put` expects a `map[K, V, N]` first argument"),
        "{}",
        error.message
    );

    // Wrong argument count.
    let source = br#"
unit t;

var
  m: map[int, int, 2];

procedure main(args: const string[1024]);
begin
  core.map_count(m, 1);
end;
"#;
    let error = compile(source, "t.ig").expect_err("one argument only");
    assert!(
        error
            .message
            .contains("`core.map_count` expects 1 argument, got 2"),
        "{}",
        error.message
    );

    // Wrong value type.
    let source = br#"
unit t;

var
  m: map[int, int, 2];

procedure main(args: const string[1024]);
begin
  core.map_put(m, 1, 1.5);
end;
"#;
    let error = compile(source, "t.ig").expect_err("value must be int");
    assert!(
        error
            .message
            .contains("`core.map_put` expects value `int`, found `f64`"),
        "{}",
        error.message
    );

    // Wrong get destination type.
    let source = br#"
unit t;

var
  m: map[int, int, 2];

procedure main(args: const string[1024]);
var
  out: uint;
begin
  core.map_get(m, 1, out);
end;
"#;
    let error = compile(source, "t.ig").expect_err("destination must be int");
    assert!(
        error
            .message
            .contains("`core.map_get` expects `var value: int`, found `uint`"),
        "{}",
        error.message
    );

    // A string key in an int-keyed map.
    let source = br#"
unit t;

var
  m: map[int, int, 2];

procedure main(args: const string[1024]);
begin
  core.map_put(m, "x", 1);
end;
"#;
    let error = compile(source, "t.ig").expect_err("string key in an int map");
    assert!(
        error
            .message
            .contains("`core.map_put` expects key `int`, found `string[1]`"),
        "{}",
        error.message
    );

    // A map is not comparable.
    let source = br#"
unit t;

var
  a: map[int, int, 2];
  b: map[int, int, 2];

procedure main(args: const string[1024]);
begin
  if a = b then
    core.write("same\n");
end;
"#;
    let error = compile(source, "t.ig").expect_err("maps do not compare");
    assert!(
        error.message.contains("cannot be used as a scalar value"),
        "{}",
        error.message
    );
}

#[test]
fn mutating_a_const_map_parameter_is_rejected_but_value_is_writable() {
    let source = br#"
unit t;

function peek(source: const map[int, int, 2]): Status;
begin
  return core.map_put(source, 1, 1);
end;

procedure main(args: const string[1024]);
begin
end;
"#;
    let error = compile(source, "t.ig").expect_err("const is read-only");
    assert!(
        error
            .message
            .contains("cannot mutate `const` parameter `source`"),
        "{}",
        error.message
    );

    let source = br#"
unit t;

function fill(source: map[int, int, 2]): int;
var
  status: Status;
begin
  status := core.map_put(source, 1, 10);
  return core.map_count(source);
end;

procedure main(args: const string[1024]);
var
  m: map[int, int, 2];
begin
  if fill(m) = 0 then
    core.write("empty\n");
end;
"#;
    compile(source, "t.ig").expect("a value map parameter is a writable copy");
}

#[test]
fn map_positions_and_parameters_work() {
    let source = br#"
unit t;

type
  Box = record m: map[int, int, 2]; end;
  Maybe = Empty | Holds(m: map[int, int, 2]);

var
  box: Box;
  maybe: Maybe;
  cells: array[map[int, int, 2], 2];
  queue: ring[map[int, int, 2], 2];
  local: map[int, int, 2];
  inner: map[int, int, 2];
  outer: map[int, int, 2];
  st: Status;
  value: int;

function peek(source: const map[int, int, 2]): int;
begin
  value := -1;
  if core.map_get(source, 1, value) <> Ok then
    return -1;
  return value;
end;

function store(target: var map[int, int, 2]): Status;
begin
  return core.map_put(target, 1, 10);
end;

procedure main(args: const string[1024]);
begin
  st := core.map_put(box.m, 1, 10);
  st := core.map_get(box.m, 1, value);
  st := core.map_put(cells[0], 2, 20);
  st := core.map_get(cells[0], 2, value);
  st := core.map_put(inner, 3, 30);
  st := core.ring_push(queue, inner);
  st := core.ring_pop(queue, inner);
  maybe := Holds(inner);
  case maybe of
    Empty: value := 0;
    Holds(m): st := core.map_put(m, 4, 40);
  end;
  st := store(local);
  value := peek(local);
  outer := local;
  st := core.map_remove(outer, 1);
  if core.map_count(outer) = 0 then
    core.write("removed\n");
end;
"#;
    compile(source, "t.ig").expect("maps in every position compile");
}

#[test]
fn map_calls_emit_the_map_opcodes() {
    let source = br#"
unit t;

var
  m: map[string[8], int, 2];
  out: int;
  st: Status;
  word: string[8];

procedure main(args: const string[1024]);
begin
  word := "a";
  st := core.map_put(m, word, 1);
  st := core.map_get(m, word, out);
  if core.map_has(m, word) then
    out := out + 1;
  st := core.map_remove(m, word);
  out := core.map_count(m);
end;
"#;
    let package = compile(source, "t.ig").expect("compiles");
    let ops = opcodes_of(&package);
    for opcode in [
        abi::opcodes::MAP_PUT,
        abi::opcodes::MAP_GET,
        abi::opcodes::MAP_HAS,
        abi::opcodes::MAP_REMOVE,
        abi::opcodes::MAP_COUNT,
    ] {
        assert!(ops.contains(&opcode), "missing {opcode:#x} in {ops:?}");
    }
    assert!(
        !ops.contains(&abi::opcodes::BUILTIN),
        "maps are opcodes, not host builtins"
    );
}

#[test]
fn the_map_sample_compiles() {
    let source = include_bytes!("../../samples/lang/map.ig");
    let package = compile(source, "map.ig").expect("the map sample compiles");
    // Smallest arena the sample accepts, found by bisection: the real scratch
    // cost, because `compile_into` caps the arena to its slice length.
    let mut low = 4 * 1024usize;
    let mut high = compiler::MAX_ARENA_BYTES;
    while low < high {
        let mid = (low + high) / 2;
        let mut arena = vec![0u8; mid];
        let mut code = vec![0u8; compiler::MAX_CODE_BYTES];
        let mut manifest = vec![0u8; compiler::MAX_MANIFEST_BYTES];
        match compiler::compile_into(source, "map.ig", &mut arena, &mut code, &mut manifest) {
            Ok(_) => high = mid,
            Err(error) if error.status == abi::Status::CapacityExceeded => low = mid + 1,
            Err(error) => panic!("unexpected diagnostic: {error}"),
        }
    }
    println!(
        "map sample: source {} B, code {} B (package {} B), state {} B, stack {} B, arena {} B",
        source.len(),
        CodeHeader::from_bytes(&package.code[..abi::CODE_HEADER_SIZE])
            .expect("header")
            .code_len,
        package.code.len(),
        state_size(&package),
        CodeHeader::from_bytes(&package.code[..abi::CODE_HEADER_SIZE])
            .expect("header")
            .stack_size,
        low,
    );
}

#[test]
fn compile_cost_of_a_map_is_bounded_and_reported() {
    // The same application with and without one map isolates the cost of the
    // type and its operations.
    let without = br#"
unit cost;
uses core;
procedure main(args: const string[1024]);
begin
  core.write("map\n");
end;
"#;
    let with = br#"
unit cost;
uses core;
var
  m: map[int, int, 4];
procedure main(args: const string[1024]);
var
  value: int;
begin
  if core.map_put(m, 1, 2) = Ok then
    value := core.map_count(m);
  if core.map_get(m, 1, value) = Ok then
    core.write("map\n");
end;
"#;
    let plain = compile(without, "cost.ig").expect("compiles");
    let map = compile(with, "cost.ig").expect("compiles");

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
    let map_arena = minimum_arena(with);
    println!(
        "map cost: code {} B (app alone {} B), state {} B, stack {} B, arena {} B (app alone {} B)",
        map.code.len(),
        plain.code.len(),
        state_size(&map),
        CodeHeader::from_bytes(&map.code[..abi::CODE_HEADER_SIZE])
            .expect("header")
            .stack_size,
        map_arena,
        plain_arena,
    );
    assert!(
        map_arena <= compiler::MAX_ARENA_BYTES,
        "the map app fits the published arena cap"
    );
    assert!(map.code.len() > plain.code.len());
    assert_eq!(
        state_size(&map),
        264,
        "map[int, int, 4] is 8 + 8 slots of 32"
    );
}

#[test]
fn realtime_on_and_heartbeat_emit_their_builtin_ids() {
    use abi::opcodes::{self, builtin};

    let source = br#"
unit t;

uses core;

var
  st: Status;
  i: int;

procedure main(args: const string[1024]);
begin
  st := core.realtime_on();
  i := 0;
  while i < 3 do
  begin
    core.heartbeat();
    i := i + 1;
  end;
end;
"#;
    let package = compile(source, "t.ig").expect("realtime primitives are known");
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
    assert_eq!(seen, vec![builtin::REALTIME_ON, builtin::HEARTBEAT]);
}

#[test]
fn realtime_primitives_reject_arguments() {
    let realtime = br#"
unit t;

uses core;

var
  st: Status;

procedure main(args: const string[1024]);
begin
  st := core.realtime_on(1);
end;
"#;
    let error = compile(realtime, "t.ig").expect_err("realtime_on takes no argument");
    assert!(
        error
            .message
            .contains("`core.realtime_on` expects 0 arguments"),
        "{}",
        error.message
    );

    let heartbeat = br#"
unit t;

uses core;

procedure main(args: const string[1024]);
begin
  core.heartbeat(1);
end;
"#;
    let error = compile(heartbeat, "t.ig").expect_err("heartbeat takes no argument");
    assert!(
        error
            .message
            .contains("`core.heartbeat` expects 0 arguments"),
        "{}",
        error.message
    );
}

// ---- streams (spec 0006) ------------------------------------------------

/// Decodes the manifest's endpoint descriptors.
fn endpoints(package: &Owned) -> Vec<(String, u8, u32)> {
    let header = ManifestHeader::from_bytes(&package.manifest[..32]).expect("manifest header");
    let mut offset = 32 + header.unit_name_len as usize;
    let mut out = Vec::new();
    for _ in 0..header.endpoint_count {
        let descriptor = abi::Endpoint::from_bytes(&package.manifest[offset..offset + 8])
            .expect("endpoint descriptor");
        offset += 8;
        let name = &package.manifest[offset..offset + descriptor.name_len as usize];
        offset += descriptor.name_len as usize;
        out.push((
            String::from_utf8(name.to_vec()).expect("UTF-8 name"),
            descriptor.direction,
            descriptor.capacity,
        ));
    }
    out
}

#[test]
fn stream_endpoints_are_emitted_in_declaration_order_with_handles_first() {
    let source = br#"
unit t;

uses core;

var
  events_out: out_stream[128];
  counter: int;
  commands_in: in_stream[64];

procedure main(args: const string[1024]);
begin
  counter := 0;
end;
"#;
    let package = compile(source, "t.ig").expect("compiles");
    let header = CodeHeader::from_bytes(&package.code[..40]).expect("code header");
    // Two 8-byte handles at the front, then the `int` state variable.
    assert_eq!(header.state_size, 24);
    assert_eq!(
        endpoints(&package),
        vec![
            ("events_out".to_string(), abi::direction::OUT, 128),
            ("commands_in".to_string(), abi::direction::IN, 64),
        ]
    );
}

#[test]
fn a_stream_may_be_a_const_parameter_with_any_capacity() {
    let source = br#"
unit t;

uses core;

var
  events_out: out_stream[128];

procedure send_it(ep: const out_stream[64]; text: const string[0]);
begin
  core.send(ep, text);
end;

procedure main(args: const string[1024]);
begin
  send_it(events_out, "x");
end;
"#;
    compile(source, "t.ig").expect("a const stream parameter borrows the caller's handle");
}

#[test]
fn a_framing_helper_accepts_any_endpoint_capacity() {
    // The framing helpers declare their endpoints as `const out_stream[1]` /
    // `const in_stream[1]`: the declared capacity of a stream parameter is
    // only a lower bound that the compiler does not check, so one helper
    // serves every capacity (spec 0003, "Streams").
    let source = br#"
unit t;

uses core, stdlib;

var
  events_out: out_stream[128];
  commands_in: in_stream[65536];
  frame: bytes[256];

procedure main(args: const string[1024]);
begin
  stdlib.send_line(events_out, "hi");
  stdlib.send_frame(events_out, "hi");
  stdlib.send_u64(events_out, 1);
  stdlib.send_bytes(events_out, frame);
  stdlib.recv_line_into(commands_in, frame);
  stdlib.recv_frame_into(commands_in, frame);
end;
"#;
    compile(source, "t.ig").expect("a `const ..._stream[1]` parameter accepts every capacity");
}

#[test]
fn stream_parameter_capacity_is_not_checked_but_the_mode_is() {
    let accepted = br#"
unit t;

uses core;

var
  events_out: out_stream[65536];
  commands_in: in_stream[128];

procedure send_it(ep: const out_stream[1]);
begin
  core.send(ep, "x");
end;

procedure recv_it(ep: const in_stream[1]; dst: var bytes[8]);
begin
  core.recv(ep, dst);
end;

procedure main(args: const string[1024]);
var
  raw: bytes[8];
begin
  send_it(events_out);
  recv_it(commands_in, raw);
end;
"#;
    compile(accepted, "t.ig")
        .expect("a `const` stream parameter of capacity 1 accepts any capacity");

    let cases: &[(&str, &str)] = &[
        (
            "procedure p(ep: var out_stream[1]);\nbegin\nend;",
            "stream parameter `ep` must be declared `const`",
        ),
        (
            "procedure p(ep: out_stream[1]);\nbegin\nend;",
            "stream parameter `ep` must be declared `const`",
        ),
        (
            "procedure p(ep: var in_stream[1]);\nbegin\nend;",
            "stream parameter `ep` must be declared `const`",
        ),
    ];
    for (procedure, expected) in cases {
        let source = format!(
            "unit t;\n\nuses core;\n\nvar\n  s: out_stream[8];\n\n{procedure}\n\nprocedure main(args: const string[1024]);\nbegin\nend;\n"
        );
        let error = compile(source.as_bytes(), "t.ig").expect_err("must be rejected");
        assert!(
            error.message.contains(expected),
            "expected `{expected}`, got `{}`",
            error.message
        );
    }
}

#[test]
fn stream_type_rules_are_compile_errors() {
    let cases: &[(&str, &str)] = &[
        (
            "var s: in_stream[0];",
            "stream capacity must be between 1 and 65536",
        ),
        (
            "var s: out_stream[65537];",
            "stream capacity must be between 1 and 65536",
        ),
        (
            "type R = record s: out_stream[8]; end;",
            "is a stream endpoint; it may only be declared",
        ),
        (
            "var xs: array[out_stream[8], 2];",
            "is a stream endpoint; it may only be declared",
        ),
        (
            "var Bad_name: out_stream[8];",
            "must match `[a-z][a-z0-9_]*`",
        ),
        (
            "var s: out_stream[8]; var s: in_stream[8];",
            "duplicate declaration",
        ),
    ];
    for (declaration, expected) in cases {
        let source = format!(
            "unit t;\n\nuses core;\n\n{declaration}\n\nprocedure main(args: const string[1024]);\nbegin\nend;\n"
        );
        let error =
            compile(source.as_bytes(), "t.ig").expect_err("declaration must be a compile error");
        assert!(
            error.message.contains(expected),
            "{declaration}: expected `{expected}`, got `{}`",
            error.message
        );
    }
}

#[test]
fn a_stream_local_is_rejected() {
    let source = br#"
unit t;

uses core;

procedure main(args: const string[1024]);
var
  local_s: out_stream[8];
begin
end;
"#;
    let error = compile(source, "t.ig").expect_err("a stream local must not compile");
    assert!(
        error
            .message
            .contains("is a stream endpoint; it may only be declared"),
        "{}",
        error.message
    );
}

#[test]
fn stream_parameters_and_values_are_checked() {
    let cases: &[(&str, &str)] = &[
        (
            r#"
unit t;
uses core;
var s: out_stream[8];
procedure p(ep: out_stream[8]);
begin
end;
procedure main(args: const string[1024]);
begin
end;
"#,
            "stream parameter `ep` must be declared `const`",
        ),
        (
            r#"
unit t;
uses core;
var s: out_stream[8];
procedure main(args: const string[1024]);
begin
  s := s;
end;
"#,
            "stream endpoint `s` cannot be assigned",
        ),
        (
            r#"
unit t;
uses core;
var s: out_stream[8];
procedure main(args: const string[1024]);
begin
  if s = s then
    core.write("x");
end;
"#,
            "cannot be used as a value",
        ),
        (
            r#"
unit t;
uses core;
var s: in_stream[8];
procedure main(args: const string[1024]);
begin
  core.send(s, "x");
end;
"#,
            "need an `out_stream`",
        ),
        (
            r#"
unit t;
uses core;
var s: in_stream[8];
var text: string[8];
procedure main(args: const string[1024]);
begin
  core.recv(s, text);
end;
"#,
            "needs a writable `var bytes[N]` destination",
        ),
    ];
    for (source, expected) in cases {
        let error = compile(source.as_bytes(), "t.ig").expect_err("must be a compile error");
        assert!(
            error.message.contains(expected),
            "expected `{expected}`, got `{}`",
            error.message
        );
    }
}

// ---- syslib graphics (spec 0009) ------------------------------------------

#[test]
fn syslib_graphics_surface_is_accepted_and_emits_its_builtins() {
    use abi::opcodes::{self, builtin};

    let source = br#"
unit t;

uses core, syslib;

var
  status: Status;
  pixel: uint;
  width: uint;
  height: uint;

procedure main(args: const string[1024]);
begin
  width := syslib.screen_width();
  height := syslib.screen_height();
  status := syslib.fill_rect(1, 2, 3, 4, uint(0x123456));
  status := syslib.fill_dither(1, 2, 3, 4, uint(0));
  status := syslib.draw_rect(1, 2, 3, 4, uint(0));
  status := syslib.draw_text(1, 2, "Hi", uint(0));
  pixel := syslib.get_pixel(1, 2);
  status := syslib.present(0, 0, 10, 10);
  status := syslib.clear(uint(0));
end;
"#;
    let package = compile(source, "t.ig").expect("the graphics surface compiles");
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
        vec![
            builtin::SCREEN_WIDTH,
            builtin::SCREEN_HEIGHT,
            builtin::FILL_RECT,
            builtin::FILL_DITHER,
            builtin::DRAW_RECT,
            builtin::DRAW_TEXT,
            builtin::GET_PIXEL,
            builtin::PRESENT,
            builtin::CLEAR,
        ]
    );
}

#[test]
fn syslib_graphics_argument_types_and_arity_are_checked() {
    let cases = [
        (
            r#"
unit t;
uses core, syslib;
var status: Status;
procedure main(args: const string[1024]);
begin
  status := syslib.fill_rect(1, 2, 3, uint(0));
end;
"#,
            "`syslib.fill_rect` expects 5 arguments",
        ),
        (
            r#"
unit t;
uses core, syslib;
var status: Status;
procedure main(args: const string[1024]);
begin
  status := syslib.fill_rect(uint(1), 2, 3, 4, uint(0));
end;
"#,
            "`syslib.fill_rect` expects an `int` x",
        ),
        (
            r#"
unit t;
uses core, syslib;
var status: Status;
procedure main(args: const string[1024]);
begin
  status := syslib.draw_rect(1, 2, 3, 4, 0);
end;
"#,
            "`syslib.draw_rect` expects a `uint` color",
        ),
        (
            r#"
unit t;
uses core, syslib;
var pixel: uint;
procedure main(args: const string[1024]);
begin
  pixel := syslib.get_pixel(1);
end;
"#,
            "`syslib.get_pixel` expects 2 arguments",
        ),
        (
            r#"
unit t;
uses core, syslib;
var width: uint;
procedure main(args: const string[1024]);
begin
  width := syslib.screen_width(uint(1));
end;
"#,
            "`syslib.screen_width` expects 0 arguments",
        ),
        (
            r#"
unit t;
uses core, syslib;
var status: Status;
var raw: bytes[8];
procedure main(args: const string[1024]);
begin
  status := syslib.draw_text(1, 2, raw, uint(0));
end;
"#,
            "expected `string`",
        ),
    ];
    for (source, expected) in cases {
        let error = compile(source.as_bytes(), "t.ig").expect_err("must be a compile error");
        assert!(
            error.message.contains(expected),
            "expected `{expected}`, got `{}`",
            error.message
        );
    }
}

// ---- syslib proportional font (spec 0015) ---------------------------------

#[test]
fn syslib_proportional_font_surface_is_accepted_and_emits_its_builtins() {
    use abi::opcodes::{self, builtin};

    let source = br#"
unit t;

uses core, syslib;

var
  status: Status;
  width: uint;
  height: uint;

procedure main(args: const string[1024]);
begin
  width := syslib.text_width_prop("ii MM");
  height := syslib.text_height_prop();
  status := syslib.draw_text_prop(1, 2, "Hi", uint(0x102030));
end;
"#;
    let package = compile(source, "t.ig").expect("the proportional font surface compiles");
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
        vec![
            builtin::TEXT_WIDTH_PROP,
            builtin::TEXT_HEIGHT_PROP,
            builtin::DRAW_TEXT_PROP,
        ]
    );
    // The ids are part of the ABI surface and must not move.
    assert_eq!(builtin::TEXT_WIDTH_PROP, 45);
    assert_eq!(builtin::TEXT_HEIGHT_PROP, 46);
    assert_eq!(builtin::DRAW_TEXT_PROP, 47);
    assert_eq!(builtin::effect(builtin::TEXT_WIDTH_PROP), Some((1, 1)));
    assert_eq!(builtin::effect(builtin::TEXT_HEIGHT_PROP), Some((0, 1)));
    assert_eq!(builtin::effect(builtin::DRAW_TEXT_PROP), Some((4, 1)));
}

#[test]
fn syslib_proportional_font_argument_types_and_arity_are_checked() {
    let cases = [
        (
            r#"
unit t;
uses core, syslib;
var width: uint;
procedure main(args: const string[1024]);
begin
  width := syslib.text_width_prop();
end;
"#,
            "`syslib.text_width_prop` expects 1 argument",
        ),
        (
            r#"
unit t;
uses core, syslib;
var width: uint;
var raw: bytes[8];
procedure main(args: const string[1024]);
begin
  width := syslib.text_width_prop(raw);
end;
"#,
            "expected `string`",
        ),
        (
            r#"
unit t;
uses core, syslib;
var height: uint;
procedure main(args: const string[1024]);
begin
  height := syslib.text_height_prop(uint(1));
end;
"#,
            "`syslib.text_height_prop` expects 0 arguments",
        ),
        (
            r#"
unit t;
uses core, syslib;
var status: Status;
procedure main(args: const string[1024]);
begin
  status := syslib.draw_text_prop(1, 2, "Hi");
end;
"#,
            "`syslib.draw_text_prop` expects 4 arguments",
        ),
        (
            r#"
unit t;
uses core, syslib;
var status: Status;
procedure main(args: const string[1024]);
begin
  status := syslib.draw_text_prop(uint(1), 2, "Hi", uint(0));
end;
"#,
            "`syslib.draw_text_prop` expects an `int` x",
        ),
        (
            r#"
unit t;
uses core, syslib;
var status: Status;
procedure main(args: const string[1024]);
begin
  status := syslib.draw_text_prop(1, 2, "Hi", 0);
end;
"#,
            "`syslib.draw_text_prop` expects a `uint` color",
        ),
    ];
    for (source, expected) in cases {
        let error = compile(source.as_bytes(), "t.ig").expect_err("must be a compile error");
        assert!(
            error.message.contains(expected),
            "expected `{expected}`, got `{}`",
            error.message
        );
    }
}

// ---- syslib pointer (spec 0010) -------------------------------------------

#[test]
fn syslib_pointer_surface_is_accepted_and_emits_its_builtins() {
    use abi::opcodes::{self, builtin};

    let source = br#"
unit t;

uses core, syslib;

var
  status: Status;
  px: int;
  py: int;
  buttons: uint;
  source: uint;

procedure main(args: const string[1024]);
begin
  px := syslib.pointer_x();
  py := syslib.pointer_y();
  buttons := syslib.pointer_buttons();
  source := syslib.pointer_source();
  status := syslib.inject_pointer(10, -20, uint(1));
  status := syslib.inject_pointer(0, 0, uint(7));
end;
"#;
    let package = compile(source, "t.ig").expect("the pointer surface compiles");
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
        vec![
            builtin::POINTER_X,
            builtin::POINTER_Y,
            builtin::POINTER_BUTTONS,
            builtin::POINTER_SOURCE,
            builtin::INJECT_POINTER,
            builtin::INJECT_POINTER,
        ]
    );
}

#[test]
fn syslib_pointer_argument_types_and_arity_are_checked() {
    let cases = [
        (
            r#"
unit t;
uses core, syslib;
var status: Status;
procedure main(args: const string[1024]);
begin
  status := syslib.inject_pointer(1, 2);
end;
"#,
            "`syslib.inject_pointer` expects 3 arguments",
        ),
        (
            r#"
unit t;
uses core, syslib;
var status: Status;
procedure main(args: const string[1024]);
begin
  status := syslib.inject_pointer(uint(1), 2, uint(0));
end;
"#,
            "`syslib.inject_pointer` expects an `int` dx",
        ),
        (
            r#"
unit t;
uses core, syslib;
var status: Status;
procedure main(args: const string[1024]);
begin
  status := syslib.inject_pointer(1, 2, 0);
end;
"#,
            "`syslib.inject_pointer` expects a `uint` buttons",
        ),
        (
            r#"
unit t;
uses core, syslib;
var px: int;
procedure main(args: const string[1024]);
begin
  px := syslib.pointer_x(uint(1));
end;
"#,
            "`syslib.pointer_x` expects 0 arguments",
        ),
        (
            r#"
unit t;
uses core, syslib;
var source: uint;
procedure main(args: const string[1024]);
begin
  source := syslib.pointer_source(uint(1));
end;
"#,
            "`syslib.pointer_source` expects 0 arguments",
        ),
    ];
    for (source, expected) in cases {
        let error = compile(source.as_bytes(), "t.ig").expect_err("must be a compile error");
        assert!(
            error.message.contains(expected),
            "expected `{expected}`, got `{}`",
            error.message
        );
    }
}

// ---- syslib events (spec 0010, M2.2.3-M2.2.5) -----------------------------

#[test]
fn syslib_event_surface_is_accepted_and_emits_its_builtins() {
    use abi::opcodes::{self, builtin};

    let source = br#"
unit t;

uses core, syslib;

var
  ev: Event;
  status: Status;
  drops: uint;
  flushed: uint;

procedure main(args: const string[1024]);
begin
  ev.kind := KeyDown;
  ev.x := 0;
  ev.y := 0;
  ev.code := uint(65);
  ev.window := 0;
  status := syslib.poll_event(ev);
  status := syslib.next_event(ev);
  status := syslib.inject_event(ev);
  drops := syslib.event_drops();
  flushed := syslib.event_flush();
end;
"#;
    let package = compile(source, "t.ig").expect("the event surface compiles");
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
        vec![
            builtin::POLL_EVENT,
            builtin::NEXT_EVENT,
            builtin::INJECT_EVENT,
            builtin::EVENT_DROPS,
            builtin::EVENT_FLUSH,
        ]
    );
}

#[test]
fn syslib_event_argument_types_and_arity_are_checked() {
    let cases = [
        (
            r#"
unit t;
uses core, syslib;
var status: Status;
procedure main(args: const string[1024]);
begin
  status := syslib.poll_event();
end;
"#,
            "`syslib.poll_event` expects 1 argument",
        ),
        (
            r#"
unit t;
uses core, syslib;
var status: Status;
procedure main(args: const string[1024]);
begin
  status := syslib.next_event(uint(1));
end;
"#,
            "`var ev` must be a variable, field or element",
        ),
        (
            r#"
unit t;
uses core, syslib;
var status: Status;
procedure poke(ev: const Event);
begin
  status := syslib.next_event(ev);
end;
procedure main(args: const string[1024]);
begin
end;
"#,
            "cannot pass `const` value `ev` as `var`",
        ),
        (
            r#"
unit t;
uses core, syslib;
var status: Status;
procedure main(args: const string[1024]);
begin
  status := syslib.inject_event(uint(1));
end;
"#,
            "`ev` must be a variable, field or element",
        ),
        (
            r#"
unit t;
uses core, syslib;
var drops: uint;
procedure main(args: const string[1024]);
begin
  drops := syslib.event_drops(uint(1));
end;
"#,
            "`syslib.event_drops` expects 0 arguments",
        ),
    ];
    for (source, expected) in cases {
        let error = compile(source.as_bytes(), "t.ig").expect_err("must be a compile error");
        assert!(
            error.message.contains(expected),
            "expected `{expected}`, got `{}`",
            error.message
        );
    }
}

#[test]
fn event_and_event_kind_are_predeclared_and_cannot_be_redeclared() {
    let cases = [
        (
            r#"
unit t;
type Event = record x: int; end;
procedure main(args: const string[1024]);
begin
end;
"#,
            "`Event` is predeclared",
        ),
        (
            r#"
unit t;
type EventKind = (Mine);
procedure main(args: const string[1024]);
begin
end;
"#,
            "`EventKind` is predeclared",
        ),
    ];
    for (source, expected) in cases {
        let error = compile(source.as_bytes(), "t.ig").expect_err("must be a compile error");
        assert!(
            error.message.contains(expected),
            "expected `{expected}`, got `{}`",
            error.message
        );
    }
}

// ---- syslib window service (spec 0011, M2.3.1) ------------------------------

#[test]
fn syslib_window_surface_is_accepted_and_emits_its_builtins() {
    use abi::opcodes::{self, builtin};

    let source = br#"
unit t;

uses core, syslib;

var
  id: int;
  other: int;
  status: Status;
  rect: Rect;
  focus: int;

procedure main(args: const string[1024]);
begin
  status := syslib.window_open(10, 20, 100, 50, "one", id);
  status := syslib.window_raise(id);
  status := syslib.window_move(id, 30, 40);
  status := syslib.window_frame_rect(id, rect);
  other := rect.x;
  status := syslib.window_content_rect(id, rect);
  other := rect.h;
  status := syslib.window_present(id);
  status := syslib.window_close(id);
  focus := syslib.window_focus();
end;
"#;
    let package = compile(source, "t.ig").expect("the window surface compiles");
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
        vec![
            builtin::WINDOW_OPEN,
            builtin::WINDOW_RAISE,
            builtin::WINDOW_MOVE,
            builtin::WINDOW_FRAME_RECT,
            builtin::WINDOW_CONTENT_RECT,
            builtin::WINDOW_PRESENT,
            builtin::WINDOW_CLOSE,
            builtin::WINDOW_FOCUS,
        ]
    );
}

#[test]
fn syslib_window_argument_types_and_arity_are_checked() {
    let cases = [
        (
            r#"
unit t;
uses core, syslib;
var status: Status;
procedure main(args: const string[1024]);
begin
  status := syslib.window_open(1, 2, 3, 4, "x");
end;
"#,
            "`syslib.window_open` expects 6 arguments",
        ),
        (
            r#"
unit t;
uses core, syslib;
var status: Status;
var id: int;
var raw: bytes[8];
procedure main(args: const string[1024]);
begin
  status := syslib.window_open(1, 2, 3, 4, raw, id);
end;
"#,
            "expected `string`, found `bytes[8]`",
        ),
        (
            r#"
unit t;
uses core, syslib;
var status: Status;
var id: int;
procedure main(args: const string[1024]);
begin
  status := syslib.window_open(1, 2, 3, 4, "x", id, id);
end;
"#,
            "`syslib.window_open` expects 6 arguments",
        ),
        (
            r#"
unit t;
uses core, syslib;
var status: Status;
var id: int;
procedure main(args: const string[1024]);
begin
  status := syslib.window_open(1, 2, 3, 4, "x", status);
end;
"#,
            "`syslib.window_open` expects `var id: int`",
        ),
        (
            r#"
unit t;
uses core, syslib;
var status: Status;
procedure main(args: const string[1024]);
begin
  status := syslib.window_close(uint(1));
end;
"#,
            "`syslib.window_close` expects an `int` window id",
        ),
        (
            r#"
unit t;
uses core, syslib;
var status: Status;
var id: int;
procedure main(args: const string[1024]);
begin
  status := syslib.window_move(id, 1);
end;
"#,
            "`syslib.window_move` expects 3 arguments",
        ),
        (
            r#"
unit t;
uses core, syslib;
var status: Status;
var id: int;
var focus: int;
procedure main(args: const string[1024]);
begin
  status := syslib.window_frame_rect(id, focus);
end;
"#,
            "`var rect` expects a record, sum, array, ring or map value",
        ),
        (
            r#"
unit t;
uses core, syslib;
var status: Status;
var id: int;
var rect: Rect;
procedure poke(r: const Rect);
begin
  status := syslib.window_content_rect(id, r);
end;
procedure main(args: const string[1024]);
begin
end;
"#,
            "cannot pass `const` value `r` as `var`",
        ),
        (
            r#"
unit t;
uses core, syslib;
var status: Status;
var focus: int;
procedure main(args: const string[1024]);
begin
  focus := syslib.window_focus(uint(1));
end;
"#,
            "`syslib.window_focus` expects 0 arguments",
        ),
        (
            r#"
unit t;
uses core, syslib;
var status: Status;
procedure main(args: const string[1024]);
begin
  status := syslib.window_present();
end;
"#,
            "`syslib.window_present` expects 1 argument",
        ),
        (
            r#"
unit t;
uses core, syslib;
var status: Status;
procedure main(args: const string[1024]);
begin
  status := syslib.window_present(uint(1));
end;
"#,
            "`syslib.window_present` expects an `int` window id",
        ),
    ];
    for (source, expected) in cases {
        let error = compile(source.as_bytes(), "t.ig").expect_err("must be a compile error");
        assert!(
            error.message.contains(expected),
            "expected `{expected}`, got `{}`",
            error.message
        );
    }
}

#[test]
fn expose_is_a_valid_event_kind() {
    let source = br#"
unit t;

uses core, syslib;

var
  ev: Event;
  status: Status;

procedure main(args: const string[1024]);
begin
  ev.kind := Expose;
  ev.x := 0;
  ev.y := 0;
  ev.code := uint(0);
  ev.window := 7;
  status := syslib.inject_event(ev);
  status := syslib.poll_event(ev);
  if ev.kind = Expose then
    status := syslib.window_present(ev.window);
end;
"#;
    compile(source, "t.ig").expect("Expose is a predeclared EventKind constant");
}

#[test]
fn rect_is_predeclared_and_cannot_be_redeclared() {
    let source = br#"
unit t;
type Rect = record x: int; end;
procedure main(args: const string[1024]);
begin
end;
"#;
    let error = compile(source, "t.ig").expect_err("must be a compile error");
    assert!(
        error.message.contains("`Rect` is predeclared"),
        "{}",
        error.message
    );
}

#[test]
fn window_info_is_builtin_forty_and_checks_its_signature() {
    use abi::opcodes::{self, builtin};

    let source = r#"
unit t;
uses core, syslib;
var
  status: Status;
  id: int;
  rect: Rect;
  title: string[64];
procedure main(args: const string[1024]);
begin
  status := syslib.window_info(uint(0), id, rect, title);
  status := syslib.window_info(uint(1), id, rect, title);
end;
"#;
    let package = compile(source.as_bytes(), "t.ig").expect("the enumeration compiles");
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
    assert!(seen.contains(&builtin::WINDOW_INFO));
    assert!(!seen.contains(&builtin::WINDOW_PRESENT));

    let cases = [
        (
            r#"
unit t;
uses core, syslib;
var status: Status; id: int; rect: Rect; title: string[64];
procedure main(args: const string[1024]);
begin
  status := syslib.window_info(uint(0), id, rect);
end;
"#,
            "`syslib.window_info` expects 4 arguments",
        ),
        (
            r#"
unit t;
uses core, syslib;
var status: Status; id: int; rect: Rect; title: string[64];
procedure main(args: const string[1024]);
begin
  status := syslib.window_info(0, id, rect, title);
end;
"#,
            "`syslib.window_info` expects a `uint` index",
        ),
        (
            r#"
unit t;
uses core, syslib;
var status: Status; count: uint; rect: Rect; title: string[64];
procedure main(args: const string[1024]);
begin
  status := syslib.window_info(uint(0), count, rect, title);
end;
"#,
            "`syslib.window_info` expects `var id: int`",
        ),
        (
            r#"
unit t;
uses core, syslib;
var status: Status; id: int; ev: Event; title: string[64];
procedure main(args: const string[1024]);
begin
  status := syslib.window_info(uint(0), id, ev, title);
end;
"#,
            "`syslib.window_info` expects `var rect: Rect`",
        ),
        (
            r#"
unit t;
uses core, syslib;
var status: Status; id: int; rect: Rect; raw: bytes[64];
procedure main(args: const string[1024]);
begin
  status := syslib.window_info(uint(0), id, rect, raw);
end;
"#,
            "`syslib.window_info` expects a `var string` title",
        ),
        (
            r#"
unit t;
uses core, syslib;
var status: Status; id: int; rect: Rect; title: string[32];
procedure main(args: const string[1024]);
begin
  status := syslib.window_info(uint(0), id, rect, title);
end;
"#,
            "needs a title buffer of at least 64 bytes",
        ),
    ];
    for (source, expected) in cases {
        let error = compile(source.as_bytes(), "t.ig").expect_err("must be a compile error");
        assert!(
            error.message.contains(expected),
            "expected `{expected}`, got `{}`",
            error.message
        );
    }
}

#[test]
fn fs_primitives_are_builtin_ids_41_to_43_and_check_their_signatures() {
    use abi::opcodes::{self, builtin};

    let source = r#"
unit t;
uses core;
var
  status: Status;
  name: string[64];
  kind: uint;
  size: uint;
  read: int;
  chunk: bytes[256];
procedure main(args: const string[1024]);
begin
  status := core.fs_list("", uint(0), name, kind, size);
  status := core.fs_list(".", uint(1), name, kind, size);
  status := core.fs_read("child", uint(0), chunk, read);
  status := core.fs_size("child", size);
end;
"#;
    let package = compile(source.as_bytes(), "t.ig").expect("the file primitives compile");
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
    for id in [builtin::FS_LIST, builtin::FS_READ, builtin::FS_SIZE] {
        assert!(seen.contains(&id), "builtin {id} is emitted");
    }
    assert_eq!(
        (builtin::FS_LIST, builtin::FS_READ, builtin::FS_SIZE),
        (41, 42, 43)
    );

    let cases = [
        (
            r#"
unit t;
uses core;
var status: Status; name: string[64]; kind: uint; size: uint;
procedure main(args: const string[1024]);
begin
  status := core.fs_list("", uint(0), name, kind);
end;
"#,
            "`core.fs_list` expects 5 arguments",
        ),
        (
            r#"
unit t;
uses core;
var status: Status; raw: bytes[16]; name: string[64]; kind: uint; size: uint;
procedure main(args: const string[1024]);
begin
  status := core.fs_list(raw, uint(0), name, kind, size);
end;
"#,
            "`core.fs_list` expects a `string` path",
        ),
        (
            r#"
unit t;
uses core;
var status: Status; name: string[64]; kind: uint; size: uint;
procedure main(args: const string[1024]);
begin
  status := core.fs_list("", 0, name, kind, size);
end;
"#,
            "`core.fs_list` expects a `uint` index",
        ),
        (
            r#"
unit t;
uses core;
var status: Status; raw: bytes[64]; kind: uint; size: uint;
procedure main(args: const string[1024]);
begin
  status := core.fs_list("", uint(0), raw, kind, size);
end;
"#,
            "`core.fs_list` expects a `var string` name",
        ),
        (
            r#"
unit t;
uses core;
var status: Status; name: string[32]; kind: uint; size: uint;
procedure main(args: const string[1024]);
begin
  status := core.fs_list("", uint(0), name, kind, size);
end;
"#,
            "needs a name buffer of at least 64 bytes",
        ),
        (
            r#"
unit t;
uses core;
var status: Status; name: string[64]; total: int; size: uint;
procedure main(args: const string[1024]);
begin
  status := core.fs_list("", uint(0), name, total, size);
end;
"#,
            "`core.fs_list` expects `var kind: uint`",
        ),
        (
            r#"
unit t;
uses core;
var status: Status; name: string[64]; kind: uint; read: int;
procedure main(args: const string[1024]);
begin
  status := core.fs_list("", uint(0), name, kind, read);
end;
"#,
            "`core.fs_list` expects `var size: uint`",
        ),
        (
            r#"
unit t;
uses core;
var status: Status; read: int; chunk: bytes[256];
procedure main(args: const string[1024]);
begin
  status := core.fs_read("child", 0, chunk, read);
end;
"#,
            "`core.fs_read` expects a `uint` offset",
        ),
        (
            r#"
unit t;
uses core;
var status: Status; read: int; text: string[256];
procedure main(args: const string[1024]);
begin
  status := core.fs_read("child", uint(0), text, read);
end;
"#,
            "needs a writable `var bytes` destination of at least 256 bytes",
        ),
        (
            r#"
unit t;
uses core;
var status: Status; read: int; chunk: bytes[128];
procedure main(args: const string[1024]);
begin
  status := core.fs_read("child", uint(0), chunk, read);
end;
"#,
            "needs a writable `var bytes` destination of at least 256 bytes",
        ),
        (
            r#"
unit t;
uses core;
var status: Status; count: uint; chunk: bytes[256];
procedure main(args: const string[1024]);
begin
  status := core.fs_read("child", uint(0), chunk, count);
end;
"#,
            "`core.fs_read` expects `var read: int`",
        ),
        (
            r#"
unit t;
uses core;
var status: Status; count: int;
procedure main(args: const string[1024]);
begin
  status := core.fs_size("child", count);
end;
"#,
            "`core.fs_size` expects `var size: uint`",
        ),
    ];
    for (source, expected) in cases {
        let error = compile(source.as_bytes(), "t.ig").expect_err("must be a compile error");
        assert!(
            error.message.contains(expected),
            "expected `{expected}`, got `{}`",
            error.message
        );
    }
}

#[test]
fn terminal_attach_is_builtin_forty_four_and_checks_its_signature() {
    use abi::opcodes::{self, builtin};

    let source = r#"
unit t;
uses core, syslib;
var
  status: Status;
  id: int;
procedure main(args: const string[1024]);
begin
  status := core.terminal_attach(id);
  status := core.terminal_attach(9);
end;
"#;
    let package = compile(source.as_bytes(), "t.ig").expect("terminal_attach compiles");
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
        vec![builtin::TERMINAL_ATTACH, builtin::TERMINAL_ATTACH]
    );
    assert_eq!(builtin::TERMINAL_ATTACH, 44);

    let cases = [
        (
            r#"
unit t;
uses core, syslib;
var status: Status;
procedure main(args: const string[1024]);
begin
  status := core.terminal_attach();
end;
"#,
            "`core.terminal_attach` expects 1 argument",
        ),
        (
            r#"
unit t;
uses core, syslib;
var status: Status;
procedure main(args: const string[1024]);
begin
  status := core.terminal_attach(uint(1));
end;
"#,
            "`core.terminal_attach` expects an `int` window id",
        ),
    ];
    for (source, expected) in cases {
        let error = compile(source.as_bytes(), "t.ig").expect_err("must be a compile error");
        assert!(error.message.contains(expected), "{}", error.message);
    }
}

#[test]
fn halt_and_reboot_are_builtins_48_and_49_and_take_no_arguments() {
    use abi::opcodes::{self, builtin};

    let source = r#"
unit t;
uses core;
var status: Status;
procedure main(args: const string[1024]);
begin
  status := core.halt();
  status := core.reboot();
end;
"#;
    let package = compile(source.as_bytes(), "t.ig").expect("system control compiles");
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
    assert_eq!(seen, vec![builtin::HALT, builtin::REBOOT]);
    assert_eq!(builtin::HALT, 48);
    assert_eq!(builtin::REBOOT, 49);

    let cases = [
        (
            r#"
unit t;
uses core;
var status: Status;
procedure main(args: const string[1024]);
begin
  status := core.halt(1);
end;
"#,
            "`core.halt` expects 0 arguments",
        ),
        (
            r#"
unit t;
uses core;
var status: Status;
procedure main(args: const string[1024]);
begin
  status := core.reboot(uint(1));
end;
"#,
            "`core.reboot` expects 0 arguments",
        ),
    ];
    for (source, expected) in cases {
        let error = compile(source.as_bytes(), "t.ig").expect_err("must be a compile error");
        assert!(error.message.contains(expected), "{}", error.message);
    }
}
