unit terminal_probe;

// One-terminal refusal proof (spec 0014). The boot shell already holds the
// machine-wide terminal; this task opens its own window and must be refused
// with Full. It closes the window before reporting through `core.write`, which
// lands in the shell's attached terminal.

uses core, syslib;

var
  id: int;
  st: Status;
  refused: bool;

procedure main(args: const string[1024]);
begin
  refused := false;
  st := syslib.window_open(80, 300, 120, 80, "Terminal Probe", id);
  if st = Ok then
  begin
    st := core.terminal_attach(id);
    if st = Full then
      refused := true;
    st := syslib.window_close(id);
  end;
  if refused then
    core.write("terminal_probe: refused Full\n")
  else
    core.write("terminal_probe: BAD\n");
end;
