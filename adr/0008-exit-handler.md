# 0008: Fatal faults invoke an application exit handler, then the task is reclaimed

- **Status:** Accepted
- **Date:** 2026-11-03
- **Deciders:** project owner
- **Amended by:** 0009 (control transfer out of the handler)
- **Related specs:** 0002 (architecture), 0003 (language), 0004 (memory model)

> **Note (amended by ADR 0009, then superseded by ADR 0010).** The handler was split into
> two: `exit` for teardown and `fault` for fault decisions. Where this document says
> "no resumption" or "invoked exactly once", read it as "no resumption at the fault site"
> and "not re-entered while running". The current model is in ADR 0010.

## Context

There is no user/kernel boundary and no process to signal. When the interpreter detects an
illegal operation — an out-of-bounds access, a capacity overflow, an illegal instruction,
a budget overrun, a stack guard-page fault — the application cannot be "sent an error" in
the conventional sense; the kernel is already running its code.

The project owner specified a uniform escape hatch: every application carries a global
`exit` function. On a fatal fault the kernel aborts the operation and calls `exit` with a
fixed, kernel-defined reason code. If the application has no such handler, the kernel
terminates it directly.

This is deliberately **not** exception handling. There is no unwinding, no rollback, and
no resumption. `exit` is a finalizer that receives a reason: it may flush streams, write a
diagnostic, or record state, and then the task is reclaimed.

## Decision

- An application may define one top-level handler: `exit(reason: ExitCode)`.
- The kernel invokes it **exactly once** per task, when either:
  - `main` returns normally (`ExitCode::Ok`), or
  - a fatal fault is detected.
- After the handler returns, the task is terminated and all its regions and stream
  endpoints are reclaimed. Execution never resumes in the faulted task.
- The handler runs on the **scheduler's kernel stack**, not the task stack, because the
  task stack may be the thing that failed (stack overflow).
- The handler runs under a small, separate time budget. If it faults, blocks, or exceeds
  that budget, it is abandoned and the task is reclaimed immediately. `exit` is never
  re-entered.
- Reason codes are a fixed, kernel-defined enum. They are part of the stable application
  ABI; adding one is an ABI change.
- If no handler is present, the kernel reclaims the task without invoking anything.

## Consequences

### Positive

- One uniform terminal path for every fatal condition: normal exit, memory fault, budget
  kill, guard-page fault.
- Applications get a last chance to flush output and leave diagnostics, which matters
  because there is no console log from a dead process.
- The kernel does not need per-fault recovery machinery; termination stays total and
  deterministic.
- Reason codes make failures observable and comparable across applications.

### Negative

- Handlers run with possibly corrupted application state, so they must be defensive and
  cannot be trusted to produce correct output.
- It is easy to mistake this for try/catch. There is no resume, and anything that looks
  like recovery must be built as restart, not as continuation.
- The handler is an ABI surface: codes and signature are hard to change later.
- A buggy handler can waste the entire emergency budget; the kernel must be able to
  abandon it cleanly.

### Neutral

- Memory reclamation does not depend on the handler running; it happens regardless.
- The handler may still use builtins (console, streams), subject to the emergency budget
  and to the kernel not holding locks that would deadlock it.

## Alternatives considered

| Alternative | Why not |
|-------------|---------|
| Kill silently, no handler | Simplest, but leaves no way to flush output or diagnose the failure from inside the application. |
| Full try/catch with unwinding | Requires unwinding through interpreter frames, a consistent stack, and a defined state after the fault; the opposite of "no hidden machinery". |
| Restart the task with a reason code | Attractive (supervisor-style), but it requires resetting interpreter state and deciding what survives in the arena. A separate decision if wanted. |
| Signal-like asynchronous handler | Interrupts the handler at arbitrary points and multiplies re-entrancy hazards; too complex for the intended value. |

## Open points

- Whether `exit` also receives a diagnostic record (faulting source position, address,
  operation) written by the kernel before the call.
- Whether applications may define their own non-fault reason codes (e.g. for normal
  shutdown status), and how those are namespaced against kernel codes.
