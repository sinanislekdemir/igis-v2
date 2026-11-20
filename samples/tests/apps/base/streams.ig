unit streams;

// Driver for the cross-application stream sample (spec 0006). The consumer is
// spawned without waiting, so it can load (creating the waiting `report`
// receiver) while the producer runs. `core.run` then starts the producer and
// blocks; the producer fills the 64-byte queue and blocks until the consumer
// drains it. Finally the driver waits for the consumer, which exits on
// `Closed` after the producer terminates.

uses core, lib.out;

var
  st: Status;
  consumer: int;
  diag: Diagnostic;
  reason: ExitReason;

procedure report(what: const string[0]; why: const ExitReason);
begin
  out.say(what);
  case why of
    Ok: out.say("ok\n")
    Killed: out.say("killed\n")
    Faulted(code): out.say("faulted\n")
  end;
end;

procedure main(args: const string[1024]);
begin
  st := core.spawn("pipe_in", "", diag, consumer);
  if st <> Ok then
  begin
    out.say("streams: spawn pipe_in failed: ");
    out.say(diag.message);
    out.say("\n");
  end
  else
  begin
    st := core.run("pipe_out", "", diag, reason);
    if st = Ok then
      report("streams: producer ", reason)
    else
    begin
      out.say("streams: run pipe_out failed: ");
      out.say(diag.message);
      out.say("\n");
    end;

    st := core.wait(consumer, reason);
    if st = Ok then
      report("streams: consumer ", reason)
    else
      out.say("streams: wait for consumer failed\n");
    out.say("streams: done\n");
  end;
end;
