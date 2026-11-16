# 0009: Graphics, the shadow screen and the window service

- **Status:** Accepted
- **Scope:** `abi`, `compiler`, `interpreter`, `syslib`, `kernel` (`gfx`, `console`),
  `arch/x86_64` framebuffer access; M2 of the desktop milestone
- **Supersedes:** none
- **Superseded by:** none
- **ADRs:** 0020, 0024
- **Updated:** 2026-11-16

## Context

The first milestone drew text straight into the linear framebuffer from
`kernel/src/console.rs`: one `fill_rect` per glyph pixel, no stored image, no clipping.
That is enough for a boot log, but the next milestone is a retro desktop (Mac OS
System 7.5 look), and every later piece — a desktop background, windows, menus, a mouse
pointer, `expose`-driven repaint — needs one image the kernel owns and can copy from.

The chosen shape (ADR 0020) is a kernel-owned **shadow screen**: a full-screen pixel
buffer in the frame allocator's memory that is the single source of truth. Applications
draw into it through `syslib`; `present` blits a rectangle to the real framebuffer; the
text console draws through it too, so there is exactly one image and no second drawing
path.

This spec defines the drawing layer only. The window service that will sit on top of it
(mouse, events, windows, z-order, `expose`) is described in "Planned M2 shape" as a
forward reference; it is not implemented here.

Non-goals for this slice:

- no mouse, keyboard events, windows, focus, z-order, or `expose`;
- no per-window canvases, no alpha blending, no scaling, no font beyond the existing
  8×8 console font;
- no new bytecode opcode: the primitives are host builtins behind `BUILTIN`;
- no compile-time screen size and no fixed `.bss` shadow.

## Requirements

Coordinates are pixels, the origin is the top-left corner, `x` grows right and `y`
grows down. Colours are `0x00RRGGBB`.

- **G1 (MUST):** The kernel keeps the multiboot2 framebuffer information (address,
  pitch, width, height, bpp, type) in one graphics module, not in the console.
- **G2 (MUST):** The screen mode is whatever the bootloader returns. Nothing in the
  kernel or the language may assume 1024×768: the shadow is sized from the actual
  `pitch * height`, and `syslib.screen_width()`/`screen_height()` report the actual
  geometry.
- **G3 (MUST):** Only 32 bpp RGB (`bpp == 32`, framebuffer type 1) is supported. Any
  other mode disables graphics; the console keeps working through its text path, which
  in this slice is serial only (the kernel never draws text into an unsupported mode).
- **G4 (MUST):** The shadow is one full-screen buffer allocated from the frame
  allocator at boot, `pitch * height` bytes of physically contiguous frames. It is not
  a static `.bss` array and the kernel does not allocate one per window.
- **G5 (MUST):** The shadow is the single source of truth. Drawing primitives write
  only the shadow; `present` copies a rectangle to the real framebuffer row by row,
  using the framebuffer pitch (not `width * 4`), and only within the clipped rectangle.
  `present` then composites the software cursor on top of the framebuffer copy; the
  cursor is never stored in the shadow (spec 0010, I7).
- **G6 (MUST):** `syslib.clear(color)` fills the whole screen in the shadow and presents
  it. Other drawing primitives do not present.
- **G7 (MUST):** Clipping is a rectangular region, initially the whole screen, applied
  to every drawing primitive (`clear` is a whole-screen operation and ignores it). The
  region is an internal kernel facility so the window manager can narrow it later; this
  slice does not expose it to applications.
- **G8 (MUST):** Drawing is clipped, never faulted. Coordinates outside the screen are
  legal and produce no pixels outside the screen. An application cannot fault the
  kernel with a coordinate that overflows the screen.
- **G9 (MUST):** Out-of-range arguments return `Status.Invalid` and draw nothing:
  `w <= 0` or `h <= 0` on a rectangle primitive, and a colour with any bit above bit 23
  set. `get_pixel` has no error path and returns `0` outside the screen.
- **G10 (MUST):** The `syslib` graphics surface is exactly the small orthogonal set in
  "Interfaces": `screen_width`, `screen_height`, `fill_rect`, `fill_dither`,
  `draw_rect`, `draw_text`, `get_pixel`, `present`, `clear`.
- **G11 (MUST):** `fill_dither` writes `color` to every other pixel of its extent — the
  pixel `(X, Y)` when `(X + Y)` is even — and leaves the other pixels untouched. This
  is the System 7 desktop background when a white `fill_rect` precedes it.
- **G12 (MUST):** `draw_rect` is a one-pixel outline inside the given extent; the
  interior is untouched.
- **G13 (MUST):** `draw_text` draws 8×8 glyphs from the console font, one byte per
  cell, advancing `x` by 8 per byte, with a transparent background. It does no newline
  processing and no wrapping; a byte outside `0x00..=0x7F` draws a blank cell.
- **G14 (MUST):** The text console draws glyphs into the shadow and presents the rows
  it touched. Kernel `print!`/`println!` output therefore has one path to the screen.
  The existing console lock and line atomicity are preserved across cores.
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

## Design

### Shadow and presentation

```
application ──syslib.fill_rect/...──▶ shadow (pitch × height, frame allocator)
                                          │
                          syslib.present  │  row-by-row copy, pitch aware
                                          ▼
                                   real framebuffer
```

- The shadow row stride equals the framebuffer pitch, so a row is one contiguous run and
  `present` is a per-row copy of `w * 4` bytes. Padding bytes beyond `width * 4` are
  allocated but never drawn or presented.
- Every pixel write goes through one helper that bounds-checks against the screen and
  the current clip rectangle. `get_pixel` reads the same helper's storage, so the
  demo's self-check sees exactly what `present` will show.
- `present` ends by compositing the software cursor into the framebuffer (spec 0010,
  I7): black outline and white fill pixels are written straight to the framebuffer
  inside the presented rectangle, never into the shadow. `get_pixel` therefore returns
  uncomposited pixels, which is what keeps the self-check meaningful and headless
  verification deterministic.
- A full-screen shadow at 1024×768×32 is 3 MiB (768 frames at 4 KiB). Nothing else in
  the kernel allocates per-pixel storage.

### Clipping

`clip` is a rectangle `(x, y, w, h)` in screen coordinates, initialised to the full
screen and intersected with the screen bounds on every set. A primitive first rejects
`w <= 0`/`h <= 0`, then converts to `i64`, intersects the rectangle with the clip, and
draws only the result. The window manager will narrow the clip to one window while it
repaints that window; the drawing code does not change.

### Console

`kernel/src/console.rs` keeps its line discipline and lock but loses its direct
framebuffer access. Each glyph is painted into the shadow with an opaque background and
its 16×16 cell is presented; scrolling copies the shadow up by 16 rows, clears the
exposed band and presents the screen once. The console draws through the same helper as
`syslib`, so a partially drawn rule or a desktop underneath is never in a different
image from what an application reads.

Text before `gfx::init` runs is serial-only. `kmain` initialises the frame allocator and
graphics before the console attaches, so the normal boot log is on screen from the first
line.

### Planned M2 shape (forward reference, not implemented)

| Slice | Content | Uses this slice |
|-------|---------|-----------------|
| M2.2 | PS/2 mouse, a composited cursor, pointer state and the `syslib` pointer surface (spec 0010); the console stays a kernel device | shadow, `present`, clip |
| M2.3 | Kernel window service (spec 0011): windows with a frame, title bar, content rectangle and z-order, single focus and task ownership (M2.3.1); then frame chrome, damage union and `present` (M2.3.2), and event routing plus `expose` repaint under the service-owned clip (M2.3.3) | shadow, clip, `present` |

The window service repaints on `expose`; it does not keep per-window canvases. An
application redraws its window's content into the clipped shadow when asked, and the
service decides when to present. Mouse/event delivery and the exact `expose` protocol
belong to M2.2/M2.3 and will be specified there.

## Interfaces

### Language surface

```text
syslib.screen_width() -> uint            // 0 when graphics are disabled
syslib.screen_height() -> uint           // 0 when graphics are disabled
syslib.fill_rect(x: int, y: int, w: int, h: int, color: uint) -> Status
syslib.fill_dither(x: int, y: int, w: int, h: int, color: uint) -> Status
syslib.draw_rect(x: int, y: int, w: int, h: int, color: uint) -> Status
syslib.draw_text(x: int, y: int, text: const string[0], color: uint) -> Status
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
fn get_pixel(&mut self, x: i64, y: i64) -> u32 { 0 }
fn present(&mut self, x: i64, y: i64, w: i64, h: i64) -> Status { Status::Invalid }
fn clear(&mut self, color: u32) -> Status { Status::Invalid }
```

The kernel implements these over `kernel/src/gfx.rs`. The defaults describe a machine
with no screen, which is also the kernel's disabled state, so both paths agree.

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
| Colour above `0x00FFFFFF` | `Status.Invalid`, nothing drawn |
| Coordinates wholly outside the screen | `Ok`, nothing drawn |
| Partly outside the screen or the clip | Clipped; `Ok` |
| `get_pixel` outside the screen | `0` |
| `draw_text` with a non-ASCII byte | Blank 8×8 cell, advance 8; `Ok` |
| Graphics disabled and a primitive is called | `Status.Invalid`; the application may ignore it |
| Two cores draw at once | The graphics lock serialises each primitive; output interleaves at primitive boundaries |
| Screen mode is larger than 1024×768 | Works: shadow and clip derive from the actual geometry |

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
| 4 | A second font or a proportional font | only if the desktop needs one |

## Changelog

| Date | Change |
|------|--------|
| 2026-11-16 | Initial accepted version: shadow screen, clipping, the nine `syslib` primitives, console through the shadow, disabled-graphics rules, and forward references to M2.2/M2.3. |
| 2026-11-16 | M2.2: `present` composites the software cursor into the framebuffer after the row copy (G5), so `get_pixel` stays uncomposited and headless self-checks stay deterministic; the cursor model itself is spec 0010. M2.2 row of the forward-reference table updated; cursor question resolved. |
| 2026-11-16 | M2.3.0/M2.3.1: spec 0011 and ADR 0024 fix the window model (content-size geometry, one z-order, one focus, ownership, 16/64/8×8 limits). `gfx` gains the kernel-internal `set_clip`/`reset_clip` used by the service, with a boot self-check (`graphics: clip ok`); no application clip primitive is added (open question 1 resolved). The `syslib` drawing surface is unchanged; chrome, damage and `present` of a union are M2.3.2. |
| 2026-11-16 | M2.3.2: the window service draws chrome and restores the desktop through the same shadow and internal clip (narrowed per visible part), then presents the damage union once; `syslib.present`/`clear` and the nine drawing primitives are unchanged. The service desktop is white with the `(X + Y)` black dither. `window: paint ok` checks the damage arithmetic at boot. |
