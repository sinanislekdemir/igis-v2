# Open questions

Parked decisions for igis-v2. This is the working register; each spec's own
"Open questions" section remains authoritative for that document. When an item is
resolved, record the answer in the relevant spec or in a new ADR, then remove it here.

**Status:** specification phase, no code. Last updated: 2026-11-13.
**Snapshot:** specs 0001–0007, ADRs 0001–0018.

## How to use

- One row per unresolved decision, grouped by area.
- `Trigger` says when it must be answered (before which milestone or component).
- A recommendation is a starting point, not a decision.

## A. Tuning constants (before the first boot)

| # | Question | Options / recommendation | Trigger |
|---|----------|--------------------------|---------|
| Q1 | Quantum size | 1 ms or 10 000 bytecodes; compile-time constant; measure later | first scheduler implementation |
| Q2 | Total CPU budget | 10 s default; compile-time constant; blocked time excluded | first scheduler implementation |
| Q3 | Handler emergency budget | 1 ms or 10 000 steps for `fault`/`exit` | fault/exit implementation |
| Q4 | Realtime heartbeat interval | 1 s default; configurable per task? | realtime implementation |

## B. Host interface and I/O

| # | Question | Options / recommendation | Trigger |
|---|----------|--------------------------|---------|
| Q8 | Debug/introspection primitive | shape open (task listing, counters, trace); none in M1 | after the shell exists |
| Q9 | DMA and block primitives | handle model fixed; no primitive in M1 | first real block device |
| Q39 | Console/stream multiplexing (`poll`/`select`) | none in M1; a task blocks on one source at a time | when an application must serve two input sources |

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

## E. Boot and platform

| # | Question | Options / recommendation | Trigger |
|---|----------|--------------------------|---------|
| Q24 | AP bring-up details | trampoline vs INIT-SIPI-SIPI; per-core stacks | second half of M1 |
| Q25 | Timer source | PIT / HPET / APIC timer | `arch/x86_64` |
| Q26 | IPI mechanism | fixed vector vs APIC shorthand | `arch/x86_64` |
| Q27 | ext2 subset edge cases | 1 KiB/4 KiB blocks, indirect depth, corrupt images | loader |
| Q28 | `shell` exists but fails to compile | fall back to `entrypoint`, or halt? Current: halt | REPL boot |
| Q29 | `no entry` behaviour | halt vs reboot vs built-in prompt | boot |
| Q30 | Framebuffer fallback | no VGA fallback; is the multiboot2 tag mandatory? | boot |

## F. Scheduler and REPL

| # | Question | Options / recommendation | Trigger |
|---|----------|--------------------------|---------|
| Q31 | Service dispatcher timing | before every claim, or only when the runnable set is empty | scheduler |
| Q32 | Realtime admission when APs come online late | count online cores? re-check blocked admissions? | AP bring-up |
| Q33 | `kill(id)` before the task is created | cancel the queued request, or mark it killed on creation | REPL |
| Q34 | Parent death with live children | continue (decided); no reparent and no kill-tree | future supervisor |
| Q35 | Exit-reason ring size and eviction | 64 default; oldest evicted | REPL |
| Q36 | Path validation | relative to the image root; directories allowed? `..` rejected? | REPL |
| Q37 | Shell/entrypoint recursion | is starting `shell` from `shell` allowed? | REPL |
| Q38 | Spawn queue full retry policy | `Status.Full` (decided); does the shell retry or report? | shell |

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
- **Bytecode format moved to its own spec.** `0008-bytecode.md` will cover encoding,
  constant pool, frames, the compiler/runtime split and ABI versioning (Q21).
- **Request limits.** A `run`/`spawn` path is at most 256 bytes and relative to the image
  root (Q22); a stream name is `string[32]` matching `[a-z][a-z0-9_]*`, and a stream
  capacity is `1..=65536` bytes (Q23, spec 0006 S12).
- Q5, Q6, Q7, Q11–Q13 and Q15–Q23 are resolved and removed from the table.

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

1. Review and mark specs 0001 and 0002 **Accepted** (they are the governing documents).
2. Create the workspace and the first bootable skeleton: linker script, multiboot2
   header, 32-bit stub, long-mode transition, 64-bit entry, serial + framebuffer console.
3. Pick the tuning constants (Q1–Q4) when the scheduler first runs.
