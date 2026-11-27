unit touch;

// `touch path` — create an empty regular file under the writable `home` tree.
//
// It is an ordinary application. The target is `join(cwd, rest)`, so a bare
// `touch note.txt` at `/home` creates `home/note.txt`, and the explicit form
// `run touch /home/note.txt` creates the same file.
//
// There is no timestamp API, so an existing regular file is a successful
// no-op: touch never truncates or rewrites it. A directory or another
// non-regular kind is refused, a missing parent is reported as a missing
// directory, and a target outside `home` is read-only. Creation is a one-byte
// write followed by `core.fs_truncate(target, 0)`, the same pair the browser's
// copy path uses for a zero-byte destination.

uses core, stdlib, lib.path, lib.out, lib.cli;

const
  // One staging byte: `fs_write` has no zero-length literal, and truncate then
  // turns the new one-byte file into the empty file touch promises.
  SeedCapacity = 1;

// Formats and prints a path for a success line (`/home/note.txt`).
procedure say_path(target: const string[0]);
var
  shown: string[256];
  st: Status;
begin
  st := path.display(target, shown);
  if st = Ok then
    out.say(shown)
  else
    out.say(target);
end;

procedure usage();
begin
  out.say("usage: touch <file>\n");
end;

procedure main(args: const string[1024]);
var
  cwd: string[256];
  rest: string[256];
  target: string[256];
  size: uint;
  seed: bytes[SeedCapacity];
  written: int;
  st: Status;
begin
  st := path.split_cwd(args, cwd, rest);
  if st <> Ok then
  begin
    out.say("touch: bad arguments\n");
    return;
  end;
  st := path.join(cwd, rest, target);
  if st <> Ok then
  begin
    if st = CapacityExceeded then
      out.say("touch: path too long\n")
    else
      out.say("touch: invalid path\n");
    return;
  end;
  if int(core.len(target)) = 0 then
  begin
    usage();
    return;
  end;
  if not cli.is_home(target) then
  begin
    out.say("touch: read-only\n");
    return;
  end;

  st := core.fs_size(target, size);
  if st = Ok then
  begin
    out.say("touch: exists ");
    say_path(target);
    out.say("\n");
    return;
  end;
  if st = Invalid then
  begin
    out.say("touch: not a regular file\n");
    return;
  end;
  if st <> NotFound then
  begin
    cli.fail("touch", st);
    return;
  end;

  seed[0] := byte(0);
  written := -1;
  st := core.fs_write(target, uint(0), seed, written);
  if st = Ok and written <> 1 then
    st := IoError;
  if st = Ok then
    st := core.fs_truncate(target, uint(0));
  if st <> Ok then
  begin
    core.fs_remove(target);
    if st = NotFound then
      out.say("touch: no such directory\n")
    else if st = CapacityExceeded then
      out.say("touch: no space left\n")
    else
      cli.fail("touch", st);
    return;
  end;

  out.say("touch: created ");
  say_path(target);
  out.say("\n");
end;
