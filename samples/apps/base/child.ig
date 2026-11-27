unit child;

uses core, lib.path;

var
  line: string[32];
  n: int;
  cwd: string[256];
  rest: string[256];
  st: Status;

procedure main(args: const string[1024]);
begin
  // Echo the command tail (not the `<cwd>\n` context line), so `run child
  // world` and a bare `child world` both prove that the argument reached
  // `main(args)`.
  st := path.split_cwd(args, cwd, rest);
  line := "child: args ";
  core.write(line);
  if st = Ok then
    core.write(rest)
  else
    core.write(args);
  core.write("\n");

  // Enough work to cross more than one scheduler quantum, so the child is
  // claimed and released rather than finishing inside its first slice.
  n := 0;
  while n < 20000 do
  begin
    n := n + 1;
  end;

  core.write("child: bye\n");
end;
