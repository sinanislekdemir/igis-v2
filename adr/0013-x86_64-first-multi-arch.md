# 0013: x86_64 first, with a multi-architecture layout from day one

- **Status:** Accepted
- **Date:** 2026-11-12
- **Deciders:** project owner
- **Related specs:** 0001 (vision), 0002 (architecture)

## Context

The system targets QEMU first (vision R6, OQ 9). x86_64 is the machine the owner knows,
the one with the most documentation and the simplest QEMU story, and the one where the
boot path (GRUB + multiboot2 + a hand-written long-mode transition) is well trodden.

At the same time, the architecture independence of applications is a stated benefit:
applications are source text evaluated by the interpreter, so they are portable for free
(vision R3, ADR 0002). That benefit is only real if the kernel itself is structured so
that ISA-specific code cannot leak into the interpreter, the scheduler or the language.
Retrofitting that structure after the fact is expensive; doing it from the first commit
costs little more than naming the crates correctly.

## Decision

- **x86_64 is the first and only supported ISA in the first milestone.** QEMU is the only
  target (OQ 9 resolved).
- The tree is split from day one:
  - `arch/<isa>` — boot entry, GDT/IDT/TSS, paging, context switch, APIC/timer, SMP
    bring-up. Architecture-specific by definition.
  - `platform/<machine>` — memory map, ACPI, device discovery, machine quirks. Machine
    specific, but not ISA specific.
  - `kernel/`, `interpreter/`, `sched/`, `mm/`, `drivers/`, `stdlib/`, `syslib/`,
    `abi/`, `libs/` — arch-agnostic. No `cfg(target_arch)` branches, no inline assembly,
    no knowledge of page-table formats or interrupt vectors.
- The build target is `x86_64-unknown-none`; the only architecture-specific build
  configuration lives in `arch/x86_64` and `xtask`.
- Adding a second ISA must not require changes outside `arch/<isa>`, `platform/<machine>`
  and the build orchestration. That is a structural requirement, not a promise to
  implement a second ISA.
- The ISA contract is deliberately small: boot handoff, memory mapping primitives,
  interrupt entry, per-core context, port/MMIO access and SMP bring-up. Everything else
  is generic.

## Consequences

### Positive

- The interpreter, scheduler and language never grow `#[cfg(target_arch)]` branches.
- Applications remain portable across architectures by construction, matching the thesis
  rather than contradicting it.
- The x86_64 details that are easy to leak (page-table formats, interrupt frames, MSRs)
  are confined to one crate and can be reviewed as such.
- A second architecture, if ever attempted, has a defined place to land.

### Negative

- Some abstractions exist with only one implementation, which is speculative structure
  and can be wrong until a second ISA tests it.
- The `platform/` and `arch/` boundary is not always obvious (for example, ACPI is
  machine-specific but the MADT is architecture-specific), so the split needs discipline.
- Early code pays a small indirection cost for operations that could be inlined per
  target.

### Neutral / follow-up work

- The crate names and the exact `arch` contract are fixed when the workspace is created;
  this ADR only fixes the boundary and the first ISA.
- Any second ISA requires its own ADR describing the boot handoff and the per-core model.

## Alternatives considered

| Alternative | Why not |
|-------------|---------|
| ARM64 first | Comparable support in QEMU, but a less familiar boot and interrupt model for the owner, and no advantage for the first milestone. |
| RISC-V first | Clean and simple ISA, but the QEMU and firmware story is less mature for this use, and the owner's experience is x86. |
| Single-architecture forever, no layout | Cheapest now, but makes the portability claim empty and guarantees a costly refactor if a second ISA is ever wanted. |
| Abstract the ISA before any implementation | Premature abstraction with no second implementation to validate it; the chosen split is the minimum needed to keep the boundary honest. |
