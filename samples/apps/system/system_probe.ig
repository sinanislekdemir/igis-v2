unit system_probe;

// M2.6.4 system-control probe (spec 0017).
//
//   run system_probe halt      // request a machine-wide halt
//   run system_probe reboot    // request a system reset
//   run system_probe           // usage, no action
//
// The two primitives never return on a live kernel: `halt` stops every core
// through the shutdown path and halts the caller, and `reboot` stops the other
// cores before driving the platform reset channels. The lines below are the
// last output of the run; the harness (`scripts/system_control_qmp.py`) uses
// them together with QEMU's exit/reset behaviour. A bare host (`langhost`)
// returns `Invalid` from both, so the sample runs there without side effects.

uses core, lib.path;

var
  status: Status;

procedure main(args: const string[1024]);
var
  cwd: string[256];
  rest: string[256];
  arg_st: Status;
begin
  rest := "";
  arg_st := path.split_cwd(args, cwd, rest);
  if rest = "halt" then
  begin
    core.write("system_probe: halt requested\n");
    status := core.halt();
    core.write("system_probe: halt returned (no kernel authority)\n");
  end
  else if rest = "reboot" then
  begin
    core.write("system_probe: reboot requested\n");
    status := core.reboot();
    core.write("system_probe: reboot returned (no kernel authority)\n");
  end
  else
  begin
    core.write("system_probe: usage: system_probe halt|reboot\n");
  end;
end;
