#!/usr/bin/env python3
"""Headless visual-regression harness (M2.6.3, spec 0009 "Visual proof").

This is a verification harness, not part of the kernel. It boots the shell-entry
ISO with no display, runs the deterministic `visual shot` specimen, waits for
its complete frame (`visual: ok` then `visual: ready`), captures the guest
framebuffer through QMP `screendump`, and compares the decoded RGB pixels with
a committed golden.

Why a full frame: `samples/apps/visual` opens a window whose frame is exactly
the screen, so the service chrome plus the specimen cover both kernel text
windows and every earlier boot line. Nothing else can appear in the capture.
No clock, animation, event or host input is involved, and the pointer stays at
its boot position, so the frame is identical on `-smp 4` and `-smp 1`.

Golden policy (ADR 0032): the golden is the decoded RGB pixel image at the
QEMU mode the bootloader returns (1024x768 here), not the PNG encoder's bytes.
The PNG is stored only as a container; the manifest records the exact SHA-256
of the raw RGB bytes and one hash per 64x64 tile for a localized diff. A normal
run never rewrites the golden. `--update` is the only writer and is a deliberate
reviewed change (a golden is tied to the QEMU/display path and the specimen).

Usage:
    python3 scripts/visual_regression.py build/igis-shell.iso
    python3 scripts/visual_regression.py build/igis-shell.iso --cores 1
    python3 scripts/visual_regression.py build/igis-shell.iso --update
    python3 scripts/visual_regression.py build/igis-shell.iso --tamper

Exit status: 0 on a match (or a correct `--update`/`--tamper`), 1 on a
mismatch, a missing golden, a boot/ready timeout, or a specimen self-check
failure. With `--expect-mismatch` the comparison result is inverted, so a
deliberately wrong golden or a corrupted capture proves the comparator fails.
The script never displays the image; it only reads its pixels and dimensions.
"""

import argparse
import hashlib
import json
import os
import re
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DEFAULT_GOLDEN_DIR = os.path.join(ROOT, "tests", "golden")
GOLDEN_STEM = "visual"
TILE = 64


class Serial:
    """Reads QEMU's stdio serial stream and lets tests wait for lines."""

    def __init__(self, stream):
        self.stream = stream
        self.lines = []
        self.condition = threading.Condition()
        self.thread = threading.Thread(target=self._reader, daemon=True)
        self.thread.start()

    def _reader(self):
        for raw in self.stream:
            line = raw.decode("utf-8", "replace").rstrip("\r\n")
            with self.condition:
                self.lines.append(line)
                self.condition.notify_all()
            print(line, flush=True)

    def wait_for(self, pattern, timeout=30.0):
        compiled = re.compile(pattern)
        deadline = time.time() + timeout
        with self.condition:
            index = 0
            while True:
                while index < len(self.lines):
                    match = compiled.search(self.lines[index])
                    index += 1
                    if match:
                        return match
                remaining = deadline - time.time()
                if remaining <= 0:
                    raise TimeoutError(f"serial: no match for {pattern!r}")
                self.condition.wait(remaining)


class Qmp:
    """A minimal QMP client over a Unix socket."""

    def __init__(self, path, timeout=10.0):
        deadline = time.time() + timeout
        while True:
            try:
                self.socket = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                self.socket.connect(path)
                break
            except OSError:
                self.socket.close()
                if time.time() >= deadline:
                    raise
                time.sleep(0.05)
        self.file = self.socket.makefile("rwb")
        self._read()  # greeting
        self.command("qmp_capabilities")

    def _read(self):
        while True:
            line = self.file.readline()
            if not line:
                raise RuntimeError("QMP closed")
            message = json.loads(line)
            if "event" in message:
                continue
            return message

    def command(self, name, arguments=None):
        request = {"execute": name}
        if arguments is not None:
            request["arguments"] = arguments
        self.file.write(json.dumps(request).encode() + b"\n")
        self.file.flush()
        response = self._read()
        if "error" in response:
            raise RuntimeError(f"QMP {name}: {response['error']}")
        return response.get("return")

    def close(self):
        try:
            self.file.close()
            self.socket.close()
        except OSError:
            pass


def require_pillow():
    try:
        from PIL import Image  # noqa: F401

        return True
    except Exception as error:  # pragma: no cover - environment check
        print(f"visual: Pillow is required to decode the capture ({error})")
        return False


def load_rgb(path):
    """Decodes PNG/PPM to `(width, height, raw RGB bytes)` without displaying."""
    from PIL import Image

    with Image.open(path) as image:
        rgb = image.convert("RGB")
        width, height = rgb.size
        return width, height, rgb.tobytes()


def sha256(raw):
    return hashlib.sha256(raw).hexdigest()


def tiles(width, height, raw):
    """One SHA-256 per `TILE`-square tile, row-major."""
    row_bytes = width * 3
    cols = (width + TILE - 1) // TILE
    rows = (height + TILE - 1) // TILE
    digests = []
    for row in range(rows):
        y0 = row * TILE
        y1 = min(y0 + TILE, height)
        for col in range(cols):
            x0 = col * TILE
            x1 = min(x0 + TILE, width)
            digest = hashlib.sha256()
            for y in range(y0, y1):
                start = y * row_bytes + x0 * 3
                digest.update(raw[start : start + (x1 - x0) * 3])
            digests.append(digest.hexdigest())
    return cols, rows, digests


def make_manifest(width, height, raw, cores):
    cols, rows, digests = tiles(width, height, raw)
    return {
        "name": GOLDEN_STEM,
        "width": width,
        "height": height,
        "sha256": sha256(raw),
        "tile_px": TILE,
        "tile_cols": cols,
        "tile_rows": rows,
        "tiles": digests,
        "captured_on_cores": cores,
        "note": (
            "Decoded RGB pixels, not PNG bytes (ADR 0032). Regenerate only with "
            "scripts/visual_regression.py --update on the project's QEMU and "
            "display mode."
        ),
    }


def write_golden(golden_dir, width, height, raw, cores):
    from PIL import Image

    os.makedirs(golden_dir, exist_ok=True)
    png_path = os.path.join(golden_dir, f"{GOLDEN_STEM}.png")
    json_path = os.path.join(golden_dir, f"{GOLDEN_STEM}.json")
    # Re-encode so the container is canonical (no encoder timestamps or chunk
    # order differences); the compared data is the manifest's RGB hash.
    Image.frombytes("RGB", (width, height), raw).save(png_path, format="PNG")
    manifest = make_manifest(width, height, raw, cores)
    with open(json_path, "w", encoding="utf-8") as handle:
        json.dump(manifest, handle, indent=2, sort_keys=True)
        handle.write("\n")
    return png_path, json_path, manifest


def read_golden(golden_dir):
    png_path = os.path.join(golden_dir, f"{GOLDEN_STEM}.png")
    json_path = os.path.join(golden_dir, f"{GOLDEN_STEM}.json")
    if not os.path.exists(png_path) or not os.path.exists(json_path):
        raise FileNotFoundError(
            f"missing golden {png_path} / {json_path}; run --update once"
        )
    with open(json_path, "r", encoding="utf-8") as handle:
        manifest = json.load(handle)
    width, height, raw = load_rgb(png_path)
    if manifest.get("width") != width or manifest.get("height") != height:
        raise ValueError(
            f"golden manifest is {manifest.get('width')}x{manifest.get('height')} "
            f"but {png_path} is {width}x{height}"
        )
    digest = manifest.get("sha256")
    if digest != sha256(raw):
        raise ValueError(
            f"golden manifest sha256 {digest} does not match {png_path}; the "
            "golden is corrupt, regenerate it with --update"
        )
    return width, height, raw, manifest


def hex_colour(raw, width, x, y):
    offset = (y * width + x) * 3
    return "#{:02x}{:02x}{:02x}".format(*raw[offset : offset + 3])


def compare(width, height, actual, golden_width, golden_height, golden, manifest):
    """Exact decoded-pixel comparison; returns a report dictionary."""
    report = {
        "match": False,
        "width": width,
        "height": height,
        "golden_width": golden_width,
        "golden_height": golden_height,
        "actual_sha256": sha256(actual),
        "golden_sha256": manifest.get("sha256", sha256(golden)),
    }
    if (width, height) != (golden_width, golden_height):
        report["dimension_mismatch"] = True
        return report
    if actual == golden:
        report["match"] = True
        return report

    row_bytes = width * 3
    count = 0
    min_x = width
    min_y = height
    max_x = -1
    max_y = -1
    first = None
    for y in range(height):
        row_start = y * row_bytes
        if actual[row_start : row_start + row_bytes] == golden[
            row_start : row_start + row_bytes
        ]:
            continue
        for x in range(width):
            offset = row_start + x * 3
            if actual[offset : offset + 3] != golden[offset : offset + 3]:
                count += 1
                if first is None:
                    first = (
                        x,
                        y,
                        hex_colour(golden, width, x, y),
                        hex_colour(actual, width, x, y),
                    )
                if x < min_x:
                    min_x = x
                if x > max_x:
                    max_x = x
                if y < min_y:
                    min_y = y
                if y > max_y:
                    max_y = y

    report.update(
        {
            "count": count,
            "bbox": (min_x, min_y, max_x, max_y),
            "first": first,
        }
    )
    cols, rows, actual_tiles = tiles(width, height, actual)
    golden_tiles = manifest.get("tiles")
    if not golden_tiles or len(golden_tiles) != len(actual_tiles):
        _, _, golden_tiles = tiles(width, height, golden)
    differing = [
        (index % cols, index // cols)
        for index, (left, right) in enumerate(zip(actual_tiles, golden_tiles))
        if left != right
    ]
    report["tiles"] = (cols, rows, differing)
    return report


def render_report(report):
    if report.get("dimension_mismatch"):
        return (
            "visual: MISMATCH (dimensions)\n"
            f"  golden: {report['golden_width']}x{report['golden_height']} "
            f"sha256 {report['golden_sha256']}\n"
            f"  actual: {report['width']}x{report['height']} "
            f"sha256 {report['actual_sha256']}"
        )
    lines = [
        "visual: MISMATCH",
        f"  golden: {report['golden_width']}x{report['golden_height']} "
        f"sha256 {report['golden_sha256']}",
        f"  actual: {report['width']}x{report['height']} "
        f"sha256 {report['actual_sha256']}",
    ]
    total = report["width"] * report["height"]
    percent = 100.0 * report["count"] / total if total else 0.0
    x0, y0, x1, y1 = report["bbox"]
    lines.append(
        f"  differing pixels: {report['count']} of {total} ({percent:.3f}%)"
    )
    lines.append(f"  bounding box: ({x0}, {y0}) .. ({x1}, {y1})")
    if report.get("first"):
        x, y, expected, actual = report["first"]
        lines.append(
            f"  first differing pixel: ({x}, {y}) golden {expected}, actual {actual}"
        )
    if "tiles" in report:
        cols, rows, differing = report["tiles"]
        shown = ", ".join(f"({col},{row})" for col, row in differing[:12])
        suffix = "" if len(differing) <= 12 else f", ... {len(differing) - 12} more"
        lines.append(
            f"  differing {TILE}px tiles: {len(differing)} of {cols * rows}"
            f" (col,row): {shown}{suffix}"
        )
    return "\n".join(lines)


def capture(qmp, output):
    """Writes a PNG to `output`; returns the path actually produced."""
    try:
        qmp.command("screendump", {"filename": output, "format": "png"})
        return output
    except RuntimeError as error:
        print(f"[harness] screendump PNG refused ({error}); using PPM")
    fd, ppm = tempfile.mkstemp(suffix=".ppm", prefix="igis-visual-")
    os.close(fd)
    try:
        qmp.command("screendump", {"filename": ppm})
        from PIL import Image

        with Image.open(ppm) as image:
            image.save(output, format="PNG")
        return output
    finally:
        try:
            os.unlink(ppm)
        except FileNotFoundError:
            pass


def boot_and_capture(iso, cores, timeout, scratch):
    """Boots, runs `visual shot`, waits for the completed frame, captures it."""
    socket_path = os.path.join(scratch, "qmp.sock")
    capture_path = os.path.join(scratch, "capture.png")

    command = [
        "qemu-system-x86_64",
        "-cdrom",
        iso,
        "-m",
        "256M",
        "-smp",
        str(cores),
        "-serial",
        "stdio",
        "-monitor",
        "none",
        "-no-reboot",
        "-no-shutdown",
        "-accel",
        "tcg",
        "-vga",
        "std",
        "-display",
        "none",
        "-qmp",
        f"unix:{socket_path},server,nowait",
    ]
    process = subprocess.Popen(
        command,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    serial = Serial(process.stdout)
    qmp = None
    try:
        qmp = Qmp(socket_path, timeout=timeout)
        serial.wait_for(r"shell> ", timeout=timeout)
        process.stdin.write(b"run visual shot\n")
        process.stdin.flush()
        first = serial.wait_for(r"visual: (ok|FAIL)", timeout=timeout)
        if first.group(1) == "FAIL":
            raise RuntimeError("visual specimen self-check failed; see the transcript")
        serial.wait_for(r"visual: ready", timeout=timeout)
        # Give the final full present time to reach the framebuffer.
        time.sleep(0.4)
        path = capture(qmp, capture_path)
        return path
    finally:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
        if qmp is not None:
            qmp.close()


def tamper(raw, width, height):
    """Flips one RGB pixel in memory; the comparator must see a mismatch."""
    mutated = bytearray(raw)
    x = width // 2
    y = height // 2
    offset = (y * width + x) * 3
    mutated[offset] ^= 0xFF
    return bytes(mutated), x, y


def main():
    parser = argparse.ArgumentParser(
        description="Headless golden-frame regression for samples/apps/visual"
    )
    parser.add_argument("iso", help="path to build/igis-shell.iso")
    parser.add_argument("--cores", default="4")
    parser.add_argument("--golden-dir", default=DEFAULT_GOLDEN_DIR)
    parser.add_argument(
        "--update", action="store_true", help="rewrite the golden (reviewed change)"
    )
    parser.add_argument(
        "--tamper",
        action="store_true",
        help="corrupt one captured pixel; the compare must report a mismatch",
    )
    parser.add_argument(
        "--expect-mismatch",
        action="store_true",
        help="require the stored golden to mismatch (diagnostics only)",
    )
    parser.add_argument("--timeout", type=float, default=120.0)
    parser.add_argument(
        "--keep-capture", metavar="PATH", help="copy the captured frame here"
    )
    args = parser.parse_args()

    if args.update and (args.tamper or args.expect_mismatch):
        sys.exit("visual: --update cannot be combined with --tamper/--expect-mismatch")
    if not require_pillow():
        return 1
    iso = os.path.abspath(args.iso)
    if not os.path.exists(iso):
        sys.exit(f"missing ISO: {iso}")
    golden_dir = os.path.abspath(args.golden_dir)

    golden = None
    if not args.update:
        try:
            golden = read_golden(golden_dir)
        except FileNotFoundError as error:
            sys.exit(f"visual: {error}")
        except ValueError as error:
            sys.exit(f"visual: {error}")

    scratch = tempfile.mkdtemp(prefix="igis-visual-")
    keep_scratch = False
    try:
        try:
            capture_path = boot_and_capture(iso, args.cores, args.timeout, scratch)
        except (TimeoutError, RuntimeError) as error:
            print(f"visual: boot/capture failed: {error}")
            return 1

        actual_size = os.path.getsize(capture_path)
        width, height, raw = load_rgb(capture_path)
        print(
            f"[harness] capture {width}x{height} ({actual_size} bytes png) "
            f"sha256 {sha256(raw)}"
        )

        if args.keep_capture:
            destination = os.path.abspath(args.keep_capture)
            os.makedirs(os.path.dirname(destination) or ".", exist_ok=True)
            shutil.copyfile(capture_path, destination)
            print(f"[harness] capture copied to {destination}")

        if args.update:
            png_path, json_path, manifest = write_golden(
                golden_dir, width, height, raw, args.cores
            )
            print(
                f"visual: golden written ({width}x{height}, "
                f"sha256 {manifest['sha256']})"
            )
            print(f"visual: {png_path}")
            print(f"visual: {json_path}")
            return 0

        golden_width, golden_height, golden_raw, manifest = golden
        if args.tamper:
            raw, tamper_x, tamper_y = tamper(raw, width, height)
            print(f"[harness] tampered pixel ({tamper_x}, {tamper_y}) in memory")

        report = compare(
            width, height, raw, golden_width, golden_height, golden_raw, manifest
        )
        if report["match"]:
            print(
                f"visual: ok ({width}x{height}, sha256 {report['actual_sha256']})"
            )
            if args.tamper or args.expect_mismatch:
                print("visual: EXPECTED a mismatch but the frames matched")
                return 1
            return 0

        print(render_report(report))
        keep_hint = capture_path
        if args.tamper:
            keep_scratch = True
            print(
                "visual: tamper check ok (the comparator rejected the corrupted "
                f"capture); full capture at {keep_hint}"
            )
            return 0
        if args.expect_mismatch:
            keep_scratch = True
            print("visual: mismatch confirmed as requested")
            return 0
        keep_scratch = True
        print(f"  captured frame: {keep_hint}")
        print("visual: run --update only for a deliberate, reviewed golden change")
        return 1
    finally:
        if not keep_scratch:
            shutil.rmtree(scratch, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
