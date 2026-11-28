# 0024: Raw PPM images

- **Status:** Implemented
- **Scope:** `samples/apps/lib` (`ppm`), `samples/apps/apps`
  (`image_viewer`), `samples/image`, `samples/image-src`, `scripts`,
  `samples/tests/apps`, `langhost`, `xtask`
- **Supersedes:** none
- **Superseded by:** none
- **ADRs:** 0058
- **Updated:** 2026-11-27

## Context

Spec 0023 shipped the image corpus as full-resolution Adam7-interlaced PNG and
decoded the fit-to-window preview from Adam7 pass 1. That decoder is retained:
`lib.bitio`, `lib.inflate` and `lib.png` stay the path for arbitrary PNG files,
and their requirements are unchanged.

The shipped corpus itself is read-only and validated at build time. Compressing
it buys image size but costs a per-pixel Huffman/filter decode on every view, and
the 512 KiB IDAT stage of `lib.png` cannot decode a full-resolution
non-interlaced stream. A raw format removes that work: a row is a direct read,
and any row can be sought by index. The four source images are 3776×2832,
3000×2000, 4000×2667 and 4608×2963; as uncompressed P6 RGB they are 32.1, 18.0,
32.0 and 41.0 MB, **123.0 MB in total**. The applications image therefore grows
from 64 MiB to **160 MiB** for both the product and the `--shell` image (128 MiB
when the corpus landed; raised for the audio samples, spec 0031).

This specification covers the P6 corpus, the `lib.ppm` header/row contract, the
viewer dispatch between PNG and PPM, and the row-sampling preview. It does not
add a kernel codec, compression, or any Netpbm format other than P6.

## Requirements

### Corpus and image

- **C1 (MUST):** The shipped corpus at `image/` is uncompressed binary PPM
  (Netpbm P6, 8-bit RGB, maxval 255), staged from `samples/image`. The source
  photographs are tracked in `samples/image-src/`; the PPMs are generated from
  them by `scripts/make_image_corpus.py` and are git-ignored build input, not
  committed source. Every file keeps its original dimensions; no file is
  resized, cropped or colour-managed.
- **C2 (MUST):** The four generated corpus files are 3776×2832, 3000×2000,
  4000×2667 and 4608×2963. The generated corpus is about 123.0 MB; the tracked
  JPEG sources are about 5.2 MB; the applications image is **160 MiB** for both
  the product and the `--shell` image.
- **C3 (MUST):** The PNG decoder and its corpus requirements stay in the tree
  for PNG input (spec 0023); the PNG path is not removed or reduced. An
  application or user file that is a PNG still decodes through `lib.png`.
- **C4 (MUST):** No kernel codec, opcode or VFS special case is added for PPM.
  The reader is ordinary `/lib` source.
- **C5 (MUST):** `cargo xtask image` generates the PPMs with
  `scripts/make_image_corpus.py` when `samples/image/` contains no `.ppm`;
  `--force` regenerates them after a source change. The generator requires
  Python 3 and Pillow. A fresh clone therefore carries the ~5.2 MB JPEG sources
  and not the ~123 MB generated PPMs; `samples/image/*.ppm` is git-ignored.

### `lib.ppm`

- **M1 (MUST):** `samples/apps/lib/ppm.ig` is an ordinary `/lib` module (spec
  0018, M5/M6): `unit ppm;`, `uses core;`, no `main`, no top-level `var` state,
  no stream endpoint and no new builtin or opcode.
- **M2 (MUST):** The public contract is:

  ```
  type Decoder = record
    path: string[256];
    width: uint;
    height: uint;
    stride: uint;       // width * 3
    data_offset: uint;  // absolute file offset of the first pixel
    opened: bool;
  end;

  function ppm.probe(path: const string[0]; width: var uint; height: var uint) -> Status;
  function ppm.open(path: const string[0]; d: var Decoder) -> Status;
  function ppm.info(d: const Decoder; width: var uint; height: var uint) -> Status;
  function ppm.read_row(d: var Decoder; y: uint; row: var bytes[16384]; got_w: var uint) -> Status;
  ```

- **M3 (MUST):** Only P6 with maxval 255 is accepted. P3, P5 and every other
  magic are `Invalid`; a maxval other than 255 is `Invalid`; a missing file is
  `NotFound`. A header or raster that ends early is `Invalid`. A file read that
  fails after a successful open is `IoError`.
- **M4 (MUST):** The header is ASCII tokens separated by Netpbm whitespace
  (space, tab, LF, VT, FF, CR); `#` starts a comment through the next newline.
  The magic must be the first token, but leading whitespace and comments before
  it are tolerated, as are comments and extra whitespace between tokens. Width,
  height and maxval are decimal digits only and are capped at 4,294,967,295.
  After maxval the reader consumes **exactly one** whitespace byte and records
  the next byte as `data_offset`.
- **M5 (MUST):** The header may span more than one 256-byte `core.fs_read`
  chunk. The scanner tracks absolute file offsets, refills at the current
  position, and never treats pre-read raster bytes as a header token; after
  maxval the offset is repositioned exactly and the raster is re-read by offset.
- **M6 (MUST):** `probe` parses the header only and returns the dimensions;
  `open` parses the header and fills the decoder, including a copy of the path;
  `info` copies the dimensions of an opened decoder and is `Invalid` before a
  successful `open`.
- **M7 (MUST):** `read_row` requires an opened decoder and `y < height`. A row is
  `stride = width * 3` bytes; `width * 3 <= 16384` (5,461 pixels) is required,
  otherwise the status is `CapacityExceeded`. The row is read from
  `data_offset + y * stride` in 256-byte `core.fs_read` chunks into a local
  `bytes[256]` and appended into the caller's `bytes[16384]` with
  `core.copy_bytes`; `got_w = width`. A short read is `Invalid`, a failed read
  is `IoError`, and there is no full-image buffer.
- **M8 (MUST):** Any row can be read in any order; `read_row` is stateless apart
  from the decoder fields, so a viewer reads every `N`th row for a preview or a
  zoom-out without decoding the rows in between.
- **M9 (MUST):** Failures leave out-parameters untouched, except that
  `read_row` clears the caller's row before the read and leaves it empty on a
  failed read (a row is never partially published); `got_w` stays untouched on
  failure. No failure faults.
- **M10 (MUST):** There is no colour conversion: PPM is already RGB in the order
  `syslib.blit_rgb` expects. There is no compression, no row filter, no palette
  and no index.

### Viewer

- **V1 (MUST):** `apps/accessories/image_viewer` dispatches by format. A
  `.png` path uses `lib.png`; a `.ppm` path uses `lib.ppm`. A path with neither
  suffix is sniffed by trying `ppm.probe` first and then `png.probe`; an
  unrecognised file opens the existing error panel. The PNG path keeps the spec
  0023 behaviour exactly.
- **V2 (MUST):** For a PPM source the fit-to-window preview is
  `ceil(width/8) x ceil(height/8)`: the viewer reads source row
  `sy = y * height / out_h` with `ppm.read_row` into a `bytes[16384]`
  `src_row`, samples column `sx = x * width / out_w` into a
  `bytes[3072]` `preview_row` (`3 * x` bytes copied per output pixel), and
  blits `preview_row` as one `syslib.blit_rgb` row. Each preview row is one
  seek; nothing outside the visible rows is read, no decoding is done and no
  full image is buffered. `out_w > 1,024` is `CapacityExceeded`.
- **V3 (MUST):** Thumbnails and zoom-out use fixed-stride row seeking
  (`read_row` at every `N`th row) with nearest-pixel column sampling. There is
  no decode, no resampling state and no image buffer.
- **V4 (MUST):** The viewer supports the existing `check` and `shot` modes. Its
  `check` path proves a PPM probe/open/row read independently of the test-only
  `ppm_probe` against the 8x8 `system/ppm_rgb.ppm` formula fixture, and the
  file dialog still starts at `image`.
- **V5 (MUST):** The PPM preview uses the same `ow x (oh + 20)` window and
  status strip as PNG: the strip is drawn at 0%, updated every 8 output rows
  and ends at `Ready`/100%. The PPM row loop is promoted to realtime when
  admission allows and heartbeats once per output row (admission refusal falls
  back to the normal class); each row read is bounded.
- **V6 (MUST, implemented):** The opening window's queued `Expose` needs no answer,
  exactly like every later one (spec 0023 V5): the retained canvas already holds the
  preview, so the viewer never re-opens the header or re-reads the rows. One open
  performs one preview pass.

## Design

The corpus path is a straight read:

```
image/<name>.ppm  ──core.fs_read(256 B)──▶  lib.ppm  ──▶  caller's 16,384-byte row
                                            │
                                            ├─ header tokens (256-byte refills)
                                            └─ seek: data_offset + y * stride
                                                          │
                                                    syslib.blit_rgb
```

The header parser keeps only a 256-byte window, its absolute base, and the next
byte offset. It reads tokens; comments are skipped through a newline; leading
whitespace/comments are tolerated. After maxval it consumes the single separator
whitespace byte and stores the resulting absolute offset, so the raster read
starts exactly at the first pixel byte even when the header crossed a refill.

`read_row` is a bounded copy: `ceil(stride / 256)` file reads into a local
`bytes[256]`, each appended with `core.copy_bytes`. The row buffer is the
caller's, so PPM adds no large decoder state; the module itself owns no state.

The format's self-describing header and RGB order are what make the `1/8`
preview cheap: PNG pass 1 requires inflating the Adam7 pass-1 stream, while PPM
needs only `ceil(height/8)` row reads at stride 8. A thumbnail or zoom-out is the
same operation at a larger stride.

## Interfaces

The four functions and `Decoder` in M2 are normative. The caller supplies the
only image-shaped buffer (`bytes[16384]`); the decoder is a small record that
holds the path and geometry.

## Failure modes

| Condition | Result |
|---|---|
| File missing | `NotFound` |
| Not P6 (P3, P5, other magic) | `Invalid` |
| maxval other than 255 | `Invalid` |
| Malformed or non-decimal dimension, width or height zero | `Invalid` |
| Missing separator after maxval | `Invalid` |
| Header or raster ends early | `Invalid` |
| Row wider than 16,384 bytes | `CapacityExceeded` |
| File read error after open | `IoError` |
| `info`/`read_row` before a successful `open` | `Invalid` |
| `read_row` at or past the height | `Invalid` |
| Any corrupt file | a status, never a fault |

## Verification

- The test-only `samples/tests/apps/base/ppm_probe.ig` runs on `langhost` and in
  the `--shell` image. It checks `probe`/`open`/`info` geometry, reads every pixel
  of an 8×8 formula fixture and of the same image with leading whitespace,
  comments and a header longer than one 256-byte read, and asserts the negative
  statuses: P5 and P3 magic and maxval 65535 are `Invalid`, a missing file is
  `NotFound`, a 10-byte raster is `Invalid` on the row read, and a 6000×1 image
  is `CapacityExceeded` for its 18,000-byte row. Fixtures are committed under
  `samples/tests/apps/system/`.
- A `langhost` e2e test asserts the probe output `ppm: ok`.
- A `--shell` kernel transcript on `-smp 4` and `-smp 1` prints `ppm: ok`.
- `cargo test -p abi -p compiler -p interpreter -p langhost -p ext2w -p net`
  stays green, and `cargo xtask build` succeeds.
- The committed visual golden is unchanged: the proof opens no window. The
  dual-format viewer's rendered preview is proved by its own `check` path and a
  QMP capture in M14.3.

## Non-goals

- P3 (ASCII), P5 (grey) and every other Netpbm variant.
- 16-bit samples, alpha, gamma or colour management.
- Compression, row filters or an index in the corpus.
- Resizing, cropping or converting the corpus.
- A kernel image codec.

## History

| Date | Change |
|---|---|
| 2026-11-22 | Accepted with the full-resolution P6 corpus (123.0 MB), the 128 MiB apps image, the `lib.ppm` header/row contract and the PNG/PPM viewer dispatch (ADR 0058). |
| 2026-11-22 | `samples/apps/lib/ppm.ig` is implemented and proved by `ppm_probe` on `langhost` and in the `--shell` image: `probe`/`open`/`info`/`read_row`, 256-byte header refills with absolute offsets and comments, one whitespace separator after maxval, P6/maxval-255 only, and the status set `NotFound`/`Invalid`/`CapacityExceeded`/`IoError`. |
| 2026-11-22 | The dual-format viewer is implemented (`apps/accessories/image_viewer`): `.png` keeps the spec 0023 `lib.png` path (normal-class open, realtime row loop, `png.rewind` on `Expose`), `.ppm` uses `lib.ppm` with `ceil(w/8) x ceil(h/8)` row-seek/nearest-column sampling into a 3,072-byte `preview_row` and a per-row realtime heartbeat (needed under TCG), and a suffix-less path tries PPM then PNG. The 20 px status strip is shared and ends at `Ready`. `check` adds the 8x8 `system/ppm_rgb.ppm` formula fixture. The `image_viewer_qmp.py` proof now opens `image/simon-twukN12EN7c-unsplash.ppm`; the Desktop capture is 1024x768 with the `Ready` strip. |
| 2026-11-27 | Image viewer: the opening window's queued Expose is answered with `window_present` instead of a second PPM open/row-seek preview pass (spec V6). `image_viewer shot` no longer consumes and re-renders that Expose; `Ready` still follows the one complete pass. No ABI, builtin, kernel or file-API change. |
| 2026-11-28 | C2 raises the applications image from 128 MiB to **160 MiB** for both the product and the `--shell` image, so the classical audio corpus (spec 0031) fits with headroom; the PPM corpus itself is unchanged at 123.0 MB. No kernel, format or reader change; `e2fsck -fn` is clean. |
