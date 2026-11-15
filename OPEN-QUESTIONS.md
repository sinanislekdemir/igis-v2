# Open questions

Parked decisions for igis-v2. This is the working register; each spec's own
"Open questions" section remains authoritative for that document. When an item is
resolved, record the answer in the relevant spec or in a new ADR, then remove it here.

**Status:** implementation phase. Last updated: 2026-11-15.
**Snapshot:** specs 0001–0008, ADRs 0001–0019.

## How to use

- One row per unresolved decision, grouped by area.
- `Trigger` says when it must be answered (before which milestone or component).
- A recommendation is a starting point, not a decision.

## A. Tuning constants (before the first boot)

| # | Question | Options / recommendation | Trigger |
|---|----------|--------------------------|---------|
| Q3 | Handler emergency budget | 1 ms or 10 000 steps for `fault`/`exit` | fault/exit implementation |

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
| Q10 | Explicit close | streams are implemented with close-on-termination only; explicit `close` stays v2. Closed state is independent of owner lifetime and `close` will drain before reporting `Closed`, while task death keeps discarding queued bytes | streams v2 |
| Q14 | Auto-wire a console stream for REPL apps | no in M1; applications use `core.console_*`; wiring needs a console application and an ownership rule | when a console application exists |

## D. Language

| # | Question | Options / recommendation | Trigger |
|---|----------|--------------------------|---------|
| Q40 | `f32` support | not in M1; additive later, but it needs its own opcode and conversion rules | when single precision is needed |
| Q41 | Float text beyond fixed decimal | shortest round-trip formatting and full decimal parsing are deferred | when float output must round-trip losslessly |
| Q46 | Moving the checked slice into `stdlib` | **Blocked, not pending.** `core.substr_into` cannot move: the destination capacity must be a *call-site immediate* (the `SUBSTR` `capacity` field), while a `stdlib` helper declares concrete capacities and the language has no generics, so one helper could serve only one destination size (it would lose `split_once`'s `string[128]` head and every smaller destination). The future option is a buffer parameter that carries its **runtime capacity**, which would need a language change. Until then the checked slice stays in `core` and there is one slicing primitive, not two (spec 0003, "Slicing and search") | only if runtime-capacity buffer parameters are added |

## Decided in the 2026-11-15 SMP session

**AP bring-up details (spec 0002 OQ 5) are resolved.** Discovery is by attempt, not by
ACPI/MADT: `MAX_CORES` is one constant (**8**, core index 0 is the boot core), and
`smp::start()` sends INIT plus two SIPIs to APIC ids `1..MAX_CORES` through a trampoline
copied to physical `0x8000` (SIPI vector 8) with a parameter page at `0x9000` (PML4,
higher-half entry, per-core stack tops, an atomic next-index counter). The waits are 10 ms
after INIT, ~200 µs between SIPIs and a bounded 100 ms for the online reports, so a
`-smp 1` or `-smp 2` machine boots with fewer cores instead of hanging. Each core has its
own TSS (kernel `RSP0`, double-fault IST, page-fault IST), a static 64 KiB kernel stack in
`.bss` (the boot core keeps its guarded 256 KiB one), and loads the shared IDT. Only the
boot core programs the timer; the clock stays one global counter. One IPI vector (`0x40`)
wakes idle cores: the producer publishes work, claims an idle core, sets `need_resched` and
sends the IPI; the handler only EOIs and the idle loop re-checks. The boot core's idle path
prints the per-core summary, sets a stop flag, IPIs every online core, waits bounded for
acknowledgements and halts.

The lane also settled two concurrency holes the single-core scheduler never showed: a
claimed task's slot is reserved by task id until it is reclaimed (a concurrent `spawn` can
no longer place a child in a running task's slot), and a reserved id stays "live" while its
request is being compiled, so `core.wait` cannot observe `NotFound` in the pop-to-spawn
window. `syslib.cpu_id()` (builtin 10) reports the executing core's APIC id — the proof
hook, not an affinity.

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
- Q5, Q6, Q7, Q11–Q13, Q15–Q38 and Q42 are resolved and removed from the table; Q1, Q2 and
  Q4 are now closed provisionally, so only the handler budget (Q3), the deferred
  debug/DMA items, explicit stream close (Q10), console-stream wiring (Q14), the staged
  port handles (Q45), the two float follow-ups (Q40/Q41) and the blocked `substr_into`
  move (Q46) remain.

## Decided in the 2026-11-15 realtime session

**Q4 — the heartbeat interval is fixed at one second.** A realtime task must call
`core.heartbeat()` at least once per second of non-blocked time. The interval is a
compile-time constant (`sched::HEARTBEAT_NS`, 1 000 000 000 ns); there is no per-task
configuration in the first milestone. The deadline is measured in the accumulated
claim-to-release time, so a blocked controller is not frozen, and a missed deadline
terminates the task with `Faulted(WatchdogTimeout)` at the next quantum boundary. The
admission slot stays held while a realtime task is blocked and is released only when the
task terminates, so the class never oversubscribes the machine.

**Time-sliced realtime beyond the core count is out of scope.** Admission allows at most
one realtime task per online core, and no core is reserved for normal tasks or the REPL
(ADR 0016 addendum). If more controllers must run than there are cores, the model needs a
different admission rule (realtime time slicing), which is a new decision.

## Decided in the 2026-11-15 streams session

- **Dynamic loading (spec 0006 amendment).** A stream endpoint is created when the task
  that declares it loads, before placement. With no live peer the stream *waits*: the
  buffer exists, a sender may fill it to capacity and a receiver blocks on it. A peer that
  arrives later with the same name and the opposite direction pairs; a second endpoint
  with the same name and direction, a capacity mismatch against a live peer, or a
  declaration against a closed stream is a load error (reported through `core.run`'s
  `diag`). This replaces S8's static "unmatched declaration is a load error" rule, which
  cannot work when tasks are created at run time. The failure table and the "Dynamic
  loading" subsection in spec 0006 are normative.
- **Handles and state layout.** The handle is `(generation << 8) | table_index`; a stale
  handle cannot reach a recycled stream. Endpoint `i` is at `state[i * 8]`, and the
  manifest lists descriptors in declaration order, so `abi::Endpoint` needs no offset
  field and `ENDPOINT_SIZE`/`MANIFEST_HEADER_SIZE` are unchanged.
- **Kernel storage.** One 32-record table over a static 256 KiB arena with a first-fit
  free list; buffers are kernel-owned and every send/receive copies. The table lock is
  never held while a slot lock is taken: operations record that a wake is needed, drop
  the stream lock, then call `task::wake_stream`.
- **Close means task death in this milestone.** `Task::reclaim` closes the owner's sides,
  discards queued bytes immediately (S7), wakes blocked peers with `Closed` and returns
  the buffer to the arena at the close; the record stays reserved by a surviving endpoint
  until its owner ends and never pairs again. Explicit `close` (Q10) and `stdlib`
  framing on top of streams remain future work; the closed state is already independent
  of owner lifetime, so the queue does not need reworking.
- **`stdlib` may not declare stream variables** (a library endpoint name would be shared
  by every application) but may take a `const` stream parameter; that is what the future
  framing helpers will use.

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
- **Still missing for a complete stdlib:** `f64` formatting/parsing, typed stream views,
  and a shared cross-unit type table so library signatures can use records, sums and user
  enums. The `substr_into` move is withdrawn (Q46): the destination capacity must be a
  call-site immediate. `map` support landed with `map_get_or` (below), and the UTF-8
  helpers landed in the 2026-11-15 session (below).
- **Rings (2026-11-14).** `ring[T, N]` is a fixed-capacity FIFO, nominal in element type
  and count, with the `N` elements first and `head: u64`/`count: u64` after them. The
  built-in `core.ring_push`/`pop`/`peek`/`full`/`empty` are compiler-recognised
  procedures lowered with the existing instruction set (`ref_offset`, `ref_element`,
  `copy_refs`, `add_u`/`mod_u`, `eq`+`jz`), so the ABI v1 opcode table is unchanged. `push` returns
  `Full` on a full ring and writes nothing; `pop`/`peek` return `NotFound` on an empty
  ring and leave the out-parameter untouched; `push`/`pop` require a writable place,
  so `const` is rejected and a `value` copy is writable but private to the callee. The structural ring table is shared with `stdlib`, whose `drain_into` takes
  a `ring[int, 8]`.
- **Maps (2026-11-15).** `map[K, V, N]` is a fixed-capacity open-addressing map, nominal
  in `(K, V, N)`: `N` a positive const, `K` one of `int`, `string[M]`, `bytes[M]`, `V` any
  fixed-size type. The image is `count: u64` then `capacity` (smallest power of two at
  least `2N`) slots of `state`/`hash`/key/value; FNV-1a, linear probing, backward-shift
  deletion with no tombstones. `core.map_put`/`get`/`has`/`remove`/`count` are five new
  opcodes (`MAP_PUT` … `MAP_COUNT`), not inlined per call site. `put` at `N` entries
  returns `Full` and writes nothing; a missing key is `NotFound`/`false`; `get` leaves its
  out-parameter untouched. One structural map table is shared with `stdlib`, whose
  `map_get_or` takes a `map[int, int, 8]`. The design was already fixed (Q18, "Maps are
  FNV-1a with linear probing"); this session implemented it.

## Decided in the 2026-11-15 UTF-8 session

- **Mixed-kind slicing lives in `core.substr_into`, with the destination deciding.** All
  four destination/source kind combinations are legal: a `string` destination from a
  `string` source keeps T4's code-point-boundary rule (including an empty range that starts
  inside a code point), a `string` destination from a `bytes` source validates the copied
  range as **strict UTF-8** (only 1- to 4-byte sequences; no overlongs, no surrogates, no
  values above `U+10FFFF`, no truncation, no stray continuation byte), and a `bytes`
  destination copies either source raw. `bytes` → `string` through this primitive is the
  checked conversion T3 names, so the `string` invariant is established at the single point
  where raw bytes can become text.
- **The source kind is the reserved `SUBSTR` `b` field** (`a` stays the destination kind,
  the `capacity` immediate is unchanged), so no opcode was added and the ABI v1 table is
  unchanged. The verifier accepts `a` and `b` in `0..=1` and requires `c = 0`.
- **The `stdlib.substr_into` move is withdrawn (Q46).** The destination capacity must be a
  call-site immediate; a concrete-capacity helper could only serve one destination size.
  The checked slice stays in `core`, and there is no second slicing API.
- **UTF-8 helpers are ordinary language code.** `stdlib.utf8_decode_into` (T3:
  `CapacityExceeded` when the source exceeds 256 bytes, `Invalid` for malformed input, both
  writing nothing), `utf8_len` (non-continuation bytes), `utf8_decode` (bounds-checked
  code-point decode; `Invalid` and nothing written at a bad index or a sequence that runs
  past the end), and `utf8_valid` (the strict validator, avoiding a conversion). No new
  opcode; the library grows by four functions.
- **A latent codegen bug was fixed on the way.** `dst := ""` emitted the destination
  reference for an `append` it then skipped, leaving one value on the run-time operand
  stack; a following call could then overflow a full operand area. Empty literals now add
  nothing after `clear` (covered by `samples/lang/utf8.ig` and a `langhost` regression
  test).

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
20. ~~Fixed-capacity rings~~ — done: `ring[T, N]` (positive const N) in state, locals,
    records, sum payloads, arrays and `value`/`const`/`var` parameters, nominal in element
    and count; layout is the `N` elements at `align8(element size)` followed by `head` and
    `count`; whole-ring copies include the header. `core.ring_push`/`pop`/`peek`/`full`/
    `empty` lower to `ref_offset`/`ref_element`/`copy_refs`/`clear`+`append` plus
    `add_u`/`mod_u` and `eq`+`jz`, adding no opcode (the opcode table is unchanged). `samples/lang/
    ring.ig` covers FIFO order, peek, full/empty transitions, wrap-around, copies, every
    element kind and the stdlib `drain_into` helper. Next: `map`, then UTF-8 helpers.
21. ~~Fixed-capacity maps~~ — done: `map[K, V, N]` (positive const N, `K` = `int`,
    `string[M]` or `bytes[M]`) in state, locals, records, sum payloads, array and ring
    elements and `value`/`const`/`var` parameters; nominal in `(K, V, N)`; `count` header
    then `capacity` (next power of two at least `2N`) slots of state/hash/key/value;
    FNV-1a, linear probing, backward-shift deletion. Five new opcodes (`MAP_PUT`,
    `MAP_GET`, `MAP_HAS`, `MAP_REMOVE`, `MAP_COUNT`) because the algorithm is not inlined
    per call site. `put` at `N` returns `Full`; a missing key is `NotFound`/`false`; `get`
    leaves its out-parameter untouched. The structural map table is shared with `stdlib`
    (`map_get_or`), and `samples/lang/map.ig` covers collisions, backward shift, key 0,
    the empty buffer key, aggregate values, whole copies and the three parameter modes.
22. ~~UTF-8 helpers and mixed-kind slicing~~ — done: `core.substr_into` takes all four
    destination/source kind combinations, with the destination deciding (boundaries for a
    `string` source, strict UTF-8 validation for a `bytes` source, a raw copy for a `bytes`
    destination). The source kind is the `SUBSTR` `b` field, so the opcode table is
    unchanged. `stdlib.utf8_decode_into` (T3), `utf8_len`, `utf8_decode` and `utf8_valid`
    are ordinary language code; `samples/lang/utf8.ig` covers 1-/2-/3-/4-byte code points
    and every malformed class, and `samples/apps/utf8` runs as `run utf8`. The
    `stdlib.substr_into` move is withdrawn (Q46). Next: APs (INIT-SIPI-SIPI and the
    per-core interpreter loops), then the realtime class and streams. Remaining on the
    language/host track: `f64` text conversion and the cross-unit type table.
23. ~~Streams~~ — done (spec 0006, amended): `in_stream[N]`/`out_stream[N]` top-level
    state variables with handles in the `state` prefix and manifest descriptors in
    declaration order; load-time create-or-pair with waiting semantics; a 32-record
    kernel table over a 256 KiB first-fit arena; four `stream_*` opcodes; blocking via
    `WaitFor::Stream(handle)` with send/receive/close wake-ups; termination discards
    queued bytes and frees the buffer when no side remains. `samples/apps/streams`
    (`pipe_in` + `pipe_out`) proves in-order delivery, sender blocking and `Closed`;
    `streams_bad` proves the duplicate-direction and capacity-mismatch load errors; the
    `bad_send`/`bad_recv`/`bad_name` samples prove the compile errors. Next: `stdlib`
    framing helpers (`send_line`, `recv_line_into`, length-prefixed frames) on top of the
    new primitives, then explicit close (Q10), `f64` text, and per-task stack guards.
