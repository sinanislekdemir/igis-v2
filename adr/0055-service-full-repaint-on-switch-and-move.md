# 0055: A switch or a completed move rebuilds the window stack in the service

- **Status:** Superseded by 0060 (recomposition replaces the erase and repaint machinery)
- **Date:** 2026-11-20
- **Deciders:** project owner
- **Spec:** 0011 (windows, W11/W17/W21/W27/W31)
- **Supersedes:** nothing

## Context

A window's content is not kept by the service (ADR 0020, ADR 0024): the one
shadow holds whatever the owners last drew, and a repair redraws only the
service-owned desktop and chrome. A window that was covered by another has its
shadow pixels overwritten by the covering window; when the covering window moves
away or is lowered, the uncovered pixels are stale until their owner repaints.

The service already marks a whole-content `Expose` on every window whose frame
intersects the damage, so the owner is asked. But the terminal emulator and the
desktop shell's own windows deliberately refused to repaint while they were not
topmost, because a client has no per-window draw clip: painting the whole content
would overwrite a higher window in the shadow. The reported symptom was exactly
that: dragging a window around polluted the windows below, and they stayed
polluted until the user raised them.

The repair also presented the whole erase and chrome damage rectangles. An erase
rectangle can contain another window's content, so the stale shadow pixels were
copied to the framebuffer even when nobody had repainted them.

## Decision

We make a **window switch** and a **completed move** a full-stack repaint owned
by the service (spec 0011, W31):

- A switch is a raise that changes z-order, focus or minimized state, plus
  `open`, `close`, `minimize` and `close_task` when the live stack changes.
- A completed move is a programmatic `window_move` that changed the origin, or
  the release that ends a pointer drag that actually moved. Individual drag
  steps keep the ordinary damage pass; a raise of the already-topmost, focused,
  non-minimized window is not a switch.

The repair replaces the accumulated damage with the whole screen. It restores
the desktop wherever no live window covers it, redraws every live window's
chrome bottom-to-top in its visible parts, and marks every live non-minimized
window for a whole-content `Expose`.

It presents **exactly the pixels it painted**: each restored desktop piece, each
chrome band/border piece, and a topmost move's snapshot destination. It never
presents a window's content rectangle and never a merged bounding box that could
contain client pixels. A client presents its own content through
`window_present`, whose visible-parts clip already keeps a lower-window repaint
from covering a higher one. This is the safety invariant that replaces the
missing per-window draw clip.

The rectangle of a persistent pointer-region claim (the desktop menu bar) is
excluded from the desktop restore: the claimant owns those pixels, so the
application-drawn bar survives a switch or move. Windows drawn over the claim
still cover it normally.

The terminal emulator and the desktop shell's `Desktop`, Window List and About
windows now repaint on an owner `Expose` unconditionally; the topmost guard is
removed. Other windowed apps already did.

## Rejected alternatives

- **A per-window client draw clip now.** It is the complete long-term fix, but
  it needs per-task clip state in `gfx`, a new `syslib` scope, and a change in
  every drawing path. The service-side full repaint fixes the reported symptom
  without an ABI or client-language change.
- **A per-window backing store.** That is the canvas rejected by ADR 0024, with
  a full window image per task and a second source of truth to keep coherent.
- **Full repaint on every drag step.** It multiplies a whole-screen dither and
  every chrome redraw by the pointer report rate. The move ends with the full
  stack redraw; during the drag the ordinary damage pass follows the frame.
- **Present the damage union as before.** An erase rectangle can include a
  lower window's content, so its stale shadow pixels are copied to the screen.
  Presenting only service-painted pixels is what makes a fresh client repaint
  the only writer of that content.
- **Protect all app-drawn pixels outside windows.** There is no general
  desktop-ownership model; the persistent region claim is the one declared
  reservation and it is enough for the menu bar.

## Consequences

### Positive

- A moved or lowered window no longer leaves the pixels it uncovered stale
  until the user raises the affected window; the service asks every window to
  repaint and rebuilds the desktop and chrome underneath.
- The service can no longer copy a stale or another window's shadow content to
  the framebuffer; content presentation is owner-only in practice, not just by
  convention.
- The app-drawn menu bar survives switches and moves.
- No opcode, builtin, `Host` method or `stdlib` helper changes.

### Negative

- A switch or a completed move performs a whole-screen desktop dither, every
  window's chrome and an expose round-trip. It is deliberately not on the drag
  step path.
- A lower window that repaints its whole content still overwrites a higher
  window's shadow pixels; it does not touch the framebuffer, but a later
  topmost snapshot copy can carry them until the higher owner repaints. The
  complete fix remains the deferred per-window draw clip.
- A client that ignores the `Expose` keeps stale content (W18); the service
  never blocks or closes it.
- Full requests are recorded by `open`/`close`/`minimize`/`close_task` too, so
  those paths now pay the full-screen pass. They are user-visible composition
  changes, not hot paths.

### Neutral / follow-up work

- A partial-damage `Expose` would let the full pass repaint content only where
  it changed; M2's whole-content expose is kept.
- A per-window draw clip would let clients repaint safely below another window
  and remove the snapshot-contamination caveat.
