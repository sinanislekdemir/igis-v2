unit f64text;

// Host sample for fixed-point `f64` text (spec 0003, "Floating point").
//
// `core.format_f64_into` writes decimal text with a fixed fraction width and
// `stdlib.parse_f64_into` reads it back. Each line is either `label: [text]
// n=N` (N is the byte count, or a negative status code when nothing was
// written) or `label: [text]` for a parsed value, with `status=N` for a
// rejected one.

uses core, stdlib;

var
  text: string[64];
  tiny: string[3];
  value: f64;
  parsed: f64;
  st: Status;
  n: int;
  zero: f64;

procedure say(line: const string[0]);
begin
  core.write(line);
end;

// Formats `v` with `digits` fraction digits.
procedure show(label: const string[0]; v: f64; digits: uint);
begin
  say(label);
  say(": ");
  n := core.format_f64_into(text, v, digits);
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

// Formats into a destination that is too small for any useful result.
procedure show_tiny(label: const string[0]; v: f64);
begin
  n := core.format_f64_into(tiny, v, uint(1));
  say(label);
  say(": n=");
  n := core.format_int_into(text, n, uint(10), uint(0));
  say(text);
  say("\n");
end;

// Parses `src` and formats the result with 3 digits, or reports the status.
procedure parse_case(label: const string[0]; src: const string[0]);
begin
  st := stdlib.parse_f64_into(src, parsed);
  say(label);
  say(": ");
  if st = Ok then
  begin
    n := core.format_f64_into(text, parsed, uint(3));
    say("[");
    say(text);
    say("]\n");
  end
  else
  begin
    say("status=");
    n := core.format_int_into(text, int(st), uint(10), uint(0));
    say(text);
    say("\n");
  end;
end;

procedure main(args: const string[1024]);
begin
  // Formatting: widths from 0 to 9, signs, rounding and the specials.
  show("zero/0", 0.0, uint(0));
  show("zero/3", 0.0, uint(3));
  show("negzero/3", -0.0, uint(3));
  show("1.5/1", 1.5, uint(1));
  show("-1.5/2", -1.5, uint(2));
  show("42/0", 42.0, uint(0));
  show("carry/2", 9.999, uint(2));
  show("-half/0", -2.5, uint(0));
  show("third/9", 1.0 / 3.0, uint(9));
  show("third/0", 1.0 / 3.0, uint(0));
  show("width10", 1.0, uint(10));

  zero := 0.0;
  value := 1.0 / zero;
  show("inf", value, uint(2));
  value := -1.0 / zero;
  show("-inf", value, uint(2));
  value := zero / zero;
  show("nan", value, uint(2));
  show("big", 1.0e300, uint(0));
  show_tiny("tiny", 42.5);

  // Parsing: valid forms, then malformed ones. `Invalid` is `-4` on the
  // formatter and `Status.Invalid`, whose ordinal is printed here.
  parse_case("p:3.25", "3.25");
  parse_case("p:-0.5", "-0.5");
  parse_case("p:+2", "+2");
  parse_case("p:.25", ".25");
  parse_case("p:1.", "1.");
  parse_case("p:2.5e-3", "2.5e-3");
  parse_case("p:1E+2", "1E+2");
  parse_case("p:-0.0", "-0.0");
  parse_case("p:inf", "inf");
  parse_case("p:-inf", "-inf");
  parse_case("p:nan", "nan");
  parse_case("p:1e400", "1e400");
  parse_case("p:1e-400", "1e-400");
  parse_case("p:empty", "");
  parse_case("p:1.2.3", "1.2.3");
  parse_case("p:1e", "1e");
  parse_case("p:sign", "+");
  parse_case("p:space", "1 2");
end;
