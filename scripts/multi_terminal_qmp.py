#!/usr/bin/env python3
"""Prove independent terminal instances on the boot desktop (spec 0021).

Boots the product image, opens Apps > Utilities > Terminal twice, and checks:

1. two emulator tasks and two shells exist;
2. the first terminal can `cd /home` while the second still reports `/`;
3. serial input follows window focus, and each emulator keeps its own cwd;
4. `exit` in the second terminal closes only that instance; the first
   terminal still answers a command.

Usage:
    cargo xtask image
    python3 scripts/multi_terminal_qmp.py build/igis.iso
    python3 scripts/multi_terminal_qmp.py build/igis.iso --cores 1
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

# Measured menu bar: Apps at x = 56, pull-down (52, 22, 220, 178).
# Terminal is row 10: y = 22 + 1 + 10*16 + 8 = 191.
APPS_LABEL = (64, 10)
TERMINAL_ROW = (140, 191)
# First emulator content origin (24, 64); second cascades by 28 pixels.
FIRST_FOCUS = (30, 200)
SECOND_FOCUS = (680, 200)


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
    time.sleep(0.2)


def type_text(process, text):
    for byte in text.encode():
        process.stdin.write(bytes([byte]))
        process.stdin.flush()
        time.sleep(0.02)


def halted(serial):
    return any(
        "scheduler: idle summary" in line or line.startswith("halt:")
        for line in list(serial.lines)
    )


def run(args):
    iso = os.path.abspath(args.iso)
    if not os.path.exists(iso):
        sys.exit(f"missing ISO: {iso}")

    socket_path = os.path.join(
        tempfile.gettempdir(), f"igis-multi-terminal-{os.getpid()}.qmp"
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
    try:
        qmp = Qmp(socket_path, timeout=args.timeout)
        serial.wait_for(r"desktop_shell: ready", timeout=args.timeout)
        graphics, _ = serial.wait_for(r"graphics: (\d+)x(\d+)", timeout=10)
        width, height = int(graphics.group(1)), int(graphics.group(2))
        print(f"[harness] screen {width}x{height}, cores {args.cores}")

        # First instance.
        click(qmp, *APPS_LABEL, width, height)
        click(qmp, *TERMINAL_ROW, width, height)
        serial.wait_for(
            r"repl: apps/utilities/terminal: spawned as task", timeout=args.timeout
        )
        serial.wait_for(
            r"repl: base/shell: spawned as task", timeout=args.timeout
        )
        serial.wait_for(r"(?:shell> |igis:/[^>]*> )", timeout=args.timeout)

        # Second instance, while the first is live.
        click(qmp, *APPS_LABEL, width, height)
        click(qmp, *TERMINAL_ROW, width, height)
        serial.wait_for(
            r"repl: apps/utilities/terminal: spawned as task", timeout=args.timeout
        )
        serial.wait_for(
            r"repl: base/shell: spawned as task", timeout=args.timeout
        )
        serial.wait_for(r"(?:shell> |igis:/[^>]*> )", timeout=args.timeout)
        print("[harness] two terminal emulators and two shells are live")

        # Focus the first window (its title bar is above the second's frame)
        # and change only its cwd.
        click(qmp, *FIRST_FOCUS, width, height)
        type_text(process, "cd /home\n")
        serial.wait_for(r"igis:/home> ", timeout=args.timeout)
        print("[harness] first terminal is at /home")

        # Focus the second terminal; its cwd is still the root.
        click(qmp, *SECOND_FOCUS, width, height)
        type_text(process, "echo second\n")
        serial.wait_for(r"^second$", timeout=args.timeout)
        type_text(process, "pwd\n")
        serial.wait_for(r"^/$", timeout=args.timeout)
        print("[harness] second terminal still reports /")

        # Back to the first; it kept its own cwd.
        click(qmp, *FIRST_FOCUS, width, height)
        type_text(process, "pwd\n")
        serial.wait_for(r"^/home$", timeout=args.timeout)
        print("[harness] first terminal still reports /home")

        # Close only the second instance with `exit`.
        click(qmp, *SECOND_FOCUS, width, height)
        type_text(process, "exit\n")
        serial.wait_for(r"^bye$", timeout=args.timeout)
        time.sleep(0.5)
        if halted(serial):
            print("[harness] FAIL: the system halted after one terminal exited")
            return 1
        print("[harness] second terminal exited without halting the system")

        # The first instance still answers and kept its own cwd.
        click(qmp, *FIRST_FOCUS, width, height)
        type_text(process, "echo first-alive\n")
        serial.wait_for(r"^first-alive$", timeout=args.timeout)
        type_text(process, "pwd\n")
        serial.wait_for(r"^/home$", timeout=args.timeout)
        print("[harness] first terminal is still alive and at /home")

        print("[harness] PASS: independent terminals")
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
    parser.add_argument("--timeout", type=float, default=120.0)
    args = parser.parse_args()
    return run(args)


if __name__ == "__main__":
    sys.exit(main())
