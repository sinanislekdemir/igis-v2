unit pipe_hold;

// Negative sample: a live sender named `hold` with no receiver. The first
// 64-byte send exactly fills the queue; the second blocks forever. Another
// task that declares `out_stream` `hold` is a load error (same direction, same
// name), which is what `streams_bad` demonstrates before killing this task.

uses core;

var
  hold: out_stream[64];
  chunk: string[64];
  st: Status;

procedure main(args: const string[1024]);
begin
  chunk := "0123456789012345678901234567890123456789012345678901234567890123";
  st := core.send(hold, chunk);
  st := core.send(hold, chunk);
end;
