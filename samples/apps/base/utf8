unit utf8;

// Kernel sample for the UTF-8 slice (spec 0003, "Text and bytes"): code-point
// decoding and counting in `stdlib`, checked `bytes`→`string` conversion, and
// the four mixed-kind `core.substr_into` combinations.
//
// The text is built from literals, so the image needs no console round-trip.
// `run utf8` in the shell prints the values below and exits `Ok`.

uses core, stdlib, lib.out;

var
  text: string[64];
  part: string[16];
  wide: string[256];
  raw: bytes[64];
  malformed: bytes[8];
  digits: string[32];

procedure show(value: int);
begin
  core.format_int_into(digits, value, uint(10), uint(0));
  core.write(digits);
end;

procedure main(args: const string[1024]);
var
  index: int;
  scalar: int;
  next: int;
  st: Status;
  length: int;
begin
  text := "aé€😀";
  raw := "aé€😀";
  length := int(core.len(text));

  out.say("utf8: bytes=");
  show(length);
  out.say(" codepoints=");
  show(stdlib.utf8_len(text));
  out.say("\n");

  index := 0;
  while index < length do
  begin
    st := stdlib.utf8_decode(text, index, scalar, next);
    if st = Ok then
    begin
      out.say("utf8: at ");
      show(index);
      out.say(" scalar=");
      show(scalar);
      out.say(" next=");
      show(next);
      out.say("\n");
      index := next;
    end
    else
    begin
      out.say("utf8: decode failed\n");
      index := length;
    end;
  end;

  // `string` → `bytes` is a raw cut; `bytes` → `string` is checked.
  out.say("utf8: raw cut length=");
  show(core.substr_into(malformed, text, 2, 1));
  out.say("\n");
  st := stdlib.utf8_decode_into(wide, raw);
  if st = Ok then
  begin
    out.say("utf8: decode_into codepoints=");
    show(stdlib.utf8_len(wide));
    out.say("\n");
  end
  else
    out.say("utf8: decode_into failed\n");

  malformed := "\x80";
  part := "keep";
  show(core.substr_into(part, malformed, 0, 1));
  if part = "keep" then
    out.say(" invalid untouched\n")
  else
    out.say(" invalid wrote\n");
end;
