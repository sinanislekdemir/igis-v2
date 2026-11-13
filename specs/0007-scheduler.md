# 0007: Scheduler

- **Status:** Draft
- **Scope:** `sched`, `arch/<isa>` timer and IPI, interpreter safe points
- **Supersedes:** none
- **Superseded by:** none
- **ADRs:** 0003 (cooperative model, budget intent), 0006 (guard pages), 0011 (work
  stealing), 0016 (realtime class), 0018 (floating point)
- **Updated:** 2026-11-13

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
- **Sch5 (MUST):** At release the complete execution state is in the task's regions,
  including the SSE state saved into the task's `fpu` region. No live application state
  remains in a core's registers.
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
- **Sch13 (MUST):** A task may promote itself once to the realtime class with
  `core.realtime_on()`. Promotion is refused with `Status.Full` if the number of realtime
  tasks would exceed the number of online cores.
- **Sch14 (MUST):** A realtime task is claimed before normal tasks and is not released at
  quantum expiry. It releases when it yields, blocks or terminates; until then it holds
  the core that claimed it. It is not pinned: any core may claim it next.
- **Sch15 (MUST):** A realtime task is exempt from the total CPU budget and must call
  `core.heartbeat()` at least once per second of non-blocked time. A missed deadline
  terminates it at the next safe point with `fault(WatchdogTimeout)`; recovery is
  rejected, `exit` runs, regions are reclaimed.
- **Sch16 (MUST):** The realtime class is soft realtime. It removes time-slicing and adds
  priority; it does not promise deadlines or deterministic timing.

## Design

### Terminology: four limits, one confusing word

The word **quantum** is operating-systems jargon for a discrete scheduling slice, borrowed
from physics; it has nothing to do with quantum mechanics. It is easy to conflate with the
other limits in the system, so they are stated together once:

| Limit | Measured over | On expiry |
|-------|---------------|-----------|
| **Quantum** | one claim: one run of one task on one core | Release at a safe point and re-enqueue; not a fault |
| **Total CPU budget** | the task's whole lifetime, blocked time excluded | Terminate unconditionally at a safe point |
| **Handler budget** | one `fault` or `exit` invocation | Abandon the handler and reclaim the task |
| **Realtime heartbeat** | one second of non-blocked realtime time | Terminate with `WatchdogTimeout`; recovery rejected |

A quantum is measured in interpreter steps (bytecodes), with a time fallback; it is not
measured in source lines. The total budget is the watchdog for normal tasks. The handler
budget is the emergency allowance for teardown and diagnosis. The heartbeat is the
liveness/progress contract that replaces the total budget for realtime tasks.

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
    service_opportunity()               // REPL: at most one pending request
    task = runnable_set.pop()           // atomic; the pop is the claim
    if task.is_none() {
        halt_until_ipi_or_interrupt()
        continue
    }

    core.current = task
    switch_to_task_stack(task)
    restore_fpu(task)                   // FXRSTOR from the task's fpu region
    run_quantum(task)                   // steps N or ~1 ms
    save_fpu(task)                      // FXSAVE into the task's fpu region
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
- `save_fpu` runs before the task can be re-enqueued and `restore_fpu` runs after the
  claim, so the `fpu` region is current whenever the task is not running. Every task pays
  the 512-byte save and restore, whether or not it uses floats (ADR 0018).

### Time

- The runtime sees one clock: a global 64-bit nanosecond counter since boot.
- The boot core's periodic APIC timer tick (1 kHz by default) advances it. Every core
  reads the same value, so a task that migrates between quanta never observes time moving
  backwards and its budget cannot lose or gain time in the move.
- Resolution is the tick period, one millisecond by default. `sleep_until`, budget
  accounting and the heartbeat deadline inherit that granularity; all three are measured
  in seconds, so the coarse tick is not a limitation.
- The TSC is not used. In QEMU TCG it is per-vCPU and can drift, so two cores could
  disagree about a task's accumulated budget or heartbeat deadline under work stealing.
  The APIC timer frequency is calibrated against the PIT or HPET at boot; the calibrated
  value is a boot-time constant.
- A core with nothing to run halts until the next timer interrupt or IPI; time therefore
  advances on an otherwise idle machine and blocked deadlines still fire.

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

### Realtime tasks

A task that controls hardware can ask for a stronger guarantee than the fair quantum:

```
core.realtime_on()   // one way; Status.Full if the realtime class is full
core.heartbeat()     // at least once per second of non-blocked time
```

- **Promotion is one-way.** There is no `realtime_off()`; returning to the normal class
  would either reset the total budget or leave an unaccounted task. Calling
  `realtime_on()` again is a no-op that returns `Ok`.
- **Admission is by core count.** The number of realtime tasks may not exceed the number
  of online cores. A blocked realtime task still occupies its slot, so an oversubscribed
  set cannot appear when it wakes.
- **Priority.** The runnable set has a realtime queue and a normal queue; a core claims
  from the realtime queue first.
- **No quantum preemption.** A claimed realtime task is not released at quantum expiry.
  It releases on yield, block or termination, and then any core may claim it again. It is
  not pinned; it simply holds the core it was claimed on until it lets go.
- **No total budget.** The heartbeat replaces it, because a controller may legitimately
  run for hours.
- **Heartbeat.** The deadline is one second of non-blocked time: `Running` and `Runnable`
  advance it, `Blocked` pauses it. A controller waiting for a device is not frozen. A
  missed deadline terminates the task at the next safe point with
  `fault(WatchdogTimeout)`; recovery is rejected, as for `BudgetExceeded`.
- **Progress, not liveness.** The interpreter already shows that a task is executing at
  every safe point. The heartbeat is the application saying "one control cycle
  completed"; that is what catches a livelocked controller.
- **Starvation is accepted.** Admission allows one realtime task per core, so N realtime
  tasks on N cores can leave no core for normal tasks, including a REPL. If one
  terminates, a core becomes available. Reserving a core was considered and rejected: the
  tradeoff is deliberately in favour of realtime (ADR 0016 addendum).

This is soft realtime: no slices in the middle of a protocol, but interpretation, claim
overhead and interrupt handling still add jitter.

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

### Services and task creation

- Before claiming a task — and whenever the runnable set is empty — the loop calls the
  kernel service dispatcher. The REPL service processes at most one pending
  compile-and-run request per opportunity (`0002-architecture.md`, ADR 0017). This bounds
  the time a core spends compiling.
- A request creates a task: the service resolves the path in ext2, reads the source,
  compiles it, allocates `code`/`state`/`stack`, builds the task record, and enqueues it
  as `Runnable`.
- The task record stores the fixed `string[1024]` argument; the loader passes it to the
  child's `main(args: const string[1024])`. The queue holds 1024 pending requests by
  default, a compile-time constant.
- The request carries the reserved task id. On child termination the id and exit reason
  go to the global ring, and the parent is woken if it is blocked in `wait`/`run`.
- Child regions are reclaimed at termination; the exit reason survives in the global ring
  until `wait` consumes it or it is evicted.
- `kill` marks the child for termination at its next safe point; it does not wait.
- A spawned child is independent: if the parent terminates, the child continues. The
  parent's memory is reclaimed; the child keeps running.
- If every core is held by a realtime task, the service gets no opportunity and no
  application starts. This is the accepted no-reserved-core tradeoff (ADR 0016
  addendum).

### Per-core state

Each core has:

- a kernel stack used by interrupt entry, `fault`/`exit` and the loop itself,
- a pointer to the currently claimed task (or none),
- a quantum counter and the last budget check,
- scratch space that must never hold application state across a release.

The SSE registers are not part of that scratch: they belong to the claimed task and are
saved into its `fpu` region before the core looks for the next task.

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

- The timer interrupt advances the global clock on the boot core and sets budget/quantum
  flags on every core. It does not switch tasks.
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
| Realtime promotion with no free core | `Status.Full`; the task stays normal |
| Realtime task misses its heartbeat | Terminate at safe point; `fault(WatchdogTimeout)`; recovery rejected; `exit`; reclaim |
| Every core held by realtime tasks | Normal tasks wait, including a REPL; accepted tradeoff |
| Spawn request queue full | `Status.Full`; `run` blocks only on the child |
| Argument longer than 1024 bytes | `Status.CapacityExceeded`; no request enqueued |
| Child terminates while parent is in `wait` | Parent wakes with the child's `ExitReason` |
| `wait` on an unknown or evicted id | `Status.NotFound` |
| `kill` on an unknown or terminated id | `Status.NotFound` |
| Parent terminates with live children | Children continue (open question) |
| Compile fails in the REPL service | Request completes with `Status.Invalid`; no task created |
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
| 5 | ~~Timer source: PIT, HPET or APIC timer~~ | **Resolved** — APIC timer for ticks, calibrated against the PIT/HPET at boot; one global nanosecond counter advanced by the boot core (see Time). |
| 6 | Whether a task can be pinned for debugging | A debug feature, not a scheduling model. |
| 7 | ~~Whether to reserve one core for the REPL/system~~ | **Resolved** — no reservation; realtime may occupy every core (ADR 0016 addendum). |
| 8 | Heartbeat interval (one second) and whether it is configurable per task | Detection latency versus overhead. |
| 9 | Whether the service dispatcher runs before every claim or only when the runnable set is empty | Compile latency versus scheduling overhead. |
| 10 | ~~Exit-reason ring size (64?) and whether `run` may block on a full queue~~ | **Resolved** — ring default 64; a full request queue returns `Status.Full`. |
| 11 | ~~Whether the argument string is `core.arguments` or a `main` parameter~~ | **Resolved** — `main(args: const string[1024])`. |

## Changelog

| Date | Change |
|------|--------|
| 2026-11-12 | Initial draft: per-core loops, atomic claim, quantum versus budget, blocking and wake-up, deterministic termination, SMP bring-up within the first milestone. |
| 2026-11-12 | Added the realtime class: one-way promotion, priority, no quantum preemption, admission by core count, heartbeat with `WatchdogTimeout` (ADR 0016). |
| 2026-11-12 | Added the service opportunity and REPL task creation: `run`/`spawn`/`wait`/`kill`, task ids and the exit-reason ring (ADR 0017). |
| 2026-11-12 | Task creation carries the fixed `string[1024]` argument, delivered as `main(args)`; queue capacity 1024; orphan children continue (ADR 0017 addendum). |
| 2026-11-13 | Time fixed as one global nanosecond counter advanced by the boot core's 1 kHz APIC timer tick; no TSC, because per-vCPU TSC drift would corrupt budget and heartbeat accounting; 1 ms resolution is accepted. |
| 2026-11-13 | Claim and release save and restore SSE state with `FXSAVE`/`FXRSTOR` into a per-task `fpu` region; Sch5 now names the FPU context explicitly (ADR 0018). |
