#!/usr/bin/env python3
"""Boot straight to the M2.5.6 desktop, launch the terminal, and capture it.

This is a verification harness, not part of the kernel. It boots the ISO with
the absolute USB tablet, waits for the boot desktop's `desktop_shell: ready`
line (no manual `run`), clicks the `Terminal` menu item through QMP absolute
pointer events, waits for the shell's window and prompt, types `terminal` over
serial, checks `terminal: ok`, and writes the guest framebuffer to a PNG through
QMP `screendump`.

Usage:
    cargo xtask image
    python3 scripts/boot_desktop_qmp.py build/igis.iso
    python3 scripts/boot_desktop_qmp.py build/igis.iso --cores 1 \
        --output /tmp/igis-boot-desktop.png

The script never opens the image for viewing: it only reads its size and
dimensions so the path can be reported.
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

DEFAULT_OUTPUT = "/home/sinan/Desktop/igis-boot-desktop.png"

# Centre of the desktop's direct `Terminal` item (x range 180..256, y 0..22).
TERMINAL_ITEM = (170, 10)


def hid_axis(pixel, extent):
    """Inverse of kernel/src/usb.rs: round to nearest, ties up."""
    if extent <= 1:
        return 0
    pixel = min(max(pixel, 0), extent - 1)
    return (pixel * 0x7FFF + (extent - 1) // 2) // (extent - 1)


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
    fd, ppm = tempfile.mkstemp(suffix=".ppm", prefix="igis-boot-")
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
    parser.add_argument("--timeout", type=float, default=90.0)
    args = parser.parse_args()

    iso = os.path.abspath(args.iso)
    if not os.path.exists(iso):
        sys.exit(f"missing ISO: {iso}")

    output = os.path.abspath(args.output)
    os.makedirs(os.path.dirname(output) or ".", exist_ok=True)

    socket_path = os.path.join(
        tempfile.gettempdir(), f"igis-boot-desktop-{os.getpid()}.qmp"
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
        # No manual `run`: the boot entry is the desktop.
        serial.wait_for(r"desktop_shell: ready", timeout=args.timeout)
        graphics, _ = serial.wait_for(r"graphics: (\d+)x(\d+)", timeout=10)
        width, height = int(graphics.group(1)), int(graphics.group(2))
        print(f"[harness] screen {width}x{height}")

        x, y = TERMINAL_ITEM
        hid_x = hid_axis(x, width)
        hid_y = hid_axis(y, height)
        qmp.send_absolute(hid_x, hid_y, left=False)
        qmp.send_absolute(hid_x, hid_y, left=True)
        qmp.send_absolute(hid_x, hid_y, left=False)

        serial.wait_for(r"desktop_shell: launched shell task", timeout=args.timeout)
        serial.wait_for(r"(?:shell> |igis:/[^>]*> )", timeout=args.timeout)
        print("[harness] shell launched from the Terminal menu")

        process.stdin.write(b"terminal\n")
        process.stdin.flush()
        serial.wait_for(r"terminal: ok", timeout=args.timeout)
        serial.wait_for(r"(?:shell> |igis:/[^>]*> )", timeout=args.timeout)
        # Give the final present time to reach the framebuffer.
        time.sleep(0.4)

        path = capture(qmp, output)
        size = os.path.getsize(path)
        dimensions = image_dimensions(path)
        if dimensions is None:
            print(f"[harness] screenshot {path} ({size} bytes, dimensions unknown)")
            return 1
        out_width, out_height = dimensions
        print(
            f"[harness] screenshot {path} ({size} bytes, {out_width}x{out_height})"
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
