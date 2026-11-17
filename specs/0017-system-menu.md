# 0017: The system menu and system control

- **Status:** Implemented
- **Scope:** `kernel` (`sched`, `smp`, `main`), `arch/x86_64` (`reboot`), `abi`,
  `compiler`, `interpreter`, `langhost`, `samples/apps/desktop_shell`,
  `samples/apps/system_probe`, `scripts/system_menu_qmp.py`,
  `scripts/system_control_qmp.py`; M2.6.4 of the desktop milestone
- **Supersedes:** none
- **Superseded by:** none
- **ADRs:** 0033
- **Updated:** 2026-11-17

## Context

By M2.6.3 the desktop is the boot entry and owns the menu bar, event loop and window
list. It has direct menu items and no pull-down; the M2.4.3 menu model
(`ui_menu_dropdown`, `ui_menu_hit`, `ui_menu_move`, `ui_menu_key`) and the M2.4.4 dialog
helpers (`ui_alert`, `ui_message`, `ui_dialog_button`, `ui_dialog_key`) are available
but the desktop does not use them. The system also has no way for an application to
stop or reset the machine: the scheduler stops the machine only when no task is live,
and `syslib.port_out8` alone is not enough for a reliable halt.

M2.6.4 closes the desktop milestone with the familiar System/Apple menu: an About box,
Restart, Halt and keyboard shortcuts.

Non-goals:

- no shutdown confirmation dialog: M2 uses direct actions (ADR 0033);
- no ACPI power-off or S5 transition (that needs an ACPI table parser the kernel does
  not have);
- no per-task system-control permission: there is no capability model (vision R14);
- no menu-window or global work area: the pull-down stays application-drawn over the
  shadow, as the M2.4.3 model assumes;
- no new bytecode opcode, no second terminal and no writable filesystem.

## Requirements

### The System menu

- **S1 (MUST):** `samples/apps/desktop_shell` adds a `System` label to the menu bar after
  `Terminal`, laid out from the measured proportional label widths (spec 0015), exactly
  like the existing direct items. Clicking the label opens a pull-down whose frame is
  `(menu_system_x - 4, 22, 224, 4 * 16 + 2)`; clicking it again closes it.
- **S2 (MUST):** The pull-down uses the M2.4.3 menu model (`ui_menu_dropdown` for the
  drawing, `ui_menu_hit` for the hit test, `ui_menu_move`/`ui_menu_first` for the
  highlight, `ui_menu_key` for the key mapping) and the M2.4.4 button helpers. Item
  order, labels and shortcut labels are:

  | # | Label | Shortcut |
  |---|-------|----------|
  | 0 | `About igis-v2` | `Ctrl+A` |
  | 1 | `-` (separator) | — |
  | 2 | `Restart` | `Ctrl+R` |
  | 3 | `Halt` | `Ctrl+H` |

  The separator is never selectable; Up/Down skip it, and Enter activates only a
  selectable row. The active bar label is inverted while the menu is open.
- **S3 (MUST):** While the menu is open, pointer-down inside the pull-down activates the
  hit row (a disabled or separator row is ignored and the menu stays open); pointer-down
  elsewhere closes it; Up/Down move the highlight; Enter activates; Escape closes. A
  pointer move over the pull-down moves the highlight to the selectable row under it.
  Because the pull-down is application-drawn and has no window, its clicks are handled
  by coordinate even when the kernel routed the click to a window underneath.
- **S4 (MUST):** Closing the pull-down restores the pixels it covered: the desktop
  raises the topmost live window whose frame intersects the pull-down (the service
  repaints its chrome and marks its `Expose`; the console pump or the owning app restores
  its content), then raises its own window so focus returns to the desktop and repaints
  its own window content. No screen save, no new window-service API.

### The About panel

- **S5 (MUST):** `About igis-v2` opens a modal, desktop-owned window titled
  `About igis-v2`, content `420x200` (clamped to the screen with a 16-pixel margin),
  centred below the menu bar. Its content is a white panel with the proportional
  `igis-v2` title, a short truthful description through `ui_message` (interpreted
  desktop at ring 0; windows, terminal, text and files; fonts, menus and a golden frame;
  M2, 2026), and a default-styled `OK` button through `ui_dialog_button`. The service's
  own chrome is the panel frame. There is no version injection.
- **S6 (MUST):** The panel is modal within the desktop's event loop: while it is open
  every event is handled by the panel. An owner `Expose` repaints the content; Enter
  (dialog action 1, the default) and Escape (action 2, cancel) both close it through
  `ui_dialog_key`; a pointer-down routed to the panel closes it (the OK button, the
  title bar or anywhere inside). Closing uses `window_close`, so the service restores
  what the panel covered through the ordinary damage/`Expose` protocol and the desktop
  repaints its own window. `Ctrl+A` opens the panel directly.
- **S7 (MUST):** Opening a second panel while one is open is a no-op (the first is kept).

### Restart and Halt

- **S8 (MUST):** `core.halt() -> Status` and `core.reboot() -> Status` are host builtins
  48 and 49 (spec 0003, spec 0005, spec 0008), with no operands and no new opcode. Both
  `Host` methods default to `Invalid`, which is the `langhost`/bare-mock contract.
- **S9 (MUST):** `core.halt` requests a machine-wide halt on a live kernel: it sets the
  shutdown flag and wakes every other online core on the existing `0x40` vector. The
  wake handler acknowledges (`smp::note_stopped`) and halts **without returning to the
  task**, so a busy core stops too. The caller counts itself as stopped, waits bounded
  by the clock and a spin ceiling (the boot core's timer may stop first), prints
  `system: N of M APs stopped`, and halts. It never returns to the application.
- **S10 (MUST):** `core.reboot` performs the same stop and then drives the platform
  reset channels: `0xCF9 <- 0x02`, `0xCF9 <- 0x06`, then a bounded wait for the 8042
  input buffer and `0x64 <- 0xFE`. QEMU resets and reboots. If the firmware ignores both
  channels the calling core halts; the other cores have already been stopped, so a
  failed reset is a halt, never a half-resumed machine. There is no confirmation.
- **S11 (MUST):** The System menu's `Restart`/`Halt` rows and the `Ctrl+R`/`Ctrl+H`
  shortcuts call the same two builtins. Shortcuts are focus-scoped and modal-aware
  (S12).

### Keyboard shortcuts

- **S12 (MUST):** The desktop's shortcuts fire only when no pull-down and no About panel
  is open, and only when the key event is routed to the desktop's own window (or has no
  target, `window = 0`). While a menu is open its own key mapping owns the keys; while
  the About panel is open the dialog mapping owns them. This keeps `Ctrl+A` in another
  application (the editor) from opening the About panel.
- **S13 (MUST):** `F10` opens/toggles the System pull-down when the desktop is focused.
  `Ctrl+A`, `Ctrl+R` and `Ctrl+H` are About, Restart and Halt. Only the Ctrl modifier is
  required (`ui_key_modifiers(code) div 2 mod 2 = 1`); the key codes are the unshifted
  ASCII letters the keyboard layer reports (65/82/72). The shortcut labels are drawn in
  the pull-down.
- **S14 (MUST):** There is no Command key in the key layer (spec 0010), so the shortcuts
  are Ctrl-based and documented here and in the desktop source. A shortcut never fires
  from a key release or from a non-desktop window.

### Proof

- **S15 (MUST):** `desktop_shell check` stays deterministic and independent of the event
  queue. It opens the System menu through the bar handler, checks the accent row and
  the separator rule with `get_pixel`, traverses with Up/Down (skipping the separator),
  closes with Escape, reopens with `F10`, activates About with Enter, checks the panel
  is topmost and its content pixels (white, button border, title ink), closes with
  Escape and again with a routed click, opens it with `Ctrl+A`, verifies that Restart and
  Halt never fired, and verifies the desktop window's content marker is restored. It
  prints `desktop_shell: ok`.
- **S16 (MUST):** `scripts/system_menu_qmp.py` boots `build/igis.iso` (the desktop
  entry), clicks the `System` label and the `About igis-v2` row through QMP absolute
  pointer events, waits for `desktop_shell: about open`, captures
  `/home/sinan/Desktop/igis-system-menu.png` through QMP `screendump`, then injects
  Escape and waits for `desktop_shell: about closed`.
- **S17 (MUST):** `samples/apps/system_probe` takes `halt`/`reboot` as its argument and
  calls the matching builtin. `scripts/system_control_qmp.py --mode halt` boots
  `build/igis-shell.iso` with `-no-shutdown -no-reboot`, runs the probe, requires the
  stop summary and then requires that a further serial command produces no echo and no
  prompt. `--mode reboot` boots without `-no-reboot`, runs the probe and requires a
  second `igis-v2: long mode entered` banner. These runs are separate from the normal
  regression suite.

## Design

### Menu and About state

```text
desktop_shell state
  sys_items: array[string[64], 16]   // 0 About, 1 -, 2 Restart, 3 Halt
  sys_keys:  array[string[32], 16]   // Ctrl+A, "", Ctrl+R, Ctrl+H
  system_open, system_selected        // pull-down state, owned by the app
  about_open, about_id, about_rect    // the modal panel window
  about_runs, restart_runs, halt_runs // check-mode bookkeeping
```

The app owns every state scalar; the library stores nothing. The menu bar's layout
(`menu_system_x`) is recomputed from the measured widths on every draw, so the label,
its hit range and the pull-down origin cannot drift.

### Closing the pull-down

The shared shadow gives the menu no window, so its pixels cover whatever is underneath
(the owner-0 terminal or the shell's attached terminal at boot). Closing therefore asks
the window service to repair: `window_info` enumerates topmost-first for the first frame
that intersects the pull-down rectangle, `window_raise` on it repaints its chrome and
marks its `Expose` (the console pump repaints the terminal grid for the attached window;
any other app repaints its own content), and `window_raise(own)` restores focus. The
desktop then redraws the bar and its own content. This reuses the M2 damage/expose
protocol and adds no service API.

### Halt and reboot plumbing

```text
core.halt  -> KernelHost::halt  -> sched::halt_system
core.reboot-> KernelHost::reboot-> sched::reboot
halt_system: println, note_stopped(self), smp::stop_all (set SHUTDOWN + broadcast),
             bounded wait, println summary, arch_x86_64::halt
reboot:      println, smp::request_shutdown (set + broadcast), arch_x86_64::reboot
wake_ipi:    if shutdown_requested { note_stopped(self); halt } else { on_ipi; eoi }
```

The wake vector is the stop channel because every core already takes it; the handler
halts mid-task rather than waiting for the idle loop, which is what makes a halt reliable
with a realtime task on another core. `stop_all` keeps its clock deadline and adds a
spin ceiling for the case where the requesting core is an AP and the boot core (the only
timer source) halts first.

## Failure modes

| Situation | Behaviour |
|-----------|-----------|
| Graphics disabled or screen too small | `desktop_shell` keeps its pre-M2.6.4 path (`no graphics`, serial shell); the System menu and About panel are never drawn |
| About window cannot open (table full, geometry) | `about_open` stays false, `desktop_shell: about failed` is printed; the menu is already closed |
| A second About open while one is live | No-op; the first panel is kept |
| Pull-down click lands on a window underneath | The kernel may raise/focus that window; the desktop still handles the menu by coordinate and the close path repairs it |
| Click on a disabled or separator row | Ignored; the menu stays open |
| Shortcut typed into another app's window | Ignored (S12); the app keeps its key |
| Shortcut while the menu or About is open | Ignored; the open surface's own key mapping applies |
| `core.halt` on `langhost` or a bare mock | `Invalid`; the probe prints its return line and the host keeps running |
| `core.halt` on the kernel with no APs | `system: 0 of 0 APs stopped`, the calling core halts |
| Reset channels ignored by firmware | The calling core halts after `system: reboot requested`; the other cores are already stopped |
| A wedged core never acknowledges | The bounded clock/spin wait returns and reports the partial count; the caller still halts |

## Open questions

| # | Question | Trigger / owner |
|---|----------|-----------------|
| 1 | Should Restart/Halt require a confirmation or an explicit menu key that cannot be reached by a focused window? | When the desktop gains a user model beyond a single operator (ADR 0033 accepts direct actions for M2) |
| 2 | Should the About panel become a window with a version/build string from the manifest? | When the build carries a version worth showing |
| 3 | ACPI S5 power-off instead of a halt loop | When an ACPI table parser exists |

## Changelog

| Date | Change |
|------|--------|
| 2026-11-17 | Initial implemented version (M2.6.4): the `System` pull-down with About/Restart/Halt, the modal desktop-owned About window, `core.halt`/`core.reboot` (builtins 48/49), the focus-scoped Ctrl/F10 shortcuts, the `desktop_shell check` extension, the two QMP harnesses and the separate halt/reboot probe. ADR 0033 records the authority and shortcut policy. |
