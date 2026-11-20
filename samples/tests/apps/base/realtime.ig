unit realtime;

// Realtime class (spec 0007, ADR 0016). The parent promotes itself, runs a
// fixed number of control cycles with one heartbeat each, spawns `beat`, waits
// and reports. On `-smp 1` the parent holds the only admission slot, so the
// child's own `realtime_on` is refused with `Full`; the parent still exits `Ok`.

uses core, lib.out;

var
  st: Status;
  i: int;
  n: int;
  id: int;
  diag: Diagnostic;
  reason: ExitReason;
  digits: string[16];
  line: string[96];

procedure main(args: const string[1024]);
begin
  st := core.realtime_on();
  if st = Ok then
    out.say("realtime: promoted\n")
  else
    out.say("realtime: realtime full\n");

  i := 0;
  while i < 200 do
  begin
    core.heartbeat();
    i := i + 1;
  end;
  n := core.format_int_into(digits, i, uint(10), uint(0));
  line := "realtime: " + digits + " control cycles\n";
  out.say(line);

  st := core.spawn("beat", "", diag, id);
  if st = Ok then
  begin
    out.say("realtime: beat spawned\n");
    st := core.wait(id, reason);
    if st = Ok then
    begin
      case reason of
        Ok: out.say("realtime: child ok\n")
        Killed: out.say("realtime: child killed\n")
        Faulted(code):
        begin
          if code = FaultCode.WatchdogTimeout then
            out.say("realtime: child watchdog\n")
          else
            out.say("realtime: child faulted\n");
        end;
      end;
    end
    else
      out.say("realtime: wait failed\n");
  end
  else
  begin
    out.say("realtime: spawn failed: ");
    out.say(diag.message);
    out.say("\n");
  end;

  out.say("realtime: bye\n");
end;
