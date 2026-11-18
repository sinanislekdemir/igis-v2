unit home_write;

// M6.2 proof app: the writable home filesystem (spec 0019, A1–A3).
//
//   run home_write
//
// Writes `home/written.txt` at offset 0 (creating it when it is missing), reads
// it back through `core.fs_size`/`core.fs_read`, appends the next line at the
// file's current end, and then writes past the end so the gap must read back as
// zero bytes. Every step re-reads the file instead of trusting the write.
//
// The app is idempotent: there is no truncate, so a second run overwrites the
// first bytes and appends again at the (now larger) end. That keeps the proof
// repeatable on a persistent image.
//
// Without a home disk `core.fs_write` is `Invalid` (the mount is read-only) and
// the verdict is `home_write: no writable home`; no other call is attempted.
// The host (`langhost`) implements the same home rule over a temporary
// directory (spec 0019, A6), so it prints the same `home_write: ok` lines.

uses core, lib.out;

var
  ok: bool;
  no_home: bool;
  st: Status;
  written: int;
  count: int;
  size: uint;
  size0: uint;
  size1: uint;
  chunk: bytes[256];
  prefix: bytes[16];
  data: bytes[256];
  more: bytes[256];
  tail: bytes[8];

// One line per step; the first failure names the step and stops the checks.
procedure check(label: const string[0]; passed: bool);
begin
  if ok then
  begin
    if passed then
    begin
      out.say("home_write: ");
      out.say(label);
      out.say(" ok\n");
    end
    else
    begin
      ok := false;
      out.say("home_write: FAIL ");
      out.say(label);
      out.say("\n");
    end;
  end;
end;

procedure setup();
begin
  if ok then
  begin
    count := core.substr_into(data, "igis home write\n", 0, 16);
    if count <> 16 then
      ok := false;
    count := core.substr_into(more, "second line\n", 0, 12);
    if count <> 12 then
      ok := false;
    count := core.substr_into(tail, "gap", 0, 3);
    if count <> 3 then
      ok := false;
  end;
end;

procedure first_write();
begin
  if ok then
  begin
    written := -1;
    st := core.fs_write("home/written.txt", uint(0), data, written);
    if st = Invalid then
    begin
      // No home disk (or a read-only mount): the primitive exists but refuses.
      ok := false;
      no_home := true;
      out.say("home_write: no writable home\n");
    end
    else
      check("write", st = Ok and written = 16);
  end;
end;

procedure check_size();
begin
  if ok then
  begin
    size := uint(0);
    st := core.fs_size("home/written.txt", size);
    check("size", st = Ok and size >= uint(16));
    if ok then
      size0 := size;
  end;
end;

procedure read_back();
begin
  if ok then
  begin
    // A later run leaves the file longer than this write (there is no
    // truncate), so compare the first sixteen bytes rather than the chunk.
    count := -1;
    chunk := "keep";
    st := core.fs_read("home/written.txt", uint(0), chunk, count);
    check("read",
      st = Ok and count >= 16
        and core.substr_into(prefix, chunk, 0, 16) = 16 and prefix = data);
  end;
end;

procedure append();
begin
  if ok then
  begin
    written := -1;
    st := core.fs_write("home/written.txt", size0, more, written);
    check("append", st = Ok and written = 12);
    if ok then
      size1 := size0 + uint(12);
  end;
end;

procedure read_append();
begin
  if ok then
  begin
    count := -1;
    chunk := "keep";
    st := core.fs_read("home/written.txt", size0, chunk, count);
    check("append read",
      st = Ok and count >= 12
        and core.substr_into(prefix, chunk, 0, 12) = 12 and prefix = more);
  end;
end;

procedure write_gap();
begin
  if ok then
  begin
    // Four zero bytes between the old end and this write.
    written := -1;
    st := core.fs_write("home/written.txt", size1 + uint(4), tail, written);
    check("gap write", st = Ok and written = 3);
  end;
end;

procedure read_gap();
begin
  if ok then
  begin
    count := -1;
    chunk := "keep";
    st := core.fs_read("home/written.txt", size1, chunk, count);
    check("gap read",
      st = Ok and count >= 7 and core.substr_into(prefix, chunk, 0, 7) = 7
        and prefix[0] = byte(0) and prefix[3] = byte(0)
        and prefix[4] = byte(103) and prefix[5] = byte(97) and prefix[6] = byte(112));
    if ok then
    begin
      size := uint(0);
      st := core.fs_size("home/written.txt", size);
      check("gap size", st = Ok and size = size1 + uint(7));
    end;
  end;
end;

procedure check_errors();
var
  each_ok: bool;
begin
  if ok then
  begin
    each_ok := true;

    // Only the home tree is writable.
    written := -1;
    st := core.fs_write("base/child", uint(0), tail, written);
    if st <> Invalid or written <> -1 then
      each_ok := false;

    // A missing parent is NotFound, at offset 0 and elsewhere.
    written := -1;
    st := core.fs_write("home/missing/x", uint(0), tail, written);
    if st <> NotFound or written <> -1 then
      each_ok := false;

    // A nonzero offset does not create the file.
    written := -1;
    st := core.fs_write("home/never.txt", uint(4), tail, written);
    if st <> NotFound or written <> -1 then
      each_ok := false;

    // A directory is not a writable file.
    written := -1;
    st := core.fs_write("home", uint(0), tail, written);
    if st <> Invalid or written <> -1 then
      each_ok := false;

    check("error paths", each_ok);
  end;
end;

procedure main(args: const string[1024]);
begin
  ok := true;
  no_home := false;
  setup();
  first_write();
  check_size();
  read_back();
  append();
  read_append();
  write_gap();
  read_gap();
  check_errors();

  if ok then
    out.say("home_write: ok\n")
  else if not no_home then
    out.say("home_write: FAILED\n");
end;
