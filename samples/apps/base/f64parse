unit f64parse;

// Kernel proof for correctly rounded decimal text → binary64 parsing
// (spec 0003, "Floating-point text"; Q41; `core.parse_f64_into`, builtin 50).
//
// `stdlib.parse_f64_into` delegates to the primitive. The oracle here is
// `core.format_f64_short_into`, whose digit search is exact integer
// arithmetic: parsing a decimal text and formatting the result must reproduce
// the expected shortest text, and therefore the exact bit pattern of the
// value. The repeated ×/÷10 reader this replaced was off by one or more ulps
// (`1e-7`, the subnormal boundaries), so the first cases are the correction.
//
//   run f64parse

uses core, stdlib;

var
  text: string[48];
  value: f64;
  st: Status;
  n: int;
  cases: int;
  failures: int;

procedure fail(src: const string[0]; why: const string[0]);
begin
  core.write("f64parse: FAIL ");
  core.write(src);
  core.write(" ");
  core.write(why);
  core.write("\n");
  failures := failures + 1;
end;

// Parses `src` through the stdlib helper and requires the shortest text of
// the resulting value to be `expected` (the exact bits, by construction).
procedure check(src: const string[0]; expected: const string[0]);
begin
  cases := cases + 1;
  st := stdlib.parse_f64_into(src, value);
  if st <> Ok then
    fail(src, "status")
  else
  begin
    n := core.format_f64_short_into(text, value);
    if n < 0 then
      fail(src, "format")
    else if text <> expected then
    begin
      core.write("f64parse: FAIL ");
      core.write(src);
      core.write(" got ");
      core.write(text);
      core.write("\n");
      failures := failures + 1;
    end;
  end;
end;

// The same check through the primitive directly, so the wrapper is not the
// only thing exercised.
procedure check_core(src: const string[0]; expected: const string[0]);
begin
  cases := cases + 1;
  st := core.parse_f64_into(src, value);
  if st <> Ok then
    fail(src, "core status")
  else
  begin
    n := core.format_f64_short_into(text, value);
    if n < 0 or text <> expected then
      fail(src, "core value");
  end;
end;

// A malformed string is `Invalid` and leaves `value` untouched (2.5 stays).
procedure check_reject(src: const string[0]);
begin
  cases := cases + 1;
  value := 2.5;
  st := core.parse_f64_into(src, value);
  if st <> Invalid then
    fail(src, "accepted")
  else
  begin
    n := core.format_f64_short_into(text, value);
    if n < 0 or text <> "2.5" then
      fail(src, "wrote through value");
  end;
end;

procedure main(args: const string[1024]);
begin
  cases := 0;
  failures := 0;

  // The Q41 correction: exact rounding, not repeated ×/÷10.
  check("1e-7", "1e-7");
  check("1e-6", "0.000001");
  check("2.2250738585072012e-308", "2.2250738585072014e-308");
  check("2.2250738585072011e-308", "2.225073858507201e-308");

  // Halfway cases round half to even.
  check("1.00000000000000011102230246251565404236316680908203125", "1");
  check("1.0000000000000002220446049250313080847263336181640625",
    "1.0000000000000002");
  check("9007199254740993", "9007199254740992");
  check("9007199254740995", "9007199254740996");

  // The subnormal tail and its half-ulp boundary.
  check("4.9406564584124654e-324", "5e-324");
  check("2.4703282292062328e-324", "5e-324");
  check("2.4703282292062327e-324", "0");
  check("1e-323", "1e-323");

  // Overflow and underflow saturate with Ok, keeping the sign; -0 is -0.
  check("1.7976931348623157e308", "1.7976931348623157e308");
  check("1.7976931348623158e308", "1.7976931348623157e308");
  check("1.7976931348623159e308", "inf");
  check("1e309", "inf");
  check("-1e309", "-inf");
  check("1e400", "inf");
  check("1e-400", "0");
  check("-1e-400", "-0");
  check("-0.0", "-0");

  // Long mantissas: 38 digits keep their sticky bit.
  check("1234567890123456789012345678901234567890e-40", "0.12345678901234568");
  check("0.1000000000000000055511151231257827021181583404541015625", "0.1");

  // The grammar's edges and the formatter's specials.
  check("3.25", "3.25");
  check("1.", "1");
  check(".5", "0.5");
  check("+2", "2");
  check("2.5e-3", "0.0025");
  check("1E+2", "100");
  check("inf", "inf");
  check("+inf", "inf");
  check("-inf", "-inf");
  check("nan", "nan");

  // The primitive itself, not only the stdlib wrapper.
  check_core("1e-7", "1e-7");
  check_core("4.9406564584124654e-324", "5e-324");
  check_core("-inf", "-inf");

  // Every malformed form is Invalid and writes nothing.
  check_reject("");
  check_reject("+");
  check_reject("-");
  check_reject(".");
  check_reject("e5");
  check_reject("1e");
  check_reject("1e+");
  check_reject("1.2.3");
  check_reject(" 1");
  check_reject("1 ");
  check_reject("--1");
  check_reject("1,5");
  check_reject("infinity");
  check_reject("NaN");
  check_reject("INF");
  check_reject("+nan");

  if failures = 0 then
  begin
    core.write("f64parse: ok ");
    n := core.format_int_into(text, cases, uint(10), uint(0));
    core.write(text);
    core.write(" cases\n");
  end
  else
  begin
    core.write("f64parse: failed ");
    n := core.format_int_into(text, failures, uint(10), uint(0));
    core.write(text);
    core.write("\n");
  end;
end;
