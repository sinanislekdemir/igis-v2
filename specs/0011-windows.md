# 0011: Windows

- **Status:** Accepted
- **Scope:** `abi`, `compiler`, `interpreter`, `kernel` (`window`, `gfx`, `task`, `sched`),
  `syslib`; M2.3 of the desktop milestone
- **Supersedes:** none
- **Superseded by:** none
- **ADRs:** 0020, 0024, 0042, 0043
- **Updated:** 2026-11-18

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
| M2.3.4 | the desktop shell and kernel text windows; read-only enumeration (`window_info`), owner `0` for kernel windows (spec 0012) | M2.3.3 |
| M2.3.5 | the two-application proof: side-by-side windows, click focus/raise, a scripted owner drag and an `Expose` repaint, headless | M2.3.4 |

Non-goals for M2.3:

- no per-window canvas, double buffering, alpha blending or scaling (ADR 0020);
- no interactive resizing, no window menus, no icons — those arrive with M2.4;
- no pointer-driven continuous drag loop: M2.3.5 proves a **scripted** owner drag
  (a sequence of `window_move` calls); following the pointer while a button is held is
  M2.4. *(Superseded by M8.1: W22 implements the service-owned pointer drag; the M2.3
  scripted proof and its text remain the historical record.)*
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
  `window_present` (W17).
- **W11 (MUST):** The service tracks damage and repairs it synchronously (M2.3.2):
  `open`, `move`, `raise`, `close` and a focus change damage the affected frame
  rectangles, the overlapped windows and the desktop below them. One repair pass restores
  the desktop where a window was removed or moved, redraws the chrome of every damaged
  window bottom-to-top in its visible parts, and presents the union once. Client content
  is the client's: the service never keeps or repaints another window's content; it asks
  the owner to repaint with a whole-content `Expose` (W12).
- **W12 (MUST):** `Expose` is delivered to the owning task as a machine event with
  `kind = Expose`, `window = id`, `x = 0`, `y = 0`, `code = 0`, and it always asks for the
  **whole content rectangle**; M2 has no partial-damage rectangles. `EventKind.Expose` has
  ordinal 5. A repair marks one pending expose per damaged live window (coalesced), and
  only that window's owner can take it; the host tries the caller's pending expose before
  the machine-wide queue. The service never blocks on a client: one that does not repaint
  leaves its previous pixels (stale content), and the repair pass still presents the union.
  A client that ignores an expose is not closed in M2 (see W18).
- **W13 (MUST):** With graphics disabled (`screen_width() == 0`), `window_open` is
  `Status.Invalid`, `close`/`raise`/`move` are `Status.NotFound` (no window can exist),
  `window_frame_rect`/`window_content_rect`/`window_present` are `Status.Invalid`,
  `window_info` is `Status.NotFound` for every index, and `window_focus` is `0`. Nothing
  hangs and no allocation is attempted.
- **W14 (MUST):** The window service is entered through host builtins; `Host` defaults
  describe a disabled service (`Invalid` at `open`, on the rect reads and on
  `window_present`; `NotFound` at `close`/`raise`/`move`; focus `0`), exactly the kernel's
  disabled-table results (W13), so `langhost`, host tests and a kernel without graphics
  behave identically. No bytecode opcode is added.
- **W15 (MUST):** The table and its focus/z-order state are one machine-wide value behind
  one lock. Any core may serve any window call; the result is the same as if calls were
  serialised.
- **W16 (MUST):** Event routing happens at **poll time**, in the kernel host, where no
  graphics lock is held. Producers (`event::push_pointer`, `push_key`) keep
  `window = 0`; they must not take the window lock, because the pointer path runs under
  the graphics lock and the repair takes window → graphics. On poll:
  - a pending `Expose` for the calling task is returned first (owner-only, no queue);
  - otherwise the machine-wide queue supplies the event: a `PointerDown` is hit-tested
    against the topmost window frame, which is raised and focused before the click is
    delivered, and the event names that window; other pointer and key events name the
    focused window (or 0); an event that already carries a non-zero `window` (an injected
    `Expose`, or any injected addressed event) is passed through unchanged.
  A click that raises a window may mark an expose; the routing path wakes input waiters
  after dropping the window lock.
- **W17 (MUST):** `syslib.window_present(id)` presents exactly the window's content
  rectangle from the shadow, clipped to its **visible parts** (content minus the frames of
  higher windows), and leaves the chrome and higher windows' pixels untouched. It is
  owner-only like `close`/`move`: `Ok`, `NotFound` for an unknown id, `Invalid` for
  another task's window or disabled graphics. It does not repair, raise or mark anything.
  `syslib.present` remains a legal generic primitive (spec 0009, G10), but
  `window_present` is the sanctioned path for a window client: it bounds what the client's
  presentation can cover.
- **W18 (MUST):** Client liveness policy for M2: a task that never reads its events and
  never repaints leaves stale content on screen; the service does **not** close it, kill
  it, or reclaim its window for missing an expose. Window lifetime remains exactly task
  lifetime (`close`, `kill`, fault, normal exit), and a repaint is always requested again
  the next time that window is damaged. Timeouts and forced closes are deferred until
  there is a desktop shell that can judge a client (M2.3.4/M2.3.5).
- **W19 (MUST):** `syslib.window_info(index, var id, var rect, var title) -> Status` is
  the read-only enumeration (M2.3.4; spec 0012 D15). Index `0` is the **topmost** live
  window and indices increase down the z-order. `Ok` writes the id, the **frame**
  rectangle and the title through the out-parameters; an index at or past the live count,
  or disabled graphics, is `NotFound` and writes nothing. The title destination is a
  compile-time `var string` of at least 64 bytes (the W9 title cap); the interpreter
  bounds-checks it before the host call. There is no write access and no new opcode
  (builtin 40).
- **W20 (MUST):** Owner id `0` is reserved for the kernel. Global task ids start at `1`,
  so no task can own or address a kernel window by identity. A kernel window is otherwise
  an ordinary window: it has geometry, z-order, focus, `Expose`, hit-testing and
  `window_present`, and the kernel calls the same public functions with owner `0`
  (spec 0012 D6).
- **W21 (MUST):** A repair coalesces its damage before painting and presenting (M2.6.2).
  Empty and contained rectangles are dropped, overlapping or edge-touching rectangles are
  merged into their bounding box by a bounded greedy pass, and the result stays within
  the static list size. Erase requests with different coverage limits are never merged; a
  request contained in one whose coverage limit is at least as high is redundant and is
  dropped. The coalesced set MUST cover the union of the original damage: a merge may
  grow the repainted area (the bounding box of diagonal neighbours), but no damaged pixel
  may be lost. The presentation is the coalesced rectangles, not one bounding box of all
  damage, and every presented rectangle is copied from the current shadow. Coalescing
  happens after the per-window `Expose` flags are marked, so the exact original damage —
  not a grown bounding box — decides which clients repaint.
- **W22 (MUST):** A left-button press inside a window's 16-pixel title bar but not inside
  its close box raises and focuses that window (any task, like `raise`) and starts a
  **service-owned drag**. The service records the grab offset `(pointer - frame origin)`;
  while button 1 stays held every pointer move sets the frame origin to
  `pointer - grab offset`, clamped on screen exactly like `window_move` (W4). The move
  records the same damage and repair as `window_move`, so the owner is exposed and the
  desktop is restored underneath. Implicit capture applies: the pointer may leave the
  title bar. Release (button 1 clear) ends the drag and later motion does not move the
  window; a second press during the drag is consumed and ignored (no raise, no new grab);
  a dragged window that closes or whose owner terminates cancels the drag. Owner `0`
  windows drag exactly like task windows, because the service move bypasses the owner
  check only for this path (W23's close box keeps the ownership rule). Resize is not part
  of this requirement.
- **W23 (MUST):** A left-button press inside a window's 11 × 11 close box at
  `(frame.x + 4, frame.y + 3)` posts a **close request** for the window; it never
  terminates the owning task. The service sets the window's pending-close flag, and the
  owner receives the predeclared `EventKind.Close` (W25). The window stays open until its
  owner closes it; an owner that ignores the request leaves the window (no timeout, no
  automatic kill). A close box on an owner-`0` (kernel) window is inert: nothing is posted
  and nothing changes. The press is consumed like a title-bar press (W24). This is a
  request, not a save-on-close handshake: the owner decides how to close and may save
  first (ADR 0043).
- **W24 (MUST):** A press the service handles under W22/W23 MUST NOT be delivered to a
  client as a `PointerDown`, **even under an SMP poll race**. The two pointer paths
  serialise the whole transaction with one dedicated pointer-order lock: update the
  pointer state under the graphics lock, drop it, run the service hook, and only then
  publish the event records with the `PointerDown` suppressed when the hook consumed the
  press. The suppressed record is never enqueued, so a task woken by publication cannot
  observe it. `PointerMove`/`PointerUp` records still publish in order. Content-area
  presses, desktop presses and key events are unchanged and still route at poll time
  (W16). The hook runs from both pointer paths after the graphics lock is dropped, so
  injected `syslib.inject_pointer` events get the same synchronous behaviour as a device.
- **W25 (MUST):** A pending close request is delivered as `EventKind.Close` (ordinal 6)
  with `window = id` and `x = y = code = 0`. It is **owner-only** and **topmost-first**
  like an expose: `window::take_close(owner)` returns and clears the topmost pending
  request of that owner, `window::has_close(owner)` reports whether one is pending, and no
  other task can see or take it. The kernel host serves a pending `Close` **before** a
  pending `Expose` and before the machine-wide queue. A close request counts as input
  readiness for the owner: `input_ready`, the scheduler's post-block `Input` re-check and
  `next_any` source `1` include `has_close(owner)`, and the pointer hook wakes
  `WaitFor::Input` after dropping the window lock. A `Close` addressed in the queue (an
  injected event with a non-zero `window`) passes through unchanged. The window is removed
  only by its owner (`window_close`) or by its task's termination (`close_task`).

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

Owner `0` is the kernel (W20). Task ids start at `1`, and the kernel uses the same public
functions, so the kernel's terminal and system-log windows (spec 0012) are ordinary
windows with a reserved owner rather than a parallel table.

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

They are kernel-internal; there is no `syslib` binding. A repair pass resets the clip, and
for each rectangle it is about to paint it narrows the clip to that rectangle, draws, and
restores it. The clip is a property of the **shadow**, not of a task, so a global clip
cannot be held across an asynchronous `Expose` to another task: after the expose is
queued the client runs on its own core and its drawing calls would race with any other
client. M2 therefore narrows for service-owned paint only (chrome and desktop restore)
and limits the client's visible result at `window_present`, which presents only the
content rectangle's visible parts. Drawing outside those parts is a client bug and stays
possible in the shadow; the service draws the chrome over the content rectangle's border
and title row at every repair, and a later client present does not cover a higher window.

Because the clip is global, a client drawing on another core during the short synchronous
narrowed interval is clipped to the same rectangle. That is a bounded visual race with no
state corruption (the next repair or client draw repaints the area); it is accepted for
M2. `window_present` bounds the presentation, not the drawing, and the client is expected
to draw only in its visible content until a per-task clip exists (M2.4).

The boot self-check narrows the clip, paints across its edge, verifies the pixels outside
were not written, restores the clip and the pixels, and prints `graphics: clip ok`
(M2.3.1 proof).

### Composition and repair (implemented in M2.3.2)

The service is the only writer of chrome and the desktop; applications write only their
content rectangles. A repair always paints in this order, all while holding the window
lock:

1. **Desktop restore.** Each `Erase { rect, covers_from }` paints the base white and the
   `(X + Y)` black dither in the parts of `rect` not covered by live windows at z-order
   index `covers_from..`. The coverage limit is what makes the cases differ:
   - `open`: `covers_from = count` (the new window is topmost) — its whole frame starts
     as desktop, including the content the client is about to draw;
   - `move`, new frame: `covers_from = moved index + 1` — windows above the moved one
     keep their pixels; windows below are covered and correctly erased;
   - `move`, old frame and `close`: `covers_from = 0` — any live window overlapping the
     area keeps its pixels and has its chrome redrawn.
2. **Chrome, bottom-to-top.** Every window whose frame intersects the chrome damage
   redraws its chrome, clipped to each visible part of its frame (its frame minus the
   frames of every higher window) intersected with the damage. Drawing in z-order and
   clipping to visible parts means a lower window can never corrupt a higher window's
   content or chrome.
3. **Present.** The damage is coalesced (W21) and each coalesced rectangle is presented
   with `gfx::present`, which composites the cursor as before (spec 0010, I7). This
   replaces the single bounding-box present of M2.3.2: a scattered repair such as a
   diagonal move now copies the two frames, not the rectangle that spans both.
   Undamaged windows are not repainted.

`move` to the same origin records nothing. A refused operation (`NotFound`, `Invalid`)
records nothing. `close_task` accumulates every removed frame and repairs once, so a
terminated task's windows disappear together.

#### Damage coalescing and present batching (implemented in M2.6.2)

The damage lists are bounded (32 rectangles each) and a repair pass may collect the old
and new frame of a move, several chrome frames and a focus change. Before painting, two
coalescing passes reduce that work:

- **Erase damage.** Requests with the same coverage limit that overlap or touch merge
  into their bounding box. A request fully contained in another whose limit is at least
  as high is dropped: the container subtracts no more windows, so it paints the desktop
  across every pixel the contained request would. Different limits stay separate, because
  their subtraction sets differ. This is exact with respect to the coverage semantics;
  no live window is painted over beyond what the container already did.
- **Chrome damage.** Chrome carries no coverage limit, so the generic pass applies: empty
  and contained rectangles disappear and touching or overlapping pieces merge. Chrome is
  redrawn per window in its visible parts, so a grown rectangle can only re-draw chrome
  from the window records (never content), and a higher window is never touched.

The presentation accumulates every erase and chrome rectangle, coalesces that set the
same way, and presents each result once. The added area of a merge is copied from the
current shadow, so it is up to date by construction. `Expose` marking runs before
coalescing, so a window in the hole of a merged bounding box is not woken for damage that
never existed.

`damage_self_check` (boot line `window: damage ok (present ...)`) proves the merge rules,
the erase coverage rule and the coverage invariant on scratch state, then measures a
realistic diagonal move of a 200 × 120 window across a 1024 × 768 screen: four damage
rectangles presented as one bounding box of 510 874 pixels become two coalesced rectangles
of 55 348 pixels, an 89 % reduction in present area (2 rects instead of 4). It never
touches the screen. The same pass is what makes a drag/scroll repair charge the client for
its own frame, not for the span between the old and new origin.

#### Chrome

| Part | Pixels |
|------|--------|
| outer border | 1 px black on all four sides (`draw_rect` over the frame) |
| title bar | rows `y + 1 ..= y + 15`, inside the border, full inner width |
| focused title | white background with a black 1-px stripe on every other row |
| unfocused title | plain `0xCCCCCC` (no stripes) |
| close box | 11 × 11 at `(x + 4, y + 3)`: title-colour fill, black outline |
| title text | 8 × 8 font, black, centred in the band right of the close box; truncated to the band and preceded by a band-coloured fill so stripes never cross a glyph |
| separator | 1 px black at the last title row (`y + 15`) |

The service draws the chrome inside-out and the outer border last, so no interior fill
can spill onto it. The content rectangle is never written by chrome.

#### Desktop

The desktop is white (`0x00FFFFFF`) with the black 50% dither on every `(X + Y)` even
pixel — the same pattern `samples/apps/desktop` draws. The service owns this background;
an application that wants a different desktop clears the screen itself before opening
windows, and a later restore returns to the service's pattern.

#### Lock order and self-checks

A repair takes the window lock and then calls `gfx` (window → graphics). The wrappers
read the screen size (graphics) *before* taking the window lock, and no path takes the
graphics lock and then the window lock, so there is no cycle. The repair is synchronous
under the window lock: the z-order it walks is exactly the state the caller produced, so
no snapshot is needed.

`window: rules ok` checks the table policy; `window: paint ok` runs the damage lists,
their merge rules, the visible-part subtraction and the damage each lifecycle operation
records, all on scratch state without touching the screen.

### Routing, expose and `window_present` (implemented in M2.3.3)

Event producers do **not** stamp a window. The pointer producers run inside `gfx` while
the graphics lock is held, and the keyboard path runs from the console; taking the window
lock there would nest graphics → window against the repair's window → graphics and can
deadlock on SMP. Producers therefore leave `Event.window = 0` and a machine-wide queue
entry stays address-free until it is polled.

The kernel host routes on `poll_event`, where no graphics lock is held:

1. **Pending close request first.** `window::take_close(current)` returns the topmost
   window of the calling task with a pending close request, clears it and builds the event
   (`kind = Close`, `window = id`, `x = y = code = 0`). Only the owner can take it (W25).
2. **Pending expose next.** `window::take_expose(current)` returns the topmost window of
   the calling task with a pending flag, clears it and builds the event
   (`kind = Expose`, `window = id`, `x = y = code = 0`). A foreign task can never take
   another task's expose or close request, because the table is searched by owner; there
   is no per-task queue to steal from.
3. **Queue next.** `event::poll` supplies one queued event. If it already carries a
   non-zero `window` (an injected `Expose` or `Close`, or any explicitly addressed
   injection) it is passed through unchanged. A `PointerDown` is hit-tested against the
   topmost window **frame**: a hit raises and focuses that window (the W7 affordance)
   before the event is handed over, and the event names it; a miss stays 0. Every other
   pointer or key event names `window_focus()` (0 when no window exists).

#### Lock order (the only nesting is window → graphics)

| Path | Takes | Then |
|------|-------|------|
| pointer producer (`gfx`) | pointer-order | graphics (state/cursor), released before the hook; then window → graphics; then the event queue (press suppressed when handled) |
| pointer service hook (`gfx` → `window::pointer_event`) | pointer-order already held | window → graphics for a drag repair; `wake_input` only **after** the window unlock (a drag repair or a posted close request) |
| keyboard/console producer | console, graphics per cell | nothing |
| poll-time routing | nothing | window (`take_close`/`take_expose`/`route_click`); routing a click repairs: window → graphics; input waiters are woken **after** the window lock is dropped |
| window wrappers (`open`/`move`/`rect` reads) | graphics (screen size, released) | window → graphics for the repair; input wake after the unlock |
| repair | window | graphics (dither, chrome, present) |
| `Task::reclaim` → `close_task` | nothing (no scheduler lock held) | window → graphics; input wake after the unlock |
| scheduler `Input` block | no lock | `event::has_event` / `window::has_expose` / `window::has_close`; then slot locks in `wake_input` |

No path takes graphics and then window, and no path takes a scheduler slot lock and then
window. The repair is synchronous under the window lock, so the z-order it walks is the
state the caller produced.

#### Expose marking and wake-ups

`repair` marks one pending `Expose` on every live window whose frame intersects the chrome
damage — the same set whose chrome it repaints — before it paints, and returns whether it
marked anything. The wrappers drop the window lock and only then call
`task::wake_input()`, so a task blocked in `next_event` wakes and retries; the scheduler's
`WaitKind::Input` block re-check also tests `window::has_expose(task_id)` (and
`window::has_close(task_id)` for a posted close request), closing the race between the
empty read and the block. A flag is coalesced per window and dropped with the window on
close; a task that never polls leaves stale content (W18) and receives the request again
on the next damage. A close request is a separate per-window flag served before the
expose (W25); it is not coalesced with damage and stays pending until its owner takes it.

#### `window_present`

`window_present(id)` is owner-only (`NotFound` unknown, `Invalid` foreign or graphics
disabled). It computes the content rectangle minus the frames of every higher window and
presents each exact visible part with a narrowed clip (`set_clip` + `present` +
`reset_clip`), so the client's presentation cannot cover a higher window or the chrome.
It records no damage, takes no part in routing, and does not raise. `syslib.present`
remains a legal generic primitive (spec 0009, G10) for an application that owns the whole
screen; a window client uses `window_present`. The service still cannot undo a client that
draws outside its visible content (no canvas, no per-task clip); that is the accepted M2
limitation, and `Expose`/chrome repair covers the frame at the next damage.

`window: route ok` checks pending-expose marking, owner isolation, topmost-first take
order, hit-testing and the loss of an expose when its window closes, all on scratch state.

### Two-application proof (implemented in M2.3.5)

`samples/apps/windows_proof` proves the whole M2.3 surface with two **separate tasks**:

| Task | Window | Role |
|------|--------|------|
| `windows_proof_a` | `Proof A`, content 300 × 240 at `(40, 300)` | scripted owner drag; repaints its own expose |
| `windows_proof_b` | `Proof B`, content 300 × 240 at `(560, 300)` | moves over A and back, all by owner `window_move` |
| `windows_proof` | `Proof Control` at `(900, 640)` | injected clicks and the command stage |

The parent cannot call another task's `window_close`/`window_move`, and the only
cross-task window mutation is `window_raise`. The script therefore uses a third,
parent-owned window as a **persistent command stage**: the frame origin of
`Proof Control` (its `y`, in steps of 10) is read by both children with
`window_info`. It is state, not an event, so an early command is not lost and the
machine-wide event queue stays free for the injected pointer clicks.

Phases (all headless):

1. **Side by side.** Both children open and draw their windows; the parent waits for
   both titles and their content pixels. A pixel in each content rectangle proves
   both are visible at once.
2. **Focus switch and raise.** The parent raises `Proof Control` (any-task raise) to
   establish a non-app focus, then uses the real `syslib.inject_pointer` path: clamp
   to `(0, 0)`, move over A's content, press, drain. The pointer-down is routed at
   poll time (hit-test → raise → focus) by whichever task polls it. The parent
   asserts `window_focus()` and the z-order from `window_info`, plus the focused
   title-bar stripes versus the unfocused grey; it does not assert which task
   received the event, because the queue is machine-wide and either task may consume
   it. A second click over B switches the focus and raises B above A.
3. **Scripted drag.** Stage 1 makes A perform four owner `window_move` calls. The
   parent waits for the final frame rectangle and the content pixels, then checks
   that the old frame area is restored to the live terminal window below (a blank
   cell is white) and the new frame's border and content are painted.
4. **Expose and repaint.** Stage 2 makes B move over A. B is topmost, so its whole
   content draw is legal. The service marks A's whole-content `Expose`; A takes it
   (owner-only) and repaints only the part left of B's frame, because M2 has no
   per-task clip: a whole-content draw would overwrite B. The proof checks that the
   overlap shows B's content, A's visible strip is still A's, and A's visible border
   remains. Stage 3 makes B move back; A takes the new expose and repaints the
   uncovered content, which the proof checks at a pixel that was B's content before
   (so a stale client would fail). The vacated area is restored to the terminal
   below.
5. **Teardown.** Stage 4 asks both children to close their own windows and exit; the
   parent's `core.wait` acknowledges each exit (`Ok`). The proof then checks that both
   windows are gone, closes the control window, and `window_focus()` falls back to the
   kernel window that was focused at entry. No timeout or kill is involved.

`run windows_proof` prints `windows_proof: ok` only when every check passed, naming
the first failing step and label otherwise. `run windows_proof shot` additionally
prints `windows_proof: ready` and holds the composed frame for three seconds after
the drag/move checks so `scripts/windows_proof_screenshot_qmp.py` can capture it
with QMP `screendump`; that harness never views the image.

The drag is scripted. Pointer-driven continuous dragging (and the resize handles it
would enable) was deliberately M2.4, not an M2.3 promise; M8.1 implements the service
drag below.

### Pointer drag and close box (implemented in M8.1)

W22–W25 add two service-owned interactions over the same table. There is no new opcode,
builtin, `Host` method, `syslib` primitive or `stdlib` helper. The drag is not
application-visible: a client opens a window as before and the service intercepts the
press. The close box is application-visible only through the predeclared
`EventKind.Close` (W25), which M8.2 added in place of M8.1's kill (ADR 0043).

- **Hook and lock order.** `gfx::update_pointer` and `gfx::set_pointer_absolute` take one
  dedicated pointer-order lock for the whole transaction: update the pointer state and
  cursor under the graphics lock, **drop the graphics lock**, call
  `window::pointer_event(old_x, old_y, old_buttons, x, y, buttons)`, then publish the
  event records (with a consumed press suppressed) and release the pointer-order lock.
  The hook reads the screen size (graphics, released), takes the table lock, runs the
  pure table methods and repairs under `WINDOW → SCREEN`, then drops the table lock. It
  never holds the table lock across `task::wake_input`: the wake for a drag repair or a
  posted close request follows the existing wrapper pattern and is issued after the
  unlock. The
  order is `POINTER_ORDER → SCREEN` and `POINTER_ORDER → WINDOW → SCREEN`; only these two
  paths take the pointer-order lock, so there is no cycle. Injected
  `syslib.inject_pointer` goes through the same two functions, so the proof is synchronous
  and deterministic.
- **Press classification.** On a left-button press (bit 0, `0 → 1`) with no drag active,
  the topmost window under the pointer is classified: the close box first
  (`frame.x + 4 .. + 15`, `frame.y + 3 .. + 14`), then the 16-pixel title band (including
  the border). A content or desktop press returns "not handled" and is routed at poll time
  exactly as before.
- **Drag.** A title-band press calls the ordinary `raise` (focus + z-order + repair), then
  records `Drag { id, offset_x, offset_y }`. Each subsequent pointer update with bit 0 held
  calls `Table::drag_move`, which is `window_move`'s clamp/damage body without the owner
  check (`Table::relocate`); the public `syslib.window_move` stays owner-only. A release or
  a vanished window clears the drag. `remove_at` also clears a drag whose id disappears, so
  `close`, `close_task` and `Task::reclaim` cancel it in the same pass that removes the
  window. A press while `Drag` is set is consumed and ignored (no raise, no new drag), so
  it cannot reach a client and raise another window mid-drag.
- **Close box.** A task-owned close box sets the window's `close_pending` flag and
  returns `PressAction::Close`; the hook drops the table lock and wakes
  `WaitFor::Input` (the owner may be blocked in `next_event`). It never calls
  `task::request_kill`. Owner `0` returns `Consumed` and does nothing. The owner takes the
  request as a `Close` event (W25) and closes its own window; until then the window stays
  open. M8.1 originally killed the owner here; the About freeze showed that a dialog must
  close itself, so W23 was rewritten and ADR 0043 records the decision (it amends ADR
  0042's close clause).
- **Consumed press.** A handled press is suppressed **before publication**: after the
  service hook returns `consumed`, `event::push_pointer(..., suppress_down = true)` omits
  the `PointerDown` record entirely (the `PointerMove`/`PointerUp` records still publish
  in order). The two pointer paths hold one dedicated `POINTER_ORDER` spin lock across the
  state update, the hook and publication, so no task woken by the publication can poll a
  press the service handled — even on SMP. A client therefore sees no click for a
  title-bar or close-box press. The lock is the outermost pointer lock and gives
  `POINTER_ORDER → SCREEN` and `POINTER_ORDER → WINDOW → SCREEN`; only the two pointer
  paths take it.
- **Damage and expose.** `drag_move` uses the same `relocate` path as `window_move`: the
  old frame is erased to the desktop in the parts no live window covers, the new frame is
  filled under the moved window, chrome is redrawn, and every intersecting window gets its
  whole-content `Expose` (W12). The drag therefore restores the area underneath and asks
  the owner for a repaint exactly like an owner `move`; the client's repaint remains its
  own (W17), and a lower window must not paint over the dragged one (no per-task clip).

`window::drag_self_check` proves the pure rules on a scratch table: title-bar vs close-box
vs content classification, the grab offset, moves and clamps at all four edges, release,
the ignored second press, the owner-0 close-box exemption, the close request posting and
drag cancellation on close. Boot prints `window: drag ok` beside the existing `window:`
lines; `kmain` halts on the first mismatch.

`samples/apps/base/windows_drag` (+ `windows_drag_victim`) is the pointer proof: it opens
an "Underlay" window and a "Drag Target" above it, injects a real title-bar press
(asserting the press is consumed, the window focused and not moved), drags with button 1
through both screen clamps, releases and checks the frame does not move again, drains the
exposes, repaints the target and the underlay's visible bands, and checks the new content
and the restored old area with `syslib.get_pixel`. It then verifies a content press is
delivered to the client with the window id, proves the same-task dialog pattern below,
spawns the victim, presses its close box, requires the victim to close its own window and
exit `Ok`, and requires the window and its frame to be gone. Without graphics it prints
`windows_drag: host ok`.

### Close requests (implemented in M8.2)

W23/W25 replace the M8.1 kill-on-close with an owner close **request**. A close-box press
sets one per-window `close_pending` flag (`PressAction::Close`); nothing else changes in
the pointer path, the lock order or the consumed-press rule. The owner takes the request
through `poll_event` (the kernel host serves it before a pending `Expose` and before the
queue) and closes its own window with the ordinary owner-only `window_close`. The window
remains live until that call, so the service can keep repairing and presenting it; an
ignored request is simply not delivered again. `take_close` searches topmost-first and
clears the taken flag; a foreign owner sees nothing because the table is searched by
owner, exactly like `take_expose`.

Delivery and wake-up:

- `EventKind.Close` (ordinal 6) is predeclared with `window = id`, `x = y = code = 0`.
  The compiler's predeclared `EventKind` list and `abi::EventKind` carry the same order;
  `Event`'s 40-byte layout is unchanged. No opcode, builtin, `Host` method, `syslib`
  primitive or `stdlib` helper is added (ABI v1 stays at 75 opcodes).
- `window::take_close(owner)` / `has_close(owner)` mirror `take_expose` / `has_expose`.
  `remove_at` drops the flag with the record, so `close`, `close_task` and `Task::reclaim`
  clear it in the same pass that removes the window.
- The pointer hook wakes `WaitFor::Input` after dropping the window lock when a request
  was posted. `input_ready`, the scheduler's post-block `Input` re-check and `next_any`
  source `1` include `has_close(owner)`, so a genuinely blocked owner wakes and observes
  the event; readiness stays a snapshot and the following `next_event` is authoritative.
- An injected `Close` with a non-zero `window` is passed through unchanged, like an
  injected `Expose`; a `Close` queued with `window = 0` is not re-addressed to the focus.

`window::close_self_check` proves the pure rules on a scratch table: a close-box press
posts a request and is consumed (no raise, no drag), `take_close` returns only the owner's
topmost window and clears it, another owner sees nothing, an owner-`0` close box posts
nothing, and closing the window or its task drops the flag. Boot prints
`window: close ok` beside `window: drag ok`; `kmain` halts on the first mismatch.

Application behaviour:

- `desktop_shell` handles `Close` in both its blocking and check-mode paths: a request for
  `about_id` closes only the About panel and keeps the desktop running, while a request for
  its own `Desktop` window closes that window and exits.
- The user-facing windows (`calculator`, `minesweeper`, `editor`, `browser`,
  `clock_window`, `shell`, `widgets`) close their own window and exit on `Close`.
  `clock_window`'s bounded poll loop breaks out and closes; `shell` closes its terminal
  window after its last line.
- Proof and demo windows (`ui_*`, `visual`, `font`, `perf`, `windows*` proofs) may ignore
  `Close` and exit on their own schedule; ignoring a request leaves the window open, which
  is the accepted liveness rule (W18).

`samples/apps/base/windows_drag` proves both owner patterns: a same-task dialog window
receives `Close` (the only event; the close-box press never reaches the client as a
`PointerDown`), the app closes only that window and its main window survives, and the
separate victim task handles `Close`, closes its own window, prints one line and exits
`Ok` (not `Killed`). `desktop_shell check` injects a real press in the About close box,
requires exactly one `Close` event and no client click, closes About, and then reopens it
to prove the desktop (and the boot entrypoint) is still alive — the exact M8.1 freeze.
`scripts/about_close_qmp.py` reproduces the reported scenario on the boot desktop: open
System → About through QMP pointer events, click the About close box, wait for
`desktop_shell: about closed`, then launch the Terminal and type `terminal`. It fails if
the scheduler idle summary or a `halt:` line appears. Resize remains deferred (open
question 4): this slice drags whole windows only.

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
syslib.window_present(id: int) -> Status
syslib.window_info(index: uint, var id: int, var rect: Rect,
                   var title: string[64]) -> Status
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
| `window_present` | content presented (visible parts only) | unknown id | live window owned by another task, graphics disabled | — |
| `window_info` | id, frame and title written | index past the end, graphics disabled | title destination below capacity 64 (compile error) | — |

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
fn window_present(&mut self, id: u64) -> Status { Status::Invalid }
fn window_info(&mut self, index: u64) -> Option<WindowInfo> { None }
```

`WindowInfo` (interpreter crate) carries `id`, the frame `Rect`, and a fixed
`[u8; 64]` title with its length; the default `None` is the disabled service.

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
pub fn present(owner: u64, id: u64) -> Status;   // window_present
pub fn info(index: u64) -> Option<Info>;          // window_info, topmost first
pub fn visible_content_parts(id: u64, out: &mut [Rect]) -> usize; // kernel-only
pub fn take_expose(owner: u64) -> Option<u64>;   // poll-time routing
pub fn has_expose(owner: u64) -> bool;           // input block re-check
pub fn take_close(owner: u64) -> Option<u64>;    // poll-time close request
pub fn has_close(owner: u64) -> bool;            // input block re-check
pub fn route_click(x: i64, y: i64) -> Option<u64>; // hit-test + raise + repair
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
| `window_info` index past the end (M2.3.4) | `NotFound`; id, rect and title are untouched |
| Kernel window clicked (M2.3.4) | Ordinary hit-test/raise/focus; `Expose` asks the console to repaint |
| Application draws outside its content rectangle | Possible (no clip primitive, no capability model); the service repairs the damage on the next pass |
| `move` to the same origin | `Ok`, no change |
| `raise` on the top window | `Ok`, focus set, order unchanged |
| Title-bar press while a drag is active | Ignored by the service; the existing drag continues |
| Drag beyond a screen edge | Clamped so the frame stays fully on screen, like `move_to` |
| Close box on an owner-`0` window | Consumed, no request, nothing changes |
| Close box on a task window | A pending `Close` request is posted for the owner; the window stays until the owner closes it (no kill, no timeout) |
| Owner ignores a `Close` request | The window stays live with its last content; no retry and no automatic close (W18/W23) |
| Close request for a task that owns several windows | Only the topmost pending window is returned per take; each window carries its own flag |
| Dragged window closes or its owner terminates | The drag is cancelled in the same removal pass |

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
| 1 | ~~Expose delivery: a new `EventKind.Expose` event, a dedicated blocking wait, or a callback-like `window_present` handshake~~ | Resolved in M2.3.3: `EventKind.Expose` (ordinal 5), one coalesced pending expose per damaged window, owner-only take at poll time; `window_present` is the presentation call, not the delivery mechanism (W12/W17) |
| 2 | ~~Damage coalescing across several operations in one pass (union vs per-window flags)~~ | Resolved in M2.6.2: damage is coalesced per list before paint (W21), the presentation is the coalesced rectangle set rather than one bounding box, and `Expose` is still a per-window whole-content flag |
| 3 | ~~Client liveness: a window whose owner never calls `next_event` or never repaints — close it, or leave stale content forever~~ | Resolved for M2: leave stale content; no timeout, forced close or kill for a missed expose (W18). Revisit when the desktop shell can judge a client (M2.3.5/M2.4) |
| 4 | Resize and minimum/maximum size interaction with the 8 × 8 content floor | Not scheduled; M8.1/M8.2 drag and close whole windows only (deferred, not M8.2) |
| 5 | Title truncation for proportional text in M2.6 (the 8 × 8 font truncates by bytes, a proportional font by glyphs) | M2.6 |
| 6 | ~~Whether `window_present` should be the only present path for a window client, or `syslib.present` stays legal~~ | Resolved in M2.3.3: `window_present` is the sanctioned window-client path and is clipped to visible content; `syslib.present` stays a legal generic full-screen primitive (W17) |

## Changelog

| Date | Change |
|------|--------|
| 2026-11-16 | Initial accepted version: frame/content geometry, z-order, single focus, ownership and close-on-termination, the 16/64/8×8 limits, the internal clip save/restore, damage/`Expose` shape, the `window_*` surface plus the predeclared `Rect`, and the disabled-graphics rules. Implementation slices M2.3.1–M2.3.3 named. |
| 2026-11-16 | M2.3.1 implemented: the kernel `window` table with lifecycle, z-order, focus, ownership and `close_task` on `Task::reclaim`; builtins 32–38 with disabled-service defaults; the predeclared `Rect` record; the internal `gfx` clip save/narrow/restore with a boot self-check; the `windows` proof app and child. Chrome, damage and the damage-union `present` are M2.3.2; routing, `Expose` and `window_present` are M2.3.3. |
| 2026-11-16 | M2.3.2 implemented: service-drawn chrome (1-px black border, 16-px title bar, focused stripes vs plain unfocused, close box, centred 8×8 title), white + black dither desktop restore, bounded damage lists (32 rects) for desktop and chrome damage with visible-part subtraction, one `present` of the damage union, and a synchronous repair under the window lock. `window: paint ok` checks the arithmetic and the damage each operation records; `samples/apps/windows_demo` plus `scripts/windows_screenshot_qmp.py` prove the composition visually. The whole-content `Expose`, event routing and `window_present` remain M2.3.3. |
| 2026-11-16 | M2.3.3 implemented: `EventKind.Expose` (ordinal 5); producer-free routing at poll time in the kernel host (pending owner expose first, pointer-down hit-test → raise/focus → delivery, other pointer/key events to the focus, explicitly addressed injections passed through); per-window coalesced expose flags marked by `repair` and input waiters woken after the window unlock, with the scheduler's input-block re-check; `syslib.window_present` (builtin 39) presenting exactly the content's visible parts; `window: route ok`; and `samples/apps/windows_events` proving routing, expose fields, foreign isolation, `window_present` and close-on-termination. Open questions 1, 3 and 6 resolved; M2.3.4 is the desktop shell. |
| 2026-11-16 | M2.3.4 (spec 0012, ADR 0025): read-only `syslib.window_info` (builtin 40, no new opcode) returning a topmost-first id, frame and 64-byte title snapshot; owner `0` admitted as the kernel owner (task ids start at 1) and kernel windows are ordinary windows; the kernel's terminal and system-log windows use the same public functions. The three samples that assumed an empty window baseline record `window_focus()` at start and compare against it. |
| 2026-11-17 | M2.3.5 implemented: `samples/apps/windows_proof` plus the `windows_proof_a`/`windows_proof_b` children prove two side-by-side windows owned by separate tasks, a focus switch and raise by real injected pointer clicks (state asserted, because the queue is machine-wide), a four-step scripted owner drag with old-area restoration, and an `Expose`/`window_present` repaint after another task moves over and away (including the covered-content check and the visible-part-only draw that M2's missing per-task clip requires). The parent-owned `Proof Control` frame origin is the persistent command channel; teardown is a stage-4 close-and-exit acknowledged by `core.wait`. `run windows_proof shot` and `scripts/windows_proof_screenshot_qmp.py` capture the composed frame. The two prerequisites the proof exposed are fixed separately: a fair claim cursor (spec 0007, Sch12) and the console's stale-parts retry (spec 0012, D3). Pointer-driven continuous dragging remains M2.4. |
| 2026-11-17 | M2.6.2 implemented (W21, ADR 0031): a repair coalesces erase and chrome damage before painting (empty/contained dropped, touching/overlapping merged; different erase coverage limits never merged) and presents the coalesced rectangles instead of one bounding box of all damage. `window::damage_self_check` proves the merge rules, the erase coverage rule and the coverage invariant on scratch state and measures a diagonal move: 4 rectangles / 510 874 px as a bounding box become 2 rectangles / 55 348 px (89 % less present area); boot prints `window: damage ok (present 4 -> 2 rects, 510874 -> 55348 px)`. Expose marking still uses the exact original damage. Open question 2 is resolved. |
| 2026-11-18 | M8.1 implemented (W22–W24, ADR 0042): service-owned pointer drag from the title bar (raise/focus, grab offset, clamp, release, ignored second press, cancellation on close) and close-box termination of the owner task via `task::request_kill` and `Task::reclaim`; owner-0 close boxes are inert. The hook runs from both `gfx` pointer paths after the graphics lock is dropped, keeps `WINDOW → SCREEN` and never holds the table lock across `request_kill`/`wake_input`; a handled press is suppressed before event publication. `window::drag_self_check` prints `window: drag ok`, and `samples/apps/base/windows_drag` (+ `windows_drag_victim`) proves both interactions headlessly. Resize remains deferred (open question 4). |
| 2026-11-18 | M8.1 follow-up: W24 strengthened against an SMP poll race. The two pointer paths serialise the whole transaction with a dedicated `POINTER_ORDER` lock (`POINTER_ORDER → SCREEN`, `POINTER_ORDER → WINDOW → SCREEN`), run the service hook after dropping the graphics lock, and publish the events afterwards with `push_pointer(..., suppress_down)` omitting a consumed press; `event::consume_pointer_down` is deleted, so a handled `PointerDown` is never enqueued and a woken client cannot observe it. A second press during a drag is now `Consumed` (swallowed) instead of delivered. |
| 2026-11-18 | M8.2 implemented (W23 rewritten, W25 added; ADR 0043 amends ADR 0042's close clause): the close box posts an owner-only `EventKind.Close` (ordinal 6, `window = id`) instead of `task::request_kill`; the window stays until its owner closes it. One per-window `close_pending` flag with `take_close`/`has_close` (topmost-first, owner-only), served before a pending `Expose` and before the queue; `input_ready`, the scheduler's post-block `Input` re-check and `next_any` source 1 include it, and the pointer hook wakes `WaitFor::Input` after the unlock. `window::close_self_check` prints `window: close ok`; `windows_drag` proves a same-task dialog and a victim that closes itself and exits `Ok`; `desktop_shell check` closes About from its real close box and stays alive; `scripts/about_close_qmp.py` reproduces the reported System → About → close-box → Terminal scenario on the boot desktop. `desktop_shell` handles `Close` in both modes, the user-facing apps close their own windows, and proof/demo windows may ignore it. No opcode, builtin, `Host`, `syslib` or `stdlib` change (ABI v1 stays 75). |
