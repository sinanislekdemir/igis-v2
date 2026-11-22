# 0058: Ship the image corpus as uncompressed PPM P6 and keep PNG support

- **Status:** Accepted
- **Date:** 2026-11-22
- **Deciders:** project owner
- **Spec:** 0024 (raw PPM images); amends the corpus choice of 0023
- **Supersedes:** nothing

## Context

The shipped corpus is read-only, written at build time and displayed by one
viewer. Spec 0023 chose full-resolution Adam7-interlaced PNG and implemented a
row-streaming decoder (`lib.bitio`, `lib.inflate`, `lib.png`) with an Adam7
pass-1 preview. That decoder works, but it pays for compression on every view:

- Every row runs a DEFLATE decode (Huffman symbols, LZ matches) and a row filter
  undo before it can be shown.
- The decoder stages a bounded 512 KiB IDAT prefix. A full-resolution
  non-interlaced stream is larger than that prefix, so the PNG path can preview
  but cannot stream the whole frame with the current stage.
- The viewer preview is cheap only because Adam7 pass 1 is 1/64 of the pixels;
  any other row selection needs decoder state and cannot be a seek.

The images are 3776×2832, 3000×2000, 4000×2667 and 4608×2963. The owner requires
the original dimensions and prefers a fast view over a small image.

The measured options for the corpus are:

| Encoding | Size |
|---|---|
| Uncompressed P6 RGB | **123.0 MB** |
| Per-row zlib, no filter | 77.9 MB |
| Per-row zlib with the Sub filter | 46.5 MB |
| PNG (current corpus) | 45.8 MB |

## Decision

The shipped corpus is **uncompressed Netpbm PPM P6 RGB888** at full resolution.
The PNG decoder stays in the tree for arbitrary PNG files.

- `samples/image` holds four `.ppm` files; `image/` is staged unchanged.
- The applications image grows to **128 MiB** for the product and `--shell`
  builds, which holds 123.0 MB of corpus with headroom for the applications.
- `lib.ppm` parses the header once and reads a row by absolute seek:
  `row = data_offset + y * stride`. It has no compressed state, no filters and no
  full-image buffer.
- The viewer dispatches PNG/PPM, keeps the PNG pass-1 path, and samples PPM rows
  by fixed stride for the 1/8 preview and for thumbnails/zoom-out.
- The PNG corpus requirements of spec 0023 remain in force for PNG input; the
  active shipped corpus is the PPM one from spec 0024.

## Alternatives

- **Keep PNG for the corpus.** Smallest image (45.8 MB) but the slowest view:
  every pixel passes through inflate and filter undo, the 512 KiB prefix bounds
  the stream, and arbitrary row seeking needs decode state. Rejected for the
  shipped corpus; retained as the format for user PNG files.
- **Per-row zlib (no filter).** 77.9 MB, and a row is independently inflatable,
  so it seeks like raw data. Still pays a Huffman decode per row and needs a
  custom framing (row offsets or fixed record size) that is not self-describing.
  Rejected: the decode cost and the custom format buy only 45 MB.
- **Per-row zlib with the Sub filter.** 46.5 MB, close to PNG, but adds a filter
  pass and custom framing on top of the per-row inflate. Rejected for the same
  reason.
- **A custom raw format.** Could add a fixed header and a row index, but it is
  not self-describing or viewable outside the system, and PPM already has the
  properties that matter (magic, dimensions, maxval, RGB order).
- **BMP or TGA.** Both are simple raster formats, but BMP's headers and
  row padding/BGR order and TGA's optional footer/compression variants add
  parsing branches for no benefit over P6 RGB.

## Consequences

- The apps image is 128 MiB and the boot path loads it as a module, so boot-time
  copying and RAM use grow with the corpus (about 123 MB of raster). The image
  stays read-only.
- Any PPM row is one bounded read; the 1/8 preview reads `ceil(h/8)` rows and a
  thumbnail reads fewer, with no decode.
- A row is at most 16,384 bytes, so the viewer's row buffer and `syslib.blit_rgb`
  contract are unchanged.
- Storage is the price: 123.0 MB instead of 45.8 MB (about 2.7×). A future
  compressor must keep a row seekable or accept the PNG path's limits before it
  replaces this choice.
- PNG support remains a full second decoder and must keep passing its probe; the
  two formats share the viewer's row/blit path, not their decode code.
- The generic PNG layers (`lib.bitio`, `lib.inflate`, `syslib.blit_rgb`) remain
  reusable and host-testable.

## Amendment 2026-11-22: the corpus and image are in place

The four source images were converted at their original dimensions to P6 and
committed as `samples/image/*.ppm` (123.0 MB total). The applications image was
grown to 128 MiB (`mke2fs -m 0 -N 8192`); boot and `e2fsck -fn` are clean.
`lib.ppm` and its probe are ordinary `/lib` modules and the PNG decoder is
unchanged, so the corpus decision does not remove any PNG requirement.

## Amendment 2026-11-22: sources are tracked, generated PPMs are not

The 123.0 MB PPM corpus is a build artifact, not source. The four original JPEG
photographs (5.2 MB total) are tracked in `samples/image-src/`; `scripts/make_image_corpus.py`
(Python + Pillow) decodes them at their original dimensions and writes `samples/image/*.ppm`.
`cargo xtask image` runs the generator when no `.ppm` exists, and the generated PPMs are
git-ignored (`/samples/image/*.ppm`); `--force` regenerates them. The repository therefore
carries the 5.2 MB sources and the generator instead of 123 MB of derived raster. The
generated corpus is reproducible; the PNG decoder, tests and viewer path remain unchanged.
A fresh image build needs Pillow only when the generated PPMs are absent.
