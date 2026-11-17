unit worker;

// Distribution probe (spec 0007, Sch1-Sch3). Each instance reports the APIC id
// of the core that runs it. A task is not pinned, so the two lines may name
// different cores: the first is where the first claim happened, the second is
// where the quantum before the final print ran.
//
// The loop crosses several quanta on purpose, so a worker is claimed, released
// and possibly claimed by another core before it prints.
//
// Each line is assembled in `line` and written with one `core.write`, so a
// worker's output cannot interleave with another core's bytes.

uses core, syslib;

var
  digits: string[16];
  line: string[64];
  n: int;
  i: int;

procedure main(args: const string[1024]);
begin
  n := core.format_int_into(digits, int(syslib.cpu_id()), uint(10), uint(0));
  line := "worker: start core=" + digits + "\n";
  core.write(line);

  i := 0;
  while i < 30000 do
  begin
    i := i + 1;
  end;

  n := core.format_int_into(digits, int(syslib.cpu_id()), uint(10), uint(0));
  line := "worker: core=" + digits + " done\n";
  core.write(line);
end;
