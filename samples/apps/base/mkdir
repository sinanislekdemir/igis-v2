unit mkdir;

// `mkdir path` — create one directory under the writable `home` tree.
//
// It is an ordinary application. The target is `join(cwd, rest)`; with an
// empty rest the cwd itself is the target, so the explicit form
// `run mkdir /home/nctest` creates `home/nctest`.
//
// The file API only writes below `home`; a target elsewhere is refused by
// `core.fs_mkdir` with `Invalid` and reported as `mkdir: read-only`. An empty
// resolved target (the root) is a usage error.

uses core, stdlib, lib.path, lib.out, lib.cli;

procedure usage();
begin
  out.say("usage: mkdir <directory>\n");
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
    out.say("mkdir: bad arguments\n");
    return;
  end;
  st := path.join(cwd, rest, target);
  if st <> Ok then
  begin
    if st = CapacityExceeded then
      out.say("mkdir: path too long\n")
    else
      out.say("mkdir: invalid path\n");
    return;
  end;
  if int(core.len(target)) = 0 then
  begin
    usage();
    return;
  end;

  st := core.fs_mkdir(target);
  if st = Ok then
  begin
    out.say("mkdir: created ");
    shown_st := path.display(target, shown);
    if shown_st = Ok then
      out.say(shown)
    else
      out.say(target);
    out.say("\n");
  end
  else if st = Invalid and not cli.is_home(target) then
    out.say("mkdir: read-only\n")
  else
    cli.fail("mkdir", st);
end;
