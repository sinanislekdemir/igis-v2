# 0052: Group the kernel by subject and put PC devices behind a platform seam

- **Status:** Accepted
- **Date:** 2026-11-20
- **Deciders:** project owner
- **Related specs:** spec 0002 (architecture), spec 0018 (image and modules), spec 0019 (home)
- **Related ADRs:** ADR 0013 (x86_64 first), ADR 0015 (in-memory ext2 image)

## Context

`kernel/src` grew to one flat directory of 25 files and roughly 16.7k lines. It held
the scheduler, the task table, the frame allocator, the stack guards, two filesystems,
the path router, the framebuffer shadow, two fonts, the console, the window server, the
REPL, the stream table, the event queue, and the PC device drivers (PCI, ATA, UHCI/HID,
PS/2 aux) side by side. Finding a subsystem meant scanning a flat listing, and the file
names gave no hint about which layer a module belonged to.

ADR 0013 says architecture-specific code is isolated. That is true for the instruction
set (boot, paging, IDT/GDT, APIC/IOAPIC, PIT, ports, serial, keyboard scancodes live in
`arch/x86_64`), but device discovery and port I/O are machine-specific, not ISA-specific,
and they had leaked into the kernel crate: `ata`, `mouse`, `pci`, `usb` and the AP
start-up path all import `arch_x86_64` for ports or paging. The "planned top-level shape"
in AGENTS.md named `platform/`, `drivers/`, `mm/`, `sched/`, `vfs/` and `core/`, but none
of those directories existed.

Splitting the kernel into separate crates now was rejected: the modules share static
tables and locks, several pairs are mutually recursive (scheduler ↔ task, window ↔
graphics ↔ console), the kernel has no host test harness, and the compiler is already
the expensive part of every build. The value we need today is navigability and an honest
boundary, not compile-time isolation.

## Decision

We will keep one `kernel` crate and organize `kernel/src` by subject:

```
kernel/src/
  main.rs, clock.rs, event.rs, mb2.rs, module.rs, repl.rs, stream.rs, sync.rs, net/
  platform/x86_64/   ata, mouse, pci, smp, usb     (PC devices and AP bring-up)
  mm/                frame, guard                  (frames and stack guards)
  sched/             mod.rs (the loop), task       (claim, quantum, budget)
  ui/                console, font, font8x8, gfx, window
  vfs/               ext2, fs                      (image reader and path router)
```

The crate root re-exports the moved modules (`pub(crate) use ...`) so the existing
`crate::gfx`, `crate::task` and `crate::ata` paths keep working; moving a file does not
rewrite every call site. `sched/mod.rs` is the scheduler loop itself and declares
`sched/task.rs` as a child, so `crate::sched::run` is unchanged.

`arch/x86_64` stays the instruction-set and boot layer. `platform/x86_64` is the machine
layer and is the seam for a future `platform` crate: it is the only place that may own
PC device ports, PCI configuration space and the AP trampoline.

The two ext2 readers stay separate for now. `vfs/ext2` reads the in-memory GRUB image
through a borrowed slice; `ext2w` owns the writable home disk and caches blocks in a
`RefCell`. `RefCell` is not `Sync`, so handing an `ext2w::Fs` to the APs would be
unsound. Unification needs a `Sync` cache (or a different cache strategy) first and is
parked as Q59 in `OPEN-QUESTIONS.md`.

## Consequences

### Positive

- The tree states the layers: a new reader can find the scheduler, the window server or
  the PC drivers without reading `main.rs`'s module list.
- Device I/O has one home (`platform/x86_64`); the rest of the kernel no longer grows
  port-I/O imports.
- The boot service and the subject directories are small enough to review on their own.
- No behavior change: module paths are preserved by crate-root re-exports.

### Negative

- The crate is still one compilation unit; a change to any module still rebuilds the
  kernel. This is accepted for now.
- The re-exports mean both `crate::gfx` and `crate::ui::gfx` resolve, so the old flat
  name remains part of the internal API. New code should use the grouped path.
- `sched` is both the group and the loop module, which is slightly subtle; the loop lives
  in `sched/mod.rs` and the task table in `sched/task.rs`.

### Neutral / follow-up work

- A later `platform` crate can take `platform/x86_64` wholesale behind a small device
  trait; that is the point of the seam.
- Extract pure arithmetic (damage coalescing, font metrics, path rules already in `abi`)
  into a host-testable crate when it next changes.
- Revisit the two ext2 readers (Q59) if a third filesystem or a cache bug appears.

## Alternatives considered

| Alternative | Why not |
|-------------|---------|
| Split `kernel` into `kernel-core`, `platform`, `drivers`, `vfs` crates now | Shared statics, recursive module pairs, no host harness for the kernel, and a large build/verification cost for no behavior gain |
| Keep the flat directory and only update the documentation | The documentation would be honest, but the navigation cost stays and the device/ISA boundary remains invisible |
| Use `#[path = ...] mod` to relocate files while leaving module names alone | Less churn, but the directory tree would not reflect the module tree; real submodules are clearer |
| Move the PC drivers into `arch/x86_64` | They are machine-specific, not ISA-specific; a future ARM port keeps PCI/ATA/USB concepts but not x86 port I/O |
| Unify the ext2 readers in the same change | `ext2w`'s cache is a `RefCell` and cannot be shared with APs; that is a separate decision with its own design |
