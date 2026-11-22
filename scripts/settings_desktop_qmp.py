#!/usr/bin/env python3
"""Change the resolution from the boot desktop through the Settings app.

This is a verification harness, not part of the kernel. It boots the product
image, opens `Apps > Settings`, selects 800x600 in the Settings list, presses
Apply, and requires the guest framebuffer captured through QMP `screendump` to
be exactly 800x600. That proves the real mode change and the desktop shell's
screen refresh (spec 0022, D9/D10).

Usage:
    cargo xtask image
    python3 scripts/settings_desktop_qmp.py build/igis.iso
    python3 scripts/settings_desktop_qmp.py build/igis.iso --cores 1
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
SHOT_DIR = os.environ.get("IGIS_SHOT_DIR", os.path.join(ROOT, "build", "screenshots"))
os.makedirs(SHOT_DIR, exist_ok=True)
DEFAULT_OUTPUT = os.path.join(SHOT_DIR, "igis-desktop-800x600.png")

APPS_LABEL = (64, 10)
SETTINGS_ROW = (140, 207)
MODE_800X600 = (400, 186)
APPLY = (320, 315)


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


def image_dimensions(path):
    try:
        from PIL import Image

        with Image.open(path) as image:
            return image.size
    except Exception:
        pass
    import subprocess as sp

    try:
        output = sp.check_output(["identify", "-format", "%w %h", path], text=True)
        width, height = output.split()
        return int(width), int(height)
    except Exception as error:
        print(f"[harness] cannot read dimensions: {error}")
        return None


def capture(qmp, output):
    try:
        qmp.command("screendump", {"filename": output, "format": "png"})
        return output
    except RuntimeError as error:
        print(f"[harness] screendump PNG refused ({error}); using PPM")
    fd, ppm = tempfile.mkstemp(suffix=".ppm", prefix="igis-settings-desktop-")
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
        tempfile.gettempdir(), f"igis-settings-desktop-{os.getpid()}.qmp"
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
        print(f"[harness] boot screen {width}x{height}")

        click(qmp, *APPS_LABEL, width, height)
        _, cursor = serial.wait_for(
            r"desktop_shell: apps menu open", timeout=args.timeout, start=cursor
        )
        click(qmp, *SETTINGS_ROW, width, height)
        _, cursor = serial.wait_for(
            r"desktop_shell: launched Settings task", timeout=args.timeout, start=cursor
        )
        print("[harness] Settings opened")
        time.sleep(0.6)

        click(qmp, *MODE_800X600, width, height)
        time.sleep(0.2)
        click(qmp, *APPLY, width, height)
        time.sleep(1.0)

        path = capture(qmp, output)
        size = os.path.getsize(path)
        dimensions = image_dimensions(path)
        if dimensions is None:
            print(f"[harness] screenshot {path} ({size} bytes, dimensions unknown)")
            return 1
        out_width, out_height = dimensions
        print(f"[harness] screenshot {path} ({size} bytes, {out_width}x{out_height})")
        if (out_width, out_height) != (800, 600):
            print(f"[harness] FAIL: expected 800x600, got {out_width}x{out_height}")
            return 1
        print("[harness] ok: desktop mode changed to 800x600")
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
