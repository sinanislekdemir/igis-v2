# AGENTS.md

Operational guide for humans and coding agents working in this repository.
Keep this file short and current. It points at documents; it does not duplicate them.

**Last updated:** 2026-11-12

## What this is

`igis-v2` is a from-scratch second version of a personal operating system whose first
version was written in 2005 as a front-end for a quantum-cryptography modem. It began as
a Linux driver, the user/kernel ring boundary got in the way, and it grew into a kernel
with its own boot path. No code survives; the continuity is design intent and the
constraints learned there.

- **Primary language:** Rust (`no_std`, freestanding).
- **First architecture:** x86_64. All ISA-specific code is isolated so additional
  architectures can be added without touching the kernel core (ADR 0013).
- **Central thesis:** the kernel is a programmable system — a language runtime plus
  standard library owning the machine. Applications are source code, not native
  binaries. Everything runs at ring 0 in one address space.
- **Execution model:** one interpreter loop per core; tasks are claimed from a global
  runnable set and are never pinned to a core (ADR 0011).
- **Long-term goal:** a general personal system. The first milestone proves the thesis
  with a booting kernel, source applications, read-only ext2 delivery, and a REPL that
  can drive a device.
- **Status:** specification. No code yet, and none is written until the governing spec is
  accepted.

## Ground rules

1. **Spec before code.** Behavior changes are described in `specs/` and accepted before
   implementation. If reality and an accepted spec disagree, fix one of them explicitly.
2. **ADRs for decisions, specs for behavior.** A lasting constraint or technology choice
   gets an ADR in `adr/`. See `adr/README.md` for the immutability rule.
3. **Ideas are free.** Capture them in `IDEAS.md`; promote them when they matter.
4. **Don't invent requirements.** Ambiguity is resolved by asking, not by assumption.
   Record the answer in the relevant spec.
5. **Keep this file true.** Update the phase, status table, and toolchain notes when they
   change. Anything that grows beyond a few lines belongs in a spec or ADR.

## Repository layout

```
.
├── AGENTS.md          # this file: operational facts and conventions
├── IDEAS.md           # unstructured inbox
├── adr/               # decision log (immutable once accepted)
├── specs/             # normative specifications (living)
└── src/ ...           # (to be created once the architecture spec is accepted)
```

Planned top-level shape once code starts — architecture-specific code must not leak
into the core:

```
arch/<isa>/         # ISA-specific: boot, long mode, paging, stack switch, IDT, APIC, ...
platform/<machine>/ # machine-specific: memory map, device layout, boot protocol
kernel/             # arch-agnostic core
interpreter/        # parse/compile, bytecode execution, safe points
core/               # native primitives (Rust): console, streams, time, text
stdlib/             # standard library written in the application language
syslib/             # privileged hardware primitives (ports, MMIO, DMA)
drivers/            # framebuffer console, keyboard, timer, block
mm/                 # frame allocator, task regions, stream buffers
sched/              # per-core loops, runnable set, claim/release, budgets
vfs/                # read-only ext2 over BlockDevice, later writable filesystems
abi/                # shared kernel/runtime ABI definitions
libs/               # no_std support crates
xtask/              # build, image, and QEMU orchestration
```

## Toolchain

| Item | Version / value |
|------|-----------------|
| stable (default) | rustc 1.99.0 |
| nightly | rustc 1.101.0-nightly (2026-11-02) |
| targets installed | `x86_64-unknown-linux-gnu`, `x86_64-unknown-none` |
| extra components | `rust-src` (nightly), required for `-Z build-std` |

No build commands exist yet. Once a workspace is created, document the canonical
commands here (`cargo xtask build`, `cargo xtask run`, `cargo xtask test`).

## Document conventions

- Specs: `specs/NNNN-kebab-title.md`, template `specs/0000-template.md`, index in
  `specs/README.md`. Status must be one of Draft / Accepted / Implemented / Withdrawn /
  Superseded.
- ADRs: `adr/NNNN-kebab-title.md`, template `adr/0000-template.md`, index in
  `adr/README.md`.
- Requirements are numbered and use MUST / SHOULD / MAY (RFC 2119).
- Dates are ISO 8601 (`YYYY-MM-DD`).
- Work in small commits; one logical change per commit. Do not commit unrelated edits.

## Decisions so far

| Decision | Record |
|----------|--------|
| Rust as the implementation language | ADR 0012 (Accepted) |
| x86_64 first, multi-arch layout from day one | ADR 0013 (Accepted) |
| All code runs at ring 0 in a single address space | ADR 0001 (Accepted) |
| Applications are interpreted source, not native binaries | ADR 0002 (Accepted) |
| Per-core interpreter loops; tasks claimed from a global set, never pinned | ADR 0011 (Accepted; supersedes pinning in 0003) |
| Cooperative safe points, quantum, total CPU budget | ADR 0003 + ADR 0011 |
| Static preallocated application memory; no malloc, no GC | ADR 0004 (Accepted) |
| Internal bytecode permitted; no JIT | ADR 0005 (Accepted) |
| MMU hardening: RO kernel text, guard pages, NX | ADR 0006 (Accepted, provisional) |
| Fully static placement; no runtime allocation of any kind | ADR 0007 (Accepted) |
| `fault(code, diag)` decides, `exit(reason, diag)` always tears down; one recovery, no retry | ADR 0010 (current; supersedes 0009) |
| Boot via GRUB + multiboot2 under QEMU, with own long-mode transition | ADR 0014 (Accepted) |
| Applications delivered on a read-only ext2 image over an in-memory block device | ADR 0015 (Accepted) |
| No capability model; every application may use every `syslib` primitive | vision R14 / spec 0001 |
| Framebuffer console with a bitmap font; no windowing | vision OQ 8 |
| Byte-FIFO streams, copied through the kernel, atomic sends | spec 0006 (Draft) |
| Pascal-like language, nominal types, no user generics | spec 0003 (Draft) |
| `core` (Rust) / `stdlib` (language) / `syslib` (hardware) split | specs 0002, 0003 |
| `bytes` (raw) and `string` (UTF-8) as distinct types | spec 0003 |
| One task per application instance; REPL-only in the first milestone | specs 0001, 0007 |
| Realtime class: no quantum preemption, one-second heartbeat, admission by core count, no reserved core | ADR 0016 (Accepted) |
| REPL is a kernel service; boot tries `shell` then `entrypoint`; `run`/`spawn` carry `string[1024]` args through a 1024-slot queue | ADR 0017 (Accepted + addendum) |

## Open questions

See `specs/0001-vision.md` for the closed table and the individual specs for the rest.
Highest-leverage unresolved items:

| # | Question | Needed for |
|---|----------|------------|
| 1 | Quantum size and total budget value | tuning after the first workloads |
| 2 | Console read granularity: bytes, lines, or key events | host interface |
| 3 | Explicit stream close; multiple endpoints per name | streams |
| 4 | Floating point (`f32`/`f64`) in the first milestone | language |
| 5 | Unit visibility and private declarations | language |
| 6 | Bytecode format and compiler/runtime split | implementation |
| 7 | AP bring-up details: trampoline, IPI, per-core stacks | arch |
| 8 | ext2 subset edge cases and corruption handling | loader |
| 9 | Handle table size (8?) and whether `run` blocks on a full queue | REPL service |
| 10 | Whether the argument string is `core.arguments` or a `main` parameter | language |

## Status log

| Date | Change |
|------|--------|
| 2026-11-03 | Repository created; documentation structure established. |
| 2026-11-03 | Vision captured in spec 0001; ADR 0001 (ring 0 only) and ADR 0002 (interpreted apps) accepted. |
| 2026-11-03 | Architecture spec 0002, language seed 0003; ADR 0003 (pinned cooperative tasks), 0004 (static memory), 0005 (bytecode), 0006 (MMU hardening, provisional). |
| 2026-11-03 | Memory model draft 0004; placement fork (static vs monotonic regions) left open. |
| 2026-11-03 | Option A selected: fully static placement, no runtime allocation (ADR 0007). Memory spec updated with rules, examples, and accepted costs. |
| 2026-11-03 | Destination-form construction recorded: `dst = a + b` writes into a declared buffer; no temporaries, no GC. |
| 2026-11-03 | `exit(reason)` as top-level catch: frames abandon to main level, handler may reclaim or transfer to a validated recovery label (ADR 0008 amended by 0009); aliasing forbidden, no views, capacity-typed strings. |
| 2026-11-03 | Handlers split: `fault(code)` decides terminate/recover, `exit(reason)` always tears down; one recovery per task, no retry (ADR 0010, supersedes 0009). |
| 2026-11-12 | Origin recorded: 2005 front-end for a quantum-cryptography modem; the ring boundary was the friction that produced ring 0. Execution changed to per-core interpreter loops with work stealing (ADR 0011). Rust, x86_64-first, GRUB/multiboot2 and read-only ext2 recorded (ADR 0012–0015). Specs 0001–0004 updated; host interface (0005), streams (0006) and scheduler (0007) added. |
| 2026-11-12 | Realtime class added: one-way promotion, no quantum preemption, admission by core count, one-second heartbeat with `WatchdogTimeout` (ADR 0016). |
| 2026-11-12 | Realtime admission deliberately allows occupying every core; no core is reserved for the REPL. Tradeoff recorded in the ADR 0016 addendum. |
| 2026-11-12 | REPL model settled: a kernel service compiles and starts applications from a request queue; the shell is an ordinary application; `run`/`spawn`/`join`/`kill` with task handles (ADR 0017). |
| 2026-11-12 | REPL details settled: `shell` then `entrypoint`, `no entry` halt, fixed `string[1024]` arguments, queue capacity 1024, orphan children continue (ADR 0017 addendum). |
