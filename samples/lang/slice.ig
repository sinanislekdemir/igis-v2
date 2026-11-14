unit slice;

uses core;

var
  text: string[16];
  part: string[8];
  raw: bytes[8];
  piece: bytes[4];
  at: int;
  n: int;

procedure main(args: const string[1024]);
begin
  text := "hello world";

  at := core.find(text, byte(119));
  if at = 6 then
    core.write("find hit ok\n")
  else
    core.write("find hit bad\n");

  at := core.find(text, byte(122));
  if at = -1 then
    core.write("find miss ok\n")
  else
    core.write("find miss bad\n");

  n := core.substr_into(part, text, 6, 5);
  if n = 5 then
  begin
    if part = "world" then
      core.write("substr ok\n")
    else
      core.write("substr bad\n");
  end
  else
    core.write("substr len bad\n");

  // Zero length clears the destination.
  n := core.substr_into(part, text, 5, 0);
  if n = 0 then
  begin
    if core.len(part) = uint(0) then
      core.write("clear ok\n")
    else
      core.write("clear bad\n");
  end
  else
    core.write("clear len bad\n");

  // A start past the source length copies nothing and returns 0.
  text := "abc";
  n := core.substr_into(part, text, 9, 2);
  if n = 0 then
    core.write("past end ok\n")
  else
    core.write("past end bad\n");

  // A `string` slice that splits the two-byte `é` is rejected: -4
  // (Status.Invalid), and the destination is untouched.
  text := "é";
  part := "keep";
  n := core.substr_into(part, text, 1, 1);
  if n = -4 then
  begin
    if part = "keep" then
      core.write("utf8 ok\n")
    else
      core.write("utf8 write bad\n");
  end
  else
    core.write("utf8 status bad\n");

  // Both boundaries fall on code points, so the two-byte sequence copies.
  n := core.substr_into(part, text, 0, 2);
  if n = 2 then
  begin
    if part = "é" then
      core.write("utf8 slice ok\n")
    else
      core.write("utf8 slice bad\n");
  end
  else
    core.write("utf8 slice len bad\n");

  // `bytes` copies raw data and clamps to the destination capacity.
  raw := "abcdef";
  n := core.substr_into(piece, raw, 1, 99);
  if n = 4 then
  begin
    if piece = "bcde" then
      core.write("bytes ok\n")
    else
      core.write("bytes bad\n");
  end
  else
    core.write("bytes len bad\n");
end;
