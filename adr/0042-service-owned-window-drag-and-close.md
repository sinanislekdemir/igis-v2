# 0042: Window drag and close are service-owned pointer interactions

- **Status:** Accepted
- **Date:** 2026-11-18
- **Deciders:** igis-v2 kernel/UI
- **Related specs:** 0011, 0010, 0009

## Context

The window service has had a complete model since M2.3 — records, z-order, one focus,
ownership, damage and `Expose` — but every mutation is an application call:
`window_move` is owner-only and a pointer press is only routed to a client at poll time.
That leaves two ordinary desktop affordances missing:

- **Drag.** A user cannot move a window by its title bar. M2.3.5 proved a *scripted*
  drag with a sequence of owner `window_move` calls; the M2.3 plan deliberately deferred
  the continuous pointer loop.
- **Close.** A window has a service-drawn close box, but clicking it does nothing. A
  task's window only disappears when the owner closes it or the task terminates, and
  `core.kill` is the only immediate termination path.

Both interactions are fundamentally service-owned. The pointer is machine-wide and a
press can target any task's window; letting a client implement a drag would require it
to receive the press first, call `window_move` repeatedly and still respect the z-order
and clamp rules, which changes the ownership model for no gain. A close box is worse: it
must terminate a task the clicker does not own.

The graphics path also constrains the design. Pointer producers run inside `gfx` under
the graphics lock; a repair takes `window → graphics`. Any window work performed while
the graphics lock is held would invert that order and can deadlock on SMP.

## Decision

We will implement **pointer drag and close-box termination as service-owned interactions
over the existing window table**, with no new opcode, builtin, `Host` method, `syslib`
primitive, `stdlib` helper, `EventKind` or language surface.

- **Hook after the graphics unlock.** `gfx::update_pointer` and
  `gfx::set_pointer_absolute` publish the pointer transition, drop the graphics guard,
  and then call `window::pointer_event(old, new, buttons)`. The hook reads the screen
  size, takes the window lock, runs the pure table methods and repairs under
  `WINDOW → SCREEN`. It drops the table lock before `task::request_kill` or
  `task::wake_input`. Injected `syslib.inject_pointer` uses the same path, so proofs are
  synchronous and deterministic.
- **Title bar starts a drag.** A left-button press in the 16-pixel title band outside
  the close box raises/focuses the window (the existing any-task `raise`) and records
  the grab offset from the frame origin. While button 1 is held, each pointer update
  moves the window to `pointer − offset` through `Table::relocate`, the same clamp and
  damage body as `window_move` (the public call stays owner-only). Release, a second
  press (ignored), or the window disappearing cancels the drag. Owner `0` windows drag
  too.
- **Close box kills the owner.** A press in the 11 × 11 close box calls
  `task::request_kill(owner)` for a task-owned window; the normal reclaim path closes the
  windows and `core.wait` observes `Killed`. Owner-`0` close boxes are inert.
- **The press is consumed.** The pointer producer already queued its `PointerDown`, so a
  handled press removes exactly that one unaddressed event from the bounded queue; the
  client never sees a click for a title-bar or close-box press. Content and desktop
  presses keep routing at poll time.
- **Self-check.** `window::drag_self_check` covers classification, grab offset, clamping,
  release, the ignored second press, the owner-0 exemption and cancellation on close on
  a scratch table; boot prints `window: drag ok`.

## Consequences

### Positive

- The two interactions every windowed system needs now work with no API growth and no
  change to the client contract: a client still opens, draws, reads events and closes.
- The safety and ownership model is unchanged. Drag uses one service-owned move path;
  close reuses `request_kill` + `Task::reclaim` + `close_task`, so focus fallback, damage
  and `Expose` are the existing code.
- The lock order stays the one documented rule (`WINDOW → SCREEN`, never the reverse),
  and the hook cannot hold the window lock across a scheduler operation.
- A real injected pointer event takes the same path as a device, so the proof needs no
  kernel-side test channel.

### Negative

- **Closing loses data.** The close box terminates immediately, exactly like
  `core.kill`; there is no save-on-close handshake and no chance for the client to
  prompt. That is an accepted M8.1 limitation and a deferred idea.
- A close-box press on a kernel window is consumed but does nothing, which can look
  inert to a user; kernel windows have no owner task to terminate.
- Drag ignores the owner check while a drag is active. Ownership still gates the public
  `window_move`, and a drag can only start from the service's own classification, so
  this is not an authority boundary; it is not a general cross-task move primitive.
- While a drag is active, a second press is ignored rather than starting a second
  interaction; multi-button gestures are not modelled.

### Neutral / follow-up work

- Interactive **resize** remains deferred (spec 0011, open question 4); this slice moves
  whole windows only.
- A graceful close-request `EventKind` (client opt-in, save, then close) is a possible
  later slice if data loss becomes a problem.
- The consumed-press mechanism is deliberately minimal: it removes one matching queued
  event. A durable per-event acknowledgment would be needed only if the queue protocol
  grows beyond the current debug/test injection.

## Alternatives considered

| Alternative | Why not |
|-------------|---------|
| Client-side drag via the event stream | The client would need the press first, repeated `window_move` calls and its own clamp/z-order rules; another task's window cannot be moved at all, and the service already owns the geometry and repair |
| Owner-only drag | A window could not be dragged when the pointer is over it while another task owns it; raise is already any-task's, and the pointer is machine-wide |
| A graceful close-request `EventKind` | Adds an event kind, a client handshake and a timeout policy for a first slice; the OS already has immediate termination (`core.kill`) with the same semantics |
| Do not implement close | A drawn close box that does nothing is worse than an inert kernel close box; the task lifetime already defines window lifetime |
| Call the window hook while holding the graphics lock | Repair takes `WINDOW → SCREEN`; calling it under `SCREEN` inverts the order and can deadlock on SMP |
| Let `poll_event` skip consumed presses by remembered coordinates | A lingering coordinate record can mis-consume a later identical press; removing the one queued event at the moment of consumption is exact and bounded |
