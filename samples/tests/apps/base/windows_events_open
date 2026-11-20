unit windows_events_open;

// Helper for the `windows_events` proof (M2.3.3): open one window owned by
// this task and exit immediately. The parent verifies that the window went
// with the task (close-on-termination), that the focus fell back to the
// topmost remaining window, and that the close repair exposed the window the
// child overlapped. It never prints.

uses syslib;

var
  id: int;
  status: Status;

procedure main(args: const string[1024]);
begin
  status := syslib.window_open(100, 100, 150, 80, "child window", id);
end;
