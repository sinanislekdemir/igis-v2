unit spin;

// The watchdog sample (spec 0007, ADR 0016). It promotes itself to the realtime
// class, then loops forever without a heartbeat. The kernel terminates it at a
// quantum boundary with `Faulted(WatchdogTimeout)` after about a second of
// non-blocked time; the shell reports `child: faulted WatchdogTimeout`.

uses core, lib.out;

var
  st: Status;
  i: int;

procedure main(args: const string[1024]);
begin
  st := core.realtime_on();
  if st = Ok then
    out.say("spin: promoted, no heartbeats\n")
  else
    out.say("spin: realtime full\n");

  i := 0;
  while true do
  begin
    i := i + 1;
  end;
end;
