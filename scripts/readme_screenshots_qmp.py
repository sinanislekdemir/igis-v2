#!/usr/bin/env python3
"""Compose the README screenshots on the boot desktop, top bar included.

Every image is a full 1024x768 QEMU framebuffer capture with the desktop
shell's menu bar visible; nothing is cropped.

    docs/screenshots/desktop.png   terminal + Desktop panel + Calculator
    docs/screenshots/apps.png      Apps pull-down over that composition
    docs/screenshots/network.png   the terminal network commands
    docs/screenshots/widgets.png   the composed Widgets gallery
    docs/screenshots/clock.png     Clock
    docs/screenshots/browser.png   File Browser
    docs/screenshots/editor.png    Editor

The terminal runs `ifconfig dhcp`, `nslookup example.com` and `net_probe dns`.
Widgets needs its `shot` gallery, so it is started from the terminal and the
terminal is arranged fully behind its 760x560 frame; the terminal is then
minimized before the single-app captures.

Usage:
    cargo xtask image
    python3 scripts/readme_screenshots_qmp.py build/igis.iso
    python3 scripts/readme_screenshots_qmp.py build/igis.iso --cores 1
"""

import argparse
import os
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
sys.dont_write_bytecode = True

from tablet_qmp import Qmp, Serial, apps_drive  # noqa: E402

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT_DIR = os.path.join(ROOT, "docs", "screenshots")

APPS_LABEL = (64, 10)
CALCULATOR_ROW = (140, 47)
CLOCK_ROW = (140, 63)
EDITOR_ROW = (140, 143)
BROWSER_ROW = (140, 159)
TERMINAL_ROW = (140, 191)
OUTSIDE = (500, 700)
PROMPT = r"(?:shell> |igis:/[^>]*> )"


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
    time.sleep(0.3)


def drag(qmp, start, end, width, height):
    send = lambda x, y, left: qmp.send_absolute(
        hid_axis(x, width), hid_axis(y, height), left=left
    )
    send(start[0], start[1], False)
    time.sleep(0.25)
    send(start[0], start[1], True)
    time.sleep(0.25)
    steps = 8
    for step in range(1, steps + 1):
        x = start[0] + (end[0] - start[0]) * step // steps
        y = start[1] + (end[1] - start[1]) * step // steps
        send(x, y, True)
        time.sleep(0.08)
    send(end[0], end[1], False)
    time.sleep(0.1)
    send(end[0], end[1], False)
    time.sleep(1.0)


def type_text(process, text):
    for byte in text.encode():
        process.stdin.write(bytes([byte]))
        process.stdin.flush()
        time.sleep(0.01)


def capture(qmp, path):
    try:
        qmp.command("screendump", {"filename": path, "format": "png"})
        return path
    except RuntimeError as error:
        print(f"[readme] screendump PNG refused ({error}); using PPM")
    fd, ppm = tempfile.mkstemp(suffix=".ppm", prefix="igis-readme-")
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


def launch(qmp, serial, width, height, row, spawn, cursor):
    click(qmp, *APPS_LABEL, width, height)
    _, cursor = serial.wait_for(r"desktop_shell: apps menu open", timeout=30, start=cursor)
    click(qmp, *row, width, height)
    _, cursor = serial.wait_for(spawn, timeout=60, start=cursor)
    time.sleep(1.0)
    return cursor


def shot(qmp, path):
    capture(qmp, path)
    print(f"[readme] {path}")


def run(args):
    iso = os.path.abspath(args.iso)
    if not os.path.exists(iso):
        sys.exit(f"missing ISO: {iso}")
    os.makedirs(OUT_DIR, exist_ok=True)

    socket_path = os.path.join(tempfile.gettempdir(), f"igis-readme-{os.getpid()}.qmp")
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
        serial.wait_for(r"desktop_shell: ready", timeout=args.timeout)
        graphics, _ = serial.wait_for(r"graphics: (\d+)x(\d+)", timeout=10)
        width, height = int(graphics.group(1)), int(graphics.group(2))
        print(f"[readme] screen {width}x{height}, cores {args.cores}")
        time.sleep(0.6)

        # Move the shell's Desktop panel to the right edge.
        drag(qmp, (750, 80), (812, 80), width, height)

        # Terminal at the left edge, running the network commands.
        cursor = launch(
            qmp, serial, width, height, TERMINAL_ROW,
            r"repl: apps/utilities/terminal: spawned as task", 0,
        )
        _, cursor = serial.wait_for(
            r"repl: base/shell: spawned as task", timeout=30, start=cursor
        )
        _, cursor = serial.wait_for(PROMPT, timeout=30, start=cursor)
        drag(qmp, (174, 72), (150, 72), width, height)

        print("[readme] terminal: ifconfig dhcp, nslookup, net_probe dns")
        type_text(process, "ifconfig dhcp\n")
        _, cursor = serial.wait_for(
            r"ifconfig: dhcp 10\.0\.2\.15/24", timeout=60, start=cursor
        )
        type_text(process, "nslookup example.com\n")
        _, cursor = serial.wait_for(r"Address:", timeout=60, start=cursor)
        type_text(process, "net_probe dns\n")
        _, cursor = serial.wait_for(r"net_probe: ok", timeout=60, start=cursor)
        _, cursor = serial.wait_for(PROMPT, timeout=30, start=cursor)
        time.sleep(0.5)

        # Calculator under the Desktop panel.
        cursor = launch(
            qmp, serial, width, height, CALCULATOR_ROW,
            r"repl: apps/accessories/calculator: spawned as task", cursor,
        )
        drag(qmp, (330, 108), (850, 288), width, height)
        time.sleep(0.8)
        shot(qmp, os.path.join(OUT_DIR, "desktop.png"))

        # Apps pull-down over that composition.
        click(qmp, *APPS_LABEL, width, height)
        _, cursor = serial.wait_for(r"desktop_shell: apps menu open", timeout=30, start=cursor)
        time.sleep(0.3)
        shot(qmp, os.path.join(OUT_DIR, "apps.png"))
        click(qmp, *OUTSIDE, width, height)
        _, cursor = serial.wait_for(
            r"desktop_shell: apps menu closed", timeout=30, start=cursor
        )

        # Close Calculator and the Desktop panel; keep the terminal and log.
        click(qmp, 709, 288, width, height)
        click(qmp, 671, 80, width, height)
        _, cursor = serial.wait_for(
            r"desktop_shell: desktop window closed", timeout=30, start=cursor
        )
        time.sleep(0.6)
        shot(qmp, os.path.join(OUT_DIR, "network.png"))

        # Widgets: arrange the terminal fully behind the 760x560 gallery.
        drag(qmp, (150, 72), (230, 88), width, height)
        type_text(process, "run apps/accessories/widgets shot\n")
        _, cursor = serial.wait_for(r"widgets: ready", timeout=60, start=cursor)
        time.sleep(0.3)
        shot(qmp, os.path.join(OUT_DIR, "widgets.png"))
        _, cursor = serial.wait_for(r"widgets: done", timeout=30, start=cursor)
        _, cursor = serial.wait_for(PROMPT, timeout=30, start=cursor)

        # Hide the terminal; its hide box is now on top and in the clear.
        click(qmp, 712, 88, width, height)
        time.sleep(0.5)

        # Clock.
        cursor = launch(
            qmp, serial, width, height, CLOCK_ROW,
            r"repl: apps/accessories/clock_window: spawned as task", cursor,
        )
        time.sleep(1.2)
        shot(qmp, os.path.join(OUT_DIR, "clock.png"))
        click(qmp, 350, 138, width, height)
        time.sleep(0.5)

        # File Browser.
        cursor = launch(
            qmp, serial, width, height, BROWSER_ROW,
            r"repl: apps/productivity/browser: spawned as task", cursor,
        )
        time.sleep(1.8)
        shot(qmp, os.path.join(OUT_DIR, "browser.png"))
        click(qmp, 170, 99, width, height)
        time.sleep(0.5)

        # Editor.
        cursor = launch(
            qmp, serial, width, height, EDITOR_ROW,
            r"repl: apps/productivity/editor: spawned as task", cursor,
        )
        time.sleep(1.8)
        shot(qmp, os.path.join(OUT_DIR, "editor.png"))
        click(qmp, 190, 99, width, height)
        print("[readme] done")
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
    parser.add_argument("--cores", default="4")
    parser.add_argument("--timeout", type=float, default=90.0)
    args = parser.parse_args()
    return run(args)


if __name__ == "__main__":
    sys.exit(main())
