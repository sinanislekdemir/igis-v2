unit frames;

// Driver for the binary framing sample (spec 0006, "Interfaces"). The consumer
// is spawned without waiting, so it can load (creating the waiting `frames`
// receiver and the waiting `frames_ack` sender) before the producer runs.
// `core.run` then starts the producer and blocks; the two peers exchange typed
// messages and the acknowledgement that ends the run, and the driver reports
// both exit reasons.

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
  st := core.spawn("frame_in", "", diag, consumer);
  if st <> Ok then
  begin
    out.say("frames: spawn frame_in failed: ");
    out.say(diag.message);
    out.say("\n");
  end
  else
  begin
    st := core.run("frame_out", "", diag, reason);
    if st = Ok then
      report("frames: producer ", reason)
    else
    begin
      out.say("frames: run frame_out failed: ");
      out.say(diag.message);
      out.say("\n");
    end;

    st := core.wait(consumer, reason);
    if st = Ok then
      report("frames: consumer ", reason)
    else
      out.say("frames: wait for consumer failed\n");
    out.say("frames: done\n");
  end;
end;
