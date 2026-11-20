unit dir;

// `dir [path]` — the same listing as `ls`, under a second name.
//
// It is an ordinary application and shares every rule with `ls` through
// `lib.list_dir`; only the error prefix differs (`dir: ...`).

uses core, stdlib, lib.path, lib.out, lib.list_dir;

procedure main(args: const string[1024]);
var
  cwd: string[256];
  rest: string[256];
  target: string[256];
  st: Status;
begin
  st := path.split_cwd(args, cwd, rest);
  if st <> Ok then
  begin
    out.say("dir: bad arguments\n");
    return;
  end;
  st := path.join(cwd, rest, target);
  if st <> Ok then
  begin
    if st = CapacityExceeded then
      out.say("dir: path too long\n")
    else
      out.say("dir: invalid path\n");
    return;
  end;
  st := list_dir.show(target, "dir");
end;
