unit cat;

// `cat [path]` — copy a regular file to the console.
//
// It is an ordinary application. The target is `join(cwd, rest)`; with an
// empty rest the cwd itself is the target, so the explicit form
// `run cat /base/child` reads that file. The file is streamed with
// `core.fs_size` plus 256-byte `core.fs_read` chunks, so its size is not
// limited by an application buffer.
//
// Errors name the command: `cat: no such file or directory`,
// `cat: not a regular file` (a directory or another non-regular kind) or
// `cat: failed <Status>`.

uses core, stdlib, lib.path, lib.out, lib.file_view;

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
    out.say("cat: bad arguments\n");
    return;
  end;
  st := path.join(cwd, rest, target);
  if st <> Ok then
  begin
    if st = CapacityExceeded then
      out.say("cat: path too long\n")
    else
      out.say("cat: invalid path\n");
    return;
  end;
  st := file_view.cat(target, "cat");
end;
