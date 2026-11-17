# 0031: Coalesce damage before repair and batch console cells into one present

- **Status:** Accepted
- **Date:** 2026-11-17
- **Deciders:** igis-v2 kernel/UI
- **Related specs:** 0011, 0012, 0016

## Context

Two paint paths were correct but wasteful in the same way: they did bounded work per
individual piece instead of per visible result.

- **Window repair** collected bounded damage lists and presented the **bounding box of
  all damage once**. A small window moved diagonally leaves two frames that share almost
  no area, so the bounding box can approach the whole screen; a contained duplicate
  (the erase frame and the chrome frame of the same window) was still painted and
  counted. The service redraws chrome from its records and copies from one shadow, so
  redundant or over-wide work is pure cost.
- **Console output** painted every 8×8 cell and presented it immediately. A 40-column
  line therefore copied 40 separate rectangles and took the graphics lock 40 times, and
  each typed byte echoed through the same path.

Both paths are allocation-free, `no_std`, bounded and synchronous under the window lock
(`WINDOW → SCREEN`), and the console also holds `CONSOLE` outside that
(`CONSOLE → WINDOW → SCREEN`). Neither has a performance counter (Q8 introspection is
deferred), so the change must be self-evident from the code and provable from
measurements the kernel can make with its own state.

## Decision

We will **coalesce damage before painting and presenting**, and **batch console cells
into one present per byte run**.

- A bounded greedy pass drops empty and contained rectangles and merges overlapping or
  edge-touching rectangles into their bounding box. Erase requests are coalesced only
  within the same coverage limit; a request contained in one whose limit is at least as
  high is dropped because that one paints a superset. The presentation accumulates the
  coalesced erase and chrome rectangles and presents each once, instead of one
  screen-sized bounding box. Coalescing happens **after** per-window `Expose` marking,
  so exposed clients are chosen from the exact original damage.
- The invariant is **coverage, not equality**: the coalesced set must cover every
  original damaged pixel. Growth (the bounding box of diagonal neighbours) is allowed and
  safe because the extra area is copied from the current shadow and chrome is redrawn
  from the window records; loss is a defect and is checked at boot.
- A kernel text terminal keeps a bounded **dirty rectangle**. `paint_cell` draws and
  marks it; `write_bytes`/`_print` flush once after their byte run; the line-discipline
  echo defers to one `flush_pending` at the end of `poll_input`; a whole-grid repaint
  clears the box and presents each visible part once. The flush re-reads the visible
  parts under `WINDOW` and copies only the dirty rectangle.
- `window::damage_self_check` proves the rules on scratch state and measures a realistic
  move: four damage rectangles / 510 874 pixels as one bounding box become two
  rectangles / 55 348 pixels (89 % less present area). The boot line is
  `window: damage ok (present ...)`.

## Consequences

### Positive

- A move, scroll, raise or close copies and paints only the frames involved, not the
  span between them; the same self-check makes the reduction visible at every boot.
- Typing a line costs one present instead of one per cell, and a service opportunity's
  echo costs one flush instead of one per byte.
- Both lists stay static and bounded (32 damage rectangles each, a 64-rectangle
  presentation, one dirty rectangle per terminal); no allocation and no new opcode,
  builtin or counter.
- The lock order is unchanged: the coalescing is pure arithmetic under the existing
  lock, and the console flush takes `WINDOW → SCREEN` exactly where the cell paint did.

### Negative

- Greedy bounding-box merging can over-cover a diagonal gap. The extra area is repainted
  or re-presented (never stale), but it is more work than an exact rectangle partition.
  A future exact partition would trade the small overdraw for more rectangles.
- The console's single dirty bounding box can span two distant rows in one flush. The
  copy is still correct; a list of dirty runs would be tighter.
- Coalescing happens after expose marking but before painting; a later exactness change
  must keep that order or a merged hole could wake a client for damage that never
  existed.

### Neutral / follow-up work

- A true A/B build toggle for the coalescing and batching is not implemented; the
  self-check reports the arithmetic baseline instead (spec 0016, open question 1).
- Pointer-driven continuous dragging (M2.4) and a visual regression harness (M2.6.3)
  will exercise the coalescing with real damage streams.
- Q8's deferred introspection could expose present counts and areas to the desktop;
  until then the boot self-check is the measurement hook.
