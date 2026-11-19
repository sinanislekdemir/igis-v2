unit rm;

// `rm path` — remove a regular file or an empty directory under `home`.
//
// It is an ordinary application. The target is `join(cwd, rest)`, so the
// explicit form `run rm /home/note.txt` removes that file.
//
// `core.fs_remove` refuses a non-empty directory (`rm: failed Invalid`), a
// missing name is `rm: failed NotFound` and a target outside `home` is
// `rm: read-only`. An empty resolved target (the root) is a usage error.

uses core, stdlib, lib.path, lib.out, lib.cli;

procedure usage();
begin
  out.say("usage: rm <path>\n");
end;

procedure main(args: const string[1024]);
var
  cwd: string[256];
  rest: string[256];
  target: string[256];
  shown: string[256];
  st: Status;
  shown_st: Status;
begin
  st := path.split_cwd(args, cwd, rest);
  if st <> Ok then
  begin
    out.say("rm: bad arguments\n");
    return;
  end;
  st := path.join(cwd, rest, target);
  if st <> Ok then
  begin
    if st = CapacityExceeded then
      out.say("rm: path too long\n")
    else
      out.say("rm: invalid path\n");
    return;
  end;
  if int(core.len(target)) = 0 then
  begin
    usage();
    return;
  end;

  st := core.fs_remove(target);
  if st = Ok then
  begin
    out.say("rm: removed ");
    shown_st := path.display(target, shown);
    if shown_st = Ok then
      out.say(shown)
    else
      out.say(target);
    out.say("\n");
  end
  else if st = Invalid and not cli.is_home(target) then
    out.say("rm: read-only\n")
  else
    cli.fail("rm", st);
end;
