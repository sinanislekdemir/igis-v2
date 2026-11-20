unit kill_stress;

// Q50 stress proof: killing a task that another core has claimed (spec 0007,
// Sch17). The bug was a lost kill request: `request_kill` published its record,
// the owning core read it just before the publication, and then blocked the
// task; the task slept with no owner and `core.wait` hung forever. The kernel
// now re-checks the id-tagged record under the slot lock in `task::block`.
//
//   run kill_stress          // 300 iterations
//   run kill_stress quick    // 40 iterations (a smoke run for a harness)
//
// Each iteration spawns a fresh `kill_victim` (a task that immediately blocks in
// `core.next_any` on console source 0, `core.recv` or `syslib.next_event`), waits a varying delay
// so the kill lands across the victim's start, its first quanta and the block
// transition (the kernel prints its block trace before parking the task, which
// is the multi-millisecond window Q50 names), kills it, and waits. Every wait
// must return `Ok` with `Killed` or `Ok`; a hang is detected by the harness
// timeout. A kill that arrives before the request is serviced cancels it and
// still records `Killed`; one that arrives while the request is being compiled
// returns `NotFound` for a moment, so the parent retries a bounded number of
// times.
//
// The three wait kinds are cycled so all of them go through `task::block`; the
// event mode assumes an otherwise quiet event queue and still passes if a stray
// event ends the block early. Deterministic proof of the exact interleaving is
// the boot self-check `kill: records ok` (task::kill_self_check), which parks a
// record for a claimed task and asserts `block` makes it runnable again.

uses core, syslib, lib.out;

const
  Iterations = 300;
  QuickIterations = 40;
  // Delay cycle: 0..23 ms, one value per iteration. The victim's block trace
  // lasts a few milliseconds (serial mirror), so a spread of a few tens of
  // milliseconds hits it often.
  DelayCycle = 24;
  DelayStepNs = 1000000;
  RetryStepNs = 200000;
  RetryLimit = 100;

var
  iterations: int;
  i: int;
  step: int;
  mode: int;
  victim: int;
  arg: string[16];
  diag: Diagnostic;
  reason: ExitReason;
  st: Status;
  delay: uint;
  until: uint;
  spins: uint;
  tries: int;
  killed_count: int;
  ok_count: int;
  failed: bool;
  failure: string[64];
  started: uint;
  elapsed_ms: uint;
  // The host has no scheduler and no blocking, so it names the console mode
  // only; a graphics-disabled kernel boot takes the same path, while the
  // kernel cycles all three wait kinds.
  host_like: bool;

procedure spin_until(deadline: uint);
begin
  while core.clock() < deadline do
    spins := spins + uint(1);
end;

// `ExitReason` is a sum type: `Killed` and `Ok` both count as a completed kill
// (the request was cancelled before the victim started, or it exited cleanly
// when a stray event ended its block). A fault is a failure.
function reason_ok(): bool;
var
  good: bool;
begin
  case reason of
    Ok: good := true
    Killed: good := true
    Faulted(code): good := false
  end;
  return good;
end;

procedure fail(what: const string[0]);
begin
  if not failed then
  begin
    failed := true;
    failure := what;
  end;
end;

procedure main(args: const string[1024]);
begin
  if args = "quick" then
    iterations := QuickIterations
  else
    iterations := Iterations;
  failed := false;
  failure := "";
  killed_count := 0;
  ok_count := 0;
  spins := uint(0);
  started := core.clock();
  host_like := syslib.screen_width() = uint(0);

  i := 0;
  while i < iterations and not failed do
  begin
    mode := i - (i div 3) * 3;
    if host_like then
      arg := "line"
    else if mode = 1 then
      arg := "stream"
    else if mode = 2 then
      arg := "event"
    else
      arg := "line";

    victim := 0;
    st := core.spawn("kill_victim", arg, diag, victim);
    if st <> Ok or victim <= 0 then
      fail("spawn")
    else
    begin
      // Vary the kill instant; 0 means "kill as soon as possible" so some
      // iterations also cover the queued and in-service request paths.
      delay := uint(i - (i div DelayCycle) * DelayCycle) * uint(DelayStepNs);
      spin_until(core.clock() + delay);

      st := core.kill(victim);
      if not host_like then
      begin
        tries := 0;
        while st = NotFound and tries < RetryLimit do
        begin
          spin_until(core.clock() + uint(RetryStepNs));
          st := core.kill(victim);
          tries := tries + 1;
        end;
      end;

      st := core.wait(victim, reason);
      if st <> Ok then
        fail("wait")
      else if not reason_ok() then
        fail("reason")
      else
      begin
        case reason of
          Ok: ok_count := ok_count + 1
          Killed: killed_count := killed_count + 1
          Faulted(code): ok_count := ok_count + 0
        end;
      end;
    end;
    i := i + 1;
  end;

  elapsed_ms := (core.clock() - started) div uint(1000000);
  if failed then
  begin
    out.say("kill_stress: FAIL ");
    out.say(failure);
    out.say(" at iteration ");
    out.say_int(i);
    out.say("\n");
  end
  else
  begin
    out.say("kill_stress: ok (");
    out.say_int(iterations);
    out.say(" iterations, ");
    out.say_int(killed_count);
    out.say(" killed, ");
    out.say_int(ok_count);
    out.say(" exited, ");
    out.say_int(int(spins));
    out.say(" spins, ");
    out.say_int(int(elapsed_ms));
    out.say(" ms)\n");
  end;
end;
