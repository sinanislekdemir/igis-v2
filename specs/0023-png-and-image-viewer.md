# 0023: The PNG corpus and the image viewer

- **Status:** Accepted
- **Scope:** `samples/apps/lib` (`bitio`, `inflate`, `png`), `samples/apps/apps`
  (`image_viewer`), `compiler`/`interpreter`/`abi` for the generic primitives,
  `kernel` VFS (read-only `image/`), `langhost`, `xtask`, `scripts`
- **Supersedes:** none
- **Superseded by:** none
- **ADRs:** 0057
- **Updated:** 2026-11-21

## Context

The image-viewer work started from a JPEG corpus, but a JPEG decoder needs a
large integer transform, a Huffman engine and substantial decoder state. PNG is
simpler, lossless and already split into the pieces this system wants anyway: a
bit reader, a DEFLATE decoder and a row filter. The corpus is therefore converted
to PNG and the viewer is built from generic primitives.

The applications image is read-only and its size was fixed at 4 MiB. The
full-resolution corpus is about 46 MB, so the image must grow to 64 MiB. The
corpus keeps its original dimensions; the viewer fits it to the window by
decoding less, not by resampling the pixels.

This specification covers the PNG corpus, the decoder contract, the planned
generic primitives and the viewer. It does **not** cover JPEG, animation, 16-bit
colour, gamma/colour-management or a kernel codec.

## Requirements

### Corpus

- **P1 (MUST):** PNG decoding lives entirely in `/lib`; no kernel codec, opcode
  or VFS special case is added for an image format.
- **P2 (MUST):** The corpus lives at `image/` in the application image and is
  staged from `samples/image`. The apps image is 64 MiB for both the product and
  the `--shell` image.
- **P3 (MUST):** Every corpus file is an 8-bit RGB (colour type 2),
  non-premultiplied, full-resolution PNG with Adam7 interlace. Files are converted
  at their original dimensions; no file is resized.
- **P4 (MUST):** The decoder accepts an 8-bit colour type 0 (grey), 2 (RGB),
  3 (palette), 4 (grey+alpha) or 6 (RGBA) image. 16-bit and other bit depths are
  rejected with `Invalid` rather than truncated.
- **P5 (MUST):** The decoder implements filters 0–4 for every row.
- **P6 (MUST):** zlib/DEFLATE is decoded by a generic streaming `lib.inflate`
  module, not by PNG-specific code.
- **P7 (MUST):** For an interlaced image the preview decodes Adam7 pass 1 only,
  producing `ceil(width/8) x ceil(height/8)` pixels. This is the fit-to-window
  preview.
- **P8 (MUST):** A non-interlaced image is decoded one row at a time, in order.
- **P9 (MUST):** The decoder exposes a row-streaming API (`png.open` then
  `png.next_row`, or an equivalent contract). It MUST NOT allocate or expose a
  full-image RGB buffer.
- **P10 (MUST):** Decoder state is bounded: one row buffer plus a 32 KiB inflate
  window. A decoded row is at most `3 * width` bytes for colour type 2 (and the
  corresponding size for the other accepted types).
- **P11 (MUST):** An alpha channel is composited over white, and a palette is
  expanded to RGB, so every decoded row is opaque RGB.
- **P12 (MUST):** PNG chunk CRC fields are not verified; a malformed stream is
  detected by structure, bounds and the zlib checksum.
- **P13 (MUST):** Decode failures use the stable statuses `NotFound` (no such
  file), `Invalid` (not a PNG, unsupported bit depth/colour type, malformed
  stream), `CapacityExceeded` (a row or the caller's buffer is too small) and
  `IoError` (the file read failed).

### Generic primitives

- **G1 (MUST):** The generic layers are separate tickets from the PNG decoder and
  are format-independent: `lib.bitio`, `lib.inflate`,
  `core.bit_and`/`bit_or`/`bit_xor`/`bit_shl`/`bit_shr`,
  `core.copy_bytes`/`core.fill_bytes`, and `syslib.blit_rgb`.
- **G2 (MUST):** `lib.inflate` is a streaming zlib/DEFLATE decoder over a bounded
  window and is reusable outside PNG.

### Viewer

- **V1 (MUST):** `apps/accessories/image_viewer` opens a PNG, decodes Adam7 pass 1
  and fits that preview to the window for the corpus. It does not resample the
  source file.
- **V2 (MUST):** Drawing streams rows into a bounded window-content buffer; the
  viewer does not keep the full image.
- **V3 (MUST):** The viewer supports `check` and `shot` modes like the other GUI
  applications, and its file dialog starts at `image`.
- **V4 (MUST):** The viewer is listed under `Accessories` in the Apps menu.

## Design

The pipeline is layered so each stage is testable on the host:

```
image/<name>.png  →  syslib file reads  →  lib.png  →  lib.inflate  →  lib.bitio
                         (8 KiB chunks)      │
                                             ├─ chunk walk, IHDR/PLTE/IDAT
                                             ├─ filter undo (0–4)
                                             └─ row / Adam7 pass-1 assembly
                                                        │
                                                 syslib.blit_rgb
```

`png.open` reads and validates the signature and the header, keeps the geometry,
colour type and interlace flag, and initialises the inflater over the concatenated
`IDAT` stream. `png.next_row` yields one row, undoing the filter with the previous
row for filters 1–4. For interlaced input it runs only pass 1, whose seven
sub-images collapse to one `ceil(w/8) x ceil(h/8)` preview; the pass-7 data after
the first pass is ignored. For non-interlaced input it yields rows in order.

The row buffer is the only image-shaped allocation. The inflater keeps a 32 KiB
window, so memory is bounded by the window width, not by the file size. The
viewer maps decoded rows to the window content with `syslib.blit_rgb`.

## Interfaces

Planned module functions (names may gain a trailing argument, but not change
shape):

| Module | Contract |
|---|---|
| `lib.bitio` | bounded LSB-first bit reader over a caller buffer |
| `lib.inflate` | `open`, `read` into a caller buffer, `close`; 32 KiB window |
| `lib.png` | `open(path, info)`, `next_row(row, got)`, `close`; opaque RGB rows |
| `syslib.blit_rgb` | blit one opaque RGB row into the window content |

## Failure modes

| Condition | Result |
|---|---|
| File missing | `NotFound` |
| Not a PNG signature, bad chunk layout, unsupported bit depth/colour type | `Invalid` |
| Truncated or corrupt DEFLATE stream | `Invalid` |
| Row wider than the caller's buffer or the decoder bound | `CapacityExceeded` |
| File read error | `IoError` |
| Interlaced image | pass 1 only for the preview; full passes not decoded |
| Alpha | composited over white |
| Palette | expanded to RGB |
| CRC mismatch | ignored (P12) |

## Verification

- Host tests for `lib.bitio`, `lib.inflate` and `lib.png` against fixed PNG
  fixtures, including every filter, colour type and interlace mode.
- A `--shell` kernel transcript proves the viewer's `check` path over the real
  `image/` corpus.
- A QMP screenshot proves the rendered preview, and `-smp 4`/`-smp 1` both pass.
- The committed visual golden is unchanged.

## Non-goals

- JPEG and other formats.
- Animation, APNG, 16-bit colour, gamma or colour management.
- Resizing or resampling the corpus.
- A kernel image codec.
- Full-resolution progressive display of interlaced images beyond pass 1.

## History

| Date | Change |
|---|---|
| 2026-11-21 | Accepted with the PNG corpus, the `/lib` row-streaming decoder contract, the generic primitive tickets and the fit-to-window viewer using Adam7 pass 1 (ADR 0057). |
