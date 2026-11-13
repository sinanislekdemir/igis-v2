# Open questions

Parked decisions for igis-v2. This is the working register; each spec's own
"Open questions" section remains authoritative for that document. When an item is
resolved, record the answer in the relevant spec or in a new ADR, then remove it here.

**Status:** specification phase, no code. Last updated: 2026-11-13.
**Snapshot:** specs 0001–0007, ADRs 0001–0017.

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
| Q10 | Explicit close | none in M1; needed for long-lived services | streams v2 |
| Q11 | Multiple senders/receivers | no; would require an MPMC queue | streams v2 |
| Q12 | Stream name collisions across units | unit prefix or per-manifest namespace? | loader pairing |
| Q13 | Framing helpers | length prefix, delimiter, or both; `stdlib` | `stdlib` |
| Q14 | Auto-wire a console stream for REPL apps | after a console application exists | loader |

## D. Language

| # | Question | Options / recommendation | Trigger |
|---|----------|--------------------------|---------|
| Q15 | Floating point | `f32`/`f64` in M1, or integer-only? | language implementation |
| Q16 | Unit visibility | all top-level declarations public, or a `private` section | language implementation |
| Q17 | Explicit conversions | `int(x)`, checked casts, or `core.to_*` | compiler |
| Q18 | Map hash and probe details | deterministic per build; iteration order unspecified | `stdlib` |
| Q19 | Diagnostics beyond `Diagnostic` | stack trace? source snippet? | language/runtime |
| Q20 | Operator precedence; integer `and`/`or` | parser and readability | compiler |
| Q21 | Bytecode format and compiler/runtime split | implementation plan; not part of the language | before the interpreter |
| Q22 | Path capacity for `run`/`spawn` | `string[256]`? fixed capacity in the manifest? | host interface |
| Q23 | Stream endpoint and name lengths | manifest and compiler constants | loader |

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
- Q5, Q6 and Q7 are resolved and removed from the table.

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
