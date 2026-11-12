# igis-v2

A second-generation, from-scratch operating system, written in Rust. The first version
(2005) was a bare-metal front-end for a quantum-cryptography modem; the second is a
general personal system built on the same thesis: the kernel is a language runtime that
owns the machine, applications are source text, and everything runs at ring 0 in one
address space.

x86_64 first, with architecture-specific code isolated so other ISAs can follow
([ADR 0013](adr/0013-x86_64-first-multi-arch.md)). Boot is GRUB + multiboot2 with a
hand-written long-mode transition ([ADR 0014](adr/0014-grub-multiboot2-boot.md)).
Applications arrive on a read-only ext2 image ([ADR 0015](adr/0015-read-only-ext2-delivery.md))
and are advanced by per-core interpreter loops that steal work; nothing is pinned
([ADR 0011](adr/0011-work-stealing-interpreters.md)).

The project is currently in the specification phase — there is no code yet.

| Document | Purpose |
|----------|---------|
| [`specs/0001-vision.md`](specs/0001-vision.md) | Start here: thesis, origin, goal, requirements. |
| [`AGENTS.md`](AGENTS.md) | Operational guide: conventions, toolchain, current phase. |
| [`specs/`](specs/README.md) | Normative specifications (what the system shall do). |
| [`adr/`](adr/README.md) | Architecture decision records (why, and what was rejected). |
| [`IDEAS.md`](IDEAS.md) | Unstructured inbox for ideas not yet worth designing. |
