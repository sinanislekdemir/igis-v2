unit home_persist;

// M6.4 persistence proof, first half (spec 0019, V2).
//
//   run home_persist
//
// Creates `home/persist` (or accepts the directory a previous boot left),
// writes `home/persist/a.txt` with a fixed payload and renames it to
// `home/persist/b.txt`. The write is synchronous (spec 0019, H4), so a
// power-cycle after this program exits leaves the file in `build/home.img`;
// `home_verify` in the next boot checks it.
//
// The app is idempotent: a leftover `a.txt` from an interrupted run is removed
// first and `rename` replaces an existing `b.txt` (spec 0019, W6), so running
// it again on the same image produces the same tree and the same payload.
//
// Without a home disk `core.fs_mkdir` is `Invalid` (the mount is read-only)
// and the verdict is `home_persist: no writable home`; no other write is
// attempted. On the host (`langhost`) the home rule is implemented over a
// temporary directory (spec 0019, A6), so the same checks print the same lines.

uses core, lib.out;

var
  dir_path: string[32];
  a_path: string[32];
  b_path: string[32];
  ok: bool;
  no_home: bool;
  st: Status;
  probe: Status;
  written: int;
  count: int;
  got: int;
  size: uint;
  name: string[64];
  kind: uint;
  entry_size: uint;
  payload: bytes[64];
  chunk: bytes[256];
  read_text: string[48];

// One line per step; the first failure names the step and stops the checks.
procedure check(label: const string[0]; passed: bool);
begin
  if ok then
  begin
    if passed then
    begin
      out.say("home_persist: ");
      out.say(label);
      out.say(" ok\n");
    end
    else
    begin
      ok := false;
      out.say("home_persist: FAIL ");
      out.say(label);
      out.say("\n");
    end;
  end;
end;

procedure setup();
begin
  // 34 bytes: the marker, two lines and their newlines (spec 0019, V2).
  count := core.substr_into(
    payload, "igis-persist-v1 line one\nline two\n", 0, 34);
  if count <> 34 then
    ok := false;
end;

// Creates the directory, or accepts one a previous persistence cycle left.
// `fs_mkdir` reports `Invalid` both for "already exists" and for "no writable
// home", so an existing directory is distinguished by listing it.
procedure ensure_directory();
begin
  if ok then
  begin
    st := core.fs_mkdir(dir_path);
    if st = Ok then
      check("dir", true)
    else if st = Invalid then
    begin
      probe := core.fs_list(dir_path, uint(0), name, kind, entry_size);
      if probe = Ok then
        check("dir", true)
      else if probe = NotFound then
      begin
        ok := false;
        no_home := true;
        out.say("home_persist: no writable home\n");
      end
      else
        check("dir", false);
    end
    else
      check("dir", false);
  end;
end;

procedure write_file();
begin
  if ok then
  begin
    // Remove a leftover source so a longer file from an interrupted run cannot
    // survive the rename; an absent name is `NotFound` and is ignored.
    st := core.fs_remove(a_path);
    written := -1;
    st := core.fs_write(a_path, uint(0), payload, written);
    check("write", st = Ok and written = 34);

    size := uint(0);
    st := core.fs_size(a_path, size);
    check("write size", st = Ok and size = uint(34));
  end;
end;

procedure rename_file();
begin
  if ok then
  begin
    st := core.fs_rename(a_path, b_path);
    check("rename", st = Ok);

    st := core.fs_size(a_path, size);
    check("source gone", st = NotFound);

    size := uint(0);
    st := core.fs_size(b_path, size);
    check("target size", st = Ok and size = uint(34));

    got := -1;
    chunk := "keep";
    st := core.fs_read(b_path, uint(0), chunk, got);
    read_text := "keep";
    check("target contents",
      st = Ok and got = 34 and int(core.len(chunk)) = got
        and core.substr_into(read_text, chunk, 0, got) = got
        and read_text = "igis-persist-v1 line one\nline two\n");
  end;
end;

procedure main(args: const string[1024]);
begin
  ok := true;
  no_home := false;
  dir_path := "home/persist";
  a_path := "home/persist/a.txt";
  b_path := "home/persist/b.txt";
  setup();
  ensure_directory();
  write_file();
  rename_file();

  if ok then
    out.say("home_persist: ok\n")
  else if not no_home then
    out.say("home_persist: FAILED\n");
end;
