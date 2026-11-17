# 0004: Application memory model

- **Status:** Accepted
- **Scope:** `mm`, loader, interpreter, language memory semantics
- **Supersedes:** none
- **Superseded by:** none
- **ADRs:** 0004 (static application memory), 0006 (MMU hardening), 0007 (fully static
  placement), 0011 (work stealing), 0015 (delivery), 0018 (floating point)
- **Updated:** 2026-11-17

## Context

The kernel allocates everything an application will ever use, before it runs, and the
interpreter validates every access. This spec fixes the shape of that memory: what an
application owns, how it names it, and where values live.

The goal is a model where four things are simultaneously true:

1. The kernel can allocate an application's entire footprint in one step.
2. The interpreter can validate any access in constant time, using only the task's own
   metadata.
3. The language can express ordinary programs without an allocator.
4. Any core can run the task, so ownership is a property of the task, not of a core.

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

From ADR 0011:

- **C8:** Regions belong to the task, not to a core. Any core may access them while it
  owns the task; nothing is per-core except scratch.
- **C9:** At every release point the task's execution state is in its regions; no
  application state lives in a core's registers. SSE state is saved into the task's `fpu`
  region (ADR 0018).

From ADR 0015:

- **C10:** Application code and data arrive from a read-only ext2 image; the loader reads
  them before the task starts.

## Vocabulary

- **Region** — a kernel-allocated, virtually contiguous, zeroed area with a size and
  permissions, owned by exactly one task.
- **Handle** — a small integer indexing the task's region table. The application never
  learns the region's base address.
- **Access** — `(handle, offset, len)`, validated against the region's size, permissions
  and the task's ownership.
- **Call-scoped reference** — a `const`/`var` parameter binding that carries
  `(handle, offset, len)` for the duration of one call. It cannot be stored or returned.
- **Capacity** — a compile-time constant bound on how much data a value or region can
  hold.

There is no **slice value**. A sub-range exists only as a call-scoped reference; this is
what removes the dangling-value problem without lifetime rules (D14).

## Structure

A task owns a small, fixed set of regions rather than one undifferentiated arena:

| Region | Permissions | Lifetime | Contents |
|--------|-------------|----------|----------|
| `code` | read-only, NX | task | bytecode and constants; one copy per instance in the first milestone |
| `state` | read-write, NX | task | all top-level variables at static offsets |
| `stack` | read-write, NX | task | native stack for interpreter frames and locals |
| guard page | not present | task | one extra frame immediately below `stack`; allocated with it, excluded from the region, restored before free |
| `fpu` | kernel-only | task | 512-byte FXSAVE image of the task's SSE state; the application never names it |
| stream buffers | kernel-only | endpoint | byte FIFO storage; the app sees only endpoint handles |
| `syslib` regions | kernel-only | mapping or buffer | MMIO windows and DMA buffers, accessed through typed handles |

The compiler computes the exact sizes of `state` and `stack` from compile-time capacities
and records them in the manifest. The loader validates them against the memory quota,
reserves everything, maps it into the shared address space, zeroes writable regions, and
builds the task's handle table before the first instruction executes.

Because the application cannot allocate and cannot hold an address, every value it can
name is inline in a frame, at a static offset in `state`, or a call-scoped reference to
one of those or to a stream.

**Code is not shared in the first milestone.** Each instance gets its own `code` region,
so its lifetime is exactly the task's and no reference counting is needed. Sharing one
read-only `code` region across instances is a later optimisation; the invariant below
already permits it.

**One `state` region.** All top-level data lives in a single read-write region. Per-field
permissions and named sub-regions are not part of the first milestone; the uniform
permission is `rw`, `NX`.

## Selected placement model: fully static (ADR 0007)

Three models were compared — fully static (A), declared regions with bounded monotonic
placement (B), and a hybrid (C). **A was selected.**

| | A: static (chosen) | B: monotonic | C: hybrid |
|---|---|---|---|
| Runtime allocation state | none | one pointer per region | one pointer |
| Layout fully static | yes | no | partly |
| Dangling-reference risk | none by construction | needs lifetime rules | needs lifetime rules |
| Ergonomics for temporaries | poor | good | good |
| Compiler complexity | high | moderate | high |
| Failure modes | capacity/stack overflow | scope escape, reset misuse | both |

The decisive argument is not performance or ergonomics: it is that A removes an entire
error class without adding a language feature. There is no free, so there is no
use-after-free; there is no runtime placement, so there is no dangling reference; there is
no allocation state, so there is nothing for the interpreter to keep coherent. B and C
would buy convenience by introducing exactly the kind of rule (lifetimes, escape analysis)
that makes a language hard to specify and hard to reason about.

### Rules of the static model

- **S1 (MUST):** No value is placed at runtime. Every offset is chosen by the compiler.
- **S2 (MUST):** A value is inline (fixed size) or a call-scoped reference. A buffer's
  length never exceeds its compile-time capacity.
- **S3 (MUST):** Capacities are compile-time constants. They may be selected by a build
  profile, but do not change while a task runs.
- **S4 (MUST):** Constructors write into caller-provided storage. No expression
  allocates, not even implicitly.
- **S5 (MUST):** Exceeding a capacity is an explicit, reportable failure.
- **S6 (SHOULD):** Sum types are sized by their largest variant; there is no boxed or
  indirect variant.
- **S7 (MUST):** Recursion depth is bounded by the declared stack; overflow terminates the
  task (ADR 0006).
- **S8 (MUST):** The compiler computes alignment and padding, target-specific, and records
  the final layout in the manifest. The loader places regions but does not compute
  internal offsets.
- **S9 (MUST):** Capacity values come from `const` expressions and build parameters; the
  same source compiled with different parameters is a different build, not a grown task.

## Design consequences

### Working set vs data set

The model forces a distinction that is usually left implicit: an application's **working
set** (what it holds in memory at once) is bounded and declared; its **data set** (files,
disk images, streams) is not. The working set lives in regions; the data set is streamed
through them. Most "I need more memory" problems are really "I tried to make the data set
the working set".

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
4. **Spawn more instances** and partition the work (spawning is future work; the first
   milestone runs what the image declares).

None of these is "allocate more at runtime", and all of them are visible to the kernel
before execution starts.

## Value representation

Values are one of:

- **Inline** — fixed-size, stored directly in a frame slot or at a static offset in
  `state`. Scalars, records, sum types and buffers (`bytes[N]`, `string[N]`,
  `array[T,N]`, `ring[T,N]`, `map[K,V,N]`) are inline.
- **Call-scoped reference** — a `const`/`var` parameter binding; `(handle, offset, len)`,
  validated on every access, dead when the call returns.
- **Stream endpoint** — an opaque handle; no direct buffer access.
- **`syslib` handle** — an opaque handle to an MMIO window or DMA buffer; typed accessors
  only.

Text is a byte buffer with an explicit length and a compile-time capacity; there is no NUL
terminator, so `string[128] + string[128]` fits exactly in `string[256]`. `string`
additionally carries the valid-UTF-8 invariant; `bytes` does not (`0003-language.md`).

### Construction: assignment into a destination

`dst := a + b` is not an expression that produces a temporary value. It is a copy whose
destination the compiler knows statically:

```
fullname := name + surname
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
  (`s := s + t`) is a compile error; a single-expression non-aliasing rule covers reference
  cases without needing a full lifetime system.
- **Overflow** is a defined failure (D10) — never silent truncation, never growth.

### Parameters: at-least-N

A parameter declared `const string[128]` or `var string[128]` accepts any buffer with
capacity `>= 128`; the callee reads the runtime length and may write at most its declared
capacity. Inside the callee, capacity proofs use the declared N, which is conservative and
sound. `const string[0]` is the idiomatic read-only "any string"; `var string[0]` is
rejected. This is the only compatibility rule between different capacities; variables and
fields require exact types.

### Locals

Locals are inline in the frame, including large buffers. The compiler computes the
worst-case frame size; the manifest's stack size must cover it. There is no separate
kernel-allocated frame stack: the native stack is the only frame storage.

## Stacks

Each task gets a native stack, sized in the manifest, with a guard page at the low end
(ADR 0006). The interpreter's own frames live on this stack, so application recursion
depth is bounded by the declared stack size, and overflow terminates the task rather than
corrupting memory.

The guard is one **extra** frame, not part of the declared size: allocation is
`stack_len + 4096` contiguous frames, the lowest frame is unmapped, and `stack.start` is
`allocation_base + 4096` with `len = stack_len` exactly. The interpreter's frame-extent
check against `stack.len` is therefore unchanged and is the legal overflow path; the guard
only catches a kernel bug that writes below the region. Reclaim returns guard and stack as
one unit (spec 0007): the guard's present bit is restored before the frames are returned
to the allocator, on every path that destroys a task (normal exit, kill, fault, failed
`spawn`, verifier rejection, stale-slot eviction). A task that cannot get the extra
contiguous frame fails placement with `Status.Full`, as any other region would.

An explicit, kernel-allocated frame stack is the alternative; it would allow precise depth
accounting and resumable tasks, at the cost of a second stack abstraction. Not proposed
for v1.

Because any core may run the task, the stack is not core-local: it is part of the task's
regions, and a core switches to it when it claims the task (ADR 0011).

**C6 is enforced at placement.** `Region::allocate_guarded` validates the constructed
layout with the pure `Region::layout_ok` rule: at least one usable frame and one byte,
`span == frames + 1` (guard plus region), `start == base + FRAME_SIZE`, and
`len <= frames * FRAME_SIZE`; the half-open `[start, start + len)` therefore lies inside
the frames above the guard and is disjoint from `[base, base + FRAME_SIZE)`. Both
`base + span * FRAME_SIZE` and `start + len` are checked for overflow, and the base must be
frame-aligned. A mismatch is a recoverable placement failure: the guard is restored, the
frames are freed, and the constructor returns `Status.Invalid` (the REPL reports it as
`cannot place regions` and records `Faulted(IoError)` like any other placement failure); a
failure to *restore* the guard remains the existing kernel-bug halt. A boot self-check runs
the same rule over valid and invalid scratch tuples and prints `stack: layout ok`, so a
geometry bug cannot reach a task guard. This is the cheap mitigation for Q47: the
interpreter is `#![forbid(unsafe_code)]` and bounds-checks its slices, so a mis-based or
oversized `Region` is the realistic route to a hardware guard hit; recovery from the fault
path itself remains blocked on lock-ownership tracking (OPEN-QUESTIONS Q47).

## Streams

Stream buffers are kernel-owned and never mapped into the application's addressable
regions. The application holds an endpoint handle that permits only send/receive operations
of a declared byte capacity. This keeps queue metadata out of reach even though the address
space is shared.

Payloads are copied through the kernel. Sends are atomic and bounded by the endpoint
capacity; there is no partial record. The full contract is in `0006-streams.md`.

## MMIO and DMA

MMIO windows and DMA buffers are owned by the kernel/`syslib`, not by the application's
`state` region. The application receives an opaque `syslib` handle and accesses the range
through typed, bounds-checked accessors (`0003-language.md`). DMA buffers are copied into
and out of by the application; zero-copy application-owned DMA is not part of the first
milestone. Raw physical addresses never reach application-visible values.

## Invariants

- No application-visible value contains a raw address.
- Every region has exactly one owning task. `code` may be shared read-only in a future
  optimisation; in the first milestone it is per instance.
- Writable regions are zeroed before first use.
- No region changes size or permissions after task start.
- Nothing is placed at runtime; every offset is compile-time.
- A call-scoped reference is dead when the call returns; it cannot be stored or returned.
- Ownership is task-level, not core-level: any core may access a task's regions while it
  owns the task.
- A region is never mapped or unmapped per core; the address space is shared.
- Stream buffers and `syslib` regions are outside the application's nameable regions.
- The `fpu` region holds the task's SSE state between claims; it is kernel-owned and never
  appears in the application's handle table.

## Failure modes

| Situation | Behaviour |
|-----------|-----------|
| Access outside an owned region | Interpreter traps; `fault(OutOfBounds)`; recover once or terminate |
| Access to a handle not owned by the task | Interpreter traps; `fault(OutOfBounds)`; recover once or terminate |
| Write to a read-only region | Interpreter rejects before touching memory; MMU is the backstop |
| Capacity exceeded | Fatal fault; `fault(CapacityExceeded)`; no growth |
| Stack overflow | Guard-page fault; `fault(StackOverflow)` on the kernel stack; recover once or terminate |
| Manifest over-declares | Loader refuses if the memory quota is exceeded |
| Capacity does not fit the build parameter | Compile error at build time |
| Caller passes a smaller capacity than the parameter declares | Compile error (at-least-N) |
| Stored reference attempt | Compile error; references are call-scoped only |
| DMA buffer lifetime | Owned by `syslib`; released explicitly or at task termination; not in the app's regions |

## Open decisions

| # | Decision | Status |
|---|----------|--------|
| D1 | Placement model | **Resolved: A, fully static (ADR 0007).** |
| D2 | Inline vs slice thresholds; are slices first-class? | **Resolved** — no first-class slices; sub-ranges are call-scoped references only. |
| D3 | Region granularity | **Resolved** — one `state` region; one `code`, one `stack` per task; no named sub-regions. |
| D4 | Code sharing across instances | **Resolved for M1** — per-instance copy; sharing is a later optimisation. |
| D5 | Stream payloads | **Resolved** — byte FIFO copied through the kernel (`0006-streams.md`). |
| D6 | Large locals in frames or `state` | **Resolved** — frames; the stack is declared in the manifest. |
| D7 | Alignment and padding ownership | **Resolved** — compiler, target-specific; manifest carries the layout. |
| D8 | MMIO/DMA ownership | **Resolved** — kernel/`syslib`-owned, typed handles. |
| D9 | Capacities fixed per build or profiles | **Resolved** — build parameters (`const` expressions). |
| D10 | Overflow for `dst := a + b` | **Resolved** — `fault(CapacityExceeded)`, never truncation or growth. |
| D11 | Are capacities part of the type | **Resolved** — distinct types; at-least-N compatibility for parameters. |
| D12 | In-place concatenation | **Resolved** — forbidden; `s := s + t` is a compile error. |
| D13 | Is `string` bytes or UTF-8 | **Resolved** — both, as distinct types with the same representation. |
| D14 | May a reference outlive the expression | **Resolved** — no; references are call-scoped parameters only. |
| D15 | Exact per-type alignment rules | Compiler detail; documented with the layout algorithm. |
| D16 | Whether `code` sharing ever returns | Future optimisation; requires a read-only shared region and a lifetime rule. |
| D17 | Where the floating-point context lives | **Resolved** — a kernel-owned 512-byte FXSAVE region per task, saved on release and restored on claim (ADR 0018). |
| D18 | Whether the guard frame belongs to the stack region | **Resolved** — one extra frame below the region, excluded from `stack.len`, present-bit restored before free (C6). |

## Changelog

| Date | Change |
|------|--------|
| 2026-11-03 | Initial draft; central fork identified (static vs monotonic placement). |
| 2026-11-03 | Option A selected (ADR 0007); added static-model rules, worked examples, and accepted costs. |
| 2026-11-03 | Destination-form construction specified: `dst := a + b` compiles to `concat_into`, with static capacity proof and no temporaries. |
| 2026-11-12 | Resolved D2-D14: no first-class slices, one `state` region, per-instance code, frames for large locals, compiler-computed alignment, `syslib`-owned MMIO/DMA, build-parameter capacities, `bytes`/`string` split, at-least-N parameters. Added task-level ownership for work stealing (ADR 0011) and ext2 delivery (ADR 0015). |
| 2026-11-13 | Added the `fpu` region: a kernel-owned 512-byte FXSAVE image per task, outside the application's nameable regions; C9 now includes SSE state (ADR 0018, D17). |
| 2026-11-16 | C6 implemented: the guard is one extra frame below the declared stack, excluded from `stack.start`/`stack.len`, allocated as `stack_len + 4096` and restored on every free path (D18). |
| 2026-11-17 | C6 enforced at placement: `Region::layout_ok` checks the guard/region geometry with overflow-safe arithmetic and `Region::allocate_guarded` rejects a mismatch as `Status::Invalid` (unguard and free first); boot self-check `stack: layout ok`. The Q47 hardware-guard recovery remains blocked on lock-ownership tracking, so this is its cheap mitigation. |
