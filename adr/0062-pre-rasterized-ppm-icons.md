# 0062: Pre-rasterize the icon set to 32×32 PPM P6

- **Status:** Accepted
- **Date:** 2026-11-27
- **Deciders:** project owner
- **Spec:** 0028 (RGB icon assets and the icon view widget)
- **Supersedes:** nothing

## Context

The owner supplied four 64×64 SVG icons (`folder`, `igfile`, `file`, `image`)
for the file browser's icon view. The kernel is freestanding `no_std` source
with no XML parser, path rasterizer, gradient engine or floating point, and
applications are interpreted source at ring 0. The icon set is fixed, small and
read-only; it never changes at run time.

The viewer already has a row-streaming PPM P6 reader (`lib.ppm`, spec 0024,
ADR 0058) and an opaque RGB blit (`syslib.blit_rgb`, spec 0009 G22). A 32×32
icon is 3,072 bytes, so four icons cost 12,288 bytes in the application's
arena and redraw with four blits.

## Decision

Rasterize the SVG sources **at authoring time** with
`scripts/make_icons.py` (ImageMagick with its librsvg delegate) into 32×32
binary PPM P6, maxval 255, no alpha. Transparent areas are composited on
`#FFFFFF`, the browser grid's background. Commit the four generated PPMs
(12 KB total) and stage them read-only as `icons/` in the application image.
The runtime loads them once with `lib.ppm` into `lib.icons.Set` and draws them
with `syslib.blit_rgb`.

## Alternatives rejected

- **SVG parser and rasterizer in the kernel.** A large, security-sensitive
  component (XML, CSS subset, paths, transforms, gradients, anti-aliasing) for
  four images that never change. It also needs floating point or a fixed-point
  rasterizer, and it would run on every draw.
- **Bake the icons into the compiler/stdlib as monochrome `ui_icon` masks.**
  That is the M2.4.5 model; it cannot carry the colour the owner asked for, and
  every icon becomes hand-written bit data.
- **PNG icons.** PNG adds a DEFLATE/filter decode per icon for no size benefit
  at 32×32, and the PNG path stages a bounded prefix. PPM is a direct read.
- **Generate the PPMs during every `cargo xtask image`.** It makes the image
  build depend on ImageMagick and the SVG renderer version, and the outputs are
  too small to justify it. The script still exists for regeneration and
  `--check`.
- **RGBA/PAM with alpha.** It would need a new blending blit and a different
  asset format; the opaque grid background makes compositing at authoring time
  sufficient.
