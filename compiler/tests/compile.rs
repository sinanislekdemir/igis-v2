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
