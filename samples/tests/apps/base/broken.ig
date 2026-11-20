unit broken;

// Fails to compile, so the REPL service discovers the error after `spawn`
// returned. `core.run` reports the compiler's message through its `diag`
// out-parameter (spec 0005, "Implemented ABI").
uses core;

procedure main(args: const string[1024]);
begin
  core.write("broken: unreachable\n")
  n := 1;
end;
