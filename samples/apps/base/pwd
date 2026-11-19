unit pwd;

// `pwd` — print the current directory's display path.
//
// It is an ordinary application and needs no filesystem call: the shell passes
// the display path as the first token of `args`, and `pwd` prints it exactly
// (`/`, `/base`, `/home/sub`, ...).

uses core, stdlib, lib.path, lib.out;

procedure main(args: const string[1024]);
var
  cwd: string[256];
  rest: string[256];
  st: Status;
begin
  st := path.split_cwd(args, cwd, rest);
  if st = Ok then
  begin
    out.say(cwd);
    out.say("\n");
  end
  else
    out.say("pwd: bad arguments\n");
end;
