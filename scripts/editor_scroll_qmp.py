#!/usr/bin/env python3
"""Prove the editor's pointer path is cheap and its scrollbar thumb drags (M9.2).

This is the M9.2 regression harness for the report "task 4: exit
Faulted(BudgetExceeded) after 2893 claims (2891 yields) while trying to scroll
in the editor". That was the normal-task 10 s CPU budget, not a crash: before
M9.2 the interactive loop repainted the whole 640x480 panel (and the M9.1
highlighter rescanned the document for the block-comment state) for **every**
pointer event addressed to the editor, including a no-op move over the text.
Moving the pointer around the scrollbar therefore spent the task's lifetime
budget on frames nobody could see change.

The harness boots `build/igis-shell.iso` with the absolute USB tablet, runs the
interactive `editor` (it opens `base/child`, 28 lines in the 26-row view), and:

1. sends a burst of no-button absolute moves over the text (the user's shape),
2. grabs the scrollbar thumb and drags it to the bottom with a real held-button
   pointer drag,
3. captures before/after and requires the text view to change and the thumb to
   leave the top of the track,
4. sends Ctrl+Q and requires the editor's exit line to be `exit Ok` with no
   `BudgetExceeded` anywhere, the shell prompt to return, and no idle
   summary/halt.

The editor repaints only when a pointer event changes visible state after
M9.2, so the burst should cost almost nothing; the drag repaints only while
`top_line` changes. The claims count in the exit line is printed as evidence
that the guest really consumed the burst (QEMU/USB can coalesce if events are
sent too fast; `--move-delay-ms`/`--drag-delay-ms` pace them).

Usage:
    cargo xtask image --shell
    python3 scripts/editor_scroll_qmp.py build/igis-shell.iso
    python3 scripts/editor_scroll_qmp.py build/igis-shell.iso --cores 1
    python3 scripts/editor_scroll_qmp.py build/igis-shell.iso --no-capture

Exit status is 0 when the editor exits `Ok`, the prompt returns, the text view
scrolled and no budget fault was printed.
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

from tablet_qmp import Qmp, Serial  # noqa: E402

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SHOT_DIR = os.environ.get("IGIS_SHOT_DIR", os.path.join(ROOT, "build", "screenshots"))
os.makedirs(SHOT_DIR, exist_ok=True)

DEFAULT_OUTPUT = os.path.join(SHOT_DIR, "igis-editor-scroll.png")

# `samples/apps/apps/productivity/editor.ig` opens content 640x480 at frame origin (180, 90), so
# the content origin is (181, 106). TextLeft 4 / TextTop 38 / ScrollLeft 620 /
# ScrollTop 38 / ScrollW 16 / ScrollH 416 (client constants in the sample).
EDITOR_CONTENT = (181, 106)
TEXT_X = EDITOR_CONTENT[0] + 4
TEXT_Y = EDITOR_CONTENT[1] + 38
SCROLL_X = EDITOR_CONTENT[0] + 620
SCROLL_Y = EDITOR_CONTENT[1] + 38
SCROLL_W = 16
SCROLL_H = 416

# A generous text area for the before/after comparison: the pointer moves stay
# inside it and the scroll shifts every visible row.
TEXT_REGION = (TEXT_X + 10, TEXT_Y + 10, 560, 320)
# The top of the scrollbar track: gray while the thumb is at `top_line = 0`,
# white once the drag moved it down.
THUMB_TOP_STRIP = (SCROLL_X + 2, SCROLL_Y + 2, SCROLL_W - 4, 20)

# `base/child` has 28 lines: track 412, thumb (412*26)//28 = 382, travel 30.
# Pressing 20 px below the track top is inside the thumb at position 0 and
# leaves a grab offset that still reaches `top_line = total - Rows`.
THUMB_PRESS = (SCROLL_X + SCROLL_W // 2, SCROLL_Y + 2 + 20)
SCROLL_BOTTOM = SCROLL_Y + SCROLL_H - 4

EXIT_RE = re.compile(
    r"task\s+(\d+): exit (Ok|Killed|Faulted\(([^)]*)\)) after (\d+) claims"
)


def hid_axis(pixel, extent):
    """Inverse of kernel/src/usb.rs: round to nearest, ties up."""
    if extent <= 1:
        return 0
    pixel = min(max(pixel, 0), extent - 1)
    return (pixel * 0x7FFF + (extent - 1) // 2) // (extent - 1)


class Tablet:
    """Absolute pointer events through QEMU's usb-tablet."""

    def __init__(self, qmp, width, height):
        self.qmp = qmp
        self.width = width
        self.height = height

    def _axis(self, x, y):
        return [
            {"type": "abs", "data": {"axis": "x", "value": hid_axis(x, self.width)}},
            {"type": "abs", "data": {"axis": "y", "value": hid_axis(y, self.height)}},
        ]

    def move(self, x, y):
        self.qmp.command("input-send-event", {"events": self._axis(x, y)})

    def button(self, x, y, down):
        events = self._axis(x, y)
        events.append(
            {"type": "btn", "data": {"down": down, "button": "left"}}
        )
        self.qmp.command("input-send-event", {"events": events})


def capture(qmp, path):
    """Writes a PNG to `path`; returns the path actually produced."""
    try:
        qmp.command("screendump", {"filename": path, "format": "png"})
        return path
    except RuntimeError as error:
        print(f"[harness] screendump PNG refused ({error}); using PPM")
    fd, ppm = tempfile.mkstemp(suffix=".ppm", prefix="igis-editor-scroll-")
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


def differing_pixels(image_a, image_b, region):
    """Count and bounding box of differing pixels in `(x, y, w, h)`."""
    x0, y0, w, h = region
    count = 0
    min_x = x0 + w
    min_y = y0 + h
    max_x = x0
    max_y = y0
    for y in range(y0, y0 + h):
        for x in range(x0, x0 + w):
            if image_a.getpixel((x, y)) != image_b.getpixel((x, y)):
                count += 1
                min_x = min(min_x, x)
                min_y = min(min_y, y)
                max_x = max(max_x, x)
                max_y = max(max_y, y)
    if count == 0:
        return 0, None
    return count, (min_x, min_y, max_x, max_y)


def gray_pixels(image, region):
    """Pixels exactly `stdlib.ui_gray()` (0xCCCCCC) in `(x, y, w, h)`."""
    x0, y0, w, h = region
    return sum(
        1
        for y in range(y0, y0 + h)
        for x in range(x0, x0 + w)
        if image.getpixel((x, y)) == (204, 204, 204)
    )


def halted(serial):
    """True when the transcript shows the idle-summary/halt path."""
    return any(
        "scheduler: idle summary" in line or line.startswith("halt:")
        for line in list(serial.lines)
    )


def send_ctrl_q(qmp):
    """Ctrl+Q through QEMU's keyboard path (the editor accepts Q or q)."""
    qmp.command(
        "send-key",
        {
            "keys": [
                {"type": "qcode", "data": "ctrl"},
                {"type": "qcode", "data": "q"},
            ],
            "hold-time": 100,
        },
    )


def run(args):
    iso = os.path.abspath(args.iso)
    if not os.path.exists(iso):
        sys.exit(f"missing ISO: {iso}")

    output = os.path.abspath(args.output)
    if not args.no_capture:
        os.makedirs(os.path.dirname(output) or ".", exist_ok=True)

    scratch = tempfile.mkdtemp(prefix="igis-editor-scroll-")
    before_path = os.path.join(scratch, "before.png")
    after_path = os.path.join(scratch, "after.png")

    socket_path = os.path.join(
        tempfile.gettempdir(), f"igis-editor-scroll-{os.getpid()}.qmp"
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
        serial.wait_for(r"(?:shell> |igis:/[^>]*> )", timeout=args.timeout)
        graphics, _ = serial.wait_for(r"graphics: (\d+)x(\d+)", timeout=10)
        width, height = int(graphics.group(1)), int(graphics.group(2))
        print(f"[harness] screen {width}x{height}, cores {args.cores}")

        process.stdin.write(b"run editor\n")
        process.stdin.flush()
        _, cursor = serial.wait_for(
            r"repl: apps/productivity/editor: spawned as task", timeout=args.timeout
        )
        # Give the first frame time to reach the framebuffer.
        time.sleep(1.2)

        tablet = Tablet(qmp, width, height)
        # Start from a known point inside the text view.
        tablet.move(TEXT_X + 20, TEXT_Y + 20)
        time.sleep(0.2)
        capture(qmp, before_path)

        # 1. The user's shape: a long burst of no-button moves over the text.
        started = time.time()
        for i in range(args.moves):
            x = TEXT_X + 20 + (i * 7) % 540
            y = TEXT_Y + 20 + (i * 13) % 300
            tablet.move(x, y)
            if args.move_delay_ms > 0:
                time.sleep(args.move_delay_ms / 1000.0)
        move_elapsed = time.time() - started

        # 2. Grab the thumb and drag it to the bottom (held primary button).
        drag_started = time.time()
        tablet.move(*THUMB_PRESS)
        time.sleep(0.05)
        tablet.button(*THUMB_PRESS, down=True)
        time.sleep(0.1)
        span = SCROLL_BOTTOM - THUMB_PRESS[1]
        for step in range(1, args.drag_moves + 1):
            y = THUMB_PRESS[1] + span * step // args.drag_moves
            tablet.move(THUMB_PRESS[0], y)
            if args.drag_delay_ms > 0:
                time.sleep(args.drag_delay_ms / 1000.0)
        tablet.button(THUMB_PRESS[0], SCROLL_BOTTOM, down=False)
        drag_elapsed = time.time() - drag_started
        time.sleep(0.3)
        capture(qmp, after_path)
        print(
            f"[harness] {args.moves} moves in {move_elapsed:.1f}s, "
            f"{args.drag_moves}-step drag in {drag_elapsed:.1f}s"
        )

        # 3. The frame must show the scroll: the text view changed and the thumb
        # left the top of the track (gray -> white).
        from PIL import Image

        with Image.open(before_path) as before_image:
            before = before_image.convert("RGB")
        with Image.open(after_path) as after_image:
            after = after_image.convert("RGB")
        text_diff, text_box = differing_pixels(before, after, TEXT_REGION)
        gray_before = gray_pixels(before, THUMB_TOP_STRIP)
        gray_after = gray_pixels(after, THUMB_TOP_STRIP)
        print(
            f"[harness] text region diff {text_diff} px {text_box}; "
            f"thumb strip gray {gray_before} -> {gray_after}"
        )
        scrolled = text_diff > 2000 and gray_before > 0 and gray_after == 0

        # 4. Ctrl+Q must end the editor `Ok` with no budget fault.
        send_ctrl_q(qmp)
        exit_match, exit_cursor = serial.wait_for(
            r"task\s+\d+: exit (Ok|Killed|Faulted\([^)]*\)) after \d+ claims",
            timeout=args.timeout,
            start=cursor,
        )
        parsed = EXIT_RE.search(exit_match.string)
        if parsed is None:
            print(f"[harness] FAIL unparsed exit line: {exit_match.string!r}")
            return 1
        task_id = parsed.group(1)
        outcome = parsed.group(2)
        claims = int(parsed.group(4))
        serial.wait_for(r"(?:shell> |igis:/[^>]*> )", timeout=args.timeout, start=exit_cursor)
        budget_faults = [line for line in serial.lines if "BudgetExceeded" in line]
        summary = halted(serial)

        print(
            f"[harness] task {task_id}: {outcome} after {claims} claims; "
            f"budget faults {len(budget_faults)}; idle/halt {summary}"
        )
        if budget_faults:
            for line in budget_faults:
                print(f"[harness]   {line}")
        ok = (
            outcome == "Ok"
            and not budget_faults
            and not summary
            and scrolled
            and claims >= 1000
        )
        print(f"[harness] {'ok' if ok else 'FAIL'}")
        if not args.no_capture:
            capture(qmp, output)
            dimensions = image_dimensions(output)
            size = os.path.getsize(output)
            if dimensions is not None:
                print(
                    f"[harness] screenshot {output} ({size} bytes, "
                    f"{dimensions[0]}x{dimensions[1]})"
                )
        return 0 if ok else 1
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


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("iso", help="path to build/igis-shell.iso")
    parser.add_argument("--cores", default="4")
    parser.add_argument("--moves", type=int, default=3000)
    parser.add_argument("--drag-moves", type=int, default=300)
    parser.add_argument("--move-delay-ms", type=float, default=2.0)
    parser.add_argument("--drag-delay-ms", type=float, default=3.0)
    parser.add_argument("--output", default=DEFAULT_OUTPUT)
    parser.add_argument("--no-capture", action="store_true")
    parser.add_argument("--timeout", type=float, default=120.0)
    args = parser.parse_args()
    return run(args)


if __name__ == "__main__":
    sys.exit(main())
