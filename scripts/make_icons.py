#!/usr/bin/env python3
"""Rasterize the tracked SVG icon sources to committed 32x32 PPM P6 files.

The SVGs in `samples/icons/` are source assets only; the target parses no SVG
(spec 0028, A1; ADR 0062). Rasterizing happens here, at authoring time, with
ImageMagick and its librsvg delegate. Transparent areas are composited on
`#FFFFFF` before the final resize, so the opaque PPM matches the browser grid
background. The four outputs total 12 KB, so they are committed: the image
build then needs neither Python nor ImageMagick, and the pixels cannot drift
with a rasterizer upgrade.

Each output is binary PPM P6, exactly 32x32, maxval 255, 8-bit RGB, no alpha,
with a 3,072-byte raster.
"""

import argparse
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "samples" / "icons"
OUTPUT = SOURCE / "ppm"
NAMES = ("folder", "igfile", "file", "image")
SIDE = 32
RASTER_BYTES = SIDE * SIDE * 3
WHITESPACE = b" \t\r\n\v\f"


class IconError(Exception):
    """A precise, user-facing failure."""


def fail(message: str) -> int:
    print(f"make_icons: {message}", file=sys.stderr)
    return 1


def find_rasterizer() -> str:
    for name in ("magick", "convert"):
        path = shutil.which(name)
        if path:
            return path
    raise IconError(
        "ImageMagick is required to rasterize the icons; "
        "install `magick` or `convert` and retry"
    )


def read_ppm_header(data: bytes, path: Path) -> tuple[bytes, bytes, bytes, bytes, int]:
    """Returns the four header tokens and the raster offset.

    Whitespace and `#` comments are allowed between header tokens, as PPM P6
    permits, but the single separator after maxval is consumed separately.
    """
    tokens: list[bytes] = []
    pos = 0
    while len(tokens) < 4:
        while pos < len(data) and data[pos] in WHITESPACE:
            pos += 1
        if pos >= len(data):
            raise IconError(f"{path}: truncated P6 header")
        if data[pos : pos + 1] == b"#":
            while pos < len(data) and data[pos] not in b"\r\n":
                pos += 1
            continue
        start = pos
        while pos < len(data) and data[pos] not in WHITESPACE:
            pos += 1
        tokens.append(data[start:pos])

    if data[pos : pos + 2] == b"\r\n":
        pos += 2
    elif pos < len(data) and data[pos] in WHITESPACE:
        pos += 1
    else:
        raise IconError(f"{path}: maxval is not followed by whitespace")

    return tokens[0], tokens[1], tokens[2], tokens[3], pos


def validate(name: str) -> Path:
    path = OUTPUT / f"{name}.ppm"
    if not path.is_file():
        raise IconError(f"{path}: missing")

    data = path.read_bytes()
    magic, width, height, maxval, raster_at = read_ppm_header(data, path)
    if magic != b"P6":
        raise IconError(f"{path}: magic is {magic!r}, expected P6")
    try:
        dimensions = (int(width), int(height))
    except ValueError:
        raise IconError(f"{path}: malformed dimensions {width!r} x {height!r}")
    if dimensions != (SIDE, SIDE):
        raise IconError(
            f"{path}: dimensions are {dimensions[0]}x{dimensions[1]}, "
            f"expected {SIDE}x{SIDE}"
        )
    if maxval != b"255":
        raise IconError(f"{path}: maxval is {maxval!r}, expected 255 (8-bit RGB)")

    raster = len(data) - raster_at
    if raster != RASTER_BYTES:
        raise IconError(
            f"{path}: raster is {raster} bytes, expected {RASTER_BYTES}"
        )
    return path


def rasterize(tool: str, name: str) -> Path:
    source = SOURCE / f"{name}.svg"
    if not source.is_file():
        raise IconError(f"{source}: missing SVG source")
    target = OUTPUT / f"{name}.ppm"
    command = [
        tool,
        "-background",
        "white",
        "-density",
        "384",
        str(source),
        "-alpha",
        "remove",
        "-alpha",
        "off",
        "-resize",
        f"{SIDE}x{SIDE}!",
        "-depth",
        "8",
        f"ppm:{target}",
    ]
    try:
        status = subprocess.run(command, check=False).returncode
    except OSError as error:
        raise IconError(f"failed to run {tool}: {error}")
    if status != 0:
        raise IconError(f"{tool} exited with status {status} for {source}")
    return validate(name)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument(
        "--force",
        action="store_true",
        help="rewrite every PPM even when the output exists",
    )
    mode.add_argument(
        "--check",
        action="store_true",
        help="validate the committed PPMs without writing anything",
    )
    args = parser.parse_args()

    try:
        if not SOURCE.is_dir():
            raise IconError(f"{SOURCE} is not a directory")

        written = 0
        if not args.check:
            missing = [name for name in NAMES if not (OUTPUT / f"{name}.ppm").is_file()]
            if args.force or missing:
                tool = find_rasterizer()
                OUTPUT.mkdir(parents=True, exist_ok=True)
                for name in NAMES if args.force else missing:
                    path = rasterize(tool, name)
                    print(f"icon: wrote {path.relative_to(ROOT)}")
                    written += 1

        for name in NAMES:
            path = validate(name)
            print(
                f"icon: {path.relative_to(ROOT)} {SIDE}x{SIDE} "
                f"{path.stat().st_size} B"
            )
    except IconError as error:
        return fail(str(error))

    print(f"make_icons: {written} written, {len(NAMES)} valid")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
