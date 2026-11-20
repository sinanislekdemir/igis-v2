#!/usr/bin/env python3
"""Prove the window minimize box and the window-list restore path.

Boots the product image, presses the System Log's minimize box, checks that the
log pixels are gone (the desktop shows through), opens `File > Window List`,
clicks the `System Log` row and checks that the log is painted again.

The same service box appears on every window title bar; this script uses the
kernel log because it is always present at boot.

Usage:
    cargo xtask image
    python3 scripts/minimize_qmp.py build/igis.iso
    python3 scripts/minimize_qmp.py build/igis.iso --cores 1
"""

import argparse
import os
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
sys.dont_write_bytecode = True

from tablet_qmp import Qmp, Serial  # noqa: E402

# `console::open_windows`: log content (8, 26) 1000x712, frame 1002x729.
# The hide box is the 11x11 square at frame + (w - 15, 3).
LOG_HIDE = (995 + 4, 26 + 8)
# The Apps label and a blank point inside its pull-down (frame 52,22,220,178).
APPS_LABEL = (64, 10)
APPS_MENU_PIXEL = (265, 100)
OUTSIDE = (700, 400)
# The desktop `File` item starts at x = 24 and is 26 px wide.
FILE_LABEL = (30, 10)
# The Window List opens content 460x300 at (320, 140); the first row is
# `Desktop`, the second (minimized bottom) is `System Log`.
WINDOW_LIST_SYSTEM_LOG = (400, 184)
# A blank lower-right log pixel; the log background is 0xf0f0f0.
LOG_PIXEL = (900, 700)


def hid_axis(pixel, extent):
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
    time.sleep(0.3)


def capture(qmp, path):
    qmp.command("screendump", {"filename": path, "format": "png"})
    return path


def pixel(path, point):
    from PIL import Image

    with Image.open(path) as image:
        return image.convert("RGB").getpixel(point)


def run(args):
    iso = os.path.abspath(args.iso)
    if not os.path.exists(iso):
        sys.exit(f"missing ISO: {iso}")

    socket_path = os.path.join(
        tempfile.gettempdir(), f"igis-minimize-{os.getpid()}.qmp"
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
        str(args.cores),
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
    scratch = tempfile.mkdtemp(prefix="igis-minimize-")
    try:
        qmp = Qmp(socket_path, timeout=args.timeout)
        serial.wait_for(r"desktop_shell: ready", timeout=args.timeout)
        graphics, _ = serial.wait_for(r"graphics: (\d+)x(\d+)", timeout=10)
        width, height = int(graphics.group(1)), int(graphics.group(2))
        print(f"[harness] screen {width}x{height}, cores {args.cores}")
        time.sleep(0.5)

        before = capture(qmp, os.path.join(scratch, "before.png"))
        before_pixel = pixel(before, LOG_PIXEL)
        print(f"[harness] log pixel before minimize {before_pixel}")
        if before_pixel != (240, 240, 240):
            print("[harness] FAIL: the System Log background was not visible")
            return 1

        # Open Apps and sweep the pointer across its rows. The log must not
        # repaint over the app-drawn pull-down while the modal grab is active.
        click(qmp, *APPS_LABEL, width, height)
        serial.wait_for(r"desktop_shell: apps menu open", timeout=args.timeout)
        for y in (47, 63, 100, 159, 191):
            qmp.send_absolute(
                hid_axis(140, width), hid_axis(y, height), left=False
            )
            time.sleep(0.08)
        time.sleep(0.3)
        menu = capture(qmp, os.path.join(scratch, "menu.png"))
        menu_pixel = pixel(menu, APPS_MENU_PIXEL)
        print(f"[harness] Apps menu pixel during pointer sweep {menu_pixel}")
        if menu_pixel == (240, 240, 240):
            print("[harness] FAIL: the System Log overdraws the Apps menu")
            return 1
        click(qmp, *OUTSIDE, width, height)
        serial.wait_for(r"desktop_shell: apps menu closed", timeout=args.timeout)
        time.sleep(0.3)

        click(qmp, *LOG_HIDE, width, height)
        time.sleep(0.5)
        hidden = capture(qmp, os.path.join(scratch, "hidden.png"))
        hidden_pixel = pixel(hidden, LOG_PIXEL)
        print(f"[harness] log pixel after minimize {hidden_pixel}")
        if hidden_pixel == (240, 240, 240):
            print("[harness] FAIL: the minimized System Log is still painted")
            return 1

        # A pointer pass over the menu bar must not bring the log back.
        qmp.send_absolute(hid_axis(64, width), hid_axis(10, height), left=False)
        time.sleep(0.2)
        after_move = capture(qmp, os.path.join(scratch, "move.png"))
        moved_pixel = pixel(after_move, LOG_PIXEL)
        print(f"[harness] log pixel after menu-bar move {moved_pixel}")
        if moved_pixel == (240, 240, 240):
            print("[harness] FAIL: the log repainted while minimized")
            return 1

        # File -> Window List -> the System Log row restores it.
        click(qmp, *FILE_LABEL, width, height)
        serial.wait_for(r"desktop_shell: window list open", timeout=args.timeout)
        time.sleep(0.3)
        click(qmp, *WINDOW_LIST_SYSTEM_LOG, width, height)
        serial.wait_for(r"desktop_shell: raised window", timeout=args.timeout)
        time.sleep(0.6)
        restored = capture(qmp, os.path.join(scratch, "restored.png"))
        restored_pixel = pixel(restored, LOG_PIXEL)
        print(
            f"[harness] log pixel after restore {restored_pixel}; "
            f"mid {pixel(restored, (500, 300))}; top {pixel(restored, (500, 100))}"
        )
        if restored_pixel != (240, 240, 240):
            print("[harness] FAIL: the System Log did not repaint after restore")
            return 1

        print("[harness] PASS: minimize and window-list restore")
        return 0
    finally:
        if qmp is not None:
            try:
                qmp.command("quit")
            except Exception:
                pass
        process.terminate()
        process.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("iso", help="path to build/igis.iso")
    parser.add_argument("--cores", type=int, default=4)
    parser.add_argument("--timeout", type=float, default=90.0)
    args = parser.parse_args()
    return run(args)


if __name__ == "__main__":
    sys.exit(main())
