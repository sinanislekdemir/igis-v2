# 0046: A topmost move copies a content snapshot; the client re-reads the content rectangle

- **Status:** Accepted
- **Date:** 2026-11-18
- **Deciders:** igis-v2 kernel/UI
- **Related specs:** 0011 (W12/W17 clarified, W28 added), 0012 (D12), 0009 (one shadow), 0010 (input), 0024 (window records)

## Context

M8.1 made windows draggable by their title bar. The service moves the frame
and redraws the service-owned chrome, but it owns no client content. Every
windowed application caches the content rectangle it read at startup with
`syslib.window_content_rect` and draws and hit-tests in **absolute screen
coordinates**; there is no per-task clip and no relative drawing handle (spec
0011, W10).

Reproduced on the boot desktop: open `Calculator`, drag it by the title bar.
The frame and chrome arrive at the new position, but the new content rectangle
shows the underlay (the window below), not the calculator panel. The owner was
asked to repaint through the ordinary `Expose` path, but its cached rectangle
was still the old origin: it redrew at the old coordinates, and
`window_present` clipped that presentation to the **new** content rectangle, so
the new area stayed underlay while stray old pixels landed outside it. The
calculator's hit-testing used the old origin too, so a button click at the new
position did nothing. Two independent defects produce one stale window.

ADR 0024 already rejected a per-window canvas: there is one shadow, windows are
records, and the service must not duplicate client pixels. Any fix must keep
that model while making a drag look right immediately.

## Decision

We will split the problem into a presentation snapshot owned by the service and
a redraw obligation owned by the client.

### Service: move the rectangle first (W28)

Add one kernel-internal `gfx::copy_rect(src_x, src_y, dst_x, dst_y, w, h)`
(no `syslib` binding): a shadow-to-shadow copy clipped to the screen and the
current shadow clip, row-wise with an overlap-safe `copy_within`, and the row
direction chosen from the vertical direction so a vertically overlapping source
is not clobbered. Zero extents are `Invalid`; disabled graphics are `Invalid`;
a fully clipped copy is an `Ok` no-op. The cursor is never touched (it lives in
the framebuffer).

`Table::relocate` (shared by `window_move` and the drag) changes only for a
**topmost** window:

- it copies the old content rectangle to the new content origin **before**
  updating the geometry and before the repair, while the source pixels are
  still the old content;
- it records the old-frame erase (`covers_from = 0`, which subtracts every live
  window including the moved window's new frame, protecting the overlap);
- it does **not** record a new-frame erase: a `covers_from = count` erase would
  paint the desktop across the whole new content and wipe the snapshot;
- the chrome damage still covers both frames, and the repair paints the new
  border and title bar (never the content), marks the usual `Expose` flags and
  presents the damage union, so the copied rectangle reaches the framebuffer.

A non-topmost move is unchanged: the source may be covered, so it records both
erases and relies on the client's refreshed repaint. A programmatic
`window_move` and the pointer drag take the same path; ownership rules are
unchanged. The copy is a presentation snapshot, not a canvas: no content is
retained between repairs, and the client still redraws.

### Client: redraw at the refreshed rectangle (W12/W17 clarified)

On an owner `Expose` every windowed application re-reads
`syslib.window_content_rect(id, rect)` before drawing or hit-testing, and
recomputes any geometry it cached from it (Calculator's `disp_x`/`disp_y`,
Clock Window's face/readout origins, Widgets' control origins, Editor's text
and scrollbar layout). The expose is served before the queue, so the next
pointer or key event already uses the new origin. A client that ignores the
expose keeps the copied snapshot (the existing W18 liveness policy).

## Consequences

### Positive

- A dragged topmost window shows its content immediately, with no round trip
  and no blank/underlay frame. This is the common case: every drag raises the
  window first.
- The shadow remains the single image and the service still owns no content
  (ADR 0020/0024 are intact).
- The client's redraw fixes the second half — drawing and hit-testing at the
  new absolute origin — so the window is fully usable after the move.
- No opcode, builtin, `Host` method or `syslib` surface is added.

### Negative

- A topmost pointer move performs a full-content memmove per reported pointer
  step. The drag is the only fast path and the copies are bounded by the
  content size; this is accepted for M8.5.
- The service copies pixels it does not own. If a client drew outside its
  content rectangle, the snapshot can carry those stray pixels to the new
  origin. There is no clip primitive, so this is the same accepted M2
  limitation that lets a client draw anywhere in the shadow.
- A client that never processes its `Expose` keeps the copied snapshot: it
  looks right until something else damages it, and its hit-testing stays stale.
  That is W18's existing policy, not a new failure mode.
- A non-topmost move still shows underlay in the new frame until its owner
  repaints, because the source may have been covered. This is deliberate; the
  drag path always raises, so a dragged window is topmost.

### Neutral / follow-up work

- `Table::relocate` takes a `copy_content` flag. The public move and the drag
  pass `true`; the pure boot self-checks pass `false`, so they still never
  touch the screen while checking the new damage shape (a topmost move has no
  new-frame erase).
- Interactive resize, if it arrives, will need the same decision: a resize has
  no translation to copy, so it would rely on the client's repaint (or a more
  elaborate service copy).

## Alternatives considered

| Alternative | Why not |
|-------------|---------|
| A per-window canvas | Contradicts ADR 0024 and the one-shadow model; more copies, several images to keep consistent, and no shared memory to make it cheap |
| Block the drag until the client repaints synchronously | The service never blocks on a client; the drag runs in the pointer producer and a slow client would freeze input. There is also no per-task clip to ask the client to draw into |
| Give clients relative drawing handles or per-window coordinate translation | Every `syslib` primitive and `stdlib` helper would need a per-task origin; the language has no such state and the whole UI toolkit would change. Absolute screen coordinates stay |
| Let the server own and redraw the content | The server has no content to redraw; that is the canvas in disguise, and clients would still hit-test the stale origin |
| Repaint only (no copy), relying on the client | That is exactly the reported bug: the client redraw lands after the repair and `window_present` clips it to the new rectangle, so the new frame is underlay until the client wakes; on a busy system it can stay stale |
| Copy every move, including non-topmost | The source rectangle may be covered by a higher window, so the copy would move the wrong pixels (or transparent holes) into the new frame |
