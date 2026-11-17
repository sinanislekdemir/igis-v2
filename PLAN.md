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

- **Last commit:** `9d0b666` (M3.2: `uses lib.*` compiles `/lib` modules into the
  package; `SourceReader`, unit ids 0/1/2.., caps 384 KiB / 16 MiB,
  `samples/apps/lib/math.ig`) plus this docs commit. The tree is clean.
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
  and screenshot harness (`65f4875`), M2.5.3 the text editor plus its
  check/shot modes and screenshot harness (`648e1de`), M2.5.4 the file browser
  plus its check/shot modes, `langhost` test and screenshot harness, M2.5.5 the
  terminal window, M2.5.6 boot to the desktop, M2.6.1 the proportional font
  (`68d7ba1`), M2.6.2 the coalesced damage repair, batched console present and
  the `perf` measurement (`2c0df71`), M2.6.3 the deterministic specimen plus the
  headless golden-frame harness (`tests/golden/visual.png`), and M2.6.4 the System
  menu, About panel, `core.halt`/`core.reboot` and the halt/reboot probes.
- **Verified at that point (M3.2, independently by the parent):** **438 host tests**
  (abi 23, compiler 184, interpreter 150, langhost 80, 1 doctest); `cargo xtask
  build`/`image`/`image --shell` clean; boot still prints `ext2: tree ok` (step 6
  now requires `lib/math.ig`) and the arena change shows as `frames: 57903 free`
  (`.bss` +8 MiB). A hand-written source with `uses lib.math;` calling
  `math.clamp`/`abs_int`/`max_int`/`min_int`/`sign` runs on `langhost` and prints
  `mod_demo: ok`, proving dotted resolution, the unit-name qualifier and the host
  reader end to end. The typed shell transcript and the 26-command regression
  batch pass on `-smp 4` and `-smp 1`; `boot_desktop_qmp.py` reaches `terminal: ok`
  on both; the visual golden still matches on both
  (`37802052…e9e81c9f`). The in-kernel module path is deliberately not wired yet.
- **Next slice:** **M3.3 — kernel-side modules and the `/lib` proof** (spec 0018).
  A fresh subagent implements `compiler::SourceReader` over the mounted `Ext2`,
  switches `repl::compile` to `compile_with_imports`, adds
  `samples/apps/base/module_demo` importing `lib.math`, and proves `run
  module_demo` on `-smp 4` and `-smp 1` plus `langhost`.

- **M2.6.3 complete:** the headless visual-regression harness (spec 0009 "Visual proof",
  G21; ADR 0032) and the resolution of M2.1.2. `samples/apps/visual` opens a window whose
  frame is exactly the screen (content `screen_w - 2` × `screen_h - 17` at `(0, 0)`), so
  the service chrome plus the specimen cover both kernel text windows and every boot
  line. The specimen has no clock, animation, event or host input: it draws a fixed
  composition (focused prop title bar, prop and 8×8 font specimens, a nine-colour swatch
  row, a `(X + Y)` dither patch, a framed box, a diagonal raster and alternating bars),
  checks representative pixels (chrome, both fonts, palette, dither parity, outlines),
  presents the whole content once, prints `visual: ok` then `visual: ready`, and holds
  without drawing. `scripts/visual_regression.py` boots `build/igis-shell.iso` headless,
  runs `visual shot`, waits for the complete present, captures through QMP `screendump`,
  decodes to RGB and compares exactly with `tests/golden/visual.png` plus
  `tests/golden/visual.json` (SHA-256 of the raw pixels and one hash per 64×64 tile). A
  mismatch prints the differing pixel count, bounding box, first pixel and tiles and
  exits 1; `--update` is the only writer, `--tamper` corrupts one captured pixel in
  memory and requires the comparator to reject it, and `--expect-mismatch` inverts the
  result. The golden matches on `-smp 4` and `-smp 1` (SHA-256
  `37802052…e9e81c9f`, 1024×768). M2.1.2 is resolved by this host-side whole-framebuffer
  dump; no kernel serial dump or debug builtin is added. `langhost` runs the
  disabled-screen path (`visual: host ok`). No new opcode, builtin, `syslib` primitive,
  kernel table or cap change; `stdlib` stays at 125 functions. **376 host tests.**
- **M2.6.2 complete:** performance (spec 0016, ADR 0031). `kernel/src/window.rs`
  coalesces the two bounded damage lists before painting (empty/contained dropped,
  touching/overlapping merged; erase requests merge only within one coverage limit and a
  contained request with a higher-or-equal container limit is dropped), presents the
  coalesced rectangle set instead of one bounding box of all damage, and marks exposes
  before coalescing. `window::damage_self_check` proves the rules and the coverage
  invariant on scratch state and measures a diagonal move (4 rects / 510 874 px →
  2 rects / 55 348 px); boot prints `window: damage ok (present ...)`. `kernel/src/console.rs`
  replaces the per-cell `present_within` with a bounded dirty rectangle: `write_bytes` and
  `_print` flush once per byte run, the input echo defers to one `flush_pending` at the end
  of `poll_input`, and `repaint` presents each visible part once; the flush re-reads the
  visible parts under `WINDOW` and copies the dirty rectangle from the current shadow, and
  the lock order stays `CONSOLE → WINDOW → SCREEN`. `samples/apps/perf` measures frame time
  and event latency (`core.clock()`, min/avg/max plus the phase total; a 1 ms clock makes a
  single sample coarse and the average carries the signal) with no load and with
  `realtime` + three `worker` children, then reaps every child and prints the numbers;
  `langhost` prints `perf: no graphics` / `perf: host ok`. Specs 0011 (W21) and 0012 (D3)
  are amended; spec 0016 and ADR 0031 are new.
- **M2.6.1 complete:** the proportional (Chicago-like) 12-pixel bitmap font (spec 0015,
  ADR 0030). `kernel/src/font.rs` holds 95 printable-ASCII glyphs as 10-column ASCII
  art plus explicit advances; the first use renders a glyph into a fixed
  `SpinLock<[Option<Glyph>; 95]>` cache of row bitmasks and ink bounds. The line box is
  12 px, the baseline row 9, caps rows 2–9, descenders 10–11; advances are variable
  (`i` 3, `m` 7, `W` 10, `M` 11, digits tabular 6). Three host builtins (45–47) expose
  `text_width_prop`, `text_height_prop` and `draw_text_prop`; every byte outside
  `0x20..=0x7E` keeps an 8×8 cell with an 8-pixel advance (blank at or above `0x80`),
  and with graphics disabled the metrics are 0 and drawing is `Invalid`. The window
  service's title bar and the desktop shell's menu bar/clock use the proportional face
  (the title is truncated and centred by measured width; the menu hit ranges derive from
  the label widths). `draw_text`, the console and every `stdlib.ui_*` helper stay 8×8.
  `font: glyphs ok` at boot; `run font` prints `font: ok` on `-smp 4` and `-smp 1` and
  `font: host ok` on `langhost`; `samples/apps/font` checks variable widths (`ii < MM`,
  `Wi`), the `i`/`M` glyph pixels, the `é` fallback advance, the 8×8 path, clipping and
  the prop chrome title. `scripts/font_screenshot_qmp.py` writes
  `/home/sinan/Desktop/igis-font.png` (1024×768). `scripts/boot_desktop_qmp.py` clicks
  the moved `Terminal` item (x=170) and updates `igis-boot-desktop.png`. The console's
  D3 paint now holds the window lock (`window::paint_visible_parts`), which removed a
  reproducible `desktop_shell check` chrome overwrite on `-smp 4`.
- **M2.5.6 complete:** `repl::boot` tries `entrypoint` first and `shell` second. `xtask
  image` stages `samples/apps/desktop_shell` under the second file name `entrypoint` (the
  unit stays `desktop_shell`), so boot lands on the desktop without duplicating source;
  `xtask image --shell` omits the alias and writes `build/igis-shell.iso`, the pre-M2.5.6
  boot used to prove the fallback and to run the event-injecting regression suite.
  `desktop_shell`'s fourth direct menu item `Terminal` `core.spawn`s `shell` non-blocking
  and records the id; later clicks raise the live `Terminal` window (or spawn again after
  the shell exits), so the one-terminal rule is never violated and the desktop stays
  interactive. The desktop never attaches the terminal; the shell does. With graphics
  disabled the desktop prints `desktop_shell: no graphics` and spawns the text shell, so
  a serial REPL survives. `scripts/boot_desktop_qmp.py` boots the ISO with no manual
  `run`, waits for `desktop_shell: ready`, clicks `Terminal` via QMP absolute pointer
  events, types `terminal`, checks `terminal: ok`, and writes
  `/home/sinan/Desktop/igis-boot-desktop.png`; it passes on `-smp 4` and `-smp 1`. The
  desktop is a permanent consumer of the one machine-wide event queue (spec 0010 I31),
  so event-reading apps compete with it and the regression suite runs on the shell image;
  the shell's byte-stream terminal is unaffected. Desktop source 17,289 B, package
  14,952 B code / 752 B state / 1,136 B stack; desktop image root 57 entries, shell image
  56; `stdlib` 125 functions, 368 host tests, no cap change. **M2.5 is complete.**
- **M2.5.5 complete:** `core.terminal_attach(window_id) -> Status` (host builtin 44,
  spec 0014, ADR 0028) binds the one machine-wide console terminal grid to a window
  owned by the calling task; the kernel host passes the task id and checks ownership,
  the first attach releases the owner-0 boot Terminal, a second live task is `Full`, a
  repeated attach by the owner is `Ok`, and a disappeared window detaches to serial-only.
  `console::pump` pulls the attached terminal's expose by id and the System Log's by
  owner `0`, follows a moved window by generation, and keeps `CONSOLE → WINDOW →
  SCREEN`. `samples/apps/shell` opens a 560×360 `Terminal` window at `(24, 64)`, attaches
  before its banner and keeps the `core.write`/`core.read_line` loop unchanged; its
  `terminal` command reports the id, refuses an unknown id, checks ink and background
  pixels, and proves `Full` with `samples/apps/terminal_probe`. `run terminal` and
  `run desktop_shell check` coexist (the terminal, menu bar and Desktop window are
  visible together). 368 host tests; `terminal: ok` and the whole regression set pass on
  `-smp 4` and `-smp 1`; `scripts/terminal_screenshot_qmp.py` writes
  `igis-terminal.png`. The image root grows to 56 entries (55 staged sources plus
  `lost+found`); the shell package is 6432 B of code (2 frames), 1504 B of state and
  336 B of stack. `stdlib` stays at 125 functions and no cap changes.
- **M2.5.4 complete:** `samples/apps/browser` is one 680×480 window with a path/count
  line, a 26-row app-drawn root listing (folder/file/other marker, name, size, accent
  selection), a `ui_scrollbar`, a 2048-byte preview pane with line clipping and a status
  line. `core.fs_list` is read from index 0 until `NotFound` into 64 fixed entries;
  `.`/`..` are excluded by the API. `lost+found` is shown as a folder and opening or
  launching it is refused with a clear status (M2.5.1 has no subdirectory traversal); a
  kind-2 entry is shown but not openable. A regular file preview is `core.fs_size` +
  256-byte `core.fs_read` chunks into a staged `bytes[2048]` and one strict-UTF-8
  `core.substr_into`, so missing, directory, unsupported path, over-cap and invalid
  UTF-8 all set the status and leave the old preview untouched. Keys are
  Up/Down/PageUp/PageDown/Home/End, Enter and Ctrl+L (launch), with double-click
  preview; drawing is client content only (no `syslib.clear`). Launch is a blocking
  launcher: `core.spawn` then `core.wait`, regular source apps only; the proof launches
  `editor check` and sees it exit `Ok`, so no child is left running.
  `run browser check` prints `browser: ok` on `-smp 4` and `-smp 1`; `langhost` runs the
  same listing/preview/selection logic (`browser: host ok`);
  `scripts/browser_screenshot_qmp.py` writes `igis-browser.png`. No new helper, opcode
  or builtin (the library stays at 125 functions).
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
2. `cargo test -q -p abi -p compiler -p interpreter -p langhost` — expect **382** tests
   plus the doctests, all green.
3. `cargo xtask build` — expect a clean kernel build.
4. Read, in this order: this file, `AGENTS.md` (status paragraph + decisions table),
   `specs/0009-graphics-and-windows.md`, `specs/0010-input-and-events.md`,
   `specs/0011-windows.md`, `specs/0012-desktop-shell.md`,
   `specs/0013-application-files.md`, `specs/0014-terminal-window.md`,
   `specs/0015-proportional-font.md`, `specs/0016-performance.md`,
   `specs/0017-system-menu.md`, `adr/0020`, `adr/0021`, `adr/0022`, `adr/0023`,
   `adr/0024`, `adr/0025`, `adr/0026` (the `stdlib` UI layer), `adr/0027` (the
   stateless file API), `adr/0030`, `adr/0031`, `adr/0032` and `adr/0033` (system
   control).
5. Baseline kernel checks (headless, no chat needed):
   - Combined boot (no window-injecting proof app, so the shared event queue stays
     quiet between samples):
     `( sleep 6; printf 'run files\n'; sleep 8; \
       printf 'run text_edit\n'; sleep 10; printf 'run ui_layout\n'; sleep 6; \
       printf 'run ui_controls\n'; sleep 10; printf 'run ui_menus\n'; sleep 10; \
       printf 'run ui_dialogs\n'; sleep 10; printf 'run ui_icons\n'; sleep 12; \
       printf 'run widgets check\n'; sleep 20; \
       printf 'run clock_window check\n'; sleep 8; \
       printf 'run editor check\n'; sleep 12; printf 'run browser check\n'; sleep 12; \
       printf 'run desktop_shell check\n'; sleep 20; printf 'run desktop\n'; sleep 4; \
       printf 'run events\n'; sleep 8; printf 'run windows\n'; sleep 6; \
       printf 'run windows_events\n'; sleep 6; printf 'run windows_demo\n'; sleep 22; \
       printf 'run perf\n'; sleep 20; printf 'run font\n'; sleep 6; \
       printf 'run text 42 hi\n'; sleep 5; \
       printf 'run frames\n'; sleep 8; printf 'run streams\n'; sleep 8; \
       printf 'run deep\n'; sleep 5; printf 'run utf8\n'; sleep 5; \
       printf 'run realtime\n'; sleep 8; printf 'run cores\n'; sleep 8; \
       printf 'exit\n' ) | timeout 500s \
       qemu-system-x86_64 -cdrom build/igis-shell.iso -m 256M -serial stdio -monitor none \
       -no-reboot -no-shutdown -accel tcg -vga std -display none -smp 4 -usb \
       -device usb-tablet`
     → `console: terminal ok`, `console: log ok`, `console: route ok`,
     `graphics: clip ok`, `window: rules ok`, `window: paint ok`, `window: route ok`,
     `files: ok`, `text_edit: ok`, `ui_layout: ok`, `ui_controls: ok`, `ui_menus: ok`,
     `ui_dialogs: ok`, `ui_icons: open Document`, `ui_icons: ok`, `widgets: ok`,
     `clock_window: ok`, `editor: ok`, `browser: ok`, `desktop_shell: ok`, `desktop: ok`,
     `perf: ok`, `font: ok`, `text: ...`, `frames: done`, `streams: done`,
     `deep: ...`, `utf8: ...`, `realtime: bye`, `cores: 6 workers done`,
     `keyboard: decoder ok`, `events: ok`,
     `windows: ok`, `windows_events: ok`, `windows_demo: checks ok`,
     `windows_demo: ready`, `windows_demo: ok`, `ioapic: version ...`,
     `mouse: aux IRQ12 enabled ...`, `usb: tablet ...` when the device is present.
     From M2.5.6 these shell-driven recipes use the shell-entry image
     (`cargo xtask image --shell` → `build/igis-shell.iso`): the boot desktop is a
     permanent consumer of the one event queue, so event-injecting apps run in the
     pre-M2.5.6 boot. The `boot_desktop` harness below uses `build/igis.iso`.
   - Boot to the desktop: `cargo xtask image` then
     `python3 scripts/boot_desktop_qmp.py build/igis.iso` (and `--cores 1`) →
     `desktop_shell: ready`, `desktop_shell: launched shell task N`, `shell> `,
     `terminal: ok`, and a 1024×768 PNG at
     `/home/sinan/Desktop/igis-boot-desktop.png`.
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
   - `python3 scripts/desktop_screenshot_qmp.py build/igis-shell.iso` → a 1024×768 PNG at
     `/home/sinan/Desktop/igis-desktop.png`.
   - `python3 scripts/windows_screenshot_qmp.py build/igis-shell.iso` → a 1024×768 PNG at
     `/home/sinan/Desktop/igis-windows.png`.
   - `python3 scripts/windows_proof_screenshot_qmp.py build/igis-shell.iso` → a 1024×768 PNG
     at `/home/sinan/Desktop/igis-m23-proof.png`.
   - `python3 scripts/ui_controls_screenshot_qmp.py build/igis-shell.iso` → a 1024×768 PNG
     at `/home/sinan/Desktop/igis-controls.png`.
   - `python3 scripts/ui_menus_screenshot_qmp.py build/igis-shell.iso` → a 1024×768 PNG at
     `/home/sinan/Desktop/igis-menus.png`.
   - `python3 scripts/ui_dialogs_screenshot_qmp.py build/igis-shell.iso` → a 1024×768 PNG at
     `/home/sinan/Desktop/igis-dialogs.png`.
   - `python3 scripts/ui_icons_screenshot_qmp.py build/igis-shell.iso` → a 1024×768 PNG at
     `/home/sinan/Desktop/igis-icons.png`.
   - `python3 scripts/widgets_screenshot_qmp.py build/igis-shell.iso` → a 1024×768 PNG at
     `/home/sinan/Desktop/igis-widgets.png`.
   - `python3 scripts/clock_window_screenshot_qmp.py build/igis-shell.iso` → a 1024×768 PNG
     at `/home/sinan/Desktop/igis-clock.png`.
   - `python3 scripts/editor_screenshot_qmp.py build/igis-shell.iso` → a 1024×768 PNG at
     `/home/sinan/Desktop/igis-editor.png`.
   - `python3 scripts/browser_screenshot_qmp.py build/igis-shell.iso` → a 1024×768 PNG at
     `/home/sinan/Desktop/igis-browser.png`.
   - `python3 scripts/terminal_screenshot_qmp.py build/igis-shell.iso` → a 1024×768 PNG at
     `/home/sinan/Desktop/igis-terminal.png` (the attached shell terminal and the
     desktop shell together).
   - `python3 scripts/system_menu_qmp.py build/igis.iso` → `desktop_shell: about open`,
     `desktop_shell: about closed` and a 1024×768 PNG at
     `/home/sinan/Desktop/igis-system-menu.png` (the desktop entry, the System pull-down
     and About panel). Use `build/igis.iso`, not the shell image.
   - `python3 scripts/system_control_qmp.py build/igis-shell.iso --mode halt` →
     `system: 3 of 3 APs stopped` on `-smp 4` (or `0 of 0` on `-smp 1`) and no serial
     reply to a later command; `--mode reboot` → a second `igis-v2: long mode entered`
     banner. These runs stop/reset the machine and are **not** in the normal suite.
   - `python3 scripts/tablet_qmp.py build/igis-shell.iso` → `7/7 absolute cases passed`.
   - `python3 scripts/tablet_qmp.py build/igis-shell.iso --no-tablet` → `mouse: ok` fallback.
   - `python3 scripts/mouse_burst_qmp.py build/igis-shell.iso` → `600/600` px with a
     `mouse: aux IRQ12 delivered ...` line; `--cores 1` is the single-core variant.
   - `python3 scripts/visual_regression.py build/igis-shell.iso` → `visual: ok
     (1024x768, sha256 3780…)` for the deterministic specimen; `--cores 1` must match
     the same golden, `--tamper` must report a mismatch and exit 0, and a wrong
     `--golden-dir` must exit 1 with a pixel/tile diff.
6. **M2 is complete.** M2.6.4 (spec 0017, ADR 0033) adds the desktop's System pull-down
   (About igis-v2, Restart, Halt), a modal About window, `core.halt`/`core.reboot`
   (builtins 48/49) with the wake-vector stop path, and the focus-scoped
   `F10`/`Ctrl+A`/`Ctrl+R`/`Ctrl+H` shortcuts; `desktop_shell check`,
   `scripts/system_menu_qmp.py` and the separate `scripts/system_control_qmp.py`
   halt/reboot probes prove it. **M2.6.3 is complete**: the deterministic full-screen
   specimen (`samples/apps/visual`) and `scripts/visual_regression.py` compare a headless
   QMP `screendump` pixel-exactly with `tests/golden/visual.png` + `visual.json` on
   `-smp 4` and `-smp 1`; `--tamper` proves the compare fails; M2.1.2 is resolved by this
   host-side dump (spec 0009 G21, ADR 0032). **M2.6.2 is complete**: damage is
   coalesced before repair and presented as a tight rectangle set (89 % less present area
   on the measured diagonal move), the console batches cells into one present per byte
   run, and `samples/apps/perf` measures frame time and event latency under the realtime
   load on `-smp 4` and `-smp 1` (spec 0016, ADR 0031). **M2.6.1 is complete**: the
   12-pixel proportional font (spec 0015, ADR 0030) drives the service title bar and the
   desktop menu bar/clock, keeps 8×8 as the console/toolkit fallback, and is proved by
   `samples/apps/font`. **M2.5 is complete** with M2.5.6: boot lands on the desktop
   (`entrypoint`, staged from `desktop_shell`), the `Terminal` menu item spawns the shell
   non-blocking and later clicks raise it, and `scripts/boot_desktop_qmp.py` proves the
   boot-to-terminal path with a QMP menu click. M2.5.5's terminal attaches the one console
   grid to the shell's window; M2.5.4 is the file browser; M2.5.3 the text editor; M2.5.2
   the clock window; M2.5.1 `core.fs_list`, `core.fs_read` and `core.fs_size` over the
   image root.
   The M2 limitations still in force: an app that ignores an `Expose` keeps stale
   content, a full-screen `syslib.clear` erases the kernel windows' pixels, there is no
   per-task clip (a covered client must repaint only its visible part), there is no
   global work area (an app at `y = 0` covers the menu bar until the shell's next event),
   the boot desktop is a permanent consumer of the one event queue, so event-reading
   apps run in the shell-entry image until per-task event delivery lands, and Restart/
   Halt are direct actions with no capability check (ADR 0033).

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
- [x] **M2.1.2** Screenshot/debug dump: a whole-framebuffer visual proof for CI-style
  checks. Resolved by the **host-side** QMP `screendump` of the deterministic specimen
  plus an exact golden compare (`scripts/visual_regression.py`, spec 0009 G21, ADR 0032,
  M2.6.3); no kernel serial dump or debug builtin is added, because the display surface
  already carries the presented framebuffer and a serial dump would duplicate it.
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
- [x] **M2.5.4** File browser: directory list from the read-only ext2 app API, open/launch.
  `samples/apps/browser` is one 680×480 window with a path/count line, a 26-row
  app-drawn root listing (folder/file/other marker, name, size, accent selection), a
  `ui_scrollbar`, a 2048-byte preview pane with line clipping and a status line.
  `core.fs_list` is read from index 0 until `NotFound` into 64 fixed entries;
  `.`/`..` are excluded by the API. `lost+found` is shown as a folder and opening or
  launching it is refused with a clear status (M2.5.1 has no subdirectory traversal); a
  kind-2 entry is shown but not openable. A regular file preview is `core.fs_size` +
  256-byte `core.fs_read` chunks into a staged `bytes[2048]` and one strict-UTF-8
  `core.substr_into`, so missing, directory, unsupported path, over-cap and invalid
  UTF-8 all set the status and leave the old preview untouched. Keys are
  Up/Down/PageUp/PageDown/Home/End, Enter and Ctrl+L (launch), with double-click
  preview; drawing is client content only (no `syslib.clear`). Launch is a blocking
  launcher: `core.spawn` then `core.wait`, regular source apps only; the proof launches
  `editor check` and sees it exit `Ok`, so no child is left running.
  `run browser check` prints `browser: ok` on `-smp 4` and `-smp 1`; `langhost` runs the
  same listing/preview/selection logic (`browser: host ok`);
  `scripts/browser_screenshot_qmp.py` writes `igis-browser.png`. No new helper, opcode
  or builtin (125 `stdlib` functions).
- [x] **M2.5.5** Terminal window: the existing shell running in a window (`core.write` /
  `core.read_line` routed), so the REPL and the desktop coexist.
- [x] **M2.5.6** Boot to desktop: `entrypoint` becomes the desktop; the shell is launched
  from a menu item. `xtask image` stages `samples/apps/desktop_shell` under the
  `entrypoint` file name (the unit stays `desktop_shell`); `repl::boot` tries
  `entrypoint` then `shell`; `xtask image --shell` / `xtask run --shell` omit the alias
  (`build/igis-shell.iso`) to prove the fallback and to run the event-injecting suite.
  The desktop's direct `Terminal` item `core.spawn`s `shell` once (non-blocking) and
  later clicks raise the live window; the desktop never attaches the terminal, and a
  graphics-disabled boot spawns the text shell for a serial REPL.
  `scripts/boot_desktop_qmp.py` clicks `Terminal`, types `terminal`, checks
  `terminal: ok` and writes `igis-boot-desktop.png` on `-smp 4` and `-smp 1`.
  **M2.5 is complete.**

### M2.6 — polish and measurement

- [x] **M2.6.1** Proportional font (Chicago-like) with a glyph cache and text metrics;
  keep 8×8 as the fallback. Spec 0015, ADR 0030, `samples/apps/font`.
- [x] **M2.6.2** Performance: damage coalescing, present batching, measured event latency
  and frame time under load (realtime controller + UI together). Specs 0011 (W21) and
  0012 (D3), spec 0016, ADR 0031, `samples/apps/perf`.
- [x] **M2.6.3** Visual regression harness: `samples/apps/visual` draws a deterministic
  full-screen specimen (fixed chrome, prop and 8×8 fonts, palette, dither, outlines) with
  no clock or input and covers the kernel text windows; `scripts/visual_regression.py`
  boots `build/igis-shell.iso` headless, captures through QMP `screendump` and compares
  the decoded RGB pixels exactly with `tests/golden/visual.png` + `visual.json` (SHA-256
  plus 64×64 tile hashes) on `-smp 4` and `-smp 1`. `--update` is the only writer;
  `--tamper`/`--expect-mismatch` prove the compare fails; a mismatch prints the pixel
  count, bounding box, first pixel and tiles and exits 1. Resolves M2.1.2 as the
  host-side dump. Spec 0009 G21, ADR 0032.
- [x] **M2.6.4** System menu: About box, restart, halt; keyboard shortcuts.
  `desktop_shell` adds a `System` pull-down (About igis-v2 / Restart / Halt) built on
  the M2.4.3 menu model and the M2.4.4 dialog helpers, a modal About window, and the
  focus-scoped `F10`/`Ctrl+A`/`Ctrl+R`/`Ctrl+H` shortcuts. `core.halt`/`core.reboot`
  (builtins 48/49) stop every core on the wake vector and halt or reset; a firmware
  that ignores the reset channels ends in a halt. Proofs: the extended
  `desktop_shell check`, `scripts/system_menu_qmp.py`, and the separate
  `samples/apps/system_probe` + `scripts/system_control_qmp.py` runs for halt/reboot.
  Spec 0017, ADR 0033. **M2 is complete.**

### Post-M2 scheduler fixes

- [x] **Q50** `core.kill` of a task another core has claimed: both races fixed. The
  per-slot record is now an id-tagged `AtomicU64` (task ids are monotonic, so a record
  never reaches a later tenant), `request_kill` publishes under the slot lock,
  `task::block` re-checks and consumes its own record under that lock after the
  quantum-boundary check (the lost wake that hung `core.wait`), and `reclaim` clears the
  record and the reservation only for its own id. Boot self-check `kill: records ok`;
  new `samples/apps/kill_stress` + `kill_victim` (300 spawn/kill/wait iterations over the
  console, stream and input waits) passes on `-smp 4` and `-smp 1`, and hangs on its first
  iteration against the pre-fix kernel. Spec 0007 Sch17, ADR 0034, OPEN-QUESTIONS Q50
  resolved.

### Post-M2 deferred items closed

- [x] **Q39 console/stream/input multiplexing.** `core.poll_any` (non-blocking) and
  `core.next_any` (blocking) are host builtins 51/52 (no opcode) over a
  `const array[uint, 8]`: source `0` is one completed console line, `1` one input event
  (including a pending `Expose`), and any other value a stream handle whose data or close
  is ready (a read select). `core.stream_source` is compiler-lowered. One
  `abi::wait::WaitSet` (`Copy + Eq`, eight sources) is both `WaitKind::Any` and
  `WaitFor::Any`; `wake_console`/`wake_input`/`wake_stream` wake a matching set, and the
  scheduler's post-block `task::wait_any_ready` recheck closes the lost-wake window.
  Readiness is a snapshot: the following `read_line`/`next_event`/`recv` is authoritative.
  Proof `samples/apps/multiplex` (+ `multiplex_sender`) waits on all three, self-injects
  the input event and takes the console line typed by
  `scripts/multiplex_qmp.py`; `multiplex check` is deterministic. Specs 0003/0005/0006/
  0007, ADR 0036. (Source `0` is redefined as "a console byte is available" when Q44
  removes the line discipline; the rest of the source set is unchanged.)

- [x] **Q44 the kernel line discipline removed; the console is a raw byte stream.** COM1
  drains into a 512-byte ring with the newest byte dropped and counted on overflow; the
  PS/2 keyboard feeds only the event queue. `core.read_input(var dst: bytes[N]) -> int`
  reuses the `READ_LINE` opcode slot as `READ_INPUT` (`0x42`, ABI v1 stays 75): a raw,
  non-blocking read that publishes the destination length and returns the count, `0`
  when empty. Q39 source `0` is now a console byte being available
  (`Host::console_byte_ready`, `console::has_byte`), `WaitFor::Console`/`WaitKind::Console`
  are gone, and `task::wake_console_input` wakes matching sets. The shell echoes and
  edits its own `string[256]` with `stdlib.str_*`, selects on source `0`/`1`, and handles
  Backspace, Enter, Ctrl+C and Tab; `multiplex` reads its console arm with
  `core.read_input`, `kill_victim line` blocks in `core.next_any`, and `langhost` drains a
  scripted byte queue. Boot adds `console: input ok`. Proofs: a typed serial shell
  session (Backspace, `help`, `echo hi`, `run child world`, `exit`) on `-smp 4`/`-smp 1`,
  `kill_stress` 300/300 killed on both, `scripts/multiplex_qmp.py` on both, and the full
  regression set. Specs 0003/0005/0007/0008/0010/0012/0014, ADR 0037.

### Post-M2 hardening

- [x] **Q41 correctly rounded decimal → binary64 parsing.** `core.parse_f64_into(src,
  var value: f64) -> Status` is builtin 50 and pure interpreter code
  (`interpreter/src/f64parse.rs`): it checks spec 0003's grammar and converts with `core`'s
  allocation-free, correctly rounded `f64: FromStr`, writing the bits only on `Ok`.
  `stdlib.parse_f64_into` keeps its signature and delegates, so the function count, every
  caller and the format→parse round trip are unchanged. The old repeated ×/÷10 reader was
  1 ulp off (`1e-7`, the smallest-normal boundary text); the new one is pinned by exact bit
  tables, an exact `f64short` round trip, builtin tests, compiler signature/encoding tests,
  a `langhost` boundary test, and `samples/apps/f64parse` (`f64parse: ok 52 cases` on
  `-smp 4` and `-smp 1`). No opcode (ABI v1 stays at 75), no `Host` method, no cap change.
  Spec 0003 ("Floating-point text"), spec 0008, ADR 0035.

- [x] **C6 guarded-stack placement invariant.** `Region::allocate_guarded` now validates
  its constructed fields with the pure `Region::layout_ok` rule before returning: `frames
  >= 1`, `len >= 1`, `span == frames + 1`, `start == base + FRAME_SIZE`,
  `len <= frames * FRAME_SIZE`, the half-open `[start, start + len)` inside the frames
  above the guard and disjoint from `[base, base + FRAME_SIZE)`, overflow-safe at both
  ends, and a frame-aligned base. A mismatch restores the guard, frees the frames and
  returns `Status::Invalid` (the REPL reports it as a placement failure); only a failure
  to *restore* the guard still halts, as before. The boot self-check runs the same rule
  over valid/invalid scratch tuples (pure arithmetic, no allocation) and prints
  `stack: layout ok` beside the existing `guards:` line. This is the cheap mitigation
  for Q47: the interpreter is `#![forbid(unsafe_code)]` and bounds-checks its slices, so
  a mis-based/oversized region was the realistic route to a task-guard fault. Q47 itself
  stays **blocked** on lock-ownership tracking (OPEN-QUESTIONS Q47); no recovery code was
  shipped. Spec 0004 C6, spec 0002 H2.

### M2 completion

All M2.1–M2.6 tickets are done. The accepted M2 limitations remain documented and are
the next milestone's input: a client that ignores `Expose` keeps stale content, a
full-screen `syslib.clear` erases the kernel windows' pixels, there is no per-task clip
or global work area, the boot desktop is a permanent consumer of the one event queue
so event-injecting regressions run on the shell-entry image (spec 0012, D19), and the
system menu/About are application-drawn with direct Restart/Halt (ADRs 0032, 0033).


## M3 — filesystem hierarchy and importable modules

Normative: spec 0018, ADR 0038. Goal: a real image tree (`lib`, `base`, `home`,
`system`), a path-walking file API, and `uses lib.x;` compiled into the
application's package. One ticket per session; the parent verifies each.

- [x] **M3.1 The image tree, ext2 path traversal and boot from the tree.**
  Move `samples/apps/*` into `samples/apps/base/` (applications) and
  `samples/apps/system/` (`entrypoint`, `system_probe`, `terminal_probe`); add
  `samples/apps/home/readme.txt`; create `samples/apps/lib/` (empty until M3.2).
  - `abi::fs`: one L6 path rule for every call site (`validate_path`), plus
    `validate_dir`; delete `validate_root`/`validate_root_name`; update tests.
  - `kernel/src/ext2.rs`: `find(path)`, `list(path, index, name)`,
    `read_file(path)`, `read_at(path, ...)`, `file_size(path)`; 8-component depth
    cap, 64-byte components, no symlink following; boot self-check
    `ext2: tree ok` on scratch lookups.
  - `kernel/src/repl.rs`: boot tries `system/entrypoint` then `base/shell`; a
    spawn path with no `/` resolves to `base/<name>`; the source read is a path.
  - `kernel/src/sched.rs` `KernelHost::fs_*` and `langhost` `ConsoleHost::fs_*`
    pass paths through; `langhost` resolves a bare run name under `base` and
    module paths from the `samples/apps` root.
  - `xtask/src/main.rs`: stage the four directories with real directories; keep
    `base/desktop_shell` and copy it to `system/entrypoint` (the `--shell` image
    omits the copy); stage `system_probe`/`terminal_probe` under `system/`; no
    file at the image root.
  - Samples/scripts that address paths or counts: `files` (root lists 5,
    `base/child`), `browser` (`base/child`, `base/text`), `editor`
    (`base/child`), `terminal` (spawns `system/terminal_probe`),
    `scripts/system_control_qmp.py` (`run system/system_probe ...`); update
    printed counts and status text.
  - Verification: `cargo test -p abi -p compiler -p interpreter -p langhost`;
    `cargo xtask build`/`image`/`image --shell`; a typed serial transcript on
    `-smp 4` and `-smp 1` (`help`, `run files`, `run child world`, `run
    base/child world`, `exit`); `scripts/boot_desktop_qmp.py`;
    `scripts/visual_regression.py` golden unchanged. No language change.
  **Done (`0f08c54`):** 408 host tests; boot `ext2: tree ok` with a five-entry root;
  `run files`/`run child world`/`run base/child world`/`run system/system_probe` typed
  on both core counts; 26-command regression batch on both; `windows_proof` in its own
  boot; `boot_desktop_qmp.py` and the visual golden pass on both; `system_probe halt`
  resolves through `system/`.

- [x] **M3.2 Compiler multi-unit modules (`uses lib.*`).**
  Update the ext2 boot self-check first: its step 6 asserts `lib` is empty, which
  stops being true once `lib/math.ig` is staged (assert the file is listed instead).
  `compiler::SourceReader` + `compile_with_imports` (arena-copied unit sources);
  dotted resolution `lib.a.b` → `lib/a/b.ig`; `unit <last>` header check; unique
  unit names; cycle detection; modules may not declare `main`, top-level `var` or
  streams; unit ids (app 0, `stdlib` 1, modules 2.. in depth-first `uses` order);
  `interpreter::MAX_UNITS` 1 → 16; caps 384 KiB / 16 MiB; deterministic output.
  Add `samples/apps/lib/math.ig` (`clamp`, `min_int`, `max_int`, `abs_int`,
  `sign`) and a compiler/langhost test that imports it. `compile_into` keeps its
  signature and rejects `lib.*`. Verification: host tests only (compiler
  fixtures, in-memory reader, `langhost` runs a sample importing `lib.math`).
  **Done (`9d0b666`):** 438 host tests; `SourceReader`/`compile_with_imports`,
  `abi::MAX_UNIT_ID` 16, `MAX_MODULE_UNITS` 15, caps 384 KiB / 16 MiB, ext2 step 6
  updated, `lib/math.ig` staged; an inline `uses lib.math;` program runs on
  `langhost` (`mod_demo: ok`); boot `ext2: tree ok`, the 26-command batch on both
  core counts, `boot_desktop_qmp.py` and the visual golden still pass.

- [ ] **M3.3 Kernel-side modules and the `/lib` proof.**
  `repl::compile` uses `compile_with_imports` over the mounted `Ext2` (a reader
  that copies each module source into the compiler's arena, so the kernel
  application-source buffer stays 128 KiB); the arena is the M3.2 16 MiB; add
  `samples/apps/base/module_demo` (imports `lib.math`, prints
  `module_demo: ok`); `run module_demo` proves the whole path on `-smp 4` and
  `-smp 1`; `langhost` prints the host path. Update image entry counts and the
  regression set.

- [ ] **M3.4 Browser subdirectory navigation (Q51 partial).**
  `samples/apps/base/browser` descends into directories (`Enter` opens a
  directory, `Backspace`/`..`-equivalent goes up within the content view),
  previews files at any depth, and keeps the 2048-byte preview cap. Spec 0013
  "The file browser" amended; proof on both core counts.

- [ ] **M3.5 Docs, spec amendments and AGENTS status row.**
  Apply the 0018 amendments to specs 0003 (units/`uses`), 0008 (unit ids, caps),
  0012/0013 (boot path, file paths), refresh this resume point and add the
  AGENTS.md status row.

## Deferred (not UI; pick up when they matter)

| Item | Where | Trigger |
|---|---|---|
| `f32` (Q40) | OPEN-QUESTIONS Q40 | when single precision is needed |
| Recovery from a hardware guard hit (Q47) | OPEN-QUESTIONS Q47 | when a task must survive a kernel-bug guard hit |
| MMIO/DMA/block handles (Q45/Q9) | OPEN-QUESTIONS | first real block device or MMIO peripheral |
| Writable filesystem / persistence | none yet | when the editor must save |
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
5. The golden-frame harness (`scripts/visual_regression.py`) for the actual framebuffer
   pixels: `samples/apps/visual` is deterministic and the committed golden must match on
   `-smp 4` and `-smp 1`; `--tamper` proves the compare rejects a changed pixel.
