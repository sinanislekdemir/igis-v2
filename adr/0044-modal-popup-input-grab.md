# 0044: A modal popup input grab makes an app-drawn menu reachable over windows

- **Status:** Accepted
- **Date:** 2026-11-18
- **Deciders:** igis-v2 kernel/UI
- **Related specs:** 0011 (W26 added), 0010 (routing while grabbed), 0012 (D10/D11/D20), 0003 (`syslib.popup_grab`/`popup_release`), 0008 (builtins 58/59)

## Context

The desktop's menu bar and its pull-downs are **application-drawn**: the shell
writes the bar and the menu into the one shadow screen and presents the
rectangle. They are not windows, so the window service knows nothing about them.
The bar occupies the top 22 rows and the desktop conventionally opens its
windows below it, which is why direct items have worked.

Event routing is positional (spec 0011, W16): a queued `PointerDown` is
hit-tested against the topmost window frame, that window is raised and focused,
and the event is delivered to it. M8.1 added a second consumer of a press: a
press in a 16-pixel title bar starts a service-owned drag and the press is
consumed before the client sees it (W22/W24).

Those two rules make a lower menu row unreachable. The first pull-down
(`System`, M2.6.4) already had this defect for rows below `y = 26`: the kernel
`Terminal` window starts at `y = 26`, so a click on `Restart` or `Halt` raised
the Terminal (or, at `y = 30`, was consumed as a title-bar drag) and the shell
never saw it. The M8.2 harness moved its About row to `y = 24` to work around
it. M8.3's Apps menu has six 16-pixel rows (22..120), so most of it lies over
the kernel's Terminal and System Log windows; without a fix the menu would be
mostly decorative.

The menu needs the same property a modal dialog has: while it is open, input
belongs to it. It cannot become a real window (that would add service chrome,
a z-order record and a second drawing surface for something that is logically
part of the menu bar), and it cannot rely on coordinates (the press never
reaches the app to be compared).

## Decision

We will add one machine-wide **modal popup input grab** owned by a task and
exposed as two `syslib` host builtins:

```text
syslib.popup_grab(x: int, y: int, w: int, h: int) -> Status   // builtin 58
syslib.popup_release() -> Status                              // builtin 59
```

- **State.** One `Grab { owner, rect }` lives in the existing window table
  behind the same lock. `popup_grab` is `Ok` when the grab is free or already
  the caller's (the rectangle is then updated), `Full` for another task, and
  `Invalid` for a non-positive `w`/`h` or disabled graphics. `popup_release` is
  `Ok` for the owner, `NotFound` when none is held and `Invalid` for another
  task. Starting a grab ends any in-progress pointer drag.
- **Pointer service disabled.** While a grab is active `window::pointer_event`
  ignores presses entirely: no title-bar drag starts and no close box posts.
  The press is still published as an ordinary event, because the grab owner
  needs it.
- **Modal routing.** `KernelHost::poll_event` first serves the caller's own
  pending `Close`/`Expose` (unchanged, owner-specific). Then, if a grab is
  active and the caller is not the owner, it returns `WouldBlock` **without
  consuming the queue**. The owner dequeues the event unchanged: producers keep
  `window = 0`, so no hit-test and no raise happen while a menu is open. The
  owner decides inside/outside from its advisory rectangle.
- **Readiness.** A non-owner's input readiness is false while another task holds
  the grab: `input_ready`, the scheduler's post-block `Input` re-check and
  `next_any` source `1` all use the same `window::grab_allows(task)` rule. The
  owner's own expose/close still count.
- **Release wakes.** `popup_release` and task termination (`close_task`) clear
  the grab and wake every input waiter after the window lock is dropped, so
  queued events flow again.
- **Advisory rectangle.** The rectangle is stored only so the owner can apply
  its own policy (the desktop dismisses on a click outside the pull-down). The
  service never uses it to filter events: routing is wholly modal.
- **Not an authority boundary.** Any task may take the grab; it is a UI focus
  mechanism, like `raise`, not a capability. One misbehaving owner can hold
  input until it releases or terminates; that is accepted for this milestone.

The desktop's `Apps` and `System` pull-downs take the grab on open (the
pull-down frame is the advisory rectangle) and release it on every close path.
The `Clock` item is a natural proof: row 4 is at `y = 94`, over the kernel
Terminal content, and clicks there reach the menu.

No opcode is added. ABI v1 stays at 75; the language surface grows by the two
`syslib` builtins, both host builtins with `Host` defaults of `Invalid`.

## Consequences

### Positive

- An app-drawn pull-down is fully usable over other windows, including the rows
  that lie over a title bar; the `System` menu's lower rows are fixed for free.
- Routing stays simple and positional when no menu is open: the grab is one
  flag checked at poll time, with no per-window owner lists or coordinate
  bookkeeping in the service.
- The press that opens and the presses that navigate a menu all arrive as
  ordinary `window = 0` events, so the existing event loop and coordinate
  hit-testing keep working unchanged.
- Release is bounded: task termination releases the grab, so a crashed menu
  owner cannot wedge input forever.

### Negative

- While a menu is open every other task's queued input is blocked (not lost):
  the queue keeps the events, but their readers do not wake. The shell's
  terminal byte stream is independent and keeps working.
- One task can hold the grab indefinitely while it runs; there is no timeout.
  A buggy menu that never releases blocks event delivery to other tasks until
  it exits.
- The advisory rectangle is deliberately not enforced by the service, so a
  buggy owner can keep the grab while claiming a smaller area.
- A second application cannot open its own menu concurrently; there is one
  grab machine-wide. This matches the single menu bar of the desktop.

### Neutral / follow-up work

- `window::grab_self_check` proves the pure rules (owner, `Full`, update,
  invalid extent, owner-only release, termination release) and boot prints
  `window: grab ok`.
- `desktop_shell check` exercises both menus' open/close, the item table, one
  real launch and the outside-click dismissal; `scripts/apps_menu_qmp.py`
  reproduces the boot scenario with a QMP click on `Clock` over the Terminal.
- A submenu or a second simultaneous popup would need either a grab stack or a
  popup window; both are out of scope for M8.3.
- The `System` menu harness's `y = 24` row is harmless with the grab; it is
  left as is.

## Alternatives considered

| Alternative | Why not |
|-------------|---------|
| Make the pull-down a borderless real window | It would get service chrome unless the service grows a borderless/titleless window class, it would enter the z-order and focus model, and the menu bar itself would still be app-drawn. It also changes the client drawing model for a transient overlay |
| Move the menu handling into the window service | The service would have to own menu labels, rows, selection and launch policy, duplicating the M2.4.3 `stdlib` menu model and mixing application policy into the kernel |
| Keep the app-drawn menu and only filter events inside the menu rectangle | The click is routed *before* the app sees it, so the app cannot compare coordinates. The service would need to know the menu rectangle per task, which is the grab by another name, and a title-bar drag would still consume the press first (W22/W24) |
| Special-case the kernel windows or the title bar | Every application window would still intercept the lower rows; the desktop would need a list of "windows a menu may cover", which is all of them |
| Give the desktop a real window for the menu and raise it | Raising a window with no content would still put service chrome around the menu and would need content clipping; and it would not stop another window from being raised above it by a click before the menu's row is handled |
| A capability/authority model for the grab | Every task may already raise windows and draw on the same screen (spec 0001 R14); treating a focus mechanism as a security boundary would be misleading and would add a model this system does not have |

## Amendment note

M8.1 (ADR 0042) and M8.2 (ADR 0043) are unchanged: with no grab active, drag
and close-request behaviour is exactly as those decisions describe. The grab
only suspends the pointer service while a popup is open.
