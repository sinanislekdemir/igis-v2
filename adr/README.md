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
| 0011 | [Per-core interpreter loops with work-stealing tasks (no core pinning)](0011-work-stealing-interpreters.md) | Accepted (budget reset amended by 0061) | 2026-11-12 |
| 0012 | [Implement the kernel and runtime in Rust](0012-rust-implementation-language.md) | Accepted | 2026-11-12 |
| 0013 | [x86_64 first, with a multi-architecture layout from day one](0013-x86_64-first-multi-arch.md) | Accepted | 2026-11-12 |
| 0014 | [Boot via GRUB and multiboot2, with a hand-written long-mode transition](0014-grub-multiboot2-boot.md) | Accepted | 2026-11-12 |
| 0015 | [Deliver applications on a read-only ext2 image over an in-memory block device](0015-read-only-ext2-delivery.md) | Superseded by 0059 | 2026-11-12 |
| 0016 | [Realtime tasks run without quantum preemption under a heartbeat contract](0016-realtime-heartbeat.md) | Accepted | 2026-11-12 |
| 0017 | [The REPL is a kernel service; applications request runs through a spawn queue](0017-repl-spawn-service.md) | Accepted (boot order superseded by 0029) | 2026-11-12 |
| 0018 | [Floating point is in the first milestone: one `f64` type and a per-task FPU region](0018-floating-point.md) | Accepted | 2026-11-13 |
| 0019 | [Applications are always compiled from source at load; no precompiled packages, ever](0019-source-only-delivery.md) | Accepted | 2026-11-13 |
| 0020 | [The window server is a kernel `syslib` service over one shadow screen](0020-kernel-window-service.md) | Accepted | 2026-11-16 |
| 0021 | [Input is one machine-wide pointer and event queue owned by the kernel, with a composited cursor](0021-input-event-model.md) | Accepted | 2026-11-16 |
| 0022 | [Use an absolute USB HID tablet for the pointer, not a scaled relative mouse](0022-absolute-pointer-usb-tablet.md) | Accepted | 2026-11-16 |
| 0023 | [Drain PS/2 auxiliary data on IRQ12 through the I/O APIC, into a lock-free ring](0023-ps2-aux-irq12.md) | Accepted | 2026-11-16 |
| 0024 | [Windows are kernel table records with monotonic ids, content-size geometry and owner-checked lifecycle](0024-window-identity-ownership-focus.md) | Accepted (canvas clause superseded by 0060) | 2026-11-16 |
| 0025 | [Kernel text windows and an application-owned desktop shell](0025-kernel-text-windows-desktop-shell.md) | Accepted | 2026-11-16 |
| 0026 | [The standard library may import `syslib` for the UI layout layer](0026-stdlib-may-use-syslib.md) | Accepted | 2026-11-17 |
| 0027 | [File access for applications is stateless, root-only host builtins](0027-stateless-root-only-file-api.md) | Accepted | 2026-11-17 |
| 0028 | [One machine-wide terminal attached to a task-owned window](0028-one-terminal-attached-window.md) | Superseded by 0053 | 2026-11-17 |
| 0029 | [Boot into the desktop; launch the shell from its Terminal item](0029-boot-to-desktop.md) | Accepted | 2026-11-17 |
| 0030 | [A kernel-owned 12-pixel proportional bitmap font with a lazy glyph cache](0030-proportional-font.md) | Accepted | 2026-11-17 |
| 0031 | [Coalesce damage before repair and batch console cells into one present](0031-damage-coalescing-present-batching.md) | Accepted | 2026-11-17 |
| 0032 | [Visual regression uses exact host-side framebuffer goldens](0032-visual-regression-goldens.md) | Accepted | 2026-11-17 |
| 0033 | [System control has no capability check and uses focus-scoped shortcuts](0033-system-control-authority.md) | Accepted | 2026-11-17 |
| 0034 | [Kill requests are id-tagged per-slot records resolved under the slot lock](0034-id-tagged-kill-records.md) | Accepted | 2026-11-17 |
| 0035 | [Correctly rounded float parsing is a `core` interpreter primitive](0035-correctly-rounded-float-parsing.md) | Accepted | 2026-11-17 |
| 0036 | [Multi-source waits are one `WaitSet` condition with readiness probes](0036-multi-source-waits.md) | Accepted (source `0` refined by 0037) | 2026-11-17 |
| 0037 | [The console is a raw byte stream; the application owns echo and line editing](0037-console-byte-stream.md) | Accepted | 2026-11-17 |
| 0038 | [A hierarchical read-only image and compile-time `/lib` modules](0038-hierarchical-image-and-modules.md) | Accepted | 2026-11-17 |
| 0039 | [Nominal type identity is package-wide; `/lib` types are unit-qualified](0039-package-wide-nominal-types.md) | Accepted | 2026-11-18 |
| 0040 | [A real ext2 writer on a dedicated ATA home disk behind a path router](0040-real-ext2-writer-home-disk.md) | Accepted | 2026-11-18 |
| 0041 | [Add a truncate primitive to the home file API](0041-home-truncate.md) | Accepted | 2026-11-18 |
| 0042 | [Window drag and close are service-owned pointer interactions](0042-service-owned-window-drag-and-close.md) | Accepted (close semantics amended by 0043) | 2026-11-18 |
| 0043 | [A close-box press is a close request, never a task kill](0043-close-is-a-request.md) | Accepted | 2026-11-18 |
| 0044 | [A modal popup input grab makes an app-drawn menu reachable over windows](0044-modal-popup-input-grab.md) | Accepted | 2026-11-18 |
| 0045 | [A persistent pointer-region claim protects the menu bar's opening click](0045-persistent-pointer-region-claim.md) | Superseded by 0060 | 2026-11-18 |
| 0046 | [A topmost move copies a content snapshot; the client re-reads the content rectangle](0046-move-content-snapshot-and-client-rect-refresh.md) | Superseded by 0060 | 2026-11-18 |
| 0047 | [Deliver queued input to the owner of its window target](0047-window-addressed-input.md) | Accepted | 2026-11-18 |
| 0048 | [Keep the IPv4 wire stack in the kernel and expose a `/lib/socket` unit](0048-kernel-net-stack-and-lib-socket.md) | Accepted | 2026-11-19 |
| 0049 | [Start with virtio-net behind a device seam, not a virtio-specific stack](0049-virtio-net-first-device-seam.md) | Accepted | 2026-11-19 |
| 0050 | [Sockets are owned opaque handles with blocking waits and deadlines](0050-socket-handles-and-blocking.md) | Accepted | 2026-11-19 |
| 0051 | [Acknowledge the NIC on its own INTx vector and keep the ring drain in the service opportunity](0051-nic-intx-acknowledge-and-poll-fallback.md) | Accepted | 2026-11-19 |
| 0052 | [Group the kernel by subject and put PC devices behind a platform seam](0052-kernel-module-layout-and-platform-seam.md) | Accepted | 2026-11-20 |
| 0053 | [A terminal emulator over kernel console sessions](0053-terminal-emulator-console-sessions.md) | Accepted | 2026-11-20 |
| 0054 | [Minimize is a service-owned window state restored through the window list](0054-service-owned-minimize.md) | Accepted | 2026-11-20 |
| 0055 | [A switch or a completed move rebuilds the window stack in the service](0055-service-full-repaint-on-switch-and-move.md) | Superseded by 0060 | 2026-11-20 |
| 0056 | [Runtime display modes use the Bochs VBE dispi, not a real-mode BIOS trampoline](0056-runtime-display-mode-bochs-vbe.md) | Accepted | 2026-11-21 |
| 0057 | [PNG row streaming with an Adam7 pass-1 preview](0057-png-row-streaming-preview.md) | Accepted (amended 2026-11-22) | 2026-11-21 |
| 0058 | [Ship the image corpus as uncompressed PPM P6 and keep PNG support](0058-ppm-raw-corpus.md) | Accepted | 2026-11-22 |
| 0059 | [Boot only the kernel and mount the application image from an ATA disk](0059-boot-only-the-kernel.md) | Accepted | 2026-11-22 |
| 0060 | [Retain every window's content and compose the screen from surfaces](0060-per-window-canvas-and-service-compositor.md) | Accepted (implemented M16; local coordinates deferred) | 2026-11-25 |
| 0061 | [The normal-task budget covers one continuous run, reset on block](0061-per-run-normal-task-budget.md) | Accepted | 2026-11-26 |
| 0062 | [Pre-rasterize the icon set to 32×32 PPM P6](0062-pre-rasterized-ppm-icons.md) | Accepted | 2026-11-27 |
