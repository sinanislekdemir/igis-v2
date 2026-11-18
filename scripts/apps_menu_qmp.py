#!/usr/bin/env python3
"""Open the desktop's Apps pull-down, launch apps, and use the menu bar.

This is a verification harness, not part of the kernel. It has two regression
proofs through the real boot desktop:

1. M8.3 (modal popup grab). The menu is application-drawn and has no window, so
   before the grab a click in a lower row was routed to the window underneath
   (the kernel Terminal at y >= 26) and never reached the menu. The `Clock` row
   is deliberately at y = 94, over the Terminal's content.
2. M8.4 (persistent pointer-region claim). After `File Browser` is launched it
   also blocks in `next_event`; without the bar claim it can consume the
   `window = 0` press on a later `Terminal` bar item (or split the
   move/down/up sequence) and the shell never launches. The harness opens Apps
   again while the browser is live, dismisses it with an outside click, then
   clicks `Terminal` and requires `terminal: ok`.

The transcript proof is:

  desktop_shell: apps menu open
  desktop_shell: launched Clock task N
  clock_window: ok                 (empty args: the deterministic check)
  desktop_shell: launched File Browser task N
  repl: base/browser: spawned as task N
  desktop_shell: launched shell task N
  terminal: ok

and no `scheduler: idle summary`/`halt:` line. The script never opens the image
for viewing: it only reads its size and dimensions.

Usage:
    cargo xtask image
    python3 scripts/apps_menu_qmp.py build/igis.iso
    python3 scripts/apps_menu_qmp.py build/igis.iso --cores 1 --no-capture
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

DEFAULT_OUTPUT = "/home/sinan/Desktop/igis-apps-menu.png"

# `desktop_shell` layout_menu: File starts at x = 24, so Apps starts at x = 56
# and the pull-down frame is (52, 22, 176, 98). Row 4 (`Clock`) is at
# y = 23 + 4*16 = 87..102; its centre is y = 94, inside the kernel Terminal
# window (frame y >= 26). That overlap is the regression the grab fixes.
APPS_LABEL = (64, 10)
CLOCK_ROW = (140, 94)
# `File Browser` is row 3: y = 22 + 3 * 16 + 8 = 78, also over the Terminal.
BROWSER_ROW = (140, 78)
SYSTEM_LABEL = (224, 10)
OUTSIDE = (700, 400)
# The desktop's direct `Terminal` item (measured prop widths, x 153..194).
TERMINAL_ITEM = (170, 10)


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
    fd, ppm = tempfile.mkstemp(suffix=".ppm", prefix="igis-apps-")
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
    parser.add_argument("--timeout", type=float, default=180.0)
    args = parser.parse_args()

    iso = os.path.abspath(args.iso)
    if not os.path.exists(iso):
        sys.exit(f"missing ISO: {iso}")

    output = os.path.abspath(args.output)
    if not args.no_capture:
        os.makedirs(os.path.dirname(output) or ".", exist_ok=True)

    socket_path = os.path.join(
        tempfile.gettempdir(), f"igis-apps-menu-{os.getpid()}.qmp"
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
        _, cursor = serial.wait_for(r"desktop_shell: ready", timeout=args.timeout)
        graphics, _ = serial.wait_for(r"graphics: (\d+)x(\d+)", timeout=10)
        width, height = int(graphics.group(1)), int(graphics.group(2))
        print(f"[harness] screen {width}x{height}")

        # Open Apps and capture the pull-down.
        click(qmp, *APPS_LABEL, width, height)
        _, cursor = serial.wait_for(
            r"desktop_shell: apps menu open", timeout=args.timeout, start=cursor
        )
        print("[harness] Apps pull-down opened")
        if not args.no_capture:
            time.sleep(0.3)
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

        # Select Clock: this row lies over the kernel Terminal window, which is
        # exactly the click the pre-grab routing stole. Empty args run the
        # app's deterministic check, so it exits on its own. The desktop's own
        # launch line is printed in several `core.write` calls, so the app's
        # own output is the deterministic gate; the launch line is asserted
        # against the joined transcript at the end.
        click(qmp, *CLOCK_ROW, width, height)
        _, cursor = serial.wait_for(r"clock_window: ok", timeout=args.timeout, start=cursor)
        print("[harness] Clock launched from a row over the Terminal")
        if halted(serial):
            print("[harness] FAIL: the system halted after the Apps launch")
            return 1

        # M8.4 regression. `File Browser` keeps running and blocks in
        # `next_event`, competing for the one machine-wide queue. Before the
        # persistent menu-bar region claim it could consume a later
        # `window = 0` `Terminal` bar press (or split its move/down/up
        # sequence), so the shell never launched. The desktop's launch line can
        # be interleaved with other tasks' output, so gate on the REPL's own
        # spawn line.
        click(qmp, *APPS_LABEL, width, height)
        _, cursor = serial.wait_for(
            r"desktop_shell: apps menu open", timeout=args.timeout, start=cursor
        )
        click(qmp, *BROWSER_ROW, width, height)
        _, cursor = serial.wait_for(
            r"repl: base/browser: spawned as task", timeout=args.timeout, start=cursor
        )
        print("[harness] File Browser launched; it now competes for the queue")
        time.sleep(0.5)
        # A second Apps open/close while the browser is live: the opening bar
        # click must reach the desktop (region claim) and the open menu must
        # own the outside click (modal grab).
        click(qmp, *APPS_LABEL, width, height)
        _, cursor = serial.wait_for(
            r"desktop_shell: apps menu open", timeout=args.timeout, start=cursor
        )
        click(qmp, *OUTSIDE, width, height)
        _, cursor = serial.wait_for(
            r"desktop_shell: apps menu closed", timeout=args.timeout, start=cursor
        )
        print("[harness] second Apps menu dismissed while the browser is live")

        # Open System and dismiss it with an outside click (the grab is modal
        # and the click is consumed, which is the documented behaviour).
        click(qmp, *SYSTEM_LABEL, width, height)
        time.sleep(0.3)
        click(qmp, *OUTSIDE, width, height)
        time.sleep(0.3)
        print("[harness] System pull-down dismissed by an outside click")

        # The desktop must still be interactive: the live browser is blocked in
        # next_event, and the bar claim is what keeps this press for the shell.
        click(qmp, *TERMINAL_ITEM, width, height)
        serial.wait_for(
            r"desktop_shell: launched shell task", timeout=args.timeout, start=cursor
        )
        serial.wait_for(r"shell> ", timeout=args.timeout)
        process.stdin.write(b"terminal\n")
        process.stdin.flush()
        serial.wait_for(r"terminal: ok", timeout=args.timeout)
        print("[harness] desktop survived; Terminal item reached `terminal: ok`")

        transcript = "\n".join(serial.lines)
        if "desktop_shell: launched Clock" not in transcript:
            print("[harness] FAIL: no `desktop_shell: launched Clock` line")
            return 1
        if "desktop_shell: launched File Browser" not in transcript:
            print("[harness] FAIL: no `desktop_shell: launched File Browser` line")
            return 1
        if "repl: base/browser: spawned as task" not in transcript:
            print("[harness] FAIL: the browser was not spawned")
            return 1
        if "desktop_shell: launched shell" not in transcript:
            print("[harness] FAIL: no `desktop_shell: launched shell` line")
            return 1

        if halted(serial):
            print("[harness] FAIL: the system halted after the terminal proof")
            return 1
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
