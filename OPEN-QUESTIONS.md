# Open questions

Parked decisions for igis-v2. This is the working register; each spec's own
"Open questions" section remains authoritative for that document. When an item is
resolved, record the answer in the relevant spec or in a new ADR, then remove it here.

**Status:** implementation phase. Last updated: 2026-11-14.
**Snapshot:** specs 0001–0008, ADRs 0001–0019.

## How to use

- One row per unresolved decision, grouped by area.
- `Trigger` says when it must be answered (before which milestone or component).
- A recommendation is a starting point, not a decision.

## A. Tuning constants (before the first boot)

| # | Question | Options / recommendation | Trigger |
|---|----------|--------------------------|---------|
| Q3 | Handler emergency budget | 1 ms or 10 000 steps for `fault`/`exit` | fault/exit implementation |
| Q4 | Realtime heartbeat interval | 1 s default; configurable per task? | realtime implementation |

## B. Host interface and I/O

| # | Question | Options / recommendation | Trigger |
|---|----------|--------------------------|---------|
| Q8 | Debug/introspection primitive | shape open (task listing, counters, trace); none in M1 | after the shell exists |
| Q9 | DMA and block primitives | handle model fixed; no primitive in M1 | first real block device |
| Q39 | Console/stream multiplexing (`poll`/`select`) | none in M1; a task blocks on one source at a time | when an application must serve two input sources |
| Q43 | Distinct `TaskId` type | `core.run`, `spawn`, `wait` and `kill` are implemented; task ids are still `int` (0 invalid). Introduce a nominal `TaskId` when `stdlib` exposes these primitives, so an id cannot be confused with another integer (spec 0003, "Task ids"). The `stdlib` has started (text helpers only) and still exposes no task primitives | before `stdlib` exposes `run` |
| Q44 | Staged kernel line discipline (`core.read_line`) | the kernel polls COM1/PS-2, echoes, assembles and stores lines in a 4-line ring; `core.read_line(var buf)` blocks until a line exists and truncates to the buffer. Replace with application-side editing when string length/truncate/insert operations exist: indexing alone reads and writes single bytes but cannot delete one (spec 0005, "Staged line discipline"). The first `stdlib` slice has no editing operations yet | when `stdlib` string editing exists |
| Q45 | Staged `syslib` handles | `syslib.port_in8`/`port_out8` are implemented with `Port` as a plain scalar `uint` (low 16 bits used); there are no opaque `Port`/`MmioRegion` types, MMIO mapping, or DMA/block primitives yet. Replace with the handle model when MMIO or DMA lands (specs 0003, 0005) | first MMIO or DMA device |

## C. Streams

| # | Question | Options / recommendation | Trigger |
|---|----------|--------------------------|---------|
| Q10 | Explicit close | deferred to streams v2; closed state is independent of owner lifetime; `close` drains then reports `Closed`, task death still discards | streams v2 |
| Q14 | Auto-wire a console stream for REPL apps | no in M1; applications use `core.console_*`; wiring needs a console application and an ownership rule | when a console application exists |

## D. Language

| # | Question | Options / recommendation | Trigger |
|---|----------|--------------------------|---------|
| Q40 | `f32` support | not in M1; additive later, but it needs its own opcode and conversion rules | when single precision is needed |
| Q41 | Float text beyond fixed decimal | shortest round-trip formatting and full decimal parsing are deferred | when float output must round-trip losslessly |

## Decided in the 2026-11-14 session

- **Q42 — bounded compilation and a guarded kernel stack.** The compiler has a fixed
  nesting limit of **32** counted levels (parser expression/statement/unary recursion, plus
  the independent codegen walks for left-deep chains). Exceeding it is a `Diagnostic`
  ("nesting too deep (limit 32)"), never a crash. The limit is measured, not guessed: a
  level costs about 4.9–5.6 KiB of release stack and the existing `shell` already reaches
  15 levels and ~70 KiB. After the frame allocator is ready, `kmain` allocates **65
  contiguous frames**: one not-present guard frame and a **256 KiB** stack. The guard's
  2 MiB identity page is split into 4 KiB PTEs in a static `.bss` page table, the guard PTE
  cleared and CR3 reloaded; the higher-half alias shares the same PD0 entry for guards
  below 1 GiB. Everything after early boot — module mount, REPL, compile, scheduler — runs
  on that stack. `#PF` now has its own IST because an overflow cannot push an exception
  frame on the exhausted stack; the double-fault IST remains the backstop. 256 KiB (not the
  proposed 128 KiB) is required to keep the 32-level bound with ~60 KiB of headroom; a
  larger limit needs either a larger stack or a non-recursive parser. Per-task stack guards
  remain separate future work (spec 0002, H2).

## Decided in the 2026-11-13 session

- **Console input is a byte stream.** `console_read` blocks until at least one byte,
  returns what is available, and never waits for a line. No echo, no line assembly, no key
  events in the kernel; `Enter`/`Backspace`/`Tab`/`Ctrl+C` map to `\n`/`0x7F`/`\t`/`0x03`
  and other non-text keys are discarded. The console is unowned (spec 0005).
- **One clock: nanoseconds since boot.** A global 64-bit counter is advanced by the boot
  core's 1 kHz APIC timer tick and read by every core; the timer is calibrated against the
  PIT/HPET. The TSC is not used, because in QEMU TCG it is per-vCPU and can drift, which
  would corrupt budget and heartbeat accounting under work stealing. Resolution is 1 ms
  (specs 0005, 0007).
- **Compile diagnostics are a caller-owned out-parameter.** `run`/`spawn` take
  `var diag: Diagnostic`, filled on `Status.Invalid` and cleared otherwise; `Diagnostic`
  gained `message: string[96]`. No global error slot, no diagnostic stream
  (specs 0003, 0005).
- **No debug/introspection primitive and no DMA/block primitive in M1** (Q8, Q9 stay open
  with later triggers).
- **Streams: one sender and one receiver, names unique application-wide.** A many-to-many
  stream would be a new shared endpoint type, not a change to the FIFO; duplicate names are
  a compile error. `stdlib` frames with a `u32` length prefix by default and offers
  newline-delimited helpers for text. Explicit close is deferred to streams v2, with the
  constraint that closed state is independent of the owner's lifetime and that `close`
  drains before `Closed` (spec 0006, S11).
- **Floating point is in the first milestone.** One type, `f64` (IEEE-754 binary64), with
  a kernel-owned 512-byte `fpu` region per task; `FXSAVE` on release, `FXRSTOR` on claim,
  no lazy FPU, no x87, no FMA. Explicit `T(x)` casts only; float division by zero is
  `Inf`/`NaN`; `int(f)` faults `Overflow` on `NaN` or out-of-range. Text conversion is
  fixed-decimal in M1 (ADR 0018, specs 0002–0004, 0007).
- **The language has no visibility modifiers.** Every top-level declaration is public;
  there is no `private`, `protected` or `public` (Q16, spec 0003 M5).
- **Conversions are `T(x)` casts.** Widening to `f64` is IEEE conversion; narrowing
  truncates and faults `Overflow` when it does not fit. `string`/`bytes` conversion stays a
  checked `stdlib` function returning `Status` (Q17).
- **Maps are FNV-1a with linear probing.** Power-of-two table of at least `2N`,
  backward-shift deletion, deterministic per build, iteration order unspecified (Q18).
- **Observability stops at `Diagnostic`.** No stack trace in M1; frames carry no debug
  metadata (Q19).
- **Comparisons bind tighter than `and`/`or`.** Both are boolean-only and short-circuit;
  bitwise operations are `core.bit_*`/`core.shl`/`core.shr`, and a shift count outside
  `0..63` faults `IllegalOperation` (Q20).
- **Bytecode format moved to its own spec.** `0008-bytecode.md` covers encoding, constant
  pool, frames, semantic splits, verification and ABI versioning (Q21).
- **Delivery is source-only, permanently.** The image never carries bytecode and a package
  exists only in memory for one instance; compile latency is not a design constraint
  (ADR 0019).
- **Request limits.** A `run`/`spawn` path is at most 256 bytes and relative to the image
  root (Q22); a stream name is `string[32]` matching `[a-z][a-z0-9_]*`, and a stream
  capacity is `1..=65536` bytes (Q23, spec 0006 S12).
- **Boot and platform.** APs are started with INIT-SIPI-SIPI through a real-mode trampoline
  below 1 MiB, with `MAX_CORES` static per-core state and a bounded wait that logs
  `IoError` and continues. Wake-up uses one fixed IPI vector (0x40) with fixed delivery and
  the all-excluding-self shorthand. The multiboot2 framebuffer tag is mandatory: without
  it the kernel writes `no framebuffer` to serial and halts (Q24, Q26, Q30).
- **ext2 subset.** 1 KiB and 4 KiB blocks, revision 1, direct/single/double indirect,
  regular files in the image root, exact case-sensitive matching, and full metadata
  validation before use. Unsupported features and triple indirect refuse the mount; there
  is no partial mount (Q27, spec 0002).
- **Scheduler and REPL.** The dispatcher runs before every claim with at most one request
  per opportunity; realtime admission counts the cores online at call time and does not
  revisit a refusal; `ExitReason` gained `Killed`, and `kill` cancels a queued request or
  terminates a live task at its next safe point; request paths are relative with no `..` and
  must name regular files; a shell may start another shell; a full queue is reported, not
  retried (Q31–Q33, Q36–Q38).
- **Quantum and total budget are provisional (Q1/Q2).** 10 000 bytecodes per claim; 10 s of
  CPU time plus a hard cap of 200 000 000 executed bytecodes, because the 1 ms clock cannot
  measure a sub-millisecond quantum. The step cap is the effective watchdog until the clock
  is finer. Tune after measurement (spec 0007, "Quantum" and "Budget").
- **Already decided, removed from the table:** Q25 (timer source, closed by Q6), Q28 and
  Q29 (ADR 0017 addendum), Q34 (children continue), Q35 (64-entry exit ring).
- Q5, Q6, Q7, Q11–Q13, Q15–Q38 and Q42 are resolved and removed from the table; Q1 and Q2
  are now closed provisionally, so only the handler budget and heartbeat (Q3/Q4), the
  deferred debug/DMA/streams items, the staged port handles (Q45) and the two float
  follow-ups remain.

## Decided in the 2026-11-12 session

These are no longer open; they were recorded in specs and ADRs:

- Origin and goal: 2005 modem front-end → general personal system horizon (spec 0001).
- Work-stealing interpreter loops, no core pinning (ADR 0011).
- Rust, x86_64-first, GRUB/multiboot2, read-only ext2 (ADR 0012–0015).
- Realtime class with heartbeat and `WatchdogTimeout`; no reserved core (ADR 0016).
- REPL as a kernel service; `shell` then `entrypoint`; `run`/`spawn`/`wait`/`kill`;
  global task ids; fixed `string[1024]` args delivered via `main(args)`; 1024-slot
  request queue (ADR 0017 and addenda).
- Language: Pascal-like, nominal types, no user generics, `bytes`/`string`, built-in
  `array`/`ring`/`map`, `Status` errors, `core`/`stdlib`/`syslib`, streams + `yield`
  (spec 0003).
- Memory: no first-class slices, one `state` region, per-instance code, compiler
  alignment, build-parameter capacities (spec 0004).
- Streams: byte FIFO, copied, atomic sends, immediate discard on peer death (spec 0006).

## Decided in the 2026-11-14 stdlib session

- **The stdlib is real source compiled with the application.** `stdlib/stdlib.ig` is
  embedded with `include_str!` (never precompiled, ADR 0019). An application with
  `uses stdlib;` compiles both units into one package: library functions first in the
  function table (entry is the app's `main`), one constant pool and instruction section,
  line-table unit ids 0 (app) / 1 (stdlib), and the manifest keeps the app's unit name.
  Names are unit-scoped: the app sees `stdlib.<name>` only; the library sees its own names
  unqualified and may import only `core`. Type ids stay per-unit, so records, sums and
  user enums may not cross the boundary yet (spec 0008).
- **Functions.** `function Name(...): T;` with `return value;` is implemented for scalar
  results; a function must end in a `return` on every path, and a procedure may `return;`.
  The caller pushes a destination reference before the arguments and the callee writes the
  result there, using the interpreter's existing `dest_ref`/`has_destination` support (no
  new opcode; spec 0003, spec 0008).
- **First helpers.** `trim_into` (destination `string[256]`), `split_once` (128/256),
  `parse_int_into` (`Status`, overflow-checked, never faults), `starts_with` and
  `contains_byte`. Concrete capacities replace generics; callers satisfy the at-least-N
  rule. The library adds no `state`, ~5.3 KiB of source and ~4.1 KiB of code blob, and
  ~154 KiB of compile scratch is enough for the sample app plus the library.
- **Arrays and `split_all` (2026-11-14).** `array[T, N]` is a fixed-size value with
  nominal element/count identity, `N` a positive constant, layout `N * align8(element
  size)`, `a[i]` checked at run time against `N` (`fault(OutOfBounds)`, never a clamp),
  whole-array copies through `copy_refs`, and one opcode `ref_element` (ABI v1 61 → 62).
  Arrays may be state, locals, parameters, record fields and sum payloads. One structural
  array table is shared by the application and `stdlib`, so arrays cross the unit
  boundary; `stdlib.split_all` fills an `array[string[64], 8]`.
- **Still missing for a complete stdlib:** `ring`/`map` (language support and helpers),
  UTF-8 helpers (`utf8_decode_into`, code-point iteration), moving `substr_into` from
  `core` to `stdlib`, `f64` formatting/parsing, typed stream views, and a shared
  cross-unit type table so library signatures can use records, sums and user enums.

## Suggested next step

Specs 0001–0008 are accepted and the boot skeleton runs (done 2026-11-13). Next:

1. ~~Bitmap-font framebuffer console~~ — done, mirrored to serial.
2. ~~IDT with an exception dump~~ — done; GDT/TSS and double-fault IST done.
3. ~~Frame allocator over the multiboot2 memory map~~ — done, 4 KiB bitmap.
4. ~~APIC timer and the global nanosecond clock~~ — done, 1 kHz tick calibrated
   against the PIT (Q6).
5. ~~Host-side language vertical slice~~ — done: `abi`, `compiler`, `interpreter`,
   `langhost`, 50 tests; ABI v1 opcode table fixed (spec 0008 OQ 1 and 3).
6. ~~Bounded-memory compiler~~ — done: `no_std`, allocation-free, `compile_into` writes into
   caller-provided arena/code/manifest buffers; exhaustion is `CapacityExceeded`.
7. ~~In-kernel REPL slice~~ — done: `xtask image` builds the ext2 `apps` module, the kernel
   mounts it read-only, compiles `shell` in the kernel, places `code`/`state`/`stack` frames,
   verifies and runs it, prints `exit: Ok`, reclaims and halts.
8. ~~Real scheduler loop~~ — done: a 64-slot global runnable set with claim/release by
   value move, a 10 000-step quantum, per-task budget accounting and a step-cap watchdog;
   the boot application crosses ~180 claims before `exit Ok`.
9. ~~Pick the tuning constants~~ — Q1/Q2 chosen provisionally: 10 000 steps, 10 s / 200M
   steps.
10. ~~REPL request service and blocking~~ — done: `core.spawn`/`wait`/`kill`, a 1024-slot
    request queue serviced before every claim, a 64-entry exit ring, blocked tasks skipped
    by `claim` and woken on child termination; the `shell` → `child` sample proves the
    path. The primitives return `Status` and fill the caller's `Diagnostic` on `spawn`
    and the `ExitReason` sum type on `wait` (Q43 is now `core.run`, `TaskId` and the
    request-completion diagnostic path).
11. ~~Console input~~ — done: COM1 and the PS/2 keyboard are polled in the service
    opportunity, a kernel-staged line ring echoes/edits, and `core.read_line` blocks the
    shell until a line exists (Q44; spec 0005).
12. ~~Buffer indexing, comparison and length~~ — done: `buf[i]` reads a raw `byte` checked
    against the runtime length, `bytes[i] := b` writes one byte checked against the capacity
    (extending the length and zero-filling the gap), `string` is read-only by index, `=`/`<>`
    compare buffers of the same kind across capacities, and `core.len` returns `uint`
    (`load_u8`, `store_u8`, `cmp_buffers`; spec 0003, spec 0008).
13. ~~String search, slicing and command arguments~~ — done: `core.find` and the staged
    `core.substr_into` (`find`/`substr` opcodes, 57 → 59 in ABI v1) give the shell `cmd`/`rest`
    splitting and `echo <text>`; `run <app> [args]` separates the tail at its first space and
    `child` prints the received `main(args)`. The host `read_line` stub scripts
    `help`, `run child world`, `echo hi there`, `exit`. Q44 still stands: the kernel keeps
    the staged 4-line ring until `stdlib` string editing (length/truncate/insert) exists.
14. ~~Integer formatting and the first `syslib` ports~~ — done: `core.format_int_into`
    (`format_int`, 59 → 60; decimal/hex, sign, zero-padding, exact fit or `-6`, bad base
    `-4`) and `syslib.port_in8`/`port_out8` (builtin ids 7/8, `Host` defaults, kernel over
    `arch_x86_64::port`). `samples/apps/clock` polls the CMOS RTC update flag, reads
    seconds/minutes/hours as BCD and prints `HH:MM:SS`; the host mock prints `00:00:00`.
    `Port` is staged as a scalar `uint` (Q45). Next for `syslib`: opaque `Port`/`MmioRegion`
    types, MMIO mapping and bounds checks, then DMA and block access. On the language side:
    `stdlib` text helpers and sum types; then APs, the realtime class and streams (Q3/Q4 when
    their features land).
15. ~~Kernel stack bound for compilation~~ — done (Q42): a fixed 32-level compiler nesting
    limit with a diagnostic, plus a dedicated 256 KiB guarded kernel stack for everything
    after early boot (one not-present frame, 2 MiB page split to 4 KiB PTEs, `#PF` on its
    own IST). The boot stack is now early-boot only. Per-task stack guards stay future work.
16. ~~Enums and the `Status` ABI~~ — done: `type Name = (Variant, ...)` with nominal
    types, ordinals `0..n-1`, same-enum assignment/comparison and ordinal ordering,
    `int(enum)` and range-checked `EnumName(int)` (`fault(Overflow)`, opcode `enum_cast`,
    ABI v1 60 → 61). `Status` is predeclared (`Ok` = 0). `core.spawn(path, args, var id)`,
    `core.wait(id, var reason)` and `core.kill(id)` return `Status`; the shell checks them
    and decodes the staged reason int.
17. ~~Records and the record-based REPL ABI~~ — done: `type Name = record field: T; ... end;`
    with nominal types, 8-byte-aligned declaration-order layout, nested records, buffer
    fields, whole-record copies (`copy_refs`), field references (`ref_offset`) and
    `value`/`const`/`var` record parameters. `Diagnostic` and `ExitReason` are predeclared
    at the fixed `abi::layout` offsets; `core.spawn(path, args, var diag, var id)` fills
    `diag` on a synchronous failure and clears it on success, and `core.wait(id, var reason)`
    writes the reason's tag/payload. Sum types and `case` landed in the same session, so
    the shell matches `ExitReason` with an exhaustive `case` and reports `Faulted`'s
    `FaultCode` payload. Q43 is now `core.run`, `TaskId` and the request-completion
    diagnostic path.
18. ~~`stdlib` start and multi-unit compilation~~ — done: `stdlib/stdlib.ig` is embedded
    source compiled into the application package (`uses stdlib;`), with library-first
    function indices, a shared constant pool, line-table unit ids (0 app, 1 stdlib) and
    unit-named diagnostics. `function ... : T` with `return` landed, and the first helpers
    are `trim_into`, `split_once`, `parse_int_into`, `starts_with` and `contains_byte`.
    `samples/lang/stdlib.ig` runs on the host and `run text 42 hi` runs in the kernel.
    Next on the language/stdlib track: rings/maps and UTF-8 helpers, then the
    `substr_into` move and cross-unit record/sum types. After that: APs, the realtime class
    and streams.
19. ~~Fixed-size arrays~~ — done: `array[T, N]` (positive const N) in state, locals,
    records, sum payloads and `value`/`const`/`var` parameters; nominal in element type and
    count; layout `N` elements at `align8(element size)`; `a[i]` read/write checked against
    `N` (`fault(OutOfBounds)`), nested `a[i][j]`/`a[i].field`, whole-array `copy_refs` and
    one new opcode `ref_element` (ABI v1 61 → 62). A structural array table is shared by
    the app and `stdlib`; `stdlib.split_all` fills `array[string[64], 8]`.
    `samples/lang/array.ig` and `array_bounds.ig` cover the slice. Next on this track:
    `ring` and `map`, which reuse the element-reference and copy machinery.
