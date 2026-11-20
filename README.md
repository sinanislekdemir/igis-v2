# igis-v2

A second-generation, from-scratch operating system written in Rust. The first version
(2005) was a bare-metal front-end for a modem; the second is a
general personal system built on the same thesis: the kernel is a language runtime that
owns the machine, applications are source text compiled in the kernel, and everything runs
at ring 0 in one address space.

x86_64 first, with architecture-specific code isolated so other ISAs can follow
([ADR 0013](adr/0013-x86_64-first-multi-arch.md)). Boot is GRUB + multiboot2 with a
hand-written long-mode transition ([ADR 0014](adr/0014-grub-multiboot2-boot.md)).
Applications arrive on a read-only ext2 image ([ADR 0015](adr/0015-read-only-ext2-delivery.md))
and are advanced by per-core interpreter loops that claim work from a global set; nothing
is pinned ([ADR 0011](adr/0011-work-stealing-interpreters.md)). The shell is itself an
application: a kernel REPL service compiles it at boot and starts it, and applications
start others through a `run`/`spawn` request queue ([ADR 0017](adr/0017-repl-spawn-service.md)).

**Status: implementation.** Specs 0001–0020 are accepted; milestones M2–M10 are complete
(desktop, language, filesystems, networking), 608 host tests pass, and ABI v1 has 75
opcodes. See [`AGENTS.md`](AGENTS.md) for the current state and
[`PLAN.md`](PLAN.md) for what is next.

| Document | Purpose |
|----------|---------|
| [`specs/0001-vision.md`](specs/0001-vision.md) | Start here: thesis, origin, goal, requirements. |
| [`AGENTS.md`](AGENTS.md) | Operational guide: current state, conventions, toolchain, layout. |
| [`PLAN.md`](PLAN.md) | Milestone plan, ticket backlog and deferred items. |
| [`specs/`](specs/README.md) | Normative specifications (what the system shall do). |
| [`adr/`](adr/README.md) | Architecture decision records (why, and what was rejected). |
| [`docs/HISTORY.md`](docs/HISTORY.md) | Long-form narrative, decision index and dated status log. |
| [`OPEN-QUESTIONS.md`](OPEN-QUESTIONS.md) | Parked decisions and unresolved questions. |
| [`IDEAS.md`](IDEAS.md) | Unstructured inbox for ideas not yet worth designing. |
