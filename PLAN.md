# Milestone plan and ticket backlog

Working breakdown of the milestones. Specs say *what* must be true; ADRs say *why*;
this file says *who does what next*, in slices small enough for one work session and
one commit. Behaviour changes still start in `specs/` (ground rule 1).

- **Owner:** the project owner decides priority and closes tickets.
- **Status:** `[ ]` todo, `[~]` in progress, `[x]` done, `[!]` blocked.
- **Slice size:** one ticket ≈ one focused session and one commit.
- **Definition of done** (every ticket): spec/ADR updated when behaviour changes; host
  tests green plus new tests; a kernel transcript proving the behaviour (headless where
  possible: `syslib.get_pixel`, injected events, printed self-checks); `--cores 1` run;
  no regression in the existing samples; `AGENTS.md` status/log row; this file updated.

## Resume point (update this when a session ends)

- **Last commit:** `98d3f3a` (M2.3.2 window composition and repair) on top of `142f04f`
  (M2.3.1 resume note). Tree clean.
- **Done and committed:** M2.1 graphics foundation (`f57e39e`), M2.2.0–2.2.2 PS/2 mouse
  and composited cursor (`3cd9337`), M2.2.6 absolute USB tablet (`7c6e93f`), the
  `pointer` diagnostic app and Q49 measurement (`16cc308`/`93fedbf`), M2.2.3–2.2.5
  keyboard key events, the event queue and the `events` proof (`4f2038c`), M2.2.7
  PS/2 aux IRQ12 with the lock-free ring (`70b82cd`), the order-independent `events`
  proof (`8afa36a`), the M2.3.0 window spec (`13aecf2`), M2.3.1 window records and
  lifecycle (`ba4f93c`), and M2.3.2 composition and repair (`98d3f3a`).
- **Verified at that point:** **313 host tests** (abi 15, compiler 139, interpreter 113,
  langhost 46, including doctests); `cargo xtask build`/`image` clean; the boot prints
  `graphics: clip ok`, `window: rules ok` and `window: paint ok` on `-smp 4` and `-smp 1`;
  `run windows` → `windows: ok` on both; `run windows_demo` → `windows_demo: checks ok`,
  `windows_demo: ready`, then `windows_demo: ok` after the 15 s realtime hold on both;
  the screenshot harness writes `/home/sinan/Desktop/igis-windows.png` (1024×768 PNG);
  regressions `events`, `mouse`, `desktop`, `text 42 hi`, `frames`, `streams`, `deep`,
  `utf8`, `realtime`, `cores` pass on both; `scripts/tablet_qmp.py` 7/7, `--no-tablet` →
  `mouse: ok`, and `scripts/mouse_burst_qmp.py --cores 1 --batch 300` → 600/600.
- **Next slice (not started):** M2.3.3 event routing and `Expose` — `EventKind.Expose`,
  `Event.window` filled from the focused window, pointer/key delivery to the focused
  window, `syslib.window_present` clipping the client's presentation to its content
  rectangle, and close-on-task-termination retested through events.

### Resuming from a fresh session (no chat history)

Trust the repository, not a summary. On a fresh start:

1. `git status --short` and `git log --oneline -8` — the tree should be clean and the top
   commit should match the "Last commit" line above; if not, stop and reconcile first.
2. `cargo test -q -p abi -p compiler -p interpreter -p langhost` — expect **313** tests
   plus the doctest, all green.
3. `cargo xtask build` — expect a clean kernel build.
4. Read, in this order: this file, `AGENTS.md` (status paragraph + decisions table),
   `specs/0009-graphics-and-windows.md`, `specs/0010-input-and-events.md`,
   `specs/0011-windows.md`, `adr/0020`, `adr/0021`, `adr/0022`, `adr/0023`, `adr/0024`.
5. Baseline kernel checks (headless, no chat needed):
   - `( sleep 6; printf 'run desktop\n'; sleep 4; printf 'run events\n'; sleep 8; \
     printf 'run windows\n'; sleep 4; printf 'run windows_demo\n'; sleep 22; \
     printf 'exit\n' ) | timeout 150s \
     qemu-system-x86_64 -cdrom build/igis.iso -m 256M -serial stdio -monitor none \
     -no-reboot -no-shutdown -accel tcg -vga std -display none -smp 4 \
     -usb -device usb-tablet`
     → `graphics: clip ok`, `window: rules ok`, `window: paint ok`, `desktop: ok`,
     `keyboard: decoder ok`, `events: ok`, `windows: ok`, `windows_demo: checks ok`,
     `windows_demo: ready`, `windows_demo: ok`, `ioapic: version ...`,
     `mouse: aux IRQ12 enabled ...`, `usb: tablet ...` when the device is present.
   - `python3 scripts/windows_screenshot_qmp.py build/igis.iso` → a 1024×768 PNG at
     `/home/sinan/Desktop/igis-windows.png`.
   - `python3 scripts/tablet_qmp.py build/igis.iso` → `7/7 absolute cases passed`.
   - `python3 scripts/tablet_qmp.py build/igis.iso --no-tablet` → `mouse: ok` fallback.
   - `python3 scripts/mouse_burst_qmp.py build/igis.iso` → `600/600` px with a
     `mouse: aux IRQ12 delivered ...` line; `--cores 1` is the single-core variant.
6. The next slice is **M2.3.3** (routing and `Expose`). Touchpoints:
   `kernel/src/event.rs` (`EventKind.Expose`, `Event.window`), `kernel/src/window.rs`
   (expose queueing, `window_present`), `kernel/src/sched.rs`/`interpreter` (the new
   builtin), `specs/0010-input-and-events.md` and `specs/0003-language.md`. M2.3.4 is the
   desktop shell; until then the console and the desktop share the shadow, so an
   application that prints after composing scrolls the windows (the demo prints first and
   repaints after).

If anything in this file disagrees with the code or the specs, the code and specs win and
this file must be corrected in the same commit.

## M1 — bootable system with source applications

Complete. Boot/long mode, GDT/TSS/IDT, APIC timer, frame allocator, ext2 read-only,
REPL, scheduler + SMP, realtime class, streams + framing, guards, UTF-8, collections
(arrays/rings/maps), float text (fixed-point + shortest round-trip), `syslib` ports.
Remaining M1 polish is in the deferred backlog below.

## M2 — retro desktop (Mac OS System 7.5 look)

Direction: a kernel-owned window service over a full-screen shadow buffer
(ADR 0020), 1024×768×32 or whatever larger mode the bootloader returns, 8×8 font,
1-pixel black chrome, 50% dither desktop. The desktop becomes the primary shell.

### M2.1 — graphics foundation

- [x] **M2.1.1** Shadow screen + nine `syslib` drawing primitives + console through the
  shadow + `samples/apps/desktop` self-check (`f57e39e`). Spec 0009, ADR 0020.
- [ ] **M2.1.2** Screenshot/debug dump: write a rectangle (or the whole shadow) to a stream
  or serial so the UI can be verified without a display. Needed for CI-style visual proof.
- [ ] **M2.1.3** Optional mode request: an `xtask`/kernel parameter to ask GRUB for a
  chosen resolution (e.g. 1280×1024), verified against `syslib.screen_width/height`.

### M2.2 — input: mouse and keyboard events

- [x] **M2.2.0** Spec 0010 "Input and events" + ADR 0021 (event model, queue, blocking,
  routing). Requirements: pointer state, key events, event record, overflow policy,
  focus-independent delivery until M2.3. Spec 0010, ADR 0021.
- [x] **M2.2.1** PS/2 AUX mouse driver: controller init, 3-byte packet stream, overflow and
  resync handling, polling in the service opportunity next to the keyboard. Decoder as a
  pure function with a boot self-check; kernel log line with the first packet.
  `kernel/src/mouse.rs`.
- [x] **M2.2.2** Pointer state + software cursor: 16×16 arrow, composited at present time
  (never in the shadow), clamping to the screen, `syslib.pointer_x/pointer_y`,
  `pointer_buttons` and the debug-only `inject_pointer`, present of the two dirty cursor
  rectangles only. Proof: `samples/apps/mouse` (`get_pixel` under a moved cursor).
- [x] **M2.2.3** Keyboard event layer: scancode set 1 → key code + ASCII (shift/ctrl/alt,
  caps lock), arrows and function keys, a bounded queue, no drops under fast typing; kept
  separate from the kernel line discipline (`core.read_line` stays for the shell).
  `arch/x86_64/src/keyboard.rs` (pure `decode`, `decoder_self_check`).
- [x] **M2.2.4** Event queue + `syslib` API: predeclared `Event` record
  (`kind, x, y, code, window`), `syslib.next_event(var ev) -> Status` (blocks via
  `WaitFor::Input`, SMP-safe wake), `syslib.poll_event`, overflow policy, and a
  deterministic `syslib.inject_event` for headless tests (documented as debug-only).
  `kernel/src/event.rs`, `samples/apps/events`.
- [x] **M2.2.5** Proof app `samples/apps/events`: injected pointer moves and clicks and a
  few keys, printed in one claim, with `get_pixel` cursor assertions; `events_poke`
  proves the blocked wake.
- [x] **M2.2.6** Absolute pointer: PCI config-space scan, UHCI host controller, HID
  `usb-tablet` (SET_ADDRESS/SET_CONFIGURATION, boot protocol, absolute X/Y scaled to the
  screen), QEMU `-usb -device usb-tablet`, PS/2 kept as the fallback, `syslib.pointer_source()`
  and a boot line. Fixes lost motion in bursts and the host-window scale mismatch, because
  an absolute report carries the current position. Proof: QMP `abs` events land on exact
  pixels headless. (Chosen 2026-11-16; see Q49.) Spec 0010, ADR 0022. `kernel/src/pci.rs`,
  `kernel/src/usb.rs`, `samples/apps/tablet`, `scripts/tablet_qmp.py`.
- [x] **M2.2.7** PS/2 aux IRQ12: the I/O APIC routes IRQ12 to the boot core as a
  level-triggered vector `0x2C`; the handler drains the controller into a 1024-byte
  lock-free ring and `mouse::poll` decodes it, with the direct poll kept as the backstop.
  Fixes the 1 ms drain latency and the multi-core status/data race that Q49 attributed to
  the relative fallback. `scripts/mouse_burst_qmp.py` (300×(+2) after clamping) is
  600/600 px on `-smp 4` and `-smp 1`; the polled HEAD is also 600/600 headlessly because
  QEMU 10.2.2 retains PS/2 deltas, so the historical 346/600 is recorded as a host-window
  result. Spec 0010, ADR 0023. `arch/x86_64/src/{ioapic.rs,interrupts.s,idt.rs}`,
  `kernel/src/mouse.rs`.

### M2.3 — window service

- [x] **M2.3.0** Spec 0011 "Windows" (frame/content rects, z-order, focus, clipping stack,
  damage, `expose`, close rules, limits: window count, minimum size, title length).
- [x] **M2.3.1** Window records and lifecycle: `syslib.window_open(x, y, w, h, title)`,
  `window_close`, `window_raise`, `window_move`, `window_content_rect`, `window_frame_rect`;
  z-order list, focus, per-window clip region; ownership by task id.
- [x] **M2.3.2** Composition and repair: frame chrome drawn by the server, damage union of
  affected rectangles, `present` of the union, background restore on move (dither pattern),
  overlapping windows, raise/focus repaint (`window: paint ok`, `samples/apps/windows_demo`,
  `scripts/windows_screenshot_qmp.py`).
- [ ] **M2.3.3** Event routing and `expose`: pointer/key events go to the focused window;
  a window gets an `expose` when damaged; `syslib.window_present` clips to the content
  rectangle automatically; close-on-task-termination.
- [ ] **M2.3.4** Desktop shell: menu bar, app launching, window list/menu, `core.write`
  routed to a terminal window, kernel diagnostics to a system log window (serial mirror
  unchanged).
- [ ] **M2.3.5** Proof: two apps side by side, focus switch, raise, drag, expose redraw
  (headless via injected events).

### M2.4 — UI toolkit (language level)

- [ ] **M2.4.0** `stdlib` UI layout: chrome helpers (window frame, striped title bar, close
  box), text metrics for the 8×8 font, standard metrics and colours.
- [ ] **M2.4.1** String editing helpers: `str_insert_into`, `str_delete_into`, backspace,
  cursor movement, selection basics; tests. Closes the language side of Q44 and unblocks
  text fields.
- [ ] **M2.4.2** Controls: button (pressed/released), checkbox, text field (uses M2.4.1),
  scrollbar, list; keyboard navigation and focus rings.
- [ ] **M2.4.3** Menus: menu-bar model, pull-down tracking, keyboard equivalents,
  checkmarks and disabled items.
- [ ] **M2.4.4** Dialogs and alerts: modal loop, default/cancel buttons, text input dialogs.
- [ ] **M2.4.5** Icons and desktop: 32×32 icon assets, icon grid, double-click, drag,
  selection rectangle, trash.
- [ ] **M2.4.6** Proof: a "widgets" demo window that exercises every control.

### M2.5 — applications

- [ ] **M2.5.1** Read-only file API for applications (`core.fs_list`/`core.fs_read`, bounded
  buffers, `Status` errors); today only the kernel reads ext2. Prerequisite for the browser
  and the editor.
- [ ] **M2.5.2** Clock window (digital + analog face), ticking from `core.clock()`.
- [ ] **M2.5.3** Text editor: in-memory buffer with the M2.4.1 helpers, cursor, scrolling,
  open via the file API; save disabled until a writable filesystem exists.
- [ ] **M2.5.4** File browser: directory list from the read-only ext2 app API, open/launch.
- [ ] **M2.5.5** Terminal window: the existing shell running in a window (`core.write` /
  `core.read_line` routed), so the REPL and the desktop coexist.
- [ ] **M2.5.6** Boot to desktop: `entrypoint` becomes the desktop; the shell is launched
  from a menu item.

### M2.6 — polish and measurement

- [ ] **M2.6.1** Proportional font (Chicago-like) with a glyph cache and text metrics;
  keep 8×8 as the fallback.
- [ ] **M2.6.2** Performance: damage coalescing, present batching, measured event latency
  and frame time under load (realtime controller + UI together).
- [ ] **M2.6.3** Visual regression harness: screenshot dump (M2.1.2) plus a hash/compare
  of known frames, run headless.
- [ ] **M2.6.4** System menu: About box, restart, halt; keyboard shortcuts.

## Deferred (not UI; pick up when they matter)

| Item | Where | Trigger |
|---|---|---|
| Correctly rounded decimal→binary64 parsing (Q41 remainder) | OPEN-QUESTIONS Q41 | when parsed text must round-trip exactly |
| `f32` (Q40) | OPEN-QUESTIONS Q40 | when single precision is needed |
| Recovery from a hardware guard hit (Q47) | OPEN-QUESTIONS Q47 | when a task must survive a kernel-bug guard hit |
| MMIO/DMA/block handles (Q45/Q9) | OPEN-QUESTIONS | first real block device or MMIO peripheral |
| Writable filesystem / persistence | none yet | when the editor must save |
| Console/stream multiplexing, `poll` (Q39) | OPEN-QUESTIONS Q39 | when a task serves two sources |
| Debug/introspection primitive (Q8) | OPEN-QUESTIONS Q8 | when the desktop needs task/system info |
| Nominal `TaskId` (Q43) | OPEN-QUESTIONS Q43 | before `stdlib` exposes task primitives |
| Runtime-capacity buffer parameters (Q46) | OPEN-QUESTIONS Q46 | when slicing/wider stream reads need it |
| Explicit stream close (Q10) | spec 0006 | streams v2 |
| Compile arena headroom (~621 KiB of 1 MiB used) | spec 0008 | as `stdlib` grows |
| Guard table cache exhaustion (32 tables) | spec 0002/spec 0009 | long-lived workloads with many stacks |
| `cargo fmt` drift in three pre-existing files + CI fmt/clippy | repo | housekeeping |

## How a ticket is verified (headless first)

1. Host tests for the language/compiler/interpreter surface.
2. Kernel transcript with a self-check the app prints (`desktop: ok` style), using
   `syslib.get_pixel` for drawing and `syslib.inject_event` for input.
3. `-smp 4` and `--cores 1` both.
4. Regression set: `streams`, `frames`, `text`, `realtime`, `deep`, `cores`, `utf8`.
5. A screenshot dump once M2.1.2 lands, for the actual framebuffer pixels.
