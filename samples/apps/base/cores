unit cores;

// Spawns six `worker` instances before waiting for any of them, so they are all
// runnable at once and several cores claim from the same global set (spec 0007,
// Sch1-Sch3). Waiting inside the spawn loop would serialize them and defeat the
// point.

uses core, syslib;

var
  children: array[int, 8];
  i: int;
  st: Status;
  id: int;
  diag: Diagnostic;
  reason: ExitReason;
  n: int;
  digits: string[16];
  line: string[96];

procedure main(args: const string[1024]);
begin
  core.write("cores: spawning 6 workers\n");

  i := 0;
  while i < 6 do
  begin
    children[i] := 0;
    st := core.spawn("worker", "", diag, id);
    if st = Ok then
      children[i] := id
    else
    begin
      line := "cores: spawn failed: " + diag.message + "\n";
      core.write(line);
    end;
    i := i + 1;
  end;

  // Now wait for all of them. The parent blocks, so the workers run on
  // whichever cores are free.
  i := 0;
  while i < 6 do
  begin
    if children[i] > 0 then
      st := core.wait(children[i], reason);
    i := i + 1;
  end;

  n := core.format_int_into(digits, int(syslib.cpu_id()), uint(10), uint(0));
  line := "cores: 6 workers done, parent on core=" + digits + "\n";
  core.write(line);
end;
