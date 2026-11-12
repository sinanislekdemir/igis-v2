# 0010: Separate exit (teardown) and fault (recovery) handlers; no retry

- **Status:** Accepted
- **Date:** 2026-11-03
- **Deciders:** project owner
- **Supersedes:** 0009 (single handler that both tore down and recovered)
- **Related specs:** 0002 (architecture), 0003 (language), 0004 (memory model)

## Context

ADR 0008 made `exit(reason)` a finalizer. ADR 0009 let the same handler transfer control
out of itself, which conflated two different jobs: **teardown** and **fault decision**.
The project owner now separates them:

- `exit` always runs when the application ends, for teardown.
- `fault(code)` runs when a fault occurs; it decides whether to end the application
  (return) or jump elsewhere (recover).

The owner also rejected retry machinery: "this is Sparta".

## Decision

### `exit` — teardown, always

- Optional top-level handler: `exit(reason)`.
- Invoked at every task termination: normal `main` return, unrecovered fault, or
  kernel-forced termination.
- It is a teardown hook only. It never resumes the application and never returns to
  application code.
- It runs on the scheduler's kernel stack under a small, fixed emergency budget.
- If it faults or exceeds that budget, it is abandoned and the task is reclaimed.
- If no `exit` handler exists, the task ends silently.

### `fault` — fault decision

- Optional top-level handler: `fault(code: FaultCode)`.
- Invoked when a fault is detected, after the frame stack has been abandoned back to the
  application's main level. It therefore sees the task's regions and global state exactly
  as the fault left them.
- It may:
  1. **Return** — the application terminates: the kernel calls `exit(code)` and reclaims.
  2. **Jump to a validated main-level recovery label** — the task resumes there, with its
     regions intact and possibly inconsistent state.
- It runs on the kernel stack (the task stack may have overflowed) under an emergency
  budget.
- If it faults or overruns, the application terminates; `exit` is still attempted.

### No retry

- A task gets **at most one recovery**. If it faults again after a recovery, the fault is
  terminal: `fault` is not invoked a second time, `exit` runs, and the task is reclaimed.
- Recovery does not reset the time budget. A recovery loop therefore cannot be used to
  escape the watchdog.

### Codes

- Fault codes are a fixed, kernel-defined enum passed to `fault`.
- `exit` receives the terminal reason: `Ok`, or the fault code that ended the task. This
  lets teardown log why it is running.
- `BudgetExceeded` is delivered to `fault`, but recovery from it is rejected: the budget
  is not reset, so a jump would be pointless. `fault` may still log before returning.

## Consequences

### Positive

- Teardown has one path and always runs; failure reporting has another and runs only on
  faults.
- The kernel needs a single bit of state (has this task recovered?) instead of a retry
  counter or policy.
- Applications can inspect their own global state in `fault` to decide whether recovery is
  plausible — no stack trace, but explicit state.
- No unwinding, no handler stack, no exception tables.

### Negative

- One recovery only: a second, unrelated fault kills a task that might have survived.
- `fault` sees possibly corrupted state; its decision can be wrong.
- `exit` is best-effort, not guaranteed, if the handler itself is broken.
- Debugging is harder because a fault may be non-terminal exactly once.

### Neutral

- Abandoned frames remain in the arena until the task ends.
- The kernel still reclaims memory regardless of whether either handler ran.

## Alternatives considered

| Alternative | Why not |
|-------------|---------|
| One handler for both jobs (ADR 0009) | Conflates teardown with recovery, making "always run at exit" and "may recover" conflict. |
| Retry counter with N attempts | Extra policy and state, and it invites fault loops; explicitly rejected. |
| Restart the task from its entry point | Clean state but loses all progress; not the same feature. |
| Full unwinding / exceptions | Requires exception tables and destructor ordering; contrary to the design. |

## Addendum (2026-11-03): why exactly one recovery

Owner's rationale: a fault is a serious event. After one recovery, the next fault is
terminal, because a programmer's infinite-loop or repeated-fault bug must not be able to
force the kernel into unbounded fault handling. The kernel processes at most one fault
per task; beyond that, shutdown.

This also settles `BudgetExceeded`: recovery is rejected, because the budget is not reset
and the purpose of the rule is to stop a runaway application, not to give it another turn.

## Open point

- The exact trigger set for `fault`: every interpreter fault, or only a subset (e.g.
  arithmetic faults but not budget kills)? Current intent is every fault, with recovery
  disabled for `BudgetExceeded`.
