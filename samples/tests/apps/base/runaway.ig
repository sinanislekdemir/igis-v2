unit runaway;

// The normal-task watchdog proof (spec 0007, ADR 0061). It never blocks and
// never exits, so the quantum releases it but the one continuous run reaches
// the 10 s / 200 000 000-step budget; the shell reports
// `child: faulted BudgetExceeded`. The editor's interactive loop blocks in
// `next_event` between events, so its watchdog resets and it survives a burst.

uses lib.out;

var
  i: int;

procedure main(args: const string[1024]);
begin
  out.say("runaway: looping\n");
  i := 0;
  while true do
  begin
    i := i + 1;
  end;
end;
