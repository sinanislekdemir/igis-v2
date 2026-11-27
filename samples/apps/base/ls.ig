unit ls;

// `ls [path]` — list a directory or a regular file.
//
// It is an ordinary `base/` command, not a shell builtin. Every bare command
// receives `args = "<cwd>\n<rest>"`; the explicit form `run ls /base` passes
// the raw tail with the image root as cwd. The target is the resolved
// `join(cwd, rest)`, and an empty rest lists the cwd.
//
// Output is one line per entry (`d <name>`, `f <name> <size>` or
// `o <name> <size>`) and a file target prints one `f <basename> <size>` line.
// See `lib.list_dir` for the listing rules.

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
    out.say("ls: bad arguments\n");
    return;
  end;
  st := path.join(cwd, rest, target);
  if st <> Ok then
  begin
    if st = CapacityExceeded then
      out.say("ls: path too long\n")
    else
      out.say("ls: invalid path\n");
    return;
  end;
  st := list_dir.show(target, "ls");
end;
