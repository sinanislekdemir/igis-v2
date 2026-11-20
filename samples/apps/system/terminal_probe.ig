unit terminal_probe;

// Console-session surface proof (spec 0021). The shell runs this probe inside
// its own session; the probe is a different task, so it may open its own
// session. One live session per owner is `Full`, a stale id is `NotFound`, and
// closing the owner's session is `Ok`.

uses core;

var
  id: int;
  second: int;
  st: Status;
  ok: bool;

procedure main(args: const string[1024]);
begin
  ok := true;
  id := 0;
  second := 0;
  st := core.session_open(id);
  if st <> Ok or id = 0 then
  begin
    core.write("terminal_probe: open failed\n");
    return;
  end;
  st := core.session_open(second);
  if st <> Full then
  begin
    core.write("terminal_probe: second open BAD\n");
    ok := false;
  end;
  st := core.session_close(999999);
  if st <> NotFound then
  begin
    core.write("terminal_probe: stale BAD\n");
    ok := false;
  end;
  st := core.session_close(id);
  if st <> Ok then
  begin
    core.write("terminal_probe: close BAD\n");
    ok := false;
  end;
  if ok then
    core.write("terminal_probe: ok\n")
  else
    core.write("terminal_probe: BAD\n");
end;
