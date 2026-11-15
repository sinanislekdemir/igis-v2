//! End-to-end tests: source → package → verified program → output.

use abi::{ExitReason, FaultCode, Status};
use interpreter::Host;

fn sample(name: &str) -> Vec<u8> {
    let path = format!("{}/../samples/lang/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|error| panic!("cannot read {path}: {error}"))
}

fn run_sample(name: &str, args: &[u8]) -> langhost::RunResult {
    let source = sample(name);
    let package = langhost::compile(&source, name).expect("compiles");
    langhost::run(&package.code, args, langhost::DEFAULT_BUDGET).expect("verifies")
}

struct Recorder {
    output: Vec<u8>,
}

impl Host for Recorder {
    fn console_write(&mut self, bytes: &[u8]) {
        self.output.extend_from_slice(bytes);
    }

    fn clock(&mut self) -> u64 {
        42
    }
}

#[test]
fn hello_world_compiles_and_runs() {
    let result = run_sample("hello.ig", b"");
    assert_eq!(result.output, b"hello, igis\n");
    assert_eq!(result.reason, ExitReason::Ok);
}

#[test]
fn loop_and_branches_produce_the_expected_text() {
    let result = run_sample("loop.ig", b"");
    assert_eq!(
        result.output,
        b"even\nodd\neven\nodd\neven\ntotal=abc\n".to_vec()
    );
    assert_eq!(result.reason, ExitReason::Ok);
}

#[test]
fn concatenation_and_parameters_work() {
    let result = run_sample("concat.ig", b"");
    assert_eq!(result.output, b"greeting: hello, igis\n");
    assert_eq!(result.reason, ExitReason::Ok);
}

#[test]
fn short_circuit_logic_works() {
    let result = run_sample("logic.ig", b"");
    assert_eq!(result.output, b"and\nor\nnot\n");
    assert_eq!(result.reason, ExitReason::Ok);
}

#[test]
fn float_arithmetic_and_casts_work() {
    let result = run_sample("float.ig", b"");
    assert_eq!(result.output, b"float ok\nclock ok\n");
    assert_eq!(result.reason, ExitReason::Ok);
}

#[test]
fn var_scalar_parameters_alias_the_caller() {
    let result = run_sample("params.ig", b"");
    assert_eq!(result.output, b"params ok\n");
    assert_eq!(result.reason, ExitReason::Ok);
}

#[test]
fn bytes_accept_raw_data_that_string_rejects() {
    let result = run_sample("bytes.ig", b"");
    assert_eq!(result.output, b"a\xffb\n");
    assert_eq!(result.reason, ExitReason::Ok);
}

#[test]
fn main_receives_the_argument_buffer() {
    let result = run_sample("args.ig", b"one two");
    assert_eq!(result.output, b"args=[one two]\n");
    assert_eq!(result.reason, ExitReason::Ok);

    let empty = run_sample("args.ig", b"");
    assert_eq!(empty.output, b"args=[]\n");
}

#[test]
fn a_runtime_capacity_overflow_faults() {
    let result = run_sample("runtime_fault.ig", b"");
    assert_eq!(result.output, b"before\n");
    assert_eq!(
        result.reason,
        ExitReason::Faulted(FaultCode::CapacityExceeded)
    );
    assert_eq!(result.fault_at, Some((15, 3)));
}

#[test]
fn compiling_twice_is_byte_identical() {
    let source = sample("loop.ig");
    let first = langhost::compile(&source, "loop.ig").expect("compiles");
    let second = langhost::compile(&source, "loop.ig").expect("compiles");
    assert_eq!(first.code, second.code);
    assert_eq!(first.manifest, second.manifest);
}

#[test]
fn oversized_literal_is_a_compile_error_with_a_position() {
    let error = langhost::compile(&sample("bad_capacity.ig"), "bad_capacity.ig")
        .expect_err("must not compile");
    assert_eq!(error.line, 8);
    assert_eq!(error.column, 11);
    assert!(error.message.contains("does not fit"), "{}", error.message);
    assert_eq!(error.unit, "bad_capacity");
}

#[test]
fn unknown_identifier_is_a_compile_error_with_a_position() {
    let error = langhost::compile(&sample("bad_unknown.ig"), "bad_unknown.ig")
        .expect_err("must not compile");
    assert_eq!(error.line, 5);
    assert_eq!(error.column, 3);
    assert!(
        error.message.contains("unknown identifier `total`"),
        "{}",
        error.message
    );
}

#[test]
fn missing_main_is_a_diagnostic() {
    let error = langhost::compile(&sample("bad_no_main.ig"), "bad_no_main.ig")
        .expect_err("must not compile");
    assert_eq!(error.line, 1);
    assert!(
        error.message.contains("missing `main`"),
        "{}",
        error.message
    );
}

#[test]
fn a_malformed_package_is_refused_by_the_verifier() {
    let source = sample("hello.ig");
    let package = langhost::compile(&source, "hello.ig").expect("compiles");

    let mut scratch = vec![0u8; package.code.len().div_ceil(32)];
    interpreter::verify(&package.code, &mut scratch).expect("the compiler output verifies");

    // Flip the first instruction byte to an undefined opcode.
    let mut corrupted = package.code.clone();
    let instructions = 40 + 24; // padded to 64 by the compiler
    let start = (instructions + 7) & !7;
    corrupted[start] = 0xEE;
    let error = interpreter::verify(&corrupted, &mut scratch).expect_err("must be refused");
    assert_eq!(error.status, Status::Invalid);
}

#[test]
fn host_trait_implementation_sees_application_bytes() {
    // `run` already uses its own host; this checks the trait is usable the way
    // the kernel will use it: a caller-supplied Host over the same machine.
    let source = sample("hello.ig");
    let package = langhost::compile(&source, "hello.ig").expect("compiles");
    let mut scratch = vec![0u8; package.code.len().div_ceil(32)];
    let program = interpreter::Program::parse(&package.code, &mut scratch).expect("verifies");
    let header = *program.header();
    let mut state = vec![0u8; header.state_size as usize];
    let mut stack = vec![0u8; header.stack_size as usize];
    let args = vec![0u8; 8 + 1024];
    let mut machine = interpreter::Machine::new();
    let mut host = Recorder { output: Vec::new() };
    let outcome = machine.run(
        &program,
        &mut host,
        &mut state,
        &mut stack,
        &args,
        langhost::DEFAULT_BUDGET,
    );
    assert_eq!(outcome, interpreter::Outcome::Exited(ExitReason::Ok));
    assert_eq!(host.output, b"hello, igis\n");
    assert_eq!(args[..8], 0u64.to_le_bytes());
}

#[test]
fn stage_app_samples_compile_with_the_spawn_builtins() {
    for name in [
        "shell",
        "child",
        "clock",
        "faulty",
        "streams",
        "pipe_in",
        "pipe_out",
        "streams_bad",
        "frames",
        "frame_in",
        "frame_out",
    ] {
        let path = format!("{}/../samples/apps/{name}", env!("CARGO_MANIFEST_DIR"));
        let source = std::fs::read(&path).unwrap_or_else(|error| panic!("read {path}: {error}"));
        let package = langhost::compile(&source, name).unwrap_or_else(|error| {
            panic!("{name} fails to compile: {error}");
        });
        assert!(!package.code.is_empty(), "{name} produced no code");
    }
}

#[test]
fn clock_app_prints_zeroed_time_with_the_mock_ports() {
    let path = format!("{}/../samples/apps/clock", env!("CARGO_MANIFEST_DIR"));
    let source = std::fs::read(&path).unwrap_or_else(|error| panic!("read {path}: {error}"));
    let package = langhost::compile(&source, "clock").expect("clock compiles");
    let result = langhost::run(&package.code, b"", langhost::DEFAULT_BUDGET).expect("verifies");
    assert_eq!(result.reason, ExitReason::Ok);
    assert_eq!(
        result.output, b"00:00:00\n",
        "the host mock reads zero from every port"
    );
}

#[test]
fn shell_spawns_and_waits_for_the_child_on_the_host() {
    let path = format!("{}/../samples/apps/shell", env!("CARGO_MANIFEST_DIR"));
    let source = std::fs::read(&path).unwrap_or_else(|error| panic!("read {path}: {error}"));
    let package = langhost::compile(&source, "shell").expect("shell compiles");
    let result = langhost::run(&package.code, b"", langhost::DEFAULT_BUDGET).expect("verifies");
    assert_eq!(result.reason, ExitReason::Ok);
    let text = String::from_utf8(result.output).expect("UTF-8 output");
    for line in [
        "shell: spawning child\n",
        "shell: child task id assigned\n",
        "child: args hello\n",
        "child: bye\n",
        "child: ok\n",
        "shell: help, echo <text>, run <app> [args], exit\n",
        "child: args world\n",
        "hi there\n",
    ] {
        assert!(text.contains(line), "missing {line:?} in {text:?}");
    }
    // The host `read_line` stub scripts `help`, `run child world`,
    // `echo hi there`, `exit`, so the same command loop the kernel runs prints
    // four prompts and exits without an unknown command.
    assert_eq!(
        text.matches("shell> ").count(),
        4,
        "the shell prompts once per scripted line: {text:?}"
    );
    assert!(
        text.contains("shell> shell: help, echo <text>, run <app> [args], exit\n"),
        "{text:?}"
    );
    assert!(text.contains("shell> hi there\n"), "{text:?}");
    assert!(
        text.ends_with("shell> bye\n"),
        "the shell exits after the scripted `exit`: {text:?}"
    );
    assert!(!text.contains("unknown"), "{text:?}");
    assert!(!text.contains("spawn failed"), "{text:?}");
}

#[test]
fn indexing_length_and_buffer_equality_work_end_to_end() {
    let result = run_sample("index.ig", b"");
    assert_eq!(
        result.output,
        b"extend ok\nlen ok\nzero ok\nwrite ok\nneq ok\neq ok\nbyte ok\n".to_vec()
    );
    assert_eq!(result.reason, ExitReason::Ok);
}

#[test]
fn find_and_substr_work_end_to_end() {
    let result = run_sample("slice.ig", b"");
    assert_eq!(
        result.output,
        b"find hit ok\nfind miss ok\nsubstr ok\nclear ok\npast end ok\nutf8 ok\nutf8 slice ok\nbytes ok\n"
            .to_vec()
    );
    assert_eq!(result.reason, ExitReason::Ok);
}

#[test]
fn utf8_helpers_and_mixed_slicing_work_end_to_end() {
    let result = run_sample("utf8.ig", b"");
    assert_eq!(
        String::from_utf8(result.output).expect("the sample writes ASCII"),
        "len empty ok\n\
         len ascii ok\n\
         len mixed ok\n\
         decode 1-byte ok\n\
         decode 2-byte ok\n\
         decode 3-byte ok\n\
         decode 4-byte ok\n\
         decode continuation ok\n\
         decode at end ok\n\
         decode negative ok\n\
         decode into ok\n\
         decode into capacity ok\n\
         decode into stray ok\n\
         decode into overlong ok\n\
         decode into surrogate ok\n\
         decode into too large ok\n\
         decode into truncated ok\n\
         valid empty ok\n\
         valid ascii ok\n\
         valid mixed ok\n\
         substr boundaries ok\n\
         substr split ok\n\
         substr past end ok\n\
         substr negative ok\n\
         substr raw slice ok\n\
         substr bytes to string ok\n\
         substr bytes invalid ok\n\
         split multibyte ok\n"
    );
    assert_eq!(result.reason, ExitReason::Ok);
}

#[test]
fn an_empty_literal_assignment_leaves_no_operand_behind() {
    // Regression: `dst := ""` used to push the destination reference for an
    // `append` it never emitted, so one value stayed on the run-time operand
    // stack. A following five-operand call then overflowed a full operand area.
    let source = r#"
unit emptyliteral;

uses core, stdlib;

var
  text: string[32];
  blank: string[8];
  scalar: int;
  next: int;
  st: Status;

procedure main(args: const string[1024]);
begin
  text := "aé€😀";
  blank := "";
  st := stdlib.utf8_decode(text, 0, scalar, next);
  if st = Ok then
    core.write("empty literal ok\n")
  else
    core.write("empty literal bad\n");
end;
"#;
    let package = langhost::compile(source.as_bytes(), "emptyliteral").expect("compiles");
    let result = langhost::run(&package.code, b"", langhost::DEFAULT_BUDGET).expect("verifies");
    assert_eq!(result.reason, ExitReason::Ok);
    assert_eq!(result.output, b"empty literal ok\n");
}

#[test]
fn enums_carry_ordinals_and_check_casts_end_to_end() {
    let source = br#"
unit enumapp;

uses core;

type Color = (Red, Green, Blue);

var
  c: Color;
  text: string[8];
  n: int;
  ok: bool;

procedure main(args: const string[1024]);
begin
  c := Green;
  n := core.format_int_into(text, int(c), uint(10), uint(0));
  core.write(text);
  core.write("\n");
  ok := c = Green;
  if ok then core.write("eq ok\n");
  ok := Red < Blue;
  if ok then core.write("order ok\n");
  c := Color(2);
  ok := c = Blue;
  if ok then core.write("cast ok\n");
  ok := c <> Red;
  if ok then core.write("ne ok\n");
end;
"#;
    let package = langhost::compile(source, "enumapp").expect("enums compile");
    let result = langhost::run(&package.code, b"", langhost::DEFAULT_BUDGET).expect("verifies");
    assert_eq!(result.reason, ExitReason::Ok);
    assert_eq!(result.output, b"1\neq ok\norder ok\ncast ok\nne ok\n");
}

#[test]
fn an_out_of_range_enum_cast_faults_overflow() {
    let source = br#"
unit badenum;

type Color = (Red, Green);

var
  c: Color;

procedure main(args: const string[1024]);
begin
  c := Red;
  c := Color(5);
end;
"#;
    let package = langhost::compile(source, "badenum").expect("compiles");
    let result = langhost::run(&package.code, b"", langhost::DEFAULT_BUDGET).expect("verifies");
    assert_eq!(result.reason, ExitReason::Faulted(FaultCode::Overflow));
    assert_eq!(result.fault_at.map(|(line, _)| line), Some(12));
}

#[test]
fn spawn_and_kill_report_status_values_on_the_host() {
    let source = br#"
unit spawnstatus;

uses core;

var
  st: Status;
  id: int;
  diag: Diagnostic;

procedure main(args: const string[1024]);
begin
  st := core.spawn("does_not_exist", "", diag, id);
  if st = NotFound then
    core.write("spawn not found ok\n")
  else
    core.write("wrong spawn status\n");
  if diag.message = "No such application" then
    core.write("diagnostic ok\n")
  else
    core.write("wrong diagnostic\n");
  st := core.kill(id);
  if st = NotFound then
    core.write("kill not found ok\n")
  else
    core.write("wrong kill status\n");
end;
"#;
    let package = langhost::compile(source, "spawnstatus").expect("compiles");
    let result = langhost::run(&package.code, b"", langhost::DEFAULT_BUDGET).expect("verifies");
    assert_eq!(result.reason, ExitReason::Ok);
    assert_eq!(
        result.output,
        b"spawn not found ok\ndiagnostic ok\nkill not found ok\n".to_vec()
    );
}

#[test]
fn records_fields_and_value_parameters_work_end_to_end() {
    let source = br#"
unit records;

type Point = record x, y: int; end;
type Named = record label: string[8]; at: Point; end;

var
  n: Named;
  p: Point;
  copy: Named;
  text: string[16];

procedure set(q: var Point; x: int; y: int);
begin
  q.x := x;
  q.y := y;
end;

// A `value` record parameter is a copy: `bump` must not touch the caller.
procedure bump(q: Point);
begin
  q.x := q.x + 100;
end;

procedure main(args: const string[1024]);
begin
  set(p, 3, 4);
  n.at := p;
  n.label := "n";
  copy := n;
  // A `value` parameter is a copy: `bump` leaves `copy.at.x` alone.
  bump(copy.at);
  core.write(copy.label);
  core.format_int_into(text, copy.at.x, uint(10), uint(0));
  core.write(text);
  core.write("/");
  core.format_int_into(text, n.at.x, uint(10), uint(0));
  core.write(text);
  core.write("/");
  // A `var` parameter is a reference to the caller's nested field.
  set(copy.at, 9, 9);
  core.format_int_into(text, copy.at.x, uint(10), uint(0));
  core.write(text);
  core.write("\n");
end;
"#;
    let package = langhost::compile(source, "records").expect("records compile");
    let result = langhost::run(&package.code, b"", langhost::DEFAULT_BUDGET).expect("verifies");
    assert_eq!(result.reason, ExitReason::Ok);
    assert_eq!(result.output, b"n3/3/9\n");
}

#[test]
fn sum_types_and_case_work_end_to_end() {
    let result = run_sample("case.ig", b"");
    assert_eq!(result.reason, ExitReason::Ok);
    assert_eq!(
        result.output,
        b"number 42\nnumber 42\nident igis\neof\nident keep\nident keep\nnested number 7\nh: number 3\nvar: eof\nnumber 7\n"
            .to_vec()
    );
}

#[test]
fn an_exit_reason_is_constructed_and_matched_with_case() {
    let source = br#"
unit reasoncase;

uses core;

var
  reason: ExitReason;
  st: Status;

procedure main(args: const string[1024]);
begin
  reason := Ok;
  case reason of
    Ok: core.write("ok\n")
    Killed: core.write("wrong\n")
    Faulted(code): core.write("wrong\n")
  end;

  st := Ok;
  if st = Ok then core.write("status still ok\n");

  reason := Killed;
  case reason of
    Ok: core.write("wrong\n")
    Killed: core.write("killed\n")
    Faulted(code): core.write("wrong\n")
  end;

  reason := Faulted(FaultCode.Overflow);
  case reason of
    Ok: core.write("wrong\n")
    Killed: core.write("wrong\n")
    Faulted(code):
    begin
      if code = FaultCode.Overflow then
        core.write("faulted overflow\n")
      else
        core.write("wrong code\n");
    end
  end;
end;
"#;
    let package = langhost::compile(source, "reasoncase").expect("ExitReason compiles");
    let result = langhost::run(&package.code, b"", langhost::DEFAULT_BUDGET).expect("verifies");
    assert_eq!(result.reason, ExitReason::Ok);
    assert_eq!(
        result.output,
        b"ok\nstatus still ok\nkilled\nfaulted overflow\n".to_vec()
    );
}

#[test]
fn a_non_exhaustive_case_is_a_compile_error_with_a_position() {
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
    let error = langhost::compile(source, "t").expect_err("must not compile");
    assert!(
        error.message.contains("non-exhaustive `case`"),
        "{}",
        error.message
    );
    assert_eq!(error.line, 12);
    assert_eq!(error.column, 8);
}

#[test]
fn a_faulted_child_reports_the_faulted_variant_and_code() {
    let source = br#"
unit faultreport;

uses core;

var
  st: Status;
  id: int;
  diag: Diagnostic;
  reason: ExitReason;

procedure main(args: const string[1024]);
begin
  st := core.spawn("faulty", "", diag, id);
  if st <> Ok then
    core.write("spawn failed\n")
  else
  begin
    st := core.wait(id, reason);
    if st <> Ok then
      core.write("wait failed\n")
    else
      case reason of
        Ok: core.write("reason ok\n")
        Killed: core.write("reason killed\n")
        Faulted(code):
        begin
          core.write("reason faulted\n");
          if code = FaultCode.DivideByZero then
            core.write("code divide by zero\n")
          else
            core.write("code wrong\n");
        end
      end;
  end;
end;
"#;
    let package = langhost::compile(source, "faultreport").expect("compiles");
    let result = langhost::run(&package.code, b"", langhost::DEFAULT_BUDGET).expect("verifies");
    assert_eq!(result.reason, ExitReason::Ok);
    // `faulty` prints one line before dividing by zero; the parent then matches
    // `ExitReason.Faulted` and binds `code = FaultCode.DivideByZero`.
    let text = String::from_utf8(result.output).expect("UTF-8");
    assert!(text.contains("faulty: about to divide\n"), "{text:?}");
    assert!(text.contains("reason faulted\n"), "{text:?}");
    assert!(text.contains("code divide by zero\n"), "{text:?}");
}

#[test]
fn core_run_reports_a_spawn_failure_through_diag_on_the_host() {
    let source = br#"
unit runfail;

uses core;

var
  st: Status;
  diag: Diagnostic;
  reason: ExitReason;

procedure main(args: const string[1024]);
begin
  st := core.run("missing", "", diag, reason);
  if st = Ok then
    core.write("unexpected ok\n")
  else
  begin
    core.write("run failed: ");
    core.write(diag.message);
    core.write("\n");
  end;
end;
"#;
    let package = langhost::compile(source, "runfail").expect("compiles");
    let result = langhost::run(&package.code, b"", langhost::DEFAULT_BUDGET).expect("verifies");
    assert_eq!(result.reason, ExitReason::Ok);
    assert_eq!(result.output, b"run failed: No such application\n");
}

#[test]
fn stdlib_helpers_trim_split_parse_and_match() {
    let result = run_sample("stdlib.ig", b"");
    assert_eq!(
        result.output,
        b"trim=[hi there] len=8\n\
split=1 head=[left] tail=[right]\n\
nosplit=0 head=[no-sep] tail=[]\n\
parse=-42 ok value=-42\n\
parse=min ok value=-9223372036854775808\n\
parse=-x invalid\n\
parse=overflow capacity exceeded\n\
prefix=yes\n\
long-prefix=no\n\
contains=yes\n\
missing=no\n"
            .to_vec()
    );
    assert_eq!(result.reason, ExitReason::Ok);
}

#[test]
fn a_kernel_sample_uses_the_stdlib_on_its_argument() {
    let path = format!("{}/../samples/apps/text", env!("CARGO_MANIFEST_DIR"));
    let source = std::fs::read(&path).unwrap_or_else(|error| panic!("cannot read {path}: {error}"));
    let package = langhost::compile(&source, "text").expect("compiles");
    let result =
        langhost::run(&package.code, b"42 hi", langhost::DEFAULT_BUDGET).expect("verifies");
    assert_eq!(
        result.output,
        b"text: trimmed=[42 hi] len=5\n\
text: split=1 head=[42] tail=[hi]\n\
text: tail is not an integer (Invalid)\n\
text: head starts with 4\n\
text: trimmed has a space\n"
            .to_vec()
    );
    assert_eq!(result.reason, ExitReason::Ok);

    let result = langhost::run(&package.code, b"7 42", langhost::DEFAULT_BUDGET).expect("verifies");
    assert_eq!(
        result.output,
        b"text: trimmed=[7 42] len=4\n\
text: split=1 head=[7] tail=[42]\n\
text: tail is 42\n\
text: head does not start with 4\n\
text: trimmed has a space\n"
            .to_vec()
    );
}

#[test]
fn fixed_size_arrays_read_write_copy_and_split() {
    let result = run_sample("array.ig", b"");
    assert_eq!(
        result.output,
        b"20\n30\n10\n406\n100\n5\n16\nrow\n7\n5\n97\nalpha\n4\nbb\n0\n".to_vec()
    );
    assert_eq!(result.reason, ExitReason::Ok);
}

#[test]
fn an_array_index_out_of_range_faults() {
    let result = run_sample("array_bounds.ig", b"");
    assert_eq!(result.output, b"".to_vec(), "nothing after the fault runs");
    assert_eq!(result.reason, ExitReason::Faulted(FaultCode::OutOfBounds));
}

#[test]
fn fixed_capacity_rings_push_pop_peek_and_wrap() {
    let result = run_sample("ring.ig", b"");
    assert_eq!(
        result.output,
        b"10\n20\n30\n40\n40\n1\n0\n3\n99\n1\n5\n1\n1\n2\n3\n4\n5\n8\n8\n0\n0\n100\n100\n100\nalpha\nbeta\n3\nkeep\n7\n11\n1\n2\n62\n1\n".to_vec()
    );
    assert_eq!(result.reason, ExitReason::Ok);
}

#[test]
fn a_value_ring_parameter_is_a_private_writable_copy() {
    let result = run_sample("ring_value.ig", b"");
    assert_eq!(
        result.output,
        b"10\n10\n10\n20\n3\n7\n1\n".to_vec(),
        "the callee mutates its copy; the caller is unchanged, including through a value record"
    );
    assert_eq!(result.reason, ExitReason::Ok);
}
