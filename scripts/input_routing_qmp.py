#!/usr/bin/env python3
"""Prove window-addressed input on the real boot desktop (M8.6, spec 0011 W29).

This is the M8.6 regression harness (ADR 0047). Before M8.6 the one
machine-wide event queue was consumed by whichever task polled first. The boot
desktop (`desktop_shell`, realtime) wakes with every event and took the
Browser's clicks and keys, so a desktop-launched application did not react.
The target rule now delivers a queued event to the task that owns its window
target: a pointer-down to the topmost hit window (or the menu-bar region
claimant), moves/keys to the focused window, and an addressed record to its
window's owner.

The harness boots `build/igis.iso` (the boot desktop stays alive throughout)
and proves, with the desktop and the Browser both live:

1. Launch File Browser from Apps. Move the pointer over the third list row
   (a directory in every image), capture, click the row, capture: the list and
   preview must change, proving the click reached the Browser instead of the
   desktop. Then send Enter (a key event addressed to the focused Browser
   window) and capture: the path line and entry count/list must change, proving
   the Browser navigated into the directory.
2. Open System -> About, click About's close box, and require
   `desktop_shell: about closed`. The Browser stays live. Open About again and
   press Escape, requiring a second `about closed`; the desktop must still be
   running afterwards (no `task 3: exit Ok`, no idle summary, no halt).
3. Launch Calculator from Apps while the Browser is still live and click its
   `7` button without closing the desktop. The display region must change.

It never displays the image: Pillow reads the captured framebuffer for the
pixel diffs only. The final capture is written to
`build/screenshots/igis-input-routing.png`.

Usage:
    cargo xtask image
    python3 scripts/input_routing_qmp.py build/igis.iso
    python3 scripts/input_routing_qmp.py build/igis.iso --cores 1 --no-capture
"""

import argparse
import os
import shutil
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

DEFAULT_OUTPUT = os.path.join(SHOT_DIR, "igis-input-routing.png")

# `desktop_shell` menu geometry: `File` starts at x = 24, `Apps` at x = 56 and
# its pull-down frame is (52, 22, 176, 98). Row 3 (`File Browser`) is
# y = 23 + 3 * 16 = 71..86; row 0 (`Calculator`) is y 23..38.
APPS_LABEL = (64, 10)
BROWSER_ROW = (140, 78)
CALCULATOR_ROW = (140, 30)
# The `System` label starts at x = 208; its pull-down frame is
# (204, 22, 224, 66) and row 0 (`About igis-v2`) is y 23..38. Row 0 is clicked
# at y = 24, above the kernel Terminal frame (y >= 26).
SYSTEM_LABEL = (224, 10)
ABOUT_ROW = (260, 24)
APPLE = (10, 10)
# The shell's own `Desktop` window opens at (600, 72); its close box centre is
# frame + (9, 8), and the title bar (y 72..88) stays above the Browser frame
# (y >= 90) so the click reaches the Desktop window while the Browser is live.
DESKTOP_CLOSE = (609, 80)

# `samples/apps/base/browser` opens content 680x480 at (160, 90); content
# origin (161, 106). `ListLeft` 8, `ListTop` 30, `LineH` 16, `BodyH` 420.
BROWSER_CONTENT = (161, 106)
BROWSER_LIST_X = BROWSER_CONTENT[0] + 8
BROWSER_LIST_Y = BROWSER_CONTENT[1] + 30
BROWSER_LIST_W = 342
# Third row centre: list_y + 1 + 2 * 16 + 8.
BROWSER_ROW_THREE = (BROWSER_LIST_X + 40, BROWSER_LIST_Y + 41)
# The visible `Up` button is content + (ContentW - 8 - 104 - 8 - 44, 2) =
# (677, 108), 44x18; its centre is (699, 117). `home writable` is 13 code
# points (104 px) in the 8x8 UI font.
BROWSER_UP_BUTTON = (699, 117)
# Root row 1 (`base`, a long directory; the image's directory order is
# lost+found, base, home, lib, system). The scrollbar is at content + (352, 30),
# track 16x420, with the thumb near the top.
BROWSER_ROW_BASE = (BROWSER_LIST_X + 40, BROWSER_LIST_Y + 25)
BROWSER_SCROLL_X = BROWSER_CONTENT[0] + 352 + 8
BROWSER_SCROLL_TOP = BROWSER_CONTENT[1] + 30 + 2
BROWSER_SCROLL_BOTTOM = BROWSER_CONTENT[1] + 30 + 420 - 4
SCROLL_REGION = (BROWSER_CONTENT[0] + 352, BROWSER_CONTENT[1] + 30, 16, 420)
# The path line is the first 22 pixels of the content; the list strip is the
# first five rows plus the selection/preview band.
PATH_REGION = (BROWSER_CONTENT[0], BROWSER_CONTENT[1], 360, 21)
LIST_REGION = (BROWSER_LIST_X, BROWSER_LIST_Y, BROWSER_LIST_W, 90)

# `samples/apps/base/calculator`: content 300x360 at frame origin (180, 100),
# so the content origin is (181, 116). The display is content + (8, 10),
# 284x40; button `7` is content + (42, 138).
CALCULATOR_CONTENT = (181, 116)
CALCULATOR_DISPLAY = (
    CALCULATOR_CONTENT[0] + 8,
    CALCULATOR_CONTENT[1] + 10,
    284,
    40,
)
CALCULATOR_SEVEN = (
    CALCULATOR_CONTENT[0] + 42,
    CALCULATOR_CONTENT[1] + 138,
)

# The About panel is centred below the 22-row bar; the close box is
# frame + (4..15, 3..14), so its centre is frame + (9, 8).
MENU_HEIGHT = 22
ABOUT_W = 420
ABOUT_H = 200


def hid_axis(pixel, extent):
    """Inverse of kernel/src/usb.rs: round to nearest, ties up."""
    if extent <= 1:
        return 0
    pixel = min(max(pixel, 0), extent - 1)
    return (pixel * 0x7FFF + (extent - 1) // 2) // (extent - 1)


def send(qmp, x, y, width, height, left):
    qmp.send_absolute(hid_axis(x, width), hid_axis(y, height), left=left)


def move(qmp, x, y, width, height):
    send(qmp, x, y, width, height, False)
    time.sleep(0.1)


def drag(qmp, x, y0, y1, width, height):
    send(qmp, x, y0, width, height, False)
    time.sleep(0.1)
    send(qmp, x, y0, width, height, True)
    time.sleep(0.1)
    for step in range(1, 6):
        y = y0 + (y1 - y0) * step // 5
        send(qmp, x, y, width, height, True)
        time.sleep(0.1)
    send(qmp, x, y1, width, height, False)
    time.sleep(0.1)


def click(qmp, x, y, width, height):
    send(qmp, x, y, width, height, False)
    # Dwell so the polled tablet reports each button transition; a press and
    # release sent back to back can otherwise coalesce into "released".
    time.sleep(0.1)
    send(qmp, x, y, width, height, True)
    time.sleep(0.1)
    send(qmp, x, y, width, height, False)
    time.sleep(0.1)


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


def about_close_box(width, height):
    aw = min(ABOUT_W, width - 16)
    ah = min(ABOUT_H, height - MENU_HEIGHT - 16)
    x = (width - aw) // 2
    y = MENU_HEIGHT + ((height - MENU_HEIGHT - ah) // 2)
    return x + 9, y + 8


def capture(qmp, path):
    """Writes a PNG to `path`; returns the path actually produced."""
    try:
        qmp.command("screendump", {"filename": path, "format": "png"})
        return path
    except RuntimeError as error:
        print(f"[harness] screendump PNG refused ({error}); using PPM")
    fd, ppm = tempfile.mkstemp(suffix=".ppm", prefix="igis-routing-")
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


def black_pixels(image, region):
    x0, y0, w, h = region
    return sum(
        1
        for y in range(y0, y0 + h)
        for x in range(x0, x0 + w)
        if image.getpixel((x, y)) == (0, 0, 0)
    )


def halted(serial):
    """True when the transcript shows the idle-summary/halt path."""
    return any(
        "scheduler: idle summary" in line or line.startswith("halt:")
        for line in list(serial.lines)
    )


def run(args):
    iso = os.path.abspath(args.iso)
    if not os.path.exists(iso):
        sys.exit(f"missing ISO: {iso}")

    output = os.path.abspath(args.output)
    if not args.no_capture:
        os.makedirs(os.path.dirname(output) or ".", exist_ok=True)

    scratch = tempfile.mkdtemp(prefix="igis-routing-")
    before_click_path = os.path.join(scratch, "before-click.png")
    after_click_path = os.path.join(scratch, "after-click.png")
    after_nav_path = os.path.join(scratch, "after-nav.png")
    after_up_path = os.path.join(scratch, "after-up.png")
    before_scroll_path = os.path.join(scratch, "before-scroll.png")
    after_scroll_path = os.path.join(scratch, "after-scroll.png")
    before_button_path = os.path.join(scratch, "before-button.png")
    after_button_path = os.path.join(scratch, "after-button.png")

    socket_path = os.path.join(
        tempfile.gettempdir(), f"igis-input-routing-{os.getpid()}.qmp"
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
        print(f"[harness] screen {width}x{height}, cores {args.cores}")
        cursor = 0

        # 1. Launch File Browser from Apps; the desktop stays alive.
        click(qmp, *APPS_LABEL, width, height)
        _, cursor = serial.wait_for(
            r"desktop_shell: apps menu open", timeout=args.timeout, start=cursor
        )
        click(qmp, *BROWSER_ROW, width, height)
        _, cursor = serial.wait_for(
            r"repl: base/browser: spawned as task", timeout=args.timeout, start=cursor
        )
        time.sleep(1.2)
        print("[harness] File Browser launched (desktop alive)")

        # Move over the third list row, capture, then click it. The pointer
        # does not move between the two captures, so a clean-cursor pixel diff
        # isolates the selection/preview change caused by the click.
        move(qmp, *BROWSER_ROW_THREE, width, height)
        time.sleep(0.3)
        capture(qmp, before_click_path)
        click(qmp, *BROWSER_ROW_THREE, width, height)
        time.sleep(0.5)
        capture(qmp, after_click_path)

        from PIL import Image

        with Image.open(before_click_path) as handle:
            before_click = handle.convert("RGB")
        with Image.open(after_click_path) as handle:
            after_click = handle.convert("RGB")
        click_changed, click_bbox = differing_pixels(
            before_click, after_click, LIST_REGION
        )
        print(
            f"[harness] list row click changed {click_changed} pixels "
            f"at {click_bbox}"
        )
        if click_changed < 50:
            print("[harness] FAIL: the list click did not reach the Browser")
            return 1

        # Enter is a key event addressed to the focused Browser window; the
        # Browser descends into the selected directory.
        send_key(qmp, "ret")
        time.sleep(0.8)
        capture(qmp, after_nav_path)
        with Image.open(after_nav_path) as handle:
            after_nav = handle.convert("RGB")
        path_changed, path_bbox = differing_pixels(after_click, after_nav, PATH_REGION)
        list_changed, list_bbox = differing_pixels(after_click, after_nav, LIST_REGION)
        path_ink_before = black_pixels(after_click, PATH_REGION)
        path_ink_after = black_pixels(after_nav, PATH_REGION)
        print(
            f"[harness] Enter navigation: path {path_changed} pixels at "
            f"{path_bbox} (ink {path_ink_before} -> {path_ink_after}), "
            f"list {list_changed} pixels at {list_bbox}"
        )
        if path_changed < 50 or path_ink_before == path_ink_after:
            print("[harness] FAIL: the Enter key did not navigate the Browser")
            return 1
        if list_changed < 100:
            print("[harness] FAIL: the Browser listing did not change")
            return 1

        # The visible `Up` button returns to the parent through a real pointer
        # click; the path line must match the pre-navigation root again.
        click(qmp, *BROWSER_UP_BUTTON, width, height)
        time.sleep(0.6)
        capture(qmp, after_up_path)
        with Image.open(after_up_path) as handle:
            after_up = handle.convert("RGB")
        up_changed, up_bbox = differing_pixels(before_click, after_up, PATH_REGION)
        print(
            f"[harness] Up button returned to the parent path "
            f"({up_changed} pixels differ at {up_bbox})"
        )
        if up_changed > 60:
            print("[harness] FAIL: the visible Up button did not go back")
            return 1

        # Scrollbar thumb drag (M8.9): enter `base` (a long directory) and drag
        # the thumb from the top to the bottom through the real pointer path.
        click(qmp, *BROWSER_ROW_BASE, width, height)
        time.sleep(0.3)
        send_key(qmp, "ret")
        time.sleep(0.8)
        capture(qmp, before_scroll_path)
        drag(
            qmp,
            BROWSER_SCROLL_X,
            BROWSER_SCROLL_TOP + 90,
            BROWSER_SCROLL_BOTTOM,
            width,
            height,
        )
        time.sleep(0.8)
        capture(qmp, after_scroll_path)
        with Image.open(before_scroll_path) as handle:
            before_scroll = handle.convert("RGB")
        with Image.open(after_scroll_path) as handle:
            after_scroll = handle.convert("RGB")
        scroll_changed, scroll_bbox = differing_pixels(
            before_scroll, after_scroll, SCROLL_REGION
        )
        scrolled_list, scrolled_bbox = differing_pixels(
            before_scroll, after_scroll, LIST_REGION
        )
        print(
            f"[harness] scrollbar drag changed {scroll_changed} thumb pixels at "
            f"{scroll_bbox}, list {scrolled_list} pixels at {scrolled_bbox}"
        )
        if scroll_changed < 20 or scrolled_list < 100:
            print("[harness] FAIL: the scrollbar thumb drag did not scroll the list")
            return 1

        # 2. About opens and closes from its close box while the Browser is
        # live. The close box is a service-consumed press, so the close request
        # is an owner-only flag for the desktop, never a Browser event.
        click(qmp, *SYSTEM_LABEL, width, height)
        time.sleep(0.3)
        click(qmp, *ABOUT_ROW, width, height)
        _, cursor = serial.wait_for(
            r"desktop_shell: about open", timeout=args.timeout, start=cursor
        )
        print("[harness] About opened with the Browser live")

        close_x, close_y = about_close_box(width, height)
        print(f"[harness] About close box at ({close_x}, {close_y})")
        click(qmp, close_x, close_y, width, height)
        _, cursor = serial.wait_for(
            r"desktop_shell: about closed", timeout=args.timeout, start=cursor
        )
        print("[harness] close box closed About, not the desktop")

        # Reopen About and close it with Escape: the desktop must answer a key
        # while the Browser is also blocked in next_event.
        click(qmp, *SYSTEM_LABEL, width, height)
        time.sleep(0.3)
        click(qmp, *ABOUT_ROW, width, height)
        _, cursor = serial.wait_for(
            r"desktop_shell: about open", timeout=args.timeout, start=cursor
        )
        send_key(qmp, "esc")
        _, cursor = serial.wait_for(
            r"desktop_shell: about closed", timeout=args.timeout, start=cursor
        )
        print("[harness] Escape closed About again")

        # 3. Launch Calculator while the Browser is still live and click `7`
        # without closing the desktop.
        click(qmp, *APPS_LABEL, width, height)
        _, cursor = serial.wait_for(
            r"desktop_shell: apps menu open", timeout=args.timeout, start=cursor
        )
        click(qmp, *CALCULATOR_ROW, width, height)
        _, cursor = serial.wait_for(
            r"repl: base/calculator: spawned as task", timeout=args.timeout, start=cursor
        )
        time.sleep(1.2)
        print("[harness] Calculator launched (desktop and Browser alive)")

        capture(qmp, before_button_path)
        click(qmp, *CALCULATOR_SEVEN, width, height)
        time.sleep(0.8)
        capture(qmp, after_button_path)
        with Image.open(before_button_path) as handle:
            before_button = handle.convert("RGB")
        with Image.open(after_button_path) as handle:
            after_button = handle.convert("RGB")
        display_changed, display_bbox = differing_pixels(
            before_button, after_button, CALCULATOR_DISPLAY
        )
        ink_before = black_pixels(before_button, CALCULATOR_DISPLAY)
        ink_after = black_pixels(after_button, CALCULATOR_DISPLAY)
        print(
            f"[harness] calculator display changed {display_changed} pixels at "
            f"{display_bbox}; ink {ink_before} -> {ink_after}"
        )
        if display_changed < 5:
            print("[harness] FAIL: the Calculator click did not change the display")
            return 1

        # 4. The shell's own `Desktop` window is a help panel, not the shell.
        # Closing it must not exit the desktop (the user-report bug: the boot
        # entrypoint exited and the idle system halted); the Apple square
        # reopens it. The Browser and Calculator stay live underneath.
        click(qmp, *DESKTOP_CLOSE, width, height)
        _, cursor = serial.wait_for(
            r"desktop_shell: desktop window closed",
            timeout=args.timeout,
            start=cursor,
        )
        print("[harness] Desktop window closed; the shell keeps running")
        click(qmp, *APPLE, width, height)
        _, cursor = serial.wait_for(
            r"desktop_shell: desktop window open",
            timeout=args.timeout,
            start=cursor,
        )
        print("[harness] Apple reopened the Desktop window")

        transcript = "\n".join(serial.lines)
        if "task 3: exit Ok" in transcript:
            print("[harness] FAIL: the desktop task exited")
            return 1
        if halted(serial):
            print("[harness] FAIL: the system halted during the proof")
            return 1
        if "desktop_shell: about closed" not in transcript:
            print("[harness] FAIL: no About close in the transcript")
            return 1
        if "repl: base/calculator: spawned as task" not in transcript:
            print("[harness] FAIL: no Calculator spawn in the transcript")
            return 1
        if "desktop_shell: desktop window closed" not in transcript:
            print("[harness] FAIL: no Desktop window close in the transcript")
            return 1
        if "desktop_shell: desktop window open" not in transcript:
            print("[harness] FAIL: the Apple did not reopen the Desktop window")
            return 1

        if not args.no_capture:
            after = after_button_path
            shutil.copyfile(after, output)
            size = os.path.getsize(output)
            dimensions = image_dimensions(output)
            if dimensions is None:
                print(f"[harness] screenshot {output} ({size} bytes, dimensions unknown)")
            else:
                out_width, out_height = dimensions
                print(
                    f"[harness] screenshot {output} "
                    f"({size} bytes, {out_width}x{out_height})"
                )
        print(
            "[harness] PASS: Browser click/keys, About close, Calculator click "
            "and the Desktop window close/reopen all worked with the desktop alive"
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
        shutil.rmtree(scratch, ignore_errors=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("iso", help="path to build/igis.iso")
    parser.add_argument("--output", default=DEFAULT_OUTPUT)
    parser.add_argument("--no-capture", action="store_true")
    parser.add_argument("--cores", default="4")
    parser.add_argument("--timeout", type=float, default=180.0)
    return run(parser.parse_args())


if __name__ == "__main__":
    sys.exit(main())
