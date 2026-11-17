unit pipe_out;

// Producer half of the stream sample (spec 0006). `report` is an `out_stream`
// endpoint declared with the same capacity as `pipe_in`'s receiver. Twenty
// numbered lines are about 180 bytes, well over the 64-byte queue, so the
// sender fills the stream and blocks until the consumer drains it.
//
// Each line goes out through `stdlib.send_line`, which appends the newline and
// sends the whole line as one atomic `core.send`. After the last send the task
// waits for the queue to drain before exiting: termination discards queued
// bytes immediately (S7), so a short delay is what makes the received total
// deterministic in this demonstration.

uses core, stdlib;

var
  report: out_stream[64];
  digits: string[16];
  line: string[32];
  st: Status;
  i: int;
  start: uint;
  now: uint;

procedure main(args: const string[1024]);
begin
  i := 0;
  while i < 20 do
  begin
    core.format_int_into(digits, i, uint(10), uint(0));
    line := "line " + digits;
    st := stdlib.send_line(report, line);
    i := i + 1;
  end;

  start := core.clock();
  now := start;
  while now < start + uint(250000000) do
  begin
    now := core.clock();
  end;
  core.write("pipe_out: sent 20 lines\n");
end;
