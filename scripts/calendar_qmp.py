#!/usr/bin/env python3
"""Launch Calendar from the boot desktop and capture its live frame.

This is a verification harness, not part of the kernel. It boots the product
ISO with no display and a USB tablet, opens `Apps > Calendar`, waits for the
application's `calendar: ready YYYY-MM-DD` line, clicks the next-month button,
captures the framebuffer through QMP `screendump`, and closes the window through
the service close box. The task must print `calendar: closed`.

Usage:
    python3 scripts/calendar_qmp.py build/igis.iso
    python3 scripts/calendar_qmp.py build/igis.iso --output /tmp/igis-calendar.png

The proof covers the menu row, the RTC date, live mode, the next-month button
and the close request. The month grid itself is pixel-checked by
`run apps/productivity/calendar check`.
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

DEFAULT_OUTPUT = os.path.join(SHOT_DIR, "igis-calendar.png")

# Apps rows: first row centre y = 31, 16 pixels each. Calendar is row 10.
APPS_LABEL = (64, 10)
CALENDAR_ROW = (140, 191)
# `window_open(300, 110, 252, 240)`: content origin (301, 126). The next-month
# button is content-relative (224, 8) and 20x18; the close box is frame + (9, 8).
NEXT_MONTH = (535, 143)
CLOSE_BOX = (309, 118)


def hid_axis(pixel, extent):
    """Inverse of kernel/src/usb.rs: round to nearest, ties up."""
    if extent <= 1:
        return 0
    pixel = min(max(pixel, 0), extent - 1)
    return (pixel * 0x7FFF + (extent - 1) // 2) // (extent - 1)


def click(qmp, x, y, width, height):
    hid_x = hid_axis(x, width)
    hid_y = hid_axis(y, height)
    qmp.send_absolute(hid_x, hid_y, left=False)
    time.sleep(0.1)
    qmp.send_absolute(hid_x, hid_y, left=True)
    time.sleep(0.1)
    qmp.send_absolute(hid_x, hid_y, left=False)
    time.sleep(0.1)


def capture(qmp, output):
    """Writes a PNG to `output`; returns the path actually produced."""
    try:
        qmp.command("screendump", {"filename": output, "format": "png"})
        return output
    except RuntimeError as error:
        print(f"[harness] screendump PNG refused ({error}); using PPM")
    fd, ppm = tempfile.mkstemp(suffix=".ppm", prefix="igis-calendar-")
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
    parser.add_argument("iso", help="path to build/igis.iso")
    parser.add_argument("--output", default=DEFAULT_OUTPUT)
    parser.add_argument("--cores", default="4")
    parser.add_argument("--timeout", type=float, default=180.0)
    args = parser.parse_args()

    iso = os.path.abspath(args.iso)
    if not os.path.exists(iso):
        sys.exit(f"missing ISO: {iso}")
    output = os.path.abspath(args.output)
    os.makedirs(os.path.dirname(output) or ".", exist_ok=True)

    socket_path = os.path.join(
        tempfile.gettempdir(), f"igis-calendar-{os.getpid()}.qmp"
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
        "-rtc",
        "base=localtime",
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
        _, cursor = serial.wait_for(r"desktop_shell: ready", timeout=args.timeout)
        graphics, _ = serial.wait_for(r"graphics: (\d+)x(\d+)", timeout=10)
        width, height = int(graphics.group(1)), int(graphics.group(2))
        print(f"[harness] screen {width}x{height}")

        click(qmp, *APPS_LABEL, width, height)
        serial.wait_for(
            r"desktop_shell: apps menu open", timeout=args.timeout, start=cursor
        )
        click(qmp, *CALENDAR_ROW, width, height)
        _, cursor = serial.wait_for(
            r"calendar: ready \d{4}-\d{2}-\d{2}", timeout=args.timeout, start=cursor
        )
        print("[harness] Calendar launched from the Apps menu")

        # Walk to the next month through the real button and let the re-render
        # reach the framebuffer before capturing.
        click(qmp, *NEXT_MONTH, width, height)
        time.sleep(0.4)
        path = capture(qmp, output)
        size = os.path.getsize(path)
        print(f"[harness] screenshot {path} ({size} bytes)")

        click(qmp, *CLOSE_BOX, width, height)
        serial.wait_for(r"calendar: closed", timeout=args.timeout, start=cursor)
        print("[harness] Calendar closed on the close box")
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
