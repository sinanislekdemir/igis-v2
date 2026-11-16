# 0024: Windows are kernel table records with monotonic ids, content-size geometry and owner-checked lifecycle

- **Status:** Accepted
- **Date:** 2026-11-16
- **Deciders:** igis-v2 design
- **Related specs:** `0003-language.md`, `0005-host-interface.md`, `0009-graphics-and-windows.md`, `0010-input-and-events.md`, `0011-windows.md`

## Context

ADR 0020 fixes the architecture: the window server is a kernel `syslib` service over one
shadow screen, with no per-window canvas and repaint driven by `expose`. It deliberately
leaves the window model itself open: what a window is, how one is identified, what
`open`'s size arguments mean, who may close or move it, how focus falls back, and how the
shadow's one clip rectangle is borrowed while the service paints.

These choices are hard to change once applications use them, because they are the visible
surface (`window_open(x, y, w, h, title)`), not internal code. They also interact with the
scheduler: a task can be killed or faulted at any time, so a window must not outlive its
owner.

## Decision

1. **A window is a record in a fixed 16-entry kernel table.** It holds the id, the owner
   task id, the frame origin, the content size, the title bytes and a used flag. Z-order
   is a separate array of table indices, bottom first. Nothing stores pixels and nothing
   allocates.
2. **Window ids are `int`, monotonic, never reused, with `0` invalid.** They are assigned
   by the service, not the application, and are not task ids. A nominal `WindowId` is not
   introduced while task ids are still `int` (Q43).
3. **`window_open` takes the frame origin and the content size.** The frame is derived:
   content plus a 1-pixel border left/right/bottom and a 16-pixel title bar on top. The
   chrome metrics are spec constants (M2.3.0), so a change is a spec revision, not a
   client surprise; the client's drawable size never depends on them.
4. **Ownership is the opening task, and it guards destruction and movement only.**
   `window_close` and `window_move` are owner-only; a live window owned by another task is
   `Invalid`. `window_raise` is allowed to any task, because a pointer click must raise and
   focus any window in M2.3.3 and raising changes only visibility order, not client data.
   `Task::reclaim` closes the task's windows on every destruction path.
5. **There is exactly one focused window or none.** `open` and `raise` focus; closing the
   focused window falls back to the topmost live window, or none. There is no focus stack:
   the topmost window is the only visible fallback.
6. **The shadow clip stays service-owned and is borrowed per paint, not held across an
   expose.** `gfx` gains an internal save/narrow/restore pair; the service uses it around
   its own chrome/background/presentation work. A client's drawing is not automatically
   clipped, because one global clip cannot span an asynchronous expose to a task that may
   run on another core. `window_present` limits the client's *presented* result to its
   content rectangle (M2.3.3). Drawing outside the content rectangle remains a client bug,
   which ADR 0020 already accepts.
7. **Expose asks for the whole content rectangle.** M2 has no partial-damage rectangles;
   one `Expose` event per damaged window per repair pass, and the service never blocks on a
   client. Partial damage and coalescing are M2.6 optimisations.

## Consequences

### Positive

- The table is static and tiny (16 × ~128 bytes), so a window costs no allocation and no
  pixel storage; the shadow stays the single image.
- Monotonic ids make every stale reference detectable as `NotFound` instead of reaching a
  recycled window; the same rule the task table already uses.
- Content-size geometry keeps the client's drawing area independent of chrome, and the
  rect reads expose the derived frame when the client needs the outline.
- Owner checks catch the common bug (one application closing or moving another's window)
  without pretending to be a security boundary, which the system does not have.
- Closing on `Task::reclaim` makes window lifetime exactly task lifetime, on every path.
- The clip save/restore is small, service-only and testable at boot, without a new
  application surface or a per-task clip the drawing primitives would have to consult.

### Negative

- A global clip that is not held across an expose means a buggy client can still paint
  outside its content rectangle; the next repair pass covers it, but a malicious client
  can corrupt the screen. Accepted: same model as ADR 0020.
- Any task can raise any window, so a task can steal focus. Accepted: no capability model
  (vision R14); a click must raise, and a task could already draw anywhere.
- No resize in M2.3: a client that wants a different size closes and reopens.
- Content-size geometry makes a frame-size request a small arithmetic step for the client
  that wants a specific outline.
- The 16-entry table is a hard limit; a desktop with many windows gets `Full` rather than
  a growable list. Accepted for the no-allocator design.

### Neutral / follow-up work

- Partial damage, expose coalescing and present batching are M2.6.
- Drag, resize and window menus are M2.3.5/M2.4 policies layered on `window_move`.
- `window_present` and routing are M2.3.3; this ADR only fixes the shape they must follow.

## Alternatives considered

| Alternative | Why not |
|-------------|---------|
| A user-space window server over streams | Pixels and geometry through a byte FIFO cost copies and need a new protocol larger than the service; ADR 0020 rejects it |
| Per-window canvases | Several images to keep consistent, extra copies, no shared memory to make them cheap |
| Nominal `WindowId` now | The language has no nominal scalars and task ids are still `int`; a second id type without the first is inconsistent (Q43) |
| Owner-only `raise` | A pointer click must raise another task's window; raising changes no data |
| Reject off-screen origins | A drag to the edge would fail rather than stop; clients would duplicate the clamp |
| A focus stack | The fallback would focus an invisible window; topmost is the only visible choice |
| A global clip held while the client repaints | The client runs asynchronously on its own core; there is no point in time at which the service can hold the clip around it |
| Expose with partial rectangles | More protocol and per-window state for an M2 that redraws whole contents anyway; an M2.6 optimisation |
