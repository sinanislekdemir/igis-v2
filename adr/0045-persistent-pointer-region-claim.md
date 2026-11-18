# 0045: A persistent pointer-region claim protects the menu bar's opening click

- **Status:** Accepted
- **Date:** 2026-11-18
- **Deciders:** igis-v2 kernel/UI
- **Related specs:** 0011 (W27 added), 0010 (routing exception), 0012 (D20), 0003 (`syslib.region_claim`/`region_release`), 0008 (builtins 60/61)

## Context

The desktop's menu bar is **application-drawn**: the shell writes the bar into the
one shadow screen and presents it. It has no window, so the window service knows
nothing about it. Every pointer-down is a `window = 0` record in the one
machine-wide event queue (spec 0010, I19/I31), and routing happens only when a
task polls: a press is hit-tested against the topmost window and raises it
(spec 0011, W16).

M8.3 added a **modal popup input grab** (W26, ADR 0044) for pull-down menus, but
a grab starts only *after* a menu opens. The click that opens the menu is still
an ordinary `window = 0` press, and every application blocked in `next_event`
wakes with it. With two event consumers the queue can hand that press to the
wrong one. Reproduced on the boot desktop: launch `File Browser` from `Apps`,
then click the `Terminal` bar item. The browser consumes the press (or splits
the move/down/up sequence) and `desktop_shell: launched shell task` never
appears. The user-visible symptom is a desktop that stops responding to its own
menu bar while an application is open.

The popup grab cannot be extended to cover this click: its whole rule is "only
the owner receives queued input while the grab is active". Taking a permanent
grab would starve every other application's event loop for the desktop's whole
lifetime. The press must instead be **reserved for the menu-bar owner** while
all other routing stays unchanged.

## Decision

We will add one machine-wide **persistent pointer-region claim**, owned by a
task and exposed as two `syslib` host builtins:

```text
syslib.region_claim(x: int, y: int, w: int, h: int) -> Status   // builtin 60
syslib.region_release() -> Status                                // builtin 61
```

- **State.** One `Region { owner, rect }` lives in the existing window table
  behind the same lock, beside the modal `Grab`. `region_claim` is `Ok` when the
  claim is free or already the caller's (the rectangle is then updated), `Full`
  for another task, and `Invalid` for a non-positive `w`/`h` or disabled
  graphics. `region_release` is `Ok` for the owner, `NotFound` when none is
  held and `Invalid` for another task. Taking a claim neither takes nor
  releases the modal grab and does not end an in-progress pointer drag.
- **Routing.** `KernelHost::poll_event` serves the caller's own pending
  `Close`/`Expose` first, then applies the modal grab (W26). When no grab is
  active it peeks at the queue head; if it is a `PointerDown` with
  `window = 0` inside the claimed rectangle, a non-owner returns `WouldBlock`
  **without consuming it**, and the owner dequeues it unchanged — no hit-test
  and no raise — so the desktop's coordinate handler receives `window = 0`.
  Moves, releases, keys and presses outside the rectangle take the ordinary
  routing path. The queue is never filtered: one peek at the head decides only
  who may consume the press.
- **Precedence.** While the modal popup grab is active its owner receives every
  queued event unchanged and the region gate is not applied. A menu opened by
  the claimed bar click therefore keeps working exactly as M8.3 specified.
- **Window service.** While a modal grab is active `window::pointer_event`
  returns early as before. Without one, a **new** press whose point lies inside
  the region claim starts no drag and posts no close request: the bar wins over
  a window moved under it. Moves and releases are deliberately not filtered, so
  a drag that began before the pointer crossed the bar still follows it.
- **Readiness.** `input_queue_ready` (used by `input_ready`, the scheduler's
  post-block `Input` re-check and `next_any` source `1`) reports a head-of-queue
  press inside another task's claim as not ready, so a non-owner does not spin
  on an event it can never consume. Readiness stays a snapshot and the
  following `next_event` remains authoritative; a spurious wake is harmless
  (poll returns `WouldBlock` and the task re-blocks).
- **Release wakes.** `region_release` and task termination (`close_task`, hence
  every `Task::reclaim` path) clear the claim and wake every input waiter after
  the window lock is dropped, so a press that was gated for an owner that
  disappears becomes deliverable again.
- **Not an authority boundary.** Any task may take the claim; it is a UI focus
  mechanism like `raise` or the popup grab, not a capability. There is one
  claim machine-wide and no timeout.

The desktop claims `(0, 0, screen_w, MenuHeight)` at startup in both normal and
check mode and releases it on exit. A failed claim (`Full`, or graphics
disabled) prints a line and leaves the shell running; on an image whose boot
desktop already owns the bar, `run desktop_shell check` observes `Full` and
must not release another task's claim.

No opcode is added. ABI v1 stays at 75; the language surface grows by the two
`syslib` builtins, both host builtins with `Host` defaults of `Invalid`.

## Consequences

### Positive

- The menu bar keeps working while any number of applications are blocked in
  `next_event`; the `Terminal` item after launching `File Browser` is the
  regression proof.
- Routing is still positional for everything else: the claim touches only a
  `PointerDown` with `window = 0` inside one rectangle, and only at the queue
  head.
- The press is not consumed by the service for the claimant: it arrives as the
  same `window = 0` event the desktop's existing `handle_menu` coordinate code
  already uses.
- The open menu stays modal through the M8.3 grab; the two mechanisms are
  independent states in the same table, with a fixed precedence.
- Termination releases the claim, so a crashed bar owner cannot reserve input
  forever.

### Negative

- One machine-wide claim exists: a second shell cannot claim its own bar while
  the first owns it. This matches the one menu bar of this desktop.
- A press inside the rectangle never starts a drag or posts a close request,
  even when it lands on a window that was moved under the bar. That is the
  intended priority, but it means a window can be partly un-draggable while a
  claim covers it.
- A non-owner may still be woken by a claimed press (the readiness probe is a
  snapshot), observe `WouldBlock` and re-block; that costs a scheduling round
  trip but no event and no busy loop.
- There is no timeout: a task that claims the bar and never releases it keeps
  every other task from consuming presses inside that rectangle until it exits.

### Neutral / follow-up work

- `window::region_self_check` proves the pure rules (claim, `Full`, update,
  invalid extent, half-open rectangle hit, coexistence with the grab,
  owner-only release and termination release); boot prints `window: region ok`.
- `scripts/apps_menu_qmp.py` extends the M8.3 proof: launch `File Browser`,
  open and dismiss a second Apps menu while it is live, then click `Terminal`
  and require `terminal: ok`. It fails on M8.3 on `-smp 4` and passes on M8.4.
- A future multi-bar, toolbar or dock would need either several claims or a
  claim stack; both are out of scope for M8.4.

## Alternatives considered

| Alternative | Why not |
|-------------|---------|
| Keep a modal popup grab for the whole desktop lifetime | It would hide the queue from every other application forever; the desktop becomes the only event consumer and no client can ever read input |
| Make the menu bar a real window | It would get service chrome, a z-order record and a drawing surface for something the shell already draws; the opening click would then be hit-tested and raised as an ordinary window, changing the bar's ownership and focus behaviour |
| Rely on the realtime class or event-loop focus | Scheduling priority does not determine which blocked task polls a shared queue entry first; on `-smp 4` a normal browser can still win a wake race, and the bug is exactly that race |
| Accept the race and tell users to click the bar twice | The desktop's own menu bar is its primary navigation; a lost press that opens a window or does nothing is a user-visible hang symptom |
| Give the desktop a capability or a special kernel flag | Every task may already draw on the shared screen and raise windows (spec 0001 R14); a claim is the same kind of UI focus mechanism, and a capability model would misrepresent it |
| Filter events by rectangle in the pointer producer | Producers run under the graphics lock and must not take the window lock; the producer also cannot know which task should win |
