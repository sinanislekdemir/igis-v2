unit f64short;

// Host sample for shortest round-trip `f64` text (spec 0003, "Floating
// point"; Q41).
//
// `core.format_f64_short_into(dst, value)` writes the shortest digit string
// that parses back to exactly the same bits, positionally while the decimal
// point stays inside the spec's window and in scientific form otherwise. Each
// line is `label: [text] n=N`, where N is the byte count or a negative status
// code when nothing was written.

uses core;

var
  text: string[64];
  tiny: string[3];
  n: int;
  zero: f64;
  value: f64;

procedure say(line: const string[0]);
begin
  core.write(line);
end;

// Formats `v` with the shortest round-trip form.
procedure show(label: const string[0]; v: f64);
begin
  say(label);
  say(": ");
  n := core.format_f64_short_into(text, v);
  if n >= 0 then
  begin
    say("[");
    say(text);
    say("] n=");
  end
  else
    say("failed n=");
  n := core.format_int_into(text, n, uint(10), uint(0));
  say(text);
  say("\n");
end;

// A destination too small for the exponent form: `1e21` needs four bytes.
procedure show_tiny(label: const string[0]; v: f64);
begin
  n := core.format_f64_short_into(tiny, v);
  say(label);
  say(": n=");
  n := core.format_int_into(text, n, uint(10), uint(0));
  say(text);
  say("\n");
end;

procedure main(args: const string[1024]);
begin
  // Zero, signs and the shortest forms of small values.
  show("zero", 0.0);
  show("negzero", -0.0);
  show("one", 1.0);
  show("-one", -1.0);
  show("tenth", 0.1);
  show("third", 1.0 / 3.0);
  show("0.1+0.2", 0.1 + 0.2);

  // The positional / scientific boundary: 1e20 stays positional, 1e21 does
  // not; 1e-6 stays positional, 1e-7 does not.
  show("1e20", 1.0e20);
  show("1e21", 1.0e21);
  show("1e-6", 1.0e-6);
  show("1e-7", 1.0e-7);
  show("1.5e-7", 1.5e-7);

  // Integers beyond the 53-bit mantissa: 2^53 + 1 is not representable, so it
  // prints as 2^53.
  show("2^53", 9007199254740992.0);
  show("2^53+1", 9007199254740993.0);
  show("2^53+2", 9007199254740994.0);

  // The extremes and the subnormal tail.
  show("max", 1.7976931348623157e308);
  show("min+", 2.2250738585072014e-308);
  show("min-sub", 5.0e-324);
  show("1e300", 1.0e300);

  zero := 0.0;
  value := 1.0 / zero;
  show("inf", value);
  value := -1.0 / zero;
  show("-inf", value);
  value := zero / zero;
  show("nan", value);

  show_tiny("tiny", 1.0e21);
end;
