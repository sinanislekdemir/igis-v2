unit home_ops;

// M6.3 proof app: directories, removal and rename on the writable home
// filesystem (spec 0019, W3–W6).
//
//   run home_ops
//
// Creates `home/opsdir`, writes `home/opsdir/a.txt`, renames it to `b.txt`,
// lists the directory to prove the entry moved, writes a second file, removes
// it, then removes the remaining file and the directory and checks that the
// tree is gone. Every step re-reads through the file API instead of trusting
// the call that changed it.
//
// The app is idempotent: it first removes any leftovers from an interrupted run
// (the removes of absent names are ignored), so a persistent image can run it
// repeatedly.
//
// Without a home disk `core.fs_mkdir` is `Invalid` (the mount is read-only) and
// the verdict is `home_ops: no writable home`; no other call is attempted. The
// host (`langhost`) implements the same home rule over a temporary directory
// (spec 0019, A6), so it prints the same `home_ops: ok` lines.

uses core, lib.out;

var
  dir_path: string[32];
  a_path: string[32];
  b_path: string[32];
  c_path: string[32];
  ok: bool;
  no_home: bool;
  st: Status;
  written: int;
  count: int;
  size: uint;
  data: bytes[32];
  chunk: bytes[256];
  read_text: string[16];
  found_a: bool;
  found_b: bool;
  found_c: bool;

// One line per step; the first failure names the step and stops the checks.
procedure check(label: const string[0]; passed: bool);
begin
  if ok then
  begin
    if passed then
    begin
      out.say("home_ops: ");
      out.say(label);
      out.say(" ok\n");
    end
    else
    begin
      ok := false;
      out.say("home_ops: FAIL ");
      out.say(label);
      out.say("\n");
    end;
  end;
end;

// Removes a name and ignores every error: used only to reach a known state.
procedure discard(path: const string[0]);
begin
  st := core.fs_remove(path);
end;

procedure cleanup();
begin
  discard(c_path);
  discard(b_path);
  discard(a_path);
  discard(dir_path);
end;

procedure create_directory();
begin
  if ok then
  begin
    st := core.fs_mkdir(dir_path);
    if st = Invalid then
    begin
      // No home disk (or a read-only mount): the primitive exists but refuses.
      ok := false;
      no_home := true;
      out.say("home_ops: no writable home\n");
    end
    else
      check("mkdir", st = Ok);
  end;
end;

procedure write_first();
begin
  if ok then
  begin
    count := core.substr_into(data, "alpha\n", 0, 6);
    written := -1;
    st := core.fs_write(a_path, uint(0), data, written);
    check("write", count = 6 and st = Ok and written = 6);
  end;
end;

procedure rename_first();
var
  after: Status;
begin
  if ok then
  begin
    after := core.fs_size(a_path, size);
    check("source before rename", after = Ok and size = uint(6));
    st := core.fs_rename(a_path, b_path);
    check("rename", st = Ok);
    after := core.fs_size(a_path, size);
    check("source gone", after = NotFound);
    after := core.fs_size(b_path, size);
    check("target size", after = Ok and size = uint(6));

    count := -1;
    chunk := "keep";
    st := core.fs_read(b_path, uint(0), chunk, count);
    check("target contents",
      st = Ok and count = 6
        and core.substr_into(read_text, chunk, 0, 6) = 6 and read_text = "alpha\n");
  end;
end;

// Lists the directory and records which of the three names are present.
procedure scan();
var
  index: uint;
  name: string[64];
  kind: uint;
  entry_size: uint;
  each: Status;
begin
  found_a := false;
  found_b := false;
  found_c := false;
  index := uint(0);
  each := core.fs_list(dir_path, index, name, kind, entry_size);
  while each = Ok do
  begin
    if name = "a.txt" then
      found_a := true
    else if name = "b.txt" then
      found_b := true
    else if name = "c.txt" then
      found_c := true;
    index := index + uint(1);
    each := core.fs_list(dir_path, index, name, kind, entry_size);
  end;
end;

procedure check_listing();
begin
  if ok then
  begin
    scan();
    check("listing after rename", found_b and not found_a and not found_c);
  end;
end;

procedure second_file();
begin
  if ok then
  begin
    count := core.substr_into(data, "beta\n", 0, 5);
    written := -1;
    st := core.fs_write(c_path, uint(0), data, written);
    check("second write", count = 5 and st = Ok and written = 5);
    scan();
    check("listing with second", found_b and found_c and not found_a);

    st := core.fs_remove(c_path);
    check("remove file", st = Ok);
    st := core.fs_size(c_path, size);
    check("removed file gone", st = NotFound);
  end;
end;

procedure remove_tree();
var
  name: string[64];
  kind: uint;
  entry_size: uint;
begin
  if ok then
  begin
    // A non-empty directory is refused before its entry is freed.
    st := core.fs_remove(dir_path);
    check("non-empty refused", st = Invalid);
    st := core.fs_remove(b_path);
    check("remove renamed file", st = Ok);
    st := core.fs_remove(dir_path);
    check("remove directory", st = Ok);
    st := core.fs_size(b_path, size);
    check("file gone", st = NotFound);
    st := core.fs_list(dir_path, uint(0), name, kind, entry_size);
    check("directory gone", st = NotFound);
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
    st := core.fs_mkdir("base/newdir");
    if st <> Invalid then
      each_ok := false;
    st := core.fs_remove("base/child");
    if st <> Invalid then
      each_ok := false;
    st := core.fs_rename("base/child", "home/child");
    if st <> Invalid then
      each_ok := false;
    // Missing names are NotFound, not Invalid.
    st := core.fs_remove("home/no_such_entry");
    if st <> NotFound then
      each_ok := false;
    st := core.fs_rename("home/no_such_entry", "home/x");
    if st <> NotFound then
      each_ok := false;
    // A parent that does not exist.
    st := core.fs_mkdir("home/no_such_parent/child");
    if st <> NotFound then
      each_ok := false;
    check("error paths", each_ok);
  end;
end;

procedure main(args: const string[1024]);
begin
  ok := true;
  no_home := false;
  dir_path := "home/opsdir";
  a_path := "home/opsdir/a.txt";
  b_path := "home/opsdir/b.txt";
  c_path := "home/opsdir/c.txt";
  cleanup();
  create_directory();
  write_first();
  rename_first();
  check_listing();
  second_file();
  remove_tree();
  errors();

  if ok then
    out.say("home_ops: ok\n")
  else if not no_home then
    out.say("home_ops: FAILED\n");
end;
