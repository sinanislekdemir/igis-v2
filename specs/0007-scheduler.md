# 0007: Scheduler

- **Status:** Accepted
- **Scope:** `sched`, `arch/<isa>` timer and IPI, interpreter safe points
- **Supersedes:** none
- **Superseded by:** none
- **ADRs:** 0003 (cooperative model, budget intent), 0006 (guard pages), 0011 (work
  stealing), 0016 (realtime class), 0018 (floating point)
- **Updated:** 2026-11-17

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
- **Sch12 (SHOULD):** Fairness is round-robin over the runnable slots: after a
  successful claim the scan starts at the next slot, so a task that is always
  runnable in a low slot cannot starve the higher slots on a single core. There are
  no priorities, no affinity and no load-balancing policy in the first milestone.
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
   │                    ├── block ──▶ Blocked(stream|timer|task exit) ── wake ──▶ Runnable
   │                    │
   │                    └── budget/fault ──▶ Terminated
   └────────────────────── wake ─────────────┘
```

- `Runnable` — in the global set, owned by nobody.
- `Running` — owned by exactly one core.
- `Blocked(stream | timer | task exit)` — not in the set; a condition will re-enqueue it.
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
- The first milestone uses **10 000 bytecodes per claim** (`QUANTUM_STEPS`, kernel
  `sched.rs`). This is provisional and chosen for measurement, not tuned (Q1).

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
- The first milestone uses **10 s of CPU time** (`TOTAL_BUDGET_NS`) with a hard cap of
  **200 000 000 executed bytecodes** (`TOTAL_BUDGET_STEPS`). The 1 ms clock cannot resolve
  a sub-millisecond quantum, so time alone under-counts; until the clock is finer, the
  step cap is the watchdog that actually fires and the time budget is the backstop for
  slow builtins. Both values are provisional (Q2).

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
  set cannot appear when it wakes. "Online" means online at admission time: an earlier
  `Status.Full` is not re-evaluated when an AP arrives later, and the caller retries. The
  online count never decreases in the first milestone.
- **Priority.** The runnable set is scanned for the realtime class first, then for normal
  tasks, so a core claims a runnable realtime task before any normal one. The class is
  part of the task value, so no separate queue is needed.
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

**Implemented slice.** `core.realtime_on()` (builtin 11) and `core.heartbeat()`
(builtin 12) are in the kernel, with `Host::realtime_on`/`Host::heartbeat`
defaults so host mocks are unaffected. Admission is one global `AtomicUsize`
compared against `smp::online_count()` in a CAS loop, so two concurrent
promotions cannot both take the last slot; a repeat `realtime_on` on an already
realtime task is `Ok` and takes nothing, and `heartbeat` on a normal task is a
no-op. `task::claim` scans the realtime class first, then the normal one. The
kernel loop keeps the claimed value and runs further quanta on the same core
until the task blocks, exits, is killed, or its deadline passes; the host records
`promoted` and the heartbeat count and the loop applies them at the boundary,
because the task value belongs to the core, not the host. The deadline is
`budget_ns + HEARTBEAT_NS` (1 s), where `budget_ns` is the accumulated
claim-to-release time and a block adds nothing; it is reset on promotion and
after any quantum that contained a heartbeat. The admission slot is released
only in `terminate`, never on block or release, and termination prints
`, realtime, N heartbeats` in the per-task line. Starvation is unchanged and
accepted: no core is reserved.

### Blocking and wake-up

- A blocking primitive sets `Blocked(reason)`, records the wait condition, and returns to
  the loop without re-enqueueing.
- The blocking conditions are `core.wait(child)`, the staged `core.read_line` (`Console`)
  and a stream send/receive (`Stream(handle)`). A blocked task stays in its slot with
  `state = Blocked` and a `WaitFor` condition (`Child(id)`, `Console` or `Stream(handle)`);
  `claim` skips blocked slots and the wake-up path clears the condition in place, so the
  task's regions and machine never move. A `Blocked` task counts as live, so the system
  does not halt.
- A blocking builtin that cannot complete returns `Blocked` from the interpreter without
  advancing `pc` or the operand stack; the instruction is retried after the wake-up, so a
  retry sees the same frame. The service dispatcher still runs before every claim (and
  polls console input), so a queued child is compiled even while its parent is blocked.
- A child termination wakes `Child(id)`; a finished console line wakes `Console`. A send
  wakes a receiver blocked on `Stream(handle)`, a receive that frees space wakes a sender,
  and a stream close wakes both. The condition enqueues the task and marks it `Runnable`.
- After `task::block` the scheduler rechecks the condition (`repl::has_exit` for a child,
  `stream::ready_for` for a stream): it may have become true between the failed attempt
  and the block, and the wake-up path would then have found nobody to wake. The stream
  check takes the stream lock and releases it before `task::wake_stream` takes any slot
  lock.
- If every core is halted, the enqueuing core sends an IPI to one of them. A per-core
  `need_resched` flag is set before the IPI; the woken core checks it after `hlt`.

### Termination

On budget expiry or an unrecovered fault:

1. The task is removed from the runnable set if present (it cannot be running elsewhere;
   the current core owns it).
2. `fault` runs, then `exit`, on the current core's kernel stack under emergency budgets.
3. Stream endpoints close; queued data is discarded (`0006-streams.md`).
4. The guard page's present bit is restored, then `code`, `state`, `stack` and the guard
   page are returned to `mm` as a unit (a frame freed while still unmapped would fault on
   the first touch of its next life).
5. The task record is freed to the kernel pool.

There are no zombies. A terminated task leaves nothing behind.

### Services and task creation

- Before claiming a task — and whenever the runnable set is empty — the loop calls the
  kernel service dispatcher. It processes at most one pending compile-and-run request per
  opportunity (`0002-architecture.md`, ADR 0017). A compile is not interrupted: it takes as
  long as it takes, and the core is unavailable meanwhile — accepted in ADR 0019. The rule
  exists so requests are serviced in order and a core returns to the scheduler between
  compiles. The dispatcher runs before every claim rather than only when the set is
  empty: if it ran only on idle, a machine with a permanently runnable task would never
  service a queued request. Only one core compiles at a time — a core claims a request by
  taking the service lock, and other cores skip the opportunity.
- A request creates a task: the service resolves the path in ext2, reads the source,
  compiles it, allocates `code`/`state`/`stack` plus the stack's guard frame/`fpu`, builds
  the task record, and enqueues it as `Runnable`. The guard is installed before the task
  can be claimed by any core.
- A request path is validated before the image is searched. It must be relative to the
  image root, at most 256 bytes, with no leading `/`, no empty component, no trailing `/`
  and no `..` component. A malformed path returns `Status.Invalid`; a well-formed path that
  names no regular file returns `Status.NotFound`; a directory or other non-regular entry
  returns `Status.Invalid`. Matching is exact and case-sensitive.
- The task record stores the fixed `string[1024]` argument; the loader passes it to the
  child's `main(args: const string[1024])`. The queue holds 1024 pending requests by
  default, a compile-time constant.
- The request carries the reserved task id. On child termination the id and exit reason
  go to the global ring, and the parent is woken if it is blocked in `wait`/`run`.
- Child regions are reclaimed at termination; the exit reason survives in the global ring
  until `wait` consumes it or it is evicted.
- `kill` on a live task marks it for termination at its next safe point; `exit(Killed)`
  runs and the task is reclaimed. `kill` on a request that is still queued cancels the
  request: no task is created, and `Killed` is recorded in the exit ring under the
  reserved id. `kill` on an unknown or already terminated id returns `Status.NotFound`.
  It never waits.
- A request that fails to load (missing file, compile error, placement failure) creates no
  task: the compiler's diagnostic is printed, `Faulted(IoError)` is recorded under the
  reserved id together with a 192-byte `Diagnostic` image, and a parent blocked in
  `wait`/`run` is woken. `wait` observes `Faulted(IoError)`; `run` also copies the
  diagnostic into its caller-owned out-parameter (spec 0005, "Implemented ABI").
- A spawned child is independent: if the parent terminates, the child continues. The
  parent's memory is reclaimed; the child keeps running.
- If every core is held by a realtime task, the service gets no opportunity and no
  application starts. This is the accepted no-reserved-core tradeoff (ADR 0016
  addendum).

### Per-core state

Each core has:

- a kernel stack used by interrupt entry, `fault`/`exit`, the service opportunity and the
  loop itself: 256 KiB on every core, with a not-present guard frame below the boot
  core's (Q42) and below every AP's static `.bss` stack (installed before the SIPIs;
  `kernel/src/guard.rs`). An AP stack matches the boot core's size because any core may
  compile a request and the compiler's bounded recursion needs up to ~200 KiB,
- a pointer to the currently claimed task (or none),
- a quantum counter and the last budget check,
- scratch space that must never hold application state across a release. The
  verifier scratch buffer is one per core (`sched::VERIFY`), so two cores parse
  at the same time without sharing a lock (Sch6),
- its loop counters (claims, yields, exits), summed by the boot core in the
  final summary.

No lock is held across `Machine::run`. The only cross-core state is the runnable
table (one spin lock per slot), the REPL request queue, the in-service request
markers, the exit-reason ring, the console, the frame allocator and the per-core
flags; each has its own lock or atomic and none nests.

The SSE registers are not part of that scratch: they belong to the claimed task and are
saved into its `fpu` region before the core looks for the next task.

There is no per-core ownership of any application region.

### SMP bring-up

- The boot core initializes the GDT/IDT, paging, APIC, frame allocator and the
  scheduler, then enters the loop.
- APs are started with the standard INIT-SIPI-SIPI sequence through a real-mode
  trampoline page at physical `0x8000` (SIPI vector 8) beside a parameter page at
  `0x9000`. The trampoline switches the AP to protected mode, then to long mode with the
  shared PML4, and calls the kernel entry on that core's stack. Each AP claims a core
  index from an atomic counter in the parameter page.
- `MAX_CORES` is one constant, **8**. Core index 0 is the boot core; APs take
  `1..MAX_CORES` as they come up. The index is internal; the APIC id is what software
  sees (`syslib.cpu_id`). Per-core state and stacks come from fixed arrays; nothing is
  allocated during bring-up.
- Discovery is by attempt: the boot core sends INIT and two SIPIs to APIC ids
  `1..MAX_CORES` (10 ms after INIT, ~200 µs between SIPIs) and waits a bounded 100 ms
  for the online reports. An id that never reports is not online; `-smp 1` and `-smp 2`
  boot cleanly and the milestone does not hang waiting for absent cores.
- All cores share the global runnable set and the same claim protocol. The single-core
  run is the same code with one loop (Sch11).

### Idle, wake-up and shutdown

- A core with nothing to run marks itself idle, polls console input once, checks the
  runnable set and then executes one atomic `sti; hlt` step. Interrupts are disabled for
  the check, so an IPI that arrives after it is pending when `hlt` executes and the core
  wakes instead of sleeping. An idle core never spins.
- A producer publishes work first and then claims an idle core: it clears that core's
  idle flag, sets its `need_resched` flag and sends the wake IPI (vector `0x40`). The
  handler only acknowledges with an EOI; the loop re-checks the runnable set after
  `hlt` returns. Producers are `task::spawn`, `task::release`, `task::wake` and the REPL
  enqueue.
- With no live task, no pending/in-service request and no console line anywhere, and
  every other online core idle, the boot core prints the summary, sets a shutdown flag,
  broadcasts the IPI, waits bounded for each AP's acknowledgement and halts. Each AP
  acknowledges from its idle loop and halts.


### Interrupts

- The timer interrupt advances the global clock on the boot core and sets budget/quantum
  flags on every core. It does not switch tasks.
- Wake-up uses one fixed IPI vector (0x40), delivered in fixed mode to a physical
  destination; a wake that any core may serve uses the all-excluding-self shorthand. The
  sender sets the target's `need_resched` flag before the IPI. The handler only
  acknowledges with an EOI; the core re-checks the runnable set after `hlt` returns.
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
| `kill` on a queued request | Request cancelled; `Killed` recorded under the reserved id; no task created |
| `wait` on a killed id | `ExitReason.Killed` |
| Request path malformed or not a regular file | `Status.Invalid`; no request enqueued |
| Parent terminates with live children | Children continue (ADR 0017 addendum 2) |
| Compile fails in the REPL service | Request fails; `Faulted(IoError)` plus the diagnostic is recorded under the reserved id; a waiting parent wakes; no task created; `run` reports the diagnostic through `diag` |
| Task blocks | Leaves the runnable set; no CPU consumed |
| Wake with all cores halted | IPI to one idle core; `need_resched` set before the IPI |
| Task terminates while owning a stream endpoint | Endpoint closes; queued data discarded; peer sees `Closed` |
| Two cores claim the same task | Must be impossible; a violation is a TCB defect (silent corruption) |
| A claimed task's slot | Reserved by task id from `spawn` until `reclaim`, so a concurrent `spawn` cannot place a child where a running task will return |
| `wait` on a child that is running, queued or being compiled | All three are "live", so the parent blocks and is woken by the exit record |
| `kill` of a task another core has claimed | Recorded per slot; the owning core terminates it at its next quantum boundary |
| Task never reaches a safe point | Core is stalled until the task reaches one; accepted liveness hole |
| AP fails to start | Never reports; the boot core continues with fewer cores after the bounded wait |
| Core idle | `sti; hlt` until an interrupt or IPI; no spinning |
| No work anywhere | Boot core prints the summary, sets the stop flag, IPIs every online core, waits bounded for acknowledgements, halts |
| Kernel panic on any core | Serial log, halt all cores (or reboot) |

## Open questions

| # | Question | Trigger / owner |
|---|----------|-----------------|
| 1 | ~~Quantum size (steps and time) and the total budget value~~ | **Resolved (provisional)** — 10 000 bytecodes per claim and a 10 s CPU budget with a 200 000 000-step cap; tune after measurement (see Quantum and Budget). |
| 2 | ~~Global queue vs per-core deques with stealing~~ | **Resolved** — one global table of slots with a per-slot lock; the atomic take is the claim. Per-core deques remain a performance option. |
| 3 | Priority or deadline scheduling | Not planned; budgets are the only policy. |
| 4 | ~~IPC mechanism: fixed-vector IPI vs APIC shorthand~~ | **Resolved** — one fixed vector (0x40), fixed delivery to a physical destination, all-excluding-self shorthand for any-core wakes. |
| 5 | ~~Timer source: PIT, HPET or APIC timer~~ | **Resolved** — APIC timer for ticks, calibrated against the PIT/HPET at boot; one global nanosecond counter advanced by the boot core (see Time). |
| 6 | Whether a task can be pinned for debugging | A debug feature, not a scheduling model. |
| 7 | ~~Whether to reserve one core for the REPL/system~~ | **Resolved** — no reservation; realtime may occupy every core (ADR 0016 addendum). |
| 8 | ~~Heartbeat interval (one second) and whether it is configurable per task~~ | **Resolved** — one second of non-blocked time, a fixed compile-time constant (`HEARTBEAT_NS`); per-task configuration is out of scope for the first milestone (OPEN-QUESTIONS Q4). |
| 9 | ~~Whether the service dispatcher runs before every claim or only when the runnable set is empty~~ | **Resolved** — before every claim, at most one request per opportunity. |
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
| 2026-11-13 | Boot and service details fixed: INIT-SIPI-SIPI through a sub-1 MiB trampoline with `MAX_CORES` static per-core state; one IPI vector (0x40); the dispatcher runs before every claim and a compile is not interrupted (ADR 0019); realtime admission counts the cores online at call time; `kill` cancels a queued request and records `Killed`; request paths are validated (relative, no `..`, regular files only). |
| 2026-11-13 | First scheduler implementation: 10 000-bytecode quantum and a 10 s / 200 000 000-step total budget, both provisional (Q1); the step cap is the effective watchdog while the clock ticks at 1 ms. |
| 2026-11-13 | Spawn/wait implemented: a 1024-slot request queue serviced before every claim, a 64-entry exit ring, `Blocked(task exit)` with wake-on-termination, and `kill` for live tasks and queued requests. A blocked builtin returns without advancing `pc` or the operand stack, so the instruction is retried unchanged. |
| 2026-11-13 | Console input implemented with a second blocking condition: the service opportunity polls COM1 and the PS/2 keyboard and wakes `Console` waiters when a finished line exists; staged line discipline in the kernel (spec 0005, Q44). |
| 2026-11-14 | `core.run` implemented (spawn + wait); the exit ring also carries a 192-byte `Diagnostic` image for a failed request, so `run` reports a post-spawn compile error through `diag` while `wait` observes `Faulted(IoError)`. |
| 2026-11-15 | Per-core schedulers: every online core runs the claim/quantum/release loop over the global set, per-core verifier scratch and loop counters, IPI wake for idle cores (`sti; hlt`, `need_resched` before the IPI), a boot-core shutdown that stops the APs and prints per-core totals, and a slot reservation plus an in-service marker so `wait` never races a spawn or a compile. |
| 2026-11-15 | Realtime class implemented: `core.realtime_on`/`core.heartbeat` (builtins 11/12), admission by a CAS counter against the online core count, realtime-first claim, an inner per-core quantum loop that does not release a realtime task, a one-second deadline in non-blocked time (`HEARTBEAT_NS`), `Faulted(WatchdogTimeout)` at the boundary, and the admission slot released only at termination. `samples/apps/{realtime,beat,spin}` exercise promotion, `Full` on a single core, and the watchdog. |
| 2026-11-15 | Stream blocking added as a third `WaitFor`: `WaitFor::Stream(handle)` with `task::wake_stream`, sender/receiver/close wake rules and a post-block `stream::ready_for` recheck that closes the failed-attempt-to-block race. Sends and receives take the stream lock without any slot lock and wake after releasing it. |
| 2026-11-16 | Per-task stack guards: placement takes `stack_len + 4096` contiguous frames, the lowest is unmapped and excluded from the region, and reclaim restores it before returning guard and stack as one unit on every destruction path (failed spawn, verifier rejection, exit, kill, fault, stale eviction). Every PTE change is followed by a CR3 reload and an IPI shootdown to all online cores. |
| 2026-11-17 | Sch12 made concrete as round-robin: `claim_class` starts its scan at a global `NEXT_CLAIM` cursor, stores the claimed slot + 1 after a successful take, and wraps; the old always-from-slot-0 scan let a busy parent starve its higher-slot children on `-smp 1` (observed by the M2.3.5 two-application proof). On SMP the races make the rotation approximate but never permanently skip a runnable slot. Realtime-first claim is unchanged. |
