# 0016: Realtime tasks run without quantum preemption under a heartbeat contract

- **Status:** Accepted
- **Date:** 2026-11-12
- **Deciders:** project owner
- **Related specs:** 0001 (vision), 0002 (architecture), 0003 (language), 0005 (host
  interface), 0007 (scheduler)
- **Extends:** ADR 0011 (work stealing), ADR 0003 (budget intent)

## Context

A hardware-control application — the original igis use case — wants low latency and no
mid-protocol interruption. The normal scheduler gives neither: a task is released at
quantum expiry, and it may wait behind other runnable tasks before it is claimed.

Three different properties hide behind the word "realtime":

1. **Latency** — when a device event arrives, the application runs soon.
2. **Non-preemption** — the application is not descheduled in the middle of a protocol.
3. **Determinism** — the same work always takes the same time.

An interpreted system cannot promise (3). It can promise (1) and (2), which is what the
owner asked for: a realtime task runs until it yields or blocks, and it is claimed before
normal tasks.

But removing preemption removes the kernel's only way to tell a busy task from a frozen
one. A cooperative interpreter can see that a task is *executing* (safe points are
frequent), but not that it is *making progress*. The owner specified the missing piece: a
heartbeat. A realtime task must call `core.heartbeat()` at least once per second; if it
does not, the kernel treats it as frozen and terminates it.

Alternatives were a larger quantum (keeps preemption, no heartbeat, but still slices the
protocol), priority only (no non-preemption), and pinning a realtime task to a dedicated
core (conflicts with the work-stealing model and needs a different admission rule).

## Decision

### Promotion

- `core.realtime_on()` promotes the calling task to the realtime class. It is **one-way**:
  there is no `realtime_off()` in the first milestone, because returning to the normal
  class would either reset the total budget or leave an unaccounted task, and both are
  exploitable.
- Promotion returns `Status.Full` if it would make the number of realtime tasks exceed
  the number of online cores. A blocked realtime task still occupies its admission slot,
  so an oversubscribed set cannot appear when it wakes.

### Scheduling

- Realtime tasks are claimed before normal tasks.
- A claimed realtime task is **not released at quantum expiry**. It releases when it
  yields, blocks, or terminates. The quantum still exists for normal tasks.
- Work stealing still applies at release: a realtime task is not pinned, and any core may
  claim it next. While it is running, it holds the core it was claimed on.
- Realtime tasks are **exempt from the total CPU budget**; the heartbeat replaces it. The
  budget is what would otherwise kill a long-running controller.

### Heartbeat

- `core.heartbeat()` resets a one-second deadline.
- The deadline is measured in **non-blocked time**: `Runnable` and `Running` advance it,
  `Blocked` pauses it. A controller waiting for a device for minutes is not frozen.
- When the deadline passes, the task is terminated at the next safe point with
  `fault(WatchdogTimeout)`. Recovery is rejected, as for `BudgetExceeded`: a task that
  missed its progress contract must not be given another turn.
- The heartbeat is a **progress claim**, not a liveness check. The interpreter already
  shows that a task is executing at every safe point; the heartbeat is the application
  saying "one control cycle completed".

### Honesty about the guarantee

This is **soft realtime**. It removes time-slicing and gives priority, but interpretation,
claim overhead and interrupt handling still add jitter. The system does not promise
deadlines, only "runs until it yields, and a frozen controller is detected within about a
second".

## Consequences

### Positive

- A hardware controller keeps a core without being sliced mid-protocol.
- Frozen realtime tasks are detected and killed even though they are exempt from the CPU
  budget.
- Normal tasks keep the fair, preemptive 1 ms quantum; realtime is opt-in and bounded by
  cores.
- The heartbeat is cheap when used at a natural progress boundary (once per control
  cycle), not per instruction.
- The mechanism is enforceable: backward branches and calls are safe points, so a looping
  task always returns control to the kernel in time.

### Negative

- Admission control means an application can be refused realtime (`Status.Full`).
- A realtime task can hold a core for up to one second between heartbeats, so normal tasks
  can be delayed; if every core is realtime, normal tasks (including a REPL) starve.
- The heartbeat is another ABI surface and another way for an application to be wrong.
- No hard realtime: jitter remains, and a missed deadline is detected late (about a
  second), not prevented.
- A blocked realtime task keeps its admission slot even while consuming no CPU.

### Neutral / follow-up work

- A realtime task is still migratable at release; "realtime" is a class, not an affinity.
- The quantum, budget and heartbeat interval are compile-time constants.
- If realtime tasks must outnumber cores, the model needs a different admission rule
  (time-sliced realtime), which is a new decision.

## Alternatives considered

| Alternative | Why not |
|-------------|---------|
| Larger quantum for realtime (10 ms), no heartbeat | Simpler and fair, but the protocol is still sliced and there is no progress contract; the owner asked for non-preemption. |
| Priority only, same 1 ms quantum | Cheapest, but "realtime" would be a misleading name: latency improves, interruption does not. |
| Pin a realtime task to a dedicated core | Strongest latency, but reintroduces core affinity, conflicts with work stealing, and needs a core-assignment policy; admission by count gives most of the benefit. |
| Kernel-level realtime scheduler | Not achievable for interpreted code without hard timing guarantees; promises more than the system can keep. |
| Watchdog with no application heartbeat (kernel resets on any safe point) | Measures interpreter liveness, not application progress; a livelocked controller would look healthy forever. |

## Addendum (2026-11-12): no reserved core

Admission allows one realtime task per online core, and **no core is reserved** for normal
tasks or for the REPL. If N realtime tasks occupy N cores, normal tasks wait until one of
them terminates or blocks. This is a deliberate trade in favour of realtime: the owner
prefers giving N controllers a chance simultaneously over guaranteeing a core for the
shell.

The cost is accepted. A set of realtime tasks can keep the machine from responding
interactively, and the only recovery is for one of them to end (or a reset). The system is
single-user and the owner is trusted; this is not a hostile-code containment boundary.
Reserving a core was considered and rejected for the first milestone.
