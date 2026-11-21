# 0057: PNG row streaming with an Adam7 pass-1 preview

- **Status:** Accepted
- **Date:** 2026-11-21
- **Deciders:** project owner
- **Spec:** 0023 (the PNG corpus and the image viewer)
- **Supersedes:** nothing

## Context

The image viewer needs to display photographs from a read-only corpus on a machine
with no filesystem cache tuned for large files and no full-image framebuffer budget.
The first corpus was JPEG. A JPEG decoder needs a lossy inverse DCT, a Huffman
decoder and a large block state; PNG needs a bit reader, a DEFLATE decoder and five
row filters, which are useful independently.

The owner requires the corpus at its original resolution. The four source images
are 3776×2832, 3000×2000, 4000×2667 and 4608×2963. A full RGB frame for the last
one is about 41 MB, and the corpus converted to interlaced 8-bit RGB PNG totals
about 46 MB. An applications image that holds the corpus must therefore be larger
than the previous 4 MiB.

Three decode strategies were considered:

1. **A kernel codec.** Fast and simple to call, but it puts a large format-specific
   parser in ring 0 and contradicts the thesis that applications and their
   libraries are source in `/lib`.
2. **A full-image RGB buffer in `/lib`.** Straightforward for pan and zoom, but a
   single image would exceed the compile arena and the window content budget, and
   interlaced files would still need a gather step.
3. **Row streaming with an early interlace pass.** Decode into one row (plus a
   32 KiB inflate window), and for interlaced input stop after Adam7 pass 1, whose
   `ceil(w/8) x ceil(h/8)` pixels are exactly the cheap fit-to-window preview.

## Decision

- No kernel image codec. The decoder is `/lib` source: `lib.bitio`, `lib.inflate`
  and `lib.png`.
- The generic primitives (`lib.bitio`, `lib.inflate`, `core` bit and byte
  operations, `syslib.blit_rgb`) are separate, format-independent tickets.
- PNG decoding is row-streaming (`png.open` / `png.next_row`). There is no
  full-image RGB buffer; state is one row buffer and a 32 KiB inflate window.
- The fit-to-window preview of an interlaced PNG decodes Adam7 pass 1 only.
  Non-interlaced images are decoded row by row.
- The corpus is full-resolution 8-bit RGB, non-premultiplied, Adam7-interlaced PNG
  under `/image`, converted at original dimensions with no resize. The apps image
  grows to 64 MiB for both the product and `--shell` images.
- 16-bit and other bit depths are rejected; alpha is composited over white; a
  palette is expanded; CRC is not verified.

## Alternatives

- **Keep JPEG.** Rejected: the decoder is much larger and lossy, and this slice has
  no JPEG requirement.
- **Resize the corpus.** Rejected by the owner: the source resolution is part of the
  artifact, and interlace is an encoding choice, not a resize.
- **Store the corpus outside the image.** Rejected for now: it would add a second
  block device or a host-shared path and break the "one read-only image" model.
- **Decode all seven Adam7 passes for the preview.** Rejected: pass 1 is 1/64 of the
  pixels and is the stated fit preview. Full-resolution progressive display remains
  future work.
- **Verify chunk CRCs.** Rejected for this slice: it adds a pass over every byte for
  a corpus that is read-only and validated at build time; structural bounds and the
  zlib checksum still catch corruption.

## Consequences

- The apps image is 64 MiB and the boot path copies it as a module; boot time and
  memory use grow with the corpus size. The image remains read-only.
- Full-resolution decode and future pan/zoom cost CPU per row; the preview path is
  cheap, but a 1:1 view of a 4608-pixel-wide image still filters and inflates the
  full stream.
- Memory stays bounded and predictable: a row plus the inflate window, independent
  of image area.
- The generic layers are reusable beyond PNG and can be host-tested without the
  kernel.
- Interlaced preview quality is pass 1 only (blocky when scaled up); that is
  accepted for fit-to-window and recorded as a non-goal for this slice.
- Adding a new format later is a `/lib` module and a viewer change, not a kernel
  change.

## Amendment 2026-11-21: the bounded IDAT prefix

`lib.inflate` is implemented as a bounded-input, bounded-output decoder rather than a
stream endpoint. The PNG decoder stages the compressed payload in its own task state and
then calls `inflate.read` with that same prefix on every call:

- The stage is capped at **512 KiB**. The measured maximum `IDAT` bytes needed for the
  Adam7 pass-1 prefix across the real corpus is **368,317 bytes**, leaving about 156 KiB
  of headroom.
- `read` takes `(input, input_len)` and appends at most 4 KiB to `out`. `in_pos` persists
  across calls. A prefix too short for the next block header or symbol returns
  `WouldBlock` and loses nothing, so the caller may pass a longer prefix later.
- A compressed payload larger than the stage is `CapacityExceeded` in the PNG layer. A
  full-resolution non-interlaced stream can exceed 512 KiB (a 4608×2963 RGB image has
  about 41 MB of filtered rows before compression), so it is not decodable through this
  bounded prefix without a larger stage; that is outside the pass-1 preview this ADR
  covers.
- The inflater itself keeps only a 32 KiB output ring plus canonical Huffman tables
  (40,816 bytes in total), independent of the prefix and the image. The Adler-32 trailer
  is not read.

This refines the "streaming" wording above without changing the row-streaming decision:
the preview still decodes Adam7 pass 1 into one row at a time, only the compressed side
is a caller-owned prefix instead of an endpoint.

## Amendment 2026-11-21: the decoder implementation and prefix statuses

`lib.png` is implemented as `samples/apps/lib/png.ig` with `probe`, `open`, `info` and
`next_row`. `probe` reads only the signature and IHDR; `open` re-reads them, parses
`PLTE`/`tRNS`, walks the chunks and stages the IDAT prefix. The decoder record holds the
40,816-byte inflater, a 524,288-byte IDAT prefix, two 20,480-byte source rows (previous
and current), a 4,096-byte inflater-output chunk, a 768-byte palette, a 256-byte `tRNS`
table and scalars: 611,352 bytes in total. The caller's RGB row is 16,384 bytes.

A row call drains `inflate.read` over the staged prefix and resumes from `d.out_pos` and
the inflater's `in_pos`. The two failure modes are deliberately distinct:

- `WouldBlock` with a prefix that filled the 512 KiB stage is `CapacityExceeded`: the
  file has more compressed payload than the bounded stage can hold.
- `WouldBlock` with a prefix that contains the file's final IDAT byte is `Invalid`: the
  compressed stream ended before the requested row.

Chunk CRCs and the Adler-32 trailer are still not verified, so a file whose only damage
is in the trailer decodes its in-range rows successfully; structural damage (a bad zlib
or DEFLATE header, table or symbol) is `Invalid`.

## Amendment 2026-11-22: `rewind` reuses the staged prefix for repaint

An `Expose` must be repainted without reading the file again. `png.rewind(d)` keeps the
staged `idat`/`idat_len`, the palette/`tRNS` and the geometry, and re-initialises only the
inflater and row cursor (previous row cleared, as after `open`). The zlib header is at the
start of the retained prefix, so decoding restarts from the first byte with no `fs_read`
and no new allocation. It returns `Ok` for an opened decoder and `Invalid` before `open`.
This does not change the bounded-prefix decision: the same 512 KiB stage is reused, and
the `CapacityExceeded`/`Invalid` mapping is unchanged.
