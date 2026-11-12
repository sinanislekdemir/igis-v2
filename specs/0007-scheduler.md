# 0007: Scheduler

- **Status:** Draft
- **Scope:** `sched`, `arch/<isa>` timer and IPI, interpreter safe points
- **Supersedes:** none
- **Superseded by:** none
- **ADRs:** 0003 (cooperative model, budget intent), 0006 (guard pages), 0011 (work
  stealing)
- **Updated:** 2026-11-12

## Context

Every application is a task whose execution state lives in its own regions. There is no
hardware context to save, no per-process address space, and no reason to bind a task to a
core. A core is a place to run bytecode; a task is a value that can be advanced anywhere.

This spec defines the loop, the claim protocol, the two time limits (quantum and budget),
blocking, wake-up, termination and the SMP bring-up path. It is the scheduler that makes
ADR 0011 concrete.

## Requirements

- **Sch1 (MUST):** Each online core runs exactly one interpreter loop.
- **Sch2 (MUST):** Runnable tasks live in a global runnable set. A core claims a task
  atomically; claiming grants exclusive ownership.
- **Sch3 (MUST):** A task is advanced by at most one core at any instant. A task in the
  runnable set is not running; a running task is not in the set.
- **Sch4 (MUST):** A core runs a claimed task for a quantum of N bytecodes or about one
  millisecond, whichever comes first, and releases it at the next safe point.
- **Sch5 (MUST):** At release the complete execution state is in the task's regions. No
  live application state remains in a core's registers.
- **Sch6 (MUST):** The interpreter has no global mutable state. Per-core scratch is the
  only core-local data.
- **Sch7 (MUST):** Exhausting the quantum releases the task and re-enqueues it; it does
  not terminate it.
- **Sch8 (MUST):** Exhausting the total CPU budget terminates the task unconditionally at
  the next safe point, with `fault(BudgetExceeded)`. Recovery is rejected.
- **Sch9 (MUST):** Blocked tasks consume no CPU and are not in the runnable set.
- **Sch10 (MUST):** Termination is deterministic: remove from all queues, close stream
  endpoints, invalidate handles, release all regions.
- **Sch11 (MUST):** The first milestone runs one core and brings up the others within the
  same milestone; the protocol is identical in both cases.
- **Sch12 (SHOULD):** Fairness is FIFO. There are no priorities, no affinity and no load
  balancing policy in the first milestone.

## Design

### Terminology: three limits, one confusing word

The word **quantum** is operating-systems jargon for a discrete scheduling slice, borrowed
from physics; it has nothing to do with quantum mechanics. It is easy to conflate with the
two other limits in the system, so they are stated together once:

| Limit | Measured over | On expiry |
|-------|---------------|-----------|
| **Quantum** | one claim: one run of one task on one core | Release at a safe point and re-enqueue; not a fault |
| **Total CPU budget** | the task's whole lifetime, blocked time excluded | Terminate unconditionally at a safe point |
| **Handler budget** | one `fault` or `exit` invocation | Abandon the handler and reclaim the task |

A quantum is measured in interpreter steps (bytecodes), with a time fallback; it is not
measured in source lines. The total budget is the watchdog. The handler budget is the
emergency allowance for teardown and diagnosis.

### Task states

```
            claim                release (quantum/yield)
Runnable ──────────▶ Running ───────────────────────────▶ Runnable
   ▲                    │
   │                    ├── block ──▶ Blocked(stream|timer) ── wake ──▶ Runnable
   │                    │
   │                    └── budget/fault ──▶ Terminated
   └────────────────────── wake ─────────────┘
```

- `Runnable` — in the global set, owned by nobody.
- `Running` — owned by exactly one core.
- `Blocked(stream | timer)` — not in the set; a condition will re-enqueue it.
- `Terminated` — removed from everything; regions reclaimed after `exit`.

### The loop

```
loop {
    task = runnable_set.pop()           // atomic; the pop is the claim
    if task.is_none() {
        halt_until_ipi_or_interrupt()
        continue
    }

    core.current = task
    switch_to_task_stack(task)
    run_quantum(task)                   // steps N or ~1 ms
    switch_to_core_stack()
    core.current = none

    if task.state == Running {
        task.state = Runnable
        runnable_set.push(task)         // release
    }
}
```

- The atomic pop is the ownership transfer. There is no separate lock.
- `pop` has acquire semantics and `push`/release has release semantics; everything the
  previous owner wrote is visible to the next owner.
- A task that blocks inside `run_quantum` is enqueued by the wake-up path, not by the
  release path.
- A task that terminates inside `run_quantum` is reclaimed by the termination path and is
  never re-enqueued.

### Safe points

The interpreter checks for quantum expiry, budget expiry and pending yields at:

- every bytecode boundary,
- backward branches,
- calls,
- builtin entry.

A release or termination happens only at one of these points. No partially applied
instruction, no half-updated frame and no inconsistent region state is ever observed by
another core.

### Quantum

- Measured in interpreter steps (a counter) and optionally in time (the per-core timer).
- Step count is the primary mechanism; the timer is the fallback for very slow builtins
  that nevertheless reach a safe point.
- Expiry releases the task; it is preemption by cooperation, not a fault and not a
  termination.
- The quantum is small enough that a runnable task waits a bounded number of quanta, not
  so small that claim overhead dominates.

### Budget

- Total CPU time consumed by the task, accumulated across cores, excluding blocked time.
- A single compile-time constant in the first milestone. Applications cannot negotiate
  (vision OQ 20).
- Checked at safe points against the monotonic clock.
- Expiry terminates: `fault(BudgetExceeded)` is delivered for logging, recovery is
  rejected, `exit` runs, regions are reclaimed.
- A task that never reaches a safe point cannot be terminated; that is the one liveness
  hole the cooperative model accepts, and it is why blocking builtins are forbidden
  (`0002`).

### Blocking and wake-up

- A blocking primitive sets `Blocked(reason)`, records the wait condition, and returns to
  the loop without re-enqueueing.
- The condition (stream data, stream space, timer deadline, peer close) enqueues the task
  and marks it `Runnable`.
- If every core is halted, the enqueuing core sends an IPI to one of them. A per-core
  `need_resched` flag is set before the IPI; the woken core checks it after `hlt`.

### Termination

On budget expiry or an unrecovered fault:

1. The task is removed from the runnable set if present (it cannot be running elsewhere;
   the current core owns it).
2. `fault` runs, then `exit`, on the current core's kernel stack under emergency budgets.
3. Stream endpoints close; queued data is discarded (`0006-streams.md`).
4. `code`, `state`, `stack` and the guard page are returned to `mm` as a unit.
5. The task record is freed to the kernel pool.

There are no zombies. A terminated task leaves nothing behind.

### Per-core state

Each core has:

- a kernel stack used by interrupt entry, `fault`/`exit` and the loop itself,
- a pointer to the currently claimed task (or none),
- a quantum counter and the last budget check,
- scratch space that must never hold application state across a release.

There is no per-core ownership of any application region.

### SMP bring-up

- The boot core initializes the GDT/IDT, paging, APIC and the scheduler, then enters the
  loop.
- APs are started with the standard INIT-SIPI-SIPI sequence, given their own stacks and
  per-core state, and enter the same loop.
- All cores share the global runnable set and the same claim protocol.
- AP bring-up is the second half of the first milestone; the single-core run is the same
  code with one loop.

### Interrupts

- The timer interrupt updates time and sets budget/quantum flags. It does not switch
  tasks.
- Handlers are short; deferred work runs in a bottom half drained by the interpreter loop
  before the next quantum.
- An interrupt that arrives while a core is between tasks (no current task) must not
  assume one exists.

## Interfaces

```rust
// Illustrative.
struct Task {
    state: TaskState,
    ip: usize,
    stack_ptr: usize,
    budget_used_ns: u64,
    quantum_left: u32,
    regions: RegionTable,
    streams: EndpointTable,
}

fn claim() -> Option<TaskRef>;       // atomic pop
fn release(task: TaskRef);           // atomic push
fn block(task: TaskRef, reason: Wait);
fn wake(task: TaskRef);              // enqueue + IPI if needed
fn terminate(task: TaskRef, reason: FaultCode);
```

## Failure modes

| Situation | Behaviour |
|-----------|-----------|
| Quantum expires | Release at safe point; re-enqueue; no fault |
| Budget expires | Terminate at safe point; `fault(BudgetExceeded)`; recovery rejected; `exit`; reclaim |
| Task blocks | Leaves the runnable set; no CPU consumed |
| Wake with all cores halted | IPI to one core; `need_resched` set before the IPI |
| Task terminates while owning a stream endpoint | Endpoint closes; queued data discarded; peer sees `Closed` |
| Two cores claim the same task | Must be impossible; a violation is a TCB defect (silent corruption) |
| Task never reaches a safe point | Core is stalled until the task reaches one; accepted liveness hole |
| AP fails to start | Boot core logs and continues with fewer cores; the milestone is not complete until AP bring-up works |
| Core idle | `hlt` until interrupt or IPI |
| Kernel panic on any core | Serial log, halt all cores (or reboot) |

## Open questions

| # | Question | Trigger / owner |
|---|----------|-----------------|
| 1 | Quantum size (steps and time) and the total budget value | Tuning after the first measurable workloads. |
| 2 | Global queue vs per-core deques with stealing | Performance; claim semantics do not change. |
| 3 | Priority or deadline scheduling | Not planned; budgets are the only policy. |
| 4 | IPC mechanism: fixed-vector IPI vs APIC shorthand | `arch/x86_64` implementation detail. |
| 5 | Timer source: PIT, HPET or APIC timer | `arch/x86_64`; the APIC timer is the likely choice. |
| 6 | Whether a task can be pinned for debugging | A debug feature, not a scheduling model. |

## Changelog

| Date | Change |
|------|--------|
| 2026-11-12 | Initial draft: per-core loops, atomic claim, quantum versus budget, blocking and wake-up, deterministic termination, SMP bring-up within the first milestone. |
