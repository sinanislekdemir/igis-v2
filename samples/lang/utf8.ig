unit utf8;

// Host sample for the UTF-8 slice (spec 0003, "Text and bytes" and "Slicing
// and search"): checked bytes→string conversion, code-point decoding in
// `stdlib`, and the four mixed-kind `core.substr_into` combinations.
//
// `text` is "aé€😀": one 1-byte, one 2-byte (é = C3 A9), one 3-byte
// (€ = E2 82 AC) and one 4-byte (😀 = F0 9F 98 80) code point, 10 bytes long.
// `langhost` asserts this output byte for byte.

uses core, stdlib;

var
  text: string[32];
  part: string[32];
  wide: string[256];
  blank: string[8];
  head: string[128];
  tail: string[256];
  raw: bytes[32];
  bad: bytes[8];
  piece: bytes[8];
  big: bytes[300];
  nothing: bytes[8];
  digits: string[32];
  scalar: int;
  next: int;
  at: int;
  n: int;
  st: Status;
  ok: bool;

procedure say(text: const string[0]);
begin
  core.write(text);
end;

procedure show(value: int);
begin
  n := core.format_int_into(digits, value, uint(10), uint(0));
  core.write(digits);
end;

procedure report(name: const string[0]; passed: bool);
begin
  if passed then
    core.write(name)
  else
    core.write("FAILED ");
  core.write("\n");
end;

// Every malformed class must be rejected by both the conversion primitive and
// the language validator, leaving the destination untouched.
procedure check_invalid(name: const string[0]; data: const bytes[0]);
begin
  wide := "keep";
  st := stdlib.utf8_decode_into(wide, data);
  report(name, st = Invalid and wide = "keep" and not stdlib.utf8_valid(data));
end;

procedure main(args: const string[1024]);
begin
  text := "aé€😀";
  raw := "aé€😀";

  // ---- utf8_len -------------------------------------------------------
  blank := "";
  ok := stdlib.utf8_len(blank) = 0;
  report("len empty ok", ok);
  ok := stdlib.utf8_len("ascii") = 5;
  report("len ascii ok", ok);
  ok := stdlib.utf8_len(text) = 4 and int(core.len(text)) = 10;
  report("len mixed ok", ok);

  // ---- utf8_decode at every boundary ----------------------------------
  st := stdlib.utf8_decode(text, 0, scalar, next);
  ok := st = Ok and scalar = 97 and next = 1;
  report("decode 1-byte ok", ok);

  st := stdlib.utf8_decode(text, 1, scalar, next);
  ok := st = Ok and scalar = 0xE9 and next = 3;
  report("decode 2-byte ok", ok);

  st := stdlib.utf8_decode(text, 3, scalar, next);
  ok := st = Ok and scalar = 0x20AC and next = 6;
  report("decode 3-byte ok", ok);

  st := stdlib.utf8_decode(text, 6, scalar, next);
  ok := st = Ok and scalar = 0x1F600 and next = 10;
  report("decode 4-byte ok", ok);

  // ---- utf8_decode rejects bad starts, writing nothing -----------------
  scalar := -1;
  next := -1;
  st := stdlib.utf8_decode(text, 2, scalar, next);
  report("decode continuation ok", st = Invalid and scalar = -1 and next = -1);

  st := stdlib.utf8_decode(text, 10, scalar, next);
  report("decode at end ok", st = Invalid and scalar = -1 and next = -1);

  st := stdlib.utf8_decode(text, -1, scalar, next);
  report("decode negative ok", st = Invalid and scalar = -1 and next = -1);

  // ---- utf8_decode_into: success --------------------------------------
  wide := "";
  st := stdlib.utf8_decode_into(wide, raw);
  ok := st = Ok and wide = text and stdlib.utf8_len(wide) = 4;
  report("decode into ok", ok);

  // ---- utf8_decode_into: source larger than the destination ------------
  at := 0;
  while at < 300 do
  begin
    big[at] := byte(97);
    at := at + 1;
  end;
  wide := "keep";
  st := stdlib.utf8_decode_into(wide, big);
  report("decode into capacity ok", st = CapacityExceeded and wide = "keep");

  // ---- one case per malformed class -----------------------------------
  bad := "\x80";
  check_invalid("decode into stray ok", bad);
  bad := "\xC0\xAF";
  check_invalid("decode into overlong ok", bad);
  bad := "\xED\xA0\x80";
  check_invalid("decode into surrogate ok", bad);
  bad := "\xF4\x90\x80\x80";
  check_invalid("decode into too large ok", bad);
  bad := "\xE2\x82";
  check_invalid("decode into truncated ok", bad);

  // ---- utf8_valid on well-formed input --------------------------------
  nothing := "";
  report("valid empty ok", stdlib.utf8_valid(nothing));
  bad := "abc";
  report("valid ascii ok", stdlib.utf8_valid(bad));
  report("valid mixed ok", stdlib.utf8_valid(raw));

  // ---- core.substr_into: string destination ---------------------------
  // 1..6 is exactly "é€" (both boundaries on code points).
  n := core.substr_into(part, text, 1, 5);
  report("substr boundaries ok", n = 5 and part = "é€");

  // A mid-code-point slice is not a `string` and leaves the destination be.
  part := "keep";
  n := core.substr_into(part, text, 2, 1);
  report("substr split ok", n = -4 and part = "keep");

  // A start past the end copies nothing and clears the destination.
  part := "keep";
  n := core.substr_into(part, text, 11, 2);
  report("substr past end ok", n = 0 and int(core.len(part)) = 0);

  part := "keep";
  n := core.substr_into(part, text, -1, 1);
  report("substr negative ok", n = -4 and part = "keep");

  // ---- string → bytes: a raw cut, no UTF-8 check -----------------------
  n := core.substr_into(piece, text, 2, 1);
  report("substr raw slice ok", n = 1 and piece[0] = byte(0xA9));

  // ---- bytes → string: the copied range is validated strictly ----------
  n := core.substr_into(part, raw, 1, 5);
  report("substr bytes to string ok", n = 5 and part = "é€");

  bad := "\x80";
  part := "keep";
  n := core.substr_into(part, bad, 0, 1);
  report("substr bytes invalid ok", n = -4 and part = "keep");

  // ---- split_once still splits multibyte text at an ASCII separator ----
  n := stdlib.split_once(head, tail, "é/€", byte(47));
  report("split multibyte ok", n = 1 and head = "é" and tail = "€");
end;
