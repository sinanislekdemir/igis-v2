# 0043: A close-box press is a close request, never a task kill

- **Status:** Accepted
- **Date:** 2026-11-18
- **Deciders:** igis-v2 kernel/UI
- **Related specs:** 0011 (W23 rewritten, W25 added), 0010 (I19/I31), 0003 (`EventKind`)
- **Amends:** 0042's close-box clause (ADR 0042 otherwise stands)

## Context

M8.1 (ADR 0042) made the service-drawn close box terminate the window's owner task with
`task::request_kill`. That works for a one-window application: the task's windows vanish
through `Task::reclaim`. It is wrong for any task that owns more than one window, and the
first ordinary case is a **dialog**.

The boot desktop (`desktop_shell`, the entrypoint task) owns its `Desktop` window and a
modal `About igis-v2` window. A click in About's close box killed the desktop task: the
entrypoint exited, the scheduler printed its idle summary and the system halted. The
captured transcript was:

```text
sched: task 3 blocked on input
task 3: exit Killed after 85 claims (0 yields, realtime, 175 heartbeats)
smp: 3 of 3 APs stopped
scheduler: idle summary
halt: no console input
```

A dialog must close itself, not its application. The same reasoning applies to a
document window with an unsaved buffer, a palette, or any task whose main window should
survive closing an auxiliary one. The service cannot know which window is "the
application" and must not guess with a kill.

The window model already has the right shape for a different answer: `Expose` is a
per-window, owner-only pending notification served by the kernel host before the queue,
and `window_close` is an owner-only call. A close box can use the same delivery route.

## Decision

We will make a close-box press post a **close request** for that window and never kill the
owning task. The owner receives the predeclared `EventKind.Close`, decides what to close,
and the window stays live until its owner calls `window_close`.

- **Per-window flag, not a queue entry.** `Table::press` sets `close_pending` on the
  pressed task-owned window and returns `PressAction::Close`. The press is still consumed
  (W24) and the pointer transaction still runs under `POINTER_ORDER`. Owner `0` (kernel)
  close boxes are inert.
- **Owner-only, topmost-first delivery.** `window::take_close(owner)` returns and clears
  the topmost pending request of that owner; `has_close(owner)` mirrors `has_expose`.
  A foreign task cannot see or take it because the table is searched by owner.
- **Served before `Expose` and before the queue.** `KernelHost::poll_event` writes
  `{ kind: Close, x: 0, y: 0, code: 0, window: id }`. An injected non-zero `window` still
  passes through; a queued `Close` with `window = 0` is not re-addressed.
- **Readiness and wake.** A pending close counts as input readiness for the owner:
  `input_ready`, the scheduler's post-block `Input` re-check and `next_any` source `1`
  include `has_close(owner)`, and the pointer hook wakes `WaitFor::Input` after dropping
  the window lock. A genuinely blocked owner therefore observes the request.
- **No timeout, no automatic kill.** An ignored request leaves the window open with its
  last content, exactly like an ignored `Expose` (spec 0011, W18). The owner's task
  lifetime remains the only service-driven removal path, plus the owner's own
  `window_close`.
- **Application policy is the application's.** `desktop_shell` closes only About on its
  request and keeps running; its own `Desktop` window request exits the desktop. The
  user-facing apps (`calculator`, `minesweeper`, `editor`, `browser`, `clock_window`,
  `shell`, `widgets`) close their own window and exit. Proof/demo windows may ignore the
  request.

The language surface grows by exactly one predeclared enum variant, `EventKind.Close`
(ordinal 6). `Event`'s 40-byte layout is unchanged. No opcode, builtin, `Host` method,
`syslib` primitive or `stdlib` helper is added; ABI v1 stays at 75 opcodes.

## Consequences

### Positive

- A dialog, palette or secondary window closes without ending its application. The About
  freeze is impossible through the close box.
- The owner chooses whether to save, discard or refuse; a close can be a no-op.
- Delivery reuses the `Expose` pattern (owner flag, poll-time routing, readiness, wake), so
  there is one notification mechanism to reason about.
- The service still consumes the press, so a client sees no `PointerDown` for its close
  box and cannot accidentally treat it as a content click.

### Negative

- A task that ignores the request leaves a window the user asked to close; only task
  termination or the owner's `window_close` removes it. There is no timeout and no
  escalation.
- The owner must handle a new event kind in its loop; an application compiled before this
  change sees `Close` as an unrecognised kind and (in practice) ignores it.
- A close request is not a "save then close" protocol by itself; the application owns any
  save, confirmation or cancellation flow.

### Neutral / follow-up work

- `window::close_self_check` proves the pure flag/take/owner-0/cleanup rules and boot
  prints `window: close ok`.
- `samples/apps/base/windows_drag` proves the same-task dialog pattern and a victim task
  that closes itself and exits `Ok`; `desktop_shell check` injects a real press in the
  About close box, requires exactly one `Close` event, and reopens About to prove the
  desktop survived. `scripts/about_close_qmp.py` reproduces the reported boot-desktop
  scenario and fails on the idle-summary/halt lines.
- Save-on-close, close confirmation and resize remain out of scope.
- ADR 0042 stays accepted; only its close-box clause is superseded by this decision.

## Alternatives considered

| Alternative | Why not |
|-------------|---------|
| Keep kill-on-close, add a per-window "is a dialog" flag | The service would need a new window class and still guess which window is the application; a task with a document and a dialog, or two independent windows, cannot be expressed |
| Keep kill, but let the owner veto it with a "busy" flag | A veto needs a timeout or the click appears broken; the owner still cannot close *only* the dialog |
| Service closes the window directly, no event | The owner is not told; it cannot save, release per-window state, or keep its dialog mode consistent, and the next frame can write into a gone window |
| Kill after a timeout if the owner ignores the request | Arbitrary deadline, data loss without consent, and indistinguishable from an unresponsive client |
| Remove the close box (or make it visual only) | A drawn control that does nothing is worse than an inert kernel close box and leaves the affordance unimplemented |
| A general callback / per-window handler registration | The language has no callbacks or objects; an event kind matches the existing `Expose`/`next_event` model |
| Make `Close` a normal queue entry | A queue entry is machine-wide and address-free; a foreign task could poll it, and the owner's blocked wait would need a different readiness rule. The per-window owner flag gives isolation for free |
| Deliver `Close` but do not count it as readiness | A task blocked in `next_event` would never wake and the window would stay forever despite the request |

## Amendment to ADR 0042

ADR 0042's decision text is unchanged. Its close-box clause ("requests termination of the
owner task through `task::request_kill`") is superseded by this ADR; the drag design,
`POINTER_ORDER` transaction, consumed-press rule and lock order all stand. ADR 0042's
header status is annotated `Accepted (close semantics amended by 0043)`.
