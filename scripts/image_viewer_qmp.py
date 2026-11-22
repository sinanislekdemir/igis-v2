#!/usr/bin/env python3
"""Capture the PNG/PPM image viewer's first frame from a headless QEMU boot.

This is a verification harness, not part of the kernel. It boots the `--shell`
ISO (the text shell, so no application competes for input), types

    run image_viewer image/simon-twukN12EN7c-unsplash.ppm shot

waits for the viewer's one deterministic line

    image_viewer: ready 4608x2963 -> 576x371

and writes the guest framebuffer to a PNG through QMP `screendump`. The `shot`
mode keeps the viewer pumping events after the ready line, so the captured
frame is the fit-to-window PPM preview with its progress strip at `Ready`. If
the QEMU build rejects the PNG format, it dumps a PPM and converts it with PIL.

Usage:
    cargo xtask image --shell
    python3 scripts/image_viewer_qmp.py
    python3 scripts/image_viewer_qmp.py build/igis-shell.iso --cores 1
    python3 scripts/image_viewer_qmp.py --output /tmp/igis-image-viewer.png

There is no golden hash: the proof is that the ready line appears on both core
counts and that the screenshot is a non-empty PNG of the screen.

The script never opens the source image for viewing; it only reads the
screenshot's size and, when PIL is present, its dimensions.
"""

import argparse
import os
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
# Importing the sibling harness must not leave a __pycache__ in the repo.
sys.dont_write_bytecode = True

from tablet_qmp import Qmp, Serial, apps_drive  # noqa: E402

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DEFAULT_ISO = os.path.join(ROOT, "build", "igis-shell.iso")
DEFAULT_HOME = os.path.join(ROOT, "build", "home.img")
DEFAULT_OUTPUT = "/home/sinan/Desktop/igis-image-viewer.png"

SOURCE = "image/simon-twukN12EN7c-unsplash.ppm"
READY_RE = r"image_viewer: ready (\d+)x(\d+) -> (\d+)x(\d+)"
PROMPT_RE = r"(?:shell> |igis:/[^>]*> )"


def image_dimensions(path):
    """Width and height of a PNG/PPM without displaying it."""
    try:
        from PIL import Image

        with Image.open(path) as image:
            return image.size
    except Exception:
        pass
    try:
        output = subprocess.check_output(
            ["identify", "-format", "%w %h", path], text=True
        )
        width, height = output.split()
        return int(width), int(height)
    except Exception as error:  # pragma: no cover - reporting only
        print(f"[harness] cannot read dimensions: {error}")
        return None


def capture(qmp, output):
    """Writes a PNG to `output`; returns the path actually produced."""
    try:
        qmp.command("screendump", {"filename": output, "format": "png"})
        return output
    except RuntimeError as error:
        print(f"[harness] screendump PNG refused ({error}); using PPM")
    fd, ppm = tempfile.mkstemp(suffix=".ppm", prefix="igis-image-viewer-")
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


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("iso", nargs="?", default=DEFAULT_ISO,
                        help="path to build/igis-shell.iso")
    parser.add_argument("--output", default=DEFAULT_OUTPUT)
    parser.add_argument("--cores", default="4")
    parser.add_argument("--timeout", type=float, default=600.0)
    args = parser.parse_args()

    iso = os.path.abspath(args.iso)
    if not os.path.exists(iso):
        sys.exit(f"missing ISO: {iso}")
    home = DEFAULT_HOME
    if not os.path.exists(home):
        sys.exit(f"missing home disk: {home} (run `cargo xtask image --shell`)")

    output = os.path.abspath(args.output)
    os.makedirs(os.path.dirname(output) or ".", exist_ok=True)

    socket_path = os.path.join(
        tempfile.gettempdir(), f"igis-image-viewer-{os.getpid()}.qmp"
    )
    try:
        os.unlink(socket_path)
    except FileNotFoundError:
        pass

    command = [
        "qemu-system-x86_64",
        "-cdrom",
        iso,
        *apps_drive(iso),
        "-m",
        "256M",
        "-smp",
        args.cores,
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
        "-drive",
        f"file={home},format=raw,if=ide",
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
        qmp = Qmp(socket_path, timeout=args.timeout)
        serial.wait_for(PROMPT_RE, timeout=args.timeout)
        # Type one byte at a time: a single burst can be reordered or dropped by
        # the serial poll under load, and the shell would see a different word.
        for byte in f"run image_viewer {SOURCE} shot\n".encode():
            process.stdin.write(bytes([byte]))
            process.stdin.flush()
            time.sleep(0.02)
        match, _ = serial.wait_for(READY_RE, timeout=args.timeout)
        src_w, src_h, out_w, out_h = (int(match.group(index)) for index in range(1, 5))
        print(
            f"[harness] viewer ready: {src_w}x{src_h} -> {out_w}x{out_h} "
            f"on {args.cores} core(s)"
        )
        # Give the first full present time to reach the framebuffer.
        time.sleep(0.5)

        path = capture(qmp, output)
        size = os.path.getsize(path)
        with open(path, "rb") as image:
            signature = image.read(8)
        if size <= 8 or signature != b"\x89PNG\r\n\x1a\n":
            print(f"[harness] FAIL: {path} is not a non-empty PNG")
            return 1
        dimensions = image_dimensions(path)
        if dimensions is None:
            print(f"[harness] screenshot {path} ({size} bytes, dimensions unknown)")
        else:
            width, height = dimensions
            print(f"[harness] screenshot {path} ({size} bytes, {width}x{height})")
        try:
            qmp.command("quit")
        except RuntimeError:
            pass
        print(
            f"image_viewer_qmp: PASS {path} ({size} bytes, "
            f"{src_w}x{src_h} -> {out_w}x{out_h})"
        )
        return 0
    finally:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
        if qmp is not None:
            qmp.close()
        try:
            os.unlink(socket_path)
        except FileNotFoundError:
            pass


if __name__ == "__main__":
    sys.exit(main())
