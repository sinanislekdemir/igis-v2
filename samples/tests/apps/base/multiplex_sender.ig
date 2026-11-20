unit multiplex_sender;

// Producer half of the Q39 multiplex proof. It waits briefly so the parent has
// entered `core.next_any`, sends one message, then stays alive long enough for
// the parent's receive to drain it: termination discards queued bytes (spec
// 0006, S7), so exiting immediately after the send would race the receiver.

uses core;

var
  demo: out_stream[64];
  st: Status;
  start: uint;
  now: uint;

procedure main(args: const string[1024]);
begin
  start := core.clock();
  now := start;
  while now < start + uint(50000000) do
  begin
    now := core.clock();
  end;

  st := core.send(demo, "ping");

  start := core.clock();
  now := start;
  while now < start + uint(500000000) do
  begin
    now := core.clock();
  end;
end;
