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

- **Current state (M12):** **613 host tests**; specs 0001–0022 (0021 and 0022
  Implemented); ABI v1 at 75 opcodes, with runtime-display builtins 80–83;
  **M2–M12 complete**. The Settings app (`apps/utilities/settings`) changes the
  real display mode through the Bochs VBE dispi (spec 0022, ADR 0056): presets,
  allocation-first shadow swap, window reflow and the Apps menu row. A window
  switch or a completed move runs a full-stack repaint (spec 0011 W31, ADR 0055).
  The terminal is an application emulator over kernel console sessions (spec 0021, ADR 0053), the
  image has top-level `apps/<category>/`, and the Apps pull-down is sectioned.
  `/home` is a separate 8 MiB ext2 image on an ATA disk, read-write through
  `ext2w`. `scripts/multi_terminal_qmp.py` proves two independent terminals.
- **Historical context (M8–M10; counts below are from those slices):** **505 host
  tests**; specs 0001–0019 (0018 and 0019 Implemented); ABI v1 at 75 opcodes; M5,
  M6, M7.1 and **M8.1–M8.9 complete**, plus **M9.1–M9.4**. The
  application image is a read-only ext2 tree (`lib`, `base`, `home`, `system`);
  `/home` is a separate 8 MiB ext2 image on an ATA disk, read-write through
  `ext2w` and one router, persistent across reboots. Write builtins are 53–57.
  Windows are draggable by the title bar, and the close box posts an owner-only
  `EventKind.Close` request (M8.2) — a dialog closes itself and the task survives.
  The desktop's `Apps` label is a real six-item application pull-down (M8.3), and a
  machine-wide modal popup input grab (`syslib.popup_grab`/`popup_release`, builtins
  58/59, no opcode) makes both pull-downs reachable over other windows. M8.4 adds a
  machine-wide persistent pointer-region claim (`syslib.region_claim`/`region_release`,
  builtins 60/61, no opcode): the desktop claims the menu-bar strip, so the press that
  opens a menu reaches it even while an application is blocked in `next_event`. M8.5 makes
  a dragged window's content follow its frame: a topmost move copies the content rectangle
  to the new origin before the repair (`gfx::copy_rect`, kernel-internal, no `syslib`
  binding) and omits the new-frame desktop erase, and every windowed app re-reads
  `window_content_rect` on an owner `Expose` before drawing or hit-testing (spec 0011
  W28, ADR 0046). M8.6 makes queued input **window-addressed** (spec 0010 I34, spec
  0011 W29, ADR 0047): `poll_event` delivers a record only to the task that owns its
  window target (an addressed window, the region claimant, the topmost hit window or
  the focused window) and a non-owner gets `WouldBlock` without consuming it, so the
  boot desktop no longer steals a launched application's clicks and keys. The modal
  grab still takes precedence, a region press stays unchanged, and readiness uses the
  same rule; `window: target ok` is the boot self-check and
  `scripts/input_routing_qmp.py` proves Browser, About and Calculator input with the
  desktop alive. M8.7 closes only the `Desktop` help panel when its close box is used;
  the shell keeps running, the Apple square reopens the window, and a lost desktop
  window can no longer halt the boot system. M8.8 adds a visible `Up` button to the
  File Browser path line (the keys remain), so the parent directory is reachable with
  the pointer alone. M8.9 makes the File Browser's scrollbar thumb draggable; the track
  still pages and the selection stays visible. M9.1 adds lexical syntax highlighting to
  the text editor: source paths (`base/`, `lib/`, `system/`, `.ig`) start coloured,
  keywords/types/strings/comments/numbers have fixed palette colours, `{ ... }` block
  state carries across visible lines without splitting UTF-8 code points, and a checked
  File > Highlight row toggles the view; old plain files are unchanged and no
  opcode/builtin/`stdlib` surface was added. M9.2 makes the editor's pointer path
  cheap: a pointer event repaints only when it changes visible state, the
  scrollbar thumb drags like the browser's, and the block-comment state is
  cached per line start and refreshed once per document publication. The
  user-reported `Faulted(BudgetExceeded)` after 2,893 claims was the normal 10 s
  CPU watchdog (a full repaint per pointer move), not a crash; the scheduler
  budget and realtime policy are unchanged. M9.3 adds a reusable `/lib` file-open
  dialog (`lib.file_dialog.open_file`) and wires the editor's File > Open and
  Ctrl+O to it: the modal window starts in the current directory, navigates the
  image tree with keys, pointer, scrollbar and buttons, and returns a chosen
  regular-file path or a null-like `Invalid` cancel with the destination
  untouched; the editor loads the result through its existing `open_path`. No
  kernel, ABI, opcode, builtin, file-API or `stdlib` change. M9.4 raises the
  text editor's document buffer from 8 KiB to 131072 B (128 KiB, the spec 0008
  per-unit source cap), so every current source sample opens; the reported
  case was `base/calculator` (32,516 B) through the dialog, which the old
  limit refused with a status line. The editor state is 530,392 B and its task
  stack 174,328 B, both under the 1 MiB region caps; `editor check` proves the
  full `base/calculator` load and the file-dialog QMP harness proves the
  user-facing path.
  **M10.1–M10.4 first slice is implemented:** spec 0020 (Accepted) and ADRs
  0048–0050 (Accepted) are in the tree; the legacy virtio-net driver, the
  allocation-free `net` crate (Ethernet/ARP/IPv4/ICMP/UDP/TCP client), the
  `core.net_*`/`core.socket_*` builtins (62–68), `lib/socket.ig`, `lib/http.ig`,
  `lib/dns.ig`, `lib/dhcp.ig` and `base/net_probe` work; a polling e1000/e1000e
  driver runs behind the same `NetDevice` seam and `xtask --net-device`
  selects virtio/e1000/e1000e. A `--shell` boot fetches
  `http://10.0.2.2:6666/` over a real TCP connection and prints
  `net_probe: ok` on `-smp 4` and `-smp 1` (1,406 body bytes); `net_probe dns`
  resolves `example.com` through `10.0.2.3`; `net_probe dhcp` obtains
  `10.0.2.15/24` from slirp and then fetches the page. Sockets now have both
  forms: the blocking `send`/`recv`/`accept` sleeps on `WaitFor::Net(handle)` and
  the non-blocking retry remains for `core.next_any`/timeouts. **M10.6 (done,
  `2051069`):** the NIC's PCI INTx line
  (GSI 11) is routed through the I/O APIC to the boot core as vector `0x2D`; the
  handler only sets a flag, acknowledges the device and EOIs, and the used-ring
  drain stays in `net::service` (ADR 0051). `xtask --net-irq intx|poll` selects
  the path through the multiboot2 command line, `core.net_stats` (73) exposes the
  counters, and `base/net_perf` proves ten timed HTTP gets with them;
  `net_probe http`/`dns`/`dhcp` and `net_perf` pass with INTx and with polling on
  `-smp 4`/`-smp 1` over virtio, e1000 and e1000e. **M10.7 (done, `efeeadf`):**
  `KIND_LISTEN` with a two-handle pending queue, inbound SYN/SYN-ACK/ACK,
  `core.socket_accept` (72), `lib/socket.listen/accept` and the `net_echo` server
  (three sequential host clients through `hostfwd`), proven over virtio, e1000 and
  e1000e. **Blocking follow-up (done, `5ae91bc`):** `WaitFor::Net(handle)` and
  `WaitFor::Deadline(ns)`, a per-socket deadline (`core.socket_timeout`, builtin
  71), `core.socket_source` (`1 << 63 | handle`) in `next_any`/`poll_any`, and
  `net_select` (selector + 300 ms recv/select timeouts); `net_echo`'s `accept`
  blocks, `kill_stress` is 300/300 and `mouse` 600/600.
- **Last commit:** `af27de8` (DNS `langhost` test) plus the docs commit.
  `build/home.img` is reset.
- **Next:** **M10 is complete.** Pick from the deferred backlog in
  [`OPEN-QUESTIONS.md`](OPEN-QUESTIONS.md) (TLS Q52, IPv6 Q53, Wi-Fi Q54,
  multi-NIC Q55, backlog Q56, DHCP/DNS follow-ups Q57) or the non-network items.
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
- **Done since M2:** M3.1–M3.5 (image tree, `/lib` modules, browser
  navigation), M4.1–M4.3 (package-wide nominal types and the `/lib` refactor),
  M5.1–M5.2 (calculator, minesweeper), M6.1–M6.4 (persistent writable `/home`)
  and M7.1 (`core.fs_truncate`). Each ticket section below has its commit hashes
  and evidence.
- **Done since M7.1:** M8.1 (window drag and close box), M8.2 (owner close requests),
  M8.3 (Apps application menu and the modal popup input grab, `94187ed`), M8.4
  (persistent menu-bar pointer-region claim, `1304df8`), M8.5 (topmost content
  snapshot plus the client content-rect refresh, `f478f9b`), M8.6 (window-addressed
  input delivery, `3598fda`), M8.7 (the `Desktop` help panel no longer owns the
  shell's lifetime, `5c97e39`), M8.8 (the browser's visible `Up` button,
  `c69e392`), M8.9 (the browser's draggable scrollbar thumb, `d3087c6`),
  M9.1 (lexical syntax highlighting in the text editor, `a912765`) and
  M9.2 (the editor's cheap pointer path, draggable thumb and block-state
  cache, `09dc57b`), M9.3 (the `/lib` file-open dialog and the editor's
  File > Open / Ctrl+O wiring, `be544fa`), and M9.4 (the editor's 128 KiB
  document buffer, `142c0f2`).
- **Verified at that point (M3.4, independently by the parent):** **439 host tests**
  (abi 23, compiler 184, interpreter 150, langhost 81, 1 doctest); `cargo xtask
  build`/`image`/`image --shell` clean; boot `ext2: tree ok`. `run browser check`
  prints `browser: ok` on `-smp 4` and `-smp 1` (the check asserts the root's
  five entries, `base` preview `base/child`, up, `system/system_probe`,
  `home/readme.txt`, `lib/math.ig` as kind 0, a 255-byte path refused, Ctrl+L
  descent, Ctrl+Up, kind-2 refusal and `base/editor check` launched by full
  path); the 27-command batch passes on both; `boot_desktop_qmp.py` reaches
  `terminal: ok` on both; the visual golden matches on both
  (`37802052…e9e81c9f`); the browser screenshot harness writes a 1024×768
  `/home/sinan/Desktop/igis-browser.png`.
- **Verified at that point (M4.2, independently by the parent):** **450 host tests**
  (abi 23, compiler 195, interpreter 151 incl. doctest, langhost 81); one
  package-wide type pool with the fixed predeclared prefix and per-unit scopes;
  `lib/probe.ig` exposes `Point`/`Shape`/`Color`, and `module_demo` prints
  `module_demo: types ok`, `module_demo: lib types ok`, `module_demo: ok` on
  `langhost`, `-smp 4` and `-smp 1`; the 27-command batch, visual golden
  (`37802052…e9e81c9f`) and `boot_desktop_qmp.py` pass on both; boot
  `ext2: tree ok` with `frames: 57890 free` (`.bss` unchanged, ~36 KiB of linked
  compiler code). The agent also compared `code`/`manifest` digests of five
  samples against `e46f21e`: byte-identical.
- **Verified at that point (M4.3, independently by the parent):** **450 host tests**;
  40 migrated apps plus 3 modules; the 27-command batch passes on `-smp 4` and
  `-smp 1`; the visual golden (`37802052…e9e81c9f`), `boot_desktop_qmp.py` and
  `scripts/browser_screenshot_qmp.py` pass on both; `events`, `windows` and
  `windows_events` were re-run on both after renaming `lib/reason` to
  `lib/outcome` (`outcome.is_ok(reason)`), so a module name cannot shadow the
  apps' common `reason` variable.
- **Verified at that point (M5.2, independently by the parent):** **452 host tests**;
  `run minesweeper check` → `minesweeper: ok` on `-smp 4` and `-smp 1`;
  `langhost` → `minesweeper: host ok`; the 29-command batch passes on both;
  visual golden (`37802052…e9e81c9f`), `boot_desktop_qmp.py` and
  `scripts/minesweeper_screenshot_qmp.py` (1024×768, ~36.9 KB) pass on both.
  Ctrl+Q was re-tested with a correct serial offset: the real decoder reports
  uppercase letter codes, so `calculator`, `editor`, `browser` and `minesweeper`
  now accept both shapes and QMP `send-key` exits each (`task 6..9: exit Ok`).
  This also fixed a pre-existing bug in the editor/browser shortcuts.
- **Verified at that point (M6.4, independently by the parent):** **486 host tests**;
  `scripts/home_persistence_qmp.py --cores 4 --reset --keep` and a second cycle
  `--cores 1 --keep` both print `home_persist: ok` in boot 1 and
  `home_verify: ok` in boot 2; `debugfs -R "cat /persist/b.txt"` shows the exact
  34-byte payload; `e2fsck -fn` exits 0 (14/2048 files, 565/8192 blocks) and the
  idempotent second cycle stays at 14/565; the image was reset afterwards
  (12/563, fsck 0). Targeted checks pass on both counts with the drive
  (`home_persist`/`home_verify`/`home_ops`/`editor`/`browser`/`files`) and
  without it (`home_persist: no writable home`, `home_verify: FAIL`, the
  read-only editor/browser arms). Visual golden (`37802052…e9e81c9f`) and
  `boot_desktop_qmp.py` pass on both.
- **Verified at that point (M7.1, independently by the parent):** **494 host
  tests**; `run home_truncate` → `home_truncate: ok` on `-smp 4` and `-smp 1`;
  `home_write`/`home_ops`/`editor check`/`browser check`/`files` still pass;
  `e2fsck -fn` exits 0 after the proofs (15 files) and after reset (12/563);
  without the disk `home_truncate: no writable home` and the read-only arms
  pass; the persistence harness, visual golden (`37802052…e9e81c9f`) and
  `boot_desktop_qmp.py` pass on both.
- **Next slice:** **M8.2 is complete.** Interactive resize (spec 0011 open
  question 4) is the obvious next window-management slice but is not yet
  planned; the deferred table is the input (virtio-blk, a journal/atomic replace,
  the handle model/Q51, `f32`/Q40, MMIO/DMA/Q45, nominal `TaskId`/Q43,
  runtime-capacity buffers/Q46, stream close/Q10, guard recovery/Q47, module
  dead-code elimination, `cargo fmt`/CI).

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
  **Optional, deferred:** not planned; the visual golden currently pins the
  1024×768 mode GRUB returns.

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
  `samples/home/readme.txt`; create `samples/apps/lib/` (empty until M3.2).
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

- [x] **M3.3 Kernel-side modules and the `/lib` proof.**
  `repl::load` uses `compile_with_imports` over the mounted `Ext2` (a reader
  that copies each module source into the compiler's arena, so the kernel
  application-source buffer stays 128 KiB); the arena is the M3.2 16 MiB; add
  `samples/apps/base/module_demo.ig` (imports `lib.math`, prints
  `module_demo: ok`); `run module_demo` proves the whole path on `-smp 4` and
  `-smp 1`; `langhost` prints the host path. Update image entry counts and the
  regression set.
  **Done (`039b861`):** 439 host tests; `ImageReader` over `Ext2`; `repl::load`
  now calls `compile_with_imports` (lock order unchanged, `SOURCE` 128 KiB, arena
  16 MiB); `base/module_demo` exercises all five `math` helpers and prints
  `module_demo: ok` on `langhost`, `-smp 4` and `-smp 1`; a missing app and a
  module file used as an app both fail with a clean `Diagnostic` and return the
  prompt; the 27-command batch, visual golden and `boot_desktop_qmp.py` pass on
  both; the kernel image grew ~17 KiB of linked code (`.bss` unchanged, free
  frames 57899).

- [x] **M3.4 Browser subdirectory navigation (Q51 partial).**
  `samples/apps/base/browser.ig` descends into directories (`Enter` opens a
  directory, `Backspace`/`Ctrl+Up` truncates the current path to go up),
  previews and launches files at any depth by full path, and keeps the
  2048-byte preview cap. Spec 0013 amended; proof on both core counts.
  **Done (`d669146`):** 439 host tests; `run browser check` → `browser: ok` on
  `-smp 4`/`-smp 1` (root → `base` preview → up → `system` → `home/readme.txt` →
  `lib/math.ig` → long-path refusal → Ctrl+L descent → Ctrl+Up → kind-2 refusal
  → `base/editor check` launched by full path); the 27-command batch, visual
  golden and `boot_desktop_qmp.py` pass on both; the browser screenshot harness
  writes `/home/sinan/Desktop/igis-browser.png` (33,016 B, 1024×768).

- [x] **M3.5 Docs, spec amendments and AGENTS status row.**
  Applied the 0018 amendments to spec 0003 (modules M2a/M3/M4/M5, the file-API
  tree paragraph), spec 0008 (multi-unit and caps, in M3.2), spec 0012 (D9 boot
  path), spec 0013 (tree view and browser navigation), spec 0018 (Implemented),
  OPEN-QUESTIONS Q51 (subdirectories resolved) and the AGENTS narrative plus the
  M3.1–M3.4 status rows. Done by the parent; see the docs commit.

## M4 — package-wide types and the `/lib` refactor

Normative: ADR 0039, spec 0008 "Types", spec 0003 "Cross-unit types". Goal: a
`/lib` module can expose real APIs (predeclared and its own records, sums and
enums), then the duplicated application helpers move into `/lib`. One ticket per
session; the parent verifies each.

- [x] **M4.1 Predeclared nominal types cross a unit boundary (C1).**
  Relax the one compiler gate (`cross_unit_type_ok`) so `Status`, `FaultCode`,
  `EventKind`, `Diagnostic`, `Event`, `Rect` and `ExitReason` — and arrays/rings/maps
  of them — may appear in another unit's parameters and results; user-declared types
  stay rejected until M4.2. Add `samples/apps/lib/probe.ig` (a `Rect` area helper, an
  `ExitReason` classifier using `case`, a `Diagnostic` field read, an `Event` kind
  helper) and extend `samples/apps/base/module_demo.ig` to call all four with real
  values; compiler tests for the accepted and still-rejected cases. Prove on
  `langhost`, `-smp 4` and `-smp 1`. No ABI, verifier or runtime change.
  **Done (`a218832`, `6a37202`):** 441 host tests; the gate admits record ids
  ≤ `RECT_RECORD`, sum id `EXIT_REASON_SUM` and enum ids ≤ `EVENT_KIND_ENUM`
  (`EventKind` added by the follow-up); `lib/probe.ig` + `module_demo` print
  `module_demo: lib types ok` then `module_demo: ok` on `langhost`, `-smp 4` and
  `-smp 1`; the 27-command batch, visual golden and `boot_desktop_qmp.py` pass on
  both; user aggregates still rejected with a precise diagnostic.

- [x] **M4.2 Package-global user types and unit-qualified names (C2).**
  Assign package-global enum/record/sum ids (predeclared fixed, then `stdlib`,
  modules in discovery order, application, declaration order inside a unit);
  allow forward references; reject by-value cycles. Parse and resolve
  `<unit>.<Type>` in every type position and `<unit>.<Type>.<Variant>` for enum
  constants; keep sum construction/`case` arms unqualified. The application's
  types stay application-local. Compiler tests for module records/sums/enums
  crossing both ways, nested and array element types, qualified enum constants,
  duplicate names across units, application-type leakage, and cycles. Runtime
  proof through `langhost` and the kernel.
  **Done (`3ac1725`):** 450 host tests; one package-wide type pool (predeclared
  prefix, then `stdlib` → modules → application) with per-unit scopes,
  two-phase declare/layout, on-demand layout and cycle detection; `TypeRef.owner`
  and three-part enum constants; the M4.1 gate deleted. `lib/probe.ig` gains
  `Point`/`Shape`/`Color` and `module_demo` prints
  `module_demo: types ok` + `lib types ok` + `ok` on `langhost`, `-smp 4` and
  `-smp 1`. The 27-command batch, visual golden and `boot_desktop_qmp.py` pass on
  both; five samples' code/manifest digests are byte-identical to `e46f21e`
  (no artifact change). Known polish: a foreign sum-variant `case` error names
  the owning sum without its unit qualifier.

- [x] **M4.3 Extract the shared helpers into `/lib` and migrate the apps.**
  Move the duplicated application helpers into modules (event/pointer loop
  helpers taking `Event`, reason classification taking `ExitReason`, text/number
  writers) and migrate the applications, with a before/after source-size and
  package report; prove the same verdicts on both core counts.
  **Done (`a0c6ae2`):** `lib/out.ig` (`say`, `say_int`), `lib/inject.ig`
  (`pointer_to/down/up`, `inject_key`, `inject_key_mods`) and
  `lib/outcome.ig` (`is_ok(const ExitReason)`; renamed from `reason` after the
  commit so the module name cannot shadow an app's common `reason` variable).
  40 applications migrated: 38 `say` and 11 `say_int` bodies deleted, 3
  `classify_reason` bodies delegated, 5 pointer/key wrappers delegated, unused
  staging state removed. Duplicated bodies are gone; net touched-tree source is
  +523 B because every call gains an `out.` qualifier and the modules carry
  comments (per file −282…+127; 40 files 463,961 → 460,666 B). Package code can
  grow slightly because a module's functions are linked wholesale
  (`ui_controls` 110,544 → 111,296 B); `visual` code is unchanged and its state
  shrank (304 → 272 B). 450 host tests; the 27-command batch, visual golden
  (`37802052…e9e81c9f`), `boot_desktop_qmp.py` and the browser screenshot pass on
  both core counts; `events`/`windows`/`windows_events` rechecked after the
  rename.

## M5 — two applications (calculator and minesweeper)

No new system surface: both are ordinary windowed applications over the existing
`stdlib` UI helpers, `syslib` primitives, events and `/lib` modules. They prove the
toolkit on real programs before the writable-filesystem milestone.

- [x] **M5.1 The calculator.** `samples/apps/base/calculator.ig`: one window
  (content 300×360) with a right-aligned f64 display and a 4×5 button grid
  (`C`, `DEL`, `+/-`, `/`; `7 8 9 *`; `4 5 6 -`; `1 2 3 +`; `0 . =`), drawn with
  `ui_button`/`ui_button_hit`/`ui_button_pressed` and `ui_focus_ring`. Typed entry
  is a `string[32]`; `core.parse_f64_into`/`core.format_f64_short_into` convert;
  `+ - * /` chain, division by zero shows `error`, `C` resets, `DEL` removes the
  last byte, `+/-` toggles the sign. Keyboard: digits, `.`, operators, Enter/`=`,
  Escape (`C`), Backspace, and Ctrl+Q quits (added after review so `run
  calculator` does not block the shell forever). One complete frame before
  `window_present`, owner `Expose` repaints, no `syslib.clear`. `check` injects
  pointer and key events and checks display/button pixels (`calculator: ok`);
  `shot` holds for a screenshot; `langhost` prints `calculator: host ok`.
  `scripts/calculator_screenshot_qmp.py` writes
  `/home/sinan/Desktop/igis-calculator.png`. No new helper, opcode, builtin,
  `state` or cap.
  **Done (`d57041a` + the Ctrl+Q follow-up):** 31,312 B source, task package
  116,328 B code (29 frames) / 480 B state / 21,872 B stack; 451 host tests;
  `calculator: ok` on `-smp 4`/`-smp 1`, `calculator: host ok`; Ctrl+Q ends the
  task (QMP `send-key`); the 28-command batch, visual golden, `boot_desktop_qmp.py`
  and the calculator screenshot pass on both.

- [x] **M5.2 Minesweeper.** `samples/apps/base/minesweeper.ig`: one window (content
  300×400), a 9×9 field of 30-pixel cells, 10 mines, a header with a mine
  counter, a restart face button and `MM:SS` elapsed time, and a status line.
  Mines come from an app-local deterministic LCG (seeded by `core.clock()` in
  play mode, fixed in `check`); the first reveal is never a mine. Left-click
  reveals with a flood fill over zero-neighbour cells, right-click (the
  `pointer_buttons` mask bit 2) toggles a flag, and a `Flag` mode button gives
  the same action without a second button. A mine ends the game and shows the
  board; revealing every non-mine cell wins; the face restarts. `check` uses the
  fixed seed, injects clicks, and checks flood fill, flags, win/loss and pixels
  (`minesweeper: ok`); `shot` holds; `langhost` prints `minesweeper: host ok`;
  `scripts/minesweeper_screenshot_qmp.py` writes
  `/home/sinan/Desktop/igis-minesweeper.png`. No new helper, opcode, builtin,
  `state` or cap.
  **Done (`ce01077` + the Ctrl+Q key-shape fix):** 36,198 B source, package
  125,568 B code (31 frames) / 3,720 B state / 23,008 B stack; 452 host tests;
  `minesweeper: ok` and `minesweeper: host ok`; 29-command batch, visual golden,
  `boot_desktop_qmp.py` and the screenshot pass on both counts. The ticket
  surfaced a pre-existing shortcut bug: the keyboard decoder reports letter key
  codes uppercase (`Q` = 0x51), while `calculator`/`editor`/`browser` compared
  the lower-case forms (0x71, 0x73/0x4F, 0x6C); all now accept both, and QMP
  `send-key` Ctrl+Q exits each app (`task 6..9: exit Ok`).

## M6 — the writable home filesystem

Normative: spec 0019, ADR 0040. Goal: a real, persistent ext2 `/home` on a
dedicated disk, reached through one path router. One ticket per session; the
parent verifies each.

- [x] **M6.1 ATA disk, home image, `ext2w` read path and the router.**
  New `ext2w` crate (`no_std`, `SectorDevice`, allocation-free) with a read-only
  mount (superblock/group/inode validation, lookup, `list`, `read_at`,
  `read_file`, `file_size`) and host tests against an `mke2fs` image in a
  temporary file. `kernel/src/ata.rs`: primary-master ATA PIO identify and
  512-byte sector read (LBA28, bounded polling, reset; no DMA/interrupts), behind
  `ext2w::SectorDevice`; `arch_x86_64::port` gains `inw`/`outw`/`insw`/`outsw`.
  `xtask`: `build/home.img` (8 MiB, 1 KiB blocks, initial tree from
  `samples/home/`), created when missing, `--reset-home`, `run` attaches
  `-drive if=ide`, `--no-home` omits it. `kernel/src/fs.rs` router: `home/*` from
  the writable mount (fallback to the application image when absent), everything
  else from the read-only image; the file builtins, `repl::load` and
  `core.spawn` use it. Boot prints `ext2w: mounted ...` or `ext2w: no home disk`
  and the self-check `ext2w: home ok` (read `readme.txt`, list the root; no write
  yet). Prove with the drive attached and without it, both core counts, and the
  existing regression set.
  **Done (`866b976`):** 458 host tests (6 new `ext2w` tests against an `mke2fs`
  image); `ext2w` (mount/validation/lookup/list/read), `kernel/src/ata.rs` (ATA
  PIO primary master, LBA28, bounded polling), `kernel/src/fs.rs` router
  (`home` → home mount or the app image's `home`), `build/home.img` (8 MiB, from
  `samples/home/`), `xtask` `--reset-home`/`--no-home` and the IDE drive. Boot
  with the drive prints `ata: …` / `ext2w: mounted` / `ext2w: home ok`; without
  it `ata: no primary master` / `ext2w: no home disk`; an invalid image prints
  `ext2w: home failed` and the system boots read-only. The 29-command batch
  passes in all four combinations (`-smp 4`/`-smp 1` × drive/no drive);
  `run files` exercises the home listing and a 238-byte `home/readme.txt` read;
  `e2fsck -fn` exits 0; visual golden and `boot_desktop_qmp.py` pass on both.

- [x] **M6.2 The ext2 writer and `core.fs_write`.**
  `ext2w` gains block/inode allocation, inode and superblock/group count
  updates, block-pointer growth (direct/single/double indirect), writing at an
  offset with gap zero-fill, implicit create, and the write-through block cache.
  Host tests over the in-memory device. `core.fs_write(path, offset, data, var
  written) -> Status` (builtin 53) plus interpreter/compiler checks; writes only
  under `home/`. Kernel proof: write a file, read it back, `e2fsck -fn` clean.
  **Done (`a06feb1`):** 474 host tests (9 new `ext2w` write tests including
  `e2fsck -fn` on the mutated image); writable mount marks the fs clean, allocates
  blocks/inodes (bitmap → group descriptor → superblock), plans capacity before
  mutating, writes data/inode/dirent in order; `core.fs_write` (builtin 53) is
  `fs_write_call` in codegen, `validate_writable` in `abi::fs`, a router
  create-on-offset-0; `samples/tests/apps/base/home_write.ig` prints nine step lines then
  `home_write: ok` on `-smp 4`/`-smp 1` and on `langhost` (which now has a
  temporary home), and `home_write: no writable home` without the disk;
  `e2fsck -fn` exits 0 after the writes (12 → 13 files, 563 → 564 blocks) and
  after reset; the 30-command batch with/without the drive, visual golden and
  `boot_desktop_qmp.py` pass on both.

- [x] **M6.3 `remove`, `mkdir`, `rename` and the applications.**
  Directory entry add/remove, `mkdir` (`.`/`..`, link counts), `remove` (empty
  dir or file, free blocks/inodes), `rename` within and across directories with a
  regular-file replace; builtins 54–56. Editor: File-menu Save enabled, `Ctrl+S`
  writes a sibling temporary and renames it over the document, status on success
  and failure. Browser: create-directory and remove actions in `home/`. Host
  `langhost` home over a temporary directory; e2e tests for all four primitives
  and the editor save.
  **Done (`2b2a910`):** 484 host tests; `ext2w` frees blocks/inodes (direct,
  single, double indirect), compacts directory tails, `mkdir` with `.`/`..`,
  links and `bg_used_dirs_count`, and `rename` with same/cross-directory moves,
  regular-file replace, `..` rewrite and directory-target refusal; builtins 54–56
  (`fs_remove`/`fs_mkdir`/`fs_rename`, ABI v1 stays at 75) with the `home`-only
  rule; the editor's Save/Ctrl+S writes `<target>.tmp` and renames it (a `base/`
  document is saved as `home/<basename>`), the browser's `N`/`Delete` create and
  remove under `home`, and `home_ops` proves the whole cycle. Kernel proofs on
  `-smp 4`/`-smp 1`: `home_ops: ok`, `editor: ok`, `browser: ok`, `files: ok`;
  `e2fsck -fn` exits 0 (12 → 13 → 14 files) and after reset; the 31-command
  batch with/without the drive, visual golden and `boot_desktop_qmp.py` pass on
  both.

- [x] **M6.4 Persistence, `e2fsck` and the M6 close.**
  A persistence harness: boot with `build/home.img`, write from an app, reboot or
  power-cycle QEMU with the same image, boot, read it back; `e2fsck -fn` exits 0
  after the write and after a remove/rename/mkdir session. Full regression on
  both core counts with and without the disk; visual golden and boot desktop
  unchanged; docs, AGENTS row, screenshots.
  **Done (`1e0548e`):** 486 host tests; `home_persist` (mkdir, write, rename) and
  `home_verify` (directory, absent old name, exact 34-byte content),
  `scripts/home_persistence_qmp.py` (two QEMU processes, same image, fsck),
  `--cores 4`/`--cores 1 --reset` and a second cycle without reset (idempotent,
  14/565), fsck exit 0, `debugfs` shows `/persist/b.txt` with the expected text;
  the four no-home arms report the read-only verdicts; visual golden and
  `boot_desktop_qmp.py` pass on both. **M6 is complete.**

## M7 — file API refinements

- [x] **M7.1 `core.fs_truncate` and the shrink/zero-extend writer.**
  `ext2w::truncate(path, size)`: shrinking frees every block at or past `size`
  (direct/single/double indirect, including empty pointer blocks), clears the
  inode pointers, zeroes the tail of the last partial block, then releases the
  bitmaps and counters; extending allocates and zero-fills after planning
  capacity (full disk `CapacityExceeded`, no change); a directory is `Invalid`,
  a missing path `NotFound`, the current size an `Ok` no-op. Builtin 57
  (`core.fs_truncate(path, size) -> Status`, only `home/*`) with compiler,
  interpreter, `Host`, router and `langhost` wiring. Add `samples/tests/apps/base/home_truncate.ig`
  (write 3000 bytes, shrink to 1000 and verify size/content, extend to 4000 and
  verify zeros, shrink to 0, idempotent, error paths) and `ext2w` host tests with
  `e2fsck -fn`. The editor keeps its temporary-file + rename save (ADR 0041).
  Spec 0019 A7, ADR 0041.
  **Done (`b6c350c`):** 494 host tests; `ext2w::truncate` shrink (frees the
  indirect tree and data blocks, zeroes the partial tail, updates `i_blocks`
  before freeing) and extend (capacity planned, zero-fills); builtin 57 with
  compiler/interpreter/`Host`/router/`langhost`; `home_truncate` prints its 16
  step lines + `home_truncate: ok` on `-smp 4`/`-smp 1`, `home_truncate: no
  writable home` without the disk; `e2fsck -fn` exits 0 after the proofs
  (15 files) and after reset (12/563); the persistence harness, the 38-command
  batch (with/without the drive), the visual golden and `boot_desktop_qmp.py`
  pass on both. The editor keeps its temporary-file save.

## M8 — window management

- [x] **M8.1 Pointer-driven window drag and close-box termination.**
  Spec 0011 W22–W24, ADR 0042. Two service-owned interactions over the existing
  window table, with no new opcode, builtin, `Host` method or `stdlib` helper:
  - **Drag:** a left-button press inside a window's title bar (16-px band, not the
    close box) raises/focuses it (any task, like `raise`), records a grab offset
    from the frame origin and moves the window with the pointer while button 1
    stays held, clamped on screen; release ends the drag. The move is the same
    damage/`Expose` path as `window_move`, so the owner repaints and the desktop
    is restored underneath. The press is consumed by the service (not delivered
    as a client `PointerDown`); a second press during a drag is ignored and a
    closed window cancels the drag.
  - **Close box:** a left-button press inside the 11×11 close box at
    `(frame.x + 4, frame.y + 3)` requests termination of the owner task
    (`task::request_kill`); `Task::reclaim` closes the task's windows through the
    existing `close_task` path, so `core.wait` observes `Killed`. Owner `0` kernel
    windows are inert. The press is consumed. No graceful save-on-close: data is
    lost exactly as with `core.kill`; a close-request event is deferred.
  The hook runs after the graphics lock is released, from both pointer paths
  (`gfx::update_pointer`, `gfx::set_pointer_absolute`), so injected events are
  deterministic and the lock order stays `window → graphics`; the table lock is
  never held across `request_kill`. Boot self-check `window: drag ok` covers the
  pure rules (title-bar vs close-box vs content, grab offset, clamp, release,
  owner-0 exemption, drag cancellation).
  Proof: `samples/tests/apps/base/windows_drag.ig` (+ `windows_drag_victim`) injects real
  pointer events, drags its own window by the title bar, checks the frame
  rectangle after each step, both screen clamps, release, the restored desktop
  and the repainted content after the `Expose`, verifies a title-bar press is
  consumed while a content press still reaches the client, then finds the
  victim's window through `window_info`, clicks its close box and requires
  `core.wait` to report `Killed` and the window to disappear. Prints
  `windows_drag: ok` on `-smp 4` and `-smp 1`; `langhost` runs the disabled-screen
  path (`windows_drag: host ok`). Optional `windows_drag shot` plus a screenshot
  script. Interactive resizing stays deferred.
  **Done (`9dcd8b3`, follow-up `d4a8f93`):** **495 host tests**; `window::drag_self_check` prints
  `window: drag ok` at boot beside `window: rules/paint/route ok`; the `gfx`
  hook runs after the graphics unlock under one `POINTER_ORDER` lock,
  `Table::relocate` is shared with `window_move` (public move stays owner-only),
  and a handled press is suppressed before `event::push_pointer` publishes it (the
  `PointerDown` is never queued, so no SMP poll race). A second press mid-drag is
  consumed (`d4a8f93`). `run windows_drag check` prints
  `windows_drag: ok` on `-smp 4` and `-smp 1` (title press consumed, focus,
  no move on press, drag steps, both clamps, release parks, new content mark,
  old area restored to the underlay, content press delivered, victim killed
  with `Killed` and its window/frame gone); `windows_drag: host ok` on
  `langhost`. Regressions pass on both core counts: `windows`, `windows_events`,
  `windows_demo`, `windows_proof` (own boot), `desktop_shell check`, `terminal`
  (shell command), `calculator`, `minesweeper`, `editor check`,
  `browser check`, `events`, `mouse` (own boot), `ui_layout`, `ui_controls`,
  `text_edit`, `files`, `module_demo`, `kill_stress`; visual golden
  `37802052…e9e81c9f` and `boot_desktop_qmp.py` pass on both; `e2fsck -fn` is
  clean (no home writes). Resize remains deferred.

- [x] **M8.2 The close box requests a close; it never kills the owner.**
  Regression: closing the desktop's `About igis-v2` window killed task 3 (the
  boot `desktop_shell`, realtime), so the entrypoint exited, the system printed
  the idle summary and halted (`About` is a desktop-owned window, and M8.1's
  W23 killed the owner of any task window). A dialog must close itself, not its
  application.
  - **Semantics:** a left-button press in the close box sets a per-window
    `close_pending` flag instead of calling `task::request_kill`; the press is
    still consumed (W24). The owner receives the new predeclared
    `EventKind.Close` (ordinal 6, `window = id`, other fields 0) through
    `poll_event`/`next_event`, owner-only and topmost-first, ahead of `Expose`;
    the window stays until the owner closes it. Owner-`0` close boxes stay
    inert. No timeout and no automatic kill for an ignored request.
  - **Surface:** `EventKind.Close` in `abi`, the compiler's predeclared enum and
    spec 0010/0011; no new opcode, builtin, `Host` method, `syslib` primitive,
    `stdlib` helper or opcode-count change.
  - **Delivery:** the window flag is served by the kernel host
    (`window::take_close`/`has_close`) and counts as input readiness
    (`input_ready`, the scheduler's post-block recheck and `next_any` source 1);
    the pointer hook wakes `WaitFor::Input` after dropping the window lock.
  - **Applications:** user-facing windows handle `Close` by closing their own
    window and exiting (`calculator`, `minesweeper`, `editor`, `browser`,
    `clock_window`, `shell`, `widgets`); `desktop_shell` closes only the `About`
    panel on its Close and keeps running, and exits on its own `Desktop` window's
    Close. Proof/demo windows may ignore it and exit on their own.
  - **Proof:** `windows_drag` becomes a close-request proof (an owned second
    window gets `Close`, the app closes only it and stays alive; the child victim
    handles `Close`, closes and exits `Ok`), and `desktop_shell check` clicks the
    About close box, requires the `Close` event (no client `PointerDown`), closes
    About and verifies the desktop still opens About again — the exact freeze.
    A QMP harness should reproduce the boot-desktop scenario (open System/About,
    click its close box, then use Terminal) and prove the system does not halt.
  - **Docs:** spec 0011 W23/W25 and the M8.1 section, spec 0010 event kinds,
    ADR 0043 (close requests; amends 0042's close clause; ADR 0042 stays
    accepted and is annotated, per the immutability rule), PLAN/AGENTS.
  Not part of this ticket: save-on-close, close timeouts, resize.
  **Done (`195f8d4`):** **497 host tests**; `EventKind.Close` (ordinal 6) is
  predeclared in `abi` and the compiler, and `Event`'s layout is unchanged. A
  close-box press sets `close_pending` and returns `PressAction::Close`; the
  service consults `take_close`/`has_close` (owner-only, topmost-first) before
  `Expose` and before the queue, writes `{ Close, 0, 0, 0, id }`, counts it in
  `input_ready`, the scheduler's post-block re-check and `next_any` source 1,
  and wakes `WaitFor::Input` after the window unlock. Owner-0 close boxes stay
  inert and an ignored request leaves the window. `window::close_self_check`
  prints `window: close ok` beside `window: drag ok`. `desktop_shell` closes
  only About on its `Close` and keeps running (the freeze), and exits on its
  own Desktop window; `calculator`, `minesweeper`, `editor`, `browser`,
  `clock_window`, `shell` and `widgets` close their own window and exit;
  proof/demo windows may ignore the request. `run windows_drag check` proves a
  same-task dialog plus a victim that closes itself and exits `Ok` (not
  `Killed`), and `run desktop_shell check` injects a real About close-box press,
  gets exactly the `Close` event, closes About and reopens it. The 13-command
  batch (`windows_drag`, `desktop_shell`, the six user apps, `terminal`,
  `widgets`, `windows`, `windows_events`, `windows_demo`, `events`) passes on
  `-smp 4` and `-smp 1`; `windows_proof` passes in its own boot on both;
  `scripts/about_close_qmp.py` boots `build/igis.iso`, opens System → About,
  clicks its close box, sees `desktop_shell: about closed`, then launches the
  terminal (`terminal: ok`) with no idle summary/halt; `boot_desktop_qmp.py`,
  `system_menu_qmp.py` (its About row moved to y = 24 because a title-bar press
  at y = 30 is now the M8.1 service drag) and the visual golden
  `37802052…e9e81c9f` pass on both; a calculator and a shell terminal closed
  interactively through their close boxes exit `Ok`; `e2fsck -fn` is clean
  (no home writes). Resize remains deferred.

- [x] **M8.3 Apps application menu and the modal popup input grab.**
  The desktop's `Apps` item launches `clock` directly. Turn it into a real
  application-selection pull-down: `Calculator`, `Minesweeper`, `Text Editor`,
  `File Browser`, `Clock` (`clock_window`), `Widgets`, each launched with a
  non-blocking `core.spawn` (never `core.run`); the desktop's own window help text
  and the check-mode assertions follow.
  - **Why a grab:** app-drawn menus have no window. A click in the pull-down
    over another window is routed to that window (and with M8.1 a title-bar row
    is consumed as a drag), so the lower items—and the lower System rows—never
    reach the desktop. Fix it with a machine-wide **modal popup input grab**:
  - **Surface:** `syslib.popup_grab(x, y, w, h) -> Status` and
    `syslib.popup_release() -> Status` as host builtins 58/59 (no opcode, ABI v1
    stays at 75). One grab machine-wide, owned by the calling task; `Full` for
    another owner, a repeat by the owner updates the rectangle; `Invalid` for a
    non-positive rect; `popup_release` is owner-only and `NotFound` otherwise.
    `Host` defaults `Invalid` so `langhost` and a service-less host stay safe.
  - **Semantics:** while a grab is active, every queued pointer/key event is
    delivered only to the owner (producers keep `window = 0`; no hit-test, no
    raise) and non-owners' `poll_event`/`next_event`/`next_any` see no input
    readiness for the queue (their own `Close`/`Expose` still works). The window
    service ignores pointer presses (no drag, no close box) while grabbed. The
    owner decides inside/outside from its rectangle; dismissing on an outside
    click is its policy. `popup_release` and `Task::reclaim` clear the grab and
    wake all input waiters, so queued events are deliverable again. Lock order
    stays acyclic (grab state lives in the window table; the queue never takes
    the table).
  - **Desktop:** both the Apps and System pull-downs take the grab on open (the
    rect is the pull-down frame) and release it on close; the menu handlers run
    for any pointer event while grabbed. `F9` toggles Apps, `F10` System, the
    open menu takes Up/Down/Enter/Escape and Left/Right switches between the two
    pull-downs; opening one closes the other. Normal mode logs
    `desktop_shell: apps menu open/closed` and `desktop_shell: launched <label>`.
    Check mode verifies both menus' open/close/grab, the Apps items and one real
    non-blocking launch (spawn then reap so no child is left), and that an
    outside click closes the menu.
  - **Proof:** `scripts/apps_menu_qmp.py` boots `build/igis.iso`, opens Apps with
    a QMP click, captures `/home/sinan/Desktop/igis-apps-menu.png`, selects
    `Clock` (row 4, over the kernel Terminal window—the regression), waits for
    `clock_window: ready`/`done`, then opens System and closes it with an outside
    click; no idle summary or halt. `desktop_shell check` and
    `windows_drag`/`mouse`/`tablet` regressions stay green (the grab must not
    steal input when no menu is open).
  - **Docs:** ADR 0044 (modal popup grab; rejected alternatives: app-drawn menu
    over windows, borderless window, service-owned menu), spec 0011 (grab
    requirement, routing, lock table, failure modes), spec 0010 (input routing
    while grabbed), spec 0003 (`syslib.popup_grab`/`popup_release`), spec 0008
    (builtins 58/59), spec 0012 (Apps menu and D10/D11 update), PLAN/AGENTS.
  Not part of this ticket: submenus, dynamic filesystem app discovery, task
  switching/window list menus, save-state.
  **Done (`94187ed`):** **501 host tests** (abi 25, compiler 200, interpreter 165,
  langhost 89, ext2w 22, including doctests; +4: one compiler signature/encoding test
  and three interpreter cases for the grab); ABI v1 stays at 75 (builtins 58/59, no
  opcode).
  `window::grab_self_check` prints `window: grab ok` beside `rules/paint/route/drag/
  close` on both core counts. `desktop_shell check` covers the six-item Apps table,
  both menus' grab/open/close (`grab_status` is `Ok`, a release afterwards is
  `NotFound`), Down/Up, one real non-blocking launch (Clock's deterministic check,
  reaped with `core.wait`; a kill right after `spawn` races the compile window, so the
  natural exit is used), the outside-click dismissal, F9/F10 toggling, Left/Right
  switching and the accent/frame/label-ink pixels; `desktop_shell: ok` on `-smp 4`
  and `-smp 1`. `scripts/apps_menu_qmp.py build/igis.iso --cores 4` captures
  `/home/sinan/Desktop/igis-apps-menu.png` (1024×768), clicks `Clock` at
  `(140, 94)` over the kernel Terminal, and the transcript shows
  `desktop_shell: apps menu open` → `desktop_shell: launched Clock task 4` →
  `clock_window: ok`, then `desktop_shell: launched shell task 5` → `terminal: ok`,
  with no idle summary or halt; `--cores 1 --no-capture` shows the same contiguous
  lines. (Empty-args `clock_window` is its deterministic check and prints
  `clock_window: ok`; `ready`/`done` belong to its `shot` mode, so the harness gates on
  the app's own exit.) `windows_drag`, `events`, `windows`, `windows_events`, `mouse`
  (own boot, `-smp 4`), `scripts/tablet_qmp.py` 7/7, `windows_proof` (own boot,
  `-smp 1`), `scripts/about_close_qmp.py`, `scripts/system_menu_qmp.py`,
  `scripts/boot_desktop_qmp.py`, the visual golden `37802052…e9e81c9f` and the desktop
  Clock/About screenshots all pass; `e2fsck -fn build/home.img` is clean (no home
  writes). `cargo xtask build`/`image`/`image --shell` are clean. No `stdlib` helper
  (125) and no capability change: the grab is a UI focus mechanism (ADR 0044).

- [x] **M8.4 Persistent menu-bar pointer-region claim.**
  Regression found while using M8.3: after launching `File Browser` from the
  Apps menu, a later click on the `Terminal` bar item no longer reaches the
  desktop. The bar click is a `window = 0` event in the one machine-wide queue;
  the browser is also blocked in `next_event`, the two tasks split the
  move/down/up events, and the browser takes the press. The popup grab only
  starts after a menu opens, so it cannot protect the click that opens it. The
  desktop must own the menu-bar strip at the service level.
  - **Surface:** `syslib.region_claim(x, y, w, h) -> Status` and
    `syslib.region_release() -> Status` as host builtins 60/61 (no opcode; ABI
    v1 stays 75): one machine-wide **pointer-region claim**, owner = caller;
    `Ok`/`Full`/`Invalid`, a repeat by the owner updates the rect, owner-only
    release, `Invalid` for another task. `Host` defaults `Invalid`.
  - **Semantics:** while held, a queued `PointerDown` with `window = 0` inside
    the rect is delivered **only** to the owner; a non-owner poll returns
    `WouldBlock` without consuming it (the modal grab, when active, still takes
    precedence). The owner receives it unchanged (no hit-test/raise), so the
    desktop's coordinate handler runs. Pointer moves/ups, keys and all events
    outside the rect route normally. The window service ignores a press inside a
    claimed region (no drag, no close box), so the bar wins over a window moved
    under it. Termination releases the claim and wakes input waiters.
  - **Desktop:** claims `(0, 0, screen_w, MenuHeight)` at startup (both normal
    and check mode) and releases on exit; the popup grab continues to cover the
    open menu. The bar click now reaches the desktop even when a client is
    blocked in `next_event`, and existing F9/F10 focus-scoped keys are unchanged.
  - **Proof:** `scripts/apps_menu_qmp.py` is extended (or a sibling harness
    added) to launch `File Browser` from Apps, wait for its spawn, then click
    `Terminal` and require `terminal: ok` — this fails on M8.3 and must pass
    here. Also prove a click outside the bar still routes to a client (the
    existing window/event regressions) and a second menu open/select works while
    the browser is live. Boot adds `window: region ok` (`region_self_check`:
    claim/Full/update/owner-only release/rect hit/termination release).
  - **Docs:** ADR 0045 (persistent region claim vs the modal grab; rejected:
    permanent modal grab, bar as a window, focus tricks, accepting the race),
    spec 0011 W27, spec 0010 routing/readiness, spec 0003 surface, spec 0008
    60/61, spec 0012 D20, PLAN/AGENTS. No `stdlib` helper; no authority change.
  **Done (`1304df8`):** **505 host tests** (abi 25, compiler 201, interpreter 168,
  langhost 89, ext2w 22; +4: one compiler signature/encoding test and three interpreter
  cases); ABI v1 stays at 75 (builtins 60/61, no opcode); `stdlib` stays at 125
  functions. `window::region_self_check` prints `window: region ok` beside
  `rules/paint/damage/route/drag/close/grab` on `-smp 4` and `-smp 1`. One
  `Region { owner, rect }` lives in the window table beside the grab; `event::peek`
  reads the head without consuming, `KernelHost::poll_event` gives a non-owner
  `WouldBlock` for a `window = 0` `PointerDown` inside the claim and delivers it to the
  owner unchanged, the modal grab keeps precedence, and `window::pointer_event` starts
  no drag/posts no close for a new press inside the rectangle while moves/releases still
  follow an in-progress drag. `task::input_queue_ready` folds the grab and region rules
  into `input_ready`, the scheduler's post-block `Input` re-check and `next_any` source 1.
  The desktop claims `(0, 0, screen_w, MenuHeight)` at startup in normal and check mode,
  releases on exit, accepts `Full` when the boot desktop owns the bar and releases only
  its own claim. `scripts/apps_menu_qmp.py` now launches `File Browser`, opens/dismisses
  a second Apps menu while it is live, then clicks `Terminal` and reaches `terminal: ok`;
  the same script fails on M8.3 on `-smp 4` with no `desktop_shell: launched shell task`
  (the browser consumes the bar press). `desktop_shell check` (shell image both core
  counts and the boot desktop's `Full` branch), `windows_drag`, `events`, `windows`,
  `windows_events`, `mouse` (own boot), `windows_proof` (own boot), `tablet_qmp.py` 7/7,
  `about_close_qmp.py`, `system_menu_qmp.py`, `boot_desktop_qmp.py`, the visual golden
  `37802052…e9e81c9f` and `e2fsck -fn build/home.img` pass. `apps_menu_qmp.py --cores 4`
  captures `/home/sinan/Desktop/igis-apps-menu.png` (1024×768).

- [x] **M8.5 Move the content rectangle, then redraw the window at the new position.**
  Regression: dragging a window moves its frame and chrome, but the client
  content does not follow or reappear. Reproduced with Calculator: after a
  title-bar drag the new frame shows stale underlay pixels, not the calculator
  panel; the old content rect is stale in the app, so its Expose repaint draws
  at the old coordinates (and `window_present` clips it to the new content) and
  its hit-testing still uses the old origin. Two coordinated fixes:
  - **Service, "move the rectangle first".** Add an internal
    `gfx::copy_rect` (shadow-to-shadow, row-wise, overlap-safe, clipped). In
    `Table::relocate`, when the moved window is **topmost** (a drag always
    raises it first), copy its visible content rectangle from the old to the
    new origin before the repair, and omit the new frame's erase so the copy
    survives; the old frame erase still subtracts every live window (including
    the moved window's new frame), the chrome pass paints the new border/title,
    and the normal damage/`Expose` path runs. Non-topmost moves keep the current
    erase and rely on the client. The copy is a presentation snapshot, not a
    canvas: the service still owns no content.
  - **Apps, "redraw at the new position".** Every windowed sample must re-read
    `syslib.window_content_rect(id, rect)` on an owner `Expose` before drawing
    or hit-testing, and recompute any cached dependent geometry (Calculator's
    `disp_x`/`disp_y`; similar locals elsewhere). At minimum Calculator,
    Minesweeper, Editor, Browser, Clock Window, Widgets, the desktop shell's own
    window and any other sample that caches the content rect; audit all
    `window_open` callers. A client that ignores the expose keeps the copied
    snapshot (W18 policy).
  - **Proof:** extend `windows_drag` — after the drag and **before** draining
    exposes, the mark must already be at the new content origin (the copy), then
    drain/present and check a repaint-counter marker at the new origin (the
    client's redraw with a fresh rect; a stale client would draw at the old
    coordinates and fail). New `scripts/drag_content_qmp.py` launches
    Calculator from Apps, drags it by the title bar, verifies the panel/display
    at the new origin, clicks the `7` button at the new coordinates and requires
    the display pixels to change (proves both the refreshed paint and the
    refreshed hit-test), then captures `/home/sinan/Desktop/igis-drag.png`; no
    halt. Run both core counts.
  - **Docs:** spec 0011 W12/W17 clarification and W28 (topmost move copy plus
    the client's re-read obligation), ADR 0046 (why a snapshot copy is not a
    canvas, why the client must re-read the rect, rejected alternatives:
    per-window canvas, blocking the drag for a repaint, absolute drawing
    handles), spec 0012 app guidance, PLAN/AGENTS. No new opcode/builtin.
  **Done (`f478f9b`):** **505 host tests** (abi 25, compiler 201, interpreter 168,
  langhost 89, ext2w 22; no new host harness — the kernel crates have none).
  Kernel-internal `gfx::copy_rect` (clipped, per-row `copy_within`, direction from the
  vertical move, cursor untouched); `Table::relocate` copies the content snapshot for a
  **topmost** move before the repair, records the old-frame erase (`covers_from = 0`,
  protecting the new frame by subtraction) and no new-frame erase, and keeps both erases
  for a non-topmost move; `window_move` and the drag pass `true`, the pure self-checks
  pass `false`. `paint_self_check` now asserts the topmost no-new-frame-erase shape
  (`window: paint ok`). Calculator (`disp_x`/`disp_y`), Minesweeper, Editor
  (`compute_layout`), Browser, Clock Window (face/readout) and Widgets (control origins)
  re-read `window_content_rect` on an owner `Expose`; the desktop shell already re-read
  its own and About rectangles. `windows_drag` proves the copied mark and marker before
  its pump and a changed repaint marker at the new origin after it (`windows_drag: ok`
  on `-smp 4` and `-smp 1`); `scripts/drag_content_qmp.py` launches Calculator from Apps,
  closes the desktop's own window with its close box (the accepted queue competition,
  spec 0012 D19), drags (330,108) → (580,308), requires the new display edge 284/284 and
  panel border 360/360 black with the old edge 0/284 and border 0/360, clicks `7` and
  requires the display region to change (22 px, ink 36 → 20), writes
  `/home/sinan/Desktop/igis-drag.png` (1024×768, ~24 KB) on `--cores 4` and passes on
  `--cores 1 --no-capture`. Regressions pass on both core counts: `desktop_shell check`,
  `windows`, `windows_events`, `windows_demo`, `windows_proof` (own boot), `editor`,
  `browser`, `minesweeper`, `calculator`, `clock_window`, `widgets`, the shell `terminal`;
  `apps_menu_qmp.py`, `about_close_qmp.py`, `boot_desktop_qmp.py`,
  `system_menu_qmp.py` and the visual golden `37802052…e9e81c9f` are unchanged; no
  opcode/builtin/`stdlib` change (ABI v1 75, 125 functions).
  **Follow-up (`4a0a3d4`):** the user's File Browser drag screenshot exposed that its
  `refresh_rect` updated `rect` but not the cached layout (`list_x`, `preview_x`,
  `status_y`), so the list/preview/status stayed at the old absolute origins; it now
  calls `compute_layout`. `scripts/browser_drag_qmp.py` launches the Browser, drags the
  title bar (460,98) → (610,233) to frame origin (310,225), and requires the new path
  separator 680/680, the list/preview borders 420/420 and the old layout gone
  (stale preview 2, old path run < 340), writing
  `/home/sinan/Desktop/igis-browser-drag.png`; it passes on `--cores 4` and `--cores 1`,
  and `browser check` still passes.

- [x] **M8.6 Window-addressed input delivery (the boot desktop must not steal app input).**
  Two user reports:
  - The File Browser does not react to clicks or keys while the boot desktop runs;
    the serial shows `task 3 blocked on input` / `task 4 blocked on input` in pairs.
    Root cause: the one machine-wide queue is consumed by whichever task polls
    first. The boot desktop (`desktop_shell`, realtime, task 3) wakes with every
    event, consumes input addressed to the Browser's window, and ignores it. The
    M8.3/M8.4 grabs only protect an open menu and the bar; ordinary app
    content/key input remains racy. This is spec 0010 I31's accepted competition,
    and it makes every app launched from the desktop unreliable.
  - Closing about can end the desktop with `task 3: exit Ok` (the own-window
    Close path). The current `about_close_qmp.py` passes on HEAD, so the report
    is likely the same mis-delivery/stale state (a click meant for About reaching
    another window) or an older ISO; re-prove it with the Browser live and fix
    any remaining path.
  **Fix:** deliver queued input to the task that owns its target, not to the
  first poller.
  - `window` gains `hit_test_owner(x,y)`, `owner_of(id)`, `focus_owner()` and one
    target rule: a `PointerDown` targets the region-claim owner at the point if
    the bar claims it, else the topmost hit window's owner; `PointerMove` /
    `PointerUp` / `KeyDown` / `KeyUp` target the focused window's owner; an
    event with a non-zero `window` targets that window's owner. Owner `0`
    (kernel windows) or no target (`None`) leaves the event to the ordinary
    window-less competition, as before.
  - `KernelHost::poll_event` peeks the head (the existing `event::peek`), and a
    non-target task returns `WouldBlock` **without consuming**; the target
    consumes and takes the ordinary routing path (a pointer-down still raises,
    focuses and stamps the window; moves/keys name the focus). The modal grab
    still takes precedence; the region claim is the first pointer-down target.
    `task::input_queue_ready` uses the same rule for `input_ready`, the
    post-block re-check and `next_any` source 1, so non-owners do not spin.
  - The queue is still one FIFO: an event targeted at an unresponsive owner can
    head-of-line block later events; this is the W18 liveness tradeoff and is
    documented. Close/Expose are still owner-specific flags served before the
    queue.
  - **Proof:** a new QMP `input_routing_qmp.py` (or an extension) on the boot
    desktop: launch Browser, click a list row and double-click a directory (the
    path/count pixels must change) **with the desktop alive**; launch Browser,
    open About, click its close box, require `desktop_shell: about closed` and
    the desktop still responsive (no `task 3: exit Ok`); launch Calculator and
    click a button **without closing the desktop** (the M8.5 `drag_content_qmp`
    workaround can then be removed). Boot self-check `window: target ok` on a
    scratch table (hit/down/focus/addressed/owner-0/no-target).
  - **Docs:** ADR 0047 (window-addressed input), spec 0010 I31 amended + new
    I34 (I32/I33 already name the IRQ12 bring-up), spec 0011 W29, spec 0012 D19
    note, PLAN/AGENTS. No new opcode/builtin.
  **Done (`3598fda`):** `window::input_target` is the one target rule (addressed
  window owner; region claimant then topmost hit owner for a `PointerDown`; focus
  owner for moves/keys; owner-0/empty/unknown → none). `KernelHost::poll_event`
  peeks and a non-owner gets `WouldBlock` without consuming; the modal grab still
  takes precedence and skips the target gate, and a region press is delivered
  unchanged. `task::input_queue_ready` uses the same rule for readiness, the
  post-block re-check and `next_any` source 1. `window: target ok` prints on
  `-smp 4`/`-smp 1`; `scripts/input_routing_qmp.py` proves Browser click (10,844
  px) and Enter navigation (path 274 px) with the desktop alive, About's close
  box plus an Escape close with the Browser live and no `task 3: exit Ok`, and a
  Calculator `7` click (display 22 px) without closing the desktop.
  `drag_content_qmp.py` drops the close-the-desktop workaround. The About symptom
  did not reproduce on `8b23ad8`; the routing regression is the guard. `events`
  and `multiplex` now own a small window because window-less keys/moves belong to
  the focus. **505 host tests** (no new host harness; kernel-internal rule);
  `cargo xtask build`/`image`/`image --shell` clean; the full regression batch,
  `apps_menu_qmp.py`, `about_close_qmp.py`, `drag_content_qmp.py`,
  `browser_drag_qmp.py`, `system_menu_qmp.py`, `boot_desktop_qmp.py`,
  `tablet_qmp.py` 7/7 and the visual golden `37802052…e9e81c9f` pass on both
  core counts. No opcode, builtin, `Host`, `stdlib` or `Event` layout change
  (ABI v1 stays 75; `stdlib` 125 functions).

- [x] **M8.7 Closing the `Desktop` window must not end the desktop.**
  User report: closing the window titled `Desktop` (the shell's own help panel)
  exited the boot `desktop_shell` with `task 3: exit Ok`; the entrypoint was gone
  and the idle system halted, which the user saw as a lock-up. The `Desktop`
  window is not the shell's lifetime: the shell is the boot desktop and must keep
  its menu bar and launched applications running.
  - **Fix (application-only, no kernel change):** a `Close` for `own` calls
    `close_desktop_window` (close the window, `own := 0`, log
    `desktop_shell: desktop window closed`) and does **not** clear `running`.
    The Apple square calls `open_desktop_window`: raise/draw the live window, or
    reopen it at `(600, 72)` with `window_open` when `own = 0`, logging
    `desktop_shell: desktop window open`. Normal mode starts through the same
    helper; Restart/Halt and task termination remain the only exits.
  - **Proof:** extend `scripts/input_routing_qmp.py` — with the Browser and
    Calculator live, click the Desktop window's close box, require
    `desktop_shell: desktop window closed`, assert no `task 3: exit Ok` and no
    idle summary/halt, then click the Apple and require
    `desktop_shell: desktop window open`; the Browser/Calculator checks still
    pass. Run both core counts.
  - **Docs:** spec 0012 D12/D19, PLAN/AGENTS (no ADR; it is the desktop's own
    lifetime policy, not a kernel decision).
  **Done (`5c97e39`):** `open_desktop_window` raises/draws or reopens at
  `(600, 72)` and logs `desktop_shell: desktop window open`; `close_desktop_window`
  closes only the help panel, sets `own := 0` and logs
  `desktop_shell: desktop window closed`; the normal loop no longer clears
  `running` on an own `Close`, and the Apple item reopens. Spec 0012 D12/D19
  updated. `scripts/input_routing_qmp.py` now closes the Desktop window with the
  Browser and Calculator live, requires the closed log and the Apple reopen, and
  asserts no `task 3: exit Ok`/idle summary/halt; it passes on `--cores 4` and
  `--cores 1`. `desktop_shell check` still prints `desktop_shell: ok` on
  `-smp 4`; no kernel/ABI change.

- [x] **M8.8 A visible Up button in the file browser.**
  User report: there is no way to go to the parent directory from the File
  Browser. `Backspace`/`Ctrl+Up` already truncate `dir`, but the pointer-only
  path had no visible control.
  - **Fix (application-only):** `samples/apps/base/browser.ig` computes a 44×18
    `stdlib.ui_button` at the right of the path line (left of the
    `home writable` label), draws it in `draw_path`, and a `PointerDown` on
    `ui_button_hit` calls `go_up()` exactly like the keys. The status hint names
    `Bksp up`. `compute_layout` owns the `up_x/up_y/up_w/up_h` geometry so a
    moved window and a refreshed `Expose` keep it aligned; no ABI/kernel change.
  - **Proof:** `browser check` draws the button, clicks it through the real
    pointer path and requires the root listing, then re-enters `base` for the
    existing Backspace/Ctrl+Up checks. `scripts/input_routing_qmp.py` clicks the
    visible button after Enter navigation and requires the path line to match
    the pre-navigation root exactly (0 differing pixels) on `-smp 4`/`-smp 1`
    with the desktop alive.
  - **Docs:** spec 0013 browser bullet/changelog, PLAN/AGENTS.
  **Done (`c69e392`):** `up_x = rect.x + ContentW - 8 - text_width("home
  writable") - 8 - 44`, `up_y = rect.y + 2`; `draw_path` draws `Up`, the pointer
  handler checks it before the list/scrollbar, and the hint string names
  `Bksp up`. `browser check` prints `browser: ok`; `input_routing_qmp.py`
  reports `Up button returned to the parent path (0 pixels differ)` on both
  core counts, with the existing Browser/About/Calculator/Desktop checks still
  passing. No kernel, ABI or `stdlib` change.

- [x] **M8.9 Drag the file browser's scrollbar thumb.**
  User request: can the scrollbar be grabbed and pulled down? The track already
  paged on click, but the thumb was not draggable.
  - **Fix (application-only):** `samples/apps/base/browser.ig` adds
    `scroll_dragging`/`scroll_grab_dy` and a `scrollbar_thumb` helper matching
    `stdlib.ui_scrollbar` (track `BodyH-4`, 8-pixel floor, travel). A
    pointer-down in hit class 3 records the offset; `PointerMove` with the
    primary button held maps the desired thumb top onto `top` over the travel
    and keeps the selection visible; `PointerUp` (or a move without the button)
    ends the drag. `load_dir` cancels a stale drag. Track clicks still page.
  - **Proof:** `browser check` grabs the thumb after `base` and drags it to the
    bottom, requiring `top = entry_count - ListRows` and `not scroll_dragging`,
    then resets the view. `scripts/input_routing_qmp.py` enters `base`, drags
    the thumb from the top toward the bottom with a real QMP pointer drag and
    requires the scrollbar and list regions to change (3,604 and 1,370 pixels
    on both core counts) while the desktop stays alive; a screenshot is
    written to `/home/sinan/Desktop/igis-input-routing.png`.
  - **Docs:** spec 0013 browser bullet/changelog, PLAN/AGENTS. No kernel/ABI or
    `stdlib` change.
  **Done (`d3087c6`):** the browser drag code and the check/QMP proofs above;
  `browser: ok` and `input_routing_qmp.py` pass on `-smp 4` and `-smp 1`.

## M9 — application experience

### M9.1 — syntax highlighting in the text editor

> Historical slice: M9.2 replaced the per-frame block-state scan with the cached
> `line_states` table, and M9.4 raised the document buffer from 8192 to 131072
> bytes. The 8192/per-frame details below are the M9.1 record, not current behaviour.

- [x] **M9.1 Syntax highlighting in the text editor.**
  User request: "In the text editor, can we do syntax highlighting for .ig
  files?" The answer is a first lexical slice inside the editor only; there is
  no parser, no new opcode, builtin, `Host` method, `stdlib` function or ABI
  change, and no new application file.
  - **Detection and menu:** a successful `open_path` enables highlighting when
    the path starts with `base/`, `lib/` or `system/`, or ends in `.ig`; other
    text files (for example `home/readme.txt`) stay plain. The File menu is now
    five rows (`Open`, `Save`, `-`, `Highlight`, `Quit`), its checked mask is
    `file_checked`, and row 3 toggles the view with `syntax on`/`syntax off` in
    the status line. Quit moved from row 3 to row 4; `Ctrl+Q` is unchanged.
  - **Lexer (`samples/apps/base/editor.ig`):** one visible line is scanned with
    `//` to the line end, `{ ... }` block comments to the first `}` (no
    nesting, strings and comments skip their interior), strings to the closing
    quote with `\"` skipped or to the line end, identifiers classified against
    a startup-built keyword table (33 words) and type table (14 predeclared
    names), and decimal/hex/float numbers. Default punctuation and whitespace
    are grouped so a plain line is one draw call. The palette is keyword
    `0x000080`, type `0x800000`, string `0x008000`, comment `0x808080`, number
    `0x804000`; selected text stays white on the accent band.
  - **Block state and UTF-8:** when the first visible line is not line 0, the
    block-comment state is computed by one allocation-free scan from byte 0,
    **line by line** (a `//` must not swallow a later line's `{`), then carried
    through the visible lines by `draw_syntax_line`. The scan is O(document),
    bounded by the 8192-byte buffer. Token runs use whole code points and still
    draw through `core.substr_into` and `snap_boundary`, so a multi-byte code
    point is never split. With highlighting off the old all-black/all-white
    `draw_line` path runs unchanged.
  - **Proof:** `editor check` opens a plain `home/readme.txt` and a source
    `base/child`, checks the menu item string and check mask, then draws
    deterministic lines and scans `syslib.get_pixel` for keyword, comment,
    type, number and string pixels; it also proves a block comment crossing a
    line and one whose `{` is above the first visible line (top_line 2 after a
    `//` line), toggles off and requires no palette pixel, and restores the
    document for the existing glyph/selection/caret/scroll probes. A real QMP
    pointer run (`press File`, drag to the Highlight row, release) turns every
    palette count to 0 and back to the exact original counts (916 keyword,
    285 type, 488 string, 4,209 comment, 380 number pixels in `base/child`).
    The editor screenshot shows the highlighted source and the shell returns
    after Ctrl+Q; the visual golden is unchanged.
  - **Docs:** spec 0013 "The text editor" and changelog, PLAN/AGENTS. No ADR:
    this is an app-local lexical colouring rule, not a kernel or ABI decision.
  **Done (`a912765`):** `editor: ok` on `-smp 4` and `-smp 1`,
  `editor: host ok` on `langhost`; 505 host tests; `browser check`,
  `desktop_shell check`, `terminal` and the visual golden
  (`37802052…e9e81c9f`) pass; `/home/sinan/Desktop/igis-editor.png` is
  1024×768, 29,772 bytes with keyword/type/string/comment/number pixels.

### M9.2 — the editor's pointer budget and draggable scrollbar

> Historical slice: M9.4 raised the editor buffer from 8192 to 131072 bytes and
> therefore `LineStateCapacity` from 8193 to 131073; the cache/redraw rules below
> are current, the 8193 figure is the M9.2 record.

- [x] **M9.2 The editor repaints only on real changes and its thumb drags.**
  User report: "task 4: exit Faulted(BudgetExceeded) after 2893 claims (2891
  yields) while trying to scroll in the editor." This was the **normal-task
  10 s CPU watchdog** (spec 0007 Sch8), not a crash. The editor's interactive
  loop called `draw_all` + `present_frame` for every `PointerDown`/`PointerUp`/
  `PointerMove` addressed to it, even when `handle_pointer` changed nothing; M9.1
  had made each frame expensive (the block-comment state rescanned the document
  from byte 0 when `top_line > 0`, plus per-token work), so moving the pointer
  over the editor spent the task's whole lifetime budget on frames nobody could
  see change. The scrollbar thumb also did not drag (unlike the browser's M8.9).
  - **Redraw rule:** `handle_pointer` returns whether visible state changed and
    the interactive and `shot` loops call `draw_all`/`present_frame` only on
    true. A plain move over the text does nothing; a move with the File menu
    open repaints only when the highlighted row changes (a separator or disabled
    row is skipped); a thumb-drag move only when `top_line` changes; a press on
    the thumb starts the drag without a repaint; release ends it without one;
    track clicks repaint only when the page actually moved. Presses that
    open/close the menu, move the caret/selection, page the view or activate an
    item repaint. Key events and owner `Expose` are unchanged. Every event is
    still delivered and handled.
  - **Thumb drag:** `scroll_dragging`/`scroll_grab_dy` plus `scrollbar_thumb`
    matching `stdlib.ui_scrollbar` (track `ScrollH-4`, visible-fraction thumb
    with an 8-pixel floor, travel `track - thumb`, `Rows` against
    `line_count(text)`). A hit class 3 press records `pointer - thumb_y`; held
    primary-button moves map the thumb top onto `top_line` over the travel and
    clamp it; release or a move without the button ends the drag. Track clicks
    still page. Both paths clamp the caret into the visible lines (collapsing
    the selection) so the next key does not jump the view.
  - **Block-state cache:** `block_state_at(start)` per frame is replaced by
    `line_states: bytes[8193]`, one state per line start, filled by
    `refresh_line_states` with the same line-by-line `scan_token` walk. It is
    refreshed by `publish_staged` (open/load) and `ed_replace` (every edit), plus
    the check-mode direct `text :=` assignments that draw. `draw_lines` reads
    the state for `top_line` and carries it through the visible rows: O(visible)
    per frame, output identical (including a block comment above the visible
    top line and a `//` that must not swallow a later `{`).
  - **Budget:** no kernel/scheduler/realtime change. The fix removes the work,
    it does not raise the budget. A QMP stress with ~3,000 no-button moves plus
    a 300-step real thumb drag now ends `Ok`; see the proof below.
  - **Proof:** `editor check` now drags the thumb in `base/child` through the
    injected pointer path (`lib.inject` + a local pump): hit class 3, drag to
    the bottom, require `top_line = line_count - Rows`, the drag ended and the
    thumb redrawn at the bottom; the M9.1 syntax checks still pass.
    `scripts/editor_scroll_qmp.py` boots `build/igis-shell.iso` with the
    absolute tablet, runs interactive `editor`, sends 3,000 no-button moves over
    the text, drags the thumb to the bottom with 300 held-button moves, compares
    before/after captures (text view changed, thumb left the top of the track),
    sends Ctrl+Q and requires `exit Ok` with no `BudgetExceeded`, the shell
    prompt back and no idle summary/halt. Measured:
    - old code (`c98d019`): `task 6: Faulted(BudgetExceeded) after 2826 claims
      (2823 yields)` on `-smp 4` — the user's failure reproduced;
    - new code: `task 6: exit Ok after 2448–2633 claims`, no budget fault, text
      region diff 12,607 px, thumb strip gray 190 → 0 on `-smp 4` and `-smp 1`
      (2,571 claims on `-smp 4`, 2,633 on `-smp 1` in the no-capture runs).
    `editor check` prints `editor: ok` on `-smp 4` and `-smp 1`,
    `editor: host ok` on `langhost`; 505 host tests; `browser check` and
    `desktop_shell check` pass; the visual golden is unchanged
    (`37802052…e9e81c9f`) on both core counts;
    `/home/sinan/Desktop/igis-editor-scroll.png` is 1024×768, 17,206 bytes.
  - **Docs:** spec 0013 "The text editor" and changelog, PLAN/AGENTS. No ADR:
    the budget policy does not change; this is an app-local drawing/scroll rule.
  **Done (`09dc57b`):** the editor redraw suppression, thumb drag and per-line
  block cache plus `editor_scroll_qmp.py`; `editor: ok`, `editor: host ok`, the
  QMP burst on both core counts and the unchanged visual golden as above.

### M9.3 — a `/lib` file-open dialog

- [x] **M9.3 A reusable `/lib` file-open dialog, wired into the editor.**
  User request: "Can we write a file-open dialog? But not in the kernel — as a
  library element. When I choose Open in the editor, an open-file dialog should
  appear, and it should return a file name or something like null."
  - **Module:** `samples/apps/lib/file_dialog.ig` is ordinary module source
    (`unit file_dialog;`, `uses core, stdlib, syslib;`) with no `main`, no
    top-level `var` and no stream endpoint, so the M5 `stdlib`-import rule and
    the M6 module rules are exercised. Its one public function is
    `file_dialog.open_file(initial: const string[0]; chosen: var string[256])
    -> Status`. The dialog state is one local `Dialog` record (id, content
    rect, directory, 96 names/kinds, selection, top, status, drag/click
    bookkeeping and the staged picked path) passed by reference to the
    helpers; the module owns no state.
  - **Window and content:** a normal task-owned `Open File` window (content
    520×380, centred with the service chrome; frame `(251,185)` and content
    `(252,201)` at 1024×768). White client face, path line
    `igis:/`+directory (ellipsis tail when deep), visible `Up` button, 17-row
    list (folder/file/other markers, name, right-aligned kind, accent
    selection), scrollbar, status line, `Open` (default) and `Cancel` buttons.
    No `syslib.clear`; drawing is `syslib` + `stdlib.ui_*` only.
  - **Start and entries:** split `initial` at its last `/`; use that directory
    when `core.fs_list` can list it (`base/child` starts in `base` with `child`
    selected), else the root. Up to 96 entries, read from index 0 until
    `NotFound`; kind 1 directories and kind 0 regular files are selectable,
    kind 2 is shown gray and refused with a status. Directory rows path-join
    with the 256-byte `PathCapacity` bound and probe the listing before
    committing the new directory.
  - **Navigation:** Up/Down, PageUp/PageDown, Home/End, Enter (descend or
    return a file), Backspace/Ctrl+Up (parent; no-op at root) and Escape
    (cancel). A row click selects, a double-click opens, the visible
    `Up`/`Open`/`Cancel` buttons act, the scrollbar track pages, the thumb
    drags (matching `stdlib.ui_scrollbar`), and the close box's owner
    `EventKind.Close` cancels. `next_event` is filtered to the dialog's own
    window; an owner `Expose` re-reads `window_content_rect`; only a visible
    change repaints (key no-ops, plain moves and the drag press/release are
    free).
  - **Result:** `Ok` copies the whole picked path into `chosen` with one
    `core.substr_into` and closes the window; a cancel is `Invalid`, a
    too-long result is `CapacityExceeded`, and both leave `chosen` untouched.
    A disabled screen (`langhost`) returns `Invalid` before opening a window.
    No new opcode, builtin, `Host` method, file API or `stdlib` helper.
  - **Editor:** `samples/apps/base/editor.ig` says `uses ... lib.file_dialog;`,
    and File > Open and Ctrl+O call `browse_open`: `open_file(path,
    dialog_path)`, then `open_path(dialog_path)` on `Ok` and a status line on
    cancel. The editor raises its own window after the dialog, and a cancel
    leaves the document, path, caret, selection, scroll and highlighting
    untouched. `PathCapacity` rises 64 → 256 (`abi::fs::PATH_MAX`) for the
    dialog result and `TempCapacity` 96 → 260 for `<target>.tmp`; the check and
    host paths still call `open_path` directly and never open the dialog.
  - **Proof:** `scripts/file_dialog_qmp.py` boots `build/igis-shell.iso` with
    the tablet, runs the interactive editor, sends Ctrl+O, verifies the dialog
    frame/chrome/face pixels (left border black, title stripe on, list border,
    both button borders, white face) and a 28,708-pixel frame diff, then goes
    from `base` to the root with Backspace, clicks the root `lib` row, Enter,
    clicks the `math.ig` row (computed from the sorted `build/apps-shell-root/lib`
    stage: `file_dialog.ig, inject.ig, math.ig, ...`), clicks `Open` and
    requires: dialog closure diff 30,742 px, editor text diff 17,959 px, path
    line 231 px with the ink bbox growing 79 → 86 px (11 cells = `lib/math.ig`).
    Ctrl+O then Escape closes the dialog (30,405 px) with a **0-pixel** text
    diff. Ctrl+Q ends `task 6: exit Ok` (161 claims), the shell prompt returns,
    and there is no budget fault or idle summary. The exact same counts occur on
    `-smp 4` and `-smp 1`. Screenshots:
    `/home/sinan/Desktop/igis-file-dialog.png` (1024×768, 29,726 B) and
    `/home/sinan/Desktop/igis-file-dialog-open.png` (1024×768, 27,826 B).
  - **Docs:** spec 0013 "The text editor" and changelog; spec 0018 "The `/lib`
    file-open dialog" and changelog; PLAN/AGENTS. No ADR: this is a library
    module and an application call site, not a kernel or ABI decision.
  **Done (`be544fa`):** `lib/file_dialog.ig`, the editor `browse_open` wiring
  (`PathCapacity` 256) and `scripts/file_dialog_qmp.py`; `editor: ok` on
  `-smp 4`/`-smp 1`, `editor: host ok`, 505 host tests, `browser check`,
  `desktop_shell check`, `boot_desktop_qmp.py` and the visual golden
  (`37802052…e9e81c9f`) pass; the editor package is 202,328 B of code
  (50 frames), 38,872 B of state and 51,416 B of stack.

### M9.4 — the editor opens the real source files

- [x] **M9.4 The editor's document buffer fits every source sample.**
  User report: "I can open code like beat and deep, but not calculator the same
  way; why? There is no error either."
  - **Diagnosis:** the file dialog was correct (`base/calculator` came back
    with `Ok`), but the editor's `BufferCapacity` was 8192 and `open_path`
    returned `CapacityExceeded` with the status line `too large (8192 max)`.
    The status was easy to miss, so the document simply stayed unchanged.
    `beat` (554 B) and `deep` (874 B) are tiny; `base/calculator` is 32,516 B
    and `base/editor` (81,509 B) is the largest current sample. This is an
    editor application limit, not a language, kernel, file-API or dialog bug
    (spec 0003 said 8192 was an app constant).
  - **Capacity:** `BufferCapacity` is 131072 (128 KiB), matching the spec 0008
    per-unit source cap; `LineStateCapacity` follows at 131073. `PathCapacity`
    stays 256 and `TempCapacity` 260. The size refusal and the buffer-full
    status read `too large (131072 max)` and `buffer full (131072 bytes)`.
  - **State and stack:** the four 128 KiB buffers (`text`, `load_stage`,
    `save_stage`, `line_states`) and the `ed_replace` local staging buffer grow
    the manifest to 530,392 B of state (130 frames) and 174,328 B of stack
    (43 frames), below the 1 MiB per-region caps. The check still runs in
    2,120–2,123 claims across the `-smp 4` home/no-home runs and 2,120 on
    `-smp 1`, with no budget fault. No kernel change was needed.
  - **Proof:** `editor check` loads `base/calculator` after the existing error
    classes, requires `Ok`, the `base/calculator` path, a length above 8192,
    the exact `core.fs_size` length (32,516 B), valid UTF-8 and a known prefix,
    then restores `base/child`; the full-buffer `ed_replace` boundary still
    returns `CapacityExceeded`, and the M9.1/M9.2 highlighting, scroll and
    thumb checks still pass in the same run.
    `scripts/file_dialog_qmp.py` now opens `base/calculator` through the
    dialog after the existing `lib/math.ig` proof: the `base` listing's
    calculator row is selected (row 6, checked by its accent band), `Open`
    closes the dialog (frame diff 40,199 px), the editor text changes by
    27,137 px (22,720 non-white text pixels) and the path line grows to
    `base/calculator` (15 cells, bbox 118 px); Escape then cancels with a
    19-pixel text diff and Ctrl+Q exits `Ok` after 1,353 claims. The QMP
    click hold is 0.35 s so a busy dialog cannot lose the USB tablet press
    report. Screenshots: `igis-file-dialog.png`,
    `igis-file-dialog-open.png` and `igis-file-dialog-large.png`.
  - **Regressions:** 505 host tests; `cargo xtask build`/`image`/`image
    --shell` clean; `editor: host ok`; `browser check` and
    `desktop_shell check` pass; the visual golden is unchanged
    (`37802052…e9e81c9f`) on `-smp 4` and `-smp 1`; `build/home.img` is reset.
  - **Docs:** spec 0003 ("Application buffers above 256 bytes"), spec 0013
    ("The text editor"), PLAN/AGENTS. No ADR: the editor's capacity is an
    application constant, not a kernel or ABI decision. The dialog stays
    generic (it does not filter by size).
  **Done (`142c0f2`):** the 128 KiB `BufferCapacity`, the `base/calculator`
  large-source check and the calculator step in `scripts/file_dialog_qmp.py`;
  `editor: ok` on `-smp 4` and `-smp 1` (2,120–2,123 claims), `editor: host ok`,
  505 host tests, the QMP dialog proof on both core counts, `browser check`,
  `desktop_shell check`, and the unchanged visual golden
  (`37802052…e9e81c9f`). Editor package: 203,136 B code (50 frames), 530,392 B
  state (130 frames), 174,328 B stack (43 frames).

## M10 — networking

> Governed by **spec 0020 (Accepted)** and **ADRs 0048–0050 (Accepted)**. M10.0 is
> closed; implementation starts at M10.1. One ticket ≈ one session and one commit.
> The wire stack lives in a new `net` crate; the application surface is the
> `/lib/socket` unit on the program/interpreter side.

### M10.0 — accept the network spec and decisions

- [x] **M10.0 Accept spec 0020 and ADRs 0048–0050.**
  The planning session wrote spec 0020 (kernel `net` crate, `/lib/socket` unit,
  virtio-net first behind a `NetDevice` seam, static IPv4, UDP, minimal TCP,
  DNS/HTTP in source, static configuration, M10 split), three ADRs, the new
  OPEN-QUESTIONS Q52–Q58, and this backlog. The owner accepted the plan and
  asked for implementation to continue as sub-agent work.
  - **Acceptance:** the owner accepts spec 0020 and ADRs 0048–0050 (or names the
    changes); `specs/README.md` and `adr/README.md` show the accepted status.
  - **Proof:** document review only; no build change.
  - **Docs:** spec 0020, ADRs 0048–0050, PLAN/AGENTS/OPEN-QUESTIONS/IDEAS.

### M10.1 — the `net` crate, PCI discovery and virtio-net bring-up

- [x] **M10.1 Add the `net` crate skeleton and the first NIC driver.**
  New allocation-free `net` crate (`no_std`, `#![forbid(unsafe_code)]`, depends on
  `abi` only) with the `Link`/`Clock` seams and an empty `Stack`; a `NetDevice`
  trait in `kernel/src/net/mod.rs`; a generalized `kernel/src/pci.rs` that returns
  a device's vendor/device/class and BARs (the UHCI path keeps working); a
  virtio-net legacy driver in `kernel/src/net/virtio.rs` (PCI I/O BAR0, legacy
  status/feature/queue registers, one RX and one TX queue, 32/16 static 2 KiB
  buffers, descriptor/avail/used tables in page-aligned `.bss`, `VIRTIO_NET_F_MAC`,
  no offloads); boot selection and `net::service()` in the service opportunity.
  `xtask` adds `-netdev user,id=n0` and
  `-device virtio-net-pci,netdev=n0,disable-modern=on`, plus `--no-net`.
  `samples/apps/base/net_probe.ig` gains `mac` and `raw` modes.
  - **Acceptance:** boot logs the device id, MAC, link state and the static DMA
    cost; `net_probe mac` prints the MAC and `net: tx ok` proves one frame left the
    TX ring; `--no-net` logs `net: no device` and returns a clean status; the
    existing UHCI tablet path is unchanged.
  - **Proof:** `net: link up`/`net: tx ok` and `net_probe: ok` on `-smp 4` and
    `-smp 1`; `--no-net` on both; the visual golden is unchanged; no sample
    regresses.
  - **Docs:** spec 0020 N1–N4, ADR 0049; AGENTS/PLAN.
  **Done (`e08745d`):** PCI 1AF4:1000 at 00:03.0, MAC
  `52:54:00:12:34:56`, `net: link up`, `net: tx ok`, `--no-net` clean; the
  higher-half physical-address rule is handled like `usb.rs` (`virtual -
  KERNEL_BASE`).

### M10.2 — Ethernet, ARP, IPv4, ICMP and static configuration

- [x] **M10.2 Reach the gateway with ARP and ICMP echo.**
  Add `net::{ethernet,arp,ipv4,icmp,checksum}` with a 16-entry ARP cache (60 s,
  three requests, one queued packet), IPv4 validation/checksum/TTL, one address +
  netmask + gateway, and ICMP echo reply. Add `core.net_config` (62),
  `core.net_info` (63) and `core.net_mac` (64); the interpreter stages the scalar
  operands and the host copies only on success. `net_probe config`/`ping` sets
  `10.0.2.15/24`, gateway `10.0.2.2`, DNS `10.0.2.3`, resolves the gateway with ARP
  and pings it.
  - **Acceptance:** ARP resolves the gateway; an echo request is answered and
    printed; malformed/foreign/fragmented frames are dropped and counted; a bad
    checksum is rejected; `net_probe ping` prints `net: ping ok`.
  - **Proof:** host tests for header build/parse, checksum vectors (including the
    `0xFFFF` fold) and the ARP cache/queue; `net_probe check`/`ping` on `-smp 4` and
    `-smp 1`; `--no-net` returns `IoError`; `cargo test -p net` joins the suite.
  - **Docs:** spec 0005 (the three builtins), spec 0020 N5–N7/N18; AGENTS/PLAN.
  **Done (`e08745d`):** Ethernet/ARP/IPv4/ICMP with host tests; `core.net_config`
  (62), `core.net_info` (63) and `core.net_mac` (64); the static
  `10.0.2.15/24` config is applied by `net_probe`, and ARP/ICMP are proven by the
  `net` crate's simulated-link tests (the kernel proof is the HTTP exchange).

### M10.3 — UDP sockets, the host ABI and `lib/socket`

- [x] **M10.3 Open the first socket: UDP and the `socket` unit.**
  Add a fixed socket table (8 slots, generation handles, owner task, 4 KiB RX/TX),
  UDP demultiplexing with the per-datagram boundary rule, and builtins
  `core.socket_open` (65, kind/remote-ip/remote-port/local-port), `socket_send`
  (66), `socket_recv` (67), `socket_try_send` (68), `socket_try_recv` (69),
  `socket_close` (70). Add
  `WaitFor::Net(handle)`, `task::wake_net`, `Host::socket_readable` defaults,
  `core.socket_source` (compiler-lowered, `1 << 63 | handle`), and the close-on-task-
  termination rule. Write `samples/apps/lib/socket.ig` (`udp`, `udp_from`, `send`,
  `send_text`, `recv`, `recv_line`, `close`, `source`; the TCP functions land in
  M10.4) and a UDP proof.
  `langhost` gets a deterministic loopback socket host; `net_probe udp` either
  echoes through the host (QEMU `10.0.2.2`) or sends a DNS query if slirp does not
  forward guest UDP to the host.
  - **Acceptance:** a UDP datagram round-trips or a DNS query gets a response; a
    foreign/stale handle is `Invalid`/`NotFound`; a full table is `Full`; a
    closed/terminated owner wakes its peer with `Closed`; `next_any` selects a
    socket source.
  - **Proof:** host tests for the socket table, generation reuse, UDP demux and the
    boundary rule; `net_probe udp` on `-smp 4` and `-smp 1`; a `langhost` socket
    test; `socket: ok`/`net: udp ok`.
  - **Docs:** spec 0005 (socket builtins/wait), spec 0020 N8/N12–N15, ADR 0050;
    AGENTS/PLAN.
  **Done (`e08745d`, blocking in `5ae91bc`):** UDP, the fixed socket table,
  generation/owner handles, builtins 65–70 and `lib/socket.ig` are implemented and
  host-tested. The blocking form is now in too: `WaitFor::Net(handle)` sleeps a
  `send`/`recv`/`accept` that returns `WouldBlock`, `net::service` wakes on
  data/space/EOF/close/timeout after dropping its lock, and the post-block recheck
  closes the lost-wake window. `core.socket_source` (`1 << 63 | handle`) feeds
  `core.next_any`/`poll_any`; `samples/tests/apps/base/net_select.ig` proves the selector,
  a 300 ms recv timeout and a 300 ms select timeout. `kill_stress` is still
  300/300 and `mouse` 600/600.

### M10.4 — TCP client, deadlines and HTTP

- [x] **M10.4 A TCP client that can fetch a page.**
  Add the TCP client state machine (active open, handshake, MSS 1460, fixed receive
  window, one outstanding segment, 1 s RTO with exponential backoff and five
  attempts, RST, FIN close), `WaitFor::Deadline(ns)` and `core.socket_timeout` (71).
  Extend `lib/socket.ig` with `tcp`/`set_timeout` and write
  `samples/apps/lib/http.ig` (`GET path HTTP/1.0`, `Host`, `Connection: close`,
  body after the first `\r\n\r\n`, fixed 4096-byte result). The deterministic proof
  starts a host TCP HTTP server and forwards a guest address to it (the exact
  `guestfwd` form is fixed here); `net_probe http` fetches `/` and checks the body.
  - **Acceptance:** the handshake completes; data and FIN arrive; a lost segment is
    retransmitted and the transfer still completes; a refused connect and a
    timeout return `Closed`; `net_probe http` prints the expected body and
    `net: http ok`; no task exceeds its budget.
  - **Proof:** host tests against a simulated peer for handshake/retransmit/FIN/
    RST/timeout; `net_probe http` on `-smp 4` and `-smp 1`; an optional
    `net_probe internet` run (not in the regression suite) resolves and fetches a
    real page.
  - **Docs:** spec 0005 (timeout/deadline), spec 0020 N9/N13/N16; AGENTS/PLAN.
  **Done (`e08745d`, blocking in `5ae91bc`):** the TCP client (handshake, one
  outstanding segment, 1/2/4/8 s retransmit, 5 s connect timeout, FIN/RST,
  close), `lib/socket.ig`, `lib/http.ig` (HTTP/1.0 GET, header strip, 8 s app
  deadline) and `base/net_probe` are implemented. The blocking form uses
  `WaitFor::Net(handle)` with a per-socket deadline (`core.socket_timeout`, 0 =
  none, default 30 s for send/recv/accept); `WaitFor::Deadline(ns)` is a generic
  timer-woken arm. `scripts/net_qmp.py` proves a real HTTP body and
  `net_probe: ok` on `-smp 4` and `-smp 1`; `net_select` proves a 300 ms recv and
  a 300 ms `next_any` timeout.

### M10.5 — DNS resolution in source

- [x] **M10.5 Resolve a name with `lib/dns.ig`.**
  Write the DNS A-record resolver as ordinary source (transaction id, one question,
  recursion desired, response validation, first A record, CNAME chain up to four,
  two retries, 2 s deadline) over a UDP socket, and add `net_probe dns`. Host tests
  parse canned queries/responses, including truncated and malformed messages. The
  kernel proof uses QEMU's slirp DNS (`10.0.2.3`); a deterministic host responder is
  used if guest UDP to `10.0.2.2` works, otherwise the DNS kernel run is optional and
  the canned-packet host test is the deterministic proof.
  - **Acceptance:** a known name resolves to its A record; a malformed or NXDOMAIN
    response is `NotFound`/`Invalid` without touching the destination; the resolver
    never spins.
  - **Proof:** host tests for the parser and builder; `net_probe dns` (or the
    documented offline arm) on `-smp 4` and `-smp 1`; `net: dns ok`.
  - **Docs:** spec 0020 N16; AGENTS/PLAN. No kernel/ABI change.
  **Done (`2b8bd70`):** `lib/dns.ig` builds a 29-byte standard A query, sends one
  UDP datagram to `10.0.2.3:53`, validates the id/QR/opcode/TC/RCODE and the
  question, follows a CNAME chain up to four links and returns the first A record;
  `net_probe dns` prints `net_probe: dns 104.20.23.154 (0x6814179a)` and
  `net_probe: ok` on `-smp 4` and `-smp 1` (the two A records alternate in slirp).
  The HTTP probe still passes; `--no-net` prints `net_probe: no device`, an
  unknown mode is rejected, and a dead server times out after ~4 s (two 2 s
  attempts) without a false `ok`. A canned-packet `langhost` check is committed as
  `samples/lang/dns_check.ig` plus `langhost/tests/e2e.rs` (`af27de8`,
  `dns_check: ok`), covering the query builder and A/NXDOMAIN/CNAME/TC/truncated/
  overrun responses; the kernel proof remains the end-to-end evidence.

### M10.6 — NIC interrupts and network counters

- [x] **M10.6 Add INTx and prove the poll fallback.**
  Route the NIC's PCI INTx (QEMU GSI 11) through the I/O APIC to the boot core as
  vector `0x2D`; the handler only sets a pending flag, acknowledges the device with
  one read-to-clear register access and EOIs, while `net::service()` does the
  used-ring drain and the protocol work (ADR 0051 refines ADR 0023's "no lock in a
  handler" rule; the rings are not SPSC-safe against the service path, so a flag is
  the bounded handler). Add `core.net_stats` (73) and
  `samples/tests/apps/base/net_perf.ig` (ten timed HTTP GETs, min/avg/max from
  `core.clock()`, counters before/after). Polling stays the default when the route
  is absent, when the device refuses to unmask, or when the boot asks for it:
  `xtask --net-irq intx|poll` writes `igis.net_irq=poll` into the multiboot2 command
  line, so the polled proof runs the same kernel and image.
  - **Acceptance met:** the boot log shows `ioapic: version 0x20, IRQ11 -> vector
    0x2d on apic 0 (level)` and `net: irq ok (vector 0x2d)` (or
    `net: polling (igis.net_irq=poll)`); QEMU `-d int` counts 103 `v=2d`
    deliveries for one virtio `net_perf` run, so the line is used; the same
    transfers complete with interrupts and with polling only; the handler takes no
    kernel lock; counters move (`17/23 -> 77/113` virtio, `9/12 -> 69/102` e1000,
    `15/21 -> 75/111` e1000e).
  - **Proof:** `net_probe http`/`dns`/`dhcp` and `net_perf` with INTx and with
    `--net-irq poll` on `-smp 4`/`-smp 1` over virtio, e1000 and e1000e;
    `scripts/net_qmp.py` passes both core counts; `--no-net` is clean; the visual
    golden and `boot_desktop_qmp.py` are unchanged. `net_perf` numbers are in the
    AGENTS status row.
  - **Docs:** spec 0020 N4/N19/N22, ADR 0023/0049 + **ADR 0051**; AGENTS/PLAN.
  - **Device notes:** legacy virtio can latch its PCI INTx level high from the
    boot-time test frame (the PCI layer only turns a level *change* into an
    interrupt), so `net::enable_irq` reads the ISR once after the route is
    installed; e1000 masks with `IMS` and unmasks only the receive causes with
    `IMC` (the old `IMC = u32::MAX` at reset actually unmasked everything, which
    polling hid). No MSI/MSI-X.

### M10.7 — TCP listener and the in-guest echo server

- [x] **M10.7 Accept a connection and echo it back.**
  Add `listen`/`accept` (`listen` is `socket_open` kind 3; `accept` is builtin 72;
  a two-entry pending queue) and the `/lib/socket` functions. Write `samples/apps/base/net_echo.ig`: listen on port 7,
  echo lines from three sequential clients, close on FIN, print `net_echo: ok`.
  `xtask`/the harness forward a host port to guest 7, and
  `scripts/net_echo_qmp.py` connects, sends lines, checks the echoes and the exit.
  - **Acceptance:** a listener accepts a connection; an empty pending queue is
    `WouldBlock`; a third pending connection is `Full`; the echo is byte-exact;
    closing the listener closes pending connections; the task exits `Ok`.
  - **Proof:** `net_echo` plus the host harness on `-smp 4` and `-smp 1`; host
    tests for the pending queue and the accept ownership transfer.
  - **Docs:** spec 0020 N10; AGENTS/PLAN.
  **Done (`efeeadf`):** `net` gains `KIND_LISTEN`, a two-handle pending queue,
  inbound SYN/SYN-ACK/final-ACK, backlog-full RST, SYN-ACK retry/expiry and
  listener cleanup; `core.socket_accept` (builtin 72), `lib/socket.listen/accept`
  and the `net_echo` app. `scripts/net_echo_qmp.py` forwards host 7007 to guest 7,
  sends three sequential lines, checks byte-exact echoes and `net_echo: ok` on
  `-smp 4`/`-smp 1` over virtio, e1000 and e1000e. HTTP/DNS/DHCP still pass; 590
  host tests; golden unchanged.

### M10.8 — DHCP client in source

- [x] **M10.8 Configure the machine from a DHCP lease.**
  Write `samples/apps/lib/dhcp.ig` over the UDP socket unit: broadcast DISCOVER from
  local port 68 (kind 2 with `local = 68`), parse OFFER, send REQUEST, parse ACK,
  apply address/netmask/gateway/DNS through `core.net_config`; bounded retries and a
  lease timer; `net_probe dhcp` prints the lease. QEMU's slirp DHCP (`10.0.2.2:67`)
  is the target; the socket accepts the OFFER/ACK from the server even though the
  request went to the broadcast address, and the unit validates the transaction id
  and offered address. Host tests cover message build/parse, option ordering and a
  NAK.
  - **Acceptance:** a cold boot that runs `net_probe dhcp` obtains `10.0.2.15/24`,
    gateway `10.0.2.2`, DNS `10.0.2.3`, and the existing ping/HTTP probes then pass
    without a static config; a missing server times out cleanly; a NAK restarts
    discovery.
  - **Proof:** `net_probe dhcp` followed by `ping`/`http` on `-smp 4` and `-smp 1`;
    host tests for the parser; `--no-net` still returns cleanly.
  - **Docs:** spec 0020 N18; AGENTS/PLAN. No kernel/ABI change (the unit uses the
    existing socket surface).
  **Done (`3752d17`):** `lib/dhcp.ig` (DISCOVER/OFFER/REQUEST/ACK, xid/cookie,
  options 1/3/6/50/53/54/55/51, broadcast from local port 68, 4+4 attempts under a
  6 s cap, NAK restart) and `socket.udp_from`; `net_probe dhcp` applies
  `10.0.2.15/24`, gateway `10.0.2.2`, DNS `10.0.2.3`, fetches the 1,406-byte HTTP
  body and prints `net_probe: ok` on `-smp 4`/`-smp 1`. A one-line `net` fix allows
  a broadcast send before an address is configured; `cargo test -p net` is 43.
  Also proven over e1000/e1000e (`c7989c4`).

### M10.9 — e1000/e1000e behind the same seam

- [x] **M10.9 Prove the device seam with a second driver.**
  Add an `e1000`/`e1000e` driver (MMIO BAR0 mapped uncached, TX/RX descriptor rings,
  INTx or poll) behind `NetDevice`; `xtask` selects it with
  `--net-device e1000`; `net_probe`/`net_echo` run unchanged. Record the VMware
  mapping (VMware exposes `e1000`/`e1000e`/`VMXNET3`, not virtio-net) and keep
  `VMXNET3` as a later option.
  - **Acceptance:** the same `net_probe` suite passes on the e1000 device; the
    protocol crate and `/lib` units are byte-identical; a machine with both devices
    selects the requested one; `--no-net` still works.
  - **Proof:** `net: e1000 ...`, `net_probe check/ping/http` on `-smp 4` and
    `-smp 1`; the virtio run still passes; no regression.
  - **Docs:** spec 0020 N2, ADR 0049; AGENTS/PLAN.
  **Done (`c7989c4`):** a polling e1000/82540EM (`8086:100e`) and e1000e/82574L
  (`8086:10d3`) driver behind `NetDevice` (MMIO BAR0 mapped uncached, 32 RX + 16 TX
  legacy descriptor rings with static 2 KiB buffers, monotonic TX cursor, RX tail
  give-back, memory-space + bus-master enable). `xtask --net-device
  virtio|e1000|e1000e` selects the QEMU device (default virtio); `--no-net`
  unchanged. HTTP and DNS pass on e1000/e1000e on `-smp 4`/`-smp 1`, DHCP + HTTP
  passes on both, virtio is unchanged, and a 10× HTTP/DNS stress run completed
  11/11. The VMXNET3 path remains a future option.

**M10 definition of done (every ticket):** the standard verification in "How a
ticket is verified" plus `cargo test -p net`; `-smp 4` and `-smp 1` transcripts;
`--no-net` where the ticket can run without a device; the visual golden unchanged;
`AGENTS.md` status/log row and this file updated. IPv6, TLS/HTTPS, Wi-Fi, multiple
NICs and TCP congestion control remain out of scope (OPEN-QUESTIONS Q52–Q58).

## M11: Terminal emulator, app tree and Apps sections

Normative: spec 0021 and ADR 0053; specs 0012, 0018, 0005 and 0003 amended.
Goal: the terminal is an application emulator over kernel console sessions, GUI
apps live under `/apps/<category>`, and the Apps menu has sections.

- [x] **M11.1 Specs and ADR.** Spec 0021, ADR 0053, the supersession of 0014/0028,
  and the amendments to 0012/0018/0005/0003. No code.
- [x] **M11.2 Session ABI and interpreter.** `core.session_open/close/send/recv`
  (builtins 74–77), compiler-lowered `core.session_source`, `Host` defaults, the
  `abi::wait` source tag, and host tests. Retire `core.terminal_attach`.
- [x] **M11.3 Kernel sessions and routing.** A static session table sized from
  `MAX_TASKS`, task session inheritance, `core.write`/`read_input` routing,
  focused-session serial input, wake paths and `Task::reclaim` cleanup.
- [x] **M11.4 Terminal emulator and shell.** `apps/utilities/terminal.ig`, the
  session shell (`base/shell`), `terminal_probe`, the retirement of the kernel
  terminal window, and the one-window System Log.
- [x] **M11.5 `/apps` tree.** Move the six GUI app sources to
  `apps/accessories|games|productivity`, add the top-level `apps` directory to
  xtask/ext2/langhost, and implement the bare-name search order.
- [x] **M11.6 Apps menu.** `stdlib.ui_menu_is_header` / header rendering,
  section table, Terminal item, direct `Terminal` removal, File window list and
  the desktop check.
- [x] **M11.7 Regression and proof.** Host tests, `-smp 4`/`-smp 1` transcripts,
  two-terminal QMP proof, updated app-menu/boot scripts, visual golden, and
  `AGENTS.md`/`HISTORY`/PLAN status.

## M12: Runtime display modes

Normative: spec 0022 and ADR 0056; spec 0011 W32, specs 0009/0012/0005 amended.
Goal: the Settings app changes the **real** screen mode without a reboot.

- [x] **M12.1 Spec, ADR and language surface.** Spec 0022, ADR 0056, builtins
  80–83 (`display_mode_count/width/height`, `set_resolution`), compiler
  diagnostics, interpreter dispatch and `Host` defaults.
- [x] **M12.2 Device and shadow swap.** `arch/x86_64::bochs` (QEMU std VGA
  dispi), the 16 MiB LFB aperture reservation, `frame::reserve_range`,
  `gfx::change_mode` under the graphics lock, and allocation-before-programming.
- [x] **M12.3 Window rebuild.** `window::screen_changed`: scaled/clamped origins,
  dropped drag/grab, clamped region claim, full repaint and an `Expose` for every
  window; the desktop shell re-reads the screen and re-claims the menu bar.
- [x] **M12.4 Settings app.** `apps/utilities/settings` with the mode list,
  Apply, `check` and `shot`; the Apps menu's `Utilities` section gains the row.
- [x] **M12.5 Proof.** `samples/tests/apps/base/display_probe.ig`,
  `run settings check` (8 modes, 800×600, restore 1024×768), `desktop_shell
  check`, `scripts/settings_resolution_qmp.py` (shell app, screendump exactly
  800×600) and `scripts/settings_desktop_qmp.py` (Apps > Settings > Apply on the
  boot desktop, exactly 800×600) on `-smp 4`; the same suite on `-smp 1`.

## Deferred (not UI; pick up when they matter)

| Item | Where | Trigger |
|---|---|---|
| `f32` (Q40) | OPEN-QUESTIONS Q40 | when single precision is needed |
| virtio-blk behind `SectorDevice` | spec 0019 | when the ATA path is measured |
| A journal or an atomic-replace primitive | spec 0019 | when the editor must save without `<target>.tmp` |
| A file handle / open-once model (Q51) | OPEN-QUESTIONS Q51 | if the per-call path walk measures badly |
| Recovery from a hardware guard hit (Q47) | OPEN-QUESTIONS Q47 | when a task must survive a kernel-bug guard hit |
| MMIO/DMA/block handles (Q45/Q9) | OPEN-QUESTIONS | first real block device or MMIO peripheral |
| Debug/introspection primitive (Q8) | OPEN-QUESTIONS Q8 | when the desktop needs task/system info |
| Nominal `TaskId` (Q43) | OPEN-QUESTIONS Q43 | before `stdlib` exposes task primitives |
| Runtime-capacity buffer parameters (Q46) | OPEN-QUESTIONS Q46 | when slicing/wider stream reads need it |
| Explicit stream close (Q10) | spec 0006 | streams v2 |
| Compile arena: 16 MiB with `/lib` in use (the `widgets` pair needs 6.55 MiB) | spec 0008 | watch as modules grow |
| Module dead-code elimination: importing a module links all its functions (`ui_controls` +752 B for `inject`) | compiler | when package code size matters |
| Guard table cache exhaustion (32 tables) | spec 0002/spec 0009 | long-lived workloads with many stacks |
| `cargo fmt` drift in three pre-existing files + CI fmt/clippy | repo | housekeeping |
| TLS/HTTPS (Q52) | OPEN-QUESTIONS Q52 | first real web application |
| IPv6 / dual stack (Q53) | OPEN-QUESTIONS Q53 | an IPv6-only peer matters |
| Wi-Fi / 802.11 (Q54) | OPEN-QUESTIONS Q54 | a laptop target |
| Multiple NICs, routing table (Q55) | OPEN-QUESTIONS Q55 | a second device or a real gateway |
| TCP backlog, multiple listeners (Q56) | OPEN-QUESTIONS Q56 | first real server application |
| DHCP renewal, DNS cache, ICMP errors (Q57) | OPEN-QUESTIONS Q57 | M10.8 follow-up |
| VMXNET3 and modern virtio | ADR 0049 | a VMware target without e1000e, or a modern-only hypervisor |

## How a ticket is verified (headless first)

1. Host tests: `cargo test -p abi -p compiler -p interpreter -p langhost -p ext2w -p net`
   (the filesystem crate is host-tested against an `mke2fs` image; `net` is
   host-tested against a simulated link).
2. Kernel transcript with a self-check the app prints (`files: ok` style), using
   `syslib.get_pixel` for drawing and `syslib.inject_event` for input; filesystem
   tickets also run `e2fsck -fn build/home.img` after the proof.
3. `-smp 4` and `-smp 1` both; filesystem tickets run with the home drive and with
   `--no-home`.
4. Regression batch: `files`, `module_demo`, the home apps, `editor`/`browser`/
   `calculator`/`minesweeper`, the UI set, `widgets`, `clock_window`, `windows*`,
   `desktop_shell check`, `terminal`, `events`, `streams`, `frames`, `deep`,
   `utf8`, `realtime`, `cores`, `kill_stress`, `multiplex`, `font`, `perf`,
   `f64parse`, `text`, `text_edit`, and `net_probe` (with `scripts/net_qmp.py`
   when the network path changes).
5. `scripts/visual_regression.py` for the framebuffer: `samples/apps/visual` is
   deterministic and the committed golden must match on both core counts;
   `--tamper` proves the compare rejects a changed pixel. `scripts/boot_desktop_qmp.py`
   proves the boot desktop → `terminal: ok`, and the per-app screenshot harnesses
   (`calculator`, `minesweeper`, `browser`, ...) are run when the app changes.
6. Leave `build/home.img` reset (`cargo xtask image --reset-home`) and fsck-clean
   after a filesystem session.
