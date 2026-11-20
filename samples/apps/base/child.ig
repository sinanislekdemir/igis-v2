unit child;

uses core;

var
  line: string[32];
  n: int;

procedure main(args: const string[1024]);
begin
  // Echo the argument the parent passed, so `run child world` proves that
  // the command tail reached `main(args)`.
  line := "child: args ";
  core.write(line);
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
