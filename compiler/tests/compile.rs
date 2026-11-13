//! Compiler-level tests: packages, determinism and diagnostics.

use abi::{CodeHeader, ABI_VERSION, CODE_MAGIC, PROFILE_X86_64_LE};

const HELLO: &[u8] = include_bytes!("../../samples/lang/hello.ig");
const LOOP: &[u8] = include_bytes!("../../samples/lang/loop.ig");

#[test]
fn hello_emits_a_well_formed_header() {
    let package = compiler::compile(HELLO, "hello.ig").expect("compiles");
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
    let first = compiler::compile(LOOP, "loop.ig").expect("compiles");
    let second = compiler::compile(LOOP, "loop.ig").expect("compiles");
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
    let error = compiler::compile(source, "t.ig").expect_err("must not compile");
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
    let error = compiler::compile(source, "t.ig").expect_err("must not compile");
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
    let error = compiler::compile(source, "t.ig").expect_err("must not compile");
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
    let error = compiler::compile(source, "t.ig").expect_err("must not compile");
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
    let error = compiler::compile(source, "t.ig").expect_err("must not compile");
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
    let error = compiler::compile(source, "t.ig").expect_err("must not compile");
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
    let error = compiler::compile(source, "t.ig").expect_err("must not compile");
    assert!(
        error.message.contains("main must be declared"),
        "{}",
        error.message
    );
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
    let error = compiler::compile(source, "t.ig").expect_err("must not compile");
    assert!(
        error.message.contains("not valid UTF-8"),
        "{}",
        error.message
    );
}
