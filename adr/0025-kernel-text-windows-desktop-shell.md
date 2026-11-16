# 0025: Kernel text windows and an application-owned desktop shell

- **Status:** Accepted
- **Date:** 2026-11-16
- **Deciders:** project owner
- **Spec:** 0012 (desktop shell), 0009 (graphics), 0011 (windows)
- **Supersedes:** nothing (narrows spec 0009 G14 for the console)

## Context

The M2.1 console is a full-screen text device. It paints glyphs at screen coordinates and
scrolls the whole shadow, so any `core.write` or kernel `println!` after a desktop has
composed scrolls every window (recorded at the end of M2.3.2). M2.3.4 must make text
output coexist with the desktop and put the first shell on it.

The window service already has the pieces: a shadow, one z-order, a service-drawn chrome,
per-window `Expose`, and an owner task id per window. What it does not have is a kernel
producer that owns windows and repaints them.

## Decision

1. **The console becomes two kernel-owned text windows.** A terminal window receives
   application output (`core.write`) and the staged line-discipline echo; a system-log
   window receives `println!`/`print!`. Each keeps its own grid, cursor and per-window
   scroll, so text never scrolls the desktop or another window. Serial mirroring is
   unchanged for both.
2. **Owner id `0` means the kernel.** Task ids start at `1`, so no task can collide with
   a kernel window. Kernel windows are ordinary windows in every other respect: focus,
   z-order, hit-testing, `Expose` and `window_present`. The kernel calls the same public
   `window::*` functions. This avoids a second window table and a second composition
   path.
3. **Expose repair for kernel windows is pulled at the service opportunity, not pushed
   from the service.** `console::pump` drains `window::take_expose(0)` and repaints the
   matching grid after `console::poll_input`. The repair holds the window lock; calling
   the console from it would take `CONSOLE` while holding `WINDOW`, reversing the chosen
   order `CONSOLE → WINDOW → SCREEN`. The pull also means the console never runs inside
   another task's quantum.
4. **The shadow stays the only image.** A terminal grid is retained state, not a canvas;
   a repaint redraws glyphs into the shadow through the ordinary primitives, clipped to
   the window's visible parts. No per-window pixel buffer is introduced (ADR 0020 still
   holds).
5. **The desktop shell is an application, not the boot entry.** `desktop_shell` is run
   manually until M2.5.6. It draws its own menu bar with direct items (no dropdowns in
   M2.3.4), launches applications with `core.spawn` (never `core.run`, so it never
   blocks on a child), and lists/raises windows through a read-only enumeration.
6. **Window enumeration is one read-only builtin.** `syslib.window_info(index, var id,
   var rect, var title)` returns the topmost-first z-order snapshot. `index = 0` is the
   frontmost window; the title destination is compile-checked at the service's 64-byte
   cap. No write access and no new opcode.
7. **No global work area yet.** The shell places its window below the bar and kernel
   windows start at `y = 26`; a window opened at `y = 0` may cover the bar, and the shell
   repaints the bar after its next event. A work-area clamp is a later service decision,
   not an M2.3.4 invention.

## Consequences

- Text output no longer disturbs the desktop: the fix for the M2.3.2 limitation.
- A full-screen `syslib.clear` still erases the console windows' pixels in the shared
  shadow. Their grids survive and the next `Expose` or output repaints them. This is the
  accepted cost of one shadow and one clip.
- The boot log lands in the system-log window from the first line; headless runs still
  read the same lines on serial.
- The window service's focus and hit-testing now have pre-existing owner-0 windows at
  boot, so the three event/window samples that assumed an empty baseline record the
  initial focus and compare against it.
- Kernel text drawing takes the console lock, then the window lock, then the graphics
  lock, always in that order; a new kernel producer must do the same or document a new
  order.

## Alternatives considered

| Alternative | Why not |
|-------------|---------|
| Keep a full-screen console and give the desktop a reserved text band | Scrolling still moves composed windows, and application output and kernel diagnostics still share one cursor and one history |
| One window for both console streams | Loses the separation that makes a "system log" useful; interleaves two producers at very different rates |
| Push repaint from `window::repair` into the console | Reverses the lock order or self-deadlocks; the repair is deliberately synchronous under the window lock |
| A per-window canvas for the terminal | A second image to keep consistent, and the grid already retains the text; the shadow must remain the single source of truth |
| Owner-0 windows hidden from focus/hit-testing | A click could never raise the terminal; a special case in the routing and focus fallback would be larger than adjusting three samples |
| Boot directly into the shell now | M2.5.6 owns the transition (entrypoint becomes the desktop); M2.3.4 keeps the text shell and `run desktop_shell` |
| Dropdown menus in the shell | Pull-down tracking, overlay restore and keyboard equivalents are M2.4.3; direct items prove the launch and window-list paths without an overlay |
| A nominal `WindowInfo` record returned by value | The language has no return of aggregate values; the existing `var` out-parameter pattern is the only shape available |
