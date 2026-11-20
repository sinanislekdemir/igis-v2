unit windows_events_child;

// Helper for the `windows_events` proof (M2.3.3): while the parent has a
// pending `Expose` for its own window, this task polls the machine-wide queue
// and tries `window_present` on that window. Both must be refused:
//
//   - `poll_event` is `WouldBlock` (the expose is per-window and owner-only),
//   - `window_present` is `Invalid` (foreign window).
//
// It reports the outcome as one injected KeyDown whose `code` is 1 for the
// expected result, 2 if it stole an expose, or 3 if it could present the
// foreign window. It never prints (console output would scroll the parent's
// pixel checks).

uses core, stdlib, syslib;

var
  target: int;
  status: Status;
  present_status: Status;
  stole: bool;
  ev: Event;
  code: uint;

procedure main(args: const string[1024]);
begin
  present_status := Invalid;
  status := stdlib.parse_int_into(args, target);
  if status = Ok then
    present_status := syslib.window_present(target);

  status := syslib.poll_event(ev);
  stole := false;
  if status = Ok then
  begin
    if ev.kind = Expose then
      stole := true;
  end;

  code := uint(1);
  if stole then
    code := uint(2)
  else if present_status <> Invalid then
    code := uint(3);

  ev.kind := KeyDown;
  ev.x := 0;
  ev.y := 0;
  ev.code := code;
  ev.window := 0;
  status := syslib.inject_event(ev);
end;
