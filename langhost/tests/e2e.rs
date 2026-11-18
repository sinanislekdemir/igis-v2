//! End-to-end tests: source → package → verified program → output.

use abi::{ExitReason, FaultCode, Status};
use interpreter::Host;

fn sample(name: &str) -> Vec<u8> {
    let path = format!("{}/../samples/lang/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|error| panic!("cannot read {path}: {error}"))
}

/// Path of a staged application sample (spec 0018): applications live under
/// `samples/apps/base`, kernel/boot-owned probes under `samples/apps/system`.
fn app_path(name: &str) -> String {
    let root = format!("{}/../samples/apps", env!("CARGO_MANIFEST_DIR"));
    for directory in ["base", "system"] {
        let path = format!("{root}/{directory}/{name}");
        if std::path::Path::new(&path).is_file() {
            return path;
        }
    }
    panic!("sample `{name}` is not under samples/apps/base or samples/apps/system")
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
        "floats",
        "text",
        "desktop",
        "ui_layout",
        "module_demo",
    ] {
        let path = app_path(name);
        let source = std::fs::read(&path).unwrap_or_else(|error| panic!("read {path}: {error}"));
        let package = langhost::compile(&source, name).unwrap_or_else(|error| {
            panic!("{name} fails to compile: {error}");
        });
        assert!(!package.code.is_empty(), "{name} produced no code");
    }
}

#[test]
fn module_demo_imports_lib_math_on_the_host() {
    // spec 0018 M1–M13, M3.3; ADR 0039 M4.1/M4.2: the staged `/lib` proof
    // reads `lib/math.ig` and `lib/probe.ig` from the same tree the kernel's
    // image reader walks, so the host run pins the module paths, the five
    // `math` helpers' results, the predeclared types crossing into `probe` and
    // `probe`'s own record, sum and enum crossing back into the application.
    let path = app_path("module_demo");
    let source = std::fs::read(&path).unwrap_or_else(|error| panic!("read {path}: {error}"));
    let package = langhost::compile(&source, "module_demo").expect("module_demo compiles");
    let result = langhost::run(&package.code, b"", langhost::DEFAULT_BUDGET).expect("verifies");
    assert_eq!(result.reason, ExitReason::Ok);
    assert_eq!(
        String::from_utf8(result.output).expect("UTF-8 output"),
        "module_demo: types ok\nmodule_demo: lib types ok\nmodule_demo: ok\n"
    );
}

#[test]
fn clock_app_prints_zeroed_time_with_the_mock_ports() {
    let path = app_path("clock");
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
    let path = app_path("shell");
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
    // The host console scripts `help`, `run child world`, `echo hi there`,
    // `exit`, so the same command loop the kernel runs prints four prompts and
    // exits without an unknown command. The shell, not the kernel, now echoes
    // each typed byte, so the typed text appears after the prompt.
    assert_eq!(
        text.matches("shell> ").count(),
        4,
        "the shell prompts once per scripted line: {text:?}"
    );
    assert!(
        text.contains("shell> help\nshell: help, echo <text>, run <app> [args], exit\n"),
        "{text:?}"
    );
    assert!(
        text.contains("shell> echo hi there\nhi there\n"),
        "{text:?}"
    );
    assert!(
        text.ends_with("shell> exit\nbye\n"),
        "the shell exits after the scripted `exit`: {text:?}"
    );
    assert!(!text.contains("unknown"), "{text:?}");
    assert!(!text.contains("spawn failed"), "{text:?}");
}

#[test]
fn shell_edits_the_line_with_backspace_on_the_host() {
    let path = app_path("shell");
    let source = std::fs::read(&path).unwrap_or_else(|error| panic!("read {path}: {error}"));
    let package = langhost::compile(&source, "shell").expect("shell compiles");
    // Type `helpzz`, delete the two `z`s with DEL, then submit `help`.
    let result = langhost::run_with_input(
        &package.code,
        b"",
        b"helpzz\x7f\x7f\nexit\n",
        langhost::DEFAULT_BUDGET,
    )
    .expect("verifies");
    assert_eq!(result.reason, ExitReason::Ok);
    let text = String::from_utf8(result.output).expect("UTF-8 output");
    assert!(
        text.contains(
            "shell> helpzz\x08 \x08\x08 \x08\nshell: help, echo <text>, run <app> [args], exit\n"
        ),
        "the shell echoes the deletion and runs the edited line: {text:?}"
    );
    assert!(text.ends_with("shell> exit\nbye\n"), "{text:?}");
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
    let path = app_path("text");
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

#[test]
fn parse_f64_into_accepts_decimal_forms_and_reports_invalid() {
    let source = br#"
unit f64parse;

uses core, stdlib;

var
  text: string[32];
  value: f64;
  st: Status;

procedure show(src: const string[0]);
var
  n: int;
begin
  st := stdlib.parse_f64_into(src, value);
  if st <> Ok then
    core.write("Invalid\n")
  else
  begin
    n := core.format_f64_into(text, value, uint(3));
    core.write("[");
    core.write(text);
    core.write("]\n");
  end;
end;

procedure main(args: const string[1024]);
begin
  show("1");
  show("-1.5");
  show("+2.25");
  show(".5");
  show("1.");
  show("1e3");
  show("2.5e-3");
  show("1E+2");
  show("inf");
  show("+inf");
  show("-inf");
  show("nan");
  show("1e400");
  show("-1e400");
  show("1e-400");
  show("-0.0");
end;
"#;
    let package = langhost::compile(source, "f64parse").expect("compiles");
    let result = langhost::run(&package.code, b"", langhost::DEFAULT_BUDGET).expect("verifies");
    assert_eq!(result.reason, ExitReason::Ok);
    assert_eq!(
        result.output,
        b"[1.000]\n[-1.500]\n[2.250]\n[0.500]\n[1.000]\n[1000.000]\n[0.003]\n\
          [100.000]\n[inf]\n[inf]\n[-inf]\n[nan]\n[inf]\n[-inf]\n[0.000]\n[-0.000]\n"
            .to_vec()
    );
}

#[test]
fn parse_f64_into_rejects_every_malformed_form() {
    let source = br#"
unit f64bad;

uses core, stdlib;

var
  text: string[32];
  value: f64;
  st: Status;
  ok: bool;
  n: int;

procedure check(src: const string[0]);
begin
  value := 7.5;
  st := stdlib.parse_f64_into(src, value);
  // A failure must write nothing: `value` stays 7.5.
  n := core.format_f64_into(text, value, uint(1));
  if st = Invalid and text = "7.5" then
    core.write("bad ok\n")
  else
    core.write("BAD\n");
end;

procedure main(args: const string[1024]);
begin
  check("");
  check("+");
  check("-");
  check(".");
  check("1e");
  check("1e+");
  check("1e-");
  check("1.2.3");
  check("1 2");
  check(" 1");
  check("1 ");
  check("e5");
  check(".e5");
  check("+nan");
  check("INF");
  check("1,5");
end;
"#;
    let package = langhost::compile(source, "f64bad").expect("compiles");
    let result = langhost::run(&package.code, b"", langhost::DEFAULT_BUDGET).expect("verifies");
    assert_eq!(result.reason, ExitReason::Ok);
    assert_eq!(result.output, b"bad ok\n".repeat(16));
}

#[test]
fn parse_f64_into_is_correctly_rounded_at_the_hard_boundaries() {
    // The expected text is produced by `core.format_f64_short_into`, the exact
    // integer implementation, so this pins the *bits* of the parsed value
    // without needing a bit-cast in the language. The repeated ×/÷10 reader
    // that Q41 replaced was 1 ulp off on `1e-7` and on subnormal boundaries.
    let source = br#"
unit f64round;

uses core, stdlib;

var
  text: string[40];
  value: f64;
  st: Status;
  n: int;

procedure show(label: const string[0]; src: const string[0]);
begin
  st := stdlib.parse_f64_into(src, value);
  core.write(label);
  core.write("=");
  if st <> Ok then
    core.write("Invalid")
  else
  begin
    n := core.format_f64_short_into(text, value);
    if n < 0 then
      core.write("failed")
    else
      core.write(text);
  end;
  core.write("\n");
end;

procedure main(args: const string[1024]);
begin
  show("tenth", "0.1");
  show("tenth-long", "0.1000000000000000055511151231257827021181583404541015625");
  show("1e-7", "1e-7");
  show("half-ulp-1", "1.00000000000000011102230246251565404236316680908203125");
  show("minnormal", "2.2250738585072012e-308");
  show("maxsubnormal", "2.2250738585072011e-308");
  show("minsub-1", "4.9406564584124654e-324");
  show("halfsub-up", "2.4703282292062328e-324");
  show("halfsub-down", "2.4703282292062327e-324");
  show("max", "1.7976931348623158e308");
  show("overflow", "1e309");
  show("underflow", "-1e-400");
  show("long", "1234567890123456789012345678901234567890e-40");
  show("tie", "9007199254740995");
end;
"#;
    let package = langhost::compile(source, "f64round").expect("compiles");
    let result = langhost::run(&package.code, b"", langhost::DEFAULT_BUDGET).expect("verifies");
    assert_eq!(result.reason, ExitReason::Ok);
    assert_eq!(
        result.output,
        b"tenth=0.1\n\
          tenth-long=0.1\n\
          1e-7=1e-7\n\
          half-ulp-1=1\n\
          minnormal=2.2250738585072014e-308\n\
          maxsubnormal=2.225073858507201e-308\n\
          minsub-1=5e-324\n\
          halfsub-up=5e-324\n\
          halfsub-down=0\n\
          max=1.7976931348623157e308\n\
          overflow=inf\n\
          underflow=-0\n\
          long=0.12345678901234568\n\
          tie=9007199254740996\n"
            .to_vec()
    );
}

#[test]
fn format_f64_short_into_prints_the_shortest_round_trip() {
    let source = br#"
unit f64short;

uses core;

var
  text: string[32];
  n: int;

procedure show(label: const string[0]; v: f64);
begin
  core.write(label);
  core.write("=");
  n := core.format_f64_short_into(text, v);
  if n < 0 then
    core.write("failed")
  else
    core.write(text);
  core.write("\n");
end;

procedure main(args: const string[1024]);
begin
  show("zero", 0.0);
  show("negzero", -0.0);
  show("tenth", 0.1);
  show("third", 1.0 / 3.0);
  show("1e20", 1.0e20);
  show("1e21", 1.0e21);
  show("1e-6", 1.0e-6);
  show("1e-7", 1.0e-7);
  show("minsub", 5.0e-324);
  show("max", 1.7976931348623157e308);
end;
"#;
    let package = langhost::compile(source, "f64short").expect("compiles");
    let result = langhost::run(&package.code, b"", langhost::DEFAULT_BUDGET).expect("verifies");
    assert_eq!(result.reason, ExitReason::Ok);
    // Round-tripping is proven against the host formatter and parser in
    // `interpreter/tests/f64short.rs`; the reader itself is pinned by
    // `parse_f64_into_is_correctly_rounded_at_the_hard_boundaries` above and
    // `interpreter/src/f64parse.rs`.
    assert_eq!(
        result.output,
        b"zero=0\nnegzero=-0\ntenth=0.1\nthird=0.3333333333333333\n\
          1e20=100000000000000000000\n1e21=1e21\n1e-6=0.000001\n1e-7=1e-7\n\
          minsub=5e-324\nmax=1.7976931348623157e308\n"
            .to_vec()
    );
}

#[test]
fn format_f64_into_and_parse_f64_into_round_trip() {
    let source = br#"
unit f64round;

uses core, stdlib;

var
  text: string[32];
  value: f64;
  back: f64;
  st: Status;
  n: int;

procedure roundtrip(v: f64; digits: uint);
begin
  n := core.format_f64_into(text, v, digits);
  core.write(text);
  core.write(" -> ");
  st := stdlib.parse_f64_into(text, back);
  if st <> Ok then
    core.write("Invalid")
  else
  begin
    n := core.format_f64_into(text, back, digits);
    if back = v then
      core.write("equal ")
    else
      core.write("near ");
    core.write(text);
  end;
  core.write("\n");
end;

procedure main(args: const string[1024]);
begin
  roundtrip(12.375, uint(3));
  roundtrip(-0.0, uint(3));
  roundtrip(0.0, uint(2));
  roundtrip(-42.5, uint(1));
  roundtrip(1.0 / 3.0, uint(9));
  roundtrip(1.0e18, uint(0));
end;
"#;
    let package = langhost::compile(source, "f64round").expect("compiles");
    let result = langhost::run(&package.code, b"", langhost::DEFAULT_BUDGET).expect("verifies");
    assert_eq!(result.reason, ExitReason::Ok);
    assert_eq!(
        result.output,
        b"12.375 -> equal 12.375\n-0.000 -> equal -0.000\n0.00 -> equal 0.00\n\
          -42.5 -> equal -42.5\n0.333333333 -> near 0.333333333\n\
          1000000000000000000 -> equal 1000000000000000000\n"
            .to_vec()
    );
}

#[test]
fn window_calls_on_a_host_without_graphics_are_invalid() {
    let source = br#"
unit t;

uses core, syslib;

var
  id: int;
  status: Status;
  rect: Rect;
  focus: int;

procedure main(args: const string[1024]);
begin
  status := syslib.window_open(10, 10, 100, 50, "one", id);
  if status = Invalid then
    core.write("open invalid\n");
  status := syslib.window_frame_rect(id, rect);
  if status = Invalid then
    core.write("rect invalid\n");
  focus := syslib.window_focus();
  if focus = 0 then
    core.write("focus zero\n");
  status := syslib.window_close(id);
  if status = NotFound then
    core.write("close notfound\n");
end;
"#;
    let package = langhost::compile(source, "t.ig").expect("compiles");
    let result = langhost::run(&package.code, b"", langhost::DEFAULT_BUDGET).expect("verifies");
    assert_eq!(result.reason, ExitReason::Ok);
    assert_eq!(
        result.output,
        b"open invalid\nrect invalid\nfocus zero\nclose notfound\n".to_vec()
    );
}

#[test]
fn windows_demo_compiles_and_reports_no_graphics_on_the_host() {
    let path = app_path("windows_demo");
    let source = std::fs::read(&path).unwrap_or_else(|error| panic!("cannot read {path}: {error}"));
    let package = langhost::compile(&source, "windows_demo").expect("compiles");
    let result = langhost::run(&package.code, b"", langhost::DEFAULT_BUDGET).expect("verifies");
    assert_eq!(result.reason, ExitReason::Ok);
    assert_eq!(result.output, b"windows_demo: no graphics\n".to_vec());
}

#[test]
fn windows_events_compiles_and_reports_no_graphics_on_the_host() {
    let path = app_path("windows_events");
    let source = std::fs::read(&path).unwrap_or_else(|error| panic!("cannot read {path}: {error}"));
    let package = langhost::compile(&source, "windows_events").expect("compiles");
    let result = langhost::run(&package.code, b"", langhost::DEFAULT_BUDGET).expect("verifies");
    assert_eq!(result.reason, ExitReason::Ok);
    assert_eq!(result.output, b"windows_events: no graphics\n".to_vec());
}

#[test]
fn windows_proof_compiles_and_reports_no_graphics_on_the_host() {
    let path = app_path("windows_proof");
    let source = std::fs::read(&path).unwrap_or_else(|error| panic!("cannot read {path}: {error}"));
    let package = langhost::compile(&source, "windows_proof").expect("compiles");
    let result = langhost::run(&package.code, b"", langhost::DEFAULT_BUDGET).expect("verifies");
    assert_eq!(result.reason, ExitReason::Ok);
    assert_eq!(result.output, b"windows_proof: no graphics\n".to_vec());
}

#[test]
fn windows_proof_a_compiles_and_reports_no_graphics_on_the_host() {
    let path = app_path("windows_proof_a");
    let source = std::fs::read(&path).unwrap_or_else(|error| panic!("cannot read {path}: {error}"));
    let package = langhost::compile(&source, "windows_proof_a").expect("compiles");
    let result = langhost::run(&package.code, b"", langhost::DEFAULT_BUDGET).expect("verifies");
    assert_eq!(result.reason, ExitReason::Ok);
    assert_eq!(result.output, b"windows_proof_a: no graphics\n".to_vec());
}

#[test]
fn windows_proof_b_compiles_and_reports_no_graphics_on_the_host() {
    let path = app_path("windows_proof_b");
    let source = std::fs::read(&path).unwrap_or_else(|error| panic!("cannot read {path}: {error}"));
    let package = langhost::compile(&source, "windows_proof_b").expect("compiles");
    let result = langhost::run(&package.code, b"", langhost::DEFAULT_BUDGET).expect("verifies");
    assert_eq!(result.reason, ExitReason::Ok);
    assert_eq!(result.output, b"windows_proof_b: no graphics\n".to_vec());
}

#[test]
fn ui_metrics_and_the_disabled_screen_path_are_deterministic() {
    let source = r#"
unit uimetrics;

uses core, stdlib, syslib;

var
  text: string[32];
  n: int;
  st: Status;

procedure show(label: const string[0]; value: int);
begin
  n := core.format_int_into(text, value, uint(10), uint(0));
  core.write(label);
  core.write("=");
  core.write(text);
  core.write("\n");
end;

procedure main(args: const string[1024]);
begin
  show("w0", stdlib.ui_text_width(""));
  show("w3", stdlib.ui_text_width("abc"));
  show("wutf8", stdlib.ui_text_width("aé"));
  show("h", stdlib.ui_text_height());
  show("line", stdlib.ui_line_height());
  show("inset", stdlib.ui_content_inset());
  show("bar", stdlib.ui_title_bar_height());
  show("box", stdlib.ui_close_box_size());
  show("cx", stdlib.ui_text_center_x(0, 100, "abc"));
  show("cy", stdlib.ui_text_center_y(0, 16));
  show("narrowx", stdlib.ui_text_center_x(7, 8, "abc"));
  show("shorty", stdlib.ui_text_center_y(9, 8));
  show("black", int(stdlib.ui_black()));
  show("white", int(stdlib.ui_white()));
  show("gray", int(stdlib.ui_gray()));
  show("focus", int(stdlib.ui_focus()));
  show("accent", int(stdlib.ui_accent()));
  st := stdlib.ui_frame(0, 0, 32, 32);
  if st = Invalid then core.write("frame=invalid\n");
  st := stdlib.ui_title_bar(0, 0, 100, "host", true);
  if st = Invalid then core.write("title=invalid\n");
  st := stdlib.ui_close_box(0, 0);
  if st = Invalid then core.write("close=invalid\n");
  st := stdlib.ui_separator(0, 0, 32);
  if st = Invalid then core.write("sep=invalid\n");
  st := stdlib.ui_center_text(0, 0, 100, 16, "host", stdlib.ui_black());
  if st = Invalid then core.write("text=invalid\n");
end;
"#;
    let package = langhost::compile(source.as_bytes(), "uimetrics").expect("compiles");
    let result = langhost::run(&package.code, b"", langhost::DEFAULT_BUDGET).expect("verifies");
    assert_eq!(result.reason, ExitReason::Ok);
    assert_eq!(
        String::from_utf8(result.output).expect("ASCII output"),
        "w0=0\n\
         w3=24\n\
         wutf8=16\n\
         h=8\n\
         line=16\n\
         inset=8\n\
         bar=16\n\
         box=11\n\
         cx=38\n\
         cy=4\n\
         narrowx=7\n\
         shorty=9\n\
         black=0\n\
         white=16777215\n\
         gray=13421772\n\
         focus=16777215\n\
         accent=128\n\
         frame=invalid\n\
         title=invalid\n\
         close=invalid\n\
         sep=invalid\n\
         text=invalid\n"
    );
}

#[test]
fn ui_layout_compiles_and_reports_the_host_path() {
    let path = app_path("ui_layout");
    let source = std::fs::read(&path).unwrap_or_else(|error| panic!("cannot read {path}: {error}"));
    let package = langhost::compile(&source, "ui_layout").expect("compiles");
    let result = langhost::run(&package.code, b"", langhost::DEFAULT_BUDGET).expect("verifies");
    assert_eq!(result.reason, ExitReason::Ok);
    assert_eq!(result.output, b"ui_layout: host ok\n".to_vec());
}

#[test]
fn ui_controls_compiles_and_reports_the_host_path() {
    let path = app_path("ui_controls");
    let source = std::fs::read(&path).unwrap_or_else(|error| panic!("cannot read {path}: {error}"));
    let package = langhost::compile(&source, "ui_controls").expect("compiles");
    let result = langhost::run(&package.code, b"", langhost::DEFAULT_BUDGET).expect("verifies");
    assert_eq!(result.reason, ExitReason::Ok);
    assert_eq!(result.output, b"ui_controls: host ok\n".to_vec());
}

#[test]
fn text_edit_compiles_and_edits_utf8_text_on_the_host() {
    let path = app_path("text_edit");
    let source = std::fs::read(&path).unwrap_or_else(|error| panic!("cannot read {path}: {error}"));
    let package = langhost::compile(&source, "text_edit").expect("compiles");
    let result = langhost::run(&package.code, b"", langhost::DEFAULT_BUDGET).expect("verifies");
    assert_eq!(result.reason, ExitReason::Ok);
    assert_eq!(
        String::from_utf8(result.output).expect("UTF-8 output"),
        "cursor left widths: ok\n\
         cursor left at zero: ok\n\
         cursor right widths: ok\n\
         cursor right at end: ok\n\
         clamp inside 3-byte: ok\n\
         clamp inside 2-byte: ok\n\
         clamp inside 4-byte: ok\n\
         clamp negative: ok\n\
         clamp past end: ok\n\
         left from inside: ok\n\
         right from inside: ok\n\
         cursor home: ok\n\
         cursor end: ok\n\
         insert at start: ok\n\
         insert at end: ok\n\
         insert multibyte: ok\n\
         insert before multibyte: ok\n\
         insert non-boundary: ok\n\
         insert negative: ok\n\
         insert past end: ok\n\
         insert empty: ok\n\
         fill full: ok\n\
         insert capacity: ok\n\
         insert capacity at end: ok\n\
         delete 2-byte: ok\n\
         delete 3-byte: ok\n\
         delete 4-byte: ok\n\
         delete at start: ok\n\
         delete zero: ok\n\
         delete non-boundary start: ok\n\
         delete non-boundary end: ok\n\
         delete negative: ok\n\
         delete past end: ok\n\
         delete past end position: ok\n\
         delete whole first point: ok\n\
         delete all: ok\n\
         backspace 2-byte: ok\n\
         backspace to start: ok\n\
         backspace at zero: ok\n\
         backspace 4-byte: ok\n\
         backspace clamped no-op: ok\n\
         backspace past end: ok\n\
         selection normalize: ok\n\
         selection all: ok\n\
         collapse to start: ok\n\
         collapse to stop: ok\n\
         replace 2-byte: ok\n\
         replace collapsed: ok\n\
         replace normalized pair: ok\n\
         replace with empty: ok\n\
         replace multibyte: ok\n\
         fill full: ok\n\
         fill big: ok\n\
         replace capacity: ok\n\
         replace shrinks: ok\n\
         text_edit: ok\n"
    );
}

/// A host with a screen that records the drawing calls instead of rasterising.
/// It proves the helper's drawing *sequence* (geometry and order) without the
/// kernel window service.
struct Painter {
    fills: Vec<(i64, i64, i64, i64, u32)>,
    outlines: Vec<(i64, i64, i64, i64, u32)>,
    texts: Vec<(i64, i64, Vec<u8>, u32)>,
    output: Vec<u8>,
}

impl Host for Painter {
    fn console_write(&mut self, bytes: &[u8]) {
        self.output.extend_from_slice(bytes);
    }

    fn clock(&mut self) -> u64 {
        0
    }

    fn screen_width(&mut self) -> u64 {
        1024
    }

    fn screen_height(&mut self) -> u64 {
        768
    }

    fn fill_rect(&mut self, x: i64, y: i64, w: i64, h: i64, color: u32) -> Status {
        self.fills.push((x, y, w, h, color));
        Status::Ok
    }

    fn draw_rect(&mut self, x: i64, y: i64, w: i64, h: i64, color: u32) -> Status {
        self.outlines.push((x, y, w, h, color));
        Status::Ok
    }

    fn draw_text(&mut self, x: i64, y: i64, text: &[u8], color: u32) -> Status {
        self.texts.push((x, y, text.to_vec(), color));
        Status::Ok
    }
}

#[test]
fn ui_title_bar_draws_the_documented_sequence() {
    let source = br#"
unit uiseq;

uses core, stdlib, syslib;

var
  st: Status;

procedure main(args: const string[1024]);
begin
  st := stdlib.ui_frame(10, 20, 100, 50);
  st := stdlib.ui_title_bar(10, 20, 100, "Hi", true);
  st := stdlib.ui_close_box(14, 23);
  st := stdlib.ui_separator(11, 35, 98);
  st := stdlib.ui_center_text(10, 20, 100, 16, "Hi", stdlib.ui_black());
  if st = Ok then
    core.write("ui sequence ok\n");
end;
"#;
    let package = langhost::compile(source, "uiseq").expect("compiles");
    let mut scratch = vec![0u8; package.code.len().div_ceil(32)];
    let program = interpreter::Program::parse(&package.code, &mut scratch).expect("verifies");
    let header = *program.header();
    let mut state = vec![0u8; header.state_size as usize];
    let mut stack = vec![0u8; header.stack_size as usize];
    let args = vec![0u8; 8 + 1024];
    let mut machine = interpreter::Machine::new();
    let mut host = Painter {
        fills: Vec::new(),
        outlines: Vec::new(),
        texts: Vec::new(),
        output: Vec::new(),
    };
    let outcome = machine.run(
        &program,
        &mut host,
        &mut state,
        &mut stack,
        &args,
        langhost::DEFAULT_BUDGET,
    );
    assert_eq!(outcome, interpreter::Outcome::Exited(ExitReason::Ok));
    assert_eq!(host.output, b"ui sequence ok\n");

    const BLACK: u32 = 0x000000;
    const WHITE: u32 = 0x00FF_FFFF;

    assert_eq!(
        host.outlines,
        vec![
            (10, 20, 100, 50, BLACK),
            (14, 23, 11, 11, BLACK),
            (14, 23, 11, 11, BLACK),
        ],
        "ui_frame, the title bar's close box, then the explicit close box"
    );
    assert!(
        host.fills.contains(&(11, 21, 98, 15, WHITE)),
        "focused title background: {:?}",
        host.fills
    );
    for row in [22, 24, 26, 28, 30, 32, 34] {
        assert!(
            host.fills.contains(&(11, row, 98, 1, BLACK)),
            "stripe at row {row}: {:?}",
            host.fills
        );
    }
    assert!(
        host.fills.contains(&(14, 23, 11, 11, WHITE)),
        "close box background: {:?}",
        host.fills
    );
    assert!(
        host.fills.contains(&(11, 35, 98, 1, BLACK)),
        "title separator: {:?}",
        host.fills
    );
    assert!(
        host.fills.contains(&(60, 23, 20, 10, WHITE)),
        "title band behind the glyphs: {:?}",
        host.fills
    );
    assert_eq!(
        host.texts,
        vec![
            (62, 24, b"Hi".to_vec(), BLACK),
            (52, 24, b"Hi".to_vec(), BLACK),
        ],
        "the centred title, then the explicit centred text"
    );
}

#[test]
fn ui_controls_draw_the_documented_sequence() {
    let source = br#"
unit uictl;

uses core, stdlib, syslib;

var
  st: Status;
  items: array[string[64], 8];

procedure main(args: const string[1024]);
begin
  items[0] := "Alpha";
  items[1] := "Bravo";
  items[2] := "Charlie";
  items[3] := "Delta";
  items[4] := "Echo";
  items[5] := "Foxtrot";
  st := stdlib.ui_button(10, 20, 100, 24, "OK", true, true);
  st := stdlib.ui_checkbox(10, 60, "On", true, true);
  st := stdlib.ui_text_field(10, 100, 100, 22, "abc", 2, 0, 0, 1, true);
  st := stdlib.ui_scrollbar(200, 20, 16, 160, 10, 10, 100, true);
  st := stdlib.ui_list(10, 200, 100, 82, items, 6, 2, 0, true);
  if st = Ok then
    core.write("ui controls sequence ok\n");
  if stdlib.ui_key_code(uint(72 * 65536 + 104)) = 72
    and stdlib.ui_key_text(uint(72 * 65536 + 104)) = byte(104)
    and stdlib.ui_key_is_backspace(uint(0x101 * 65536))
    and stdlib.ui_key_is_tab(uint(0x102 * 65536))
    and stdlib.ui_key_is_enter(uint(0x103 * 65536))
    and stdlib.ui_key_is_left(uint(0x110 * 65536))
    and stdlib.ui_key_is_right(uint(0x111 * 65536))
    and stdlib.ui_key_is_up(uint(0x112 * 65536))
    and stdlib.ui_key_is_down(uint(0x113 * 65536))
    and stdlib.ui_key_is_home(uint(0x114 * 65536))
    and stdlib.ui_key_is_end(uint(0x115 * 65536))
    and stdlib.ui_key_is_delete(uint(0x119 * 65536))
    and stdlib.ui_key_modifiers(uint(65 + 1 * 256)) = 1 then
    core.write("ui keys ok\n");
end;
"#;
    let package = langhost::compile(source, "uictl").expect("compiles");
    let mut scratch = vec![0u8; package.code.len().div_ceil(32)];
    let program = interpreter::Program::parse(&package.code, &mut scratch).expect("verifies");
    let header = *program.header();
    let mut state = vec![0u8; header.state_size as usize];
    let mut stack = vec![0u8; header.stack_size as usize];
    let args = vec![0u8; 8 + 1024];
    let mut machine = interpreter::Machine::new();
    let mut host = Painter {
        fills: Vec::new(),
        outlines: Vec::new(),
        texts: Vec::new(),
        output: Vec::new(),
    };
    let outcome = machine.run(
        &program,
        &mut host,
        &mut state,
        &mut stack,
        &args,
        langhost::DEFAULT_BUDGET,
    );
    assert_eq!(outcome, interpreter::Outcome::Exited(ExitReason::Ok));
    assert_eq!(host.output, b"ui controls sequence ok\nui keys ok\n");

    const BLACK: u32 = 0x000000;
    const WHITE: u32 = 0x00FF_FFFF;
    const GRAY: u32 = 0x00CC_CCCC;
    const ACCENT: u32 = 0x0000_80;

    assert_eq!(
        host.outlines,
        vec![
            (10, 20, 100, 24, BLACK),
            (10, 60, 11, 11, BLACK),
            (10, 100, 100, 22, BLACK),
            (200, 20, 16, 160, BLACK),
            (202, 37, 12, 15, BLACK),
            (10, 200, 100, 82, BLACK),
        ],
        "button, checkbox, field, scrollbar frame and thumb, list"
    );

    // Button: pressed gray face, focused inset ring, shifted centred label.
    assert!(
        host.fills.contains(&(11, 21, 98, 22, GRAY)),
        "{:?}",
        host.fills
    );
    assert!(
        host.fills.contains(&(13, 23, 1, 1, BLACK)),
        "ring top: {:?}",
        host.fills
    );
    assert!(!host.fills.contains(&(14, 23, 1, 1, BLACK)), "dotted ring");

    // Checkbox: ring first, then the box, check mark and label.
    assert!(
        host.fills.contains(&(42, 60, 1, 1, BLACK)),
        "checkbox ring: {:?}",
        host.fills
    );
    assert!(
        host.fills.contains(&(10, 60, 11, 11, WHITE)),
        "checkbox face: {:?}",
        host.fills
    );
    assert!(
        host.fills.contains(&(12, 65, 1, 1, BLACK)),
        "check mark: {:?}",
        host.fills
    );

    // Text field: white face, the text, the accent selection with white text,
    // the caret and the inset ring.
    assert!(
        host.fills.contains(&(11, 101, 98, 20, WHITE)),
        "field face: {:?}",
        host.fills
    );
    assert!(
        host.fills.contains(&(13, 107, 8, 8, ACCENT)),
        "selection: {:?}",
        host.fills
    );
    assert!(
        host.fills.contains(&(29, 107, 1, 8, BLACK)),
        "caret: {:?}",
        host.fills
    );
    assert!(
        host.fills.contains(&(12, 102, 1, 1, BLACK)),
        "field ring: {:?}",
        host.fills
    );

    // Scrollbar: white track, gray thumb at position 10 and its inset ring.
    assert!(
        host.fills.contains(&(201, 21, 14, 158, WHITE)),
        "track: {:?}",
        host.fills
    );
    assert!(
        host.fills.contains(&(202, 37, 12, 15, GRAY)),
        "thumb: {:?}",
        host.fills
    );
    assert!(
        host.fills.contains(&(201, 21, 1, 1, BLACK)),
        "scroll ring: {:?}",
        host.fills
    );

    // List: the selected row is accent-filled and its text is white.
    assert!(
        host.fills.contains(&(11, 233, 98, 16, ACCENT)),
        "selection row: {:?}",
        host.fills
    );
    assert!(
        host.fills.contains(&(12, 202, 1, 1, BLACK)),
        "list ring: {:?}",
        host.fills
    );

    assert_eq!(
        host.texts,
        vec![
            (53, 29, b"OK".to_vec(), BLACK),
            (27, 61, b"On".to_vec(), BLACK),
            (13, 107, b"abc".to_vec(), BLACK),
            (13, 107, b"a".to_vec(), WHITE),
            (14, 205, b"Alpha".to_vec(), BLACK),
            (14, 221, b"Bravo".to_vec(), BLACK),
            (14, 237, b"Charlie".to_vec(), WHITE),
            (14, 253, b"Delta".to_vec(), BLACK),
            (14, 269, b"Echo".to_vec(), BLACK),
        ]
    );
}

#[test]
fn ui_menus_compiles_and_reports_the_host_path() {
    let path = app_path("ui_menus");
    let source = std::fs::read(&path).unwrap_or_else(|error| panic!("cannot read {path}: {error}"));
    let package = langhost::compile(&source, "ui_menus").expect("compiles");
    let result = langhost::run(&package.code, b"", langhost::DEFAULT_BUDGET).expect("verifies");
    assert_eq!(result.reason, ExitReason::Ok);
    assert_eq!(result.output, b"ui_menus: host ok\n".to_vec());
}

#[test]
fn ui_menus_draw_the_documented_sequence() {
    let source = br#"
unit uimenu;

uses core, stdlib, syslib;

var
  st: Status;
  titles: array[string[32], 8];
  items: array[string[64], 16];
  keys: array[string[32], 16];
  checked: uint;
  disabled: uint;

procedure main(args: const string[1024]);
begin
  titles[0] := "File";
  titles[1] := "Edit";
  titles[2] := "View";
  items[0] := "New";
  items[1] := "Open";
  items[2] := "-";
  items[3] := "Quit";
  keys[0] := "Ctrl+N";
  keys[1] := "Ctrl+O";
  keys[2] := "";
  keys[3] := "Ctrl+Q";
  checked := stdlib.ui_menu_bit(0);
  disabled := stdlib.ui_menu_bit(3);
  st := stdlib.ui_menu_bar(0, 0, 200, titles, 3, 0);
  st := stdlib.ui_menu_dropdown(
    4, 22, 200, items, keys, 4, 1, checked, disabled, true);
  if st = Ok
    and stdlib.ui_menu_bit(3) = uint(8)
    and stdlib.ui_menu_has(checked, 0)
    and not stdlib.ui_menu_has(disabled, 0)
    and stdlib.ui_menu_selectable(items[1], disabled, 1)
    and not stdlib.ui_menu_selectable(items[3], disabled, 3)
    and stdlib.ui_menu_first(4, items, disabled) = 0
    and stdlib.ui_menu_move(1, 4, 1, items, disabled) = 1
    and stdlib.ui_menu_hit(4, 22, 200, items, 4, 10, 28) = 0
    and stdlib.ui_menu_hit(4, 22, 200, items, 4, 10, 60) = -1
    and stdlib.ui_menu_bar_hit(0, 0, 200, titles, 3, 10, 10) = 0
    and stdlib.ui_menu_key(uint(0x100 * 65536), false) = 6
    and stdlib.ui_menu_key(uint(0x103 * 65536), true) = 5 then
    core.write("ui menus sequence ok\n");
end;
"#;
    let package = langhost::compile(source, "uimenu").expect("compiles");
    let mut scratch = vec![0u8; package.code.len().div_ceil(32)];
    let program = interpreter::Program::parse(&package.code, &mut scratch).expect("verifies");
    let header = *program.header();
    let mut state = vec![0u8; header.state_size as usize];
    let mut stack = vec![0u8; header.stack_size as usize];
    let args = vec![0u8; 8 + 1024];
    let mut machine = interpreter::Machine::new();
    let mut host = Painter {
        fills: Vec::new(),
        outlines: Vec::new(),
        texts: Vec::new(),
        output: Vec::new(),
    };
    let outcome = machine.run(
        &program,
        &mut host,
        &mut state,
        &mut stack,
        &args,
        langhost::DEFAULT_BUDGET,
    );
    assert_eq!(outcome, interpreter::Outcome::Exited(ExitReason::Ok));
    assert_eq!(host.output, b"ui menus sequence ok\n");

    const BLACK: u32 = 0x000000;
    const WHITE: u32 = 0x00FF_FFFF;
    const GRAY: u32 = 0x00CC_CCCC;
    const ACCENT: u32 = 0x0000_80;

    // The bar: white background, inverted File cell, the three labels and the
    // bottom rule. Only the active cell paints a fill; the bar has no border.
    assert!(
        host.fills.contains(&(0, 0, 200, 22, WHITE)),
        "bar: {:?}",
        host.fills
    );
    assert!(
        host.fills.contains(&(4, 2, 40, 18, BLACK)),
        "active cell: {:?}",
        host.fills
    );
    assert!(
        host.fills.contains(&(0, 21, 200, 1, BLACK)),
        "bar rule: {:?}",
        host.fills
    );

    // The pull-down: white face, black frame, accent selection on row 1 and the
    // separator row.
    assert!(
        host.fills.contains(&(5, 23, 198, 64, WHITE)),
        "dropdown face: {:?}",
        host.fills
    );
    assert_eq!(
        host.outlines,
        vec![(4, 22, 200, 66, BLACK)],
        "the pull-down frame"
    );
    assert!(
        host.fills.contains(&(5, 39, 198, 16, ACCENT)),
        "selected row: {:?}",
        host.fills
    );
    assert!(
        host.fills.contains(&(12, 63, 184, 1, BLACK)),
        "separator: {:?}",
        host.fills
    );

    // The check mark on the checked item 0 (not selected, so black).
    for (x, y) in [
        (14, 28),
        (15, 29),
        (16, 30),
        (17, 29),
        (18, 28),
        (19, 27),
        (20, 26),
    ] {
        assert!(
            host.fills.contains(&(x, y, 1, 1, BLACK)),
            "check mark at ({x}, {y}): {:?}",
            host.fills
        );
    }

    // Labels and right-aligned shortcuts: black, white on the selection, gray
    // on the disabled item.
    assert!(host.texts.contains(&(26, 27, b"New".to_vec(), BLACK)));
    assert!(host.texts.contains(&(148, 27, b"Ctrl+N".to_vec(), BLACK)));
    assert!(host.texts.contains(&(26, 43, b"Open".to_vec(), WHITE)));
    assert!(host.texts.contains(&(148, 43, b"Ctrl+O".to_vec(), WHITE)));
    assert!(host.texts.contains(&(26, 75, b"Quit".to_vec(), GRAY)));
    assert!(host.texts.contains(&(148, 75, b"Ctrl+Q".to_vec(), GRAY)));
    assert!(host.texts.contains(&(8, 7, b"File".to_vec(), WHITE)));
    assert!(host.texts.contains(&(56, 7, b"Edit".to_vec(), BLACK)));
    assert!(host.texts.contains(&(104, 7, b"View".to_vec(), BLACK)));
}

#[test]
fn ui_dialogs_compiles_and_reports_the_host_path() {
    let path = app_path("ui_dialogs");
    let source = std::fs::read(&path).unwrap_or_else(|error| panic!("cannot read {path}: {error}"));
    let package = langhost::compile(&source, "ui_dialogs").expect("compiles");
    let result = langhost::run(&package.code, b"", langhost::DEFAULT_BUDGET).expect("verifies");
    assert_eq!(result.reason, ExitReason::Ok);
    assert_eq!(result.output, b"ui_dialogs: host ok\n".to_vec());
}

#[test]
fn ui_dialogs_draw_the_documented_sequence() {
    let source = br#"
unit uidialog;

uses core, stdlib, syslib;

var
  st: Status;
  text: string[256];

procedure main(args: const string[1024]);
begin
  text := "Hi";
  st := stdlib.ui_alert(10, 20, 300, 120, "Title", "The message.", "OK", "Cancel", 0);
  st := stdlib.ui_input_dialog(
    10, 160, 300, 200, "Rename", "Name:", "OK", "Cancel", text, 2, 0, 0, 0, 0);
  st := stdlib.ui_message(0, 300, 88, 32, "alpha beta gamma");
  if st = Ok
    and stdlib.ui_dialog_key(uint(0x103 * 65536), true) = 1
    and stdlib.ui_dialog_key(uint(0x100 * 65536), true) = 2
    and stdlib.ui_dialog_key(uint(0x100 * 65536), false) = 0
    and stdlib.ui_dialog_key(uint(0x102 * 65536), true) = 3
    and stdlib.ui_dialog_key(uint(0x102 * 65536 + 256), true) = 4
    and stdlib.ui_alert_hit(10, 20, 300, 120, "OK", "Cancel", 150, 112) = 0
    and stdlib.ui_alert_hit(10, 20, 300, 120, "OK", "Cancel", 234, 112) = 1
    and stdlib.ui_input_dialog_hit(10, 160, 300, 200, "OK", "Cancel", 20, 210) = 0
    and stdlib.ui_input_dialog_hit(10, 160, 300, 200, "OK", "Cancel", 150, 332) = 1
    and stdlib.ui_input_dialog_hit(10, 160, 300, 200, "OK", "Cancel", 234, 332) = 2 then
    core.write("ui dialogs sequence ok\n");
end;
"#;
    let package = langhost::compile(source, "uidialog").expect("compiles");
    let mut scratch = vec![0u8; package.code.len().div_ceil(32)];
    let program = interpreter::Program::parse(&package.code, &mut scratch).expect("verifies");
    let header = *program.header();
    let mut state = vec![0u8; header.state_size as usize];
    let mut stack = vec![0u8; header.stack_size as usize];
    let args = vec![0u8; 8 + 1024];
    let mut machine = interpreter::Machine::new();
    let mut host = Painter {
        fills: Vec::new(),
        outlines: Vec::new(),
        texts: Vec::new(),
        output: Vec::new(),
    };
    let outcome = machine.run(
        &program,
        &mut host,
        &mut state,
        &mut stack,
        &args,
        langhost::DEFAULT_BUDGET,
    );
    assert_eq!(outcome, interpreter::Outcome::Exited(ExitReason::Ok));
    assert_eq!(host.output, b"ui dialogs sequence ok\n");

    const BLACK: u32 = 0x000000;
    const WHITE: u32 = 0x00FF_FFFF;

    // The exact frame/outline order: the alert frame, its close box and both
    // buttons, then the input dialog, its close box and field, and both buttons.
    assert_eq!(
        host.outlines,
        vec![
            (10, 20, 300, 120, BLACK),
            (14, 23, 11, 11, BLACK),
            (146, 108, 72, 24, BLACK),
            (148, 110, 68, 20, BLACK),
            (230, 108, 72, 24, BLACK),
            (10, 160, 300, 200, BLACK),
            (14, 163, 11, 11, BLACK),
            (18, 204, 284, 22, BLACK),
            (146, 328, 72, 24, BLACK),
            (148, 330, 68, 20, BLACK),
            (230, 328, 72, 24, BLACK),
        ],
        "the dialog frames, close boxes, fields and default rings"
    );

    // The alert background, the focused title bar with a stripe row and the
    // separator, its band, and both button faces.
    assert!(
        host.fills.contains(&(10, 20, 300, 120, WHITE)),
        "alert background: {:?}",
        host.fills
    );
    assert!(
        host.fills.contains(&(11, 21, 298, 15, WHITE)),
        "alert title bar: {:?}",
        host.fills
    );
    assert!(
        host.fills.contains(&(11, 22, 298, 1, BLACK)),
        "alert title stripe: {:?}",
        host.fills
    );
    assert!(
        host.fills.contains(&(11, 35, 298, 1, BLACK)),
        "alert title separator: {:?}",
        host.fills
    );
    assert!(
        host.fills.contains(&(148, 23, 44, 10, WHITE)),
        "alert title band: {:?}",
        host.fills
    );
    assert!(
        host.fills.contains(&(147, 109, 70, 22, WHITE)),
        "alert default face: {:?}",
        host.fills
    );
    assert!(
        host.fills.contains(&(231, 109, 70, 22, WHITE)),
        "alert cancel face: {:?}",
        host.fills
    );
    assert!(
        host.fills.contains(&(149, 111, 1, 1, BLACK)),
        "alert default focus ring: {:?}",
        host.fills
    );

    // The input dialog: white background, title bar, prompt row, field face,
    // caret and both button faces; the field ring marks its focus.
    assert!(
        host.fills.contains(&(10, 160, 300, 200, WHITE)),
        "input background: {:?}",
        host.fills
    );
    assert!(
        host.fills.contains(&(11, 161, 298, 15, WHITE)),
        "input title bar: {:?}",
        host.fills
    );
    assert!(
        host.fills.contains(&(19, 205, 282, 20, WHITE)),
        "field face: {:?}",
        host.fills
    );
    assert!(
        host.fills.contains(&(37, 211, 1, 8, BLACK)),
        "field caret: {:?}",
        host.fills
    );
    assert!(
        host.fills.contains(&(20, 206, 1, 1, BLACK)),
        "field focus ring: {:?}",
        host.fills
    );
    assert!(
        host.fills.contains(&(147, 329, 70, 22, WHITE)),
        "ok face: {:?}",
        host.fills
    );
    assert!(
        host.fills.contains(&(231, 329, 70, 22, WHITE)),
        "cancel face: {:?}",
        host.fills
    );
    assert!(
        !host.fills.contains(&(149, 331, 1, 1, BLACK)),
        "the unfocused OK button has no focus ring: {:?}",
        host.fills
    );

    // Text in paint order: the title and message, the alert buttons, the title,
    // prompt, field text, the input buttons, then the wrapped message lines.
    assert_eq!(
        host.texts,
        vec![
            (150, 24, b"Title".to_vec(), BLACK),
            (18, 44, b"The message.".to_vec(), BLACK),
            (174, 116, b"OK".to_vec(), BLACK),
            (242, 116, b"Cancel".to_vec(), BLACK),
            (146, 164, b"Rename".to_vec(), BLACK),
            (18, 184, b"Name:".to_vec(), BLACK),
            (21, 211, b"Hi".to_vec(), BLACK),
            (174, 336, b"OK".to_vec(), BLACK),
            (242, 336, b"Cancel".to_vec(), BLACK),
            (0, 300, b"alpha beta".to_vec(), BLACK),
            (0, 316, b"gamma".to_vec(), BLACK),
        ]
    );
}

#[test]
fn ui_icons_compiles_and_reports_the_host_path() {
    let path = app_path("ui_icons");
    let source = std::fs::read(&path).unwrap_or_else(|error| panic!("cannot read {path}: {error}"));
    let package = langhost::compile(&source, "ui_icons").expect("compiles");
    let result = langhost::run(&package.code, b"", langhost::DEFAULT_BUDGET).expect("verifies");
    assert_eq!(result.reason, ExitReason::Ok);
    assert_eq!(result.output, b"ui_icons: host ok\n".to_vec());
}

#[test]
fn ui_icons_draw_the_documented_sequence() {
    let source = br#"
unit iconseq;

uses core, stdlib, syslib;

var
  rows: array[uint, 32];
  x: int;
  y: int;
  w: int;
  h: int;
  dx: int;
  dy: int;
  st: Status;

procedure main(args: const string[1024]);
begin
  rows[2] := uint(7);
  rows[3] := uint(5);
  st := stdlib.ui_icon(10, 20, rows, stdlib.ui_black());
  st := stdlib.ui_icon_label(10, 54, "Doc");
  st := stdlib.ui_selection_rect(40, 20, 20, 10);
  if st = Ok
    and stdlib.ui_icon_size() = 32
    and stdlib.ui_icon_row_bit(rows[2], 2)
    and not stdlib.ui_icon_row_bit(rows[2], 3)
    and stdlib.ui_icon_cell_x(1, 10, 100) = 110
    and stdlib.ui_icon_cell_y(1, 10, 80) = 90
    and stdlib.ui_icon_label_y(20) = 54
    and stdlib.ui_icon_grid_hit(0, 0, 2, 4, 100, 80, 110, 10) = 1
    and stdlib.ui_icon_grid_hit(0, 0, 2, 4, 100, 80, 110, 90) = 3
    and stdlib.ui_icon_grid_hit(0, 0, 2, 3, 100, 80, 110, 90) = -1
    and stdlib.ui_double_click(uint(1000), uint(400001000), true)
    and not stdlib.ui_double_click(uint(1000), uint(400001001), true)
    and not stdlib.ui_double_click(uint(1000), uint(2000), false)
    and stdlib.ui_icon_grab(10, 20, 15, 25, dx, dy)
    and dx = 5 and dy = 5
    and stdlib.ui_icon_follow(100, 200, 5, 7, x, y) = Ok
    and x = 95 and y = 193
    and stdlib.ui_selection_normalize(100, 50, 20, 80, x, y, w, h) = Ok
    and x = 20 and y = 50 and w = 81 and h = 31
    and stdlib.ui_rect_intersects(0, 0, 10, 10, 5, 5, 10, 10)
    and not stdlib.ui_rect_intersects(0, 0, 10, 10, 10, 0, 10, 10)
    and stdlib.ui_icon_drop(0, 0, 2, 4, 100, 80, 300, 300, 50, 50, 320, 320) = -2
    and stdlib.ui_icon_drop(0, 0, 2, 4, 100, 80, 300, 300, 50, 50, 110, 10) = 1
    and stdlib.ui_icon_drop(0, 0, 2, 4, 100, 80, 300, 300, 50, 50, 250, 10) = -1
    and stdlib.ui_trash_hit(300, 300, 50, 50, 320, 320)
    and not stdlib.ui_trash_hit(300, 300, 50, 50, 299, 320) then
    core.write("ui icons sequence ok\n");
end;
"#;
    let package = langhost::compile(source, "iconseq").expect("compiles");
    let mut scratch = vec![0u8; package.code.len().div_ceil(32)];
    let program = interpreter::Program::parse(&package.code, &mut scratch).expect("verifies");
    let header = *program.header();
    let mut state = vec![0u8; header.state_size as usize];
    let mut stack = vec![0u8; header.stack_size as usize];
    let args = vec![0u8; 8 + 1024];
    let mut machine = interpreter::Machine::new();
    let mut host = Painter {
        fills: Vec::new(),
        outlines: Vec::new(),
        texts: Vec::new(),
        output: Vec::new(),
    };
    let outcome = machine.run(
        &program,
        &mut host,
        &mut state,
        &mut stack,
        &args,
        langhost::DEFAULT_BUDGET,
    );
    assert_eq!(outcome, interpreter::Outcome::Exited(ExitReason::Ok));
    assert_eq!(host.output, b"ui icons sequence ok\n");

    const BLACK: u32 = 0x000000;

    // `ui_icon` draws one run per maximal set-bit run: row 2 is 0b111 and row
    // 3 is 0b101, so three fills, and the all-zero rows draw nothing.
    assert_eq!(
        host.fills
            .iter()
            .filter(|fill| *fill == &(10, 22, 3, 1, BLACK))
            .count(),
        1,
        "row 2 run: {:?}",
        host.fills
    );
    assert!(
        host.fills.contains(&(10, 23, 1, 1, BLACK)),
        "row 3 first bit: {:?}",
        host.fills
    );
    assert!(
        host.fills.contains(&(12, 23, 1, 1, BLACK)),
        "row 3 third bit: {:?}",
        host.fills
    );
    assert_eq!(
        host.fills
            .iter()
            .filter(|fill| (fill.1 == 22 || fill.1 == 23) && fill.0 < 40)
            .count(),
        3,
        "no other icon rows: {:?}",
        host.fills
    );
    assert!(host.texts.contains(&(10, 54, b"Doc".to_vec(), BLACK)));

    // The rubber band is the dotted 1-px ring, one pixel every two positions.
    for at in [(40, 20), (42, 20), (40, 29), (59, 20), (40, 22), (59, 28)] {
        assert!(
            host.fills.contains(&(at.0, at.1, 1, 1, BLACK)),
            "selection dot at {at:?}: {:?}",
            host.fills
        );
    }
}

#[test]
fn widgets_compiles_and_reports_the_host_path() {
    let path = app_path("widgets");
    let source = std::fs::read(&path).unwrap_or_else(|error| panic!("cannot read {path}: {error}"));
    let package = langhost::compile(&source, "widgets").expect("compiles");
    let result = langhost::run(&package.code, b"", langhost::DEFAULT_BUDGET).expect("verifies");
    assert_eq!(result.reason, ExitReason::Ok);
    assert_eq!(result.output, b"widgets: host ok\n".to_vec());
}

#[test]
fn clock_window_compiles_and_reports_the_host_path() {
    let path = app_path("clock_window");
    let source = std::fs::read(&path).unwrap_or_else(|error| panic!("cannot read {path}: {error}"));
    let package = langhost::compile(&source, "clock_window").expect("compiles");
    let result = langhost::run(&package.code, b"", langhost::DEFAULT_BUDGET).expect("verifies");
    assert_eq!(result.reason, ExitReason::Ok);
    assert_eq!(result.output, b"clock_window: host ok\n".to_vec());
}

/// The M5.1 calculator against `langhost`'s disabled screen: the same routine
/// arithmetic, parsing, formatting, entry editing and key dispatch run with no
/// window and print the host verdict.
#[test]
fn calculator_compiles_and_reports_the_host_path() {
    let path = app_path("calculator");
    let source = std::fs::read(&path).unwrap_or_else(|error| panic!("cannot read {path}: {error}"));
    let package = langhost::compile(&source, "calculator").expect("compiles");
    let result = langhost::run(&package.code, b"", langhost::DEFAULT_BUDGET).expect("verifies");
    assert_eq!(result.reason, ExitReason::Ok);
    assert_eq!(result.output, b"calculator: host ok\n".to_vec());
}

/// The M5.2 minesweeper against `langhost`'s disabled screen: the same board
/// placement, neighbour counts, first-click safety, flood fill, flag cap and
/// win/loss logic run with no window and print the host verdict.
#[test]
fn minesweeper_compiles_and_reports_the_host_path() {
    let path = app_path("minesweeper");
    let source = std::fs::read(&path).unwrap_or_else(|error| panic!("cannot read {path}: {error}"));
    let package = langhost::compile(&source, "minesweeper").expect("compiles");
    let result = langhost::run(&package.code, b"", langhost::DEFAULT_BUDGET).expect("verifies");
    assert_eq!(result.reason, ExitReason::Ok);
    assert_eq!(result.output, b"minesweeper: host ok\n".to_vec());
}

#[test]
fn widgets_draw_the_documented_sequence() {
    // A composed panel: menu bar and one pull-down, every control, one icon
    // row, an alert and an input dialog, plus the pure control/key results the
    // widgets app drives.
    let source = br#"
unit widseq;

uses core, stdlib, syslib;

var
  st: Status;
  items: array[string[64], 8];
  file_items: array[string[64], 16];
  shortcuts: array[string[32], 16];
  titles: array[string[32], 8];
  rows: array[uint, 32];
  text: string[256];
  cursor: int;
  start: int;
  stop: int;
  mask: uint;
  disabled: uint;
  open: int;
  selected: int;
  ok: bool;

procedure main(args: const string[1024]);
begin
  titles[0] := "File";
  titles[1] := "Edit";
  titles[2] := "View";
  file_items[0] := "New";
  file_items[1] := "Rename...";
  file_items[2] := "About...";
  file_items[3] := "-";
  file_items[4] := "Quit";
  shortcuts[0] := "Ctrl+N";
  shortcuts[1] := "Ctrl+R";
  shortcuts[2] := "";
  shortcuts[3] := "";
  shortcuts[4] := "Ctrl+Q";
  items[0] := "Alpha";
  items[1] := "Bravo";
  items[2] := "Charlie";
  items[3] := "Delta";
  items[4] := "Echo";
  items[5] := "Foxtrot";
  rows[2] := uint(5);
  text := "Hi";
  cursor := 2;
  start := 0;
  stop := 1;
  mask := stdlib.ui_menu_bit(0);
  disabled := stdlib.ui_menu_set(uint(0), 4, true);
  open := stdlib.ui_menu_open(0, 5, file_items, disabled, selected);
  st := stdlib.ui_menu_bar(10, 20, 240, titles, 3, open);
  st := stdlib.ui_menu_dropdown(
    14, 42, 220, file_items, shortcuts, 5, selected, mask, disabled, true);
  st := stdlib.ui_button(10, 120, 112, 24, "Press", false, true);
  st := stdlib.ui_checkbox(10, 152, "Enabled", true, false);
  st := stdlib.ui_text_field(10, 176, 180, 22, text, cursor, 0, start, stop, true);
  st := stdlib.ui_scrollbar(210, 120, 16, 150, 8, 8, 16, true);
  st := stdlib.ui_list(10, 210, 340, 98, items, 6, 2, 0, true);
  st := stdlib.ui_icon(10, 320, rows, stdlib.ui_black());
  st := stdlib.ui_alert(
    380, 20, 360, 160, "About Widgets", "The widgets demo.", "OK", "Cancel", 0);
  st := stdlib.ui_input_dialog(
    380, 200, 360, 200, "Rename", "Enter a name:", "OK", "Cancel",
    text, cursor, 0, start, stop, 0);
  ok := stdlib.ui_focus_next(4, 5, 1) = 0
    and stdlib.ui_list_move(6, 8, 1) = 7
    and stdlib.ui_list_ensure_visible(7, 0, 6) = 2
    and stdlib.ui_scrollbar_hit(0, 0, 16, 150, 8, 140, 0, 8, 16) = 2
    and stdlib.ui_scrollbar_clamp(99, 8, 16) = 8
    and stdlib.ui_menu_key(uint(0x100 * 65536), true) = 6
    and stdlib.ui_menu_key(uint(0x103 * 65536), true) = 5
    and stdlib.ui_menu_move(2, 5, 1, file_items, stdlib.ui_menu_set(uint(0), 4, true)) = 2
    and stdlib.ui_dialog_key(uint(0x103 * 65536), true) = 1
    and stdlib.ui_dialog_key(uint(0x102 * 65536 + 256), true) = 4
    and stdlib.ui_text_field_reveal("abcdef", 6, 0, 24) = 3
    and stdlib.ui_icon_grid_hit(0, 0, 2, 2, 72, 56, 80, 10) = 1
    and stdlib.ui_double_click(uint(0), uint(400000000), true)
    and stdlib.ui_alert_hit(380, 20, 360, 160, "OK", "Cancel", 594, 152) = 0
    and stdlib.ui_input_dialog_hit(380, 200, 360, 200, "OK", "Cancel", 400, 250) = 0;
  if ok and st = Ok then
    core.write("widgets sequence ok\n");
end;
"#;
    let package = langhost::compile(source, "widseq").expect("compiles");
    let mut scratch = vec![0u8; package.code.len().div_ceil(32)];
    let program = interpreter::Program::parse(&package.code, &mut scratch).expect("verifies");
    let header = *program.header();
    let mut state = vec![0u8; header.state_size as usize];
    let mut stack = vec![0u8; header.stack_size as usize];
    let args = vec![0u8; 8 + 1024];
    let mut machine = interpreter::Machine::new();
    let mut host = Painter {
        fills: Vec::new(),
        outlines: Vec::new(),
        texts: Vec::new(),
        output: Vec::new(),
    };
    let outcome = machine.run(
        &program,
        &mut host,
        &mut state,
        &mut stack,
        &args,
        langhost::DEFAULT_BUDGET,
    );
    assert_eq!(outcome, interpreter::Outcome::Exited(ExitReason::Ok));
    assert_eq!(host.output, b"widgets sequence ok\n");

    const BLACK: u32 = 0x000000;
    const WHITE: u32 = 0x00FF_FFFF;
    const GRAY: u32 = 0x00CC_CCCC;
    const ACCENT: u32 = 0x000080;

    // Menu bar: white bar, inverted File cell and the bottom rule.
    assert!(host.fills.contains(&(10, 20, 240, 22, WHITE)));
    assert!(host.fills.contains(&(14, 22, 40, 18, BLACK)));
    assert!(host.fills.contains(&(10, 41, 240, 1, BLACK)));
    assert!(host.texts.contains(&(18, 27, b"File".to_vec(), WHITE)));
    assert!(host.texts.contains(&(66, 27, b"Edit".to_vec(), BLACK)));
    assert!(host.texts.contains(&(114, 27, b"View".to_vec(), BLACK)));

    // Pull-down: selected accent row, checked mark, shortcut, separator and a
    // gray disabled row.
    assert!(host.outlines.contains(&(14, 42, 220, 82, BLACK)));
    assert!(host.fills.contains(&(15, 43, 218, 16, ACCENT)));
    assert!(host.fills.contains(&(24, 48, 1, 1, WHITE)));
    assert!(host.texts.contains(&(36, 47, b"New".to_vec(), WHITE)));
    assert!(host.texts.contains(&(178, 47, b"Ctrl+N".to_vec(), WHITE)));
    assert!(host.fills.contains(&(22, 99, 204, 1, BLACK)));
    assert!(host.texts.contains(&(36, 111, b"Quit".to_vec(), GRAY)));

    // Controls: button face/frame/label/ring, checkbox mark, text field
    // selection and caret, scrollbar thumb and the list selection.
    assert!(host.fills.contains(&(11, 121, 110, 22, WHITE)));
    assert!(host.outlines.contains(&(10, 120, 112, 24, BLACK)));
    assert!(host.texts.contains(&(46, 128, b"Press".to_vec(), BLACK)));
    assert!(host.fills.contains(&(13, 123, 1, 1, BLACK)));
    assert!(host.outlines.contains(&(10, 152, 11, 11, BLACK)));
    assert!(host.fills.contains(&(14, 159, 1, 1, BLACK)));
    assert!(host.texts.contains(&(27, 153, b"Enabled".to_vec(), BLACK)));
    assert!(host.fills.contains(&(11, 177, 178, 20, WHITE)));
    assert!(host.texts.contains(&(13, 183, b"Hi".to_vec(), BLACK)));
    assert!(host.fills.contains(&(13, 183, 8, 8, ACCENT)));
    assert!(host.texts.contains(&(13, 183, b"H".to_vec(), WHITE)));
    assert!(host.fills.contains(&(29, 183, 1, 8, BLACK)));
    assert!(host.fills.contains(&(211, 121, 14, 148, WHITE)));
    assert!(host.fills.contains(&(212, 195, 12, 73, GRAY)));
    assert!(host.fills.contains(&(11, 243, 338, 16, ACCENT)));
    assert!(host.texts.contains(&(14, 247, b"Charlie".to_vec(), WHITE)));
    assert!(host.texts.contains(&(14, 215, b"Alpha".to_vec(), BLACK)));

    // The icon row is one pixel run per maximal set-bit run.
    assert!(host.fills.contains(&(10, 322, 1, 1, BLACK)));
    assert!(host.fills.contains(&(12, 322, 1, 1, BLACK)));

    // Alert: frame, focused stripe, message, double-bordered default, cancel.
    assert!(host.outlines.contains(&(380, 20, 360, 160, BLACK)));
    assert!(host.fills.contains(&(381, 22, 358, 1, BLACK)));
    assert!(host
        .texts
        .contains(&(518, 24, b"About Widgets".to_vec(), BLACK)));
    assert!(host
        .texts
        .contains(&(388, 44, b"The widgets demo.".to_vec(), BLACK)));
    assert!(host.outlines.contains(&(578, 150, 68, 20, BLACK)));
    assert!(host.texts.contains(&(604, 156, b"OK".to_vec(), BLACK)));
    assert!(host.texts.contains(&(672, 156, b"Cancel".to_vec(), BLACK)));

    // Input dialog: prompt, focused field with its selection and caret, buttons.
    assert!(host.outlines.contains(&(380, 200, 360, 200, BLACK)));
    assert!(host.texts.contains(&(546, 204, b"Rename".to_vec(), BLACK)));
    assert!(host
        .texts
        .contains(&(388, 224, b"Enter a name:".to_vec(), BLACK)));
    assert!(host.outlines.contains(&(388, 244, 344, 22, BLACK)));
    assert!(host.texts.contains(&(391, 251, b"Hi".to_vec(), BLACK)));
    assert!(host.fills.contains(&(391, 251, 8, 8, ACCENT)));
    assert!(host.fills.contains(&(407, 251, 1, 8, BLACK)));
    assert!(host.texts.contains(&(604, 376, b"OK".to_vec(), BLACK)));
    assert!(host.texts.contains(&(672, 376, b"Cancel".to_vec(), BLACK)));
}

/// The M2.5.3 editor sample against `langhost`'s disabled screen: it loads a
/// small file through the host file view (spec 0013) and checks the same line,
/// edit, capacity, UTF-8 and read-only logic the kernel's check mode runs.
#[test]
fn the_editor_sample_checks_its_logic_on_the_host() {
    let path = app_path("editor");
    let source = std::fs::read(&path).unwrap_or_else(|error| panic!("cannot read {path}: {error}"));
    let package = langhost::compile(&source, "editor").expect("the editor sample compiles");
    let result =
        langhost::run(&package.code, b"", langhost::DEFAULT_BUDGET).expect("verifies and runs");
    assert_eq!(result.reason, ExitReason::Ok);
    assert_eq!(result.output, b"editor: host ok\n".to_vec());
}

/// The M2.5.4/M3.4 browser sample against `langhost`'s disabled screen: it runs
/// the same navigation (root, enter, full-path preview, up), listing, preview
/// and selection logic as the kernel check, without a window or a child launch.
#[test]
fn the_browser_sample_lists_and_previews_on_the_host() {
    let path = app_path("browser");
    let source = std::fs::read(&path).unwrap_or_else(|error| panic!("cannot read {path}: {error}"));
    let package = langhost::compile(&source, "browser").expect("the browser sample compiles");
    let result =
        langhost::run(&package.code, b"", langhost::DEFAULT_BUDGET).expect("verifies and runs");
    assert_eq!(result.reason, ExitReason::Ok);
    let output = String::from_utf8(result.output).expect("UTF-8 output");
    // The host path proves the tree walk: it starts in `base` (from the data
    // checks), previews by full path, goes up to the root, then enters `system`,
    // `home` and `lib` and previews a file in each.
    assert!(output.contains("browser: nav dir=base\n"), "{output}");
    assert!(
        output.contains("browser: nav preview=base/child\n"),
        "{output}"
    );
    assert!(output.contains("browser: nav dir=/\n"), "{output}");
    assert!(
        output.contains("browser: nav preview=system/system_probe\n"),
        "{output}"
    );
    assert!(
        output.contains("browser: nav preview=home/readme.txt\n"),
        "{output}"
    );
    assert!(output.contains("browser: nav lib=math.ig\n"), "{output}");
    assert_eq!(
        output,
        "browser: nav dir=base\n\
         browser: nav preview=base/child\n\
         browser: nav dir=/\n\
         browser: nav preview=system/system_probe\n\
         browser: nav preview=home/readme.txt\n\
         browser: nav lib=math.ig\n\
         browser: host ok\n"
    );
}

/// The file-API proof sample against `langhost`'s tree file host (spec 0013,
/// spec 0018): listing, `fs_size`, chunked reads, offsets and error paths all
/// run on the host with the same 256-byte chunk the kernel uses.
#[test]
fn the_files_sample_lists_and_reads_the_application_directory() {
    let path = app_path("files");
    let source = std::fs::read(&path).unwrap_or_else(|error| panic!("cannot read {path}: {error}"));
    let package = langhost::compile(&source, "files").expect("the files sample compiles");
    let result =
        langhost::run(&package.code, b"", langhost::DEFAULT_BUDGET).expect("verifies and runs");
    let output = String::from_utf8(result.output).expect("UTF-8 output");
    assert!(output.contains("files: root entries="), "{output}");
    assert!(output.contains("files: chunked read ok"), "{output}");
    assert!(output.ends_with("files: ok\n"), "{output}");
    assert_eq!(result.reason, ExitReason::Ok);
}

/// The writable home proof sample (spec 0019, A6): `langhost` implements the
/// home rule over a temporary directory, so the write primitive, the router and
/// the sample run end to end on the host and print the same verdict as the
/// kernel.
#[test]
fn the_home_write_sample_writes_and_reads_back_on_the_host() {
    let path = app_path("home_write");
    let source = std::fs::read(&path).unwrap_or_else(|error| panic!("cannot read {path}: {error}"));
    let package = langhost::compile(&source, "home_write").expect("the sample compiles");
    let result =
        langhost::run(&package.code, b"", langhost::DEFAULT_BUDGET).expect("verifies and runs");
    let output = String::from_utf8(result.output).expect("UTF-8 output");
    assert!(output.contains("home_write: write ok\n"), "{output}");
    assert!(output.contains("home_write: gap read ok\n"), "{output}");
    assert!(output.ends_with("home_write: ok\n"), "{output}");
    assert_eq!(result.reason, ExitReason::Ok);
}

/// The M6.3 home-operations proof (spec 0019, A6/W3–W6): mkdir, write, rename,
/// listing, remove and the error paths all run over `langhost`'s temporary home
/// directory, so the new builtins and the router are host-tested end to end.
#[test]
fn the_home_ops_sample_creates_renames_and_removes_on_the_host() {
    let path = app_path("home_ops");
    let source = std::fs::read(&path).unwrap_or_else(|error| panic!("cannot read {path}: {error}"));
    let package = langhost::compile(&source, "home_ops").expect("the sample compiles");
    let result =
        langhost::run(&package.code, b"", langhost::DEFAULT_BUDGET).expect("verifies and runs");
    let output = String::from_utf8(result.output).expect("UTF-8 output");
    assert!(output.contains("home_ops: mkdir ok\n"), "{output}");
    assert!(output.contains("home_ops: rename ok\n"), "{output}");
    assert!(output.contains("home_ops: listing after rename ok\n"), "{output}");
    assert!(output.contains("home_ops: remove directory ok\n"), "{output}");
    assert!(output.contains("home_ops: error paths ok\n"), "{output}");
    assert!(output.ends_with("home_ops: ok\n"), "{output}");
    assert_eq!(result.reason, ExitReason::Ok);
}

/// The M7.1 truncate proof (spec 0019, A7/A6): shrink, extend, zero-fill,
/// idempotence and the error classes run over `langhost`'s temporary home.
#[test]
fn the_home_truncate_sample_shrinks_and_extends_on_the_host() {
    let path = app_path("home_truncate");
    let source = std::fs::read(&path).unwrap_or_else(|error| panic!("cannot read {path}: {error}"));
    let package = langhost::compile(&source, "home_truncate").expect("the sample compiles");
    let result =
        langhost::run(&package.code, b"", langhost::DEFAULT_BUDGET).expect("verifies and runs");
    let output = String::from_utf8(result.output).expect("UTF-8 output");
    assert!(output.contains("home_truncate: write ok\n"), "{output}");
    assert!(output.contains("home_truncate: shrink ok\n"), "{output}");
    assert!(output.contains("home_truncate: extend ok\n"), "{output}");
    assert!(output.contains("home_truncate: zero fill ok\n"), "{output}");
    assert!(
        output.contains("home_truncate: idempotent ok\n"),
        "{output}"
    );
    assert!(
        output.contains("home_truncate: error paths ok\n"),
        "{output}"
    );
    assert!(output.ends_with("home_truncate: ok\n"), "{output}");
    assert_eq!(result.reason, ExitReason::Ok);
}

/// The M6.4 persistence write half (spec 0019, V2/A6): `home_persist` creates
/// the directory, writes the payload and renames it over `langhost`'s temporary
/// home directory, so the write/mkdir/rename path runs end to end on the host.
#[test]
fn the_home_persist_sample_writes_and_renames_on_the_host() {
    let path = app_path("home_persist");
    let source = std::fs::read(&path).unwrap_or_else(|error| panic!("cannot read {path}: {error}"));
    let package = langhost::compile(&source, "home_persist").expect("the sample compiles");
    let result =
        langhost::run(&package.code, b"", langhost::DEFAULT_BUDGET).expect("verifies and runs");
    let output = String::from_utf8(result.output).expect("UTF-8 output");
    assert!(output.contains("home_persist: dir ok\n"), "{output}");
    assert!(output.contains("home_persist: rename ok\n"), "{output}");
    assert!(output.contains("home_persist: target contents ok\n"), "{output}");
    assert!(output.ends_with("home_persist: ok\n"), "{output}");
    assert_eq!(result.reason, ExitReason::Ok);
}

/// The M6.4 persistence read half (spec 0019, V2/A6): a wrapper runs
/// `home_persist` and then `home_verify` as two children of the same host, so
/// both see the same temporary home directory. That is the host-side analogue
/// of the two-boot kernel proof; the real image persistence is owned by
/// `scripts/home_persistence_qmp.py`.
#[test]
fn the_home_verify_sample_reads_the_persisted_file_on_the_host() {
    let source = br#"
unit persist_sequence;
uses core, lib.out;

var
  diag: Diagnostic;
  reason: ExitReason;
  st: Status;

procedure main(args: const string[1024]);
begin
  st := core.run("home_persist", "", diag, reason);
  if st <> Ok then
    out.say("persist_sequence: run home_persist failed\n");
  st := core.run("home_verify", "", diag, reason);
  if st <> Ok then
    out.say("persist_sequence: run home_verify failed\n");
end;
"#;
    let package = langhost::compile(source, "persist_sequence").expect("the wrapper compiles");
    let result =
        langhost::run(&package.code, b"", langhost::DEFAULT_BUDGET).expect("verifies and runs");
    let output = String::from_utf8(result.output).expect("UTF-8 output");
    assert!(output.contains("home_persist: ok\n"), "{output}");
    assert!(output.contains("home_verify: directory ok\n"), "{output}");
    assert!(output.contains("home_verify: contents ok\n"), "{output}");
    assert!(output.ends_with("home_verify: ok\n"), "{output}");
    assert_eq!(result.reason, ExitReason::Ok);
}

/// A host with a screen that implements the proportional font's metrics and
/// records the drawing call (spec 0015). The widths are a small stand-in table:
/// `i` 3, `M` 11, other printable bytes 6, non-ASCII 8.
struct PropPainter {
    widths: Vec<Vec<u8>>,
    texts: Vec<(i64, i64, Vec<u8>, u32)>,
    output: Vec<u8>,
}

impl Host for PropPainter {
    fn console_write(&mut self, bytes: &[u8]) {
        self.output.extend_from_slice(bytes);
    }

    fn clock(&mut self) -> u64 {
        0
    }

    fn screen_width(&mut self) -> u64 {
        1024
    }

    fn screen_height(&mut self) -> u64 {
        768
    }

    fn text_width_prop(&mut self, text: &[u8]) -> u64 {
        self.widths.push(text.to_vec());
        text.iter()
            .map(|byte| match *byte {
                b'i' => 3,
                b'M' => 11,
                0x20..=0x7E => 6,
                _ => 8,
            })
            .sum()
    }

    fn text_height_prop(&mut self) -> u64 {
        12
    }

    fn draw_text_prop(&mut self, x: i64, y: i64, text: &[u8], color: u32) -> Status {
        self.texts.push((x, y, text.to_vec(), color));
        Status::Ok
    }
}

#[test]
fn proportional_font_calls_reach_the_host_in_order() {
    let source = br#"
unit propseq;

uses core, syslib;

var
  w: uint;
  m: uint;
  h: uint;
  st: Status;

procedure main(args: const string[1024]);
begin
  w := syslib.text_width_prop("ii");
  m := syslib.text_width_prop("MM");
  h := syslib.text_height_prop();
  st := syslib.draw_text_prop(3, 4, "Mi", uint(0x102030));
  if w = uint(6) and m = uint(22) and h = uint(12) and st = Ok then
    core.write("prop ok\n");
end;
"#;
    let package = langhost::compile(source, "propseq").expect("compiles");
    let mut scratch = vec![0u8; package.code.len().div_ceil(32)];
    let program = interpreter::Program::parse(&package.code, &mut scratch).expect("verifies");
    let header = *program.header();
    let mut state = vec![0u8; header.state_size as usize];
    let mut stack = vec![0u8; header.stack_size as usize];
    let args = vec![0u8; 8 + 1024];
    let mut machine = interpreter::Machine::new();
    let mut host = PropPainter {
        widths: Vec::new(),
        texts: Vec::new(),
        output: Vec::new(),
    };
    let outcome = machine.run(
        &program,
        &mut host,
        &mut state,
        &mut stack,
        &args,
        langhost::DEFAULT_BUDGET,
    );
    assert_eq!(outcome, interpreter::Outcome::Exited(ExitReason::Ok));
    assert_eq!(host.output, b"prop ok\n");
    assert_eq!(host.widths, vec![b"ii".to_vec(), b"MM".to_vec()]);
    assert_eq!(host.texts, vec![(3, 4, b"Mi".to_vec(), 0x10_2030)]);
}

/// The M2.6.1 font sample against `langhost`'s disabled screen: it checks the
/// 0/invalid contract and prints the host transcript.
#[test]
fn the_font_sample_reports_the_host_path() {
    let path = app_path("font");
    let source = std::fs::read(&path).unwrap_or_else(|error| panic!("cannot read {path}: {error}"));
    let package = langhost::compile(&source, "font").expect("the font sample compiles");
    let result =
        langhost::run(&package.code, b"", langhost::DEFAULT_BUDGET).expect("verifies and runs");
    assert_eq!(result.reason, ExitReason::Ok);
    assert_eq!(result.output, b"font: host ok\n".to_vec());
}

/// The M2.6.2 performance sample against `langhost`'s disabled screen (spec
/// 0016): without a framebuffer there is no frame to measure, so it reports
/// the host path and exits instead of spinning.
#[test]
fn the_perf_sample_reports_the_host_path() {
    let path = app_path("perf");
    let source = std::fs::read(&path).unwrap_or_else(|error| panic!("cannot read {path}: {error}"));
    let package = langhost::compile(&source, "perf").expect("the perf sample compiles");
    let result =
        langhost::run(&package.code, b"", langhost::DEFAULT_BUDGET).expect("verifies and runs");
    assert_eq!(result.reason, ExitReason::Ok);
    assert_eq!(
        result.output,
        b"perf: no graphics\nperf: host ok\n".to_vec()
    );
}

/// The M2.6.3 visual-regression specimen against `langhost`'s disabled screen:
/// there is no frame to compare, so it checks the 0/`Invalid` contract and
/// prints the host transcript (spec 0009, "Visual proof").
#[test]
fn the_visual_sample_reports_the_host_path() {
    let path = app_path("visual");
    let source = std::fs::read(&path).unwrap_or_else(|error| panic!("cannot read {path}: {error}"));
    let package = langhost::compile(&source, "visual").expect("the visual sample compiles");
    let result =
        langhost::run(&package.code, b"", langhost::DEFAULT_BUDGET).expect("verifies and runs");
    assert_eq!(result.reason, ExitReason::Ok);
    assert_eq!(result.output, b"visual: host ok\n".to_vec());
}

/// The M2.6.4 desktop sample on `langhost`'s disabled screen: the menu bar and
/// the System pull-down need a framebuffer, so the host path only reports the
/// no-graphics contract (spec 0012, spec 0017).
#[test]
fn the_desktop_shell_reports_the_host_path() {
    let path = app_path("desktop_shell");
    let source = std::fs::read(&path).unwrap_or_else(|error| panic!("cannot read {path}: {error}"));
    let package = langhost::compile(&source, "desktop_shell").expect("the desktop sample compiles");
    let result = langhost::run(&package.code, b"check", langhost::DEFAULT_BUDGET)
        .expect("verifies and runs");
    assert_eq!(result.reason, ExitReason::Ok);
    assert_eq!(result.output, b"desktop_shell: no graphics\n".to_vec());
}

/// `core.halt`/`core.reboot` have no authority on a bare host: both return
/// `Invalid`, so the probe prints its return line and the host keeps running
/// (spec 0017).
#[test]
fn the_system_probe_reports_no_kernel_authority() {
    let path = app_path("system_probe");
    let source = std::fs::read(&path).unwrap_or_else(|error| panic!("cannot read {path}: {error}"));
    let package = langhost::compile(&source, "system_probe").expect("the probe compiles");
    let result =
        langhost::run(&package.code, b"halt", langhost::DEFAULT_BUDGET).expect("verifies and runs");
    assert_eq!(result.reason, ExitReason::Ok);
    assert_eq!(
        result.output,
        b"system_probe: halt requested\nsystem_probe: halt returned (no kernel authority)\n"
            .to_vec()
    );
    let result = langhost::run(&package.code, b"reboot", langhost::DEFAULT_BUDGET)
        .expect("verifies and runs");
    assert_eq!(
        result.output,
        b"system_probe: reboot requested\nsystem_probe: reboot returned (no kernel authority)\n"
            .to_vec()
    );
}

#[test]
fn poll_any_and_next_any_report_console_stream_and_input_on_the_host() {
    // The host models every endpoint as one loopback FIFO, so `out`/`in` can be
    // two names here even though the kernel would pair them separately. The
    // point is the builtin contract: a ready stream, a queued event and the
    // scripted console bytes are each reported by index.
    let source = br#"
unit mh;

uses core, syslib;

var
  out: out_stream[16];
  inp: in_stream[16];
  sources: array[uint, 8];
  count: int;
  ready: int;
  st: Status;
  raw: bytes[16];
  dst: bytes[16];
  got: int;
  ev: Event;

procedure main(args: const string[1024]);
begin
  st := core.send(out, "ping");
  sources[0] := core.stream_source(inp);
  sources[1] := uint(1);
  count := 2;
  ready := -1;
  st := core.next_any(sources, count, ready);
  if st = Ok and ready = 0 then core.write("stream ok\n") else core.write("stream FAIL\n");
  got := core.recv(inp, dst);
  if got = 4 then core.write("recv ok\n") else core.write("recv FAIL\n");

  ev.kind := KeyDown;
  ev.x := 0;
  ev.y := 0;
  ev.code := uint(88 * 65536);
  ev.window := 0;
  st := syslib.inject_event(ev);
  sources[0] := uint(1);
  count := 1;
  ready := -1;
  st := core.next_any(sources, count, ready);
  if st = Ok and ready = 0 then core.write("input ok\n") else core.write("input FAIL\n");
  st := syslib.next_event(ev);
  if st = Ok and ev.code = uint(88 * 65536) then core.write("event ok\n") else core.write("event FAIL\n");

  sources[0] := uint(0);
  count := 1;
  ready := -1;
  st := core.poll_any(sources, count, ready);
  if st = Ok and ready = 0 then core.write("console ready ok\n") else core.write("console ready FAIL\n");
  got := core.read_input(raw);
  if got >= 4 and raw[0] = byte(116) and raw[1] = byte(105) and raw[2] = byte(99) and raw[3] = byte(107) then
    core.write("console line ok\n")
  else
    core.write("console line FAIL\n");
  core.write("multiplex_host: ok\n");
end;
"#;
    let package = langhost::compile(source, "mh").expect("compiles");
    let result = langhost::run_with_input(&package.code, b"", b"tick\n", langhost::DEFAULT_BUDGET)
        .expect("verifies");
    assert_eq!(result.reason, ExitReason::Ok);
    assert_eq!(
        String::from_utf8(result.output).expect("UTF-8 output"),
        "stream ok\n\
         recv ok\n\
         input ok\n\
         event ok\n\
         console ready ok\n\
         console line ok\n\
         multiplex_host: ok\n"
    );
}
