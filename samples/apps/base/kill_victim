unit kill_victim;

// Victim for the `kill_stress` proof of Q50 (spec 0007, Sch17). It does one
// thing: transition from runnable to blocked in one of the three wait kinds and
// stay there until the parent kills the task.
//
//   run kill_victim line     // core.next_any, source 0 (WaitFor::Any)
//   run kill_victim stream   // core.recv, no peer  (WaitFor::Stream)
//   run kill_victim event    // syslib.next_event   (WaitFor::Input)
//
// The console is a raw byte stream (spec 0005, ADR 0037), so the line mode
// waits for a byte with `core.next_any` and then reads it; the blocking
// condition is the same `Any` set the shell uses. No input arrives during the
// stress run, so a victim that is not terminated is asleep forever and the
// parent's `core.wait` hangs: the harness timeout is the hang detector. An
// accidental console byte or event ends the block early; the parent then sees
// `wait` return `Ok` instead of `Killed` and still passes, so one stray byte
// can never produce a false failure. The stream endpoint has no peer, which is
// legal (a waiting endpoint) and closes with the task, freeing the name for
// the next victim.

uses core, syslib;

var
  report: in_stream[16];
  sources: array[uint, 8];
  count: int;
  ready: int;
  one: bytes[1];
  chunk: bytes[16];
  ev: Event;
  st: Status;
  got: int;

procedure main(args: const string[1024]);
begin
  if args = "stream" then
    got := core.recv(report, chunk)
  else if args = "event" then
    st := syslib.next_event(ev)
  else
  begin
    sources[0] := uint(0);
    count := 1;
    ready := -1;
    st := core.next_any(sources, count, ready);
    if st = Ok then
      got := core.read_input(one);
  end;
end;
