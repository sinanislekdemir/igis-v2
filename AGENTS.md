# AGENTS.md

Operational guide for humans and coding agents working in this repository.
Keep this file short and current. It points at documents; it does not duplicate them.

**Last updated:** 2026-11-03

## What this is

`igis-v2` is a from-scratch second version of a personal operating system originally
written around 2005–2006. No code from the original survives; only the design intent
does.

- **Primary language:** Rust (`no_std`, freestanding).
- **First architecture:** x86_64. All ISA-specific code is isolated so additional
  architectures can be added without touching the kernel core.
- **Central thesis:** the kernel is a programmable system — a language runtime plus
  standard library owning the machine. Applications are source code, not native
  binaries. Everything runs at ring 0 in one address space.
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
arch/<isa>/        # ISA-specific: paging, context switch, IDT, APIC, ...
platform/<machine>/ # machine-specific: memory map, device layout, boot protocol
kernel/            # arch-agnostic core
drivers/           # device drivers, registered through traits
libs/              # no_std support crates
abi/               # shared kernel/user ABI definitions
xtask/             # build, image, and QEMU orchestration
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
| Rust as the implementation language | pending ADR |
| x86_64 first, multi-arch layout from day one | pending ADR |
| All code runs at ring 0 in a single address space | ADR 0001 (Accepted) |
| Applications are interpreted source, not native binaries | ADR 0002 (Accepted) |
| One cooperative scheduler thread per core; tasks pinned; watchdog termination | ADR 0003 (Accepted) |
| Static preallocated application memory; no malloc, no GC | ADR 0004 (Accepted) |
| Internal bytecode permitted; no JIT | ADR 0005 (Accepted) |
| MMU hardening: RO kernel text, guard pages, NX | ADR 0006 (Accepted, provisional) |
| Fully static placement; no runtime allocation of any kind | ADR 0007 (Accepted) |
| Fatal faults call `fault(code)`; `exit(reason)` always runs at teardown; one recovery, no retry | ADR 0010 (current; supersedes 0009) |
| Boot via GRUB under QEMU | proposed; see vision spec OQ 11 |

## Open questions

See `specs/0001-vision.md` for the full table. Highest-leverage unresolved items:

| # | Question | Needed for |
|---|----------|------------|
| 2 | Which ideas from the original igis carry into v2 | vision spec |
| 4 | Application storage and delivery (ramdisk vs filesystem) | vfs scope |
| 5 | Self-hosting development vs cross-development | UI and tooling scope |
| 10 | Language identity: syntax, type system, memory model details | language spec |
| 13 | ~~Sub-allocation inside a fixed arena~~ | **Resolved** — fully static (ADR 0007) |
| 14 | Standard library in Rust or in the language | bootstrapping and speed |
| 16 | SMP enabled in milestone 1 or only designed for | boot and locking work |
| 19 | Streams: copy-through-kernel or shared ring buffers | IPC and syslib |

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
