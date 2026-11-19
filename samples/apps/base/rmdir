unit rmdir;

// `rmdir path` — remove an empty directory under `home`.
//
// It is an ordinary application. The target is `join(cwd, rest)`, so the
// explicit form `run rmdir /home/nctest` removes that directory.
//
// A directory is detected before the removal: `core.fs_list` succeeding at
// index 0 is a non-empty directory, and `core.fs_list` returning `NotFound`
// with `core.fs_size` returning `Invalid` is an empty one. A regular file
// reports `rmdir: not a directory`; a missing name reports
// `rmdir: no such file or directory`. `core.fs_remove` then refuses a
// non-empty directory (`Invalid`), which prints `rmdir: directory not empty`.
// A target outside `home` is `rmdir: read-only`.

uses core, stdlib, lib.path, lib.out, lib.cli;

procedure usage();
begin
  out.say("usage: rmdir <directory>\n");
end;

procedure main(args: const string[1024]);
var
  cwd: string[256];
  rest: string[256];
  target: string[256];
  shown: string[256];
  probe_name: string[64];
  probe_kind: uint;
  probe_size: uint;
  size: uint;
  st: Status;
  side: Status;
  shown_st: Status;
begin
  st := path.split_cwd(args, cwd, rest);
  if st <> Ok then
  begin
    out.say("rmdir: bad arguments\n");
    return;
  end;
  st := path.join(cwd, rest, target);
  if st <> Ok then
  begin
    if st = CapacityExceeded then
      out.say("rmdir: path too long\n")
    else
      out.say("rmdir: invalid path\n");
    return;
  end;
  if int(core.len(target)) = 0 then
  begin
    usage();
    return;
  end;

  st := core.fs_list(target, uint(0), probe_name, probe_kind, probe_size);
  if st = Ok then
  begin
    // A non-empty directory; the remove below refuses it as `Invalid`.
  end
  else if st = Invalid or st = NotFound then
  begin
    side := core.fs_size(target, size);
    if side = Ok then
    begin
      out.say("rmdir: not a directory\n");
      return;
    end;
    if side = NotFound then
    begin
      out.say("rmdir: no such file or directory\n");
      return;
    end;
    if side <> Invalid then
    begin
      cli.fail("rmdir", side);
      return;
    end;
  end
  else
  begin
    cli.fail("rmdir", st);
    return;
  end;

  st := core.fs_remove(target);
  if st = Ok then
  begin
    out.say("rmdir: removed ");
    shown_st := path.display(target, shown);
    if shown_st = Ok then
      out.say(shown)
    else
      out.say(target);
    out.say("\n");
  end
  else if st = Invalid then
  begin
    if cli.is_home(target) then
      out.say("rmdir: directory not empty\n")
    else
      out.say("rmdir: read-only\n");
  end
  else
    cli.fail("rmdir", st);
end;
