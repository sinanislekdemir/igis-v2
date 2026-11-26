# 0061: The normal-task budget covers one continuous run, reset on block

- **Status:** Accepted
- **Date:** 2026-11-26
- **Deciders:** project owner
- **Related specs:** 0007 (Sch8, Budget), 0002 (safe points and budgets), 0013
  (the editor)
- **Amends:** ADR 0011's "cumulative CPU time consumed by the task" budget wording;
  the quantum, work stealing and realtime rules are unchanged

## Context

ADR 0011 made the normal-task budget a lifetime quota: CPU time accumulated across
every claim and every block, with only the blocked intervals themselves excluded. That
rule catches a runaway task, but it also kills a long-lived interactive task whose work
is spread over many short, individually harmless bursts.

The text editor reproduced the false positive. Holding Down redraws one 640×480 frame
per key, and a 98 KiB document also spends several seconds in its 256-byte chunked load.
After a few hundred key presses the task exited
`Faulted(BudgetExceeded)` even though it blocked in `syslib.next_event` between every
burst and was completely responsive. The desktop shell avoids the quota by promoting to
realtime, but realtime is a one-way, per-core, controller-oriented class: every
long-lived UI task cannot take a slot, and an editor is not a hardware controller. The
terminal and the browser have the same latent lifetime limit.

The quantum already provides fairness: a normal task is released and re-enqueued every
10 000 bytecodes. What the budget must still catch is a task that keeps consuming CPU
**without ever letting go of the machine**, not a task that has been used for a long
time in total.

## Decision

The normal-task budget is a **per-run watchdog**.

- **Run start.** A normal task's run starts at spawn and at every block. `task::block`
  resets `budget_ns` to zero and records the current machine step count as the new
  `run_steps` base.
- **Expiry.** At a quantum boundary the run expires when
  `budget_ns >= TOTAL_BUDGET_NS` or `machine.steps() - run_steps >= TOTAL_BUDGET_STEPS`.
  Expiry keeps its existing semantics: terminate unconditionally with
  `fault(BudgetExceeded)`, recovery rejected, `exit`, reclaim (spec 0007 Sch8).
- **No reset on a plain yield.** A task that yields only because its quantum expired
  keeps accumulating. An infinite loop therefore still reaches the budget; blocking is
  the only reset.
- **Realtime is unchanged.** A realtime task is exempt from the total budget and keeps
  accumulating `budget_ns` across blocks because its heartbeat deadline is measured in
  non-blocked time since promotion (ADR 0016). Only the normal class resets.
- **Values unchanged.** 10 s and 200 000 000 steps remain provisional (spec 0007 Q2).
  This decision changes what is measured, not the constants.

## Alternatives considered

| Alternative | Why not |
|-------------|---------|
| Keep the lifetime quota and only make the editor cheaper | A long editing session still reaches the lifetime cap. The line index removes the per-key O(document) scans, but a full-window redraw per key is inherent and the cap remains a false positive for any long-lived UI task. |
| Raise `TOTAL_BUDGET_NS`/`TOTAL_BUDGET_STEPS` | Moves the arbitrary wall and still kills a long session; the next larger document or slower host returns to the reported fault. |
| Promote every long-lived UI task to realtime | Admission is one-way and capped by online cores; a blocked realtime task keeps its slot, so a handful of windows would exhaust the machine. The class is designed for controllers with a heartbeat contract, not for ordinary editors. |
| Suspend/resume on expiry (the rejected ADR 0011 option) | Needs resurrection semantics, retained state and a policy for when to resume, and it never actually stops a task that monopolises the machine. |
| Reset on every quantum yield | Destroys the watchdog entirely: every infinite loop yields at its quantum and would run forever. |
| Reset on input delivery instead of block | Narrower than the actual property we want (a task that waits on a stream or socket is also well behaved), and it makes the rule depend on the window/input model rather than the scheduler. |

## Consequences

### Positive

- A long-lived interactive application that blocks between events (editor, browser,
  terminal, clock, network clients) no longer accumulates a lifetime quota. The editor
  survives a sustained Down-key burst and exits `Ok` on Ctrl+Q.
- A task that never blocks — the actual runaway — is still terminated by the same
  `BudgetExceeded` fault after one continuous run. `samples/tests/apps/base/runaway`
  proves it, and the realtime `spin` keeps proving `WatchdogTimeout`.
- The rule is host-speed independent in spirit: it measures one run, not a machine's
  wall-clock total. Under TCG the time component still fires first on a slow host, but
  only for a task that has not blocked.
- No ABI, builtin, opcode or application change is required.

### Negative

- A deliberately misbehaving task can call a blocking primitive (for example `sleep`)
  just before the deadline and run indefinitely. It is cooperative and yields the
  machine, so it cannot monopolise a core; the budget is a bug watchdog, not a security
  boundary.
- Budget accounting is no longer a lifetime total, so a diagnostic cannot read
  `budget_ns` as "CPU used by this task since spawn" for a normal task.
- The first quantum after a block is unbudgeted; a task that blocks every quantum could
  use slightly more than the nominal cap. The step/time caps still bound each run.

### Neutral

- Realtime diagnostics and heartbeat arithmetic are unchanged.
- The editor's sparse line index and its 128 KiB state budget are independent of this
  rule; they reduce the work in a run, not the accounting.
