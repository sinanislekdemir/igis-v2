#!/usr/bin/env python3
"""Drive the absolute-pointer proof (spec 0010, M2.2.6) over QMP and serial.

This is a verification harness, not part of the kernel. It boots the ISO with
QEMU's `usb-tablet`, waits for the shell, runs `tablet`, injects absolute HID
axis values through `input-send-event`, and compares the guest's reported
screen pixels with the driver's rounding rule.

Usage:
    python3 scripts/tablet_qmp.py build/igis.iso
    python3 scripts/tablet_qmp.py build/igis.iso --no-tablet

The exit status is 0 when every injected position matches within one pixel.
The full serial transcript is printed so a boot-line check can paste it.
"""

import argparse
import json
import os
import re
import socket
import subprocess
import sys
import tempfile
import threading
import time

HID_MAX = 0x7FFF
POINTER_RE = re.compile(r"tablet: pointer (-?\d+) (-?\d+) (\d+)")


def scale(value: int, extent: int) -> int:
    """Same rule as kernel/src/usb.rs: round to nearest, ties up."""
    if extent <= 1:
        return 0
    value = min(max(value, 0), HID_MAX)
    return (value * (extent - 1) * 2 + HID_MAX) // (2 * HID_MAX)


class Serial:
    """Reads QEMU's stdio serial stream and lets tests wait for lines."""

    def __init__(self, stream):
        self.stream = stream
        self.lines = []
        self.condition = threading.Condition()
        self.thread = threading.Thread(target=self._reader, daemon=True)
        self.thread.start()

    def _reader(self):
        for raw in self.stream:
            line = raw.decode("utf-8", "replace").rstrip("\r\n")
            with self.condition:
                self.lines.append(line)
                self.condition.notify_all()
            print(line, flush=True)

    def wait_for(self, pattern, timeout=30.0, start=0):
        """Waits for a regex in a line at or after index `start`."""
        compiled = re.compile(pattern)
        deadline = time.time() + timeout
        with self.condition:
            index = start
            while True:
                while index < len(self.lines):
                    match = compiled.search(self.lines[index])
                    index += 1
                    if match:
                        return match, index
                remaining = deadline - time.time()
                if remaining <= 0:
                    raise TimeoutError(f"serial: no match for {pattern!r}")
                self.condition.wait(remaining)

    def wait_for_pointer(self, timeout=5.0, start=0):
        match, index = self.wait_for(POINTER_RE, timeout=timeout, start=start)
        return (int(match.group(1)), int(match.group(2)), int(match.group(3))), index


class Qmp:
    """A minimal QMP client over a Unix socket."""

    def __init__(self, path, timeout=10.0):
        deadline = time.time() + timeout
        while True:
            try:
                self.socket = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                self.socket.connect(path)
                break
            except OSError:
                self.socket.close()
                if time.time() >= deadline:
                    raise
                time.sleep(0.05)
        self.file = self.socket.makefile("rwb")
        self._read()  # greeting
        self.command("qmp_capabilities")

    def _read(self):
        while True:
            line = self.file.readline()
            if not line:
                raise RuntimeError("QMP closed")
            message = json.loads(line)
            if "event" in message:
                continue
            return message

    def command(self, name, arguments=None):
        request = {"execute": name}
        if arguments is not None:
            request["arguments"] = arguments
        self.file.write(json.dumps(request).encode() + b"\n")
        self.file.flush()
        response = self._read()
        if "error" in response:
            raise RuntimeError(f"QMP {name}: {response['error']}")
        return response.get("return")

    def send_absolute(self, x, y, left=False):
        events = [
            {"type": "abs", "data": {"axis": "x", "value": x}},
            {"type": "abs", "data": {"axis": "y", "value": y}},
        ]
        if left:
            events.append(
                {"type": "btn", "data": {"down": True, "button": "left"}}
            )
        else:
            events.append(
                {"type": "btn", "data": {"down": False, "button": "left"}}
            )
        self.command("input-send-event", {"events": events})

    def close(self):
        try:
            self.file.close()
            self.socket.close()
        except OSError:
            pass


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("iso", help="path to build/igis.iso")
    parser.add_argument("--cores", default="4")
    parser.add_argument("--no-tablet", action="store_true")
    parser.add_argument("--timeout", type=float, default=90.0)
    args = parser.parse_args()

    iso = os.path.abspath(args.iso)
    if not os.path.exists(iso):
        sys.exit(f"missing ISO: {iso}")

    socket_path = os.path.join(
        tempfile.gettempdir(), f"igis-tablet-{os.getpid()}.qmp"
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
    ]
    if not args.no_tablet:
        command += ["-device", "usb-tablet"]

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
        # The shell echoes `shell> ` once its child is done.
        serial.wait_for(r"shell> ", timeout=args.timeout)

        if args.no_tablet:
            # No absolute device: prove the PS/2 fallback drives the pointer.
            process.stdin.write(b"run mouse\n")
            process.stdin.flush()
            serial.wait_for(r"mouse: ok", timeout=60)
            process.stdin.write(b"exit\n")
            process.stdin.flush()
            serial.wait_for(r"bye", timeout=20)
            print("\n[harness] no tablet: PS/2 fallback proved (mouse: ok)")
            return 0

        process.stdin.write(b"run tablet\n")
        process.stdin.flush()

        source_match, _ = serial.wait_for(r"tablet: source", timeout=30)
        same_line = re.search(r"tablet: source\s+(\d+)", source_match.string)
        if same_line:
            source = int(same_line.group(1))
        else:
            # A service log can split the two writes across lines; the number
            # then arrives as a standalone line.
            digits, _ = serial.wait_for(r"^\s*(\d+)\s*$", timeout=5)
            source = int(digits.group(1))
        print(f"\n[harness] pointer_source reported by guest: {source}")

        # The tablet app prints its initial position; use it as the baseline.
        _, index = serial.wait_for_pointer(timeout=10)
        print("[harness] baseline pointer report received")

        # (HID x, HID y, expect left button) at known positions. Each pair is
        # injected after the previous report, so QEMU cannot compress reports.
        cases = [
            (16384, 16384, False),
            (32767, 32767, False),
            (0, 0, False),
            (0, 32767, False),
            (32767, 0, False),
            (8192, 16384, True),
            (16384, 16384, False),
        ]

        # The screen size is in the boot log; the expected mapping needs it.
        graphics, _ = serial.wait_for(r"graphics: (\d+)x(\d+)", timeout=5)
        width, height = int(graphics.group(1)), int(graphics.group(2))
        print(f"[harness] screen {width}x{height}")

        failures = 0
        for hid_x, hid_y, left in cases:
            expected = (scale(hid_x, width), scale(hid_y, height), 1 if left else 0)
            qmp.send_absolute(hid_x, hid_y, left=left)
            try:
                reported, index = serial.wait_for_pointer(timeout=5, start=index)
            except TimeoutError:
                print(
                    f"[harness] FAIL no report for hid ({hid_x},{hid_y}) "
                    f"expected {expected}"
                )
                failures += 1
                continue
            ok = (
                abs(reported[0] - expected[0]) <= 1
                and abs(reported[1] - expected[1]) <= 1
                and reported[2] == expected[2]
            )
            status = "ok" if ok else "FAIL"
            print(
                f"[harness] hid ({hid_x:5d},{hid_y:5d}) buttons={expected[2]} -> "
                f"expected {expected[0]:4d},{expected[1]:4d},{expected[2]} "
                f"reported {reported[0]:4d},{reported[1]:4d},{reported[2]} [{status}]"
            )
            if not ok:
                failures += 1

        serial.wait_for(r"tablet: done", timeout=20)
        process.stdin.write(b"exit\n")
        process.stdin.flush()
        serial.wait_for(r"bye", timeout=20)
        print(
            f"\n[harness] {len(cases) - failures}/{len(cases)} absolute cases "
            f"passed, pointer_source={source}"
        )
        return 0 if failures == 0 else 1
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
