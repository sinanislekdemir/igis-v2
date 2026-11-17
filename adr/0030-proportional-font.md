# 0030: A kernel-owned 12-pixel proportional bitmap font with a lazy glyph cache

- **Status:** Accepted
- **Date:** 2026-11-17
- **Deciders:** igis-v2 kernel/UI
- **Related specs:** 0015, 0003, 0009, 0012

## Context

The desktop milestone reached a point where every text surface used the single 8×8
console font: window titles, the menu bar, client controls and the terminal. The
System 7.5 look it imitates uses a proportional system face (Chicago) for chrome, and a
fixed-width bitmap makes the title bar and menu bar look mechanical. The kernel has no
font loader, no allocation and no vector rasteriser, and the existing 8×8 path is
pinned by console grids, `syslib.draw_text` and dozens of `stdlib.ui_*` pixel tests.

The choice is therefore not "which font file" but how to add a proportional face
without a new subsystem, while keeping the 8×8 path and its tests intact.

## Decision

We will add a **first-party, kernel-owned 12-pixel proportional bitmap font** for
printable ASCII (`0x20..=0x7E`), stored as 10-column ASCII art plus a per-glyph advance,
and rendered lazily into a fixed spin-locked **glyph cache** of 95 row bitmasks. Its
three primitives are host builtins behind `syslib`:

```text
text_width_prop(text) -> uint      // sum of advances; 8 per fallback byte
text_height_prop() -> uint         // 12
draw_text_prop(x, y, text, color) -> Status
```

`syslib.draw_text`, the console grids and every `stdlib.ui_*` helper stay 8×8. M2.6.1
integrates the proportional face only in the window-service title bar and the desktop
shell's menu bar/clock; client body text moves later, with its own proof.

## Consequences

### Positive

- The title bar and menu bar read as a proportional system UI while the console and
  the UI toolkit keep their tested 8×8 behaviour.
- No new opcode, allocator, font file, loader or rasteriser; the data is static and the
  cache is about 3 KiB of `.bss`.
- Metrics and glyph rendering share one source of truth, and the boot self-check
  validates every glyph before an application draws.
- The 8-pixel fallback gives every byte a deterministic advance, so mixed text cannot
  fault or change width between hosts.

### Negative

- The source table is about 12 KiB of readable ASCII art rather than a compact binary
  blob; changing a glyph changes a source file.
- The font is fixed: no scaling, bold/italic or Unicode. A later proportional client
  toolkit will need a second decision on metrics and coverage.
- The cache is taken under the graphics lock (`SCREEN → FONT`); a future path that
  needs the font outside graphics must not take `SCREEN` after `FONT`.

### Neutral / follow-up work

- Client text and `stdlib.ui_*` proportional metrics are open (spec 0015, question 1).
- The console stays 8×8 by design; a proportional terminal would be a separate spec.
- The `font` sample and `scripts/font_screenshot_qmp.py` pin the metrics, a narrow and
  a wide glyph, the fallback and the chrome integration.

## Alternatives considered

| Alternative | Why not |
|-------------|---------|
| Trim the existing 8×8 glyphs to their ink and call that proportional | `i` becomes a one-pixel stem without a proper dot, wide letters keep blocky 8×8 shapes, and it is not a Chicago-like face |
| A vector font plus a rasteriser | A new subsystem (outlines, curves, scan conversion) for a retro 1-bit desktop; far more code and test surface |
| A generated compact bit table with no ASCII-art source | Smaller `.rodata`, but the glyphs become opaque hex and the cache would have nothing meaningful to render |
| A font file on the ext2 image | The font is trusted kernel chrome, not application data; loading it would add a parser and boot-order dependency |
| Move `stdlib.ui_*` to the proportional font in the same slice | Breaks pinned metrics/pixels in dozens of tests and mixes a visual change with the chrome change; deferred to its own slice |
