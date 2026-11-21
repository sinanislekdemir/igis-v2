# 0056: Runtime display modes use the Bochs VBE dispi, not a real-mode BIOS trampoline

- **Status:** Accepted
- **Date:** 2026-11-21
- **Deciders:** project owner
- **Spec:** 0022 (runtime display modes and the Settings application)
- **Supersedes:** nothing

## Context

The desktop has always used the one framebuffer mode GRUB selected. A Settings
application needs to change the **real** mode, not scale a fixed logical desktop.
Three paths were considered:

1. **VBE BIOS `int 10h AX=4F02`.** The VBE services are real-mode BIOS calls. The kernel
   runs in long mode with paging on, so this needs a low-memory trampoline that leaves
   long mode, clears paging and LME, enters real mode, performs the interrupt and
   returns. A prototype was built and reached the VGA BIOS handler, but the return path
   depended on hidden segment descriptors (`CS.D`, `SS.B`) and on the BIOS preserving
   the 16-bit interrupt frame; QEMU kept the old hidden descriptors across real-mode
   null-selector loads, and the call looped at the interrupt. Making that path robust
   would need careful real-mode stack and descriptor management plus a BIOS-presence
   policy for UEFI boots.
2. **A boot-time GRUB mode request.** Change `gfxmode` and reboot. This changes the mode
   but cannot be called "runtime", requires persistence and a reboot, and the golden
   screenshots pin the current 1024×768 boot mode.
3. **Bochs VBE dispi.** QEMU's `std` VGA exposes VBE registers at I/O ports
   `0x1CE`/`0x1CF`. Writing resolution, bpp and enable changes the real mode from normal
   ring-0 code with paging on, without a real-mode transition. The device reports the
   applied geometry, so the kernel can verify the change. This is the interface QEMU
   itself uses for the `std` VGA.

## Decision

Runtime mode changes use the **Bochs VBE dispi** on the QEMU `std` VGA:

- The kernel offers a fixed preset list, programs the dispi, reads the geometry back
  and only then swaps the shadow.
- The new shadow is allocated before the device is touched; a failure leaves the old
  mode and image intact.
- The device and shadow are changed under the graphics lock; the window service then
  scales/clamps window origins and performs the existing full-stack repaint. The kernel
  System Log, sized from the screen, is closed and reopened at the new geometry at the
  bottom of the z-order so it cannot cover an application.
- The boot path still uses the GRUB mode. When the dispi is absent the feature reports
  itself unavailable; graphics keep working in the boot mode.

## Consequences

- The feature is real and immediate on the target machine (QEMU `std` VGA), with no
  reboot, no real-mode code and no BIOS dependency.
- The runtime path is QEMU-specific. A real VGA BIOS, UEFI GOP or another GPU needs a
  separate backend; spec 0022 leaves that out of scope rather than pretending the dispi
  is universal.
- The LFB aperture must be reserved conservatively (16 MiB, the largest preset) at boot
  so a later, larger shadow cannot overlap the framebuffer.
- A mode change discards the old shadow. Clients repaint because the service marks every
  window for an `Expose`; an app that ignores it keeps stale pixels until its next draw.
- The VBE BIOS trampoline is rejected for this slice. Its prototype and the low-memory
  blob are not part of the build; the reasoning is recorded here so a later non-QEMU
  backend does not repeat the dead end blindly.
