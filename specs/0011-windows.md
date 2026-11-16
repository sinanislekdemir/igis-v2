# 0011: Windows

- **Status:** Accepted
- **Scope:** `abi`, `compiler`, `interpreter`, `kernel` (`window`, `gfx`, `task`, `sched`),
  `syslib`; M2.3 of the desktop milestone
- **Supersedes:** none
- **Superseded by:** none
- **ADRs:** 0020, 0024
- **Updated:** 2026-11-16

## Context

M2.1 (spec 0009) delivered one full-screen shadow, a rectangular clip and nine drawing
primitives. M2.2 (spec 0010) delivered the pointer, the composited cursor and the
machine-wide event queue. What is missing is the thing those were built for: **windows** —
records the kernel owns, with geometry, a z-order, one focus and an owner, drawn onto the
same shadow. ADR 0020 already fixes the architecture: the window server is a kernel
`syslib` service over the one shadow, not an application and not a per-window canvas.

This spec fixes the window model. It is implemented in slices so each lands with a proof:

| Slice | Content | Depends on |
|-------|---------|------------|
| M2.3.1 | window records, lifecycle, geometry, z-order, focus, ownership, the internal clip helpers and the `syslib` window surface | this spec |
| M2.3.2 | composition and repair: chrome drawn by the service, damage accumulation, `present` of the damage union, background restore on move, raise/focus repaint | M2.3.1 |
| M2.3.3 | event routing (pointer/key to the focused window), `expose` delivery and the automatic content clip of `window_present`, close-on-task-termination proof | M2.3.2 |

Non-goals for M2.3:

- no per-window canvas, double buffering, alpha blending or scaling (ADR 0020);
- no interactive resizing, no drag loop, no window menus, no icons — those arrive with
  M2.3.5/M2.4;
- no multiple displays, no off-screen windows, no virtual desktops;
- no capability model: ownership is a service check, not a security boundary (vision
  R14);
- no new bytecode opcode: the window surface is host builtins behind `BUILTIN`.

## Requirements

Coordinates are pixels with the origin at the top-left of the screen. A **window id** is
an `int`; `0` is never a valid id.

- **W1 (MUST):** A window is a record in a fixed kernel table, never a region the
  application owns and never a second pixel buffer. The shadow remains the single image
  (spec 0009, G5).
- **W2 (MUST):** Window ids are global, monotonic, never reused and unique for the life
  of the boot; the first window opened after boot is `1`. When the id counter would wrap,
  it skips `0`.
- **W3 (MUST):** `syslib.window_open` takes the **frame origin** `(x, y)` and the
  **content size** `(w, h)`. The frame is the content plus a 1-pixel border on the left,
  right and bottom and a 16-pixel title bar on top:
  `frame = (x, y, w + 2, h + 17)`, `content = (x + 1, y + 16, w, h)`.
- **W4 (MUST):** The frame origin is clamped so the whole frame stays on screen. A content
  size below the minimum (8 × 8) or a frame larger than the screen is `Status.Invalid` and
  opens nothing. No window is ever partly off screen.
- **W5 (MUST):** Windows are ordered bottom-to-top in one total z-order. `open` and
  `raise` put the window on top; `close` removes it and compacts the order. Overlapping
  windows are legal.
- **W6 (MUST):** At most one window is focused; the focus is either one live window id or
  `0` (none). `open` and `raise` focus their window. `move` and closing a non-focused
  window do not change the focus. Closing the focused window moves the focus to the
  topmost live window, or `0` when none remains.
- **W7 (MUST):** A window belongs to the task that opened it.
  - `window_close` and `window_move` may be called only by the owner; a live window owned
    by another task is `Status.Invalid` and is not changed.
  - `window_raise` may be called by any task: it changes only z-order and focus, which is
    the affordance a pointer click needs in M2.3.3.
  - An unknown id (including `0`) is `Status.NotFound` for `close`/`raise`/`move` and for
    the rect reads.
- **W8 (MUST):** When a task terminates, every window it owns is closed through the same
  path as `window_close`, so the table cannot retain a window whose owner is gone. The
  focus fallback of W6 applies. This happens on **every** destruction path (normal exit,
  kill, fault, failed spawn, stale-slot eviction), because it is hooked into
  `Task::reclaim`.
- **W9 (MUST):** Limits are fixed: at most **16** windows machine-wide (`window_open` is
  `Status.Full` at the limit), a title of at most **64** bytes (longer titles are
  truncated at a UTF-8 code-point boundary), title bar **16** pixels, border **1** pixel,
  and a minimum content size of **8 × 8**. Limits are compile-time constants, not runtime
  configuration.
- **W10 (MUST):** The shadow keeps exactly one clip rectangle, owned by the service. The
  service may save it, narrow it to the rectangle it is about to paint and restore it.
  The drawing primitives do not change and applications get no clip primitive. Client
  content drawing is not automatically clipped (one global clip, several cores; see
  "Clip discipline"); a client's presentation is limited to its content rectangle by
  `window_present` in M2.3.3.
- **W11 (MUST):** The service tracks damage and repair (M2.3.2): `open`, `move`, `raise`,
  `close` and a focus change damage the affected frame rectangles, the overlapped windows
  and the desktop below them; a repair pass asks each damaged window's owner to repaint
  its content and presents the union once. The exact accumulation and present order is
  M2.3.2; this spec fixes what is damaged and that a client repaint is requested with a
  whole-content `Expose`.
- **W12 (MUST):** `Expose` is delivered to the owning task as a machine event with
  `kind = Expose`, `window = id`, `x = 0`, `y = 0`, `code = 0`, and it always asks for the
  **whole content rectangle**; M2 has no partial-damage rectangles. `EventKind.Expose`
  and its ordinal are added when M2.3.3 implements routing, together with the routing
  that fills spec 0010's `Event.window` field. The service never blocks on a client: one
  that does not repaint leaves its previous pixels (stale content), and the repair pass
  still presents the union.
- **W13 (MUST):** With graphics disabled (`screen_width() == 0`), `window_open` is
  `Status.Invalid`, `close`/`raise`/`move` are `Status.NotFound` (no window can exist),
  `window_frame_rect`/`window_content_rect` are `Status.Invalid`, and `window_focus` is
  `0`. Nothing hangs and no allocation is attempted.
- **W14 (MUST):** The window service is entered through host builtins; `Host` defaults
  describe a disabled service (`Invalid` at `open` and on the rect reads, `NotFound` at
  `close`/`raise`/`move`, focus `0`), exactly the kernel's disabled-table results (W13),
  so `langhost`, host tests and a kernel without graphics behave identically. No bytecode
  opcode is added.
- **W15 (MUST):** The table and its focus/z-order state are one machine-wide value behind
  one lock. Any core may serve any window call; the result is the same as if calls were
  serialised.

## Design

### Geometry

```text
frame origin (x, y)
  ┌──────────────────────────────────────┐
  │ title bar (16 px)                    │   content (w × h)
  ├──────────────────────────────────────┤   starts at (x + 1, y + 16)
  │                                      │
  │        content (w × h)               │
  │                                      │
  └──────────────────────────────────────┘
   frame (w + 2) × (h + 17), border 1 px
```

`window_open(x, y, w, h, title, var id)` stores `x, y, w, h` and derives both rectangles
on every read; nothing stores a redundant frame rect. `window_move` sets a new frame
origin with the same size. The chrome metrics are constants of this spec, so an
application never has to guess them: it reads `window_frame_rect` when it needs the whole
outline and draws in `window_content_rect`.

### Table, z-order and focus

```text
Table
  windows: [Window; 16]      // unordered storage; `used == false` means free
  order:   [u8; 16]          // indices into `windows`, bottom first, `count` entries
  count:   usize
  focus:   u64               // 0 or the id of a window in `order`
  next_id: u64               // 1, then monotonic

Window
  id, owner, x, y, w, h, title_len, title[64], used
```

- `open` takes a free storage slot, appends it to the top of `order`, sets `focus`, and
  clamps the origin (W4).
- `raise` moves the entry to the top, shifting the rest down; it sets `focus`.
- `close` removes the entry from `order`, clears the storage slot, and if it was focused
  sets `focus` to the topmost remaining id or `0`.
- `close_task` removes every window whose owner matches, in one pass, with the same focus
  rule after each removal.

Focus is an id, not an index, so it survives reordering. There is no "focus stack": the
fallback is always the topmost live window, which is the visible one.

### Ownership

The owner is the global task id of the caller, supplied by the kernel host (the
interpreter does not know task ids). A task may always close and move its own windows and
may raise any window. A finished task's windows are closed by `Task::reclaim` before its
regions are freed, on every path. Because the REPL can start the same application twice,
ownership is per task instance, not per application path; the second instance cannot touch
the first one's windows.

There is no capability model (spec 0003, H4; vision R14): the check exists so a bug or a
mistake in one application cannot silently destroy another's window, not to isolate a
hostile client. Every application can still draw anywhere on the shadow when it is not
clipped (ADR 0020, accepted).

### Clip discipline

`gfx` owns one clip rectangle, initialised to the full screen. The service adds two
internal operations that go through the same clipped drawing helpers:

```text
gfx::set_clip(x, y, w, h) -> bool   // intersect with the screen; false when disabled
gfx::reset_clip()                   // restore the full-screen clip
```

They are kernel-internal; there is no `syslib` binding. A repair pass saves the current
clip, narrows it to the rectangle it is painting, draws, and restores it. The clip is a
property of the **shadow**, not of a task, so a global clip cannot be held across an
asynchronous `Expose` to another task: after the expose is queued the client runs on its
own core and its drawing calls would race with any other client. M2 therefore narrows for
service-owned paint (chrome, background restore, presentation) and limits the client's
visible result at `window_present`, which presents exactly the content rectangle.
Drawing outside the content rectangle is a client bug and stays possible; M2.3.2 draws the
chrome over the content rectangle's border and bottom row so the damage is at least
repaired at the next pass.

The boot self-check narrows the clip, paints across its edge, verifies the pixels outside
were not written, restores the clip and the pixels, and prints `graphics: clip ok`
(M2.3.1 proof for the helper M2.3.2 needs).

### Damage and repair (shape fixed here, implemented in M2.3.2)

| Operation | Damaged |
|-----------|---------|
| `open` | the new frame rect; every window intersecting it; the desktop under the new frame |
| `raise` | the raised frame rect; the two focus title bars (old and new); windows intersecting either |
| `move` | the old frame rect ∪ the new frame rect; windows intersecting either; the desktop under both |
| `close` | the closed frame rect; windows intersecting it; the desktop under it |
| focus change | the title bars of the old and new focused frames |

A repair pass runs bottom-to-top over the damaged windows: draw the desktop/dither where a
window was removed, draw the chrome of each window (M2.3.2), queue one whole-content
`Expose` to each damaged window's owner, then present the union of the damaged rectangles
once. `present` composites the cursor as before (spec 0010, I7). Nothing in the repair
pass blocks on a client; a live task's expose sits in the event queue and is read by
`syslib.next_event` (M2.3.3). Windows that are not damaged are not redrawn.

This is the smallest protocol that keeps one image and one copy out: clients redraw on
demand, the service owns the order and the present.

## Interfaces

### Language surface

```text
syslib.window_open(x: int, y: int, w: int, h: int, title: const string[0],
                   var id: int) -> Status
syslib.window_close(id: int) -> Status
syslib.window_raise(id: int) -> Status
syslib.window_move(id: int, x: int, y: int) -> Status
syslib.window_frame_rect(id: int, var rect: Rect) -> Status
syslib.window_content_rect(id: int, var rect: Rect) -> Status
syslib.window_focus() -> int
```

`(x, y)` in `window_open` and `window_move` is the **frame origin**; `(w, h)` in
`window_open` is the **content size**. `Rect` is a new predeclared record, laid out like
every record (fields in declaration order, 8-byte aligned, size 32):

```text
type Rect = record        // abi::layout::rect
  x: int;                 // 0
  y: int;                 // 8
  w: int;                 // 16
  h: int;                 // 24
end;
```

Declaring `Rect` is a compile error, like `Diagnostic`. `Rect` is a plain value: it can be
declared, copied and passed to `stdlib` helpers, and it carries no reference to a window.

Error values:

| Call | `Ok` | `NotFound` | `Invalid` | `Full` |
|------|------|------------|-----------|--------|
| `window_open` | window created, `id` written | — | graphics disabled, `w < 8`, `h < 8`, frame larger than the screen | 16 live windows |
| `window_close` | closed | unknown id | live window owned by another task | — |
| `window_raise` | raised and focused | unknown id | — | — |
| `window_move` | moved (origin clamped) | unknown id | live window owned by another task | — |
| `window_frame_rect` | rect written | unknown id | graphics disabled | — |
| `window_content_rect` | rect written | unknown id | graphics disabled | — |
| `window_focus` | id of the focused window or `0` | — | — | — |

An out-parameter is written only on `Ok`, like `core.spawn`/`core.wait` (spec 0003, E1).
The id out-parameter and `window_focus` are `int`; ids are not compared to task ids except
by the service.

### Host contract

```rust
// `Rect` is `abi::Rect`; the host fills it only on `Ok`.
fn window_open(&mut self, x: i64, y: i64, w: i64, h: i64, title: &[u8])
    -> Result<u64, Status> { Err(Status::Invalid) }
fn window_close(&mut self, id: u64) -> Status { Status::NotFound }
fn window_raise(&mut self, id: u64) -> Status { Status::NotFound }
fn window_move(&mut self, id: u64, x: i64, y: i64) -> Status { Status::NotFound }
fn window_frame_rect(&mut self, id: u64) -> Result<Rect, Status> { Err(Status::Invalid) }
fn window_content_rect(&mut self, id: u64) -> Result<Rect, Status> { Err(Status::Invalid) }
fn window_focus(&mut self) -> u64 { 0 }
```

The interpreter pops the operands in declaration order, resolves the `var` references and
the title, calls the host, writes the id or the `Rect` image (32 bytes at the
`abi::layout::rect` offsets) on `Ok`, and pushes the `Status` ordinal. The kernel host
supplies its current task id as the owner. The defaults are the disabled service, so a
host mock and `langhost` never need a screen (W14).

### Kernel contract

```rust
// kernel/src/window.rs
pub fn open(owner: u64, x: i64, y: i64, w: i64, h: i64, title: &[u8])
    -> Result<u64, Status>;
pub fn close(owner: u64, id: u64) -> Status;
pub fn raise(id: u64) -> Status;
pub fn move_to(owner: u64, id: u64, x: i64, y: i64) -> Status;
pub fn frame_rect(id: u64) -> Result<Rect, Status>;
pub fn content_rect(id: u64) -> Result<Rect, Status>;
pub fn focus() -> u64;
pub fn close_task(owner: u64);       // Task::reclaim
```

The monolithic table is one `SpinLock<Table>`. `open` reads the screen size before taking
the table lock, so the two locks are never nested (a future composition pass that needs
both takes them in the same order: graphics, then window). The pure policy lives on
`Table` methods that take the screen size and the owner as arguments, so a boot self-check
runs the same code on a scratch table with fixed geometry (`window: rules ok`).

## Failure modes

| Situation | Behaviour |
|-----------|-----------|
| Graphics disabled and a window call arrives | `open` `Invalid`, rect reads `Invalid`, `focus` 0, `close`/`raise`/`move` `NotFound`; nothing hangs |
| Table full | `open` `Full`, nothing opened |
| Content below 8 × 8 or frame larger than the screen | `open` `Invalid`, nothing opened |
| Origin partly off screen | Clamped so the frame stays fully on screen; `Ok` |
| Unknown id (or `0`) | `NotFound` for `close`/`raise`/`move` and the rect reads |
| Non-owner `close`/`move` | `Invalid`, window unchanged |
| Non-owner `raise` | Allowed; z-order and focus change |
| Focused window closes | Focus moves to the topmost remaining window, else 0 |
| Owner task terminates (exit, kill, fault, failed spawn) | All its windows close, focus falls back |
| Title longer than 64 bytes | Truncated at a UTF-8 code-point boundary; `Ok` |
| Empty title | Legal; a 2-pixel content offset is unaffected |
| Two cores call the service at once | One table lock serialises them; no partial state is visible |
| Client ignores an `Expose` (M2.3.3) | Its previous pixels remain; the repair pass still presents |
| Application draws outside its content rectangle | Possible (no clip primitive, no capability model); the service repairs the damage on the next pass |
| `move` to the same origin | `Ok`, no change |
| `raise` on the top window | `Ok`, focus set, order unchanged |

## Alternatives considered

| Alternative | Why not |
|-------------|---------|
| `w, h` mean the frame size | The caller's drawable area would depend on chrome metrics, and a title- bar change would silently shrink every client's content; content-size is the value an application actually uses |
| A nominal `WindowId` type | Task ids are still `int` (Q43) and the language has no nominal scalars; `int` with 0 invalid matches `spawn`/`wait` and keeps the surface small |
| Reject off-screen origins instead of clamping | A drag to the edge would fail instead of stopping at the edge, and every client would need the same clamp arithmetic |
| Keep a focus stack | The visible topmost window is the only sensible fallback; a stack would focus an invisible window |
| Owner-only `raise` | A pointer click (M2.3.3) must raise any window; raising changes no client data |
| A per-window canvas | More copies, several images to keep consistent, and no shared memory to make it cheap (ADR 0020) |
| Expose with partial damage rectangles | One whole-content repaint per pass is simpler and matches "the client redraws its content"; partial rectangles are an M2.6 optimisation |
| A global clip held across the expose | One shadow and several cores: the client runs asynchronously, so there is nothing to hold the clip against |
| Resize in M2.3 | Needs a drag loop, minimum-size policy and client reflow; deferred to M2.4/M2.3.5 |
| Let a client draw into the shadow with its own clip | No clip primitive, no capability model, and a second clip would need per-task state the drawing primitives do not have |

## Open questions

| # | Question | Trigger / owner |
|---|----------|-----------------|
| 1 | Expose delivery: a new `EventKind.Expose` event, a dedicated blocking wait, or a callback-like `window_present` handshake | M2.3.3; this spec fixes the whole-content shape and the `Expose` event as the working design |
| 2 | Damage coalescing across several operations in one pass (union vs per-window flags) | M2.3.2/M2.6, when a drag produces many moves per frame |
| 3 | Client liveness: a window whose owner never calls `next_event` or never repaints — close it, or leave stale content forever | M2.3.3/M2.3.5, after event routing exists |
| 4 | Resize and minimum/maximum size interaction with the 8 × 8 content floor | M2.3.5/M2.4 |
| 5 | Title truncation for proportional text in M2.6 (the 8 × 8 font truncates by bytes, a proportional font by glyphs) | M2.6 |
| 6 | Whether `window_present` should be the only present path for a window client, or `syslib.present` stays legal | M2.3.3, with the expose protocol |

## Changelog

| Date | Change |
|------|--------|
| 2026-11-16 | Initial accepted version: frame/content geometry, z-order, single focus, ownership and close-on-termination, the 16/64/8×8 limits, the internal clip save/restore, damage/`Expose` shape, the `window_*` surface plus the predeclared `Rect`, and the disabled-graphics rules. Implementation slices M2.3.1–M2.3.3 named. |
| 2026-11-16 | M2.3.1 implemented: the kernel `window` table with lifecycle, z-order, focus, ownership and `close_task` on `Task::reclaim`; builtins 32–38 with disabled-service defaults; the predeclared `Rect` record; the internal `gfx` clip save/narrow/restore with a boot self-check; the `windows` proof app and child. Chrome, damage and the damage-union `present` are M2.3.2; routing, `Expose` and `window_present` are M2.3.3. |
