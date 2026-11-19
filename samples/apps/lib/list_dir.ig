unit list_dir;

// Shared listing body for the `ls` and `dir` commands.
//
// Ordinary `/lib` module source: no `main`, no top-level `var` state and no
// stream endpoint. `show` reads a directory entry at a time with
// `core.fs_list` and prints one line per entry:
//
//   d <name>              kind 1 (directory)
//   f <name> <size>       kind 0 (regular file)
//   o <name> <size>       kind 2 (device, socket, symlink)
//
// A file target is detected through `core.fs_size` (a regular file lists with
// `Invalid` at index 0) and prints one `f <basename> <size>` line. A directory
// with no entries returns `NotFound` at index 0 and `Invalid` from `fs_size`,
// so it prints nothing and succeeds. A missing path prints
// `<cmd>: no such file or directory`.

uses core, stdlib, lib.out, lib.path, lib.cli;

function show(target: const string[0]; cmd: const string[0]): Status;
var
  index: uint;
  name: string[64];
  kind: uint;
  size: uint;
  st: Status;
  probe: Status;
  base: string[64];
  copied: Status;
begin
  index := uint(0);
  st := core.fs_list(target, index, name, kind, size);
  if st = Ok then
  begin
    while st = Ok do
    begin
      if kind = uint(1) then
      begin
        out.say("d ");
        out.say(name);
        out.say("\n");
      end
      else if kind = uint(0) then
      begin
        out.say("f ");
        out.say(name);
        out.say(" ");
        out.say_int(int(size));
        out.say("\n");
      end
      else
      begin
        out.say("o ");
        out.say(name);
        out.say(" ");
        out.say_int(int(size));
        out.say("\n");
      end;
      index := index + uint(1);
      st := core.fs_list(target, index, name, kind, size);
    end;
    if st <> NotFound then
    begin
      cli.fail(cmd, st);
      return st;
    end;
    return Ok;
  end;

  if st <> NotFound and st <> Invalid then
  begin
    cli.fail(cmd, st);
    return st;
  end;

  // index 0 was missing or the path is a regular file; `fs_size` separates the
  // two. A directory reports `Invalid` here and is a valid empty listing.
  probe := core.fs_size(target, size);
  if probe = Ok then
  begin
    copied := path.basename(target, base);
    out.say("f ");
    out.say(base);
    out.say(" ");
    out.say_int(int(size));
    out.say("\n");
    return Ok;
  end;
  if probe = Invalid then
    return Ok;
  if probe = NotFound then
  begin
    out.say(cmd);
    out.say(": no such file or directory\n");
    return NotFound;
  end;
  cli.fail(cmd, probe);
  return probe;
end;
