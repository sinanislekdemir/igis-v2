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

- **Last commit:** `648e1de` (the M2.5.3 text editor, its `langhost` test, the QMP
  screenshot harness and the spec 0003 note) plus this docs/resume commit on top.
- **Done and committed:** M2.1 graphics foundation (`f57e39e`), M2.2.0–2.2.2 PS/2 mouse
  and composited cursor (`3cd9337`), M2.2.6 absolute USB tablet (`7c6e93f`), the
  `pointer` diagnostic app and Q49 measurement (`16cc308`/`93fedbf`), M2.2.3–2.2.5
  keyboard key events, the event queue and the `events` proof (`4f2038c`), M2.2.7
  PS/2 aux IRQ12 with the lock-free ring (`70b82cd`), the order-independent `events`
  proof (`8afa36a`), the M2.3.0 window spec (`13aecf2`), M2.3.1 window records and
  lifecycle (`ba4f93c`), M2.3.2 composition and repair (`98d3f3a`), M2.3.3 routing,
  `Expose` and `window_present` (`dc14442`), M2.3.4 the desktop shell, kernel text
  windows and `window_info` (`4f9a45e`), M2.3.5 the two-application proof
  (`8688f88`), M2.4.0 the `stdlib` UI layout layer (`655faf1`, `d8f6609`), M2.4.1 the
  `stdlib` string editing and selection layer (`b7b813c`), M2.4.2 the `stdlib` controls
  layer plus the `ui_controls` proof (`320d086`), M2.4.3 the `stdlib` menus layer
  plus the `ui_menus` proof (`9fa9fd0`), M2.4.4 the `stdlib` dialogs layer plus the
  `ui_dialogs` proof (`f03559b`), M2.4.5 the `stdlib` icons and desktop layer plus
  the `ui_icons` proof (`fb452bd`), M2.4.6 the widgets demo window plus the
  `widgets` proof (`9d9b0f1`), M2.5.1 the root-only read-only file API plus the
  `files` proof (`03064f2`), M2.5.2 the windowed clock plus its check/shot modes
  and screenshot harness (`65f4875`), and M2.5.3 the text editor plus its
  check/shot modes and screenshot harness (`648e1de`).
- **Verified at that point:** **364 host tests** (abi 18, compiler 152, interpreter 127,
  langhost 67, including doctests); `cargo xtask build`/`image` clean; the boot prints
  `console: terminal ok`, `console: log ok`, `console: route ok`, `graphics: clip ok`,
  `window: rules ok`, `window: paint ok` and `window: route ok` on `-smp 4` and
  `-smp 1`; `run files` → `files: root listing ok`, `files: root dot ok`,
  `files: size agrees ok`, `files: chunked read ok`, `files: read 559/559`,
  `files: offset rules ok`, `files: error paths ok`, `files: ok` on both; `run text_edit`
  → `text_edit: ok` on both; `run ui_layout` → `ui_layout: ok`
  on both; `run ui_controls` → `ui_controls: ok` on both; `run ui_menus` →
  `ui_menus: ok` on both; `run ui_dialogs` → `ui_dialogs: ok` on both; `run ui_icons` →
  `ui_icons: open Document` then `ui_icons: ok` on both; `run widgets` (or
  `run widgets check`) → `widgets: ok` on both, and `widgets: host ok` on `langhost`;
  `run clock_window check` → `clock_window: ok` on both, and
  `clock_window: host ok` on `langhost`; `run clock_window shot` →
  `clock_window: ready`, then `clock_window: done` after the five-second live hold;
  `run editor check` → `editor: ok` on `-smp 4` and `-smp 1`, and `editor: host ok`
  on `langhost`; `run editor shot` → `editor: ready`, then `editor: done` after the
  five-second hold; `run windows_proof` → `windows_proof: ok` in its own boot;
  `run desktop_shell check` →
  `desktop_shell: ok`; `run windows` → `windows: ok`; `run windows_events` →
  `windows_events: ok`; `run windows_demo` → `windows_demo: checks ok`,
  `windows_demo: ready`, `windows_demo: ok`; `run mouse` → `mouse: ok` in its own boot
  (and through both QMP harnesses); regressions `events`, `desktop`, `text 42 hi`,
  `frames`, `streams`, `deep`, `utf8`, `realtime`, `cores` pass on `-smp 4` (the UI
  samples also on `-smp 1`);
  `scripts/tablet_qmp.py` 7/7, `--no-tablet` → `mouse: ok`,
  `scripts/mouse_burst_qmp.py --cores 1` → 600/600; the embedded `stdlib` is 114,823 B
  of source, 84,920 B of code and 4,134,600 B of compile scratch with a minimal
  application (the `ui_icons` pair needs 5,386,584 B, the `widgets` pair 6,546,928 B
  with a 57,388 B application; the source cap is 128 KiB and the arena cap 8 MiB). The
  `widgets` task package is 165,424 B of code (41 frames), 9,608 B of state and a
  25,360 B stack; the `files` package is 6,936 B of code (2 frames), 520 B of state and
  664 B of stack, the `clock_window` source is 17,744 B and its task package is
  103,696 B of code (26 frames), 1,304 B of state and a 20,896 B stack, the `editor`
  source is 43,834 B and its package is 134,176 B of code (33 frames), 19,856 B of state
  and a 33,416 B stack, and the image root lists 54 entries (53 staged source files plus
  the `mke2fs` `lost+found` directory). Screenshots
  `/home/sinan/Desktop/igis-desktop.png`
  (1024×768, 31555 bytes), `igis-windows.png` (1024×768, 28169 bytes),
  `igis-m23-proof.png` (1024×768, 30901 bytes), `igis-controls.png` (1024×768, 32967
  bytes), `igis-menus.png` (1024×768, 27590 bytes), `igis-dialogs.png`
  (1024×768, 29544 bytes), `igis-icons.png` (1024×768, 33991 bytes),
  `igis-widgets.png` (1024×768, 25894 bytes), `igis-clock.png` (1024×768,
  31443 bytes) and `igis-editor.png` (1024×768, 30492 bytes).
- **Next slice (not started):** **M2.5.4** — the file browser: a directory list from the
  read-only ext2 app API, open/launch.
- **M2.5.3 complete:** `samples/apps/editor` is a 640×480 window with an app-drawn File
  menu (Open, a disabled Save, Quit), a path/`read-only` line, a 26-row text view, a
  scrollbar and a status/`Ln`/`Col` line. The document is one `string[8192]` loaded with
  `core.fs_size` + 256-byte `core.fs_read` chunks and one strict-UTF-8
  `core.substr_into`, so every load failure leaves the old document intact; the one
  mutator is app-local and atomic (`ed_replace`) because the `str_*` mutators stay at
  256 bytes, while the read-only `str_*` helpers work on the larger buffer. Keys, a
  scalar selection, vertical/horizontal scrolling, an injected routed key and drawing
  pixels are checked by `run editor check`; `langhost` runs the same load/edit/line
  logic (`editor: host ok`); `scripts/editor_screenshot_qmp.py` writes
  `igis-editor.png`. Saving stays disabled (the Save row is disabled and Ctrl+S reports
  "read-only"). No new helper, opcode or builtin; spec 0003 records the
  above-256-byte-buffer contract.
- **M2.5.2 complete:** `samples/apps/clock_window` is an ordinary windowed app with an
  integer midpoint-circle analog face (60 ticks, hour/minute/second hands via a built-once
  fixed-point 6-degree direction table), a framed `HH:MM:SS` readout, a deterministic
  `10:08:30` check (`get_pixel` over the rim, marks, hands, centre, readout and bezel),
  and a `shot` mode that ticks from `core.clock()` in a bounded poll/redraw loop
  (no `next_event`, no `syslib.clear`), repaints the whole content on an owner `Expose`
  and presents once per frame. No new helper, opcode or builtin (stdlib stays at 125
  functions); `scripts/clock_window_screenshot_qmp.py` writes `igis-clock.png`.
- **M2.5.1 complete:** applications can read the image through `core.fs_list`,
  `core.fs_read` and `core.fs_size` (builtins 41–43, no opcode), root-only, with the
  shared `core.spawn` path syntax, a 64-byte name, 256-byte chunks and `Status` errors.
  The editor and browser can now be built on it.
- **M2.4 complete:** the language-level UI toolkit now has the layout/chrome (M2.4.0),
  string editing (M2.4.1), controls (M2.4.2), menus (M2.4.3), dialogs (M2.4.4), icons
  (M2.4.5) and the one composed widgets demo (M2.4.6).
- **Known sample sensitivity:** `windows_proof` must run in its own boot in this
  environment. Its step 23 (`A old area restored to the terminal`) samples `(100, 350)`,
  which can land on scrolled terminal text after several text apps; the same failure
  reproduces on the M2.4.2 top commit `92b140c`, so it is a pre-existing sample-harness
  sensitivity, not an M2.4.3 or M2.4.5 regression. On one loaded `-smp 4` boot its step 8
  (`B content drawn`) failed before the child had painted and passed on a rerun in the
  same build, so treat the whole proof as timing-sensitive. `mouse` likewise runs in its
  own boot (a full-screen `syslib.clear` races a console repaint).

### Resuming from a fresh session (no chat history)

Trust the repository, not a summary. On a fresh start:

1. `git status --short` and `git log --oneline -8` — the tree should be clean and the top
   commit should match the "Last commit" line above; if not, stop and reconcile first.
2. `cargo test -q -p abi -p compiler -p interpreter -p langhost` — expect **364** tests
   plus the doctests, all green.
3. `cargo xtask build` — expect a clean kernel build.
4. Read, in this order: this file, `AGENTS.md` (status paragraph + decisions table),
   `specs/0009-graphics-and-windows.md`, `specs/0010-input-and-events.md`,
   `specs/0011-windows.md`, `specs/0012-desktop-shell.md`,
   `specs/0013-application-files.md`, `adr/0020`, `adr/0021`, `adr/0022`, `adr/0023`,
   `adr/0024`, `adr/0025`, `adr/0026` (the `stdlib` UI layer) and `adr/0027` (the
   stateless file API).
5. Baseline kernel checks (headless, no chat needed):
   - Combined boot (no window-injecting proof app, so the shared event queue stays
     quiet between samples):
     `( sleep 6; printf 'run files\n'; sleep 8; \
       printf 'run text_edit\n'; sleep 10; printf 'run ui_layout\n'; sleep 6; \
       printf 'run ui_controls\n'; sleep 10; printf 'run ui_menus\n'; sleep 10; \
       printf 'run ui_dialogs\n'; sleep 10; printf 'run ui_icons\n'; sleep 12; \
       printf 'run widgets check\n'; sleep 20; \
       printf 'run clock_window check\n'; sleep 8; \
       printf 'run editor check\n'; sleep 12; \
       printf 'run desktop_shell check\n'; sleep 20; printf 'run desktop\n'; sleep 4; \
       printf 'run events\n'; sleep 8; printf 'run windows\n'; sleep 6; \
       printf 'run windows_events\n'; sleep 6; printf 'run windows_demo\n'; sleep 22; \
       printf 'exit\n' ) | timeout 360s \
       qemu-system-x86_64 -cdrom build/igis.iso -m 256M -serial stdio -monitor none \
       -no-reboot -no-shutdown -accel tcg -vga std -display none -smp 4 -usb \
       -device usb-tablet`
     → `console: terminal ok`, `console: log ok`, `console: route ok`,
     `graphics: clip ok`, `window: rules ok`, `window: paint ok`, `window: route ok`,
     `files: ok`, `text_edit: ok`, `ui_layout: ok`, `ui_controls: ok`, `ui_menus: ok`,
     `ui_dialogs: ok`, `ui_icons: open Document`, `ui_icons: ok`, `widgets: ok`,
     `clock_window: ok`, `editor: ok`, `desktop_shell: ok`, `desktop: ok`,
     `keyboard: decoder ok`, `events: ok`,
     `windows: ok`, `windows_events: ok`, `windows_demo: checks ok`,
     `windows_demo: ready`, `windows_demo: ok`, `ioapic: version ...`,
     `mouse: aux IRQ12 enabled ...`, `usb: tablet ...` when the device is present.
   - `windows_proof` in its own boot: `( sleep 6; printf 'run windows_proof\n'; sleep 30; \
     printf 'exit\n' ) | timeout 60s qemu-system-x86_64 ...` → `windows_proof: ok`.
     Its step 23 samples `(100, 350)`, which can be scrolled terminal text after several
     text apps; the same failure reproduces on `92b140c`, so this is a sample-harness
     sensitivity, not a product regression. Run `mouse` in its own boot too (the QMP
     harness does): a full-screen `syslib.clear` erases the kernel windows' pixels and a
     concurrent console repaint can overwrite the background pixels the sample checks.
   - Repeat the UI samples with `-smp 1`: `text_edit: ok`, `ui_layout: ok`,
     `ui_controls: ok`, `ui_menus: ok`, `ui_dialogs: ok`, `ui_icons: ok`,
     `widgets: ok`, `clock_window: ok`, `editor: ok` and `windows_proof: ok` must hold
     there too.
   - `python3 scripts/desktop_screenshot_qmp.py build/igis.iso` → a 1024×768 PNG at
     `/home/sinan/Desktop/igis-desktop.png`.
   - `python3 scripts/windows_screenshot_qmp.py build/igis.iso` → a 1024×768 PNG at
     `/home/sinan/Desktop/igis-windows.png`.
   - `python3 scripts/windows_proof_screenshot_qmp.py build/igis.iso` → a 1024×768 PNG
     at `/home/sinan/Desktop/igis-m23-proof.png`.
   - `python3 scripts/ui_controls_screenshot_qmp.py build/igis.iso` → a 1024×768 PNG
     at `/home/sinan/Desktop/igis-controls.png`.
   - `python3 scripts/ui_menus_screenshot_qmp.py build/igis.iso` → a 1024×768 PNG at
     `/home/sinan/Desktop/igis-menus.png`.
   - `python3 scripts/ui_dialogs_screenshot_qmp.py build/igis.iso` → a 1024×768 PNG at
     `/home/sinan/Desktop/igis-dialogs.png`.
   - `python3 scripts/ui_icons_screenshot_qmp.py build/igis.iso` → a 1024×768 PNG at
     `/home/sinan/Desktop/igis-icons.png`.
   - `python3 scripts/widgets_screenshot_qmp.py build/igis.iso` → a 1024×768 PNG at
     `/home/sinan/Desktop/igis-widgets.png`.
   - `python3 scripts/clock_window_screenshot_qmp.py build/igis.iso` → a 1024×768 PNG
     at `/home/sinan/Desktop/igis-clock.png`.
   - `python3 scripts/editor_screenshot_qmp.py build/igis.iso` → a 1024×768 PNG at
     `/home/sinan/Desktop/igis-editor.png`.
   - `python3 scripts/tablet_qmp.py build/igis.iso` → `7/7 absolute cases passed`.
   - `python3 scripts/tablet_qmp.py build/igis.iso --no-tablet` → `mouse: ok` fallback.
   - `python3 scripts/mouse_burst_qmp.py build/igis.iso` → `600/600` px with a
     `mouse: aux IRQ12 delivered ...` line; `--cores 1` is the single-core variant.
6. The next slice is **M2.5.4** (the file browser: a directory list from the read-only
   ext2 app API, open/launch). M2.5.3 landed the text editor with its check/shot modes
   and screenshot harness; M2.5.2 landed the clock window; M2.5.1 landed `core.fs_list`,
   `core.fs_read` and `core.fs_size` over the image root, no handle or writable path.
   The M2 limitations still in force: an app that ignores an `Expose` keeps stale
   content, a full-screen `syslib.clear` erases the kernel windows' pixels, there is no
   per-task clip (a covered client must repaint only its visible part), and there is no
   global work area (an app at `y = 0` covers the menu bar until the shell's next event).

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
- [x] **M2.3.3** Event routing and `expose`: pointer/key events go to the focused window;
  a window gets an `expose` when damaged; `syslib.window_present` clips to the content
  rectangle automatically; close-on-task-termination (`window: route ok`,
  `samples/apps/windows_events`).
- [x] **M2.3.4** Desktop shell: menu bar, app launching, window list/menu, `core.write`
  routed to a terminal window, kernel diagnostics to a system log window (serial mirror
  unchanged); check mode, normal mode and `scripts/desktop_screenshot_qmp.py`
  (spec 0012, ADR 0025; `samples/apps/desktop_shell`, `kernel/src/console.rs`).
- [x] **M2.3.5** Proof: two apps side by side, focus switch, raise, scripted drag,
  expose redraw (headless via injected events). `samples/apps/windows_proof` +
  `windows_proof_a`/`windows_proof_b`, `scripts/windows_proof_screenshot_qmp.py`
  (`8688f88`). Also fixed the fair claim cursor (spec 0007 Sch12) and the console
  stale-parts retry (spec 0012 D3) that the proof exposed.

### M2.4 — UI toolkit (language level)

- [x] **M2.4.0** `stdlib` UI layout: text metrics for the 8×8 font (code points × 8, line
  height, content inset), the System 7.5 palette as functions, and client chrome
  (`ui_frame`, `ui_title_bar` with focused stripes and a close box, `ui_close_box`,
  `ui_separator`, `ui_center_text`). The library imports `core` and `syslib` (ADR 0026);
  no new opcode or builtin. Proof `samples/apps/ui_layout` draws a synthetic dialog inside
  a window, presents it and checks pixels (`ui_layout: ok`; `ui_layout: host ok` on
  `langhost`). The compile arena cap moved to 2 MiB. Specs 0003/0008/0009, ADR 0026;
  `655faf1`, `d8f6609`.
- [x] **M2.4.1** String editing helpers: `str_insert_into`, `str_delete_into`, backspace,
  cursor movement, selection basics; tests. Closes the language side of Q44 and unblocks
  text fields. `samples/apps/text_edit` is the proof; 13 helpers, no new opcode or builtin
  (`b7b813c`). Specs 0003/0005.
- [x] **M2.4.2** Controls: button (pressed/released), checkbox, text field (uses M2.4.1),
  scrollbar, list; keyboard navigation and focus rings. 32 ordinary-language helpers in
  `stdlib` (84 total) with no new opcode, builtin or `state`; the app owns the state and
  event loop. Proof `samples/apps/ui_controls` (injected pointer/key events, `get_pixel`)
  and the `langhost` sequence test; the source cap rose to 128 KiB and the arena to 4 MiB
  (spec 0008). Specs 0003/0008.
- [x] **M2.4.3** Menus: menu-bar model, pull-down tracking, keyboard equivalents,
  checkmarks and disabled items. 15 ordinary-language helpers in `stdlib` (99 total) with
  no new opcode, builtin or `state`: `ui_menu_bar`/`ui_menu_bar_hit`,
  `ui_menu_dropdown`/`ui_menu_hit`, the `uint` masks (`ui_menu_bit`/`has`/`set`), the
  selectability/state helpers (`ui_menu_selectable`, `ui_menu_first`, `ui_menu_open`,
  `ui_menu_close`, `ui_menu_move`) and `ui_menu_key`. Checked and disabled are independent
  of the highlight; separators are `-` and are never selectable; disabled rows are gray
  and skipped. Proof `samples/apps/ui_menus` (injected pointer/key events, `get_pixel`)
  and the `langhost` sequence test; screenshot `igis-menus.png` (`9fa9fd0`).
  Specs 0003/0008.
- [x] **M2.4.4** Dialogs and alerts: modal loop, default/cancel buttons, text input
  dialogs. 10 ordinary-language helpers in `stdlib` (109 total) with no new opcode,
  builtin or `state`: `ui_message` (word wrap in a fixed box), the button-row geometry
  (`ui_dialog_button_width`/`_left`/`_top`), `ui_dialog_button` (default double border),
  `ui_alert`/`ui_alert_hit` and `ui_input_dialog`/`ui_input_dialog_hit` (alert slots 0
  default / 1 cancel; input slots 0 field / 1 default / 2 cancel; the close box cancels)
  and `ui_dialog_key` (0 none / 1 default / 2 cancel / 3 focus next / 4 focus previous;
  Enter always default, Escape cancel with `has_cancel`). The app owns the modal loop, the
  focus slot, the result and the text buffer; the input dialog only draws, so editing
  stays the M2.4.1 helpers. Proof `samples/apps/ui_dialogs` (injected key/pointer events,
  `get_pixel`) and the `langhost` sequence test; screenshot `igis-dialogs.png`
  (`f03559b`). Specs 0003/0008.
- [x] **M2.4.5** Icons and desktop: 32×32 icon assets, icon grid, double-click, drag,
  selection rectangle, trash. 16 ordinary-language helpers in `stdlib` (125 total) with
  no new opcode, builtin or `state`: `ui_icon` (one `fill_rect` per maximal set-bit run)
  and the bit/label helpers, the cell/label/grid geometry, `ui_double_click` (400 ms,
  inclusive, same target), the drag `ui_icon_grab`/`ui_icon_follow` pair, the selection
  `ui_selection_normalize`/`ui_selection_rect`/`ui_rect_intersects` helpers and the
  `ui_icon_drop` (-2 trash / cell / -1) and `ui_trash_hit` rules. The four 32×32 bitmaps
  stay in the sample; the app owns the cells, live/selected flags, drag and click state.
  Proof `samples/apps/ui_icons` (injected pointer events, double-click open marker, drag
  to an empty cell, drag to the trash, rubber-band selection, `get_pixel`) and the
  `langhost` sequence/pure test; `scripts/ui_icons_screenshot_qmp.py` writes
  `igis-icons.png`. The arena cap rose to 8 MiB (the pair needs 5,386,584 B; spec 0008).
- [x] **M2.4.6** Proof: the `widgets` demo window that exercises every control. One 760×560
  client panel (`samples/apps/widgets`) with an app-drawn File/Edit/View menu bar and clock,
  a button, a checkbox, a text field (typing, selection, scroll, a 2-byte `é` through the
  M2.4.1 helpers), a scrollbar over a 16-line document, an 8-row list, a folder/document
  icon strip with a trash target, an alert and a text-input dialog (one modal state
  machine: while a dialog is open every key and pointer event goes to it; the field is
  published only on OK). Check mode drives injected pointer/key events and checks pixels
  with `get_pixel`; it prints `widgets: ok` on `-smp 4` and `-smp 1` and `widgets: host ok`
  on `langhost`, and `langhost/tests/e2e.rs` pins the composed drawing sequence and the
  control/key results. No new helper, opcode or builtin (125 `stdlib` functions); the
  `widgets` source is 57,388 B and the pair needs 6,546,928 B of scratch.
  `scripts/widgets_screenshot_qmp.py` writes `igis-widgets.png` (`9d9b0f1`). **M2.4 is
  complete.**

### M2.5 — applications

- [x] **M2.5.1** Read-only file API for applications (`core.fs_list`/`core.fs_read`,
  bounded buffers, `Status` errors); today only the kernel reads ext2. Prerequisite for
  the browser and the editor.
- [x] **M2.5.2** Clock window (digital + analog face), ticking from `core.clock()`.
  `samples/apps/clock_window` draws an integer midpoint-circle face (60 ticks,
  hour/minute/second hands from a built-once fixed-point 6-degree direction table) and
  a framed `HH:MM:SS` readout inside a 260×280 window; `check` verifies a fixed
  `10:08:30` frame with `get_pixel` and `shot` ticks live in a bounded poll/redraw loop
  (no `next_event`, no `syslib.clear`) and presents once per frame; an owner `Expose`
  repaints the whole content. `langhost` covers the pure decomposition/formatting/
  direction logic (`clock_window: host ok`); `scripts/clock_window_screenshot_qmp.py`
  writes `igis-clock.png` (`65f4875`). No new helper, opcode or builtin.
- [x] **M2.5.3** Text editor: in-memory buffer, cursor, scrolling, open via the file API;
  save disabled until a writable filesystem exists. `samples/apps/editor` is one 640×480
  window with an app-drawn File menu (Open, a disabled Save, Quit), a path/`read-only`
  line, a 26-row text view, a scrollbar and a status/`Ln`/`Col` line. The document is one
  `string[8192]` loaded through `core.fs_size` + 256-byte `core.fs_read` chunks and one
  strict-UTF-8 `core.substr_into`, so every load failure leaves the old document intact;
  editing is one app-local atomic mutator (`ed_replace`, boundary checks, prefix/insert/
  suffix staging in a local `bytes[8192]`, one publish) because the M2.4.1 mutators stay
  at 256 bytes, while the read-only `str_*` helpers work on the larger buffer. Keys,
  selection, vertical/horizontal scrolling, page clicks on the scrollbar, an injected
  routed key and drawing pixels are checked by `run editor check` (`editor: ok` on
  `-smp 4` and `-smp 1`); `langhost` runs the same logic (`editor: host ok`);
  `run editor shot` holds a frame for `scripts/editor_screenshot_qmp.py`
  (`igis-editor.png`). No new helper, opcode or builtin; spec 0003 records the
  above-256-byte-buffer contract. `648e1de`.
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
| Compile arena: 4.13 MiB of 8 MiB used by `stdlib` with a minimal app (115 KiB source); the `ui_icons` pair needs 5.39 MiB and the `widgets` pair 6.55 MiB | spec 0008 | watch as M2.5 grows the library |
| Guard table cache exhaustion (32 tables) | spec 0002/spec 0009 | long-lived workloads with many stacks |
| `cargo fmt` drift in three pre-existing files + CI fmt/clippy | repo | housekeeping |

## How a ticket is verified (headless first)

1. Host tests for the language/compiler/interpreter surface.
2. Kernel transcript with a self-check the app prints (`desktop: ok` style), using
   `syslib.get_pixel` for drawing and `syslib.inject_event` for input.
3. `-smp 4` and `--cores 1` both.
4. Regression set: `streams`, `frames`, `text`, `realtime`, `deep`, `cores`, `utf8`.
5. A screenshot dump once M2.1.2 lands, for the actual framebuffer pixels.
