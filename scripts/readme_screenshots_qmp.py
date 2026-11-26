#!/usr/bin/env python3
"""Compose the README screenshots on the boot desktop, top bar included.

Every image is a full 1024x768 QEMU framebuffer capture with the desktop
shell's menu bar visible; nothing is cropped and nothing is drawn by the
harness. Window geometry comes from the sample sources, not from constants
guessed at capture time:

    content = window_open(x, y, w, h) + (1, 16)
    frame   = content - (1, 16) and content + (2, 17)
    close   = (frame.x + 9, frame.y + 8)          (11x11 box at frame + (4, 3))
    hide    = (frame.x + frame.w - 10, frame.y + 8) (11x11 box at the right end)

The Apps pull-down opens at `y = 22` (the bar height) with 16-pixel rows, so
row `i` is clicked at `y = 31 + 16 * i`; the `#` section headers are not
selectable. The System Log is a full-screen kernel window under everything, so
the harness minimizes it first (its hide box is free at boot); the uncovered
area is then the service desktop pattern.

    docs/screenshots/desktop.png   terminal + Calculator + Desktop panel
    docs/screenshots/apps.png      Apps pull-down over that composition
    docs/screenshots/network.png   the terminal network commands
    docs/screenshots/widgets.png   the composed Widgets gallery
    docs/screenshots/clock.png     Clock
    docs/screenshots/browser.png   File Browser
    docs/screenshots/editor.png    Editor

The terminal runs `ifconfig dhcp`, `nslookup example.com` and `net_probe dns`.
Widgets needs its `shot` gallery, so it is started from the terminal and the
terminal is arranged fully behind its 762x577 frame; the terminal is then
closed before the single-app captures, which all launch from Apps.

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

# The bar is 22 px tall and every pull-down row is 16 px with a 1 px border.
APPS_LABEL = (64, 10)
MENU_X = 140
OUTSIDE = (500, 700)
PROMPT = r"(?:shell> |igis:/[^>]*> )"


def menu_row(index):
    return (MENU_X, 31 + 16 * index)


# Apps item order (`samples/apps/base/desktop_shell.ig`, spec 0012 D20):
#  0 #Accessories  1 Calculator   2 Clock        3 Widgets   4 Image Viewer
#  5 #Games        6 Minesweeper  7 #Productivity 8 Editor   9 File Browser
# 10 #Utilities   11 Terminal    12 Settings
ROW_CALCULATOR = 1
ROW_CLOCK = 2
ROW_WIDGETS = 3
ROW_EDITOR = 8
ROW_BROWSER = 9
ROW_TERMINAL = 11

# `window_open` arguments (frame origin and content size), from each sample.
LOG = (8, 26, 1002, 729)          # kernel System Log, content + chrome, opened bottom
PANEL = (600, 72, 362, 181)
TERMINAL = (24, 64, 642, 417)
CALCULATOR = (180, 100, 302, 377)
WIDGETS = (64, 64, 762, 577)
CLOCK = (340, 130, 262, 297)
BROWSER = (160, 90, 682, 497)
EDITOR = (180, 90, 642, 497)


def hid_axis(pixel, extent):
    if extent <= 1:
        return 0
    pixel = min(max(pixel, 0), extent - 1)
    return (pixel * 0x7FFF + (extent - 1) // 2) // (extent - 1)


def close_point(frame):
    return (frame[0] + 9, frame[1] + 8)


def hide_point(frame):
    return (frame[0] + frame[2] - 10, frame[1] + 8)


def title_point(frame):
    return (frame[0] + frame[2] // 2, frame[1] + 8)


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


def park(qmp, width, height):
    """Move the pointer to the empty desktop corner before a capture."""
    qmp.send_absolute(hid_axis(OUTSIDE[0], width), hid_axis(OUTSIDE[1], height), left=False)
    time.sleep(0.25)


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
    _, cursor = serial.wait_for(
        r"desktop_shell: apps menu open", timeout=30, start=cursor
    )
    click(qmp, *menu_row(row), width, height)
    _, cursor = serial.wait_for(spawn, timeout=90, start=cursor)
    time.sleep(1.0)
    return cursor


def shot(qmp, path, width, height):
    park(qmp, width, height)
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
        if (width, height) != (1024, 768):
            sys.exit(f"[readme] unexpected mode {width}x{height}, want 1024x768")
        print(f"[readme] screen {width}x{height}, cores {args.cores}")
        time.sleep(0.6)
        cursor = 0

        # The System Log is the bottom full-screen kernel window; minimize it so
        # the uncovered pixels are the desktop pattern rather than log text.
        click(qmp, *hide_point(LOG), width, height)
        time.sleep(0.4)

        # Desktop panel to the right edge; the service clamps the frame to the
        # screen, so it ends at x = 662 and leaves the left side to the terminal.
        drag(qmp, title_point(PANEL), (843, 80), width, height)
        panel = (662, 72, PANEL[2], PANEL[3])

        # Terminal at the left edge, running the network commands.
        cursor = launch(
            qmp,
            serial,
            width,
            height,
            ROW_TERMINAL,
            r"repl: apps/utilities/terminal: spawned as task",
            cursor,
        )
        _, cursor = serial.wait_for(
            r"repl: base/shell: spawned as task", timeout=30, start=cursor
        )
        _, cursor = serial.wait_for(PROMPT, timeout=30, start=cursor)
        drag(qmp, title_point(TERMINAL), (title_point(TERMINAL)[0] - 24, 72), width, height)
        terminal = (0, 64, TERMINAL[2], TERMINAL[3])

        print("[readme] terminal: ifconfig dhcp, nslookup, net_probe dns")
        type_text(process, "ifconfig dhcp\n")
        _, cursor = serial.wait_for(
            r"ifconfig: dhcp 10\.0\.2\.15/24", timeout=90, start=cursor
        )
        type_text(process, "nslookup example.com\n")
        _, cursor = serial.wait_for(r"Address:", timeout=90, start=cursor)
        type_text(process, "net_probe dns\n")
        _, cursor = serial.wait_for(r"net_probe: ok", timeout=90, start=cursor)
        _, cursor = serial.wait_for(PROMPT, timeout=30, start=cursor)
        time.sleep(0.5)

        # Calculator under the Desktop panel.
        cursor = launch(
            qmp,
            serial,
            width,
            height,
            ROW_CALCULATOR,
            r"repl: apps/accessories/calculator: spawned as task",
            cursor,
        )
        drag(qmp, title_point(CALCULATOR), (850, 278), width, height)
        calculator = (699, 270, CALCULATOR[2], CALCULATOR[3])
        time.sleep(0.8)
        shot(qmp, os.path.join(OUT_DIR, "desktop.png"), width, height)

        # Apps pull-down over that composition.
        click(qmp, *APPS_LABEL, width, height)
        _, cursor = serial.wait_for(
            r"desktop_shell: apps menu open", timeout=30, start=cursor
        )
        time.sleep(0.3)
        shot(qmp, os.path.join(OUT_DIR, "apps.png"), width, height)
        click(qmp, *OUTSIDE, width, height)
        _, cursor = serial.wait_for(
            r"desktop_shell: apps menu closed", timeout=30, start=cursor
        )

        # Close Calculator and the Desktop panel; keep the terminal alone.
        click(qmp, *close_point(calculator), width, height)
        time.sleep(0.5)
        click(qmp, *close_point(panel), width, height)
        _, cursor = serial.wait_for(
            r"desktop_shell: desktop window closed", timeout=30, start=cursor
        )
        time.sleep(0.6)
        shot(qmp, os.path.join(OUT_DIR, "network.png"), width, height)

        # Widgets: put the terminal fully behind the 762x577 gallery, run the
        # `shot` mode from the terminal's shell, then close the terminal.
        drag(qmp, title_point(terminal), (421, 108), width, height)
        terminal = (100, 100, TERMINAL[2], TERMINAL[3])
        type_text(process, "run apps/accessories/widgets shot\n")
        _, cursor = serial.wait_for(r"widgets: ready", timeout=90, start=cursor)
        time.sleep(0.3)
        shot(qmp, os.path.join(OUT_DIR, "widgets.png"), width, height)
        _, cursor = serial.wait_for(r"widgets: done", timeout=30, start=cursor)
        _, cursor = serial.wait_for(PROMPT, timeout=30, start=cursor)
        click(qmp, *close_point(terminal), width, height)
        time.sleep(0.6)

        # Clock.
        cursor = launch(
            qmp,
            serial,
            width,
            height,
            ROW_CLOCK,
            r"repl: apps/accessories/clock_window: spawned as task",
            cursor,
        )
        time.sleep(1.2)
        shot(qmp, os.path.join(OUT_DIR, "clock.png"), width, height)
        click(qmp, *close_point(CLOCK), width, height)
        time.sleep(0.6)

        # File Browser.
        cursor = launch(
            qmp,
            serial,
            width,
            height,
            ROW_BROWSER,
            r"repl: apps/productivity/browser: spawned as task",
            cursor,
        )
        time.sleep(2.0)
        shot(qmp, os.path.join(OUT_DIR, "browser.png"), width, height)
        click(qmp, *close_point(BROWSER), width, height)
        time.sleep(0.6)

        # Editor.
        cursor = launch(
            qmp,
            serial,
            width,
            height,
            ROW_EDITOR,
            r"repl: apps/productivity/editor: spawned as task",
            cursor,
        )
        time.sleep(2.0)
        shot(qmp, os.path.join(OUT_DIR, "editor.png"), width, height)
        click(qmp, *close_point(EDITOR), width, height)
        time.sleep(0.5)
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
    parser.add_argument("--timeout", type=float, default=120.0)
    args = parser.parse_args()
    return run(args)


if __name__ == "__main__":
    sys.exit(main())
