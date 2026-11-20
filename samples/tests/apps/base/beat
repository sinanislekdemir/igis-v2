unit beat;

// The realtime child (spec 0007, ADR 0016). It tries to promote itself, prints
// the status (`Full` when the single-core parent already holds the only slot),
// runs a few heartbeat cycles and exits `Ok`.

uses core, lib.out;

var
  st: Status;
  i: int;

procedure main(args: const string[1024]);
begin
  st := core.realtime_on();
  if st = Ok then
    out.say("beat: promoted\n")
  else
    out.say("beat: realtime full\n");

  i := 0;
  while i < 50 do
  begin
    core.heartbeat();
    i := i + 1;
  end;
  out.say("beat: bye\n");
end;
