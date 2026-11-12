# 0006: Use MMU hardening at ring 0 (provisional)

- **Status:** Accepted (provisional, with a review trigger — see below)
- **Date:** 2026-11-03
- **Deciders:** project owner
- **Related specs:** 0002 (architecture), 0001 (vision)

## Context

The system has no privilege boundary and no inter-application hardware protection
(ADR 0001), and cannot have any: x86-64 enforces CPL and per-page permissions, not
"applications". The only meaningful bug class left is the kernel's own memory errors —
in the interpreter, in `syslib`, and in drivers.

The MMU still enforces three things at CPL 0: not-present pages fault, read-only pages
fault when `CR0.WP = 1`, and non-executable pages fault when NX is enabled. The
question is whether using them is worth any architectural complexity in a system whose
main selling point is simplicity.

## Decision

Adopt three hardening measures, implemented entirely inside the existing single shared
address space:

1. Kernel text and read-only data are mapped read-only, with `CR0.WP = 1`.
2. Every task stack has a not-present guard page at its growing end; a fault there
   terminates the task instead of panicking the kernel.
3. NX is enabled (`EFER.NXE`), and kernel data, task stacks and application arenas are
   mapped non-executable.

Explicitly: **no per-task page tables and no CR3 switching.** All mappings coexist in
one address space.

## Review trigger

This decision is accepted only on the claim that it does not increase architectural
complexity. If any of the following becomes true, the measure is dropped (or replaced)
rather than the architecture being changed to accommodate it:

- it requires per-task page tables, address-space switching, or a separate mapping per
  task;
- it requires a page-fault handler with more than trivial policy (the guard-page case
  must reduce to: fault address inside a known stack guard range → terminate that task;
  otherwise panic);
- it forces the loader, allocator, or interpreter to track mapping state beyond what
  they already track.

`CR0.WP` and NX are expected to cost a handful of lines in early paging setup. Guard
pages cost a hole in the mapping and a bounded fault check. If that estimate turns out
wrong in practice, the estimate — not the architecture — loses.

## Consequences

### Positive

- Catches the most common catastrophic bug at ring 0: a stack overflow silently
  overwriting adjacent kernel or task memory.
- Prevents accidental writes to kernel text.
- Removes executable-data as an accident class, at negligible cost.
- Guard pages replace a potentially more invasive alternative (interpreter call-depth
  accounting on every call).

### Negative

- A page-fault handler must distinguish "task stack overflow" from a genuine kernel bug,
  which introduces a small amount of policy and a chance of misattribution.
- Stack overflow becomes a task kill, which destroys the task's state; acceptable, and
  consistent with budget-based termination.
- Read-only kernel text forbids runtime patching (not planned).

### Neutral

- U/S bits remain unused for isolation; supervisor may access all pages regardless.
- NX is meaningless for interpreted code but harmless and protects interpreter data.

## Alternatives considered

| Alternative | Why not |
|-------------|---------|
| No hardening at all | Simplest, but leaves the only remaining bug class completely unguarded; stack overflow becomes silent corruption. |
| Interpreter call-depth accounting instead of guard pages | Moves complexity into the hot path (every call) and is less precise; guard pages are cheaper and catch native recursion too. |
| Per-task page tables for real isolation | Reintroduces the address-space machinery ADR 0001 exists to avoid. |
| Full W^X with a stricter mapping policy | Stronger, but more machinery than the thesis needs; NX plus read-only text already covers the realistic accidents. |
