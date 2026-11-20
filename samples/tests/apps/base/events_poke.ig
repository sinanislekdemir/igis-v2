unit events_poke;

// Helper for the `events` proof app (M2.2.5): after a short delay it injects
// one key event, which must wake the parent blocked in `syslib.next_event`
// (`WaitFor::Input`, spec 0010, I19). The delay guarantees the parent reaches
// the blocking call first, so the run proves the block-and-wake path rather
// than a lucky queue hit.

uses core, syslib;

const
  KeyZ = 90 * 65536; // text 0, no modifiers, key code 'Z'
  DelayNs = 150000000;

var
  ev: Event;
  status: Status;
  started: uint;
  now: uint;

procedure main(args: const string[1024]);
begin
  started := core.clock();
  now := started;
  while now - started < uint(DelayNs) do
    now := core.clock();

  ev.kind := KeyDown;
  ev.x := 0;
  ev.y := 0;
  ev.code := uint(KeyZ);
  ev.window := 0;
  status := syslib.inject_event(ev);
end;
