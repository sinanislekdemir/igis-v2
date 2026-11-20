#!/usr/bin/env python3
"""Prove the desktop prints its final halt notice before halting (S9a).

Boots `build/igis.iso`, clicks `System > Halt`, waits for the halt transcript,
and captures the paused framebuffer (`-no-shutdown` keeps QEMU alive after the
guest halts). The checks are:

- the screen is black except for the pointer and the message text (no System
  Log background, desktop dither or window chrome is left);
- white proportional ink sits in the vertical centre band, is reasonably wide
  and is centred horizontally (the message
  `You may now shut down your computer.`);
- the transcript shows `system: N of M APs stopped` and never
  `desktop_shell: halt returned`.

Usage:
    cargo xtask image
    python3 scripts/halt_screen_qmp.py build/igis.iso
    python3 scripts/halt_screen_qmp.py build/igis.iso --cores 1
"""

import argparse
import os
import re
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
DEFAULT_OUTPUT = os.path.join(SHOT_DIR, "igis-halt.png")

# `desktop_shell` menu geometry: `System` is the fourth direct label; its
# pull-down frame is (204, 22, 224, 66) and row 3 (`Halt`) is y 71..86.
SYSTEM_LABEL = (170, 10)
HALT_ROW = (260, 79)

BLACK = (0, 0, 0)
WHITE = (255, 255, 255)


def hid_axis(pixel, extent):
    if extent <= 1:
        return 0
    pixel = min(max(pixel, 0), extent - 1)
    return (pixel * 0x7FFF + (extent - 1) // 2) // (extent - 1)


def click(qmp, x, y, width, height):
    hid_x = hid_axis(x, width)
    hid_y = hid_axis(y, height)
    qmp.send_absolute(hid_x, hid_y, left=False)
    time.sleep(0.12)
    qmp.send_absolute(hid_x, hid_y, left=True)
    time.sleep(0.12)
    qmp.send_absolute(hid_x, hid_y, left=False)
    time.sleep(0.25)


def capture(qmp, path):
    try:
        qmp.command("screendump", {"filename": path, "format": "png"})
        return path
    except RuntimeError as error:
        print(f"[harness] screendump PNG refused ({error}); using PPM")
    fd, ppm = tempfile.mkstemp(suffix=".ppm", prefix="igis-halt-")
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


def inspect(path, width, height):
    from PIL import Image

    with Image.open(path) as handle:
        image = handle.convert("RGB")

    # The message is one proportional line centred vertically.
    top = height // 2 - 4
    bottom = height // 2 + 20

    # The background is black everywhere outside the message band and the
    # pointer. The pointer is at the last click (HALT_ROW), so ignore a small
    # box around it; the rest must have no log/dither/chrome colours.
    non_black = 0
    for y in range(0, height, 4):
        if top <= y <= bottom:
            continue
        for x in range(0, width, 4):
            if 250 <= x <= 275 and 70 <= y <= 100:
                continue
            if image.getpixel((x, y)) != BLACK:
                non_black += 1
    xs = []
    ys = []
    for y in range(top, bottom):
        for x in range(width):
            if image.getpixel((x, y)) == WHITE:
                xs.append(x)
                ys.append(y)
    if not xs:
        return non_black, 0, 0, 0
    return non_black, len(xs), min(xs), max(xs)


def run(args):
    iso = os.path.abspath(args.iso)
    if not os.path.exists(iso):
        sys.exit(f"missing ISO: {iso}")

    output = os.path.abspath(args.output)
    if not args.no_capture:
        os.makedirs(os.path.dirname(output) or ".", exist_ok=True)

    socket_path = os.path.join(tempfile.gettempdir(), f"igis-halt-{os.getpid()}.qmp")
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
        time.sleep(0.5)

        click(qmp, *SYSTEM_LABEL, width, height)
        time.sleep(0.4)
        click(qmp, *HALT_ROW, width, height)
        serial.wait_for(r"desktop_shell: halt", timeout=args.timeout)
        summary, _ = serial.wait_for(r"system: (\d+) of (\d+) APs stopped", timeout=args.timeout)
        time.sleep(0.5)

        path = os.path.join(tempfile.mkdtemp(prefix="igis-halt-"), "halt.png")
        capture(qmp, path)
        non_black, ink, left, right = inspect(path, width, height)
        print(
            f"[harness] background anomalies {non_black}; message ink {ink} px "
            f"in x {left}..{right}"
        )

        if any("desktop_shell: halt returned" in line for line in serial.lines):
            print("[harness] FAIL: core.halt returned to the desktop")
            return 1
        if int(summary.group(1)) > int(summary.group(2)):
            print("[harness] FAIL: the AP summary is inconsistent")
            return 1
        if non_black > 8:
            print("[harness] FAIL: the halt screen is not black outside the message")
            return 1
        if ink < 200 or right - left < 100:
            print("[harness] FAIL: the halt message is missing or too small")
            return 1
        centre = (left + right) // 2
        if abs(centre - width // 2) > 40:
            print(f"[harness] FAIL: the halt message is off centre ({centre})")
            return 1

        if not args.no_capture:
            capture(qmp, output)
            print(f"[harness] screenshot {output}")
        print("[harness] PASS: the desktop showed its final halt notice")
        return 0
    finally:
        if qmp is not None:
            try:
                qmp.command("quit")
            except Exception:
                pass
        process.terminate()
        try:
            process.wait(timeout=5)
        except Exception:
            pass


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("iso", help="path to build/igis.iso")
    parser.add_argument("--output", default=DEFAULT_OUTPUT)
    parser.add_argument("--cores", default="4")
    parser.add_argument("--timeout", type=float, default=90.0)
    parser.add_argument("--no-capture", action="store_true", help="skip the final PNG")
    args = parser.parse_args()
    return run(args)


if __name__ == "__main__":
    sys.exit(main())
