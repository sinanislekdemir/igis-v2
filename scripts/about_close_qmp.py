#!/usr/bin/env python3
"""Close the desktop's About panel with its close box and prove the desktop lives.

This is a verification harness, not part of the kernel. It reproduces the M8.2
regression through the real boot desktop: boot `build/igis.iso` (the boot entry
is `desktop_shell`), open `System` -> `About igis-v2` with QMP absolute pointer
events, click the About close box, wait for `desktop_shell: about closed`, then
use the `Terminal` menu item and type `terminal`. A close box is an owner close
request (spec 0011, W23/W25), so the dialog closes itself and the desktop keeps
running; a kill-on-close build dies here and prints the scheduler idle summary.

The script checks the serial transcript for the success path and for the two
lines a halted system prints, then optionally captures the framebuffer. It
never opens the image for viewing: it only reads its size and dimensions.

Usage:
    cargo xtask image
    python3 scripts/about_close_qmp.py build/igis.iso
    python3 scripts/about_close_qmp.py build/igis.iso --cores 1 --no-capture
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

DEFAULT_OUTPUT = os.path.join(SHOT_DIR, "igis-about-close.png")

# Menu bar geometry from `desktop_shell` (measured prop widths): the `System`
# label starts at x = 208, and its pull-down frame is (204, 22, 224, 66); row 0
# (`About igis-v2`) is y 23..38. See scripts/system_menu_qmp.py.
SYSTEM_LABEL = (170, 10)
# Row 0 is clicked at y = 24, above the kernel Terminal frame (y >= 26). A
# title-bar press is the service-owned drag since M8.1 (W22) and is consumed
# before the client, so a click at y = 30 would never reach the menu.
ABOUT_ROW = (260, 24)
# The desktop's direct `Terminal` item (x range 180..256, y 0..22).
APPS_LABEL = (64, 10)
TERMINAL_ROW = (140, 239)

MENU_HEIGHT = 22
ABOUT_W = 420
ABOUT_H = 200


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
    # Small dwell so the polled tablet reports each button state; a press and
    # release sent back to back can otherwise coalesce into "released".
    time.sleep(0.1)
    qmp.send_absolute(hid_x, hid_y, left=True)
    time.sleep(0.1)
    qmp.send_absolute(hid_x, hid_y, left=False)
    time.sleep(0.1)


def about_close_box(width, height):
    """Centre of the About frame's close box, from `open_about`/`W23`.

    `window_open` takes the frame origin `(screen_w - aw) / 2,
    MenuHeight + (screen_h - MenuHeight - ah) / 2`; the close box is
    `frame + (4..15, 3..14)`, so its centre is `frame + (9, 8)`.
    """
    aw = min(ABOUT_W, width - 16)
    ah = min(ABOUT_H, height - MENU_HEIGHT - 16)
    x = (width - aw) // 2
    y = MENU_HEIGHT + ((height - MENU_HEIGHT - ah) // 2)
    return x + 9, y + 8


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
    fd, ppm = tempfile.mkstemp(suffix=".ppm", prefix="igis-about-")
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


def halted(serial):
    """True when the transcript shows the idle-summary/halt path."""
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
        tempfile.gettempdir(), f"igis-about-close-{os.getpid()}.qmp"
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
        print(f"[harness] screen {width}x{height}")

        # Open the System pull-down, then its first item (About).
        click(qmp, *SYSTEM_LABEL, width, height)
        time.sleep(0.3)
        click(qmp, *ABOUT_ROW, width, height)
        serial.wait_for(r"desktop_shell: about open", timeout=args.timeout)
        print("[harness] About panel opened")

        # Click the About close box: an owner close request, not a kill.
        close_x, close_y = about_close_box(width, height)
        print(f"[harness] About close box at ({close_x}, {close_y})")
        click(qmp, close_x, close_y, width, height)
        serial.wait_for(r"desktop_shell: about closed", timeout=args.timeout)
        print("[harness] close box closed only the About panel")
        time.sleep(0.3)

        if halted(serial):
            print("[harness] FAIL: the system halted after the close box")
            return 1

        # The desktop must still be interactive: launch the terminal and use it.
        click(qmp, *APPS_LABEL, width, height)
        time.sleep(0.2)
        click(qmp, *TERMINAL_ROW, width, height)
        serial.wait_for(
            r"repl: apps/utilities/terminal: spawned as task", timeout=args.timeout
        )
        serial.wait_for(r"(?:shell> |igis:/[^>]*> )", timeout=args.timeout)
        process.stdin.write(b"terminal\n")
        process.stdin.flush()
        serial.wait_for(r"terminal: ok", timeout=args.timeout)
        print("[harness] desktop survived; Terminal item reached `terminal: ok`")

        if halted(serial):
            print("[harness] FAIL: the system halted after the terminal proof")
            return 1

        if not args.no_capture:
            time.sleep(0.4)
            path = capture(qmp, output)
            size = os.path.getsize(path)
            dimensions = image_dimensions(path)
            if dimensions is None:
                print(f"[harness] screenshot {path} ({size} bytes, dimensions unknown)")
            else:
                out_width, out_height = dimensions
                print(
                    f"[harness] screenshot {path} "
                    f"({size} bytes, {out_width}x{out_height})"
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
