unit floats;

// Kernel sample for fixed-point `f64` text (spec 0003, "Floating point").
//
// It formats a fixed table of values through `core.format_f64_into`, then —
// when `main(args)` is non-empty — parses the argument with
// `stdlib.parse_f64_into` and formats the value back with 3 digits.
//
//   run floats            the table only
//   run floats 3.25       table, then `value=3.250`
//   run floats nope       table, then `Invalid`

uses core, stdlib, lib.out;

var
  text: string[64];
  tiny: string[3];
  value: f64;
  parsed: f64;
  st: Status;
  n: int;
  zero: f64;

procedure show(label: const string[0]; v: f64; digits: uint);
begin
  out.say(label);
  out.say("=");
  n := core.format_f64_into(text, v, digits);
  if n >= 0 then
    out.say(text)
  else
  begin
    // A negative result is a `Status` code: -4 Invalid, -6 CapacityExceeded.
    out.say("status ");
    n := core.format_int_into(text, n, uint(10), uint(0));
    out.say(text);
  end;
  out.say("\n");
end;

// A destination too small for any useful result: `42.5` with one fraction
// digit needs four bytes, so a three-byte buffer reports `-6` and is not
// written.
procedure show_tiny();
begin
  out.say("tiny=");
  n := core.format_f64_into(tiny, 42.5, uint(1));
  n := core.format_int_into(text, n, uint(10), uint(0));
  out.say(text);
  out.say("\n");
end;

// The shortest round-trip form (Q41): no width argument, no magnitude bound.
procedure show_short(label: const string[0]; v: f64);
begin
  out.say(label);
  out.say("=");
  n := core.format_f64_short_into(text, v);
  if n >= 0 then
    out.say(text)
  else
  begin
    out.say("status ");
    n := core.format_int_into(text, n, uint(10), uint(0));
    out.say(text);
  end;
  out.say("\n");
end;

procedure main(args: const string[1024]);
begin
  show("zero", 0.0, uint(2));
  show("negzero", -0.0, uint(3));
  show("neg", -1.5, uint(1));
  show("carry", 9.999, uint(2));
  show("third", 1.0 / 3.0, uint(9));
  show("whole", 42.0, uint(0));

  zero := 0.0;
  value := 1.0 / zero;
  show("inf", value, uint(0));
  value := -1.0 / zero;
  show("-inf", value, uint(0));
  value := zero / zero;
  show("nan", value, uint(0));
  show("big", 1.0e300, uint(0));
  show("width10", 1.0, uint(10));
  show_tiny();

  show_short("s:third", 1.0 / 3.0);
  show_short("s:1e21", 1.0e21);
  show_short("s:tinysub", 5.0e-324);
  show_short("s:max", 1.7976931348623157e308);

  if core.len(args) > uint(0) then
  begin
    out.say("parse=[");
    out.say(args);
    out.say("] ");
    st := stdlib.parse_f64_into(args, parsed);
    if st = Ok then
      show("value", parsed, uint(3))
    else
      out.say("Invalid\n");
  end
  else
    out.say("no argument\n");
end;
