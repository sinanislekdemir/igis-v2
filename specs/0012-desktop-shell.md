# 0012: The desktop shell and kernel text windows

- **Status:** Accepted
- **Scope:** `kernel` (`console`, `gfx`, `window`, `sched`, `main`), `compiler`,
  `interpreter`, `abi`; `samples/apps`; M2.3.4 of the desktop milestone
- **Supersedes:** the full-screen console of spec 0009 G14 (superseded in part)
- **Superseded by:** none
- **ADRs:** 0020, 0024, 0025, 0029, 0030
- **Updated:** 2026-11-17

## Context

M2.1–M2.3 delivered the shadow screen, the input model and the window service. The
console, however, is still the M2.1 full-screen text device: it paints at screen
coordinates, scrolls the whole shadow, and therefore scrolls every composed window with
it (the limitation recorded at the end of M2.3.2). M2.3.4 has to make the desktop an
image that survives text output, then put the first real shell on top of it.

Two things are needed:

1. Text output must live in **windows**, not on the whole screen. Applications print
   through `core.write`; the kernel prints diagnostics through `println!`. They must not
   share one cursor, and neither may scroll the desktop.
2. A **desktop shell** application must draw the System 7.5-style menu bar, launch
   applications without blocking itself, and list/raise existing windows. It is an
   ordinary application, not boot code. M2.3.4 ran it manually (`run desktop_shell`);
   M2.5.6 makes it the boot entry and gives it the `Terminal` item that starts the text
   shell, so booting the image lands on the desktop.

M2.3.4 does not add a text widget, a control toolkit, dropdown menus, icons, drag or
resize. Those are M2.4/M2.5.

## Requirements

### Kernel text windows

- **D1 (MUST):** After `gfx::init` and before the first kernel `println!`,
  `console::init` opens exactly two kernel-owned text windows with owner id `0`: a
  **terminal** for application output (`core.write`, including the shell's own input
  echo), and a **system log** for kernel `println!`/`print!` diagnostics. When graphics
  are disabled, no window is opened and every byte still reaches serial through the
  existing mirror.
- **D2 (MUST):** Each window has its own character grid, cursor and scroll state. The
  grid dimensions derive from the window's **content** rectangle and the 8×8 font at
  scale 1. A full column wraps and a newline at the last row scrolls that window's grid
  one row; it never scrolls the shadow or touches another window. Cells of a grid that
  are not printable are blank.
- **D3 (MUST):** A cell is painted only inside the intersection of its own rectangle,
  the window's content rectangle and the window's **visible parts** (content minus the
  frames of higher windows). The console passes that clip **explicitly** to the drawing
  helpers (`gfx::draw_cell`/`gfx::present_within`) and never narrows the shadow's one
  global clip: a whole-grid repaint must not clip an application drawing on another
  core. A cell that is partly or wholly covered leaves the covering window untouched. A
  painted cell joins the terminal's bounded dirty rectangle instead of being presented
  immediately (M2.6.2, spec 0016): `console::write_bytes` and `_print` present once after
  their byte run, one flush at the end of `poll_input` presents anything still
  pending, and a whole-window repaint presents each visible part once. The flush
  reads the visible parts under the window lock and copies only the dirty rectangle from
  the current shadow, so a window that moved above the console in between cannot be
  overwritten. Because a window on another core can open, raise or move above the
  console between the visible-parts query and the paint, the console holds the window
  table lock across the whole cell/part paint (`window::paint_visible_parts`); this is
  the same `WINDOW → SCREEN` order as the service's repair, and the paint callback never
  calls the console. A window opened or moved afterwards runs its own repair, which
  paints above the console; the console follows a moved window through the
  visible-geometry generation on the next pump.
- **D4 (MUST):** On a pending `Expose` for owner `0`, the console repaints that window's
  grid. The repaint is **pulled**, not called from the window service: `console::pump`
  takes exposed owner-0 windows at the service opportunity (and the idle loop, through
  the same `console::poll_input`). This keeps the lock order `CONSOLE → WINDOW →
  SCREEN`; no path takes `WINDOW` and then `CONSOLE`.
- **D5 (MUST):** The routing is fixed: `print!`/`println!` (`console::_print`) feed the
  system log; `core.write` feeds the terminal (the application echoes its own input);
  every byte of both is still mirrored to COM1. Text is never echoed to both windows.
- **D6 (MUST):** Owner id `0` means "the kernel". Global task ids start at `1`, so no
  task can be confused with a kernel window. A kernel window is an ordinary window for
  the service: it has geometry, z-order, focus, `Expose`, hit-testing and
  `window_present`, and the kernel calls the same public `window::*` functions with
  owner `0`. A task can raise or focus it (a pointer click) but cannot close or move it.
- **D7 (MUST):** Layout is derived from the reported screen; nothing assumes 1024×768.
  The terminal frame is at `(8, 26)`, the log frame follows it with an 8-pixel gap, and
  both use the full height minus an 8-pixel bottom margin. The top margin keeps the
  frames clear of the 22-pixel menu bar of D8. If the derived content is below the 8×8
  window minimum, no window is opened (serial only). Both titles are ordinary window
  titles (at most 64 bytes).
- **D8 (MUST):** A full-screen `syslib.clear` by an application erases the console
  windows' pixels in the shared shadow, because the service has no per-window canvas.
  Their records and grids survive; the next `Expose` or console output paints the cells
  again. This is a documented consequence of the one-shadow model, not a service bug.
  The desktop shell itself does not clear the screen.

### Desktop shell

- **D9 (MUST):** `samples/apps/desktop_shell` is an ordinary application. It is also the
  boot entry from M2.5.6: the image stages the same source under the file name
  `entrypoint`, and the REPL tries `entrypoint` first and `shell` second. The file name
  is the load path; the source keeps `unit desktop_shell;`, which is only diagnostic
  identity. An image without `entrypoint` therefore boots the text `shell` exactly as
  before M2.5.6. The desktop never attaches the console terminal; the `shell` it
  launches does (spec 0014, the one-terminal rule).
- **D10 (MUST):** The shell draws a System 7.5-style menu bar at the top of the screen:
  a white bar 22 pixels high, a black 8×8 Apple square, the labels `File`, `Apps`,
  `Windows` and `Terminal`, a black baseline on row 21, and the `core.clock()` `HH:MM`
  right-aligned. The bar is application-drawn and is presented after it is drawn and
  redrawn after every event. There are no dropdown menus in M2.3.4: each label is a
  direct item; `Terminal` is the M2.5.6 addition.
- **D11 (MUST):** A pointer-down with `window = 0` at a menu item runs it:
  - `File` enumerates the live windows and writes their id, title and frame into the
    terminal window;
  - `Apps` launches `clock` with `core.spawn` (non-blocking), records the task id, and
    reports the outcome;
  - `Windows` raises the existing window titled `Terminal` (the "bring the terminal
    forward" affordance without a dropdown; it sits left of the shell's window, so the
    raise cannot uncover the shell);
  - `Terminal` launches `shell` with `core.spawn` when no shell has been launched yet,
    and records the task id. The desktop keeps pumping and never waits for it. After the
    first launch, a Terminal click raises the live window titled `Terminal` instead of
    spawning a second shell (which the one-terminal rule would refuse with `Full`); if
    that window is gone because the shell exited, the click spawns a fresh shell.
  - the Apple square raises the shell's own window.

  A launched application is never waited for: the shell uses `core.spawn`, not
  `core.run`.
- **D12 (MUST):** The shell may own one window of its own ("Desktop"). It draws the
  window content on open and on every `Expose` for that id, calls `window_present`, and
  ignores exposes for windows it does not own. It repaints the content only while it is
  the topmost window; when another window is above it, it presents without redrawing, so
  a client with no per-task clip (M2) cannot overwrite the window above. It never draws in
  another window's content rectangle.
- **D13 (MUST):** `check` mode is deterministic and headless: draw the bar and window,
  check menu-bar/chrome/content pixels with `get_pixel`, call each menu item's handler
  directly (the boot desktop is itself an event consumer from M2.5.6, so an injected
  click from a second instance could be taken by it; the real pointer path is proved by
  the boot harness and the routing proofs), verify the listing (Terminal and System Log
  titles present), the spawn result and the focus change, verify that the `Terminal`
  item raises the already-live terminal (check mode runs from a shell that holds it, so
  it never spawns a second shell), verify enumeration ends in `NotFound` and
  `window_present` succeeds, and print `desktop_shell: ok`. Normal mode draws the same
  image, prints `desktop_shell: ready`, promotes itself to the realtime class with
  heartbeats (falling back to a normal loop when admission is full), and pumps
  `next_event` until it is killed.
- **D14 (MUST):** There is no global work-area rule in M2.3.4. The shell places its own
  window below the bar, and kernel windows start at `y = 26`. An application that opens
  a window at `y = 0` covers the menu bar; the shell repaints the bar on its next event
  and the overlap is accepted until a documented work area lands with a later slice.
- **D15 (MUST):** Window enumeration is read-only and adds no bytecode opcode:

  ```text
  syslib.window_info(index: uint, var id: int, var rect: Rect, var title: string[64]) -> Status
  ```

  `index = 0` is the **topmost** window, increasing towards the bottom of the z-order.
  `Ok` writes the id, the **frame** rectangle and the title (at most 64 bytes, the
  service's title cap) and pushes `Ok`; an index at or past the live count, or disabled
  graphics, is `NotFound` and writes nothing. The compiler requires a `var string`
  destination of at least capacity 64; the interpreter bounds-checks the destination
  before calling the host. It is builtin **40** and the ABI opcode table is unchanged.
- **D16 (MUST):** With graphics disabled (`screen_width() == 0`), the desktop prints
  `desktop_shell: no graphics`. In normal (boot) mode it then spawns the text `shell`
  with `core.spawn` and exits, so the machine keeps a serial REPL; in `check` mode it
  exits without spawning. The kernel windows do not exist and the console is
  serial-only. `window_info` is `NotFound` for every index.
- **D17 (MUST):** Lock order stays acyclic: console drawing and `console::pump` take
  `CONSOLE`, then `WINDOW` (to read visible parts **and hold through the paint**), then
  `SCREEN`; the window service's repair takes `WINDOW`, then `SCREEN`; no path takes
  `WINDOW` and then `CONSOLE`. Producers never take the window lock.
- **D18 (MUST):** M2.5.5 extends D1/D4: the **terminal** half may be attached to an
  ordinary task-owned window through `core.terminal_attach` (spec 0014, ADR 0028). The
  first successful task attach releases the owner-`0` boot Terminal; the System Log
  stays owner `0`. The console then pulls the terminal's `Expose` by id and the log's by
  owner `0`, keeps D3's explicit-clip/generation rules and D17's lock order, and
  detaches to serial-only when the terminal's window disappears. A terminal repaint
  asserts the attached task's output, including children it spawns, as D5 already
  requires; it does not add a second terminal or per-task routing.
- **D19 (MUST):** Boot lands on the desktop. The REPL tries `entrypoint` first and
  `shell` second (spec 0002, spec 0005); the image stages `desktop_shell` under both
  names (`entrypoint` is an alias, not a second source). The boot desktop draws the bar
  and its `Desktop` window, prints `desktop_shell: ready`, promotes to realtime with
  heartbeats, and pumps events; it never exits and never waits on a child. The text
  shell is started by the `Terminal` item (D11), and the desktop never attaches the
  terminal itself. If `entrypoint` is absent or fails to load, `shell` boots unchanged;
  if graphics are disabled, D16's serial-shell spawn runs. The desktop is a permanent
  consumer of the one machine-wide event queue (spec 0010 I31), so an application that
  also calls `next_event` competes with it; the event-injecting regression suite runs on
  the shell-entry image (`cargo xtask image --shell`), which has no `entrypoint` and is
  exactly the pre-M2.5.6 boot. The shell's terminal (`core.write` and its own
  `core.read_input` echo) is independent of the queue, though the shell's input loop
  also consumes key events when it is waiting at the prompt.

## Design

### Console state and drawing

```text
Console
  enabled: bool                  // false until console::init; gates text output
  term: Terminal                 // core.write (the client echoes its input)
  log:  Terminal                 // println!/print!

Terminal
  id: u64                        // owner-0 window id, 0 when unattached
  attached: bool
  cols, rows: usize              // from the content rectangle / 8
  col, row: usize                // cursor
  x, y: i64                      // content origin
  fg, bg: u32                    // black on white, light grey for the log
  grid: [[u8; 128]; 128]         // bounded by the largest supported grid
```

Both terminals are fields of one `SpinLock<Console>` (a `const`-initialised static; the
32 KiB of grids live in `.bss`, never on a stack). `Console::init` is called on the boot
stack but mutates the static in place.

`put_byte` serial-mirrors and feeds one terminal. The feed handles `\n`, `\r`, `\b`, `\t`
and `0x20..=0x7E`; other bytes are dropped after the serial mirror, as before. A
printable byte updates the grid, paints its cell and advances; at `cols` it wraps, and a
newline at the last row scrolls the grid (`copy_within` one row up, clear the last row,
repaint the visible parts).

Painting one cell asks the window service for the content's visible parts
(`window::visible_content_parts`, kernel-internal, no application binding) and, for each
part the cell intersects, calls `gfx::draw_cell` (opaque background plus one glyph,
clipped to the part) and `gfx::present_within` (that intersection). The clip is a
parameter, not the shadow's global one, so no other task's drawing is affected while a
grid repaints. A full repaint draws every visible cell of every part and presents each
part once. The cursor is composited by `present_within` as usual.

`console::pump` drains `window::take_expose(0)`; for each id it finds the matching
terminal and repaints it. It runs at the end of `console::poll_input`, so every service
opportunity and every idle-poll cycle pulls pending kernel-window repaints. A repair that
uncovers a kernel window is therefore visible one scheduler cycle later (bounded), and
the repair path never calls back into the console.

`console::route_self_check` runs once after `console::init`, before the boot log: it
writes a probe byte through the real `write_bytes` (terminal) and `_print` (log) paths,
inspects both grids, checks one foreground and one background pixel of each cell with
`get_pixel`, then clears the cells. A mismatch halts the boot; success prints
`console: route ok`.

### Boot order

```text
serial, keyboard, frame, gfx::init, clip self-check
console::init       // desktop fill, two owner-0 windows, grids
console::route_self_check
keyboard decoder self-check and the ordinary boot log (system log window)
repl::boot          // `entrypoint` (the desktop), else `shell`, else `no entry`
```

`entrypoint` is an image alias of `samples/apps/desktop_shell` (the unit keeps its name);
`--shell` omits it and exercises the fallback. The desktop's normal loop is:

```text
draw menu bar + Desktop window
promote to realtime
print `desktop_shell: ready`
loop: heartbeat -> next_event -> Expose repaint / menu action -> redraw bar
```

`Terminal` calls `core.spawn("shell", "", diag, id)` and does not wait. The shell opens
its 560×360 window, attaches the console terminal and runs its command loop. A second
Terminal click raises the live window instead of spawning a second shell.

`gfx` keeps only the generic primitives; the whole-screen console helpers
(`console_clear`, `console_glyph`, `console_scroll`) and the shadow scroll are removed.

### Desktop shell composition

```text
run desktop_shell          // normal: menu bar, own window, ready, pump events
run desktop_shell check    // deterministic self-check, prints desktop_shell: ok
```

The shell draws its window content with the ordinary nine drawing primitives and
presents it with `window_present`. Because the service does not keep content, every
`Expose` for the shell's id repaints the same text. The menu bar is redrawn after every
event, which also repairs a bar an app covered.

`File` lists windows through `window_info` (`index` 0 upward until `NotFound`) and writes
the listing into the terminal window. `Apps` calls `core.spawn("clock", "", diag, id)`
and keeps `id`; `Windows` finds the window titled `Terminal` in the same enumeration and
raises it. A normal-mode task promotes to realtime and heartbeats before blocking in
`next_event`; blocked time does not count against the heartbeat, and each wake beats
again. `draw_shell_window` enumerates the topmost id first and only redraws its content
while that id is its own; otherwise it presents what the service last left.

### Proof

- Host tests: the `window_info` builtin's operand order, destination writes on `Ok`,
  untouched destinations on `NotFound`, the disabled-service default, and the compiler
  signatures (four arguments, `uint` index, `var int` id, `var Rect`, `var string[64]`).
- Kernel: `console: route ok` on `-smp 4` and `-smp 1`; the serial transcript still
  carries the whole boot log and the sample output; `desktop_shell: ok` in check mode
  and `desktop_shell: ready` in normal mode.
- Boot-to-desktop: `scripts/boot_desktop_qmp.py` boots `build/igis.iso` with no manual
  `run`, waits for `desktop_shell: ready`, clicks the `Terminal` menu item through QMP
  absolute pointer events, waits for `shell> `, types `terminal`, waits for
  `terminal: ok`, and captures `/home/sinan/Desktop/igis-boot-desktop.png` (PNG, PPM
  fallback). The image is reported by path, dimensions and byte size; it is never
  opened for viewing. The same harness runs on `-smp 4` and `-smp 1`.
- Shell fallback: `cargo xtask image --shell` stages the same sources without the
  `entrypoint` alias; the transcript shows `repl: entrypoint: not found`,
  `repl: trying `shell`` and a working shell. The event-injecting regression suite runs
  on that image because the boot desktop is a permanent consumer of the machine-wide
  queue (D19).
- No-graphics fallback: a `-vga none` boot prints `graphics: disabled (no framebuffer)`,
  `desktop_shell: no graphics`, `desktop_shell: serial shell task N`, and then a working
  `shell> ` prompt on serial (the desktop exits and the shell is the only task).
- Screenshot: `scripts/desktop_screenshot_qmp.py` on the shell-entry image runs
  `desktop_shell`, waits for `desktop_shell: ready`, and captures
  `/home/sinan/Desktop/igis-desktop.png` through QMP `screendump` (PNG, PPM fallback).
- Regressions: `events`, `mouse`, `desktop`, `text 42 hi`, `frames`, `streams`, `deep`,
  `utf8`, `realtime`, `cores`, `windows`, `windows_demo`, `windows_events`, both QMP
  pointer harnesses. The three samples that assumed a window-free baseline (`events`,
  `windows`, `windows_events`) record `window_focus()` at start and use it as the
  expected idle focus.

## Failure modes

| Situation | Behaviour |
|-----------|-----------|
| Graphics disabled | No kernel windows, no desktop shell window; console serial only; the desktop prints `desktop_shell: no graphics` and, in boot mode, spawns the text `shell` so the serial REPL survives |
| Screen too small for two 8×8 windows | No kernel windows opened; serial only, graphics primitives still work; the desktop spawns the text `shell` in boot mode |
| `entrypoint` absent from the image | The REPL tries `shell` next; the pre-M2.5.6 boot is unchanged |
| `entrypoint` fails to read or compile | The diagnostic is logged, the reserved id is burned, and the REPL tries `shell` |
| A second Terminal click while the shell lives | The desktop raises the existing `Terminal` window; no second shell is spawned |
| A Terminal click after the shell exited | Its window is gone, so the desktop spawns a fresh shell |
| A manually launched second `desktop_shell` | It draws a second bar and has its own `shell_launched` flag; single-instance rules remain an M2 limitation |
| Opening a kernel window fails (`Full`) | That terminal is unattached; its bytes still reach serial; the other may exist |
| An application calls `syslib.clear` | Console pixels are erased; grids survive; the next expose/output repaints |
| A window covers a kernel window | The kernel grid keeps the hidden cells; the covering pixels are never overwritten; uncover marks `Expose` and the pump repaints |
| Console window is clicked | Ordinary hit-test/raise/focus; the repair marks `Expose`; the pump repaints |
| `window_info` index past the end | `NotFound`, destinations untouched |
| `window_info` title destination smaller than 64 | Compile error (`var string` capacity) |
| Shell normal mode cannot promote to realtime | It keeps pumping under the normal budget (the stand-in for a screenshot window) |
| Full-screen `clear` during the shell | Out of policy: the shell does not clear; apps that do erase the bar's pixels (D14/D8) |

## Alternatives considered

| Alternative | Why not |
|-------------|---------|
| Keep one full-screen console and have the desktop use a "console area" | Scrolling would still move a fixed region over the desktop, and applications would still share one cursor |
| Give the console one window for both kinds of text | Loses the separation between application output and kernel diagnostics, and mixes two clock-rate producers into one scroll |
| Let `window::repair` call the console repaint synchronously | The repair holds `WINDOW`; a console callback would take `CONSOLE` and then `WINDOW` again (self-deadlock) or reverse the lock order (`WINDOW → CONSOLE`), which ADR 0025 rejects |
| Keep a per-window canvas for the text windows | Contradicts ADR 0020's one-shadow model; the grid is already the retained state, so repainting is cheap |
| Block the shell on launched apps (`core.run`) | A launched application would freeze the shell and therefore the menu bar/event loop |
| Dropdown menus now | They need pull-down tracking, overlay save/restore and keyboard equivalents; M2.4.3 owns them. Direct items keep M2.3.4 small and testable |
| A global menu-bar work area in the window service | A policy primitive the service does not need yet; the overlap is documented instead |
| Enumerate windows by count + `id_at` + `title` | Three round trips instead of one, and the title still needs a destination capacity; one call with a fixed 64-byte cap is smaller |
| Return a title in a nested buffer with a runtime capacity | The language has no runtime-capacity parameter; the compiler-checked 64-byte destination is the existing pattern (`window_open`'s title cap is the same constant) |
| Make owner-0 windows invisible to focus and hit-testing | Kernel output windows would then never raise by click; making them ordinary windows keeps one model and the sample baseline is adjusted instead |

## Open questions

| # | Question | Trigger / owner |
|---|----------|-----------------|
| 1 | Work area: should the window service clamp every open below the menu bar? | When a second shell/app cannot avoid the bar (M2.3.5/M2.4) |
| 2 | Kernel windows as a general logging target (level, ring, scrollback) | When the log window needs history beyond the visible grid |
| 3 | Menu-bar ownership: the shell app draws it; a second shell instance would draw a second bar | **Resolved for the boot path (M2.5.6):** the desktop's `Terminal` item raises the live shell instead of spawning a second one. A manually launched second `run desktop_shell` still draws a second bar; that remains a documented M2 limitation. |
| 4 | Per-window clip/ownership for kernel drawing (today the console passes an explicit clip per cell and does not mutate the global one) | If a kernel producer other than the console draws into a window |

## Changelog

| Date | Change |
|------|--------|
| 2026-11-16 | Initial accepted version: kernel-owned terminal and system-log windows, per-window grids with pull-based expose repair, the owner-0 rule, the desktop shell and its direct menu items, read-only `window_info` (builtin 40), the lock order and the proof plan. |
| 2026-11-16 | M2.3.4 implemented: `console::init` opens `Terminal` and `System Log` (owner `0`) after the desktop fill; grids and per-window scroll; `console::route_self_check` (`console: terminal ok`, `console: log ok`, `console: route ok`); `console::pump` pulls owner-0 exposes in `poll_input`; explicit-clip `gfx::draw_cell`/`present_within` (the console never narrows the global clip); `window::open` admits owner `0`, `window::info`/`visible_content_parts`; builtin 40 with compiler capacity and interpreter destination checks; `samples/apps/desktop_shell` check/normal modes; the bar is presented after it is drawn; `events`/`windows`/`windows_events` record the boot focus baseline; `scripts/desktop_screenshot_qmp.py` captures `/home/sinan/Desktop/igis-desktop.png` (1024×768). M2.3.5 remains the two-app/focus/drag/expose proof. |
| 2026-11-17 | D3 made race-free: `window::generation()` (kernel-internal) is bumped by every visible mutation and read by `paint_cell`/`repaint` before computing visible parts and after painting; a changed generation retries the cell/part with fresh parts (bounded `PAINT_RETRIES`). M2.3.5's two-application proof deterministically reproduced the old stale-parts overwrite (a log-grid repaint computed before a window moved above it painted that window's area afterwards); the retry keeps the covered window's pixels. `samples/apps/windows_proof` and `scripts/windows_proof_screenshot_qmp.py` added. |
| 2026-11-17 | D18 added by M2.5.5 (spec 0014, ADR 0028): the terminal half of the console can be attached to a task-owned window; the first attach releases the owner-0 boot Terminal, exposes are pulled by id (terminal) and owner 0 (log), and a vanished window detaches to serial-only. The System Log, D3's explicit clip/generation retry and D17's lock order are unchanged. |
| 2026-11-17 | M2.5.6 boot to the desktop: `entrypoint` (the `desktop_shell` source staged under that file name) is the boot entry, `shell` the fallback; the `Terminal` menu item spawns `shell` with `core.spawn` and later clicks raise the live window; the desktop never attaches the terminal; graphics-disabled boot spawns the text shell. D19 records the permanent-queue-consumer limitation and the `--shell` image used for the event-injecting regression suite. `scripts/boot_desktop_qmp.py` proves the boot menu click and captures `igis-boot-desktop.png`. |
| 2026-11-17 | M2.6.1 (spec 0015, ADR 0030): the desktop shell's menu-bar labels and clock use the proportional font, and the service title bar does too; the item hit ranges are derived from the measured label widths. D3 is strengthened: `console::paint_cell`/`repaint` now call `window::paint_visible_parts`, which holds the window table lock across the paint, replacing the bounded generation retry (a cell painted with stale parts could still overwrite a newly opened window's chrome once; `desktop_shell check` reproduced it on `-smp 4`). The generation still follows a moved terminal on the next pump. |
| 2026-11-17 | M2.6.2 (spec 0016, ADR 0031): D3's per-cell present is replaced by one present per byte run. `Terminal` keeps a bounded dirty rectangle; `paint_cell` draws and marks it, `write_bytes`/`_print` flush after the run, the input echo defers to one `flush_pending` at the end of `poll_input` (after `pump`), and `repaint` clears the box and presents each visible part once. The flush re-reads the visible parts under `WINDOW` and copies only the dirty rectangle from the current shadow. The console byte stream, line discipline, lock order (`CONSOLE → WINDOW → SCREEN`) and every grid/pixel proof are unchanged. |
