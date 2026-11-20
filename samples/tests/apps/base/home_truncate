unit home_truncate;

// M7.1 proof app: truncate on the writable home filesystem (spec 0019, A7;
// ADR 0041).
//
//   run home_truncate
//
// Writes 3000 bytes of a deterministic pattern (`i mod 251`) to
// `home/trunc.txt`, truncates to 1000 and checks the size and every remaining
// byte, extends to 4000 and checks the old 3000 bytes plus 1000 zero bytes,
// truncates to 0 and checks the empty file, repeats the 0 truncate as an
// idempotent no-op, and finally checks the three error classes (outside
// `home`, a missing name and a directory). Every step re-reads through the file
// API instead of trusting the call that changed it.
//
// Without a home disk `core.fs_write` is `Invalid` (the mount is read-only) and
// the verdict is `home_truncate: no writable home`; no other call is attempted.
// The host (`langhost`) implements the same home rule over a temporary
// directory (spec 0019, A6), so it prints the same `home_truncate: ok` lines.

uses core, lib.out;

const
  FullSize = 3000;
  ShrunkSize = 1000;
  GrownSize = 4000;
  PatternModulus = 251;

var
  path: string[32];
  ok: bool;
  no_home: bool;
  st: Status;
  written: int;
  got: int;
  count: int;
  size: uint;
  data: bytes[256];
  read_chunk: bytes[256];

// One line per step; the first failure names the step and stops the checks.
procedure check(label: const string[0]; passed: bool);
begin
  if ok then
  begin
    if passed then
    begin
      out.say("home_truncate: ");
      out.say(label);
      out.say(" ok\n");
    end
    else
    begin
      ok := false;
      out.say("home_truncate: FAIL ");
      out.say(label);
      out.say("\n");
    end;
  end;
end;

// Fills `data` with `len` pattern bytes belonging at absolute offset `base`.
procedure fill_chunk(base: int; len: int);
var
  i: int;
begin
  data := "";
  i := 0;
  while i < len do
  begin
    data[i] := byte((base + i) mod PatternModulus);
    i := i + 1;
  end;
end;

// Writes `total` pattern bytes from offset 0 in 256-byte `fs_write` calls.
procedure write_pattern(total: int);
var
  offset: int;
  len: int;
  all_ok: bool;
begin
  if ok then
  begin
    all_ok := true;
    offset := 0;
    while offset < total do
    begin
      len := total - offset;
      if len > 256 then
        len := 256;
      fill_chunk(offset, len);
      written := -1;
      st := core.fs_write(path, uint(offset), data, written);
      if st <> Ok or written <> len then
        all_ok := false;
      offset := offset + len;
    end;
    check("write", all_ok);
  end;
end;

// Checks `total` bytes from `start`: `zero` selects the zero-fill rule, else
// the pattern rule.
procedure verify(start: int; total: int; zero: bool);
var
  offset: int;
  len: int;
  i: int;
  all_ok: bool;
  expect: byte;
begin
  if ok then
  begin
    all_ok := true;
    offset := 0;
    while offset < total do
    begin
      len := total - offset;
      if len > 256 then
        len := 256;
      read_chunk := "";
      got := -1;
      st := core.fs_read(path, uint(start + offset), read_chunk, got);
      if st <> Ok or got < len then
        all_ok := false;
      i := 0;
      while i < len do
      begin
        if zero then
          expect := byte(0)
        else
          expect := byte((start + offset + i) mod PatternModulus);
        if read_chunk[i] <> expect then
          all_ok := false;
        i := i + 1;
      end;
      offset := offset + len;
    end;
    if zero then
      check("zero fill", all_ok)
    else
      check("contents", all_ok);
  end;
end;

procedure check_size(label: const string[0]; expected: uint);
begin
  if ok then
  begin
    size := uint(0);
    st := core.fs_size(path, size);
    check(label, st = Ok and size = expected);
  end;
end;

procedure truncate_to(label: const string[0]; new_size: uint);
begin
  if ok then
  begin
    st := core.fs_truncate(path, new_size);
    check(label, st = Ok);
  end;
end;

procedure shrink();
begin
  if ok then
  begin
    truncate_to("shrink", uint(ShrunkSize));
    check_size("shrink size", uint(ShrunkSize));
    verify(0, ShrunkSize, false);
  end;
end;

procedure extend();
begin
  if ok then
  begin
    truncate_to("extend", uint(GrownSize));
    check_size("extend size", uint(GrownSize));
    verify(0, ShrunkSize, false);
    verify(ShrunkSize, GrownSize - ShrunkSize, true);
  end;
end;

procedure empty();
begin
  if ok then
  begin
    truncate_to("truncate to zero", uint(0));
    check_size("zero size", uint(0));
    read_chunk := "";
    got := -1;
    st := core.fs_read(path, uint(0), read_chunk, got);
    check("empty read", st = Ok and got = 0);
    // The current size is an `Ok` no-op.
    truncate_to("idempotent", uint(0));
    check_size("still zero", uint(0));
  end;
end;

procedure errors();
var
  each_ok: bool;
begin
  if ok then
  begin
    each_ok := true;
    // Only the home tree is writable.
    st := core.fs_truncate("base/child", uint(0));
    if st <> Invalid then
      each_ok := false;
    // A missing name is NotFound, not Invalid.
    st := core.fs_truncate("home/no_such_file", uint(0));
    if st <> NotFound then
      each_ok := false;
    // A directory is not truncatable.
    st := core.fs_truncate("home", uint(0));
    if st <> Invalid then
      each_ok := false;
    check("error paths", each_ok);
  end;
end;

procedure main(args: const string[1024]);
begin
  path := "home/trunc.txt";
  ok := true;
  no_home := false;
  // A later run starts from a file the previous run emptied; the first write at
  // offset 0 creates it when missing.
  fill_chunk(0, 16);
  written := -1;
  st := core.fs_write(path, uint(0), data, written);
  if st = Invalid then
  begin
    // No home disk (or a read-only mount): the primitive exists but refuses.
    ok := false;
    no_home := true;
    out.say("home_truncate: no writable home\n");
  end
  else if st <> Ok or written <> 16 then
    check("write", false)
  else
  begin
    write_pattern(FullSize);
    check_size("write size", uint(FullSize));
    verify(0, FullSize, false);
    shrink();
    extend();
    empty();
    errors();
    if ok then
      out.say("home_truncate: ok\n")
    else
      out.say("home_truncate: FAILED\n");
  end;
end;
