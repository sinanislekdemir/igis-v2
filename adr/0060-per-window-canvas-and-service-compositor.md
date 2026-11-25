# 0060: Retain every window's content and compose the screen from surfaces

- **Status:** Accepted; implemented by M16 on 2026-11-26, except the local-coordinate clause (spec 0026 C8–C10), which is deferred to M17
- **Date:** 2026-11-25
- **Deciders:** project owner
- **Related specs:** 0026 (composition, surfaces and coordinate spaces — normative),
  0009, 0010 (I19/I35), 0011, 0012, 0022
- **Supersedes:** ADR 0046 (move snapshot), ADR 0045 (persistent pointer-region claim),
  ADR 0024's "no per-window canvas" clause, ADR 0055's erase/full-repaint machinery

## Context

The display has been one shared shadow that applications draw into directly (ADR 0020).
That single decision produces every reported display defect:

- **Overlap corruption.** A lower window's repaint writes over a higher window's pixels in
  the shared image. `window_present` clips the copy, so the screen is not corrupted at that
  instant, but the shared image is. The next present of the higher window, the topmost move
  snapshot (ADR 0046) or a full repaint (ADR 0055) publishes the wrong pixels.
- **Cursor trails.** The cursor is composited into the framebuffer only (spec 0010 I7), so
  clearing it copies shadow pixels back. When those pixels are the ones the overlap bug
  corrupted, the pointer leaves a trail. The cursor is the most visible reader of a
  corrupted image, not a separate bug.
- **Repair complexity.** Because a window's content is not retained, a repair must
  reconstruct "what belongs on this pixel" from damage lists with per-window coverage
  limits (`covers_from`), desktop erases, a topmost-only snapshot and a full-stack-repaint
  flag. ADR 0031's coalescing, ADR 0046's snapshot, ADR 0055's full repaint and ADR 0045's
  pixel region claim are all workarounds for the same missing invariant.

ADR 0024 rejected a per-window canvas on memory grounds and because it would be a second
source of truth. The memory argument does not hold at project scale (a content-sized
canvas is well under 1 MiB for a typical window), and the "second source of truth" is
avoided by giving each canvas exactly one writer (its owner) and one reader (the
compositor) — a producer/consumer split, not duplicated state.

## Decision

The composed screen becomes a **pure function of retained surface state**: the service
retains every surface's pixels and recomposes the shadow from them.

- **Surfaces.** A **window** (frame, chrome, content, z-order, owner) and a **plane** (a
  screen rectangle above all windows) each own one **canvas**: a rectangle of
  `w * h * 4` bytes with no row padding, from the frame allocator, freed with the surface.
  There is **no canvas budget constant**; the allocator is the only limit and a failed
  allocation is `Status.Full` with nothing created.
- **Draw scope.** A task draws into a surface only inside `window_begin`/`plane_begin`…
  `*_present`. The scope lives in the surface record, so preemption or a blocking call
  cannot redirect a later draw. Outside a scope every drawing primitive is
  `Status.Invalid`: there is no "draw the shadow" escape hatch, so the compositor is the
  only writer of the shadow.
- **Local coordinates.** Drawing, `get_pixel` and window-addressed pointer events use
  **surface-local** coordinates; `(0, 0)` is the surface top-left. Absolute geometry
  queries remain for placing a second window, but a client never needs an origin to draw
  or hit-test, so there is no cached origin to go stale and no "re-read the rectangle on
  `Expose`" discipline. This is the part that makes the model simple rather than merely
  correct: a move is an origin change in the service only.
- **Planes replace the region claim.** The desktop shell's menu bar and pull-downs are
  planes: retained rectangles above all windows. A press inside a plane belongs to its
  owner (the old claim rule), and destroying a plane recomposites what was underneath, so
  the shell's `repair_menu_area` workaround disappears. `work_area_reserve(height)` keeps
  window frames out of a reserved top strip so a permanently visible bar plane can never
  hide a title bar.
- **Painter's algorithm.** `composite(rect)` paints the desktop pattern, then every live
  window bottom to top (chrome, then its canvas), then every plane, then the cursor. A
  higher layer overwrites a lower one. There is no visible-parts subtraction, no coverage
  limit, no "present only what I painted" rule and no content snapshot: the compositor
  owns every pixel it paints, so "what is on this pixel" is answered by construction.
- **Damage = recomposite.** Damage is a bounded list of screen rectangles; a repair
  recomposites and presents each. `Erase`/`covers_from`, the move snapshot, the
  full-stack-repaint flag, the shell's visible-part drawing discipline and the console's
  explicit-clip paint path are deleted.

## Consequences

### Positive

- The two reported defect classes disappear by construction: a lower window cannot write a
  higher window's canvas, and the cursor's restore source is always a valid composition.
- The repair model shrinks to one function. The most intricate parts of specs 0011/0012 —
  erase coverage, visible parts, the generation check, the full-repaint flag, the move
  snapshot, the region claim — are removed rather than maintained.
- Uncovering, raising, switching, closing a menu and changing the mode need no client
  round trip; a slow or blocked client keeps showing its canvas.
- Local coordinates delete per-app origin arithmetic and the W28 re-read discipline from
  every application.
- Memory is explicit and bounded by the allocator; there is no arbitrary constant to tune.

### Negative

- A present costs one extra copy (canvas → shadow, then shadow → framebuffer), and a
  recomposition repaints the windows it overlaps. The `perf` baselines must be re-taken.
- Every windowed application is rewritten to local coordinates and loses its
  visible-part-only drawing discipline: mechanical, but it touches the whole corpus and the
  proof apps.
- Six new builtins (92–97) and a new `Host` surface; ABI v1 opcodes stay at 75.
- The work area changes layout: a full-screen window no longer has a frame equal to the
  screen, so the visual golden is re-baselined once, deliberately.
- Client self-checks must read pixels inside their draw block (`get_pixel` is
  scope-local), which reorders a few `verify` blocks.
- A client that exhausts memory with planes or windows can deny others a surface; the trust
  model is unchanged (no capability model, vision R14), but the failure is now `Full`.

### Neutral / follow-up work

- A wallpaper/icon layer below windows needs a plane layer (spec 0026 open question 1).
- Per-surface damage and a partial-canvas composite if `perf` shows recomposition cost.
- Cursor composited into the shadow (spec 0010 open question 4) becomes cheap once the
  shadow is authoritative; it is not needed for correctness.
- Window resize becomes straightforward on a canvas (reallocate, reflow) but is out of
  scope.

## Alternatives considered

| Alternative | Why not |
|-------------|---------|
| Per-window client draw clip on the shared shadow (ADR 0055's deferred fix) | It removes the corruption only while every clip is exactly current, which needs per-task visible-part clip state refreshed atomically with z-order changes — the same per-task machinery as a canvas, with no retained content, so the service still cannot restore a covered window and every visibility change still needs a client round trip. |
| Content-sized canvas with absolute-coordinate translation | The expedient version: application code is unchanged, but the origin becomes a cacheable value again, a stale cache paints offset inside the canvas, and the kernel pays a translation per primitive. Local coordinates are the same work for the kernel and less for every client. |
| A full-screen canvas per window | No translation needed in the old absolute model, but 16 MiB per window at 2560×1600 and no conceptual benefit over local coordinates. |
| Keep the shared shadow and add a machine-wide lock around every draw | Correct only if clients also stop drawing outside their window, which is the clip problem again; it serialises all drawing for no structural gain. |
| Make the menu bar a plain window | It must stay above all windows and must not be raised or closed by a click, which is exactly a plane, not a window. |
| Keep the region claim and give it a canvas | A claim is "these pixels are mine" without content; a plane is the same rectangle with retained content and a real layer, and it removes the special case instead of extending it. |
| Keep the erase/`covers_from` repair and composite on top | Two sources of truth for the background, and every erase special case survives. Recomposition answers the same question without history. |
| A machine-wide 32 MiB canvas budget | An arbitrary constant that either refuses legitimate windows at high resolutions or needs raising; the frame allocator already answers the same question exactly. |
