# 0054: Minimize is a service-owned window state restored through the window list

- **Status:** Accepted
- **Date:** 2026-11-20
- **Deciders:** project owner
- **Spec:** 0011 (windows, W30), 0012 (desktop window list)
- **Supersedes:** nothing

## Context

Every title bar had a close box only. The System Log is a permanent kernel
window and can cover the desktop or accept pointer focus; the user asked for a
hide/minimize button and a way to stop the log's repaint from overdrawing an
open Apps menu.

A minimized window must still exist: its task, content, geometry and title are
unchanged, and there must be a discoverable way to restore it. There is no
taskbar and no per-task client clip.

## Decision

The window service draws a second, 11×11 box at the right end of every title
bar. A press on it is consumed, sets `minimized`, moves the window to the bottom
of the z-order, moves focus to the topmost visible window and erases its frame.
Minimized windows are skipped by hit-testing, visible-part subtraction, chrome
painting and expose marking, but remain in `window_info` at the end of the
topmost-first enumeration.

`window_raise` restores a minimized window: it clears the flag, moves the
window to the top, focuses it and marks its content `Expose`. The desktop's
`Windows` pull-down lists every live window in topmost-first order and marks a
minimized row with `min`; selecting a row calls `window_raise`. The `File`
Window List is clickable for the same purpose and marks minimized rows with `*`.
Owner-`0` kernel windows may be minimized.

The System Log writer keeps updating its grid and the serial mirror while the
log is minimized or while an app-drawn pull-down holds the modal grab, but it
does not paint. `console::pump` repaints it once the menu closes or the window
is restored, so kernel output never draws over the menu.

## Rejected alternatives

- **Close the System Log.** Kernel diagnostics would lose their window and there
  would be no owner to reopen it.
- **A taskbar or dock.** A new desktop surface and layout model for one button;
  the existing window enumeration plus the Windows pull-down is enough.
- **Off-screen minimize.** The window would still be hit-testable or would need
  saved geometry; the z-order bottom plus a flag is simpler and keeps the
  geometry intact.
- **Client-drawn minimize.** The button belongs to the service chrome, like the
  close box; an application cannot affect another task's window.
- **Blocking kernel output while a menu is open.** The grid and serial stream
  must not lose diagnostics; suppressing only the paint preserves both.

## Consequences

### Positive

- Any window, including the System Log, can be hidden and restored with the same
  pointer action.
- The window list gains a second role without a new window type.
- The log no longer overdraws an open menu, and pointer movement cannot cause a
  repaint storm.

### Negative

- `window_info` includes a hidden window; a client that assumes every listed
  window is visible must call `window_raise` before presenting.
- The service has one more state flag and one more press action.
- The visual golden changes because the chrome has a second box.
