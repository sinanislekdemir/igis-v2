# 0026: Composition, surfaces and coordinate spaces

- **Status:** Accepted; implemented in M16. The local-coordinate and window-local-event clauses (C4's surface-local wording, C8–C10) are **deferred to M17**: M16 translates absolute client coordinates by the surface origin and keeps screen-coordinate events.
- **Scope:** `abi`, `compiler`, `interpreter`, `kernel` (`gfx`, `window`, `console`, `sched`),
  `syslib`, `samples/apps`; display model for M16
- **Supersedes:** the one-shadow drawing model of spec 0009 (G5), the region claim of
  spec 0011 (W27), the move snapshot of spec 0011 (W28), the erase/`covers_from` repair of
  spec 0011 (W11/W21/W31) and the console's explicit-clip path of spec 0012 (D3)
- **Superseded by:** none
- **ADRs:** 0060
- **Updated:** 2026-11-25

## Context

The display has been one shared shadow: applications draw into it, and `present` copies
rectangles to the framebuffer. Everything that goes wrong on screen comes from that one
fact. A window's content exists only as the residue of the client's last drawing pass, so:

- a lower window's repaint overwrites a higher window's pixels in the shared image
  (the overlap artifacts), and the window service can only ask the damaged owner to
  repaint again (`Expose`) and hope it does;
- the cursor is composited into the framebuffer only (spec 0010 I7), so "erasing" the old
  cursor copies shadow pixels back — which are exactly the pixels the overlap bug
  corrupted, producing pointer trails;
- the repair pass must reconstruct "what should be under this rectangle" from damage
  lists, per-window coverage limits (`covers_from`), desktop erases, a topmost move
  snapshot and a full-stack-repaint flag. Each of those is a special case of the same
  question ("what is on this pixel?") that the shared image cannot answer.

Every workaround built so far — the persistent region claim, the app-level
"repaint only the visible strip" discipline, the console's visible-parts machine, the
shell's `repair_menu_area` — is a patch on the missing invariant.

The invariant this spec establishes:

> **The composed screen is a pure function of retained surface state.** Nothing is
> reconstructed by guessing; the service owns every pixel of every surface and paints
> the shadow from those surfaces, bottom to top, on demand.

There is no memory constraint that argues against retained surfaces: a content-sized
canvas at 1024×768 is well under 1 MiB for a typical window, and the frame allocator is
the only limiter. Non-goals: alpha blending, scaling, window resize, damage-independent
damage tracking, hardware cursors, a wallpaper.

## Requirements

### Surfaces

- **C1 (MUST):** There are two kinds of **surface**, both kernel-owned and
  screen-composited: a **window** (spec 0011: frame, chrome, content, z-order, owner,
  window-local coordinates) and a **plane** (C8: a screen rectangle above all windows,
  screen coordinates). Every surface owns exactly one **canvas**, a rectangle of pixels
  `w * h * 4` bytes with no row padding, allocated from the frame allocator and freed
  when the surface is destroyed or its task terminates.
- **C2 (MUST):** Canvas storage is bounded only by the frame allocator and by the
  rectangle's own size (a window's content size or a plane's rectangle, each at most one
  screen area). There is **no machine-wide canvas budget constant**: a failed
  `alloc_contiguous` is the only reason a canvas cannot be created, and the creation call
  then returns `Status.Full` and creates nothing. A half-created surface is never
  observable.
- **C3 (MUST):** A task draws into a surface only inside an open **draw scope**, opened by
  `syslib.window_begin(id)` or `syslib.plane_begin(id)` and closed by the matching
  present call, by the surface's destruction or by the task's termination. There is at
  most one scope per task. The scope is recorded in the surface record, not in per-quantum
  host state, so preemption, a blocking `next_event` or a long frame cannot redirect a
  later draw. Opening a scope is owner-only: `Ok` for the caller's live surface,
  `NotFound` for an unknown id, `Invalid` for another task's surface or disabled
  graphics.
- **C4 (MUST):** Every application drawing primitive requires an open scope. Inside it
  the primitive writes the scope surface's canvas, and coordinates are **surface-local**:
  `(0, 0)` is the canvas top-left. With no open scope a drawing primitive returns
  `Status.Invalid` and writes nothing. A rectangle is clipped to the canvas and never
  faults; an off-surface coordinate is legal and draws nothing. Canvas writes never
  present.
- **C5 (MUST):** `syslib.clear(color)` fills the **scope surface** with `color`. It is
  `Status.Invalid` with no open scope. It is no longer a screen-wide operation.
- **C6 (MUST):** `syslib.get_pixel(x, y)` answers from the **composed shadow** in screen
  coordinates (uncomposited cursor, spec 0010 I7). A client's pixels reach the shadow on
  `window_present`/`plane_present` or on a repair, so a pixel self-check runs after the
  present. Reading a canvas directly is deferred with the local-coordinate model (M17).
- **C7 (MUST):** `syslib.present(x, y, w, h)` is a **recomposite** request: the service
  repaints that screen rectangle from the surface stack and presents it. It is
  idempotent, needs no scope, and is always correct because it recomputes the pixels
  rather than copying a client's guess.

### Coordinate spaces

- **C8 (MUST):** Client drawing uses **absolute screen coordinates**, translated into
  the surface canvas by its current origin; `window_frame_rect` and
  `window_content_rect` report absolute screen rectangles. A client therefore reads the
  content rectangle from the service when it needs its origin (on an `Expose`, W12/W28),
  exactly as before M16. An origin is a value the client caches, so a move followed by a
  stale-cache draw lands offset inside its own canvas; the next `Expose` repaint
  corrects it. The surface-local model (no origin, `(0,0)` at the canvas top-left) is
  deferred to M17.
- **C9 (MUST):** Event coordinates are **screen coordinates** for every event, whatever
  its target (the M16 model, unchanged from before). Window-local coordinates
  (`x`/`y` relative to the content rectangle for a window-addressed event) are deferred
  to M17 with C8; the `Event` layout is unchanged either way (spec 0010 I19).
- **C10 (MUST):** A move, raise, minimize or restore is an origin change in the service
  only: the canvas keeps its pixels and the compositor places it at the new origin, so
  the content follows the frame with no pixel copy. Because client coordinates are
  absolute (C8), the owner MUST re-read `window_content_rect` on an `Expose` before
  drawing or hit-testing; a client that keeps the old origin draws offset by the move.
  M17 removes that discipline together with C8.

### Planes

- **C11 (MUST):** A **plane** is a screen rectangle drawn above every window and below the
  cursor, with its own canvas and screen coordinates. At most **8** planes exist
  machine-wide. `syslib.plane_create(x, y, w, h) -> id` creates one for the caller with a
  zeroed canvas, clamped to the screen, appended above the existing planes
  (`Status.Full` at the limit or when the canvas cannot be allocated, `Invalid` for a
  non-positive or off-screen rectangle or disabled graphics). `syslib.plane_begin(id)` /
  `syslib.plane_present(id)` are the draw scope, owner-only. `syslib.plane_destroy(id)` is
  owner-only, frees the canvas and composites the vacated rectangle, so the windows under
  it reappear immediately. A plane is composited only after its first present (C14).
- **C12 (MUST):** A plane replaces the persistent pointer-region claim (spec 0011 W27,
  ADR 0045): a queued `PointerDown` inside a plane's rectangle is dequeued only by that
  plane's owner (the topmost plane at the point wins), with screen coordinates and
  `window = 0`; other tasks read `WouldBlock` without consuming it. The modal popup grab
  (spec 0011 W26) still takes precedence. Planes are also hit-tested in the pointer
  service: a press inside a plane starts no drag and posts no close request.
- **C13 (MUST):** `syslib.work_area_reserve(height)` reserves a top strip of the screen
  (height `0` clears it) into which window frames are not placed: `open` refuses a frame
  taller than the remaining area (`Invalid`), and `move`, drag clamping and the
  mode-change reflow clamp origins below the strip. It exists because
  a plane always stays above windows, so a title bar dragged under a bar plane would
  otherwise become unreachable. The value is machine-wide and the last caller wins.

### Composition

- **C14 (MUST):** One compositor is the **only** writer of the shadow, and therefore of
  the framebuffer. For the rectangle it is given it paints, bottom to top:
  1. the service-owned desktop pattern (base colour plus the `(X + Y)` dither);
  2. every live non-minimized window, bottom to top: its chrome, then its canvas blitted
     into the content rectangle — each clipped to the window's frame and the rectangle;
  3. every plane, in creation order: its canvas into its rectangle;
  4. the software cursor (into the framebuffer, never the shadow, spec 0010 I7).

  This is the painter's algorithm: a higher window or plane simply overwrites what is
  below it. There is no visible-parts subtraction, no coverage limit and no "presents only
  its own pixels" rule, because the compositor owns every pixel it paints.
- **C15 (MUST):** A surface is composited only after its first present (`canvas_valid`);
  before that the pixels of what lies beneath are left untouched, so a freshly opened
  window shows the desktop rather than uninitialized canvas.
- **C16 (MUST):** Damage is a bounded list of **screen rectangles to recomposite**:
  `open`, `close`, `move`, `raise`, `minimize`, a focus change, a plane's creation and
  destruction, a client present, console output and a mode change record the affected
  rectangles (coalesced, spec 0011 W21). A repair recomposites each coalesced rectangle
  and presents it. There is no desktop erase, no per-window coverage limit, no content
  snapshot and no full-stack-repaint flag: recomposition is the only repair.
- **C17 (MUST):** A runtime display mode change (spec 0022) keeps every canvas (content
  and plane rectangles are unchanged), reallocates the shadow, scales and clamps window
  origins below the work area, clamps planes to the new screen, and recomposites the whole
  screen. The desktop pattern is repainted by the compositor, not by a client.
- **C18 (MUST):** With graphics disabled every surface call is `Status.Invalid`
  (`NotFound` for destroy of an unknown id), `screen_width`/`screen_height` are `0`,
  `get_pixel` is `0` and the kernel keeps its text path. No canvas is allocated.

### API surface

- **C19 (MUST):** The new builtins are **92** `window_begin(id)`, **93**
  `plane_create(x, y, w, h) -> id`, **94** `plane_destroy(id)`, **95** `plane_begin(id)`,
  **96** `plane_present(id)` and **97** `work_area_reserve(height)`. No bytecode opcode is
  added; ABI v1 stays at 75. Every one has a `Host` default that models the disabled
  service, so `langhost` and host tests stay deterministic.
- **C20 (MUST):** Drawing into the shadow without a surface is gone. There is no
  application escape hatch: the desktop pattern is service-owned and every other pixel
  belongs to a window or a plane. A client that wants to paint screen furniture creates a
  plane.

## Design

### Layer stack

```text
cursor            framebuffer only (spec 0010 I7)
planes[0..n)      created order, screen rectangle, own canvas
windows           z-order bottom → top: chrome + content canvas
desktop pattern   service-owned procedural fill
```

Every layer's pixels are retained except the desktop pattern, which is deterministic and
cheap to regenerate. That is why no repair needs to know history.

### Draw path

```text
window_begin(id) / plane_begin(id) ──▶ scope (in the surface record)
syslib.fill_rect(x, y, …)           ──▶ canvas write at local (x, y), clipped
window_present(id) / plane_present  ──▶ scope closes; recomposite the surface rect
```

A client's whole frame lands in its canvas and is presented in one step; nothing it draws
can reach another surface. Preemption cannot split a frame into "half canvas, half
shadow", because there is no shadow path to fall into.

### Lock order

`POINTER_ORDER → WINDOW → CANVAS → SCREEN`, with `CONSOLE` outside `WINDOW` as today
(spec 0012 D17). A client draw resolves its surface under `WINDOW`, writes under `CANVAS`
and never takes `SCREEN`; the compositor holds `WINDOW`, takes `CANVAS` per surface and
`SCREEN` for the shadow. No path takes `SCREEN` and then `WINDOW`, so the order is
acyclic. This also removes today's "one global clip touched by two cores" race: the shared
clip no longer exists, because a client never draws the shadow.

### Retired mechanisms

| Retired | Replaced by |
|---|---|
| `gfx::set_clip`/`reset_clip` and the global clip | the surface canvas plus painter's-order composition (C14) |
| visible-parts computation (`Parts`, `visible_parts`, `paint_visible_parts`) | painting higher layers over lower ones (C14) |
| `Erase { rect, covers_from }` and desktop erases | recomposition of damaged rectangles (C16) |
| the topmost move snapshot (`gfx::copy_rect`, spec 0011 W28) | a window-local canvas that does not move (C10) |
| the full-stack-repaint flag (spec 0011 W31) | the same recomposition, which is always full-stack for its rectangle |
| the persistent pointer-region claim (spec 0011 W27, ADR 0045) | a plane (C11/C12) |
| the shell's `repair_menu_area` and pixel-ownership workarounds | plane destruction recomposites what was underneath |
| the console's explicit-clip paint and geometry generation | the console's own canvas (C14) |
| `syslib.clear` as a screen-wide fill | a scope-local fill (C5) |

### Costs

- One extra copy per present: `w * h` pixels canvas → shadow, then the rectangle
  shadow → framebuffer. A recomposition also repaints the windows it overlaps (painter's
  algorithm) instead of subtracting visible parts.
- Memory is retained state: the shadow plus one content-sized canvas per window and one
  rectangle per plane. That is the price of the invariant, and it is bounded by the frame
  allocator.
- Every windowed application drops its absolute-coordinate arithmetic (local layout) and
  its visible-part-only drawing discipline; that is less code, not more.

## Verification

- Self-checks: `gfx::surface_self_check` (translated surface read-back and clipping),
  `window::canvas_self_check` (alloc/free/reuse, `Full` on exhaustion, scope lifetime),
  `window::compose_self_check` (painter's order on scratch surfaces: a lower window
  repainting cannot change a higher window's pixels; a plane covers a window; destroying
  it restores the window).
- QMP: `scripts/canvas_overlap_qmp.py` (a lower window repaints its whole content with a
  higher window on top; the higher window's pixels are unchanged), a pointer sweep across
  window and plane boundaries with no trail, a menu plane over a window that is then
  destroyed, and a work-area clamp on drag and mode change.
- The visual golden is re-baselined exactly once, at the work-area cutover, with an
  explicit reviewed `--update`.

## Implementation status (M16, 2026-11-26)

- Builtins 92–97 exist: `window_begin`, `plane_create`, `plane_destroy`, `plane_begin`,
  `plane_present`, `work_area_reserve`. The persistent region claim (60/61) is deleted and
  its press rule is the plane rule (C12).
- Every window owns a content-sized canvas; drawing requires a scope (window or plane)
  and is `Status::Invalid` without one; `window_begin(0)` clears the scope. `window_open`
  gives the new window to its owner as the implicit scope, so a single-window application
  never calls `window_begin`.
- The compositor runs on every damage rectangle and on `*_present`; the erase/coverage
  machinery (`covers_from`), the move snapshot, the visible-parts subtraction, the
  full-repaint flag, the console's explicit clip and the shell's `repair_menu_area` are
  gone. `syslib.present` recomposites.
- The desktop shell's menu bar, pull-downs and halt screen are planes; the bar strip is
  reserved with `work_area_reserve(22)` and window frames clamp below it. The visual
  golden was re-baselined once for that 22-pixel shift
  (`686d0330…de52754c` → `c9546f45…79fbc65e`); the diff is that shift only.
- Deferred to M17: C4's surface-local coordinates, C8–C10, spec 0010 I35 (window-local
  events) and spec 0011 W36.

## Open questions

| # | Question | Trigger |
|---|----------|---------|
| 1 | A wallpaper or icon layer below windows (a plane layer, or a service image) | when the desktop gets a background image |
| 2 | A per-surface damage rectangle instead of whole-surface recomposition | if `perf` shows recomposition cost |
| 3 | Cursor composited into the shadow (spec 0010 open question 4) | a cursor-only fast path or a hardware sprite |
| 4 | Window resize on a canvas (reallocate, reflow, scale) | a resize ticket |
