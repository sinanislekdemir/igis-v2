# 0003: Per-core cooperative scheduling with core-pinned tasks

- **Status:** Accepted
- **Date:** 2026-11-03
- **Deciders:** project owner
- **Related specs:** 0001 (vision), 0002 (architecture)

## Context

The system has no privilege boundary and runs everything at ring 0 in one address space
(ADR 0001). It still needs to run more than one application, and applications must not
be able to stall the machine indefinitely. The obvious answer — preemptive kernel threads
— is the classic place where interpreter-based systems collapse: every preemption point
must be safe with respect to interpreter state, partially updated values, and any
collector or allocator. This system has no collector, but it does have interpreter frames,
kernel locks, and shared streams.

The project owner specified the model directly: one cooperative green thread per CPU
core, applications pinned to a core for life, and a watchdog that can terminate a task
whose interpreted line runs too long while other tasks are waiting.

## Decision

- Each CPU core runs exactly **one kernel scheduler thread**.
- Applications are **cooperative tasks** multiplexed on exactly one core; they never
  migrate.
- Task switches happen only at **interpreter safe points**: explicit yield, blocking on
  a stream or timer, or budget expiry.
- A per-core timer maintains task budgets; the interpreter checks remaining budget and
  pending yield requests at safe points (bytecode boundaries, backward branches, calls,
  builtin entry).
- If a task exceeds its budget **and** other tasks are runnable on that core, the task is
  **terminated**, not suspended. Termination reclaims its arena and closes its stream
  endpoints deterministically.
- Builtins that can take unbounded time must complete in bounded time or transition the
  task to `Blocked`; they may not spin on the core.
- A later permission system may let critical applications request larger budgets or
  exclusive use of a core. It is not part of the first milestone.

Terminology: the per-core scheduler context is a *kernel thread*; applications are
*green tasks* running inside it.

## Consequences

### Positive

- No involuntary preemption of the interpreter, so no re-entrancy discipline for
  half-finished operations.
- Deterministic per-application execution with respect to core-local state; no migration
  means no cross-core invalidation of application state.
- Termination is a clean, total reclaim: no zombies, no GC, no leaked arenas.
- The scheduler stays small; the hard part is the interpreter's safe-point discipline,
  which is localized.

### Negative

- A task that never reaches a safe point stalls its core. This is why blocking builtins
  are forbidden, and it makes `syslib` design constrained by the scheduler.
- A single long-running application can monopolize a core; budgets are the only
  mitigation.
- Pinning prevents load balancing; core assignment policy becomes a visible design
  decision (OQ 17).
- Termination is abrupt and loses state; applications must be written to tolerate being
  killed at any safe point.
- Watchdog termination is not real-time scheduling; it is a liveness backstop.

### Neutral

- SMP still requires locking for kernel-global state, since cores run concurrently even
  though applications do not migrate.
- Hardware interrupts still preempt the interpreter and must be short.

## Alternatives considered

| Alternative | Why not |
|-------------|---------|
| Preemptive kernel thread per application | Real preemption, but every interpreter and kernel state becomes a preemption hazard; far more locking and debugging than the thesis justifies. |
| Single-core only, no SMP | Much simpler, but abandons the per-core model and caps the system unnecessarily. |
| Async/await executor with work stealing | Adds a second concurrency model, breaks pinning and determinism, and work stealing contradicts "an application cannot change its core". |
| Involuntary timer preemption inside the interpreter | Requires saving and resuming interpreter state at arbitrary points; effectively reinventing preemption for the runtime. |
| Suspend instead of terminate on budget overrun | Preserves task state but requires storing full interpreter state, resurrection semantics, and a policy for when to resume; contradicts the specified behaviour. |
