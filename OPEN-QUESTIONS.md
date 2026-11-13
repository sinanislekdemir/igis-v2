# Open questions

Parked decisions for igis-v2. This is the working register; each spec's own
"Open questions" section remains authoritative for that document. When an item is
resolved, record the answer in the relevant spec or in a new ADR, then remove it here.

**Status:** implementation phase. Last updated: 2026-11-13.
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
| Q42 | Kernel stack bound for in-kernel compilation | the recursive compiler runs on the boot stack (now 64 KiB); needs a guard page or a bounded recursion depth before untrusted source is accepted | before the REPL takes `run` requests |

## B. Host interface and I/O

| # | Question | Options / recommendation | Trigger |
|---|----------|--------------------------|---------|
| Q8 | Debug/introspection primitive | shape open (task listing, counters, trace); none in M1 | after the shell exists |
| Q9 | DMA and block primitives | handle model fixed; no primitive in M1 | first real block device |
| Q39 | Console/stream multiplexing (`poll`/`select`) | none in M1; a task blocks on one source at a time | when an application must serve two input sources |
| Q43 | Staged scalar `spawn`/`wait`/`kill` ABI | `int` task ids, negated `Status`, `0`/`1`/`2 + fault_code` wait results; replace with `Status`/`Diagnostic`/`ExitReason` records when the language has enums/records (spec 0005, "Staged ABI") | before `stdlib` exposes `run`, and before console input builds a shell on top of it |
| Q44 | Staged kernel line discipline (`core.read_line`) | the kernel polls COM1/PS-2, echoes, assembles and stores lines in a 4-line ring; `core.read_line(var buf)` blocks until a line exists and truncates to the buffer. Replace with application-side editing when string length/truncate/insert operations exist: indexing alone reads and writes single bytes but cannot delete one (spec 0005, "Staged line discipline") | when `stdlib` string editing exists |

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
- Q5, Q6, Q7, Q11–Q13 and Q15–Q38 are resolved and removed from the table; Q1 and Q2 are
  now closed provisionally, so only the handler budget and heartbeat (Q3/Q4), the deferred
  debug/DMA/streams items and the two float follow-ups remain.

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
    path. The ABI is staged as scalars (Q43).
11. ~~Console input~~ — done: COM1 and the PS/2 keyboard are polled in the service
    opportunity, a kernel-staged line ring echoes/edits, and `core.read_line` blocks the
    shell until a line exists (Q44; spec 0005).
12. ~~Buffer indexing, comparison and length~~ — done: `buf[i]` reads a raw `byte` checked
    against the runtime length, `bytes[i] := b` writes one byte checked against the capacity
    (extending the length and zero-filling the gap), `string` is read-only by index, `=`/`<>`
    compare buffers of the same kind across capacities, and `core.len` returns `uint`
    (`load_u8`, `store_u8`, `cmp_buffers`; spec 0003, spec 0008). The shell parses `help`,
    `hello` and `exit` and the host `read_line` stub scripts them. Next: string slicing and
    `stdlib` text helpers so the kernel can keep its own line editor instead of the staged
    ring (Q44 still stands until length/truncate/insert operations exist), command
    arguments, then the record-based spawn ABI; then APs, the realtime class and streams
    (Q3/Q4 when their features land).
