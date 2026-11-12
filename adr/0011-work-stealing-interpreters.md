# 0011: Per-core interpreter loops with work-stealing tasks (no core pinning)

- **Status:** Accepted
- **Date:** 2026-11-12
- **Deciders:** project owner
- **Supersedes:** 0003 (the core-pinning clause only; cooperative safe points, one
  scheduler loop per core, budgets and deterministic termination remain)
- **Related specs:** 0001 (vision), 0002 (architecture), 0007 (scheduler)

## Context

ADR 0003 pinned every application task to exactly one core for its entire lifetime. The
arguments were determinism with respect to core-local state, no cross-core invalidation,
and a simple scheduler: a core owns a run queue, and a task's state is only ever touched
by that core.

Those arguments assume that a task's execution state is a *hardware* context — registers,
a native stack, and a program counter that only a context switch can move. For an
interpreted application this is false. The interpreter already keeps the instruction
pointer, the frame pointer, the operand stack and every local in the task's own `stack`
region in memory (`0004:62`, `0004:224-233`). A task is therefore a data structure, not a
core-local resource. Moving it to another core is a stack switch and a pointer update, not
a context save/restore.

Pinning has real costs that the interpreter does not need to pay:

- Load imbalance: three runnable tasks on one core and an idle second core is a normal
  outcome, and nothing can move work.
- Idle cores: when a core's only task blocks or exits, that core idles even if another
  core is overloaded.
- A convoluted watchdog rule: "terminate only if the budget is exceeded *and* another
  task is waiting on the same core" (`0002:80-83`). Without pinning the phrase "the same
  core" has no meaning, and the rule collapses to an unconditional budget kill.

The project owner specified the model directly: a free core takes any runnable
application, advances it by a quantum, and puts it back. Applications and threads are not
assigned to cores.

## Decision

### The interpreter loop is per core; the application is not

- Each CPU core runs one interpreter loop.
- Runnable applications live in a **global runnable set** (a single queue in the first
  milestone; per-core deques with stealing are a later optimisation).
- A core **claims** one runnable application atomically before running it. Claiming
  grants exclusive ownership; no other core may advance the same application.
- The core runs the application for a **quantum**: a fixed bytecode step count or about
  one millisecond, whichever comes first.
- At the end of the quantum — always at a safe point — the core **releases** the
  application and claims the next one. Release writes the complete VM state back to the
  task's memory; no live execution state remains in the core's registers.

### Coherence rules

- A task has exactly one owning core at a time. The ownership word is the only
  synchronisation required to run application code.
- Release and claim are the memory-ordering boundaries: writes made by the previous
  owner are visible to the next owner.
- At a release point the task is in one of: `Runnable`, `Blocked`, or `Terminated`. A
  task that yields or blocks releases immediately; a task that exhausts its quantum is
  re-enqueued.
- The interpreter must have **no global mutable state**. All execution state lives in
  the task record or the task's regions; any scratch space is per-core.

### Blocking and wake-up

- Blocking on a stream or timer transitions the task to `Blocked` and removes it from
  the runnable set. A blocked task consumes no CPU and no core.
- When the blocking condition clears (data arrives, space appears, the deadline passes),
  the task is re-enqueued and an idle core is woken with an IPI if every core is halted.

### Budgets

- There are two distinct limits. The **quantum** is a fairness mechanism: expiry
  releases the task, it does not kill it.
- The **total budget** is a watchdog: cumulative CPU time consumed by the task, with
  blocked time excluded. It is a kernel-wide constant in the first milestone. Expiry
  terminates the task at the next safe point, unconditionally.
- Termination is delivered as `fault(BudgetExceeded)`. Recovery from it is rejected
  (ADR 0010).

### Scope of the change

- One application instance is still exactly one task.
- A task never runs on two cores at once, and application-visible execution is still
  single-threaded.
- Migration is invisible to the application: it sees no core identity, no affinity, and
  no thread ID.

## Consequences

### Positive

- No idle core while work is runnable; load balancing is automatic.
- The watchdog rule becomes unconditional and loses its "same core" special case.
- A task's state is fully materialised in memory at every release point, which makes
  freezing, inspection, and (later) snapshotting natural.
- Applications are pure data, not scheduled resources: the thesis that applications are
  values survives contact with the scheduler.
- Blocked tasks cost exactly zero cores, independent of how many cores exist.

### Negative

- Application state is now shared across cores. Cache-line bouncing and cross-core
  invalidation are real, and the "no cross-core invalidation" property of ADR 0003 is
  lost.
- The claim/release protocol is a new correctness surface in the trusted computing base.
  A mistake here means two cores advance the same stack — silent corruption, not a crash.
- Waking idle cores requires an IPI path that pinning did not need.
- The interpreter must be re-entrant and free of global mutable state, which is a
  stricter implementation discipline than a per-core interpreter.
- Interleaving across runs is nondeterministic. This was already true across cores; it is
  now true for a single task as well. Memory accounting and termination remain
  deterministic.

### Neutral / follow-up work

- A hardware context switch between applications becomes a stack switch at a safe point.
- Per-core interrupts, timers and the per-core scheduler context remain.
- Kernel-global state still needs locking, as it did under ADR 0003.
- The global run queue may become per-core deques with stealing if contention shows up;
  the claim protocol is unchanged by that.

## Alternatives considered

| Alternative | Why not |
|-------------|---------|
| Keep core-pinned tasks (ADR 0003) | Load imbalance, idle cores, and a conditional watchdog rule that exists only to compensate for pinning. |
| Preemptive native kernel threads per application | Reintroduces arbitrary preemption points inside the interpreter and every kernel structure that touches task state; the exact complexity the cooperative model exists to avoid. |
| One global interpreter lock | Serialises all application execution and makes SMP pointless. |
| Work stealing at the native-thread level | There are no native application threads to steal; applications are interpreted. |
| Deterministic scheduling / record-replay | Extra machinery and a second execution mode; the system does not promise deterministic interleaving. |
