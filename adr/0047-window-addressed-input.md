# 0047: Deliver queued input to the owner of its window target

- **Status:** Accepted
- **Date:** 2026-11-18
- **Deciders:** igis-v2 kernel/UI
- **Related specs:** 0010 (I31 amended, I34 added), 0011 (W29 added), 0012 (D19), 0016 (input measurement), 0024 (window records), 0044 (modal popup grab), 0045 (pointer-region claim), 0046 (window move snapshot)

## Context

The desktop has one machine-wide event queue (spec 0010, I19). Producers write
`window = 0`; the kernel host routes a queued record when it is polled (spec 0011,
W16): a pointer-down is hit-tested, raises and focuses the topmost window and names
it; moves and keys name the focused window. Routing was always correct. Delivery was
not: the queue is a FIFO with no owner check, so the **first task that polls** gets
the next record, whatever window it names.

That was acceptable while the desktop was a set of windows that each polled in turn.
M2.5.6 made the boot desktop a permanent consumer: `desktop_shell` blocks in
`next_event` for its whole life, wakes on every insertion and drains the queue. M8.3's
modal popup grab (W26) protects an open pull-down, and M8.4's persistent pointer-region
claim (W27) protects the 22-pixel menu bar. Ordinary application content and keys have
no such reservation.

A user reported the File Browser not reacting to clicks or keys while the boot desktop
ran; the serial showed pairs of `sched: task 3 blocked on input` (the desktop) and
`sched: task 4 blocked on input` (the Browser). The desktop woke first, consumed the
Browser-addressed press or key, saw `ev.window` naming a window it did not own, and
ignored it. Every application launched from the desktop was unreliable under load, and
the failure depended on which core won the wake. A second report said closing the
About panel could end the desktop (`task 3: exit Ok`, the own-window `Close` path).
The current `scripts/about_close_qmp.py` passes on the parent commit; the symptom is a
mis-delivery or a stale ISO, not a second close protocol. The protection is a
regression that keeps the Browser live while About is opened and closed, and the same
target rule is what makes that regression meaningful.

The window service already knows every window's owner, the topmost hit, the focus and
the claim. The queue record already carries a `window` field that the router fills.
What was missing is a rule that uses those facts **before** the record is consumed.

## Decision

We will deliver a queued event only to the task that owns its window target, and leave
an untargeted event to the ordinary first-poller competition.

### One target rule (W29)

`window::input_target(event) -> Option<u64>` is the single kernel-internal rule:

- an addressed event (`window != 0`) targets that window's owner; a vanished window
  or a kernel window (owner `0`) has no task target;
- a window-less `PointerDown` targets the persistent region claimant at the point if
  one covers it (the M8.4 bar rule), else the topmost window's owner; a hit on an
  owner-`0` kernel window, the desktop or no window has no target;
- a window-less `PointerMove`/`PointerUp`/`KeyDown`/`KeyUp` targets the focused
  window's owner; no focus or a kernel window has no target;
- a window-less `Expose`/`Close` has no target; the real ones are per-window pending
  flags served before the queue (W12/W25).

`None` means "no window target": the event keeps the machine-wide competition of
spec 0010 I31. The modal popup grab is checked first and keeps its stronger rule
(W26): while it is held, its owner receives every queued event unchanged and every
other task gets `WouldBlock`, whatever the focus is. A press inside a persistent
region claim is consumed by its owner unchanged (`window = 0`, no hit-test, no raise),
so the app-drawn menu bar keeps working (W27).

### Consumption and readiness

`KernelHost::poll_event` peeks the queue head without consuming it (`event::peek`),
computes the target, and returns `WouldBlock` when the target is another task. The
target owner (or any task, for an untargeted event) consumes the record and then takes
the ordinary routing path. `task::input_queue_ready` uses the same target for
`input_ready`, the scheduler's post-block `Input` re-check and `next_any` source `1`,
so a non-owner is not woken to spin on a record it cannot consume. The peek releases
the queue lock before the window lock is taken; the two never nest.

### FIFO and liveness

The queue is still one FIFO. A record targeted at an owner that never polls sits at
the head and blocks later records, exactly as W18 already accepts for a client that
never repaints. `Close` and `Expose` are not queue records, so they still reach their
owners behind a blocked head. A targeted move or press is not filtered out or copied
to another task; it waits for its owner or dies with the owner's window (the target
then becomes `None`/the fallback focus). This is an input-targeting rule, not an
authority boundary and not an allocation.

### Test apps own the focus

Window-less input now belongs to the focused window, so a proof app that injects keys
and pointer moves and reads them must own the focus. `samples/apps/base/events` and
`samples/apps/base/multiplex` open a small window, drain its owner `Expose`, and close
it at the end. That is the same contract a real application follows, not a kernel
exception. Their injected records stay `window = 0` for the dispatch checks; the
routing still names the focus in the returned record.

## Consequences

### Positive

- A desktop-launched application receives its own clicks and keys regardless of which
  task wakes first; the user-reported Browser failure is fixed without disabling the
  desktop or serialising every event through it.
- Routing and delivery now agree: the record is consumed by the task the router will
  name, so a task never dequeues an event for a window it does not own.
- Readiness and consumption use one function, so a non-owner does not spin on a
  head-of-queue record; the existing grab/region wakes are unchanged.
- Pending `Close`/`Expose` stay per-window flags: closing About with the Browser live
  still delivers to the desktop alone.

### Negative

- A targeted record can head-of-line block later records if its owner never polls
  (W18 liveness). The queue cannot skip a record without either per-target queues or
  an out-of-order delivery rule.
- Window-less key/move input is no longer readable by a task that owns no window. Test
  apps and future daemons must own a window or use console bytes; the two proof apps
  were adapted.
- The desktop no longer receives an application's key events, so it cannot implement
  global shortcuts over a focused app by reading the queue; it must use the existing
  focus-scoped/modal-aware rule or a future explicit reservation.

### Neutral / follow-up work

- No opcode, builtin, `Host` method, `stdlib` helper, `Event` layout or ABI change. The
  rule is kernel-internal; `window: target ok` is the boot self-check.
- Per-task queues and a forwarding window manager remain possible future designs if
  the head-of-line tradeoff ever matters; see the alternatives.
- The visual golden is unaffected because `visual` opens a screen-sized window and
  injects no input.

## Alternatives considered

| Alternative | Why not |
|-------------|---------|
| Per-task event queues (one queue per window/task) | Needs allocation or a fixed per-task table, duplicates the address-free producer path, and complicates ordering between a task's pending `Close`/`Expose` and queued records. The one FIFO is already the documented model (I31). |
| A user-space window manager that forwards events | There is no window manager task: the service is a kernel `syslib` module (ADR 0024/0029). Adding a forwarding task/protocol would add a hop, an authority boundary and a liveness dependency on that task. |
| Raise/focus the target on every event | Fixes only pointer-down; moves and keys would still be consumed by the first poller, and a background task's press could still be stolen. |
| Accept the race and document it | It makes every desktop-launched app unreliable, which is the user-reported defect. The M8.3/M8.4 grabs already show that per-input reservations are the project's direction. |
| Filter the queue at push time (stamp a task id in the producer) | Producers run under the graphics/console locks and must not take the window lock (graphics → window would invert the repair's window → graphics). Routing must stay at poll time (W16). |
