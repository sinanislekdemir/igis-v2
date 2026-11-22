#!/usr/bin/env python3
"""Generate the full-resolution PPM P6 image corpus from the tracked sources.

The source photographs live in `samples/image-src/` and are committed. The
generated PPM P6 files live in `samples/image/` and are git-ignored: they are a
build input, not source. `cargo xtask image` runs this script automatically when
`samples/image/` has no `.ppm` file, so a fresh clone only needs Python 3 and
Pillow to build the image.

The conversion is lossless with respect to the decoded source: no resize, no
crop, no colour conversion beyond RGB, no metadata. PPM P6 is uncompressed
RGB888 with an ASCII header, so the viewer can seek any row by its fixed
`width * 3` stride and build a 1/8 preview without decoding.
"""

import argparse
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "samples" / "image-src"
OUTPUT = ROOT / "samples" / "image"
SOURCE_SUFFIXES = {".jpg", ".jpeg", ".png"}


def fail(message: str) -> int:
    print(f"make_image_corpus: {message}", file=sys.stderr)
    return 1


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--force",
        action="store_true",
        help="rewrite every PPM even when the output exists",
    )
    args = parser.parse_args()

    try:
        from PIL import Image
    except ImportError:
        return fail("Pillow is required (python3 -m pip install Pillow)")

    Image.MAX_IMAGE_PIXELS = None
    if not SOURCE.is_dir():
        return fail(f"{SOURCE} is not a directory")
    sources = sorted(
        path
        for path in SOURCE.iterdir()
        if path.is_file() and path.suffix.lower() in SOURCE_SUFFIXES
    )
    if not sources:
        return fail(f"{SOURCE} has no source images")

    OUTPUT.mkdir(parents=True, exist_ok=True)
    total = 0
    written = 0
    for source in sources:
        target = OUTPUT / f"{source.stem}.ppm"
        if target.exists() and not args.force:
            print(f"ppm: keep {target.name}")
            continue
        with Image.open(source) as image:
            rgb = image.convert("RGB")
            width, height = rgb.size
            rgb.save(target, format="PPM")
        with target.open("rb") as handle:
            magic = handle.readline().strip()
            dimensions = handle.readline().split()
            maxval = handle.readline().strip()
        if magic != b"P6" or maxval != b"255":
            return fail(f"{target.name}: not a P6/maxval-255 file")
        if len(dimensions) != 2:
            return fail(f"{target.name}: malformed dimensions")
        if (int(dimensions[0]), int(dimensions[1])) != (width, height):
            return fail(f"{target.name}: dimension mismatch")
        size = target.stat().st_size
        total += size
        written += 1
        print(f"ppm: {target.name} {width}x{height} {size} B")

    print(f"ppm: {written} written, {len(sources)} source(s), {total} B")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
