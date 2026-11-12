# 0007: Fully static memory placement — no runtime allocation of any kind

- **Status:** Accepted
- **Date:** 2026-11-03
- **Deciders:** project owner
- **Related specs:** 0004 (memory model), 0003 (language)
- **Amends:** resolves the sub-allocation question left open by ADR 0004 (vision OQ 13)

## Context

ADR 0004 established that applications get a fixed, kernel-allocated footprint, with no
`malloc`, no growth and no GC. It deliberately left one question open: may an application
run a bump or slab allocator *inside* its fixed arena, so that offsets are chosen at
runtime? That question decides the language's value representation, its constructors, and
how manifests are parameterised, so it cannot stay open.

Three positions were available: fully static placement (A), declared regions with bounded
monotonic placement (B), and a hybrid (C). B and C both introduce allocation state and a
class of dangling references that the compiler must then rule out with lifetime rules.

The project owner selected A, on the principle that constrained resources produce better
designs: an application that expects a packet must buffer it in a fixed-width array, and
"I don't know how much memory I might need" is not an accepted answer.

## Decision

All application memory is placed at compile time. There is **no runtime placement of any
kind** — no bump pointer, no free list, no scratch allocator, no hidden temporary.

- A value is either **inline** (fixed size, stored in a frame slot or at a static offset)
  or a **slice** `(handle, offset, len)` whose length is bounded by a compile-time
  capacity.
- Capacities are compile-time constants. They may be chosen by a build parameter or a
  size class, but they never change while a task runs.
- Constructors write into caller-provided storage (destination-passing style). No
  expression allocates.
- Reaching capacity is an explicit, reported failure — never growth.
- Obtaining more memory means re-instantiating the application with a larger declared
  footprint (new build profile or new manifest), not growing a running task.

## Consequences

### Positive

- No allocation state exists anywhere in the runtime, so there is nothing to corrupt and
  no allocator bug class.
- Use-after-free, double-free and dangling slices are impossible by construction, without
  lifetime or borrow rules: nothing is ever freed, and nothing is ever placed at runtime.
- Every access is a static offset plus a handle check; the interpreter's memory model
  stays small and auditable.
- Resource exhaustion is detected at load time, not in the middle of an operation.
- Footprints are visible, comparable and accounted for by the kernel.

### Negative

- APIs become destination-passing: `format_into(&mut buf, ...)` rather than
  `format(...) -> String`. Verbose, but explicit.
- No `Vec::push` that grows, no string concatenation that returns a new string, no
  boxed values. Capacity failures must be handled everywhere.
- Programs that must hold data-sized state (emulators, image canvases, databases) must
  declare a maximum and fail or spill beyond it.
- Fixed-size slots fragment memory when objects vary in size; size classes or byte
  streams are the escape route, not a general allocator.
- Changing a capacity requires recompiling or re-instantiating, not a runtime decision.

### Neutral

- The compiler and host tooling may allocate freely; only the target program's runtime
  memory is static.
- The kernel's own heap is unaffected.

## Alternatives considered

| Alternative | Why not |
|-------------|---------|
| B — monotonic placement inside declared regions | Comfortable for temporaries, but introduces allocation state and a dangling-slice class that requires lifetime rules to control. |
| C — static state plus scratch region | Same dangling-slice problem, plus two models to explain and test. |
| Per-application heap with a cap | Still a heap: free lists, fragmentation, runtime failure paths, and pointers in values. |
| Compile-time capacities with runtime layout | Contradictory: if sizes vary at runtime, offsets cannot be static. |

## Note on revisiting

This is a deliberate constraint, not a temporary limitation. If a required workload class
turns out to be inexpressible without runtime placement, the answer is a new ADR that
introduces B **for that class only**, with its own isolation and lifetime rules — not a
silent weakening of this one.
