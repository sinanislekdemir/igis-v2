# 0018: Floating point is in the first milestone — one `f64` type and a per-task FPU region

- **Status:** Accepted
- **Date:** 2026-11-13
- **Deciders:** project owner
- **Related specs:** 0002 (architecture), 0003 (language), 0004 (memory model), 0007
  (scheduler)
- **Extends:** ADR 0011 (work-stealing; complete task state in regions), ADR 0007 (fully
  static placement)

## Context

The first draft of the language deferred floating point and specified integers only,
justified by a small first milestone and the cost of formatting and parsing. The project
owner rejected that: ordinary numeric work — measurement, control laws, geometry — is not
something to postpone, and a language that cannot express it is not a foundation. Floating
point therefore has to be designed in from the start, not added later.

The constraint that makes this non-trivial is the execution model. Under work stealing
(ADR 0011), a task may be released on one core and claimed by another, and Sch5 requires
that at a release point the complete execution state is in the task's regions with nothing
live in a core's registers. Every other piece of state already follows that rule; the SSE
registers are the first hardware context the system has had to carry.

Three approaches are available on x86_64:

1. **Always save and restore SSE state.** A 512-byte FXSAVE image per task, written at
   release and read at claim. Simple, deterministic, no fault path.
2. **Lazy FPU.** Set `CR0.TS`, take `#NM` on first float use, save on context switch. Saves
   the FXSAVE when a task never touches floats, but introduces a per-core "last FPU owner"
   that is exactly the affinity ADR 0011 removed.
3. **XSAVE with AVX state.** Larger area, `XSAVE`/`XRSTOR`, CPUID dispatch. More state per
   task than the system needs.

## Decision

### One float type

- `f64`, IEEE-754 binary64, is the only float type in the first milestone. `f32` is not
  included; it can be added later without changing the task-state format, but adding it
  now would double the arithmetic and formatting surface for no M1 need.
- There is no x87 usage, no fused multiply-add and no extended precision. The compiler
  emits SSE2 scalar double operations.

### The FPU region

- Each task owns a **kernel-owned `fpu` region**: a 512-byte, 16-byte-aligned FXSAVE image.
  It is not part of the application's nameable regions, exactly like a stream buffer. The
  application cannot read or write it; it never learns the address.
- The loader zeroes it and initialises the image with default control state: round to
  nearest even, all exceptions masked. An application therefore cannot start with a
  rounding mode inherited from an earlier task.
- On release the scheduler executes `FXSAVE` into the task's `fpu` region before the task
  is re-enqueued. On claim it executes `FXRSTOR` after taking ownership and before the
  first bytecode. The region is always current while the task is not running.
- No `CR0.TS`, no `#NM` handler, no per-core FPU ownership. Every task saves and restores
  its own state, whether or not it uses floats.

### Semantics

- No implicit conversion between integers and `f64`. Conversions are the explicit casts
  `f64(x)`, `int(x)`, `uint(x)` and `byte(x)`.
- `f64(x)` from an integer is ordinary IEEE conversion; values wider than 53 bits round.
  That is not a fault.
- `int(x)` and the other integer casts truncate toward zero and fault `Overflow` when the
  value is `NaN` or outside the destination range.
- Float arithmetic follows IEEE-754, including `Inf` and `NaN`. Float division by zero
  produces `Inf`/`NaN`; it is not `fault(DivideByZero)`, which remains an integer rule.
- Comparisons are IEEE: `NaN` is unequal to everything including itself, ordering
  comparisons are false when an operand is `NaN`, and `<>` is true.
- Determinism: round-to-nearest-even, exceptions masked, no FMA, no x87. The same bytecode
  produces the same bits on any x86_64 core.

### Text conversion

- The first milestone converts floats to text with an explicit fractional-digit count:
  `core.format_f64_into(dst, value, frac_digits)`.
- Parsing is `stdlib.parse_f64_into(text, var value)`, decimal only, returning `Status`.
- Shortest round-trip formatting (Ryū/Grisu class) and full IEEE text conformance are
  deferred. The arithmetic is complete; the text layer is not pretending to be.
- Transcendental helpers (`sqrt`, `sin`, ...) are `stdlib` code written in the language, so
  they are deterministic and inspectable. They move to `core` only if measurement demands
  it, as for any other helper.

## Consequences

### Positive

- Real numeric work is possible from the first boot: control laws, geometry, sensor math.
- The task-state rule survives intact: the SSE context is another region, saved and
  restored like everything else, and migration stays invisible.
- No lazy-FPU bookkeeping, no `#NM` path, no per-core "last owner" that would reintroduce
  affinity through the back door.
- Rounding and masking are fixed by the kernel, so a task cannot inherit another task's
  floating-point environment.
- The interpreter's arithmetic is closed under IEEE for the four basic operations; no
  host library is needed for correctness.

### Negative

- 512 bytes per task, saved and restored at every claim and release, including for tasks
  that never touch floats. Against a ~1 ms quantum this is noise, but it is not free.
- `f32` is absent. An application that wants single precision must use `f64` and round
  explicitly.
- Float text output is fixed-point in M1; printing a value with an unknown magnitude (for
  example a computed exponent) needs `stdlib` work.
- Adding AVX or `f32` later changes the `fpu` region size and/or the opcode set; each is a
  new decision with its own ADR, not a silent extension.

### Neutral / follow-up work

- The `fpu` region is a normal task region for allocation, reclamation and the guard
  policy, but it is never exposed through a handle.
- Float values are allowed in records, sum types, arrays, rings and map values. They are
  not map keys, because the map contract requires a total order-free deterministic hash
  and `NaN` breaks equality.
- A debugging feature may want the saved context; that is introspection work, not part of
  the host interface.

## Alternatives considered

| Alternative | Why not |
|-------------|---------|
| Integer-only, with fixed-point in `stdlib` | Rejected by the owner. It pushes ordinary numeric work into a library, loses range and precision, and makes every application invent its own scaling convention. |
| Lazy FPU (`CR0.TS` + `#NM`) | Saves saves for non-float tasks, but needs per-core ownership of the last FPU state, which conflicts with migration and adds a fault path that must be correct on every core. |
| `XSAVE` with AVX/AVX-512 state from the start | 2.5 KiB or more per task, CPUID dispatch and a variable-size area, for registers the compiler does not use. |
| `f32` and `f64` together | Two arithmetic surfaces and mixed-precision rules in the first milestone; `f32` is additive later. |
| Host `libm` for transcendentals | Results vary between hosts and libc versions, which breaks the "same bytecode, same answer" property; `stdlib` code in the language is deterministic. |
| Extend `Diagnostic` with FPU state on fault | Larger diagnostic ABI for a debugging convenience; the saved `fpu` region can be inspected by a future introspection feature instead. |
