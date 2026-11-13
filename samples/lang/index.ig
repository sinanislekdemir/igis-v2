unit index;

uses core;

var
  raw: bytes[16];
  text: string[8];

procedure main(args: const string[1024]);
begin
  // `bytes` is writable by index. Writing at the current length appends; the
  // result is a normal buffer, so it compares like one.
  raw := "ab";
  raw[2] := byte(99);
  if raw = "abc" then
    core.write("extend ok\n")
  else
    core.write("extend bad\n");

  // Writing past the length extends it and zero-fills the gap.
  raw[5] := byte(46);
  if core.len(raw) = uint(6) then
    core.write("len ok\n")
  else
    core.write("len bad\n");
  if raw[3] = byte(0) then
    core.write("zero ok\n")
  else
    core.write("zero bad\n");

  // Writing inside the length replaces one byte and keeps the length.
  raw[0] := byte(65);
  if raw[0] = byte(65) then
    core.write("write ok\n")
  else
    core.write("write bad\n");
  if raw = "abc" then
    core.write("neq bad\n")
  else
    core.write("neq ok\n");

  // `string` is read-only by index but compares with a literal of any capacity
  // (T4: an index reads a raw byte, which is all equality needs).
  text := "help";
  if text = "help" then
    core.write("eq ok\n")
  else
    core.write("eq bad\n");
  if text[0] = byte(104) then
    core.write("byte ok\n")
  else
    core.write("byte bad\n");
end;
