#!/usr/bin/env python3
"""Prove a topmost window's content follows a drag and the client redraws there.

This is the M8.5 regression harness (spec 0011, W28; ADR 0046). It runs on the
real boot desktop with QEMU's absolute tablet:

1. Boot `build/igis.iso`, open `Apps` and launch `Calculator` (frame origin
   (180, 100), content 300x360). The desktop stays alive: since M8.6 queued
   input is delivered to the task that owns its window target (spec 0011, W29;
   ADR 0047), so a content click can no longer be stolen by the desktop and
   no close-the-desktop workaround is needed.
2. Drag the calculator by its title bar from (330, 108) to (580, 308), i.e. a
   frame origin of (430, 300). Capture the screen and verify programmatically
   that the calculator's display frame and panel border are present at the new
   content origin and gone from the old one. A window whose content did not
   follow would leave the underlay there.
3. Click the `7` button at the new content origin + (42, 138) and require the
   display-interior pixels to change. A client that kept the stale content
   rectangle would miss the button and the display would stay `0`.

The script never displays the image: Pillow reads the captured framebuffer for
the pixel checks only, and the final capture is written to
`build/screenshots/igis-drag.png`. It fails if the transcript shows the
scheduler idle summary or a `halt:` line.

Usage:
    cargo xtask image
    python3 scripts/drag_content_qmp.py build/igis.iso
    python3 scripts/drag_content_qmp.py build/igis.iso --cores 1 --no-capture
"""

import argparse
import os
import shutil
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
os.makedirs(SHOT_DIR, exist_ok=True)

DEFAULT_OUTPUT = os.path.join(SHOT_DIR, "igis-drag.png")

# `desktop_shell` menu geometry: `File` starts at x = 24, `Apps` at x = 56 and
# its pull-down frame is (52, 22, 176, 98). Row 0 (`Calculator`) is y 23..38.
APPS_LABEL = (64, 10)
CALCULATOR_ROW = (140, 30)

# `samples/apps/base/calculator`: WindowX/WindowY = (180, 100), content size
# 300x360, frame = content + (2, 17), so the title band starts at (180, 100).
FRAME_START = (180, 100)
FRAME_END = (430, 300)
TITLE_GRAB = (150, 8)          # inside the title bar, away from the close box
CONTENT_W = 300
CONTENT_H = 360

# The display is `DisplayX/DisplayY = (8, 10)`, 284x40, with a black 1-px
# outline; the panel itself has a black bezel at the content origin.
DISPLAY_X = 8
DISPLAY_Y = 10
DISPLAY_W = 284
DISPLAY_H = 40

# Button index 4 (`7`) is row 1, column 0: content + (42, 138).
BUTTON_SEVEN = (42, 138)

# Colours in the calculator content.
WHITE = (255, 255, 255)
BLACK = (0, 0, 0)


def hid_axis(pixel, extent):
    """Inverse of kernel/src/usb.rs: round to nearest, ties up."""
    if extent <= 1:
        return 0
    pixel = min(max(pixel, 0), extent - 1)
    return (pixel * 0x7FFF + (extent - 1) // 2) // (extent - 1)


def send(qmp, x, y, width, height, left):
    qmp.send_absolute(hid_axis(x, width), hid_axis(y, height), left=left)


def click(qmp, x, y, width, height):
    send(qmp, x, y, width, height, False)
    # Dwell so the polled tablet reports each button transition; a press and
    # release sent back to back can otherwise coalesce into "released".
    time.sleep(0.12)
    send(qmp, x, y, width, height, True)
    time.sleep(0.12)
    send(qmp, x, y, width, height, False)
    time.sleep(0.12)


def capture(qmp, path):
    """Writes a PNG to `path`; returns the path actually produced."""
    try:
        qmp.command("screendump", {"filename": path, "format": "png"})
        return path
    except RuntimeError as error:
        print(f"[harness] screendump PNG refused ({error}); using PPM")
    fd, ppm = tempfile.mkstemp(suffix=".ppm", prefix="igis-drag-")
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


def black_run(image, x0, x1, y):
    """Black pixels along a horizontal line."""
    return sum(1 for x in range(x0, x1) if image.getpixel((x, y)) == BLACK)


def black_column(image, x, y0, y1):
    """Black pixels along a vertical line."""
    return sum(1 for y in range(y0, y1) if image.getpixel((x, y)) == BLACK)


def differing_pixels(image_a, image_b, x0, y0, x1, y1):
    """Count and bounding box of differing pixels in a rectangle."""
    count = 0
    min_x = x1
    min_y = y1
    max_x = x0
    max_y = y0
    for y in range(y0, y1):
        for x in range(x0, x1):
            if image_a.getpixel((x, y)) != image_b.getpixel((x, y)):
                count += 1
                min_x = min(min_x, x)
                min_y = min(min_y, y)
                max_x = max(max_x, x)
                max_y = max(max_y, y)
    if count == 0:
        return 0, None
    return count, (min_x, min_y, max_x, max_y)


def halted(serial):
    """True when the transcript shows the idle-summary/halt path."""
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

    scratch = tempfile.mkdtemp(prefix="igis-drag-")
    before_path = os.path.join(scratch, "before-click.png")
    after_path = os.path.join(scratch, "after-click.png")

    socket_path = os.path.join(
        tempfile.gettempdir(), f"igis-drag-content-{os.getpid()}.qmp"
    )
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

        # Launch Calculator from Apps.
        click(qmp, *APPS_LABEL, width, height)
        _, cursor = serial.wait_for(
            r"desktop_shell: apps menu open", timeout=args.timeout
        )
        click(qmp, *CALCULATOR_ROW, width, height)
        _, cursor = serial.wait_for(
            r"desktop_shell: launched Calculator", timeout=args.timeout, start=cursor
        )
        _, cursor = serial.wait_for(
            r"repl: base/calculator: spawned as task",
            timeout=args.timeout,
            start=cursor,
        )
        # Give the child its first claim (window_open and the first frame).
        time.sleep(1.0)
        print("[harness] Calculator launched (desktop alive)")

        # Drag by the title bar from the initial origin to (430, 300).
        start = (FRAME_START[0] + TITLE_GRAB[0], FRAME_START[1] + TITLE_GRAB[1])
        end = (FRAME_END[0] + TITLE_GRAB[0], FRAME_END[1] + TITLE_GRAB[1])
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
        print(
            f"[harness] dragged the title bar {start} -> {end} "
            f"(frame origin {FRAME_END})"
        )

        capture(qmp, before_path)
        from PIL import Image

        with Image.open(before_path) as handle:
            before = handle.convert("RGB")

        content_x = FRAME_END[0] + 1
        content_y = FRAME_END[1] + 16
        display_y = content_y + DISPLAY_Y
        display_x = content_x + DISPLAY_X
        old_content_x = FRAME_START[0] + 1
        old_content_y = FRAME_START[1] + 16

        new_edge = black_run(before, display_x, display_x + DISPLAY_W, display_y)
        new_border = black_column(before, content_x, content_y, content_y + CONTENT_H)
        old_edge = black_run(
            before,
            old_content_x + DISPLAY_X,
            old_content_x + DISPLAY_X + DISPLAY_W,
            old_content_y + DISPLAY_Y,
        )
        old_border = black_column(
            before, old_content_x, old_content_y, old_content_y + CONTENT_H
        )
        panel = before.getpixel((content_x + 2, content_y + 2))
        print(
            f"[harness] after drag: new display edge {new_edge}/{DISPLAY_W} black, "
            f"new panel border {new_border}/{CONTENT_H} black, "
            f"panel {panel}"
        )
        print(
            f"[harness] after drag: old display edge {old_edge}/{DISPLAY_W} black, "
            f"old panel border {old_border}/{CONTENT_H} black"
        )
        content_ok = (
            new_edge >= DISPLAY_W - 4
            and new_border >= CONTENT_H - 10
            and panel == WHITE
            and old_edge <= 10
            and old_border <= 10
        )
        if not content_ok:
            print("[harness] FAIL: the calculator content did not follow the move")
            return 1

        # Click `7` at the new content origin. The press targets the topmost
        # window there (W29), so it is routed to the calculator's refreshed
        # rectangle even with the desktop live; the display must change from
        # `0` to `7`.
        button = (content_x + BUTTON_SEVEN[0], content_y + BUTTON_SEVEN[1])
        print(f"[harness] clicking `7` at {button}")
        click(qmp, button[0], button[1], width, height)
        time.sleep(0.8)
        capture(qmp, after_path)
        with Image.open(after_path) as handle:
            after = handle.convert("RGB")

        display_left = display_x + 2
        display_top = display_y + 2
        changed, bbox = differing_pixels(
            before,
            after,
            display_left,
            display_top,
            display_left + DISPLAY_W - 4,
            display_top + DISPLAY_H - 4,
        )
        ink_before = sum(
            1
            for y in range(display_top, display_top + DISPLAY_H - 4)
            for x in range(display_left, display_left + DISPLAY_W - 4)
            if before.getpixel((x, y)) == BLACK
        )
        ink_after = sum(
            1
            for y in range(display_top, display_top + DISPLAY_H - 4)
            for x in range(display_left, display_left + DISPLAY_W - 4)
            if after.getpixel((x, y)) == BLACK
        )
        print(
            f"[harness] display region changed {changed} pixels at {bbox}; "
            f"ink {ink_before} -> {ink_after}"
        )
        if changed < 5:
            print("[harness] FAIL: the `7` click did not change the display")
            return 1

        if halted(serial):
            print("[harness] FAIL: the system halted after the drag/click proof")
            return 1

        if not args.no_capture:
            shutil.copyfile(after_path, output)
            size = os.path.getsize(output)
            print(f"[harness] screenshot {output} ({size} bytes)")
        print("[harness] PASS: content copied to the new origin and redrawn there")
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
        shutil.rmtree(scratch, ignore_errors=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("iso", help="path to build/igis.iso")
    parser.add_argument("--output", default=DEFAULT_OUTPUT)
    parser.add_argument("--no-capture", action="store_true")
    parser.add_argument("--cores", default="4")
    parser.add_argument("--timeout", type=float, default=180.0)
    return run(parser.parse_args())


if __name__ == "__main__":
    sys.exit(main())
