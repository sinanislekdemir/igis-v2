#!/usr/bin/env python3
"""Prove a switch/move rebuilds the windows below instead of leaving stale pixels.

This is the W31 regression harness (spec 0011, ADR 0055; the reported case
"moving windows pollute the windows below until they are redrawn"). It runs on
the real boot desktop with QEMU's absolute tablet and Pillow pixel checks:

1. Boot `build/igis.iso`, open `Apps` and launch `Calculator`. The desktop
   shell's own `Desktop` window (frame origin (600, 72), content 360x164 with
   an 8x8 black marker at its bottom-right) is below it.
2. Drag the calculator to (600, 150), so it stays over the shell window and
   the marker: the calculator's display edge, panel border and white panel must
   survive. The shell repaints its whole content (it no longer refuses while
   not topmost), and the presented pixels are the service's desktop/chrome plus
   each owner's own `window_present`, never the damaged union.
3. Drag it to (700, 150), which covers the shell's content marker, then away to
   (100, 400). The exposed shell window must repaint its marker black; a
   client left stale would show the calculator's white panel there.
4. Capture the final frame to `build/screenshots/igis-window-repaint.png`.

The script never displays the image; Pillow reads the captured framebuffer for
the pixel checks only. It fails on a stale marker, a corrupted calculator, or a
scheduler idle/halt line in the transcript.

Usage:
    cargo xtask image
    python3 scripts/window_repaint_qmp.py build/igis.iso
    python3 scripts/window_repaint_qmp.py build/igis.iso --cores 1
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

from tablet_qmp import Qmp, Serial  # noqa: E402

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SHOT_DIR = os.environ.get("IGIS_SHOT_DIR", os.path.join(ROOT, "build", "screenshots"))
DEFAULT_OUTPUT = os.path.join(SHOT_DIR, "igis-window-repaint.png")

# `desktop_shell` menu geometry: `Apps` starts at x = 56; its pull-down frame is
# (52, 22, 176, 98), so row 0 (`Calculator`) is y 23..38.
APPS_LABEL = (64, 10)
CALCULATOR_ROW = (140, 47)

# `apps/accessories/calculator.ig`: frame origin (180, 100), content 300x360.
FRAME_START = (180, 100)
FRAME_OVER = (600, 150)
FRAME_COVER = (700, 150)
FRAME_AWAY = (100, 400)
TITLE_GRAB = (150, 8)
CONTENT_W = 300
CONTENT_H = 360
DISPLAY_X = 8
DISPLAY_Y = 10
DISPLAY_W = 284

# `desktop_shell.ig`: `Desktop` content is (601, 88, 360, 164); its marker is the
# last 8x8 square of the content.
MARKER = (956, 247)

WHITE = (255, 255, 255)
BLACK = (0, 0, 0)


def hid_axis(pixel, extent):
    """Inverse of arch/x86_64/src/usb.rs: round to nearest, ties up."""
    if extent <= 1:
        return 0
    pixel = min(max(pixel, 0), extent - 1)
    return (pixel * 0x7FFF + (extent - 1) // 2) // (extent - 1)


def send(qmp, x, y, width, height, left):
    qmp.send_absolute(hid_axis(x, width), hid_axis(y, height), left=left)


def click(qmp, x, y, width, height):
    send(qmp, x, y, width, height, False)
    time.sleep(0.12)
    send(qmp, x, y, width, height, True)
    time.sleep(0.12)
    send(qmp, x, y, width, height, False)
    time.sleep(0.12)


def drag(qmp, frm, to, width, height):
    start = (frm[0] + TITLE_GRAB[0], frm[1] + TITLE_GRAB[1])
    end = (to[0] + TITLE_GRAB[0], to[1] + TITLE_GRAB[1])
    send(qmp, start[0], start[1], width, height, False)
    time.sleep(0.25)
    send(qmp, start[0], start[1], width, height, True)
    time.sleep(0.25)
    steps = 8
    for step in range(1, steps + 1):
        x = start[0] + (end[0] - start[0]) * step // steps
        y = start[1] + (end[1] - start[1]) * step // steps
        send(qmp, x, y, width, height, True)
        time.sleep(0.08)
    send(qmp, end[0], end[1], width, height, False)
    time.sleep(0.1)
    send(qmp, end[0], end[1], width, height, False)
    time.sleep(1.0)


def capture(qmp, path):
    """Writes a PNG to `path`; returns the path actually produced."""
    try:
        qmp.command("screendump", {"filename": path, "format": "png"})
        return path
    except RuntimeError as error:
        print(f"[harness] screendump PNG refused ({error}); using PPM")
    fd, ppm = tempfile.mkstemp(suffix=".ppm", prefix="igis-repaint-")
    os.close(fd)
    try:
        qmp.command("screendump", {"filename": ppm})
        from PIL import Image

        with Image.open(ppm) as image:
            image.save(path, format="PNG")
        return path
    finally:
        try:
            os.unlink(ppm)
        except FileNotFoundError:
            pass


def marker_of(path):
    from PIL import Image

    with Image.open(path) as handle:
        return handle.convert("RGB").getpixel(MARKER)


def calculator_pixels(path):
    from PIL import Image

    with Image.open(path) as handle:
        image = handle.convert("RGB")
    cx = FRAME_OVER[0] + 1
    cy = FRAME_OVER[1] + 16
    dx = cx + DISPLAY_X
    dy = cy + DISPLAY_Y
    edge = sum(1 for x in range(dx, dx + DISPLAY_W) if image.getpixel((x, dy)) == BLACK)
    border = sum(1 for y in range(cy, cy + CONTENT_H) if image.getpixel((cx, y)) == BLACK)
    panel = image.getpixel((cx + 2, cy + 2))
    return edge, border, panel


def halted(serial):
    return any(
        "scheduler: idle summary" in line or line.startswith("halt:")
        for line in list(serial.lines)
    )


def run(args):
    iso = os.path.abspath(args.iso)
    if not os.path.exists(iso):
        sys.exit(f"missing ISO: {iso}")

    output = os.path.abspath(args.output)
    if not args.no_capture:
        os.makedirs(os.path.dirname(output) or ".", exist_ok=True)

    scratch = tempfile.mkdtemp(prefix="igis-repaint-")
    over_path = os.path.join(scratch, "over.png")
    covered_path = os.path.join(scratch, "covered.png")
    away_path = os.path.join(scratch, "away.png")

    socket_path = os.path.join(tempfile.gettempdir(), f"igis-repaint-{os.getpid()}.qmp")
    try:
        os.unlink(socket_path)
    except FileNotFoundError:
        pass

    command = [
        "qemu-system-x86_64",
        "-cdrom",
        iso,
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
        "-usb",
        "-device",
        "usb-tablet",
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
        serial.wait_for(r"desktop_shell: ready", timeout=args.timeout)
        graphics, _ = serial.wait_for(r"graphics: (\d+)x(\d+)", timeout=10)
        width, height = int(graphics.group(1)), int(graphics.group(2))
        print(f"[harness] screen {width}x{height}, cores {args.cores}")

        click(qmp, *APPS_LABEL, width, height)
        _, cursor = serial.wait_for(r"desktop_shell: apps menu open", timeout=args.timeout)
        click(qmp, *CALCULATOR_ROW, width, height)
        serial.wait_for(
            r"repl: apps/accessories/calculator: spawned as task",
            timeout=args.timeout,
            start=cursor,
        )
        # Give the child its first claim (window_open and the first frame).
        time.sleep(1.0)

        # The calculator stays over the shell window: its own pixels must not
        # be damaged by the lower window's full-content repaint/present.
        drag(qmp, FRAME_START, FRAME_OVER, width, height)
        capture(qmp, over_path)
        edge, border, panel = calculator_pixels(over_path)
        print(
            f"[harness] over the shell: display edge {edge}/{DISPLAY_W} black, "
            f"panel border {border}/{CONTENT_H} black, panel {panel}"
        )
        if edge < DISPLAY_W - 4 or border < CONTENT_H - 10 or panel != WHITE:
            print("[harness] FAIL: the lower window corrupted the dragged content")
            return 1

        # Cover the shell's content marker, then move away.
        drag(qmp, FRAME_OVER, FRAME_COVER, width, height)
        capture(qmp, covered_path)
        covered = marker_of(covered_path)
        drag(qmp, FRAME_COVER, FRAME_AWAY, width, height)
        capture(qmp, away_path)
        restored = marker_of(away_path)
        print(f"[harness] shell marker covered {covered}, after the move {restored}")
        if covered == BLACK:
            print("[harness] FAIL: the calculator did not cover the shell marker")
            return 1
        if restored != BLACK:
            print("[harness] FAIL: the uncovered shell window left stale pixels")
            return 1

        if halted(serial):
            print("[harness] FAIL: the transcript shows an idle summary or halt")
            return 1

        if not args.no_capture:
            capture(qmp, output)
            print(f"[harness] screenshot {output}")
        print("[harness] PASS: covered windows repaint and dragged content stays intact")
        return 0
    finally:
        if qmp is not None:
            qmp.close()
        process.kill()
        try:
            process.wait(timeout=5)
        except Exception:
            pass


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("iso", help="path to build/igis.iso")
    parser.add_argument("--output", default=DEFAULT_OUTPUT)
    parser.add_argument("--cores", default="4")
    parser.add_argument("--timeout", type=float, default=90.0)
    parser.add_argument("--no-capture", action="store_true", help="skip the final PNG")
    args = parser.parse_args()
    return run(args)


if __name__ == "__main__":
    sys.exit(main())
