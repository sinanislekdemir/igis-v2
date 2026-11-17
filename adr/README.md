# Architecture Decision Records

A numbered log of significant technical decisions. Each ADR captures the context, the
decision, and the alternatives that were rejected — the reasoning that is otherwise
lost once code exists.

## Rules

- **Immutable once accepted.** Do not rewrite an accepted ADR. If the decision changes,
  write a new ADR and mark the old one `Superseded by NNNN`.
- **One decision per ADR.** "Use Rust and a microkernel" is two ADRs.
- **Record the losers.** The rejected options and *why* are the most valuable part.
- **Small is fine.** A page is enough; this is not a design document.
- ADRs do not restate requirements — link to the spec instead.

## Status values

`Proposed` · `Accepted` · `Rejected` · `Deprecated` · `Superseded by NNNN`

## Numbering

Four digits, monotonic, never reused: `0007-uefi-boot.md`.

## Index

| ID | Title | Status | Date |
|----|-------|--------|------|
| 0001 | [Single privilege level (ring 0 only)](0001-single-privilege-ring0.md) | Accepted | 2026-11-03 |
| 0002 | [Applications are interpreted source, not native binaries](0002-interpreted-applications.md) | Accepted | 2026-11-03 |
| 0003 | [Per-core cooperative scheduling with core-pinned tasks](0003-cooperative-pinned-tasks.md) | Accepted (core pinning superseded by 0011) | 2026-11-03 |
| 0004 | [Static, preallocated application memory](0004-static-application-memory.md) | Accepted | 2026-11-03 |
| 0005 | [Internal bytecode representation is permitted](0005-internal-bytecode.md) | Accepted | 2026-11-03 |
| 0006 | [Use MMU hardening at ring 0 (provisional)](0006-mmu-hardening.md) | Accepted (provisional) | 2026-11-03 |
| 0007 | [Fully static memory placement — no runtime allocation of any kind](0007-fully-static-placement.md) | Accepted | 2026-11-03 |
| 0008 | [Fatal faults invoke an application exit handler, then the task is reclaimed](0008-exit-handler.md) | Accepted (amended by 0010) | 2026-11-03 |
| 0009 | [Exit handlers may transfer control to an application recovery point](0009-exit-recovery-point.md) | Superseded by 0010 | 2026-11-03 |
| 0010 | [Separate exit (teardown) and fault (recovery) handlers; no retry](0010-exit-and-fault-handlers.md) | Accepted | 2026-11-03 |
| 0011 | [Per-core interpreter loops with work-stealing tasks (no core pinning)](0011-work-stealing-interpreters.md) | Accepted | 2026-11-12 |
| 0012 | [Implement the kernel and runtime in Rust](0012-rust-implementation-language.md) | Accepted | 2026-11-12 |
| 0013 | [x86_64 first, with a multi-architecture layout from day one](0013-x86_64-first-multi-arch.md) | Accepted | 2026-11-12 |
| 0014 | [Boot via GRUB and multiboot2, with a hand-written long-mode transition](0014-grub-multiboot2-boot.md) | Accepted | 2026-11-12 |
| 0015 | [Deliver applications on a read-only ext2 image over an in-memory block device](0015-read-only-ext2-delivery.md) | Accepted | 2026-11-12 |
| 0016 | [Realtime tasks run without quantum preemption under a heartbeat contract](0016-realtime-heartbeat.md) | Accepted | 2026-11-12 |
| 0017 | [The REPL is a kernel service; applications request runs through a spawn queue](0017-repl-spawn-service.md) | Accepted | 2026-11-12 |
| 0018 | [Floating point is in the first milestone: one `f64` type and a per-task FPU region](0018-floating-point.md) | Accepted | 2026-11-13 |
| 0019 | [Applications are always compiled from source at load; no precompiled packages, ever](0019-source-only-delivery.md) | Accepted | 2026-11-13 |
| 0020 | [The window server is a kernel `syslib` service over one shadow screen](0020-kernel-window-service.md) | Accepted | 2026-11-16 |
| 0021 | [Input is one machine-wide pointer and event queue owned by the kernel, with a composited cursor](0021-input-event-model.md) | Accepted | 2026-11-16 |
| 0022 | [Use an absolute USB HID tablet for the pointer, not a scaled relative mouse](0022-absolute-pointer-usb-tablet.md) | Accepted | 2026-11-16 |
| 0023 | [Drain PS/2 auxiliary data on IRQ12 through the I/O APIC, into a lock-free ring](0023-ps2-aux-irq12.md) | Accepted | 2026-11-16 |
| 0024 | [Windows are kernel table records with monotonic ids, content-size geometry and owner-checked lifecycle](0024-window-identity-ownership-focus.md) | Accepted | 2026-11-16 |
| 0025 | [Kernel text windows and an application-owned desktop shell](0025-kernel-text-windows-desktop-shell.md) | Accepted | 2026-11-16 |
| 0026 | [The standard library may import `syslib` for the UI layout layer](0026-stdlib-may-use-syslib.md) | Accepted | 2026-11-17 |
| 0027 | [File access for applications is stateless, root-only host builtins](0027-stateless-root-only-file-api.md) | Accepted | 2026-11-17 |
| 0028 | [One machine-wide terminal attached to a task-owned window](0028-one-terminal-attached-window.md) | Accepted | 2026-11-17 |
