# 0028: One machine-wide terminal attached to a task-owned window

- **Status:** Superseded by 0053
- **Date:** 2026-11-17
- **Deciders:** project owner
- **Spec:** 0014 (terminal window), 0005 (host interface), 0012 (console routing)
- **Supersedes:** nothing

## Context

The console has one byte input stream (`core.read_line` and the staged line
discipline) and one byte output stream (`core.write` plus the line echo). Since
M2.3.4 it paints that stream into a kernel-owned text window (owner `0`). M2.5.5
moves the shell into the desktop, so the terminal must be an ordinary
task-owned window that the shell can open, move and lose — but without changing
the console contract or adding per-task output routing.

The options are: (a) one terminal, attachable by one task at a time; (b) one
terminal per task, with `console_write` carrying a task id; (c) a writable
application-side terminal protocol. (b) would break the rule that a child's
output appears in its parent's REPL and needs a task id in the host interface;
(c) replaces the line discipline and belongs to M2.5.3/M2.6.

## Decision

There is exactly **one** machine-wide terminal grid. `core.terminal_attach(id)`
(builtin 44) binds it to a window owned by the calling task; the kernel host
supplies the task id, so the language has no task id. The kernel's boot terminal
(owner `0`) steps aside for the first task attach. A second live task is
`Full`; the same owner may rebind to another of its windows. When the owning
task's window disappears the console detaches at the next service opportunity:
output continues on serial, nothing is drawn into a stale id, and a later attach
succeeds. `println!` keeps the separate System Log window.

The console pulls its attached terminal's `Expose` by window id and the System
Log's by owner `0`, both at the service opportunity, preserving the order
`CONSOLE → WINDOW → SCREEN` and keeping the window repair out of the console.

## Consequences

### Positive

- One line discipline, one output stream and one terminal: the shell, a child app
  launched by `core.run`/`core.spawn` and the boot transcript all share the same
  visible console, as they did before.
- Attach is owner-checked in the kernel with the caller's task id; there is no
  task handle or capability in the language.
- Expose routing by id keeps a task's other windows independent, and the stale
  detach makes task termination safe without a close callback.
- The boot path and `langhost` keep working: a host without a console returns
  `Invalid` and the shell falls back to serial.

### Negative

- A second shell cannot own a terminal while the first lives; its output still
  reaches the one machine-wide terminal. M2.5.6 (desktop boot) makes the desktop
  launch the first shell, so the limitation is transitional.
- When the owner exits, the console does not reopen a kernel terminal
  automatically; the desktop boot must keep a terminal owner or accept the
  serial-only fallback until the next attach.
- Terminal geometry is cached and followed by a generation check rather than
  pushed by a move notification; a move is visible at the next byte or pump.
