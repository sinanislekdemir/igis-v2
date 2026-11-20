# 0053: A terminal emulator over kernel console sessions

- **Status:** Accepted
- **Date:** 2026-11-20
- **Deciders:** project owner
- **Spec:** 0021 (terminal emulator), 0005 (host interface)
- **Supersedes:** 0028 (one machine-wide terminal attached to a task-owned window)

## Context

ADR 0028 chose one machine-wide console grid attached to a task-owned window.
That was the smallest way to move the M2 shell into the desktop without
introducing per-task output routing. It cannot support several independent
terminal instances: one grid, one input ring and one attachment mean a second
shell is refused with `Full`, and the kernel stays responsible for terminal
layout, scroll and repaint.

The project owner asked for terminals to be application emulators, and for as
many instances as the kernel can support. The kernel already has the right
primitive for independent byte streams: a bounded FIFO with wake-ups. What it
lacks is a way for one task to own both directions of a console pair while a
child task uses them as its standard byte stream.

## Decision

The terminal is an application, not a kernel console. The kernel provides
**console sessions**: a fixed table of owner-checked pairs of byte rings, one
per task slot, with these properties.

- The owner (the terminal emulator) opens a session, writes keystrokes to the
  input ring and reads screen bytes from the output ring.
- A child spawned by the owner inherits the session. Its `core.write` goes to
  the output ring and its `core.read_input`/source `0` reads the input ring.
  Grandchildren inherit it in turn, so a shell's `core.run` children appear in
  the same emulator.
- The emulator owns the visible grid, cursor and scroll. The kernel stores no
  terminal cells.
- Serial input is routed to the focused window owner's session. Keyboard input
  stays a window-addressed event and is converted to bytes by the emulator.
- The number of sessions is bounded by `MAX_TASKS` and the window table, not by
  a separate terminal count. The rings are small because the emulator has no
  scrollback and drains continuously.

`core.terminal_attach` is retired. `println!`/`print!` keep the separate System
Log window; `core.write` never feeds it.

## Rejected alternatives

- **Multi-slot kernel console.** Extend the existing console from one grid to N
  grids, each with an attached window. This keeps terminal layout in the kernel,
  duplicates the app's drawing and scrolling, and still needs per-task routing.
  It also conflicts with the owner's instruction that the terminal is an
  emulator.
- **One global console, several views.** Several Terminal windows over the same
  grid. Not independent sessions: output interleaves and only one shell can
  own input.
- **Terminal app owns the shell command loop, no child process.** The shell's
  external commands and their output would have no path back into the emulator.
  A session pair gives every descendant the same byte stream.
- **Dynamic allocation for sessions.** ADR 0007 keeps the kernel static. A
  bounded table sized from `MAX_TASKS` is enough; a slot's rings are 4.5 KiB.

## Consequences

### Positive

- Several independent terminals, each with its own shell and cwd, start from
  the Apps menu.
- The kernel stops drawing terminal text; it keeps one System Log grid and
  byte transport.
- The host surface is four stateful builtins plus a compiler-lowered source
  tag; no bytecode opcode is added.
- `langhost` and a bare host keep working through `Invalid`/`false` defaults.

### Negative

- Per-task output routing is now real: every task carries a session id and
  `core.write`/`read_input` consult it.
- Serial input is focus-dependent; a serial harness must focus a terminal
  before typing, as a user would.
- The output ring is lossy under extreme bursts. The terminal has no scrollback
  anyway, and the drop counter makes the policy visible.
- A session is a new resource that task termination must release. `Task::reclaim`
  is the single cleanup point, next to streams and sockets.
