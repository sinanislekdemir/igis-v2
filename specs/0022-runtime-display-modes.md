# 0022: Runtime display modes and the Settings application

- **Status:** Implemented
- **Scope:** `abi`, `compiler`, `interpreter`, `kernel` (`display`, `gfx`, `window`,
  `frame`), `arch/x86_64` (Bochs VBE dispi), `samples/apps` (`settings`,
  `desktop_shell`), `scripts`
- **Supersedes:** none
- **Superseded by:** none
- **ADRs:** 0056
- **Updated:** 2026-11-21

## Context

The bootloader chooses one framebuffer mode (GRUB requests 1024×768×32 for the
kernel) and every later service inherits it. Spec 0009 deliberately assumed no
specific mode, but it had no way to change the mode after boot. The user asked for a
Settings application that changes the real screen resolution, not a scaled desktop.

QEMU's `std` VGA exposes the Bochs VBE dispi register pair at I/O ports `0x1CE`/`0x1CF`.
Writing horizontal resolution, vertical resolution, 32 bpp and the virtual dimensions,
then enabling the linear framebuffer, changes the real device mode without a BIOS call.
The earlier VBE BIOS path (`int 10h AX=4F02` through a low-memory real-mode trampoline)
was implemented far enough to prove the transition, then rejected: the BIOS interrupt
return and hidden segment descriptors made it fragile, while the dispi path is
deterministic on the target machine.

This specification defines the runtime mode surface, the selection rules, the screen
rebuild and the Settings application. It does not specify an arbitrary-resolution
model, a persistent mode preference, or a non-QEMU backend.

## Requirements

- **D1 (MUST):** The initial mode is still whatever the bootloader returns. Nothing may
  assume 1024×768; `syslib.screen_width()`/`screen_height()` always report the current
  shadow geometry.
- **D2 (MUST):** Runtime modes are a fixed device-supported preset list. The kernel
  accepts a preset only when the Bochs VBE dispi is present and the device reports the
  requested geometry after programming it.
- **D3 (MUST):** Only 32 bpp direct-colour modes are selectable, matching the graphics
  module. The presets are 640×480, 800×600, 1024×768, 1280×720, 1280×1024, 1440×900,
  1600×1200 and 1920×1080.
- **D4 (MUST):** `syslib.display_mode_count()` returns the number of selectable modes,
  or `0` when the runtime interface is absent.
- **D5 (MUST):** `syslib.display_mode_width(index)` and `display_mode_height(index)`
  return the preset's dimension, or `0` for an out-of-range index.
- **D6 (MUST):** `syslib.set_resolution(width, height)` returns `Ok` only after the
  device and the shadow agree on the new mode. `NotFound` means the geometry is not a
  preset; `Invalid` means graphics are disabled or the arguments cannot be a mode;
  `Full` means the new shadow cannot be allocated; `IoError` means the device rejected
  the mode.
- **D7 (MUST):** A change allocates the new shadow (`pitch × height` contiguous frames)
  **before** the device is programmed, so a memory failure cannot leave the machine in
  a mode the kernel cannot draw into. On success the old shadow is freed after the swap;
  on failure the new shadow is freed and the old screen, device mode and window table
  are untouched.
- **D8 (MUST):** The device programming and the shadow swap execute while the graphics
  lock is held, so no other core can draw into the old image or present it with the new
  framebuffer between the two. The pointer is clamped to the new bounds and the clip is
  reset to the whole screen.
- **D9 (MUST):** The window service rebuilds the stack after a successful change: frame
  origins are scaled by the new/old screen ratio and clamped so each frame starts
  on-screen; content sizes are unchanged; a stale drag and a stale modal grab are
  dropped; the persistent region claim is clamped; the full-repaint flag is set (W31)
  and every live window is marked for an `Expose`. The kernel System Log is derived
  from the screen, so the console closes and reopens it at the new geometry at the
  **bottom** of the z-order (its previous diagnostics are cleared); it never covers an
  application after a mode change.
- **D10 (MUST):** The desktop shell re-reads `screen_width`/`screen_height` before it
  draws the menu bar and re-claims its pointer region when the size changed.
- **D11 (MUST):** At boot, when the runtime interface is present, the whole linear
  framebuffer aperture the largest preset can use (16 MiB from the boot framebuffer
  base) is reserved from the frame allocator before the first shadow is allocated. A
  later shadow must never overlap the LFB.
- **D12 (MUST):** No bytecode opcode is added. The four operations are host builtins
  behind `BUILTIN` (ids 80–83), with disabled-host defaults.
- **D13 (MUST):** Settings is an ordinary application at
  `apps/utilities/settings` and is listed under `Utilities` in the Apps pull-down.
  It lists the modes, selects the current one, applies a row and reports failures.
- **D14 (MUST):** `run settings check` lists the modes, sets 800×600, verifies the
  shadow size, restores 1024×768, verifies again and prints `settings: ok`.
  `run settings shot` sets 800×600 before opening its window, prints
  `settings: ready`, and holds for the screenshot harness.
- **D15 (MUST):** The proof includes the kernel transcript, a QMP screenshot of the
  shell app at exactly 800×600 (`scripts/settings_resolution_qmp.py`), and a QMP run
  that opens `Apps > Settings`, applies 800×600 from the boot desktop and captures
  exactly 800×600 (`scripts/settings_desktop_qmp.py`), on `-smp 4` and `-smp 1`.

## Interfaces

```text
syslib.display_mode_count() -> uint
syslib.display_mode_width(index: uint) -> uint   // 0 when out of range
syslib.display_mode_height(index: uint) -> uint  // 0 when out of range
syslib.set_resolution(width: uint, height: uint) -> Status
```

The kernel implementation is `kernel/src/ui/display.rs`; the device interface is
`arch/x86_64/src/bochs.rs`. The window rebuild is `window::screen_changed`; the shadow
swap is `gfx::change_mode`; the kernel text console reopens its log through
`console::screen_changed`/`window::open_bottom`.

## Failure behaviour

| Situation | Result |
|---|---|
| Bochs dispi absent (UEFI, other VGA, real hardware) | `display_mode_count` is 0; `set_resolution` is `Invalid`; boot graphics still use the bootloader mode |
| Unknown geometry | `NotFound`; nothing changes |
| Shadow allocation fails | `Full`; the device is not touched |
| Device rejects the geometry | `IoError`; the old screen and mode stay |
| Change succeeds | New shadow, new device mode, clamped pointer, full window repaint |
| Task calls `window_present` during the change | It waits on the graphics lock and then draws into the new image |

## Non-goals

- The VBE BIOS `int 10h` path and UEFI GOP mode setting.
- Arbitrary width/height or refresh-rate selection.
- Software scaling or letterboxing instead of a real mode change.
- Persisting the chosen mode across reboots.
- Multiple displays.

## History

| Date | Change |
|---|---|
| 2026-11-21 | Initial implemented version: Bochs VBE dispi runtime presets, builtins 80–83, allocation-first shadow swap, window reflow, the bottom-z-order System Log reopen, boot LFB reservation, the Settings app and the QMP 800×600 proof (ADR 0056). |
