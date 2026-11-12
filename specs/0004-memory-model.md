# 0004: Application memory model

- **Status:** Draft
- **Scope:** `mm`, loader, interpreter, language memory semantics
- **Supersedes:** none
- **Superseded by:** none
- **ADRs:** 0004 (static application memory), 0006 (MMU hardening), 0007 (fully static placement)
- **Updated:** 2026-11-03

## Context

The kernel allocates everything an application will ever use, before it runs, and the
interpreter validates every access. This spec fixes the shape of that memory: what an
application owns, how it names it, and where values live.

The goal is a model where three things are simultaneously true:

1. The kernel can allocate an application's entire footprint in one step.
2. The interpreter can validate any access in constant time, using only the task's own
   metadata.
3. The language can express ordinary programs without an allocator.

This document is about layout, not syntax.

## Settled constraints

From ADR 0004 and ADR 0006:

- **C1:** No runtime allocation, no `malloc`, no growth, no GC.
- **C2:** The kernel allocates and zeroes the footprint before the task starts.
- **C3:** Application-visible references are handles/offsets, never raw addresses.
- **C4:** Streams have fixed capacity and never grow.
- **C5:** Reclaim is deterministic and total on task termination.
- **C6:** Each task has a fixed stack with a not-present guard page.

From ADR 0007:

- **C7:** No runtime placement of any kind — no bump pointer, no scratch allocator, no
  hidden temporary. All offsets are fixed at compile time.

## Vocabulary

- **Region** — a kernel-allocated, virtually contiguous, zeroed area with a size and
  permissions, owned by exactly one task (or shared read-only).
- **Handle** — a small integer indexing the task's region table. The application never
  learns the region's base address.
- **Access** — `(handle, offset, len)`, validated against the region's size, permissions
  and the task's ownership.
- **Slice** — a value carrying `(handle, offset, len)`; the only way to name non-inline
  data.
- **Capacity** — a compile-time constant bound on how much data a given value or region
  can hold.

## Structure

A task owns a small, fixed set of regions rather than one undifferentiated arena:

| Region | Permissions | Lifetime | Contents |
|--------|-------------|----------|----------|
| `code` | read-only, NX | task (shareable across instances) | bytecode and constants |
| `state` | read-write, NX | task | long-lived application data at static offsets |
| `stack` | read-write, NX | task | native stack for interpreter frames and locals |
| stream buffers | kernel-only writable | endpoint | FIFO storage; app sees only send/receive handles |

The compiler computes the exact sizes of `state` and `stack` from compile-time capacities
and records them in the manifest. The loader validates them against the memory quota,
reserves everything, maps it, zeroes writable regions, and builds the task's handle table
before the first instruction executes.

Because the application cannot allocate and cannot hold an address, every value it can
name is inline in a frame, at a static offset in `state`, or a slice into one of those or
into a stream.

## Selected placement model: fully static (ADR 0007)

Three models were compared — fully static (A), declared regions with bounded monotonic
placement (B), and a hybrid (C). **A was selected.**

| | A: static (chosen) | B: monotonic | C: hybrid |
|---|---|---|---|
| Runtime allocation state | none | one pointer per region | one pointer |
| Layout fully static | yes | no | partly |
| Dangling-slice risk | none by construction | needs lifetime rules | needs lifetime rules |
| Ergonomics for temporaries | poor | good | good |
| Compiler complexity | high | moderate | high |
| Failure modes | capacity/stack overflow | scope escape, reset misuse | both |

The decisive argument is not performance or ergonomics: it is that A removes an entire
error class without adding a language feature. There is no free, so there is no
use-after-free; there is no runtime placement, so there is no dangling slice; there is no
allocation state, so there is nothing for the interpreter to keep coherent. B and C would
buy convenience by introducing exactly the kind of rule (lifetimes, escape analysis) that
makes a language hard to specify and hard to reason about.

### Rules of the static model

- **S1 (MUST):** No value is placed at runtime. Every offset is chosen by the compiler.
- **S2 (MUST):** A value is inline (fixed size) or a slice whose length never exceeds its
  compile-time capacity.
- **S3 (MUST):** Capacities are compile-time constants. They may be selected by a build
  profile, but do not change while a task runs.
- **S4 (MUST):** Constructors write into caller-provided storage. No expression allocates,
  not even implicitly.
- **S5 (MUST):** Exceeding a capacity is an explicit, reportable failure.
- **S6 (SHOULD):** Sum types are sized by their largest variant; there is no boxed or
  indirect variant.
- **S7 (MUST):** Recursion depth is bounded by the declared stack; overflow terminates
  the task (ADR 0006).

## Design consequences

### Working set vs data set

The model forces a distinction that is usually left implicit: an application's **working
set** (what it holds in memory at once) is bounded and declared; its **data set** (files,
network streams, disk images) is not. The working set lives in regions; the data set is
streamed through them. Most "I need more memory" problems are really "I tried to make the
data set the working set".

### Bounds are usually natural

Most domains already have inherent maxima, which the model simply makes explicit:

| Data | Natural bound |
|------|---------------|
| Network packet | MTU |
| Disk block | sector size |
| Screen surface | declared resolution |
| Input line | declared line length |
| Shell history | ring capacity |
| Undo stack | declared step count |

The unpleasant cases are the ones without a natural bound, and those are exactly the
cases where the application should be streaming or spilling rather than buffering.

### Growth becomes re-instantiation

A running task never grows. If an application needs more memory for a new job, the
options are, in order of preference:

1. **Re-scope the job** so the working set fits (stream, window, spill, multi-pass).
2. **Rebuild with a larger capacity** — capacity is a compile-time parameter, so the
   toolchain produces a different build profile (`--set symbols=65536`). This is a new
   binary variant, not a grown task.
3. **Use a larger size class** — the same program built with several capacity profiles;
   the loader selects one from the manifest.
4. **Spawn more instances** and partition the work.

None of these is "allocate more at runtime", and all of them are visible to the kernel
before execution starts.

## Worked examples

| Application | Apparent unbounded need | Static-model answer |
|-------------|-------------------------|---------------------|
| Text editor, unknown file size | "Load the whole file" | Fixed page cache (e.g. 64 KiB) + on-demand reads; editing writes back through the cache. Undo becomes a bounded journal (oldest discarded). Whole-file operations become multi-pass. |
| Compiler, unknown symbol count | "Grow the symbol table" | Declared capacities (`max_symbols`, `max_nodes`, `max_diagnostics`); exceeding them is a clear failure naming the limit. Or spill the AST to disk with a fixed cache. |
| Network server, unknown connections | "Allocate per connection" | Fixed pool of connection slots, each with fixed receive/send buffers; refuse or queue beyond the limit. Backpressure is explicit. |
| Sorting a file larger than memory | "Load it all" | External merge sort with a fixed buffer and multiple passes — the classic answer, and a better algorithm under the constraint. |
| Emulator / nested VM | "Guest RAM is whatever the guest wants" | Guest RAM is a declared parameter of the instance; launch with a larger footprint for a larger guest, or save/restore across instances. |
| Database | "Cache the whole data set" | Fixed page cache; the data set lives on disk; eviction policy, not growth. |
| Image editor | "Canvas size is user-chosen" | Declared maximum canvas; larger images are tiled through a fixed working set or refused at open. |

## Where the model hurts

Honest costs, accepted deliberately:

- **Variable-size records.** Fixed slots waste memory when objects vary widely in size.
  The answers are size classes, byte-oriented streams, or sum types sized by the largest
  variant — not a general allocator.
- **Data-sized state that must be randomly accessed.** An emulator cannot stream guest
  RAM; it must declare it. A database working set cannot exceed its cache. These become
  explicit maximums, and exceeding them is a hard failure.
- **Ergonomics.** Destination-passing APIs are verbose, and every capacity is a decision
  the programmer must make up front.
- **Recursion.** Deep recursion consumes the declared stack; there is no heap-allocated
  continuation.
- **Capacity changes are build-time.** A user cannot hand the running application a
  bigger buffer; the application must be re-instantiated.

## Value representation

Values are one of:

- **Inline** — fixed-size, stored directly in a frame slot or at a static offset.
- **Slice** — `(handle, offset, len)`, validated on every access; `len` is runtime state,
  bounded by a compile-time capacity.
- **Stream endpoint** — an opaque handle; no direct buffer access.

Text is a byte buffer with an explicit length and a compile-time capacity; there is no
NUL terminator, so `string[128] + string[128]` fits exactly in `string[256]`.

### Construction: assignment into a destination

`dst = a + b` is not an expression that produces a temporary value. It is a copy whose
destination the compiler knows statically:

```
fullname = name + surname
      ↓
concat_into(dst = fullname, a = name, b = surname)
```

- **No temporaries.** `a + b + c` lowers to sequential appends into `dst`, not to
  intermediate buffers.
- **Static proof first.** If `cap(dst) >= cap(a) + cap(b)`, the operation provably cannot
  overflow and the compiler emits no check. `string[256] = string[128] + string[128]` is
  exactly this case.
- **Runtime check otherwise.** When capacities do not prove safety, the compiler inserts a
  length check before writing.
- **Aliasing.** The destination must not overlap the operands. The statically visible case
  (`s = s + t`) is a compile error; a single-expression non-aliasing rule covers reference
  cases without needing a full lifetime system.
- **Overflow** is a defined failure (D10) — never silent truncation, never growth.

Because the destination is always a declared buffer, the language needs no allocator and
no return-value temporaries: every construction has somewhere to write.

Buffer reuse is the one place where a static model can still produce a stale view: if a
view of `my_multi_purpose_array` is stored and the array is then rewritten, the view keeps
pointing at valid memory whose meaning has changed. Nothing is freed, so this is not a
dangling pointer — but it is a dangling *value*. See D14.

## Stacks

Each task gets a native stack, sized in the manifest, with a guard page at the low end
(ADR 0006). The interpreter's own frames live on this stack, so application recursion
depth is bounded by the declared stack size, and overflow terminates the task rather than
corrupting memory.

An explicit, kernel-allocated frame stack is the alternative; it would allow precise depth
accounting and resumable tasks, at the cost of a second stack abstraction. Not proposed
for v1.

## Streams

Stream buffers are kernel-owned and never mapped into the application's addressable
regions. The application holds an endpoint handle that permits only `send`/`receive`
operations of a declared element type and fixed count. This keeps queue metadata out of
reach even though the address space is shared.

Whether payloads are copied through the kernel or placed in a genuinely shared ring is
open (vision OQ 19). Copying is proposed for v1: it preserves opaque handles, avoids
atomics and memory ordering in applications, and makes "shared memory" a performance
optimisation rather than a correctness dependency.

## Invariants

- No application-visible value contains a raw address.
- Every region has exactly one owning task, except `code`, which may be shared read-only.
- Writable regions are zeroed before first use.
- No region changes size or permissions after task start.
- Nothing is placed at runtime; every offset is compile-time.
- A slice's length never exceeds its capacity, and its region outlives the task.

## Failure modes

| Situation | Behaviour |
|-----------|-----------|
| Slice out of bounds | Interpreter traps; `fault(OutOfBounds)`; recover once or terminate |
| Access to a handle not owned by the task | Interpreter traps; `fault(OutOfBounds)`; recover once or terminate |
| Write to a read-only region | Interpreter rejects before touching memory; MMU is the backstop |
| Capacity exceeded | Fatal fault; `fault(CapacityExceeded)`; no growth |
| Stack overflow | Guard-page fault; `fault(StackOverflow)` on the kernel stack; recover once or terminate |
| Manifest over-declares | Loader refuses if the memory quota is exceeded |

## Open decisions

| # | Decision | Options |
|---|----------|---------|
| D1 | Placement model | **Resolved: A, fully static (ADR 0007).** |
| D2 | Inline vs slice thresholds; are slices first-class values or restricted? | Language design. |
| D3 | Region granularity: one `state` region or named sub-regions (`state.config`, `state.cache`)? | Affects layout visibility and permissions. |
| D4 | Code sharing: one `code` region per application name, shared across instances? | Affects instance count and reference counting. |
| D5 | Stream payloads: copy-through-kernel or shared ring. | OQ 19. Recommendation: copy for v1. |
| D6 | Are large locals allowed in frames, or must they live in `state`? | Stack sizing and compiler rules. |
| D7 | Alignment and padding rules; who computes them, compiler or loader? | Manifest ABI stability. |
| D8 | MMIO/DMA buffers: `syslib`-owned, outside task regions? | `syslib` spec. |
| D9 | Are capacities fixed per build, or does the toolchain emit capacity profiles/size classes? | Tooling and manifest design. |
| D10 | Overflow behaviour for `dst = a + b` when the compiler cannot prove fit: trap/terminate, error value, or opt-in truncation? | **Resolved** — fatal fault `fault(CapacityExceeded)`, then `exit`; never truncation or growth. |
| D11 | Are capacities part of the type (`string[128]` vs `string[256]` distinct), and what assignment compatibility exists? | **Resolved** — distinct types; compatibility is a separate language rule. |
| D12 | Is in-place concatenation (`s = s + t`) forbidden outright, or defined as an append with overlap handling? | **Resolved** — mutable aliasing forbidden; `s = s + t` is a compile error. |
| D13 | Is `string` a byte buffer or a UTF-8 string with character-level operations? | Text semantics. |
| D14 | May a view outlive the statement that created it (e.g. be stored in `state`), or is it restricted to expression lifetime? | **Resolved** — no view construction; references are call-scoped parameters only. |

## Changelog

| Date | Change |
|------|---------|
| 2026-11-03 | Initial draft; central fork identified (static vs monotonic placement). |
| 2026-11-03 | Option A selected (ADR 0007); added static-model rules, worked examples, and accepted costs. |
| 2026-11-03 | Destination-form construction specified: `dst = a + b` compiles to `concat_into`, with static capacity proof and no temporaries. |
