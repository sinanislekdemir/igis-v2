unit home_verify;

// M6.4 persistence proof, second half (spec 0019, V2).
//
//   run home_verify
//
// Boot the same `build/home.img` after `home_persist` wrote it and check the
// tree `home_persist` left behind: `home` lists `persist` as a directory,
// `home/persist/a.txt` is gone, and `home/persist/b.txt` has the exact size and
// contents. This program never writes (spec 0019, V3 runs `e2fsck -fn` on the
// image afterwards).
//
// Without a home disk the router falls back to the application image's `home`,
// which has no `persist` directory, so the first step fails and the verdict is
// `home_verify: FAIL`. That is the honest result: there is nothing to verify.

uses core, lib.out;

var
  dir_path: string[32];
  a_path: string[32];
  b_path: string[32];
  ok: bool;
  st: Status;
  got: int;
  size: uint;
  name: string[64];
  kind: uint;
  entry_size: uint;
  index: uint;
  found_persist: bool;
  found_a: bool;
  found_b: bool;
  b_size: uint;
  chunk: bytes[256];
  read_text: string[48];

// One line per step; the first failure names the step and stops the checks.
procedure check(label: const string[0]; passed: bool);
begin
  if ok then
  begin
    if passed then
    begin
      out.say("home_verify: ");
      out.say(label);
      out.say(" ok\n");
    end
    else
    begin
      ok := false;
      out.say("home_verify: FAIL ");
      out.say(label);
      out.say("\n");
    end;
  end;
end;

// Home's own listing must name `persist` as a directory (kind 1).
procedure check_home_listing();
begin
  if ok then
  begin
    found_persist := false;
    index := uint(0);
    st := core.fs_list("home", index, name, kind, entry_size);
    while st = Ok do
    begin
      if name = "persist" and kind = uint(1) then
        found_persist := true;
      index := index + uint(1);
      st := core.fs_list("home", index, name, kind, entry_size);
    end;
    check("directory", st = NotFound and found_persist);
  end;
end;

// The directory holds exactly the renamed file with its persisted size.
procedure check_directory_listing();
begin
  if ok then
  begin
    found_a := false;
    found_b := false;
    b_size := uint(0);
    index := uint(0);
    st := core.fs_list(dir_path, index, name, kind, entry_size);
    while st = Ok do
    begin
      if name = "a.txt" then
        found_a := true
      else if name = "b.txt" and kind = uint(0) then
      begin
        found_b := true;
        b_size := entry_size;
      end;
      index := index + uint(1);
      st := core.fs_list(dir_path, index, name, kind, entry_size);
    end;
    check("old gone", st = NotFound and not found_a);
    check("renamed file", st = NotFound and found_b and b_size = uint(34));
  end;
end;

procedure check_contents();
begin
  if ok then
  begin
    st := core.fs_size(a_path, size);
    check("source size", st = NotFound);

    size := uint(0);
    st := core.fs_size(b_path, size);
    check("size", st = Ok and size = uint(34));

    got := -1;
    chunk := "keep";
    st := core.fs_read(b_path, uint(0), chunk, got);
    read_text := "keep";
    check("contents",
      st = Ok and got = 34 and int(core.len(chunk)) = got
        and core.substr_into(read_text, chunk, 0, got) = got
        and read_text = "igis-persist-v1 line one\nline two\n");
  end;
end;

procedure main(args: const string[1024]);
begin
  ok := true;
  dir_path := "home/persist";
  a_path := "home/persist/a.txt";
  b_path := "home/persist/b.txt";
  check_home_listing();
  check_directory_listing();
  check_contents();

  if ok then
    out.say("home_verify: ok\n")
  else
    out.say("home_verify: FAILED\n");
end;
