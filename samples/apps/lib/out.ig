unit out;

// Importable module source (spec 0018, M2/M3/M10; ADR 0039). It holds the two
// text writers every sample application used to declare locally, so the
// application source keeps only its own logic. It is not an application: no
// `main`, no top-level `var` state and no stream endpoint.
//
// Ordinary language code only: no new syntax, opcode or builtin. `say` forwards
// the caller's buffer to `core.write`; `say_int` formats a decimal `int` into a
// local `bytes[32]` and writes that. The staging buffer is local, so the module
// owns no state and the text reaches the console in one write.

uses core;

// Write a string or bytes buffer exactly as given (no newline is added).
procedure say(text: const string[0]);
begin
  core.write(text);
end;

// Write `value` in decimal with no padding.
procedure say_int(value: int);
var
  digits: bytes[32];
  used: int;
begin
  used := core.format_int_into(digits, value, uint(10), uint(0));
  core.write(digits);
end;
