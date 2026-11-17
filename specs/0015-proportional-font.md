# 0015: The proportional font

- **Status:** Implemented
- **Scope:** `kernel` (`font`, `gfx`, `window`, `console`), `abi`, `compiler`,
  `interpreter`, `langhost`, `samples/apps`; M2.6.1 of the desktop milestone
- **Supersedes:** none
- **Superseded by:** none
- **ADRs:** 0030
- **Updated:** 2026-11-17

## Context

The first desktop slices used one 8×8 monochrome font for everything: the console,
window titles, the menu bar, client text and the `stdlib` UI helpers. The System 7.5
look the desktop imitates uses a proportional system face (Chicago) for its menu bar
and window titles, and a fixed-width bitmap makes those bands look mechanical: `ii`
takes as much room as `MM`, and a 16-pixel title bar only fits three or four wide
letters.

M2.6.1 adds a small **proportional** bitmap font for the chrome and menu-bar text
without disturbing the 8×8 path: the console, `syslib.draw_text` and every existing
`stdlib.ui_*` helper must keep their exact behaviour and metrics, because their tests
pin pixels and sizes. The font is a first-party, static asset: no file loading, no
allocation, no vector rasteriser and no second font engine.

Non-goals:

- no Unicode coverage beyond printable ASCII (`0x20..=0x7E`); other bytes fall back;
- no bold, italic, condensed or scaled variants;
- no anti-aliasing or sub-pixel positioning; one bit per pixel, integer advances;
- no per-application font selection and no font files;
- no new bytecode opcode: the three new primitives are host builtins;
- no change to `syslib.draw_text` (the 8×8 path), the console or `stdlib.ui_*`.

## Requirements

- **F1 (MUST):** The proportional font covers every printable ASCII byte
  `0x20..=0x7E`. Its line box is **12 pixels** tall, with the baseline at row 9
  (0-based from the top of the box): cap height 8 (rows 2..=9), x-height 6 (rows 4..=9),
  ascenders from row 1, descenders rows 10..=11. Stems are one pixel.
- **F2 (MUST):** The font is a kernel-owned static table: one 10-column ASCII-art
  definition and one advance per printable byte. Advances are variable and include the
  right bearing; the widest glyphs (`M` = 11, `W` = 10) are visibly wider than the
  narrowest (`i`, `l`, `.` = 3).
- **F3 (MUST):** A **glyph cache** renders each glyph on first use into a row bitmask
  plus its ink bounds, and keeps it for the system's lifetime. The cache is fixed-size,
  allocation-free and serialised by a spin lock. Metrics depend only on the table, not
  on whether the cache is warm.
- **F4 (MUST):** The boot self-check validates all 95 definitions (12 rows of 10
  columns), every rendered glyph (ink inside the box, `advance >= ink width`), the
  variable-width relations (`ii` narrower than `MM` and `Wi`), the 8×8 fallback advance
  and the line height. A mismatch prints `font: glyphs FAIL at step N` and halts.
- **F5 (MUST):** `syslib.text_width_prop(text) -> uint` returns the sum of the byte
  advances in pixels; `syslib.text_height_prop() -> uint` returns the 12-pixel line
  height. A byte outside `0x20..=0x7E` contributes **8** pixels, including each UTF-8
  byte of a non-ASCII code point.
- **F6 (MUST):** `syslib.draw_text_prop(x, y, text, color) -> Status` draws with a
  transparent background, `y` being the top of the line box. Printable ASCII uses its
  proportional glyph and advance; any other byte keeps an 8×8 console-font cell with an
  8-pixel advance, drawn two rows down so its ink sits on the proportional baseline;
  bytes at or above `0x80` draw a blank 8×8 cell. A colour above `0x00FF_FFFF` is
  `Status.Invalid` and draws nothing. The screen and the clip bound every pixel: an
  off-screen or partly visible origin is clipped, never an error, and never faults.
- **F7 (MUST):** With graphics disabled, `text_width_prop` and `text_height_prop`
  return 0 and `draw_text_prop` returns `Status.Invalid`; the host defaults model the
  same machine, so `langhost`, host tests and a framebuffer-less kernel agree.
- **F8 (MUST):** The 8×8 path is unchanged: `syslib.draw_text` keeps its glyphs,
  advance and metrics; the console's terminal and system-log grids and every
  `stdlib.ui_*` helper keep their 8×8 drawing and metrics. `draw_text_prop` is
  **additional**, not a replacement.
- **F9 (MUST):** M2.6.1 integrates the proportional font in two visible places: the
  window service's title-bar text (`window::draw_chrome`, truncated and centred by
  measured width) and the desktop shell's menu-bar labels and clock. Client body text,
  the console and the `ui_*` toolkit remain 8×8 in this slice.
- **F10 (MUST):** The three primitives are host builtins `45`–`47`, with compiler
  signature checks and interpreter operand decoding; ABI v1 stays at **75** opcodes.
- **F11 (MUST):** The kernel lock order stays acyclic. Drawing holds `SCREEN` and may
  take the font cache (`SCREEN → FONT`); no path takes the font cache and then
  `SCREEN`. The console/window lock order is unchanged.

## Design

### Glyph box and advances

```text
row  0   (blank, tight top)
row  1   ascender top (b d f h i j k l t)
row  2   cap top / digit top
...
row  9   baseline (caps, x-height and digits sit here)
row 10   descender
row 11   descender tail
```

The glyph art is stored as a 10-column grid; ink usually starts at column 0 or 1 and
ends before the advance. The advance is `rightmost_ink + 2` for most glyphs (one pixel
of right bearing), with tabular digits fixed at 6 and a space at 4. The cache trims the
ink to `left`/`width` so the draw loop touches only inked columns.

Selected advances (the self-check and `samples/apps/font` pin these):

| Bytes | Advance |
|-------|---------|
| `i`, `l`, `.` | 3 |
| `f`, `r` | 5 |
| most lowercase, most capitals, digits | 6 (digits tabular) |
| `m`, `w`, `M`, `W` | 7, 8, 11, 10 |
| space | 4 |

### Cache

`kernel/src/font.rs` holds `SOURCE: [Source; 95]` (advance plus ASCII art) and

```rust
struct Cache { slots: [Option<Glyph>; 95] }
static CACHE: SpinLock<Cache> = SpinLock::new(Cache { slots: [None; 95] });
```

`font::glyph(byte)` takes the lock, returns a cached `Glyph` or renders the source
(rows → `u16` masks, ink bounds) and stores it. The lock is held only while looking up or
rendering one glyph. `text_width` and the boot check use the same table. No heap, no
lazy static, no data beyond the 95 entries (about 3 KiB of `.bss`) and the source
strings (about 12 KiB of `.rodata`).

### Drawing and fallback

`gfx::Screen::text_prop` walks the bytes with a pen: a printable byte draws its cached
rows at the pen and advances by the glyph advance; any other byte draws the 8×8 cell at
`y + 2` (baseline-aligned) with an 8-pixel advance. Every pixel goes through the existing
clipped `put`, so an off-screen origin, a partly visible glyph or a long string cannot
fault or wrap. There is no background fill, so text composes over the shadow like
`draw_text`.

### Integration

- **Window chrome:** the title prefix is chosen by measured width (not byte count), the
  band is repainted 12 rows tall at `frame.y + 2`, and the title is centred by the
  measured width. A title that does not fit is truncated on a byte boundary.
- **Desktop shell:** the four direct items and the `HH:MM` clock use
  `draw_text_prop`/`text_width_prop`; the item hit ranges are derived from the same
  measured widths (`menu_file_x`…`menu_terminal_end`), so a label change moves the
  drawn text and the hit boxes together.
- **Not integrated:** the console grids, `syslib.draw_text`, the `stdlib.ui_*` toolkit,
  the editor/browser/widgets body text and the kernel terminal stay 8×8. Proportional
  client text is a later, separately proved change.

## Interfaces

```text
syslib.text_width_prop(text: const string[0]) -> uint   // 0 when graphics are disabled
syslib.text_height_prop() -> uint                       // 12, or 0 when disabled
syslib.draw_text_prop(x: int, y: int, text: const string[0], color: uint) -> Status
```

The language signatures are normative. The host contract:

```rust
fn text_width_prop(&mut self, text: &[u8]) -> u64 { 0 }
fn text_height_prop(&mut self) -> u64 { 0 }
fn draw_text_prop(&mut self, x: i64, y: i64, text: &[u8], color: u32) -> Status {
    Status::Invalid
}
```

Builtins: `TEXT_WIDTH_PROP` (45, 1 → 1), `TEXT_HEIGHT_PROP` (46, 0 → 1) and
`DRAW_TEXT_PROP` (47, 4 → 1). The interpreter resolves the text reference with the
same bounds checks as `draw_text` and pushes the measured width/height or the status.

Kernel-internal helpers (no application binding): `font::LINE_HEIGHT`,
`font::advance`, `font::text_width`, `gfx::prop_advance`, `gfx::PROP_LINE_HEIGHT`,
`gfx::draw_text_prop` and the screen-checked `gfx::text_width_prop`/
`gfx::text_height_prop`.

## Failure modes

| Situation | Behaviour |
|-----------|-----------|
| Byte outside `0x20..=0x7E` | 8-pixel advance; an 8×8 blank cell for `>= 0x80`, the 8×8 glyph otherwise |
| Non-ASCII UTF-8 code point | Each byte is an 8-pixel blank cell (`é` = 16) |
| Off-screen or partly visible origin | Clipped; `Status.Ok`; never faults |
| Colour above `0x00FF_FFFF` | `Status.Invalid`, nothing drawn |
| Graphics disabled | Metrics 0, drawing `Invalid` |
| Malformed font table | Boot `font: glyphs FAIL at step N`, halt |
| A title wider than the title bar | Truncated by measured width |
| Two cores draw at once | The graphics lock serialises; the font lock only guards the cache (`SCREEN → FONT`) |

## Open questions

| # | Question | Trigger / owner |
|---|----------|-----------------|
| 1 | Proportional client text and `stdlib.ui_*` metrics | When the UI toolkit moves beyond the 8×8 grid (M2.6.x) |
| 2 | Unicode coverage and a code-point fallback glyph | When applications need non-Latin titles or body text |
| 3 | Bold/italic variants | Only if the desktop needs a second weight or emphasis |

## Changelog

| Date | Change |
|------|--------|
| 2026-11-17 | Initial version: 12-pixel proportional bitmap font for printable ASCII, lazy glyph cache, three host builtins (45–47), 8×8 fallback, window-chrome and desktop-menu integration, the `font` proof and its screenshot. |
