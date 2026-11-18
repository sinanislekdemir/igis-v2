unit streams_bad;

// Diagnostic driver for the stream load rules (spec 0006, "Dynamic loading").
//
// 1. A live 64-byte `report` receiver exists while `pipe_out128` declares the
//    same name with 128 bytes: the arriving task fails to load and `core.run`
//    reports the loader's Diagnostic through its `diag` out-parameter.
// 2. A live 64-byte `hold` sender exists (blocked on a full queue) while
//    `pipe_dup` declares the same name and direction: the second sender fails
//    to load.
//
// Each held task is killed before the driver exits so the machine can shut
// down; its endpoint then closes through the normal termination path.

uses core, lib.out;

var
  st: Status;
  id: int;
  diag: Diagnostic;
  reason: ExitReason;

procedure main(args: const string[1024]);
begin
  out.say("streams_bad: creating a live `report` receiver\n");
  st := core.spawn("pipe_in", "", diag, id);
  if st = Ok then
  begin
    st := core.run("pipe_out128", "", diag, reason);
    out.say("streams_bad: capacity run status=");
    if st = Ok then
      out.say("Ok")
    else
      out.say("failed");
    out.say(" diag=[");
    out.say(diag.message);
    out.say("]\n");
    core.kill(id);
  end
  else
  begin
    out.say("streams_bad: spawn pipe_in failed\n");
  end;

  out.say("streams_bad: creating a live `hold` sender\n");
  st := core.spawn("pipe_hold", "", diag, id);
  if st = Ok then
  begin
    st := core.run("pipe_dup", "", diag, reason);
    out.say("streams_bad: duplicate run status=");
    if st = Ok then
      out.say("Ok")
    else
      out.say("failed");
    out.say(" diag=[");
    out.say(diag.message);
    out.say("]\n");
    core.kill(id);
  end
  else
  begin
    out.say("streams_bad: spawn pipe_hold failed\n");
  end;
  out.say("streams_bad: done\n");
end;
