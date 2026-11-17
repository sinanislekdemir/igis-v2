# 0014: The terminal window

- **Status:** Accepted
- **Scope:** `abi`, `compiler`, `interpreter`, `kernel` (`console`, `window`, `sched`),
  `langhost`, `samples/apps`; M2.5.5 of the desktop milestone
- **Supersedes:** the owner-`0`-only terminal of spec 0012 D1 (extended, not removed)
- **Superseded by:** none
- **ADRs:** 0025 (kernel text windows), 0028 (one machine-wide terminal attached to a
  task-owned window)
- **Updated:** 2026-11-17

## Context

M2.3.4 gave the console two kernel-owned text windows: a terminal for
`core.write` and the line-discipline echo, and a system log for
`println!`/`print!`. The M2.5.1–M2.5.4 applications are ordinary windowed
clients, while the REPL's terminal is still a kernel window, so the shell and
the desktop do not share one user model: the shell cannot place, move or own
its terminal, and the terminal cannot follow the shell into the desktop.

M2.5.5 makes the existing shell run in a real (task-owned) window without
changing the language's console contract: `core.write` still writes a byte
stream and `core.read_line` still blocks on the staged line discipline. It does
not replace the line discipline with application-side key editing (M2.5.3/M2.6),
add a second terminal, a writable console or a stream-based terminal protocol.

The desktop boot entry stays `shell`; M2.5.6 switches the entry to the desktop.

## Requirements

### Surface

- **T1 (MUST):** The application surface is exactly one new `core` host builtin,
  with the predeclared `Status` result (spec 0003, E1):

  ```text
  core.terminal_attach(window_id: int) -> Status
  ```

  It is builtin **44**. No bytecode opcode is added; the ABI v1 opcode table
  stays at **75** (spec 0008). The kernel host supplies the caller's task id;
  the language has no task id and no window-content drawing primitive.
- **T2 (MUST):** `Ok` binds the machine-wide console terminal grid to the named
  window when the calling task owns it. `NotFound` for an unknown window
  id, `Invalid` for a live foreign window, for disabled graphics or for a
  content rectangle smaller than one 8×8 cell, and `Full` when another live task
  already holds the terminal. A repeated attach by the current owner (same
  window) is `Ok` and changes nothing; the same owner may rebind to another of
  its windows.
- **T3 (MUST):** There is exactly **one** terminal machine-wide, because there is
  one line discipline and one console byte stream. The kernel's boot terminal
  (owner `0`) is released by the first successful task attach: its record and
  grid are dropped and its window is closed through the ordinary owner-0 path.
  The System Log window stays owner `0` and is not affected. A second live task
  is refused with `Full` until the holder's window disappears.
- **T4 (MUST):** On attach the grid is cleared, its cursor returns home and the
  grid dimensions derive from the window's **content** rectangle and the 8×8
  font at scale 1 (bounded by the console's static maximum). The console paints
  only inside that content rectangle and its visible parts, through the existing
  explicit-clip helpers (`gfx::draw_cell`/`present_within`); it never narrows the
  shadow's global clip.
- **T5 (MUST):** `core.write` and the line-discipline echo go to the attached
  terminal window. `println!`/`print!` still go to the System Log. Every byte of
  both is still mirrored to COM1; `core.read_line` and its blocking behaviour are
  unchanged. Text is never echoed to both windows.
- **T6 (MUST):** A terminal whose window was closed (its task terminated, or the
  owner closed it) is **detached** at the next service opportunity. Its bytes
  still reach serial, nothing is drawn into a stale id, and the next successful
  attach works. With no attachment the console is serial-only: the kernel does
  not reopen a boot terminal automatically.
- **T7 (MUST):** A window move is followed: writing checks the service's
  visible-geometry generation and re-reads the content rectangle before painting
  into the old origin; the service generation is also sampled at the service
  opportunity, so a moved terminal repaints in its new position.

### Expose and repair

- **T8 (MUST):** The window service marks a whole-content `Expose` on every live
  window whose frame intersects damage, as before. The console pulls its
  attached terminal's expose **by id** (`window::take_expose_id`) and the System
  Log's expose by owner `0`; it never consumes the owner's other windows'
  exposes. A terminal repaint uses the same visible-parts, generation-retry path
  as the kernel terminal (spec 0012 D3).
- **T9 (MUST):** Exposes are pulled at the service opportunity
  (`console::poll_input`), never pushed from the repair. The repair path never
  calls the console.

### Locking

- **T10 (MUST):** The lock order stays `CONSOLE → WINDOW → SCREEN`. Attach takes
  `CONSOLE`, then the window table for the ownership check and the content
  rectangle, and may close the kernel terminal (`WINDOW → SCREEN` repair) while
  `CONSOLE` is held. Pump takes `CONSOLE`, then the table and `gfx`. No path
  takes `WINDOW` and then `CONSOLE`; the window repair never calls the console.
- **T11 (MUST):** The window service's ownership and geometry helpers used by
  attach (`owned_content_rect`, `exists`, `take_expose_id`) are
  kernel-internal: no application binding and no new authority.

### Shell

- **T12 (MUST):** `samples/apps/shell` opens one ordinary window (`560×360`
  content at `(24, 64)`, title `Terminal`) and attaches it before its first
  banner byte, then uses the existing `core.write`/`core.read_line` command loop
  unchanged. If the window cannot be opened or the attach fails, it closes its
  window (when one was opened) and keeps the serial path, printing one status
  line; it does not draw into a window it does not own.
- **T13 (MUST):** `terminal` is a shell command that proves the surface without a
  second line discipline: it reports the attached window id, checks that an
  unknown id is `NotFound`, checks one foreground and one background pixel in the
  window's content rectangle with `syslib.get_pixel`, spawns `terminal_probe`
  (which opens its own window and must be refused with `Full` while the shell
  holds the terminal), and prints `terminal: ok`.
- **T14 (MUST):** `desktop_shell` and the terminal coexist: running
  `desktop_shell` from the attached shell draws the menu bar and the desktop
  window beside the terminal, and the System Log keeps kernel output. The
  desktop shell is not given a shell launcher in M2.5.5 because the boot entry is
  still `shell` and a second live shell would hold no terminal; M2.5.6 adds the
  launcher when the desktop becomes the entry.

## Design

```text
Console
  enabled: bool
  term: Terminal                 // attached terminal (kernel, a task, or none)
  log:  Terminal                 // println!/print!, always owner 0

Terminal
  id, owner: u64                 // 0/0 while unattached
  attached: bool
  cols, rows: usize              // content rectangle / 8, bounded
  col, row: usize                // cursor
  x, y: i64                      // content origin
  fg, bg: u32
  generation: u64                // window-service generation of the cached geometry
  grid: [[u8; 128]; 128]
```

`console::attach(owner, id)` is the kernel side of builtin 44. It validates the
owner and derives the grid from `window::owned_content_rect`, releases the
owner-0 boot terminal on the first task attach, resets the grid and repaints.
`KernelHost::terminal_attach` passes `self.current`, so the language needs no
task id.

`console::pump` detaches a terminal whose id no longer exists, takes that id's
own expose, syncs the cached geometry when the window generation changed, and
then drains owner-0 exposes for the System Log. A terminal move is therefore
followed either immediately (generation check in `feed`) or at the next service
opportunity.

## Failure modes

| Situation | Behaviour |
|-----------|-----------|
| Graphics disabled | Open fails, attach is `Invalid`; the shell keeps the serial path |
| Unknown window id | `NotFound`, terminal unchanged |
| Foreign window | `Invalid`, terminal unchanged |
| Another live task holds the terminal | `Full`; the caller can fall back to serial or close its window |
| Content rectangle below one cell | `Invalid`, terminal unchanged |
| Terminal owner exits or closes its window | Detached at the next pump; bytes reach serial only; a later attach succeeds |
| Window moved | Geometry is re-read (generation) and the grid repaints in the new content |
| Second shell while the boot shell lives | Its attach is `Full`; it prints the fallback line and its output still reaches the one machine-wide terminal |
| Full-screen `syslib.clear` | The attached terminal's pixels are erased; its grid survives and the next expose/output repaints (spec 0012 D8) |

## Alternatives considered

| Alternative | Why not |
|-------------|---------|
| A writable terminal/console API with application-side line editing | This ticket routes the existing stream; editing is M2.5.3/M2.6 and the staged line discipline stays one implementation |
| One terminal per task | There is one line discipline and one console byte stream; per-task routing would need a task id in `Host::console_write` and would break child output |
| Let the window repair call `console::repaint` | It holds `WINDOW`; a callback would invert the lock order (`WINDOW → CONSOLE`), which ADR 0025 rejects |
| Reopen a boot terminal when the owner exits | The next attach should own the window; a kernel-owned fallback would race the new owner and is not needed for the milestone |
| Make the shell draw its own terminal cells | The console already owns the grid, font and clipping; duplicating them would create a second text device |

## Open questions

| # | Question | Trigger / owner |
|---|----------|-----------------|
| 1 | Scrollback/history beyond the visible grid | When the terminal needs a copy buffer |
| 2 | Per-task output routing (a tab per task) | When more than one application must show text at once |
| 3 | Application-side line editing replacing the staged discipline | M2.5.3/M2.6 |

## Changelog

| Date | Change |
|------|--------|
| 2026-11-17 | Initial accepted version: `core.terminal_attach` (builtin 44), the one-terminal rule, owner/Expose routing, stale detach/fallback, geometry following, the lock order, the shell window and the `terminal` proof. `samples/apps/windows_proof` moves its "B old area restored to the terminal" probe from `(400, 500)` to `(400, 435)`: the boot Terminal is now the shell's window at `(24, 64)` (content to `y = 440`), so the old point is desktop dither rather than a terminal cell. |
