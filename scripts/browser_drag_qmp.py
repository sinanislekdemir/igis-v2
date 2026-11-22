#!/usr/bin/env python3
"""Drag File Browser and prove its layout follows the frame.

This is a verification harness, not part of the kernel. It reproduces the
M8.5 regression a user found by dragging the File Browser: the frame moved but
the client kept its old content-rectangle-derived layout (the list, preview
and status stayed at the old absolute coordinates, leaving stale rows in the
new frame). After M8.5 the service copies the topmost content snapshot and the
client re-reads the content rect and recomputes `compute_layout` on `Expose`.

The harness boots `build/igis.iso` (the boot desktop), launches File Browser
from Apps, drags the title bar, captures the framebuffer through QMP and checks
the new layout's black borders:
  - path separator at content + 21 spans the content width;
  - the list border at content + (8, 30) and the preview border at
    content + (378, 30) run the full 420-pixel body height;
  - the old absolute preview border (content_old + 378) and the old path
    separator are gone.
It never displays the image: Pillow reads the captured framebuffer for the
pixel checks only. The screenshot is written to
`build/screenshots/igis-browser-drag.png`.

Usage:
    cargo xtask image
    python3 scripts/browser_drag_qmp.py build/igis.iso
    python3 scripts/browser_drag_qmp.py build/igis.iso --cores 1 --no-capture
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
SHOT_DIR = os.environ.get("IGIS_SHOT_DIR", os.path.join(ROOT, "build", "screenshots"))
os.makedirs(SHOT_DIR, exist_ok=True)

DEFAULT_OUTPUT = os.path.join(SHOT_DIR, "igis-browser-drag.png")

# `desktop_shell` menu geometry: `File` starts at x = 24, `Apps` at x = 56 and
# its pull-down frame is (52, 22, 176, 98). Row 3 (`File Browser`) is
# y = 23 + 3*16 = 71..86; its centre is y = 78.
APPS_LABEL = (64, 10)
BROWSER_ROW = (140, 159)

# `samples/apps/apps/productivity/browser.ig` opens content 680x480 at (160, 90); the frame is
# content + (2, 17). The title bar grab is 300 pixels from the frame's left edge
# and 8 rows down. The target frame origin is (310, 225), which fits the
# 1024x768 screen (frame 682x497 ends at (992, 722)).
OLD_FRAME = (160, 90)
NEW_FRAME = (310, 225)
TITLE_GRAB = (300, 8)

CONTENT_W = 680
CONTENT_H = 480
LIST_LEFT = 8
LIST_TOP = 30
BODY_H = 420
PREVIEW_LEFT = 378
PATH_H = 22


def hid_axis(pixel, extent):
    """Inverse of kernel/src/usb.rs: round to nearest, ties up."""
    if extent <= 1:
        return 0
    pixel = min(max(pixel, 0), extent - 1)
    return (pixel * 0x7FFF + (extent - 1) // 2) // (extent - 1)


def send(qmp, x, y, left, width, height):
    qmp.send_absolute(hid_axis(x, width), hid_axis(y, height), left=left)
    time.sleep(0.08)


def click(qmp, x, y, width, height):
    send(qmp, x, y, False, width, height)
    send(qmp, x, y, True, width, height)
    send(qmp, x, y, False, width, height)


def capture(qmp, output):
    """Writes a PNG to `output`; returns the path actually produced."""
    try:
        qmp.command("screendump", {"filename": output, "format": "png"})
        return output
    except RuntimeError as error:
        print(f"[harness] screendump PNG refused ({error}); using PPM")
    fd, ppm = tempfile.mkstemp(suffix=".ppm", prefix="igis-browser-drag-")
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


def image_dimensions(path):
    try:
        from PIL import Image

        with Image.open(path) as image:
            return image.size
    except Exception:
        return None


def longest_black_run(image, y, x0, x1):
    best = 0
    run = 0
    for x in range(x0, x1):
        if image.getpixel((x, y)) == (0, 0, 0):
            run += 1
            if run > best:
                best = run
        else:
            run = 0
    return best


def black_column(image, x, y0, y1):
    return sum(1 for y in range(y0, y1) if image.getpixel((x, y)) == (0, 0, 0))


def halted(serial):
    return any(
        "scheduler: idle summary" in line or line.startswith("halt:")
        for line in list(serial.lines)
    )


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("iso", help="path to build/igis.iso")
    parser.add_argument("--output", default=DEFAULT_OUTPUT)
    parser.add_argument("--no-capture", action="store_true")
    parser.add_argument("--cores", default="4")
    parser.add_argument("--timeout", type=float, default=120.0)
    args = parser.parse_args()

    iso = os.path.abspath(args.iso)
    if not os.path.exists(iso):
        sys.exit(f"missing ISO: {iso}")

    output = os.path.abspath(args.output)
    if not args.no_capture:
        os.makedirs(os.path.dirname(output) or ".", exist_ok=True)

    socket_path = os.path.join(
        tempfile.gettempdir(), f"igis-browser-drag-{os.getpid()}.qmp"
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
        serial.wait_for(r"desktop_shell: apps menu open", timeout=args.timeout)
        click(qmp, *BROWSER_ROW, width, height)
        serial.wait_for(
            r"repl: apps/productivity/browser: spawned as task", timeout=args.timeout
        )
        print("[harness] File Browser launched")
        time.sleep(2.0)

        start = (OLD_FRAME[0] + TITLE_GRAB[0], OLD_FRAME[1] + TITLE_GRAB[1])
        end = (NEW_FRAME[0] + TITLE_GRAB[0], NEW_FRAME[1] + TITLE_GRAB[1])
        send(qmp, *start, False, width, height)
        send(qmp, *start, True, width, height)
        # The last held move must be the target: the release does not move the
        # window, so a release-only endpoint would stop one step short.
        for step in range(1, 7):
            x = start[0] + (end[0] - start[0]) * step // 6
            y = start[1] + (end[1] - start[1]) * step // 6
            send(qmp, x, y, True, width, height)
        send(qmp, *end, False, width, height)
        print(f"[harness] dragged the title bar {start} -> {end}")
        time.sleep(2.0)

        if halted(serial):
            print("[harness] FAIL: the system halted after the drag")
            return 1

        path = capture(qmp, output) if not args.no_capture else os.path.join(
            tempfile.gettempdir(), f"igis-browser-drag-{os.getpid()}.png"
        )
        if args.no_capture:
            qmp.command("screendump", {"filename": path, "format": "png"})
        time.sleep(0.2)

        from PIL import Image

        content_x = NEW_FRAME[0] + 1
        content_y = NEW_FRAME[1] + 16
        old_content_x = OLD_FRAME[0] + 1
        old_content_y = OLD_FRAME[1] + 16
        with Image.open(path) as raw:
            image = raw.convert("RGB")
            path_run = longest_black_run(
                image, content_y + PATH_H - 1, content_x, content_x + CONTENT_W
            )
            list_column = black_column(
                image,
                content_x + LIST_LEFT,
                content_y + LIST_TOP,
                content_y + LIST_TOP + BODY_H,
            )
            preview_column = black_column(
                image,
                content_x + PREVIEW_LEFT,
                content_y + LIST_TOP,
                content_y + LIST_TOP + BODY_H,
            )
            stale_preview = black_column(
                image,
                old_content_x + PREVIEW_LEFT,
                content_y + LIST_TOP,
                content_y + LIST_TOP + BODY_H,
            )
            old_path_run = longest_black_run(
                image,
                old_content_y + PATH_H - 1,
                old_content_x,
                old_content_x + CONTENT_W,
            )

        print(
            f"[harness] new path separator {path_run}/{CONTENT_W} black; "
            f"list border {list_column}/{BODY_H}; "
            f"preview border {preview_column}/{BODY_H}"
        )
        print(
            f"[harness] old layout is gone: stale preview {stale_preview}, "
            f"old path run {old_path_run}"
        )
        failures = 0
        if path_run < CONTENT_W - 4:
            print("[harness] FAIL: the path separator is not at the new origin")
            failures += 1
        if list_column < BODY_H - 4:
            print("[harness] FAIL: the list border is not at the new origin")
            failures += 1
        if preview_column < BODY_H - 4:
            print("[harness] FAIL: the preview border is not at the new origin")
            failures += 1
        if stale_preview >= BODY_H // 2:
            print("[harness] FAIL: the old preview border is still visible")
            failures += 1
        if old_path_run >= CONTENT_W // 2:
            print("[harness] FAIL: the old path separator is still visible")
            failures += 1
        if failures:
            return 1
        if path:
            dimensions = image_dimensions(path)
            suffix = (
                f", {dimensions[0]}x{dimensions[1]}"
                if dimensions is not None
                else ""
            )
            print(f"[harness] screenshot {path} ({os.path.getsize(path)} bytes{suffix})")
        print("[harness] PASS: File Browser layout followed the drag")
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
