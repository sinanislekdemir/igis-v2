#!/usr/bin/env python3
"""Prove window-local client coordinates with a real drag (spec 0026 C4/C8/C9).

This is the M17.3 regression harness. It boots `build/igis-shell.iso` with
QEMU's absolute USB tablet, runs `local_coords ready` (which draws one frame,
reports its frame origin and then repaints on every owner `Expose`), and drags
the window by its title bar through the real pointer path while the owner is
between frames. The service moves the frame, the owner repaints without
re-reading any origin, and the completed frame must land at the new position:

  - a pixel inside the content's top-left corner mark is at the *new* content
    origin,
  - the same local point at the *old* content origin no longer holds the mark,
  - the bottom-right corner mark is at the new origin too, so the whole frame
    moved, not just its top-left.

The harness never displays the image: Pillow reads the captured framebuffer for
the pixel checks only, and the final capture is written to
`build/screenshots/igis-local-coords.png`. It fails if the app reports a
failure, a second frame origin never arrives, the moved origin disagrees with
the injected delta, or the scheduler prints the idle summary/halt.

Usage:
    cargo xtask image --shell
    python3 scripts/local_coords_qmp.py build/igis-shell.iso
    python3 scripts/local_coords_qmp.py build/igis-shell.iso --cores 1

Exit status is 0 when the drag lands the completed frame at the new origin.
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
os.makedirs(SHOT_DIR, exist_ok=True)

DEFAULT_OUTPUT = os.path.join(SHOT_DIR, "igis-local-coords.png")

# `samples/tests/apps/base/local_coords.ig`: content 240x160 at frame origin
# (200, 150); the title bar is frame rows 0..15 and the content starts at
# frame + (1, 16).
FRAME_START = (200, 150)
CONTENT_W = 240
CONTENT_H = 160
DRAG = (60, 40)
TITLE_GRAB = (120, 8)

# The app's corner marks and paper.
MARK_A = (0x11, 0x66, 0x33)
MARK_B = (0xAA, 0x22, 0x66)


def hid_axis(pixel, extent):
    """Inverse of kernel/src/usb.rs: round to nearest, ties up."""
    if extent <= 1:
        return 0
    pixel = min(max(pixel, 0), extent - 1)
    return (pixel * 0x7FFF + (extent - 1) // 2) // (extent - 1)


def send(qmp, x, y, width, height, left):
    qmp.send_absolute(hid_axis(x, width), hid_axis(y, height), left=left)


def capture(qmp, path):
    """Writes a PNG to `path`; returns the path actually produced."""
    try:
        qmp.command("screendump", {"filename": path, "format": "png"})
        return path
    except RuntimeError as error:
        print(f"[harness] screendump PNG refused ({error}); using PPM")
    fd, ppm = tempfile.mkstemp(suffix=".ppm", prefix="igis-local-")
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
        for line in list(serial.lines)
    )


def run(args):
    iso = os.path.abspath(args.iso)
    if not os.path.exists(iso):
        sys.exit(f"missing ISO: {iso}")

    output = os.path.abspath(args.output)
    os.makedirs(os.path.dirname(output) or ".", exist_ok=True)

    socket_path = os.path.join(
        tempfile.gettempdir(), f"igis-local-coords-{os.getpid()}.qmp"
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
    ok = False
    try:
        qmp = Qmp(socket_path, timeout=args.timeout)
        serial.wait_for(r"(?:shell> |igis:/[^>]*> )", timeout=args.timeout)
        graphics, _ = serial.wait_for(r"graphics: (\d+)x(\d+)", timeout=10)
        width, height = int(graphics.group(1)), int(graphics.group(2))
        print(f"[harness] screen {width}x{height}, cores {args.cores}")

        process.stdin.write(b"run local_coords ready\n")
        process.stdin.flush()
        start, _ = serial.wait_for(
            r"local_coords: frame (\d+) (\d+)", timeout=args.timeout
        )
        frame_x, frame_y = int(start.group(1)), int(start.group(2))
        print(f"[harness] frame origin ({frame_x}, {frame_y})")
        if (frame_x, frame_y) != FRAME_START:
            sys.exit(f"unexpected frame origin: {(frame_x, frame_y)}")
        _, cursor = serial.wait_for(
            r"local_coords: ready", timeout=args.timeout
        )

        # Drag the title bar through the real pointer path. The press starts a
        # service-owned drag; the moves follow it; the release ends it.
        start_pt = (frame_x + TITLE_GRAB[0], frame_y + TITLE_GRAB[1])
        end_pt = (start_pt[0] + DRAG[0], start_pt[1] + DRAG[1])
        send(qmp, start_pt[0], start_pt[1], width, height, False)
        time.sleep(0.2)
        send(qmp, start_pt[0], start_pt[1], width, height, True)
        time.sleep(0.2)
        for index in range(1, 9):
            x = start_pt[0] + DRAG[0] * index // 8
            y = start_pt[1] + DRAG[1] * index // 8
            send(qmp, x, y, width, height, True)
            time.sleep(0.08)
        send(qmp, end_pt[0], end_pt[1], width, height, False)
        time.sleep(0.4)

        # The owner repainted on its Expose and reported the new frame origin.
        # The drag produces one repaint per move, so wait for the *final*
        # origin rather than the next line.
        new_x, new_y = frame_x + DRAG[0], frame_y + DRAG[1]
        serial.wait_for(
            rf"local_coords: frame {new_x} {new_y}\b",
            timeout=args.timeout,
            start=cursor,
        )
        print(f"[harness] frame after the drag ({new_x}, {new_y})")

        if any("local_coords: FAIL" in line for line in list(serial.lines)):
            sys.exit("local_coords reported a failure")

        # Give the composition a moment, then check the captured pixels.
        time.sleep(0.5)
        path = capture(qmp, output)
        from PIL import Image

        with Image.open(path) as image:
            image = image.convert("RGB")
            new_content = (new_x + 1, new_y + 16)
            old_content = (frame_x + 1, frame_y + 16)
            checks = [
                ("mark A at the new origin",
                 (new_content[0] + 5, new_content[1] + 5), MARK_A),
                ("mark B at the new origin",
                 (new_content[0] + CONTENT_W - 11, new_content[1] + CONTENT_H - 11),
                 MARK_B),
            ]
            for label, at, want in checks:
                saw = image.getpixel(at)
                if saw != want:
                    sys.exit(f"{label}: {at} is {saw}, want {want}")
                print(f"[harness] {label} ok")
            stale = image.getpixel((old_content[0] + 5, old_content[1] + 5))
            if stale == MARK_A:
                sys.exit("the old content origin still holds the corner mark")
            print("[harness] old origin repainted ok")

        if halted(serial):
            sys.exit("the transcript shows a halt/idle summary")
        if any(": Faulted(" in line for line in list(serial.lines)):
            sys.exit("a task faulted")
        print("[harness] capture:", path)
        print("local_coords: ok")
        ok = True
    finally:
        process.stdin.close()
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
    return 0 if ok else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("iso", help="build/igis-shell.iso")
    parser.add_argument("--cores", default="4")
    parser.add_argument("--output", default=DEFAULT_OUTPUT)
    parser.add_argument("--timeout", type=float, default=60.0)
    args = parser.parse_args()
    sys.exit(run(args))


if __name__ == "__main__":
    main()
