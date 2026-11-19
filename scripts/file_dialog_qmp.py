#!/usr/bin/env python3
"""Prove the `/lib` file-open dialog on the real editor (M9.3).

This harness boots `build/igis-shell.iso` with the absolute USB tablet, runs
the interactive editor (it opens `base/child`) and drives the M9.3 dialog:

1. Ctrl+O opens the dialog. Its service chrome and client face are checked at
   known pixels, and the whole dialog frame must differ from the editor frame.
2. Backspace goes from `base` to the root, the `lib` row is clicked, Enter
   descends, the `math.ig` row is clicked and `Open` is clicked. The editor
   must load `lib/math.ig`: the text and path regions change and the dialog
   window is gone.
3. Ctrl+O opens the dialog again; Escape cancels. The editor text must be
   unchanged and the dialog gone.
4. Ctrl+Q exits the editor `Ok`, the shell prompt returns, and no budget fault
   or idle summary is printed.

The dialog is an ordinary task-owned window from `samples/apps/lib/file_dialog.ig`;
nothing here is kernel-specific. The final frames are written to
`/home/sinan/Desktop/igis-file-dialog.png` (dialog open) and
`/home/sinan/Desktop/igis-file-dialog-open.png` (the editor after `lib/math.ig`
loaded). Pillow reads the captures for pixel diffs only; images are never
displayed.

Usage:
    cargo xtask image --shell
    python3 scripts/file_dialog_qmp.py build/igis-shell.iso
    python3 scripts/file_dialog_qmp.py build/igis-shell.iso --cores 1 --no-capture
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

DIALOG_OUTPUT = "/home/sinan/Desktop/igis-file-dialog.png"
OPENED_OUTPUT = "/home/sinan/Desktop/igis-file-dialog-open.png"

# `samples/apps/base/editor` opens content 640x480 at frame origin (180, 90), so
# the content origin is (181, 106). Text/Dialog constants mirror the samples.
EDITOR_FRAME = (180, 90)
EDITOR_CONTENT = (181, 106)
EDITOR_TEXT_REGION = (EDITOR_CONTENT[0] + 4, EDITOR_CONTENT[1] + 38, 612, 416)
EDITOR_PATH_REGION = (EDITOR_CONTENT[0], EDITOR_CONTENT[1] + 22, 640, 16)
# The actual path glyphs (left-aligned at content + (8, 26), before the right
# aligned Ctrl+S hint and the separator at +41). Nine rows stop before the
# separator, whose full-width black line would otherwise dominate the bbox.
EDITOR_PATH_TEXT = (EDITOR_CONTENT[0] + 8, EDITOR_CONTENT[1] + 26, 300, 9)

DIALOG_W = 520
DIALOG_H = 380
LIST_LEFT = 8
LIST_TOP = 30
LIST_W = 472
LIST_ROWS = 17
LINE_H = 16
BODY_H = LIST_ROWS * LINE_H + 2
SCROLL_LEFT = 488
SCROLL_W = 16
BUTTON_TOP = 340
BUTTON_W = 72
BUTTON_H = 22


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

    def click(self, x, y):
        self.move(x, y)
        time.sleep(0.1)
        self.qmp.command(
            "input-send-event",
            {
                "events": self._axis(x, y)
                + [{"type": "btn", "data": {"down": True, "button": "left"}}]
            },
        )
        time.sleep(0.1)
        self.qmp.command(
            "input-send-event",
            {
                "events": self._axis(x, y)
                + [{"type": "btn", "data": {"down": False, "button": "left"}}]
            },
        )
        time.sleep(0.15)


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
        time.sleep(0.05)


def send_combo(qmp, modifier, key):
    """A modifier chord through QEMU's keyboard path."""
    qmp.command(
        "send-key",
        {
            "keys": [
                {"type": "qcode", "data": modifier},
                {"type": "qcode", "data": key},
            ],
            "hold-time": 100,
        },
    )
    time.sleep(0.15)


def capture(qmp, path):
    """Writes a PNG to `path`; returns the path actually produced."""
    try:
        qmp.command("screendump", {"filename": path, "format": "png"})
        return path
    except RuntimeError as error:
        print(f"[harness] screendump PNG refused ({error}); using PPM")
    fd, ppm = tempfile.mkstemp(suffix=".ppm", prefix="igis-file-dialog-")
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
    try:
        from PIL import Image

        with Image.open(path) as image:
            return image.size
    except Exception as error:  # pragma: no cover - reporting only
        print(f"[harness] cannot read dimensions: {error}")
        return None


def differing_pixels(image_a, image_b, region):
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


def black_pixels(image, region):
    x0, y0, w, h = region
    return sum(
        1
        for y in range(y0, y0 + h)
        for x in range(x0, x0 + w)
        if image.getpixel((x, y)) == (0, 0, 0)
    )


def ink_bbox(image, region):
    """Bounding box of exact black pixels in `(x, y, w, h)`, or None."""
    x0, y0, w, h = region
    min_x = x0 + w
    min_y = y0 + h
    max_x = x0
    max_y = y0
    found = False
    for y in range(y0, y0 + h):
        for x in range(x0, x0 + w):
            if image.getpixel((x, y)) == (0, 0, 0):
                found = True
                min_x = min(min_x, x)
                min_y = min(min_y, y)
                max_x = max(max_x, x)
                max_y = max(max_y, y)
    return (min_x, min_y, max_x, max_y) if found else None


def pixel(image, x, y):
    return image.getpixel((x, y))


def halted(serial):
    return any(
        "scheduler: idle summary" in line or line.startswith("halt:")
        for line in list(serial.lines)
    )


def lib_math_row(iso):
    """The `lib` listing row of `math.ig` in the staged image.

    `mke2fs -d` keeps the source entries in name order (debugfs confirms
    `file_dialog.ig, inject.ig, math.ig, ...`), so the sorted staging directory
    is the image order. Fall back to the observed row if the tree is absent.
    """
    build = os.path.dirname(os.path.abspath(iso))
    for staging in ("apps-shell-root", "apps-root"):
        lib = os.path.join(build, staging, "lib")
        if os.path.isdir(lib):
            names = sorted(os.listdir(lib))
            if "math.ig" in names:
                return names.index("math.ig"), names
    return 2, []


EXIT_RE = re.compile(
    r"task\s+(\d+): exit (Ok|Killed|Faulted\(([^)]*)\)) after (\d+) claims"
)


def run(args):
    iso = os.path.abspath(args.iso)
    if not os.path.exists(iso):
        sys.exit(f"missing ISO: {iso}")

    dialog_output = os.path.abspath(args.output)
    opened_output = os.path.abspath(args.opened_output)
    if not args.no_capture:
        os.makedirs(os.path.dirname(dialog_output) or ".", exist_ok=True)
        os.makedirs(os.path.dirname(opened_output) or ".", exist_ok=True)

    scratch = tempfile.mkdtemp(prefix="igis-file-dialog-")
    before_path = os.path.join(scratch, "before.png")
    dialog_path = os.path.join(scratch, "dialog.png")
    dialog2_path = os.path.join(scratch, "dialog2.png")
    opened_path = os.path.join(scratch, "opened.png")
    cancelled_path = os.path.join(scratch, "cancelled.png")

    socket_path = os.path.join(
        tempfile.gettempdir(), f"igis-file-dialog-{os.getpid()}.qmp"
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
        serial.wait_for(r"shell> ", timeout=args.timeout)
        graphics, _ = serial.wait_for(r"graphics: (\d+)x(\d+)", timeout=10)
        width, height = int(graphics.group(1)), int(graphics.group(2))
        print(f"[harness] screen {width}x{height}, cores {args.cores}")

        # The dialog frame is centred with the service chrome included.
        dialog_x = (width - (DIALOG_W + 2)) // 2
        dialog_y = (height - (DIALOG_H + 17)) // 2
        content_x = dialog_x + 1
        content_y = dialog_y + 16
        list_x = content_x + LIST_LEFT
        list_y = content_y + LIST_TOP
        scroll_x = content_x + SCROLL_LEFT
        button_y = content_y + BUTTON_TOP
        cancel_x = content_x + DIALOG_W - 8 - BUTTON_W
        open_x = cancel_x - 12 - BUTTON_W
        dialog_region = (dialog_x, dialog_y, DIALOG_W + 2, DIALOG_H + 17)

        def row_center(index):
            return list_x + 40, list_y + 1 + index * LINE_H + 8

        math_row, lib_names = lib_math_row(iso)
        print(
            f"[harness] dialog frame ({dialog_x}, {dialog_y}), content "
            f"({content_x}, {content_y}); lib row {math_row} "
            f"({', '.join(lib_names) if lib_names else 'staging absent'})"
        )

        process.stdin.write(b"run editor\n")
        process.stdin.flush()
        _, cursor = serial.wait_for(
            r"repl: base/editor: spawned as task", timeout=args.timeout
        )
        time.sleep(1.0)
        tablet = Tablet(qmp, width, height)
        tablet.move(10, 10)
        time.sleep(0.2)
        capture(qmp, before_path)

        # 1. Ctrl+O opens the dialog on top of the editor.
        send_combo(qmp, "ctrl", "o")
        time.sleep(0.8)
        tablet.move(10, 10)
        time.sleep(0.2)
        capture(qmp, dialog_path)

        from PIL import Image

        with Image.open(before_path) as handle:
            before = handle.convert("RGB")
        with Image.open(dialog_path) as handle:
            dialog = handle.convert("RGB")

        changed, bbox = differing_pixels(before, dialog, dialog_region)
        chrome_ok = (
            pixel(dialog, dialog_x, dialog_y + 100) == (0, 0, 0)
            and pixel(dialog, dialog_x + 40, dialog_y + 2) == (0, 0, 0)
            and pixel(dialog, dialog_x + 40, dialog_y + 3) == (255, 255, 255)
            and pixel(dialog, list_x, list_y) == (0, 0, 0)
            and pixel(dialog, open_x, button_y) == (0, 0, 0)
            and pixel(dialog, cancel_x, button_y) == (0, 0, 0)
            and pixel(dialog, content_x + 300, content_y + 10) == (255, 255, 255)
        )
        print(
            f"[harness] dialog frame diff {changed} px {bbox}; chrome/face "
            f"{'ok' if chrome_ok else 'FAILED'}"
        )
        if changed < 10000 or not chrome_ok:
            print("[harness] FAIL: the dialog window did not appear correctly")
            return 1

        # 2. base -> root -> lib -> math.ig -> Open.
        send_key(qmp, "backspace")
        time.sleep(0.3)
        tablet.click(*row_center(3))  # root row 3 is `lib`
        time.sleep(0.2)
        send_key(qmp, "ret")
        time.sleep(0.4)
        tablet.click(*row_center(math_row))
        time.sleep(0.2)
        tablet.click(open_x + BUTTON_W // 2, button_y + BUTTON_H // 2)
        time.sleep(0.9)
        tablet.move(10, 10)
        time.sleep(0.2)
        capture(qmp, opened_path)

        with Image.open(opened_path) as handle:
            opened = handle.convert("RGB")

        dialog_gone, gone_bbox = differing_pixels(dialog, opened, dialog_region)
        text_changed, text_bbox = differing_pixels(opened, before, EDITOR_TEXT_REGION)
        path_changed, path_bbox = differing_pixels(opened, before, EDITOR_PATH_REGION)
        path_ink_before = black_pixels(before, EDITOR_PATH_REGION)
        path_ink_after = black_pixels(opened, EDITOR_PATH_REGION)
        path_text_before = ink_bbox(before, EDITOR_PATH_TEXT)
        path_text_after = ink_bbox(opened, EDITOR_PATH_TEXT)
        print(
            f"[harness] Open: dialog closure diff {dialog_gone} px {gone_bbox}; "
            f"text {text_changed} px {text_bbox}; path {path_changed} px "
            f"{path_bbox} (ink {path_ink_before} -> {path_ink_after}), "
            f"text bbox {path_text_before} -> {path_text_after}"
        )
        if dialog_gone < 10000:
            print("[harness] FAIL: the dialog window did not close on Open")
            return 1
        if text_changed < 1000:
            print("[harness] FAIL: the editor document did not change")
            return 1
        if path_changed < 30 or path_ink_before == path_ink_after:
            print("[harness] FAIL: the editor path line did not change")
            return 1
        # `lib/math.ig` is eleven 8-pixel cells; `base/child` is ten. The ink
        # bbox grows from 79 to 86 pixels, so an 82-pixel floor proves the
        # longer chosen path was published (the exact row order comes from the
        # staged image and is printed above).
        if (
            path_text_after is None
            or path_text_after[2] - path_text_after[0] < 82
        ):
            print("[harness] FAIL: the editor path is not the chosen file")
            return 1

        # 3. Ctrl+O again, Escape cancels; the document is untouched.
        send_combo(qmp, "ctrl", "o")
        time.sleep(0.8)
        tablet.move(10, 10)
        time.sleep(0.2)
        capture(qmp, dialog2_path)
        with Image.open(dialog2_path) as handle:
            dialog2 = handle.convert("RGB")
        send_key(qmp, "esc")
        time.sleep(0.7)
        tablet.move(10, 10)
        time.sleep(0.2)
        capture(qmp, cancelled_path)
        with Image.open(cancelled_path) as handle:
            cancelled = handle.convert("RGB")

        cancel_gone, cancel_bbox = differing_pixels(
            dialog2, cancelled, dialog_region
        )
        text_kept, kept_bbox = differing_pixels(
            opened, cancelled, EDITOR_TEXT_REGION
        )
        print(
            f"[harness] Cancel: dialog closure diff {cancel_gone} px "
            f"{cancel_bbox}; text kept diff {text_kept} px {kept_bbox}"
        )
        if cancel_gone < 10000:
            print("[harness] FAIL: Escape did not close the dialog")
            return 1
        if text_kept > 30:
            print("[harness] FAIL: Escape changed the editor document")
            return 1

        # 4. Ctrl+Q exits the editor; the shell prompt returns.
        send_combo(qmp, "ctrl", "q")
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
        serial.wait_for(r"shell> ", timeout=args.timeout, start=exit_cursor)
        budget_faults = [line for line in serial.lines if "BudgetExceeded" in line]
        summary = halted(serial)
        print(
            f"[harness] editor task {task_id}: {outcome} after {claims} claims; "
            f"budget faults {len(budget_faults)}; idle/halt {summary}"
        )
        if outcome != "Ok" or budget_faults or summary:
            print("[harness] FAIL: the editor did not exit cleanly")
            return 1

        if not args.no_capture:
            import shutil

            shutil.copyfile(dialog_path, dialog_output)
            shutil.copyfile(opened_path, opened_output)
            for path in (dialog_output, opened_output):
                size = os.path.getsize(path)
                dimensions = image_dimensions(path)
                if dimensions is None:
                    print(
                        f"[harness] screenshot {path} ({size} bytes, "
                        "dimensions unknown)"
                    )
                else:
                    print(
                        f"[harness] screenshot {path} ({size} bytes, "
                        f"{dimensions[0]}x{dimensions[1]})"
                    )

        print(
            "[harness] PASS: dialog opened with chrome, lib/math.ig loaded, "
            "Escape cancelled without changing the document, editor exited Ok"
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
        import shutil

        shutil.rmtree(scratch, ignore_errors=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("iso", help="path to build/igis-shell.iso")
    parser.add_argument("--output", default=DIALOG_OUTPUT)
    parser.add_argument("--opened-output", default=OPENED_OUTPUT)
    parser.add_argument("--no-capture", action="store_true")
    parser.add_argument("--cores", default="4")
    parser.add_argument("--timeout", type=float, default=180.0)
    return run(parser.parse_args())


if __name__ == "__main__":
    sys.exit(main())
