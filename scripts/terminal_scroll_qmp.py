#!/usr/bin/env python3
"""Prove the terminal emulator survives a long output burst that scrolls it.

This is the terminal-scroll regression harness (spec 0021; the reported case
"terminal freezes and crashes when it has to scroll down (cursor reaches end of
the text area)"). The cause was `scroll_one` copying all 48 `string[256]` rows
on every scroll; `cat` of a large source forces ~1,800 scrolls and the terminal
task hit its normal-task CPU/step watchdog (`Faulted(BudgetExceeded)`). The
emulator now keeps the grid as a ring and advances `top`, so a scroll is O(1).

The script:
1. boots `build/igis.iso`, opens `Apps` and launches `Terminal`;
2. waits for the shell prompt and cats `apps/productivity/browser` (the largest
   source in the image) to scroll far past the 48 visible rows;
3. waits for the prompt to return, requires no
   `task <terminal>: exit Faulted(BudgetExceeded)` line, then types
   `echo terminal-alive` and requires the reply, proving the emulator and its
   shell are still running;
4. optionally captures the final frame.

Usage:
    cargo xtask image
    python3 scripts/terminal_scroll_qmp.py build/igis.iso
    python3 scripts/terminal_scroll_qmp.py build/igis.iso --cores 1
"""

import argparse
import os
import re
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
DEFAULT_OUTPUT = os.path.join(SHOT_DIR, "igis-terminal-scroll.png")

# `desktop_shell` menu geometry: Terminal is row 10, y = 22 + 1 + 10*16 + 8.
APPS_LABEL = (64, 10)
TERMINAL_ROW = (140, 191)
LARGE_SOURCE = "apps/productivity/browser"
PROMPT = r"(?:shell> |igis:/[^>]*> )"
ALIVE = "terminal-alive"


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
    time.sleep(0.25)


def type_text(process, text):
    """Serial bytes follow the focused window owner's console session."""
    for byte in text.encode():
        process.stdin.write(bytes([byte]))
        process.stdin.flush()
        time.sleep(0.01)


def capture(qmp, path):
    try:
        qmp.command("screendump", {"filename": path, "format": "png"})
        return path
    except RuntimeError as error:
        print(f"[harness] screendump PNG refused ({error}); using PPM")
    fd, ppm = tempfile.mkstemp(suffix=".ppm", prefix="igis-term-scroll-")
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


def halted(serial):
    return any(
        "scheduler: idle summary" in line or line.startswith("halt:")
        for line in serial.lines
    )


def run(args):
    iso = os.path.abspath(args.iso)
    if not os.path.exists(iso):
        sys.exit(f"missing ISO: {iso}")

    output = os.path.abspath(args.output)
    if not args.no_capture:
        os.makedirs(os.path.dirname(output) or ".", exist_ok=True)

    socket_path = os.path.join(tempfile.gettempdir(), f"igis-term-scroll-{os.getpid()}.qmp")
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
        print(f"[harness] screen {width}x{height}, cores {args.cores}")

        click(qmp, *APPS_LABEL, width, height)
        serial.wait_for(r"desktop_shell: apps menu open", timeout=args.timeout)
        click(qmp, *TERMINAL_ROW, width, height)
        match, cursor = serial.wait_for(
            r"repl: apps/utilities/terminal: spawned as task (\d+)",
            timeout=args.timeout,
        )
        terminal_task = int(match.group(1))
        print(f"[harness] terminal task {terminal_task}")
        serial.wait_for(r"repl: base/shell: spawned as task", timeout=args.timeout, start=cursor)
        _, cursor = serial.wait_for(PROMPT, timeout=args.timeout, start=cursor)

        # A large source forces far more lines than the 48 visible rows and
        # therefore ~1,800 scrolls. Its own text contains Faulted(...) mentions,
        # so the fault check below is anchored to a task-exit line.
        print(f"[harness] cat {LARGE_SOURCE} (scrolls the whole grid)")
        type_text(process, f"cat {LARGE_SOURCE}\n")
        cursor = serial.wait_for(PROMPT, timeout=args.timeout, start=cursor)[1]
        time.sleep(0.5)

        fault = re.compile(
            rf"^task {terminal_task}: exit Faulted\(BudgetExceeded\)"
        )
        faults = [line for line in serial.lines if fault.search(line)]
        if faults:
            print(f"[harness] FAIL: {faults[-1]}")
            return 1

        # The emulator and its shell are still alive and responsive.
        type_text(process, f"echo {ALIVE}\n")
        serial.wait_for(rf"^{ALIVE}$", timeout=args.timeout, start=cursor)
        print("[harness] terminal still answers after the scroll")

        if halted(serial):
            print("[harness] FAIL: the transcript shows an idle summary or halt")
            return 1

        if not args.no_capture:
            capture(qmp, output)
            print(f"[harness] screenshot {output}")
        print("[harness] PASS: terminal survived the scroll and stayed interactive")
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
    parser.add_argument("--output", default=DEFAULT_OUTPUT)
    parser.add_argument("--cores", default="4")
    parser.add_argument("--timeout", type=float, default=90.0)
    parser.add_argument("--no-capture", action="store_true", help="skip the final PNG")
    args = parser.parse_args()
    return run(args)


if __name__ == "__main__":
    sys.exit(main())
