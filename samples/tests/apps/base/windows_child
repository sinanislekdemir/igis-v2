unit windows_child;

// Helper for the `windows` proof app (M2.3.1). The parent passes its own
// window id as `main(args)`. This task:
//
//   1. parses the id (a failure is a fault, so the parent sees it),
//   2. opens a window of its own (id = the next one after the parent's),
//   3. tries to close and move the parent's window, which must be refused
//      (`Invalid`) because it is not the owner, and
//   4. exits `Ok` only when every step succeeded; any failure faults on
//      purpose (divide by zero) so the parent's `ExitReason` check fails with
//      a named label.
//
// Running as a separate task is the point: ownership is per task, and an
// application started twice must not touch the first instance's windows.

uses core, stdlib, syslib;

var
  parent_win: int;
  mine: int;
  status: Status;
  zero: int;
  junk: int;

procedure main(args: const string[1024]);
begin
  zero := 0;
  status := stdlib.parse_int_into(args, parent_win);
  if status <> Ok then
    junk := 1 div zero
  else
  begin
    status := syslib.window_open(500, 400, 80, 60, "child", mine);
    if status <> Ok then
      junk := 1 div zero
    else if syslib.window_close(parent_win) <> Invalid then
      junk := 1 div zero
    else if syslib.window_move(parent_win, 0, 0) <> Invalid then
      junk := 1 div zero;
  end;
end;
