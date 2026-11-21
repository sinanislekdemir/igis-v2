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
- **P6 (MUST):** zlib/DEFLATE is decoded by a generic `lib.inflate` module, not
  by PNG-specific code. The PNG layer stages at most 512 KiB of `IDAT` payload in
  its own state and hands that prefix to the inflater, which returns up to 4 KiB
  of output per call; the decoder never holds the whole `IDAT` stream.
- **P7 (MUST):** For an interlaced image the preview decodes Adam7 pass 1 only,
  producing `ceil(width/8) x ceil(height/8)` pixels. This is the fit-to-window
  preview.
- **P8 (MUST):** A non-interlaced image is decoded one row at a time, in order.
- **P9 (MUST):** The decoder exposes a row-streaming API: `png.probe` reads the
  signature and IHDR only, `png.open` prepares a decoder, `png.info` copies its
  geometry, and `png.next_row` yields one row at a time. It MUST NOT allocate or
  expose a full-image RGB buffer.
- **P10 (MUST):** Decoder state is bounded: two source rows of 20,480 bytes
  (previous/current, for filters), a 32 KiB inflate window plus its canonical
  Huffman tables (40,816 bytes, about 40 KiB, for the inflater in total), a
  4,096-byte inflater-output staging buffer, a 768-byte palette, a 256-byte
  `tRNS` table, the staged compressed prefix (at most 512 KiB), and scalars. The
  caller's RGB row is 16,384 bytes (at most 5,461 pixels). A decoded row is at
  most `3 * width` bytes for colour type 2 (and the corresponding size for the
  other accepted types). There is no full-image buffer.
- **P11 (MUST):** An alpha channel is composited over white, and a palette is
  expanded to RGB, so every decoded row is opaque RGB. A palette `tRNS` alpha is
  supported; for colour types 0 and 2 a `tRNS` chunk is ignored and the row
  stays opaque.
- **P12 (MUST):** PNG chunk CRC fields and the zlib Adler-32 trailer are not
  verified (an early stop for the pass-1 preview cannot see the trailer); a
  malformed stream is detected by structure and bounds.
- **P13 (MUST):** Decode failures use the stable statuses `NotFound` (no such
  file), `Invalid` (not a PNG, unsupported bit depth/colour type, malformed
  stream, or a truncated IDAT that ended before the requested row),
  `CapacityExceeded` (a row or the caller's buffer is too small, or the
  inflater needed more input than a prefix that filled its 512 KiB stage) and
  `IoError` (the file read failed).

### Generic primitives

- **G1 (MUST):** The generic layers are separate tickets from the PNG decoder and
  are format-independent: `lib.bitio`, `lib.inflate`,
  `core.bit_and`/`bit_or`/`bit_xor`/`shl`/`shr`,
  `core.copy_bytes`/`core.fill_bytes`, and `syslib.blit_rgb`.
- **G2 (MUST):** `lib.inflate` is a bounded-input, bounded-output zlib/DEFLATE
  (RFC 1950/1951) decoder and is reusable outside PNG. The caller supplies a
  compressed prefix and receives at most 4 KiB of output per call; the decoder
  keeps a 32 KiB window plus tables, never a full image, and does not verify the
  zlib Adler-32 trailer.

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
colour type and interlace flag, and stages up to 512 KiB of concatenated `IDAT`
payload in the decoder state. `png.next_row` yields one row, undoing the filter
with the previous row for filters 1–4. For interlaced input it runs only pass 1,
whose seven sub-images collapse to one `ceil(w/8) x ceil(h/8)` preview; the
pass-7 data after the first pass is ignored. For non-interlaced input it yields
rows in order.

**Why a bounded compressed prefix.** The inflater takes the whole staged prefix
and a length, not a stream endpoint: `lib.inflate.read` decodes from that prefix
and returns at most 4 KiB of output. The PNG layer can therefore stage a fixed
size (512 KiB; the measured corpus maximum for the Adam7 pass-1 prefix is
368,317 bytes) and call the decoder repeatedly. A prefix that is too short for
the next block or symbol is `WouldBlock` with no state lost, so passing a grown
prefix later resumes exactly where it stopped; a prefix larger than the staging
buffer is `CapacityExceeded` in the PNG layer. The alternative (a full
`IDAT` buffer) would make the decoder state grow with the file, and the
interlaced preview already stops after pass 1.

The row buffer is the only image-shaped allocation. The inflater keeps a 32 KiB
window and its canonical Huffman tables (40,816 bytes in total), so memory is
bounded by the window width and the staging prefix, not by the image area. The
viewer maps decoded rows to the window content with `syslib.blit_rgb`.

## Interfaces

Module functions (`lib.bitio` and `lib.inflate` are implemented; the rest are
planned; names may gain a trailing argument, but not change shape):

| Module | Contract |
|---|---|
| `lib.bitio` | bounded LSB-first bit reader over a caller buffer (below) |
| `lib.inflate` | `init`, `done`, `read` from a compressed prefix into a 4 KiB caller buffer; 32 KiB window + tables (below) |
| `lib.png` | `probe`, `open`, `info`, `next_row`; opaque RGB rows (below) |
| `syslib.blit_rgb` | blit one opaque RGB row into the window content |

### `lib.bitio` (implemented)

`samples/apps/lib/bitio.ig` is an ordinary `/lib` module (M5/M6): no `main`, no
top-level state, no stream endpoint, no new builtin or opcode. It uses
`core.shl`/`core.shr` for the shifts and `core.copy_bytes` for the window copy,
because the language has no bitwise operators.

Bit order is **LSB-first**: the first bit read is bit 0 of the next byte, and
`read_bits(n)` returns those n bits with the first bit in bit 0. This is the
DEFLATE order: a stored-block length or a header field is read directly, and a
Huffman code is decoded from the bits in the order they arrive. `0xB5, 0x03`
read as 3/5/4 bits gives 5, 22, 3.

```
type Reader = record
  buf: bytes[512];   // byte window
  len: int;          // bytes valid in buf
  pos: int;          // next unread byte
  acc: uint;         // next bits in the low positions
  nbits: int;        // valid bits in acc
end;

function init(r: var Reader): Status;                              // zero the state
function fill(r: var Reader; data: const bytes[0]; offset: uint;
              used: var int): Status;                              // stage bytes
function available(r: const Reader): uint;                         // nbits + 8 * (len - pos)
function ensure(r: var Reader; count: uint): Status;               // buffer >= count bits
function peek(r: var Reader; count: uint; value: var uint): Status;// no consumption
function drop(r: var Reader; count: uint): Status;                 // consume buffered bits
function read_bits(r: var Reader; count: uint; value: var uint): Status; // peek + drop
procedure align(r: var Reader);                                    // skip to byte boundary
function read_byte(r: var Reader; value: var uint): Status;        // one whole byte
```

- **Window.** `fill` copies up to `512 - len` bytes from the caller's
  `data[offset..]` into `buf` by its runtime length; a reader whose bytes were
  all consumed (`pos = len`) is rewound first, so a stream is staged in chunks.
  `used` receives the count. A full window with unread bytes is `Full` with
  `used = 0`. No allocation and no whole-file staging: the caller's buffer is
  the only input.
- **Bits.** `ensure(count)` pulls whole bytes from the window and places each
  above the buffered bits (`acc = acc + byte * shl(1, nbits)`), so `nbits` after
  a successful call is at most 39. An empty window is `WouldBlock` and changes
  nothing, which lets the caller refill and retry. `count <= 32`; a larger
  count is `Invalid` and the accumulator never shifts by more than 63.
- **Consumption.** `peek` masks the accumulator to `count` bits; `drop`
  requires `nbits >= count` (`Invalid` otherwise) and shifts the consumed bits
  out. `read_bits` is `peek` then `drop`, so a failure consumes nothing.
- **Byte alignment.** `align` discards `nbits mod 8` bits, which reaches the
  next byte boundary for a stored block; whatever whole bytes are buffered stay
  in the accumulator. `read_byte` aligns, then drains a whole byte already in
  the accumulator (`value = acc mod 256`, `acc = shr(acc, 8)`, `nbits -= 8`) or
  takes one from the window; it is `WouldBlock` only when the accumulator holds
  no whole byte and the window is empty. `nbits` is always a multiple of 8
  after `align`, so the `Invalid` branch for `0 < nbits < 8` is defensive.
- **Failure rule.** Every failure leaves out-parameters untouched except
  `fill`'s `used` on `Full`, which is defined to be 0. No bad range faults; the
  errors are `WouldBlock`, `Invalid` and `Full`.

### `lib.inflate` (implemented)

`samples/apps/lib/inflate.ig` is an ordinary `/lib` module (M5/M6) over
`lib.bitio`: no `main`, no top-level state, no stream endpoint, no new builtin or
opcode. It decodes zlib (RFC 1950) and DEFLATE (RFC 1951) from a caller-supplied
compressed prefix into bounded output chunks.

```
function init(d: var Inflater): Status;                    // zero the state
function done(d: const Inflater): bool;                    // BFINAL end reached
function read(d: var Inflater; input: const bytes[0]; input_len: uint;
              out: var bytes[4096]; got: var int): Status;
```

- **Prefix.** `read` stages only `input[..input_len]` and advances `d.in_pos`;
  the same buffer is passed on every call and `input_len` may grow. A larger
  `input_len` than `core.len(input)` is `Invalid`.
- **Output.** `read` appends 0..4096 bytes to `out` and stops when fewer than 258
  bytes remain (a DEFLATE match is at most 258), the stream is done, or it needs
  more input. Runs are resumable at symbol and byte boundaries with no lost or
  duplicated output.
- **Statuses.** `Ok` with `got >= 0` is progress or a done stream; `WouldBlock`
  means more compressed input is needed than the prefix provides and nothing is
  lost; `Invalid` is a bad zlib header (CM ≠ 8, CINFO > 7, FDICT set), a bad
  block header or Huffman table, a bad stored-block complement, an invalid
  symbol (286/287, distance 30/31), a bad length or distance, or a distance past
  the produced output. Nothing faults.
- **Blocks.** BTYPE 0 stored (align, LEN/NLEN, resumable byte copy), BTYPE 1 fixed
  Huffman, BTYPE 2 dynamic Huffman (code-length code, repeat codes 16/17/18),
  BTYPE 3 `Invalid`. Canonical mincode/maxcode/valptr tables decode short codes
  from `min(max_bits, available)` bits, so the final padding is not over-read.
- **Window.** A 32 KiB ring holds every produced byte; `wpos` wraps and
  `total_out` bounds a match. A match copies byte by byte from the ring, so
  overlapping matches (distance < length) are correct.
- **No trailer.** The Adler-32 trailer is not read or checked (P12): a caller may
  stop after a prefix, and structural bounds catch corruption.

### `lib.png` (implemented)

`samples/apps/lib/png.ig` is an ordinary `/lib` module (M5/M6) over
`lib.inflate`: no `main`, no top-level state, no stream endpoint, and no kernel
codec or builtin. It decodes row by row and never allocates a full-image RGB
buffer.

```
function png.probe(path: const string[0];
                   width: var uint; height: var uint;
                   out_w: var uint; out_h: var uint;
                   color_type: var uint; interlace: var uint): Status;
function png.open(path: const string[0]; d: var Decoder): Status;
function png.info(d: const Decoder; width: var uint; height: var uint;
                  out_w: var uint; out_h: var uint;
                  color_type: var uint; interlace: var uint): Status;
function png.next_row(d: var Decoder; row: var bytes[16384];
                      got_w: var uint; y: var uint): Status;
```

- **`probe`.** Reads the 8-byte signature and the 13-byte IHDR only, validates
  the supported subset, and computes the output geometry; it does not touch the
  IDAT. `open` re-reads the header, parses `PLTE`/`tRNS`, finds the `IDAT`
  chunks, stages their concatenated payload and initialises the inflater.
  `info` copies the geometry of an opened decoder. `next_row` yields rows top
  to bottom with `got_w = out_w` and `y` = the row index; after `out_h` rows the
  caller stops (a further call is `Invalid`, not a fault).
- **Accepted subset.** 8-bit colour type 0 (grey), 2 (RGB), 3 (palette), 4
  (grey+alpha) and 6 (RGBA). Bit depths other than 8, unknown colour types,
  non-zero compression/filter methods, interlace flags other than 0/1, and
  malformed chunk layouts are `Invalid`.
- **Geometry.** For non-interlaced input `out_w = width`, `out_h = height`.
  For interlaced input only Adam7 pass 1 is decoded, so `out_w =
  ceil(width/8)`, `out_h = ceil(height/8)`; pass 1 is a rectangular sub-image,
  so rows are not scattered. Every configuration checks `out_w > 0`,
  `out_h > 0`, `out_w * 3 <= 16384` and `1 + out_w * bpp <= 20480`
  (`bpp = 1, 3, 1, 2, 4` for colour types 0, 2, 3, 4, 6). A wider preview is
  `CapacityExceeded`.
- **Rows.** One filtered source row is `1 + out_w * bpp` bytes: a filter byte
  and the scanline. Filters 0–4 (None, Sub, Up, Average, Paeth) are undone
  against the previous unfiltered row. The row is then expanded to RGB: colour
  type 0 replicates grey, 2 copies RGB, 3 looks the index up in the palette, 4
  composites grey+alpha over white and 6 composites RGBA over white, with
  `(c*a + 255*(255-a)) div 255`. A palette `tRNS` alpha is applied; for colour
  types 0 and 2 `tRNS` is ignored. Every output row is exactly `out_w * 3`
  opaque RGB bytes.
- **State.** `type Decoder` holds `inflate.Inflater` (40,816 B), the
  `idat: bytes[524288]` stage (512 KiB), `out_buf: bytes[4096]`, `prev` and
  `cur` source rows (`bytes[20480]` each), `palette: bytes[768]`,
  `trns: bytes[256]`, and scalars: 611,352 bytes compiled. It is well under the
  1 MiB state cap. The caller's `row` is `bytes[16384]`.
- **Prefix and statuses.** `inflate.read` is called with the staged
  `(idat, idat_len)` on every row; `in_pos` persists inside the inflater, so a
  row is resumable across calls. `WouldBlock` maps to `CapacityExceeded` when
  the stage was filled to the 512 KiB limit (the file had more IDAT payload),
  and to `Invalid` when the file's IDAT ended and the stream is truncated. A
  bad zlib header, block header, Huffman table or symbol is `Invalid`. Chunk
  CRCs and the zlib Adler-32 are not verified (P12).

## Failure modes

| Condition | Result |
|---|---|
| File missing | `NotFound` |
| Not a PNG signature, bad chunk layout, unsupported bit depth/colour type | `Invalid` |
| Truncated or corrupt DEFLATE stream | `Invalid` at the PNG layer; `lib.inflate` reports `WouldBlock` for a short prefix and `Invalid` for bad structure |
| Row wider than the caller's buffer or the decoder bound | `CapacityExceeded` |
| File read error | `IoError` |
| Interlaced image | pass 1 only for the preview; full passes not decoded |
| Alpha | composited over white |
| Palette | expanded to RGB |
| CRC mismatch | ignored (P12) |

## Verification

- Host tests for `lib.bitio` (the test-only `bitio_probe` through `langhost`),
  `lib.inflate` and `lib.png` against fixed PNG
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
| 2026-11-21 | G1's `lib.bitio` is implemented (`samples/apps/lib/bitio.ig`): a 512-byte window with `init`/`fill`/`available`/`ensure`/`peek`/`drop`/`read_bits`/`align`/`read_byte`, LSB-first DEFLATE order, `WouldBlock`/`Invalid`/`Full` statuses, no new builtin or opcode. `read_byte` drains a whole byte already buffered in the accumulator after `align`, so switching from bits to stored-block bytes never loses data. Proved by the test-only `bitio_probe` on `langhost` and in the `--shell` image. |
| 2026-11-21 | G2's `lib.inflate` is implemented (`samples/apps/lib/inflate.ig`): a bounded-input, bounded-output zlib/DEFLATE decoder with `init`/`done`/`read`, a 32 KiB output ring plus canonical Huffman tables (40,816 bytes), a caller-supplied compressed prefix (`in_pos` persists, `input_len` bounds it), stored/fixed/dynamic blocks, `Ok`/`WouldBlock`/`Invalid` statuses, no faults and no Adler-32 check. The PNG layer's staging prefix is capped at 512 KiB (measured pass-1 maximum 368,317 B). Proved by the test-only `inflate_probe` over committed stored, fixed, dynamic, LZ and mixed vectors, a truncated and a corrupt stream, and a half-prefix plus a mid-header pause/resume of the dynamic vector (ADR 0057). |
| 2026-11-21 | The row-streaming decoder is implemented (`samples/apps/lib/png.ig`): `probe`/`open`/`info`/`next_row`, 8-bit colour types 0/2/3/4/6, filters 0–4, Adam7 pass 1 only, palette expansion with `tRNS` alpha, alpha composited over white, a 512 KiB IDAT prefix and `WouldBlock` mapped to `CapacityExceeded` when the prefix is full and `Invalid` when the IDAT ended. State is 611,352 bytes; the caller row is 16,384 bytes. Proved by the test-only `png_probe` over committed fixtures (every filter, colour type, interlace pass 1, a 1000-pixel row, unsupported depth/width, truncated, corrupt and non-PNG input) on `langhost` and the `--shell` image. |
