# 0003: Language and standard library

- **Status:** Draft (constraints only — the language is not yet designed)
- **Scope:** application language, type system, memory model, standard library
- **Supersedes:** none
- **Superseded by:** none
- **ADRs:** 0002, 0004
- **Updated:** 2026-11-03

## Context

The language is a separate workstream because it defines the system's real ABI: every
application, the standard library, and the host interface are written against it. It can
be developed and tested on the host before the kernel can boot.

This document currently records only the constraints inherited from the architecture.
Syntax and detailed semantics are deliberately undecided.

## Inherited constraints

- **L1 (MUST):** The language is statically typed. Types are the primary tool for making
  memory layouts explicit and checkable.
- **L2 (MUST):** No implicit allocation. Every piece of memory a program uses is either
  declared, obtained from a declared arena, or part of a fixed-size value.
- **L3 (MUST):** No dynamic growth. Collections have declared maximum sizes; operations
  that would exceed them fail explicitly.
- **L4 (MUST):** Safe code has no raw pointers and no pointer arithmetic. References are
  handles or arena-relative offsets, validated by the runtime.
- **L5 (MUST):** Shared-memory streams (fixed-capacity FIFO channels) are the
  inter-application communication primitive.
- **L6 (MUST):** Destruction is deterministic. No tracing garbage collector.
- **L7 (SHOULD):** The language must remain usable for small programs with one screenful
  of code; ceremony is a cost.
- **L8 (MUST):** No runtime placement whatsoever (ADR 0007). Constructors write into
  caller-provided storage; capacities are compile-time constants; exceeding a capacity is
  an explicit failure.
- **L9 (MUST):** Buffers are declared with fixed capacity (`string name[128]`), carry a
  runtime length and a compile-time capacity, and are never resized. Assignment is the
  construction form: `dst = a + b` means "write the concatenation into `dst`", not
  "allocate a result".
- **L10 (MUST):** Text is a byte buffer with an explicit length; no implicit NUL
  terminator, so a 128+128 concatenation fits exactly in 256.
- **L11 (MUST):** Mutable aliasing is forbidden. There is no explicit view or slice
  construction, and no reference value may be stored. Parameter passing is restricted to
  value copies and call-scoped borrows; the exact forms are OQ 13.
- **L12 (MUST):** An application may define two optional top-level handlers:
  `fault(code: FaultCode)` and `exit(reason: ExitReason)`. `exit` runs at every
  termination for teardown; `fault` runs on a fault and decides between termination and
  recovery. At most one recovery per task; there is no retry (ADR 0010).

## Fault and exit handlers

```text
fault(code: FaultCode)      // on a fault: return to terminate, or jump to recover
exit(reason: ExitReason)    // at termination: teardown only
```

Both are optional top-level functions.

**Fault path.** A fault abandons all frames back to main level and invokes `fault(code)`
with a fresh frame. It may:

- **return** — the application terminates (`exit` then runs), or
- **jump to a statically declared main-level recovery label** — the task resumes there.

At most one recovery is allowed per task. A second fault is terminal: `fault` is not
invoked again. Recovery does not reset the time budget, so it cannot be used to escape the
watchdog. `BudgetExceeded` is delivered to `fault`, but recovery from it is rejected: a
runaway application must not occupy the kernel with repeated fault handling.

**Exit path.** `exit(reason)` runs at every termination — normal return, unrecovered
fault, or kernel-forced end — and never resumes application code. It receives `Ok` or the
fault code that ended the task. If no `exit` handler exists, the task ends silently. If
the handler faults or overruns its small emergency budget, it is abandoned.

Both handlers run on the scheduler's kernel stack, because the task stack may have
overflowed. Inside either handler, loops, labels, jumps and calls that return are allowed.
A `fault` may transfer only to a main-level recovery label validated by the compiler and
the interpreter: no nested function interiors, no return to the faulting instruction, no
other application.

Fault codes (delivered to `fault`). `exit` additionally receives `Ok`:

| Code | Meaning |
|------|---------|
| `GenericFault` | Unclassified interpreter fault |
| `OutOfBounds` | Access outside an owned region |
| `CapacityExceeded` | Buffer operation would exceed capacity |
| `StackOverflow` | Guard-page fault |
| `BudgetExceeded` | Watchdog termination (recovery rejected) |
| `IllegalOperation` | Invalid opcode or type operation |
| `DivideByZero` | Arithmetic fault |
| `StreamError` | Invalid or closed endpoint |
| `IoError` | Loader/device failure |
| `ExitFault` | Terminal reason recorded when `exit` itself failed; never delivered to `fault` |

## Open questions

| # | Question | Why it matters |
|---|----------|----------------|
| 1 | Syntax family: C-like, Pascal-like, ML-like, Lisp-like, or original to this project? | Affects everything downstream, including how the original igis felt. |
| 2 | Type system style: nominal vs structural; generics or monomorphised traits; inference depth. | Determines compiler complexity and ergonomics. |
| 3 | Mutability model: `let`/`var`, regions, ownership, or plain mutable variables with checked handles? | Affects aliasing rules and how streams are shared. |
| 4 | Representation of text: fixed buffers, length-prefixed values, slices over arenas, UTF-8? | Text is the hardest case for a no-allocation language. |
| 5 | Collections: fixed-capacity arrays, vectors with declared max, maps; how are they expressed? | Bounded data must be ergonomic or applications become ugly. |
| 6 | Non-fatal error style: return values, sum types, or status codes? Fatal faults are settled (`fault`/`exit`, ADR 0010). | Affects every API signature. |
| 7 | Module and namespace model; how `stdlib` and `syslib` are addressed. | Determines the import surface and tooling. |
| 8 | Concurrency surface: only streams, or also explicit yield/spawn? | Ties directly to ADR 0003. |
| 9 | Does the language expose a bounded `unsafe`/raw layer, or is raw access only via `syslib` calls? | Raw hardware access must come from somewhere. |
| 10 | Is the standard library written in the language or in Rust? | Portability, bootstrapping, and performance trade-off. |
| 11 | Compile-time metaprogramming: none, macros, generics, comptime? | Large complexity lever; easy to over-scope. |
| 12 | Is the application manifest written in the language, a data format, or generated by tooling? | It carries the memory footprint (ADR 0004). |
| 13 | Parameter passing: `value` / `const` / `var` (Pascal-style), or something else? How does a capacity-typed `string[128]` decay when passed to a function? | Determines how buffers move between functions without views or aliasing. |
| 14 | Does `exit` receive diagnostics beyond the code (source position, faulting address)? Are app-defined reason codes allowed? | Fault observability and ABI stability. |

## Changelog

| Date | Change |
|------|--------|
| 2026-11-03 | Created as a constraints-only seed. |
