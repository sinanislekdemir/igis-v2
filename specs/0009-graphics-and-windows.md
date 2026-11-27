# 0009: Graphics, the shadow screen and the window service

- **Status:** Accepted
- **Scope:** `abi`, `compiler`, `interpreter`, `syslib`, `kernel` (`font`, `gfx`, `console`),
  `arch/x86_64` framebuffer access; M2 of the desktop milestone
- **Supersedes:** none
- **Superseded by:** none
- **ADRs:** 0020, 0024, 0030, 0032, 0060 (0026 is the composition spec)
- **Updated:** 2026-11-25

## Context

The first milestone drew text straight into the linear framebuffer from
`kernel/src/console.rs`: one `fill_rect` per glyph pixel, no stored image, no clipping.
That is enough for a boot log, but the next milestone is a retro desktop (Mac OS
System 7.5 look), and every later piece — a desktop background, windows, menus, a mouse
pointer, `expose`-driven repaint — needs one image the kernel owns and can copy from.

The chosen shape (ADR 0020) is a kernel-owned **shadow screen**: a full-screen pixel
buffer in the frame allocator's memory that is the single composed image. Since ADR 0060
and spec 0026 applications draw into their own surface canvases and the service composes
the shadow from them; the text console draws through its own canvas too, so the
framebuffer still receives exactly one image from exactly one writer.

This spec defines the drawing layer only. The window service that will sit on top of it
(mouse, events, windows, z-order, `expose`) is described in "Planned M2 shape" as a
forward reference; it is not implemented here.

Non-goals for this slice:

- no mouse, keyboard events, windows, focus, z-order, or `expose`;
- no alpha blending, no scaling, no full-screen buffer per window; each window's canvas
  is content-sized (G23, spec 0011 W33). The 8×8 console font is the default and a
  separate 12-pixel proportional face is specified by 0015;
- no new bytecode opcode: the primitives are host builtins behind `BUILTIN`;
- no compile-time screen size and no fixed `.bss` shadow.

## Requirements

Coordinates are pixels, the origin is the top-left corner, `x` grows right and `y`
grows down. Colours are `0x00RRGGBB`.

- **G1 (MUST):** The kernel keeps the multiboot2 framebuffer information (address,
  pitch, width, height, bpp, type) in one graphics module, not in the console.
- **G2 (MUST):** The screen mode at boot is whatever the bootloader returns. Nothing in
  the kernel or the language may assume 1024×768: the shadow is sized from the actual
  `pitch * height`, and `syslib.screen_width()`/`screen_height()` report the current
  geometry. A runtime change is defined separately by spec 0022 and keeps the same
  no-assumption rule.
- **G3 (MUST):** Only 32 bpp RGB (`bpp == 32`, framebuffer type 1) is supported. Any
  other mode disables graphics; the console keeps working through its text path, which
  in this slice is serial only (the kernel never draws text into an unsupported mode).
- **G4 (MUST):** The shadow is one full-screen buffer allocated from the frame
  allocator at boot, `pitch * height` bytes of physically contiguous frames. It is not
  a static `.bss` array and it is not duplicated per window. Every surface (a window or
  a plane) additionally owns one canvas sized to its own rectangle, from the same
  allocator (spec 0026 C1/C2).
- **G5 (MUST):** The shadow is the single **composed** image and only the compositor
  writes it (spec 0026 C14). Drawing primitives write the caller's open surface canvas,
  and nothing else (G23). `present` copies a rectangle of the shadow to the real
  framebuffer row by row, using the framebuffer pitch (not `width * 4`), and only
  within the rectangle. `present` then composites the software cursor on top of the
  framebuffer copy; the cursor is never stored in the shadow (spec 0010, I7).
- **G6 (MUST):** `syslib.clear(color)` fills the caller's open surface canvas with
  `color` and is `Status.Invalid` without an open scope (spec 0026 C5). It is no longer
  a screen-wide operation, and no application can paint the desktop or another surface.
- **G7 (MUST):** A drawing primitive is clipped to the target surface's canvas, and no
  other clip exists: the service's one shadow clip is gone (spec 0026 "Retired
  mechanisms"). The
  region is an internal kernel facility so the window manager can narrow it later; this
  slice does not expose it to applications.
- **G8 (MUST):** Drawing is clipped, never faulted. Coordinates outside the target
  canvas are legal and produce no pixels outside it. An application cannot fault the
  kernel with a coordinate that overflows the canvas.
- **G9 (MUST):** Out-of-range arguments return `Status.Invalid` and draw nothing:
  `w <= 0` or `h <= 0` on a rectangle primitive, and a colour with any bit above bit 23
  set. `get_pixel` has no error path and returns `0` outside the screen.
- **G10 (MUST):** The `syslib` graphics surface is exactly the small orthogonal set in
  "Interfaces": `screen_width`, `screen_height`, `fill_rect`, `fill_dither`,
  `draw_rect`, `draw_text`, `blit_rgb`, `get_pixel`, `present`, `clear`, plus the three
  proportional-text primitives of G20 (`text_width_prop`, `text_height_prop`,
  `draw_text_prop`). Runtime mode selection is the separate surface of spec 0022; it
  adds no drawing primitive.
- **G11 (MUST):** `fill_dither` writes `color` to every other pixel of its extent — the
  pixel `(X, Y)` when `(X + Y)` is even — and leaves the other pixels untouched. `X` and
  `Y` are the coordinates passed to the primitive, i.e. the surface's own grid, so a
  window's pattern is stable across moves (spec 0026 C4). This is the System 7 desktop
  background when a white `fill_rect` precedes it.
- **G12 (MUST):** `draw_rect` is a one-pixel outline inside the given extent; the
  interior is untouched.
- **G13 (MUST):** `draw_text` draws 8×8 glyphs from the console font, one byte per
  cell, advancing `x` by 8 per byte, with a transparent background. It does no newline
  processing and no wrapping; a byte outside `0x00..=0x7F` draws a blank cell.
- **G14 (MUST):** The text console draws glyphs into the shadow and presents the rows
  it touched. Kernel `print!`/`println!` output therefore has one path to the screen.
  The existing console lock and line atomicity are preserved across cores. Since
  M2.3.4 (spec 0012) the console paints into two kernel-owned text windows instead of
  the whole screen: application output (including the shell's own input echo) goes to a
  terminal window, kernel diagnostics to a system-log window, and no text scrolls the
  shadow. The one-shadow and one-lock rules are unchanged.
- **G15 (MUST):** When the framebuffer is missing, is not 32 bpp, or the shadow cannot
  be allocated, graphics are disabled: `screen_width`/`screen_height` return 0, every
  drawing primitive returns `Status.Invalid`, `get_pixel` returns 0, and the kernel and
  console continue on the text path.
- **G16 (MUST):** The primitives are host builtins with defaults that model a disabled
  screen, so `langhost`, host tests and a kernel without graphics stay deterministic.
- **G17 (MUST):** The window service of the next slices is a kernel service exposed
  through `syslib`; it uses this shadow and `present`. It is never a user-space program
  over streams (ADR 0020).
- **G18 (SHOULD):** A representative desktop application (background, menu bar, one
  window, text) runs in a few thousand interpreted instructions per quantum: all
  rasterisation is in `core`/`syslib`, not in per-pixel language loops.
- **G19 (MUST):** The UI layout/chrome helpers (`stdlib.ui_*`, spec 0003) are ordinary
  language code over G10's primitives and add no drawing builtin, opcode or window-service
  path. They draw **client** chrome (a dialog/panel/control inside an application's own
  content); the window service remains the only producer of real window frames and its
  own title bar, close box and border (spec 0011, ADR 0024). The library may call
  `syslib` for this (ADR 0026) and inherits G9/G15's error and disabled-screen rules.
- **G20 (MUST):** `text_width_prop`, `text_height_prop` and `draw_text_prop` expose the
  12-pixel proportional bitmap font of spec 0015. The first two are pure metrics
  (advances and line height; 0 when graphics are disabled); `draw_text_prop` draws with a
  transparent background, `y` at the top of the line box, printable ASCII through the
  variable-width glyphs and every other byte through an 8-pixel 8×8 fallback cell (blank
  at or above `0x80`). The same clipping, colour and disabled-screen rules as G9/G15
  apply. `draw_text` (G13) is unchanged and remains the 8×8 primitive; the console and
  the `stdlib.ui_*` helpers keep using it in M2.6.1. No opcode is added.
- **G21 (MUST):** The framebuffer is proved without a display by a **host-side
  whole-screen dump**, not by a kernel serial dump: QEMU's QMP `screendump` captures the
  display surface that `present` writes, and `scripts/visual_regression.py` compares the
  decoded RGB pixels **exactly** against a committed golden (`tests/golden/visual.png`
  plus a SHA-256/tile manifest). The captured frame must be deterministic: the specimen
  (`samples/apps/visual`) opens a window whose frame is exactly the screen, contains no
  clock, animation, event or host input, covers the kernel text windows and holds after
  `visual: ready` without changing a pixel. The same golden MUST match on `-smp 4` and
  `-smp 1`. A normal harness run MUST NOT rewrite the golden; only an explicit,
  reviewed `--update` may. No kernel debug builtin or serial dump is added (ADR 0032).
- **G22 (MUST):** `blit_rgb(x, y, w, h, pixels, pw, ph)` draws a `pw × ph` image of
  three bytes per pixel (`0xRRGGBB`, no alpha, no colour conversion) scaled into the
  destination rectangle with nearest-neighbour sampling: destination pixel `(dx, dy)`
  samples source `((dx * pw) / w, (dy * ph) / h)` with integer arithmetic. `w`, `h`,
  `pw` and `ph` must be positive and `pixels` at least `pw * ph * 3` bytes; otherwise
  the result is `Status.Invalid` and nothing is drawn. The destination is clipped to
  the target canvas exactly like `fill_rect` (G8/G9); a fully clipped rectangle is `Ok`
  with no writes. Disabled graphics return `Status.Invalid`
  (G15/G16). No opcode is added.
- **G23 (MUST):** A drawing primitive requires an open **surface draw scope**
  (`window_begin`/`plane_begin`) and writes that surface's canvas; with no open scope it
  is `Status.Invalid` and draws nothing (spec 0026 C3/C4). Client coordinates are
  **window-local** for a window canvas and screen for a plane canvas (spec 0026
  C4/C8/C11). A rectangle is clipped to the canvas; an off-surface coordinate is legal
  and draws nothing (G8). A canvas write never presents; presentation is a recomposition
  (spec 0026 C14/C16).
- **G24 (MUST):** `get_pixel` reads the caller's open scope surface in its own coordinates
  (window-local or plane-screen) and, with no scope, the **composed shadow** in screen
  coordinates — always the uncomposited pixel (spec 0026 C6, spec 0010 I7). A client's
  pixels reach the shadow on its `*_present` or a repair, so a shadow read of the
  composition runs after the present (spec 0026 C6).

## Design

### Shadow and presentation

```
application ──syslib.fill_rect/...──▶ surface canvas (window or plane, surface-local)
                                          │
                          *_present       │  compositor: desktop pattern, then every
                          or a repair     │  window and plane bottom to top
                                          ▼
                                   shadow (pitch × height, frame allocator)
                                          │
                          presentation    │  row-by-row copy, pitch aware
                                          ▼
                                   real framebuffer
```

- The surface and composition model is ADR 0060 and spec 0026. The compositor is the
  only writer of the shadow; there is no application path into it.

- The shadow row stride equals the framebuffer pitch, so a row is one contiguous run and
  `present` is a per-row copy of `w * 4` bytes. Padding bytes beyond `width * 4` are
  allocated but never drawn or presented.
- Every pixel write goes through one helper that bounds-checks against the target
  canvas. `get_pixel` reads the same helper's storage inside a draw scope, so a
  self-check sees exactly what was drawn.
- `present` ends by compositing the software cursor into the framebuffer (spec 0010,
  I7): black outline and white fill pixels are written straight to the framebuffer
  inside the presented rectangle, never into the shadow. `get_pixel` therefore returns
  uncomposited pixels, which is what keeps the self-check meaningful and headless
  verification deterministic.
- A full-screen shadow at 1024×768×32 is 3 MiB (768 frames at 4 KiB). The only other
  per-pixel storage is one rectangle-sized canvas per live surface, bounded by the frame
  allocator (spec 0026 C2).

### Clipping

There is no shared clip. A primitive first rejects `w <= 0`/`h <= 0`, then converts to
`i64`, intersects the rectangle with the **target canvas** and draws only the result
(spec 0026 C4). A surface's own rectangle is the only clipping authority an application
sees; the compositor's painter's order decides which surface's pixels are visible
(spec 0026 C14). The `gfx::set_clip`/`reset_clip` pair of M2.3 is retired.

### Console

`kernel/src/console.rs` keeps its lock and byte-output paths but loses its direct
framebuffer access. Each glyph is painted into the console window's **canvas** with an
opaque background; scrolling moves the grid and recomposites the frame once. The console
writes through the same surface helper as `syslib`, and the compositor is the only writer
of the shadow, so a partially drawn rule or a desktop underneath is never in a different
image from what an application reads.

**Superseded in part by spec 0012 (M2.3.4):** the console no longer owns the full
screen. It keeps two character grids and paints inside two kernel-owned window content
rectangles (owner `0`), one per stream. **Spec 0026 supersedes the per-cell clip:** the
console paints into its own canvas and the compositor decides visibility.

Text before `gfx::init` runs is serial-only. `kmain` initialises the frame allocator and
graphics before the console attaches, so the normal boot log is on screen from the first
line.

### Planned M2 shape (forward reference, not implemented)

| Slice | Content | Uses this slice |
|-------|---------|-----------------|
| M2.2 | PS/2 mouse, a composited cursor, pointer state and the `syslib` pointer surface (spec 0010); the console stays a kernel device | shadow, `present`, clip |
| M2.3 | Kernel window service (spec 0011): windows with a frame, title bar, content rectangle and z-order, single focus and task ownership (M2.3.1); then frame chrome, damage union and `present` (M2.3.2), and event routing plus `expose` repaint under the service-owned clip (M2.3.3) | shadow, clip, `present` |

The window service repaints on `expose`; in this slice it does not keep per-window
canvases. **Superseded by ADR 0060 and spec 0026:** every surface owns a rectangle-sized
canvas, the service recomposes the shadow from the surface stack, and a client draws into
its canvas inside a `window_begin`/`plane_begin` scope. An application redraws its canvas
when asked, and the service decides when to recompose. Mouse/event delivery and the exact
`expose` protocol
belong to M2.2/M2.3 and will be specified there.

### Visual proof and golden frames (M2.6.3)

```
build/igis-shell.iso ──QEMU (headless)──▶ serial: shell> ──▶ run visual shot
                                                    │
                                   full-screen deterministic window_present
                                                    │
                          QMP screendump ──▶ decoded RGB ──▶ exact compare
                                                    │              │
                                     tests/golden/visual.png   match / diff
                                     tests/golden/visual.json  (exit code)
```

- The specimen window's frame is `frame = content + (2, 17)`, so opening it with content
  `(screen_w - 2, screen_h - 17)` at `(0, 0)` makes the frame exactly the screen. The
  service paints the border and the focused title bar (in the proportional face), and
  the application paints every content pixel; the two kernel text windows and all boot
  output underneath are covered. The app then checks representative pixels with
  `get_pixel` (chrome, both fonts, palette, dither parity, outlines) and prints
  `visual: ok`, so a broken primitive fails before the golden is consulted.
- Nothing in the shot is time- or input-dependent. Printing `visual: ok`/`visual: ready`
  goes to the shell's terminal window, which is fully covered: the console paints only
  its visible parts, so the lines change no pixel of the specimen. The hold after
  `visual: ready` is a bounded clock loop that also draws nothing.
- The pointer cursor is composited into the framebuffer by `present` and stays at the
  boot position `(0, 0)` because the harness injects no events. It is therefore part of
  the deterministic golden, not a source of noise.
- The golden is stored as a PNG container and a JSON manifest with the exact SHA-256 of
  the decoded RGB bytes, the dimensions and a SHA-256 per 64×64 tile. A compare failure
  prints the differing pixel count, the bounding box, the first differing pixel and the
  differing tiles; `--update` is the only writer, and a manifest that disagrees with its
  PNG is a hard error (a corrupt golden cannot silently pass).
- The harness supports a negative check: `--tamper` flips one captured pixel in memory
  and requires the comparator to reject it; `--expect-mismatch` inverts the comparison
  result for a deliberately wrong golden. Both are diagnostics, and neither is used by
  a normal run.
- The golden is tied to the bootloader's returned mode and the QEMU display path. A mode
  or QEMU conversion change, or an intended visual change, is a reviewed `--update`
  (ADR 0032); the golden is not regenerated automatically.

## Interfaces

### Language surface

```text
syslib.screen_width() -> uint            // 0 when graphics are disabled
syslib.screen_height() -> uint           // 0 when graphics are disabled
syslib.fill_rect(x: int, y: int, w: int, h: int, color: uint) -> Status
syslib.fill_dither(x: int, y: int, w: int, h: int, color: uint) -> Status
syslib.draw_rect(x: int, y: int, w: int, h: int, color: uint) -> Status
syslib.draw_text(x: int, y: int, text: const string[0], color: uint) -> Status
syslib.text_width_prop(text: const string[0]) -> uint  // 0 when disabled
syslib.text_height_prop() -> uint                      // 12, or 0 when disabled
syslib.draw_text_prop(x: int, y: int, text: const string[0], color: uint) -> Status
syslib.blit_rgb(x: int, y: int, w: int, h: int,
                pixels: const bytes[0], pw: int, ph: int) -> Status
syslib.get_pixel(x: int, y: int) -> uint // 0x00RRGGBB, or 0 outside
syslib.present(x: int, y: int, w: int, h: int) -> Status
syslib.clear(color: uint) -> Status
```

The language signature is normative; the compiler enforces `int` coordinates, a `uint`
colour and a `string` text argument, and the interpreter enforces the runtime rules
(G9). No opcode is added: each primitive is a `BUILTIN` call whose operands are on the
operand stack in declaration order.

The pointer surface (`pointer_x`, `pointer_y`, `pointer_buttons`, `inject_pointer`) is
part of the desktop too but belongs to spec 0010; it adds no primitive here.

### Host contract

```rust
fn screen_width(&mut self) -> u64 { 0 }
fn screen_height(&mut self) -> u64 { 0 }
fn fill_rect(&mut self, x: i64, y: i64, w: i64, h: i64, color: u32) -> Status { Status::Invalid }
fn fill_dither(&mut self, x: i64, y: i64, w: i64, h: i64, color: u32) -> Status { Status::Invalid }
fn draw_rect(&mut self, x: i64, y: i64, w: i64, h: i64, color: u32) -> Status { Status::Invalid }
fn draw_text(&mut self, x: i64, y: i64, text: &[u8], color: u32) -> Status { Status::Invalid }
fn text_width_prop(&mut self, text: &[u8]) -> u64 { 0 }
fn text_height_prop(&mut self) -> u64 { 0 }
fn draw_text_prop(&mut self, x: i64, y: i64, text: &[u8], color: u32) -> Status { Status::Invalid }
fn blit_rgb(&mut self, x: i64, y: i64, w: i64, h: i64, pixels: &[u8], pw: i64, ph: i64) -> Status { Status::Invalid }
fn get_pixel(&mut self, x: i64, y: i64) -> u32 { 0 }
fn present(&mut self, x: i64, y: i64, w: i64, h: i64) -> Status { Status::Invalid }
fn clear(&mut self, color: u32) -> Status { Status::Invalid }
```

The kernel implements these over `kernel/src/gfx.rs`. The defaults describe a machine
with no screen, which is also the kernel's disabled state, so both paths agree.

### Visual harness

```text
python3 scripts/visual_regression.py build/igis-shell.iso            # compare, -smp 4
python3 scripts/visual_regression.py build/igis-shell.iso --cores 1  # compare, -smp 1
python3 scripts/visual_regression.py build/igis-shell.iso --update   # rewrite golden
python3 scripts/visual_regression.py build/igis-shell.iso --tamper   # negative check
```

```text
visual: ok
visual: ready
[harness] capture 1024x768 (9439 bytes png) sha256 3780...
visual: ok (1024x768, sha256 3780...)
```

Exit code 0 means the decoded frame matched the golden; 1 means a mismatch (with the
pixel/tile diff), a missing or corrupt golden, a boot/ready timeout, or a specimen
self-check failure. `run visual` without `shot` performs the same composition and
checks, closes the window and prints `visual: ok`; with graphics disabled it prints
`visual: host ok`.

### Kernel contract

`gfx::init(fb: Option<Framebuffer>)` runs after the frame allocator and returns either
an `Info` (width, height, pitch, bpp, shadow address/size/frames, free-frame delta) or a
`Disabled` reason (no framebuffer, unsupported bpp, bad geometry, allocation failed).
`gfx` owns the shadow for the lifetime of the system; the frames are never freed.

## Failure modes

| Situation | Behaviour |
|-----------|-----------|
| Framebuffer tag absent | Graphics disabled; console is serial-only; kernel continues (G15) |
| Framebuffer type is not RGB or bpp is not 32 | Graphics disabled with a boot line naming the mode; console on the text path |
| `pitch < width * 4` or `width`/`height` zero | Geometry rejected as unsupported; graphics disabled |
| Shadow allocation fails | Graphics disabled with a boot line naming the byte count; console on the text path |
| `w <= 0` or `h <= 0` | `Status.Invalid`, nothing drawn |
| `blit_rgb` with `w`, `h`, `pw` or `ph` non-positive, or a buffer shorter than `pw * ph * 3` | `Status.Invalid`, nothing drawn |
| Colour above `0x00FFFFFF` | `Status.Invalid`, nothing drawn |
| Coordinates wholly outside the screen | `Ok`, nothing drawn |
| Partly outside the screen or the clip | Clipped; `Ok` |
| `get_pixel` outside the screen | `0` |
| `draw_text` with a non-ASCII byte | Blank 8×8 cell, advance 8; `Ok` |
| `draw_text_prop` with a printable byte | Proportional glyph, variable advance; `Ok` |
| `draw_text_prop` with a non-ASCII byte | Blank 8×8 cell, advance 8; `Ok` |
| `text_width_prop`/`text_height_prop` with graphics disabled | `0` |
| `blit_rgb` with a fully clipped destination | `Ok`, nothing drawn |
| `blit_rgb` with graphics disabled | `Status.Invalid`, nothing drawn |
| Graphics disabled and a primitive is called | `Status.Invalid`; the application may ignore it |
| Two cores draw at once | The graphics lock serialises each primitive; output interleaves at primitive boundaries |
| Screen mode is larger than 1024×768 | Works: shadow and clip derive from the actual geometry |
| Visual specimen: graphics disabled | `visual: no graphics`; the harness refuses to capture |
| Visual specimen: screen below 640×480 | `visual: screen too small`; the harness refuses to capture |
| Visual specimen self-check fails | `visual: FAIL at step N: label`, no `ready`; the harness exits 1 |
| Golden missing | `visual: missing golden ...; run --update once`, no boot |
| Golden manifest disagrees with its PNG | hard error naming the mismatch; no compare |
| Captured frame differs from the golden | differing pixel count, bounding box, first pixel and tiles; exit 1 |
| `--tamper` comparison still matches | `EXPECTED a mismatch` and exit 1 |

## Alternatives considered

| Alternative | Why not |
|-------------|---------|
| Keep drawing directly to the framebuffer | No source of truth, `get_pixel` impossible, repaint and window clipping would need a second copy anyway |
| Static `.bss` shadow at a fixed 1024×768 | Contradicts G2/G4 and wastes memory on small modes; the frame allocator already exists |
| Per-window canvas | Extra copies, ownership questions, and no benefit without shared memory (ADR 0020) |
| A user-space window server over streams | Byte FIFOs cannot carry pixel data efficiently and cannot share a canvas; the kernel owns the framebuffer (ADR 0020) |
| A new opcode per primitive | No rasterisation belongs in bytecode; the host trait already stages hardware-adjacent work with safe defaults |
| Mask an over-wide colour instead of rejecting it | Hides a caller bug and makes the host tests non-deterministic; G9 rejects it |

## Open questions

| # | Question | Trigger / owner |
|---|----------|-----------------|
| 1 | ~~Clip-region exposure to applications (`set_clip`/`reset_clip`)~~ | Resolved: no application-facing clip. The clip stays service-owned with an internal save/narrow/restore (spec 0011, W10) |
| 2 | ~~Cursor and pointer rendering: soft cursor in the shadow, or a hardware sprite~~ | Resolved: composited at present time, never in the shadow (spec 0010, ADR 0021) |
| 3 | Dirty-rectangle accumulation for `present` | M2.3, with `expose` |
| 4 | ~~A second font or a proportional font~~ | Resolved: spec 0015 and ADR 0030 add the 12-pixel proportional face; `draw_text` stays 8×8 |

## Changelog

| Date | Change |
|------|--------|
| 2026-11-16 | Initial accepted version: shadow screen, clipping, the nine `syslib` primitives, console through the shadow, disabled-graphics rules, and forward references to M2.2/M2.3. |
| 2026-11-16 | M2.2: `present` composites the software cursor into the framebuffer after the row copy (G5), so `get_pixel` stays uncomposited and headless self-checks stay deterministic; the cursor model itself is spec 0010. M2.2 row of the forward-reference table updated; cursor question resolved. |
| 2026-11-16 | M2.3.0/M2.3.1: spec 0011 and ADR 0024 fix the window model (content-size geometry, one z-order, one focus, ownership, 16/64/8×8 limits). `gfx` gains the kernel-internal `set_clip`/`reset_clip` used by the service, with a boot self-check (`graphics: clip ok`); no application clip primitive is added (open question 1 resolved). The `syslib` drawing surface is unchanged; chrome, damage and `present` of a union are M2.3.2. |
| 2026-11-16 | M2.3.2: the window service draws chrome and restores the desktop through the same shadow and internal clip (narrowed per visible part), then presents the damage union once; `syslib.present`/`clear` and the nine drawing primitives are unchanged. The service desktop is white with the `(X + Y)` black dither. `window: paint ok` checks the damage arithmetic at boot. |
| 2026-11-16 | M2.3.4 (spec 0012, ADR 0025): the console paints two kernel-owned text windows (owner `0`, terminal and system log) instead of the whole screen, with per-window grids and scrolling; expose repair is pulled by `console::pump` at the service opportunity. The whole-screen console helpers and the shadow scroll are removed from `gfx`; the nine drawing primitives, `present`, `clear` and the clip are unchanged. A full-screen `clear` erases the console windows' pixels; their grids survive and repaint on the next expose/output. |
| 2026-11-17 | M2.6.1 (spec 0015, ADR 0030): G20 adds the three proportional-text builtins (`text_width_prop`, `text_height_prop`, `draw_text_prop`, ids 45–47) and the window-service title bar uses the proportional face; `draw_text`, the console and the `stdlib.ui_*` helpers stay 8×8. No opcode; ABI v1 stays at 75. |
| 2026-11-17 | M2.6.3 (ADR 0032): G21 resolves M2.1.2 with a host-side QMP `screendump` of a deterministic full-screen specimen (`samples/apps/visual`) compared exactly against `tests/golden/visual.png` + a SHA-256/tile manifest (`scripts/visual_regression.py`). No kernel serial dump or debug builtin is added. The specimen covers the kernel text windows, has no clock/input, checks its own pixels and holds after `visual: ready`; the same golden matches on `-smp 4` and `-smp 1`, `--tamper` proves the comparator rejects a changed pixel, and only `--update` rewrites the golden. 376 host tests. |
| 2026-11-21 | Runtime display modes (spec 0022, ADR 0056): G2 now separates the boot mode from a later change; the Bochs VBE dispi changes the real mode, the shadow is reallocated and swapped under the graphics lock, and builtins 80–83 expose the mode list and `set_resolution`. No drawing primitive, opcode or boot-mode assumption is added. |
| 2026-11-21 | Graphics: G22 adds `blit_rgb` (builtin 91, effect `(7,1)`), a nearest-neighbour scaled blit of a `pw × ph` RGB buffer into a destination rectangle. The interpreter rejects non-positive `w`/`h`/`pw`/`ph` and a buffer shorter than `pw * ph * 3` with `Invalid` before the host call; the kernel clips to the screen and the clip region and writes the shadow through the same path as `fill_rect`, so a fully clipped rectangle is `Ok`. No alpha, no colour conversion and no opcode; ABI v1 stays at 75. |
| 2026-11-25 | Surface canvases and composition accepted (ADR 0060, spec 0026): G4 admits one rectangle-sized canvas per surface from the same frame allocator, G5 makes the shadow the compositor's **composed** image with no application write path, G6 turns `clear` into a scope-local fill, and G7/G23/G24 replace the global clip with a canvas-bounded draw. `syslib.present` becomes a recomposite request. No drawing primitive signature or opcode changes; ABI v1 stays at 75. |
| 2026-11-26 | Implemented in M16.2/M16.4 (spec 0026 "Implementation status"): the `Surf` surface view, `Canvas`, per-window canvases, the draw scope (builtin 92), the compositor and recomposition. Client coordinates remained absolute screen coordinates translated by the surface origin until M17. |
| 2026-11-27 | M17 implements the local-coordinate model (spec 0026 "Implementation status (M17)"): client drawing, `get_pixel` inside a scope and window-addressed pointer events are window-local (G23/G24 amended). Because `fill_dither`'s parity is surface-local, the deterministic golden was re-baselined once (`c9546f45…79fbc65e` → `62e8140a…e809d83c`); the diff is the 130×100 dither patch in the `visual` specimen only. No drawing primitive signature, opcode or builtin change; ABI v1 stays at 75. |
