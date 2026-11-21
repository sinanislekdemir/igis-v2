unit files;

// M3.1 proof app: the read-only application file API over the image tree
// (spec 0013, spec 0018).
//
//   run files
//
// Lists the image root (the six directories plus `mke2fs`'s `lost+found`),
// lists `base` and `system`, then reads `base/child` in fixed 256-byte chunks
// and checks the total against `core.fs_size`. It also proves the offset rules
// (a middle byte, the last byte, an offset at the end and past it), atomic
// failure (`dst` and `read` untouched), and the path rules: a missing
// component is `NotFound`, a directory is `Invalid` for a read, a file is
// `Invalid` for a listing, and `..`, an empty component, an over-long
// component and an over-deep path are `Invalid`. A destination smaller than
// 256 bytes is a compile error, so it cannot be exercised here; the compiler
// test covers it.

uses core, lib.out;

var
  ok: bool;
  st: Status;
  index: uint;
  count: uint;
  name: string[64];
  kind: uint;
  size: uint;
  each_ok: bool;
  found_base: bool;
  found_apps: bool;
  found_home: bool;
  found_lib: bool;
  found_system: bool;
  found_image: bool;
  found_lost: bool;
  found_child: bool;
  found_text: bool;
  found_probe: bool;
  found_readme: bool;
  child_size: uint;
  readme_size: uint;
  total: uint;
  off: uint;
  got: int;
  first: int;
  chunk: bytes[256];
  prefix: string[16];
  deep: string[64];
  long_name: string[80];
  long_stage: bytes[80];
  i: int;

// One line per step; the first failure names the step and stops the checks.
procedure check(label: const string[0]; passed: bool);
begin
  if ok then
  begin
    if passed then
    begin
      out.say("files: ");
      out.say(label);
      out.say(" ok\n");
    end
    else
    begin
      ok := false;
      out.say("files: FAIL ");
      out.say(label);
      out.say("\n");
    end;
  end;
end;

procedure reset_flags();
begin
  count := uint(0);
  each_ok := true;
  found_base := false;
  found_apps := false;
  found_home := false;
  found_lib := false;
  found_system := false;
  found_image := false;
  found_lost := false;
  found_child := false;
  found_text := false;
  found_probe := false;
  found_readme := false;
  index := uint(0);
end;

procedure list_root();
begin
  reset_flags();
  st := core.fs_list("", index, name, kind, size);
  while st = Ok do
  begin
    count := count + uint(1);
    if kind > uint(1) then
      each_ok := false;
    if kind = uint(1) and size = uint(0) then
      each_ok := false;
    if name = "base" and kind = uint(1) then
      found_base := true;
    if name = "apps" and kind = uint(1) then
      found_apps := true;
    if name = "home" and kind = uint(1) then
      found_home := true;
    if name = "lib" and kind = uint(1) then
      found_lib := true;
    if name = "system" and kind = uint(1) then
      found_system := true;
    if name = "image" and kind = uint(1) then
      found_image := true;
    if name = "lost+found" and kind = uint(1) then
      found_lost := true;
    index := index + uint(1);
    st := core.fs_list("", index, name, kind, size);
  end;
  // The kernel image has six directories and lost+found; the host tree has
  // the six directories only.
  check("root listing",
    each_ok and st = NotFound and found_base and found_apps and found_home
      and found_lib and found_system and found_image
      and ((found_lost and count = uint(7)) or (not found_lost and count = uint(6))));
  out.say("files: root entries=");
  out.say_int(int(count));
  out.say("\n");
end;

procedure list_dir(path: const string[0]);
begin
  reset_flags();
  st := core.fs_list(path, index, name, kind, size);
  while st = Ok do
  begin
    count := count + uint(1);
    if kind > uint(1) then
      each_ok := false;
    if kind = uint(0) and size = uint(0) then
      each_ok := false;
    if path = "base" and name = "child" then
    begin
      found_child := true;
      if kind = uint(0) then
        child_size := size
      else
        each_ok := false;
    end;
    if path = "base" and name = "text" then
      found_text := true;
    if path = "system" and name = "system_probe" then
      found_probe := true;
    index := index + uint(1);
    st := core.fs_list(path, index, name, kind, size);
  end;
  if path = "base" then
    check("base listing", each_ok and st = NotFound and count > uint(0) and found_child and found_text)
  else
    check("system listing", each_ok and st = NotFound and count > uint(0) and found_probe);
end;

procedure read_child();
begin
  total := uint(0);
  off := uint(0);
  each_ok := true;
  while off < child_size and each_ok do
  begin
    st := core.fs_read("base/child", off, chunk, got);
    if st <> Ok or got < 0 then
      each_ok := false
    else if got = 0 then
      each_ok := false
    else
    begin
      if int(core.len(chunk)) <> got then
        each_ok := false;
      if off = uint(0) then
      begin
        first := core.substr_into(prefix, chunk, 0, 11);
        if first <> 11 or prefix <> "unit child;" then
          each_ok := false;
      end;
      total := total + uint(got);
      off := off + uint(got);
    end;
  end;
  check("chunked read", each_ok and total = child_size and off = child_size);
  out.say("files: read ");
  out.say_int(int(total));
  out.say("/");
  out.say_int(int(child_size));
  out.say("\n");
end;

procedure read_home();
begin
  // `home` is the home mount when a disk is attached and the application
  // image's `home` otherwise (spec 0019, A4); either way the router resolves
  // `home/readme.txt` and the read looks identical (spec 0013).
  reset_flags();
  st := core.fs_list("home", index, name, kind, size);
  while st = Ok do
  begin
    count := count + uint(1);
    if name = "readme.txt" then
    begin
      found_readme := kind = uint(0) and size > uint(0);
      readme_size := size;
    end;
    index := index + uint(1);
    st := core.fs_list("home", index, name, kind, size);
  end;
  check("home listing", st = NotFound and count > uint(0) and found_readme);

  total := uint(0);
  off := uint(0);
  each_ok := true;
  while off < readme_size and each_ok do
  begin
    st := core.fs_read("home/readme.txt", off, chunk, got);
    if st <> Ok or got <= 0 then
      each_ok := false
    else
    begin
      if off = uint(0) then
      begin
        first := core.substr_into(prefix, chunk, 0, 12);
        if first <> 12 or prefix <> "igis-v2 home" then
          each_ok := false;
      end;
      total := total + uint(got);
      off := off + uint(got);
    end;
  end;
  check("home read", each_ok and total = readme_size and readme_size > uint(0));
  out.say("files: home read ");
  out.say_int(int(total));
  out.say("/");
  out.say_int(int(readme_size));
  out.say("\n");
end;

procedure check_offsets();
begin
  each_ok := true;
  // Offset 1 starts on the second byte of "unit child;".
  got := -1;
  st := core.fs_read("base/child", uint(1), chunk, got);
  if st <> Ok or got <= 0 then
    each_ok := false
  else if chunk[0] <> byte(110) then
    each_ok := false;

  // The last byte is the file's trailing newline and returns exactly one byte.
  got := -1;
  st := core.fs_read("base/child", child_size - uint(1), chunk, got);
  if st <> Ok or got <> 1 or int(core.len(chunk)) <> 1 then
    each_ok := false
  else if chunk[0] <> byte(10) then
    each_ok := false;

  // At the end and past it the read is an Ok with zero bytes, not an error.
  got := -1;
  st := core.fs_read("base/child", child_size, chunk, got);
  if st <> Ok or got <> 0 or int(core.len(chunk)) <> 0 then
    each_ok := false;
  got := -1;
  st := core.fs_read("base/child", child_size + uint(256), chunk, got);
  if st <> Ok or got <> 0 then
    each_ok := false;
  check("offset rules", each_ok);
end;

procedure check_errors();
begin
  each_ok := true;

  // A missing component leaves the destination and the count untouched.
  chunk := "keep";
  got := -1;
  st := core.fs_read("base/no_such_app", uint(0), chunk, got);
  if st <> NotFound or chunk <> "keep" or got <> -1 then
    each_ok := false;

  // A directory is not a readable file; a file is not a listable directory.
  chunk := "keep";
  got := -1;
  st := core.fs_read("base", uint(0), chunk, got);
  if st <> Invalid or chunk <> "keep" or got <> -1 then
    each_ok := false;
  st := core.fs_list("base/child", uint(0), name, kind, size);
  if st <> Invalid then
    each_ok := false;

  // A missing directory is NotFound; a file path used as one is Invalid.
  st := core.fs_list("no_such_dir", uint(0), name, kind, size);
  if st <> NotFound then
    each_ok := false;

  // Syntax: `..`, an empty component and an over-deep path are Invalid.
  st := core.fs_read("../base/child", uint(0), chunk, got);
  if st <> Invalid then
    each_ok := false;
  st := core.fs_read("base//child", uint(0), chunk, got);
  if st <> Invalid then
    each_ok := false;
  deep := "base/a/b/c/d/e/f/g/h";
  st := core.fs_read(deep, uint(0), chunk, got);
  if st <> Invalid then
    each_ok := false;
  st := core.fs_list("..", uint(0), name, kind, size);
  if st <> Invalid then
    each_ok := false;

  // A component longer than 64 bytes is Invalid.
  i := 0;
  while i < 65 do
  begin
    long_stage[i] := byte(120);
    i := i + 1;
  end;
  got := core.substr_into(long_name, long_stage, 0, 65);
  if got <> 65 then
    each_ok := false;
  st := core.fs_read(long_name, uint(0), chunk, got);
  if st <> Invalid then
    each_ok := false;

  // Size: a missing name is NotFound, a directory is Invalid.
  st := core.fs_size("base/no_such_app", size);
  if st <> NotFound then
    each_ok := false;
  st := core.fs_size("base", size);
  if st <> Invalid then
    each_ok := false;
  check("error paths", each_ok);
end;

procedure main(args: const string[1024]);
begin
  ok := true;

  list_root();
  check("root dot", core.fs_list(".", uint(0), name, kind, size) = Ok);
  list_dir("base");
  list_dir("system");
  check("size agrees",
    core.fs_size("base/child", size) = Ok and size = child_size
      and child_size > uint(0));
  read_child();
  check_offsets();
  read_home();
  check_errors();

  if ok then
    out.say("files: ok\n")
  else
    out.say("files: FAILED\n");
end;
