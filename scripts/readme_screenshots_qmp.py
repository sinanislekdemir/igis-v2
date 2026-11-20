#!/usr/bin/env python3
"""Compose the README screenshots on the real desktop.

The script boots `build/igis.iso` with the absolute tablet, minimizes the
System Log so the dither desktop shows, closes the shell's Desktop panel, then
opens a dark terminal with `ifconfig dhcp`, `nslookup` and `net_probe dns`
output, Calculator and Clock. It captures:

    docs/screenshots/desktop.png    terminal + calculator + clock
    docs/screenshots/network.png    the terminal frame (network commands)
    docs/screenshots/apps.png       the Apps pull-down over the desktop
    docs/screenshots/widgets.png    the Widgets window
    docs/screenshots/editor.png     the Editor window with highlighted source

All captures are framebuffer grabs through QMP `screendump`; Pillow is used
for the window crops only. The script never displays the images.

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
# Importing the sibling harness must not leave a __pycache__ in the repo.
sys.dont_write_bytecode = True

from tablet_qmp import Qmp, Serial  # noqa: E402

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT_DIR = os.path.join(ROOT, "docs", "screenshots")

APPS_LABEL = (64, 10)
CALCULATOR_ROW = (140, 47)
CLOCK_ROW = (140, 63)
EDITOR_ROW = (140, 143)
TERMINAL_ROW = (140, 191)
LOG_HIDE = (999, 34)
DESKTOP_CLOSE = (611, 83)
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


def crop(src, dst, box):
    from PIL import Image

    with Image.open(src) as image:
        image.crop(box).save(dst, format="PNG")
    print(f"[readme] {dst}")


def launch(qmp, serial, width, height, row, spawn, cursor):
    click(qmp, *APPS_LABEL, width, height)
    serial.wait_for(r"desktop_shell: apps menu open", timeout=30, start=cursor)
    click(qmp, *row, width, height)
    match, cursor = serial.wait_for(spawn, timeout=60, start=cursor)
    time.sleep(1.0)
    return cursor


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("iso", help="path to build/igis.iso")
    parser.add_argument("--cores", default="4")
    parser.add_argument("--timeout", type=float, default=90.0)
    args = parser.parse_args()

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
        print(f"[readme] screen {width}x{height}, cores {args.cores}")
        time.sleep(0.6)

        # A clean desktop: hide the System Log and close the shell's help panel.
        click(qmp, *LOG_HIDE, width, height)
        time.sleep(0.5)
        _, cursor = serial.wait_for(r"desktop_shell: desktop window open", timeout=30)
        click(qmp, *DESKTOP_CLOSE, width, height)
        _, cursor = serial.wait_for(
            r"desktop_shell: desktop window closed", timeout=30, start=cursor
        )
        time.sleep(0.4)

        # Dark terminal with the network surface.
        cursor = launch(
            qmp, serial, width, height, TERMINAL_ROW,
            r"repl: apps/utilities/terminal: spawned as task", cursor,
        )
        cursor = serial.wait_for(r"repl: base/shell: spawned as task", timeout=30, start=cursor)[1]
        cursor = serial.wait_for(PROMPT, timeout=30, start=cursor)[1]

        print("[readme] terminal: ifconfig dhcp, nslookup, net_probe dns")
        type_text(process, "ifconfig dhcp\n")
        _, cursor = serial.wait_for(r"ifconfig: dhcp 10\.0\.2\.15/24", timeout=60, start=cursor)
        type_text(process, "nslookup example.com\n")
        _, cursor = serial.wait_for(r"Address:", timeout=60, start=cursor)
        type_text(process, "net_probe dns\n")
        _, cursor = serial.wait_for(r"net_probe: ok", timeout=60, start=cursor)
        _, cursor = serial.wait_for(PROMPT, timeout=30, start=cursor)
        time.sleep(0.6)

        # Calculator top-right, Clock bottom-right.
        print("[readme] calculator + clock")
        cursor = launch(
            qmp, serial, width, height, CALCULATOR_ROW,
            r"repl: apps/accessories/calculator: spawned as task", cursor,
        )
        drag(qmp, (330, 108), (850, 88), width, height)
        cursor = launch(
            qmp, serial, width, height, CLOCK_ROW,
            r"repl: apps/accessories/clock_window: spawned as task", cursor,
        )
        drag(qmp, (440, 138), (800, 468), width, height)
        time.sleep(0.8)

        desktop = os.path.join(OUT_DIR, "desktop.png")
        capture(qmp, desktop)
        print(f"[readme] {desktop}")

        # Terminal-only crop for the network picture.
        crop(desktop, os.path.join(OUT_DIR, "network.png"),
             (24, 64, 24 + 642, 64 + 417))

        # Apps pull-down over the composed desktop.
        click(qmp, *APPS_LABEL, width, height)
        _, cursor = serial.wait_for(r"desktop_shell: apps menu open", timeout=30, start=cursor)
        time.sleep(0.3)
        capture(qmp, os.path.join(OUT_DIR, "apps.png"))
        print(f"[readme] {os.path.join(OUT_DIR, 'apps.png')}")
        click(qmp, *OUTSIDE, width, height)
        _, cursor = serial.wait_for(r"desktop_shell: apps menu closed", timeout=30, start=cursor)
        time.sleep(0.3)

        # Widgets: `shot` composes the gallery, holds it for three seconds and
        # closes itself; the terminal shell is the focused input target.
        print("[readme] widgets")
        type_text(process, "run apps/accessories/widgets shot\n")
        _, cursor = serial.wait_for(r"widgets: ready", timeout=60, start=cursor)
        time.sleep(0.3)
        capture(qmp, os.path.join(OUT_DIR, "_widgets_full.png"))
        crop(os.path.join(OUT_DIR, "_widgets_full.png"),
             os.path.join(OUT_DIR, "widgets.png"),
             (64, 64, 64 + 762, 64 + 577))
        os.unlink(os.path.join(OUT_DIR, "_widgets_full.png"))
        _, cursor = serial.wait_for(r"widgets: done", timeout=30, start=cursor)
        _, cursor = serial.wait_for(PROMPT, timeout=30, start=cursor)
        time.sleep(0.4)

        # Editor with the default highlighted source.
        print("[readme] editor")
        cursor = launch(
            qmp, serial, width, height, EDITOR_ROW,
            r"repl: apps/productivity/editor: spawned as task", cursor,
        )
        time.sleep(1.5)
        capture(qmp, os.path.join(OUT_DIR, "_editor_full.png"))
        crop(os.path.join(OUT_DIR, "_editor_full.png"),
             os.path.join(OUT_DIR, "editor.png"),
             (180, 90, 180 + 642, 90 + 497))
        os.unlink(os.path.join(OUT_DIR, "_editor_full.png"))
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


if __name__ == "__main__":
    sys.exit(main())
