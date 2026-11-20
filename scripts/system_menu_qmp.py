#!/usr/bin/env python3
"""Open the M2.6.4 System menu and About panel, then capture the frame.

This is a verification harness, not part of the kernel. It boots the desktop
image (`build/igis.iso`, whose boot entry is `desktop_shell`), waits for
`desktop_shell: ready`, clicks the `System` menu-bar label with QMP absolute
pointer events, clicks the `About igis-v2` pull-down row, waits for
`desktop_shell: about open`, and captures the guest framebuffer through QMP
`screendump`. It then injects Escape through the PS/2 keyboard, waits for
`desktop_shell: about closed`, and reports both the screenshot and the
menu/About path.

Usage:
    cargo xtask image
    python3 scripts/system_menu_qmp.py build/igis.iso
    python3 scripts/system_menu_qmp.py build/igis.iso --cores 1 \
        --output /tmp/igis-system-menu.png

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

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SHOT_DIR = os.environ.get("IGIS_SHOT_DIR", os.path.join(ROOT, "build", "screenshots"))
os.makedirs(SHOT_DIR, exist_ok=True)

DEFAULT_OUTPUT = os.path.join(SHOT_DIR, "igis-system-menu.png")

# The menu bar's `System` label sits after `Terminal` (measured prop widths:
# File 24, Apps 56, Windows 94, Terminal 153..194, System starts at 208). Its
# pull-down frame is (204, 22, 224, 66); row 0 (About) is y 23..38. Click it at
# y = 24, above the kernel Terminal frame (y >= 26): since M8.1 a title-bar
# press is the service-owned drag (W22) and is consumed before the client.
SYSTEM_LABEL = (224, 10)
ABOUT_ROW = (260, 24)


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
    fd, ppm = tempfile.mkstemp(suffix=".ppm", prefix="igis-system-")
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


def click(qmp, x, y, width, height):
    hid_x = hid_axis(x, width)
    hid_y = hid_axis(y, height)
    qmp.send_absolute(hid_x, hid_y, left=False)
    qmp.send_absolute(hid_x, hid_y, left=True)
    qmp.send_absolute(hid_x, hid_y, left=False)


def send_key(qmp, qcode):
    """One press and release through QEMU's PS/2 keyboard path."""
    for down in (True, False):
        qmp.command(
            "input-send-event",
            {
                "events": [
                    {
                        "type": "key",
                        "data": {
                            "down": down,
                            "key": {"type": "qcode", "data": qcode},
                        },
                    }
                ]
            },
        )


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
        tempfile.gettempdir(), f"igis-system-menu-{os.getpid()}.qmp"
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
        serial.wait_for(r"desktop_shell: ready", timeout=args.timeout)
        graphics, _ = serial.wait_for(r"graphics: (\d+)x(\d+)", timeout=10)
        width, height = int(graphics.group(1)), int(graphics.group(2))
        print(f"[harness] screen {width}x{height}")

        # Open the System pull-down from the menu bar, then its first item.
        click(qmp, *SYSTEM_LABEL, width, height)
        time.sleep(0.3)
        click(qmp, *ABOUT_ROW, width, height)
        serial.wait_for(r"desktop_shell: about open", timeout=args.timeout)
        print("[harness] About panel opened from the System menu")

        # Give the full-content present time to reach the framebuffer.
        time.sleep(0.5)
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

        # Escape closes the modal panel through the keyboard path.
        send_key(qmp, "esc")
        serial.wait_for(r"desktop_shell: about closed", timeout=args.timeout)
        print("[harness] Escape closed the About panel")
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
