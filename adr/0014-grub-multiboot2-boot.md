# 0014: Boot via GRUB and multiboot2, with a hand-written long-mode transition

- **Status:** Accepted
- **Date:** 2026-11-12
- **Deciders:** project owner
- **Related specs:** 0001 (vision), 0002 (architecture)

## Context

Vision R6 already requires that the system boots under QEMU via GRUB from a single
documented command. That fixes the loader family but not the firmware path or the entry
mode:

- GRUB has a BIOS variant and a UEFI variant. Both are GRUB; the choice of firmware is
  orthogonal to the loader.
- The multiboot2 protocol hands the kernel control in **32-bit protected mode**,
  regardless of which GRUB variant loaded it. A 64-bit kernel must therefore enable long
  mode, build page tables, and enter the higher half itself.
- The alternatives that boot a 64-bit kernel directly (Limine, BOOTBOOT, a hand-written
  UEFI stub) would remove that work, but they either replace the loader mandated by R6 or
  move a substantial amount of firmware-specific code into the trusted base.

The project owner chose to keep GRUB/multiboot2 and own the transition, on the grounds
that the transition is a bounded, well-documented piece of architecture code and that
GRUB is the least surprising dependency for the first milestone.

## Decision

- **GRUB 2 with multiboot2**, running on QEMU's default BIOS firmware, is the only boot
  path in the first milestone.
- The kernel is a multiboot2 ELF loaded by GRUB and entered in 32-bit protected mode.
- The 32-bit stub in `arch/x86_64`:
  1. validates the multiboot2 information structure,
  2. builds identity and higher-half page tables,
  3. enables PAE, long mode and paging,
  4. jumps to the 64-bit entry point in the higher half.
- GRUB is asked for a **linear framebuffer** via the multiboot2 framebuffer tag; the
  console driver uses it (see OQ 8 resolution), and serial remains the debug channel.
- The read-only ext2 application image is delivered as a **GRUB module** and read through
  an in-memory block device (ADR 0015).
- UEFI, real-hardware firmware paths and a second loader are out of scope. The boot code
  is confined to `arch/x86_64` so a later UEFI path can be added without touching the
  kernel core (ADR 0013).

## Consequences

### Positive

- The most trodden x86 boot path: GRUB, multiboot2 and a long-mode stub are extensively
  documented and easy to debug with QEMU and GDB.
- R6 is satisfied without inventing a loader or depending on a less common protocol.
- A single `qemu-system-x86_64 -cdrom ...` style command is enough for the development
  loop; no firmware image or ESP setup is needed.
- The 32-bit stub is small, isolated, and a natural place to enforce early invariants
  (no interrupts, no stack use beyond a fixed scratch, no allocation).
- GRUB can produce a hybrid BIOS/UEFI image later if that is ever wanted; the kernel-side
  contract does not change.

### Negative

- We own the long-mode transition, page-table setup and higher-half mapping before the
  first line of kernel Rust runs. This is real assembly and real debugging.
- The first instructions run with a 32-bit address space, so the kernel layout must be
  chosen to survive the transition (link address, load address, direct map).
- multiboot2 is a GRUB-era protocol; its structures are pointers into low memory and must
  be copied or consumed before that memory is reused.
- BIOS assumptions (VBE framebuffer, no UEFI runtime services) are baked into the first
  milestone.

### Neutral / follow-up work

- The linker script, the multiboot2 header and the 64-bit entry point are the first
  files in `arch/x86_64`.
- UEFI support, if added, is a new boot path behind the same `arch` entry contract; it
  requires its own ADR.
- The framebuffer geometry is taken from the multiboot2 tag; a fallback to VGA text is
  not planned.

## Alternatives considered

| Alternative | Why not |
|-------------|---------|
| Limine | Boots a 64-bit kernel directly with paging already on, but replaces the loader required by R6 and makes a third-party protocol part of the boot contract. |
| Hand-written UEFI stub | Full control and a 64-bit entry, but adds firmware services, a FAT ESP image and a second boot path to maintain before the kernel exists. |
| BOOTBOOT | 64-bit entry and a small protocol, but less active than GRUB/Limine and still a third-party dependency. |
| GRUB with the UEFI variant | Possible, but multiboot2 still enters in 32-bit protected mode, so the long-mode work remains; it buys nothing for the first milestone. |
| Direct QEMU `-kernel` with a custom protocol | Removes GRUB but violates R6 and bakes a QEMU-specific loader into the design. |
