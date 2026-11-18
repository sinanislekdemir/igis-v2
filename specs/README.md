# igis-v2 Specifications

Normative documents describing what igis-v2 **shall** do. Specs are living: they are
revised as understanding improves, and their history lives in git.

## Index

| ID   | Title | Status | Scope | Updated |
|------|-------|--------|-------|---------|
| 0001 | [Vision and guiding principles](0001-vision.md) | Accepted | whole system | 2026-11-12 |
| 0002 | [System architecture](0002-architecture.md) | Accepted | kernel, runtime, execution/memory model | 2026-11-13 |
| 0003 | [Language and standard library](0003-language.md) | Accepted | application language | 2026-11-13 |
| 0004 | [Application memory model](0004-memory-model.md) | Accepted | mm, loader, interpreter, language | 2026-11-13 |
| 0005 | [Host interface](0005-host-interface.md) | Accepted | core, syslib, interpreter, runtime | 2026-11-13 |
| 0006 | [Streams and inter-application IPC](0006-streams.md) | Accepted | mm, sched, core/stdlib, loader | 2026-11-13 |
| 0007 | [Scheduler](0007-scheduler.md) | Accepted | sched, arch timer/IPI, interpreter | 2026-11-13 |
| 0008 | [Bytecode format and the compiler/runtime split](0008-bytecode.md) | Accepted | abi, compiler, interpreter, repl | 2026-11-13 |
| 0009 | [Graphics, the shadow screen and the window service](0009-graphics-and-windows.md) | Accepted | syslib, kernel gfx/console, compiler, interpreter | 2026-11-16 |
| 0010 | [Input, the pointer and events](0010-input-and-events.md) | Accepted | syslib, kernel mouse/usb/pci/gfx/console/sched, arch i8042/ports, compiler, interpreter | 2026-11-16 |
| 0011 | [Windows](0011-windows.md) | Accepted | abi, compiler, interpreter, kernel window/gfx/task/sched, syslib | 2026-11-17 |
| 0012 | [The desktop shell and kernel text windows](0012-desktop-shell.md) | Accepted | kernel console/gfx/window/sched, compiler, interpreter, abi, samples/apps | 2026-11-17 |
| 0013 | [The application file API](0013-application-files.md) | Accepted | abi, compiler, interpreter, kernel ext2/sched/repl, langhost, samples/apps | 2026-11-17 |
| 0014 | [The terminal window](0014-terminal-window.md) | Accepted | abi, compiler, interpreter, kernel console/window/sched, langhost, samples/apps | 2026-11-17 |
| 0015 | [The proportional font](0015-proportional-font.md) | Implemented | abi, compiler, interpreter, kernel font/gfx/window/console, langhost, samples/apps | 2026-11-17 |
| 0016 | [Performance measurement](0016-performance.md) | Implemented | kernel window/console/gfx/clock, samples/apps/perf | 2026-11-17 |
| 0017 | [The system menu and system control](0017-system-menu.md) | Implemented | kernel sched/smp/main, arch reboot, abi, compiler, interpreter, langhost, samples/apps | 2026-11-17 |
| 0018 | [The image filesystem and importable library modules](0018-filesystem-and-modules.md) | Accepted | abi, compiler, interpreter, kernel ext2/repl/sched, langhost, xtask, samples/apps | 2026-11-17 |
| 0019 | [The writable home filesystem](0019-writable-home-filesystem.md) | Draft | ext2w, kernel ata/fs/repl/sched, abi, compiler, interpreter, langhost, xtask, samples/apps | 2026-11-18 |

## Lifecycle

    idea  ──▶  draft  ──▶  accepted  ──▶  implemented
                  │                         │
                  └──▶  withdrawn           └──▶  superseded by NNNN

- **Draft** — being written, not authoritative. Anything may change.
- **Accepted** — reviewed and agreed. Implementation may proceed against it.
- **Implemented** — the system matches the spec; changes now require a spec revision
  (or a new superseding spec).
- **Withdrawn** — abandoned before acceptance; kept for the record.
- **Superseded by NNNN** — replaced; kept as history, never deleted.

Only **Accepted** and **Implemented** specs are authoritative. If code and an accepted
spec disagree, that is a bug in one of them — fix it explicitly.

## Numbering and naming

- Four digits, assigned monotonically, never reused: `0042-scheduler.md`.
- Prefer short, specific titles: `page-table-isolation`, not `memory-management-part-3`.
- Keep the tree flat while it is small. Split into subdirectories
  (`kernel/`, `arch/`, `abi/`, `user/`) only when the flat list gets hard to scan.

## Required sections

Use `0000-template.md`. Every spec states:

1. **Status and scope** — where the rules apply (which crates, which architectures).
2. **Context** — the problem and constraints, including what we are *not* solving.
3. **Requirements** — numbered, testable, using MUST / SHOULD / MAY.
4. **Design** — the chosen approach and the important invariants.
5. **Interfaces** — the contracts other components depend on.
6. **Failure modes** — what happens when things go wrong.
7. **Open questions** — unresolved items, with an owner or a trigger for resolution.

## Requirements keywords

Use RFC 2119 semantics strictly:

- **MUST** — mandatory; violating it is a defect.
- **SHOULD** — expected, but a documented reason may justify deviation.
- **MAY** — genuinely optional.

## Relationship to ADRs

Specs say *what*. ADRs (`../adr/`) record *why* a significant choice was made and what
was rejected. A spec may reference an ADR; an ADR never restates a spec.

## Relationship to code

Every spec that is Accepted SHOULD name the crate(s) implementing it, so drift is
visible. Implementation is not started before the governing spec is Accepted.
