# 0009: Exit handlers may transfer control to an application recovery point

- **Status:** Superseded by 0010
- **Date:** 2026-11-03
- **Deciders:** project owner
- **Superseded by:** 0010 (separate exit and fault handlers; no retry)
- **Related specs:** 0002 (architecture), 0003 (language), 0004 (memory model)

## Context

ADR 0008 defined `exit(reason)` as a finalizer: the kernel calls it once, it returns, and
the task is reclaimed. The project owner has now specified that `exit` may also jump
outside its own body, and clarified the intended model: a **single top-level try/catch**,
conceptually

```
try   { main() }
catch (reason) { exit(reason) }
```

`exit` is therefore the application's only catch handler, not a per-function feature. A
fault anywhere abandons all frames and enters it; from there the handler may terminate the
task or transfer control to a recovery point.

There is no unwinding machinery in this system and none is being added. The mechanism is
therefore a validated non-local jump, not exception handling with stack unwinding.

## Decision

When the kernel invokes `exit(reason)`, the handler may do one of two things:

1. **Return** — the task is reclaimed, exactly as in ADR 0008.
2. **Transfer control to a recovery point** declared by the same application.

If it transfers:

- The target must be a statically declared label or entry at the application's main level.
  It is validated by the compiler and by the interpreter at runtime. Arbitrary computed
  addresses, nested function interiors, and any target outside the application's `code`
  region are forbidden.
- The interpreter **discards the faulted frame stack** and establishes a fresh frame at
  the recovery point. There is no unwinding: no destructors, no finalizers, and no
  cleanup of the abandoned frames runs. Their memory is simply abandoned inside the
  task's existing regions.
- The application's regions are unchanged. The handler and the recovery point may read
  and write them normally.
- Application state may be inconsistent. The kernel guarantees only that memory outside
  the task's own regions was not touched during the fault; everything inside is the
  application's responsibility.
- The task resumes under the normal scheduling and budget rules. A recovery point that
  faults again is subject to a retry limit, after which the task is terminated.
- If the handler returns, or the target is invalid, the task is reclaimed as in ADR 0008.

The handler itself still runs on the kernel stack with an emergency budget; the transfer
must happen within that budget.

## Consequences

### Positive

- A primitive catch is possible without unwinding tables, exception objects, or a
  second stack machinery.
- Long-running applications can reset a subsystem after a fault instead of dying.
- The interpreter implementation is simple: reset the frame pointer to a known, validated
  label instead of walking frames.

### Negative

- This is resume with possibly corrupted state. Nothing checks invariants; the
  application can continue in a logically broken state and produce wrong results.
- Abandoned frames leak their contents inside the arena until the task ends. Since there
  is no allocation, this is bounded but not reclaimed.
- Debugging becomes harder: a fault may not be terminal, and the failure may surface much
  later.
- The retry limit is a policy the kernel must enforce, or a faulting recovery point can
  loop forever.
- Recovery is not composable like exceptions: there is no handler stack, no typed catch,
  no automatic propagation.

### Neutral

- The handler's own control flow remains restricted to its code region, except for the
  single validated transfer to a recovery point.
- Recovery does not affect memory reclamation; the task is only reclaimed if it returns
  or is terminated.

## Alternatives considered

| Alternative | Why not |
|-------------|---------|
| Finalizer only (ADR 0008) | Simpler, but cannot express "catch and continue", which the owner wants. |
| Full unwinding / exceptions | Requires exception tables, a consistent stack, and destructor ordering; far more machinery than the thesis allows. |
| Restart the task from its entry point | Clean state, but loses all progress; a different feature (supervisor restart), not a catch. |
| Any computed jump target | Unverifiable; corrupt bytecode or a bug could jump into kernel or another application's code. |

## Open points

- **Target set:** only function-entry labels, or also labelled blocks inside a function?
  A target must have a well-defined frame layout; entering the middle of a scope is not
  safe.
- **Retry limit:** how many recoveries per task, and does a successful recovery reset the
  counter?
- **Budget faults:** may `exit(BudgetExceeded)` recover, or is budget exhaustion always
  terminal? Recovering from a watchdog kill invites an immediate repeat.
- **Reason-dependent recovery:** can the handler choose a different target per reason?
  Presumably yes, but the language needs a form for it.
