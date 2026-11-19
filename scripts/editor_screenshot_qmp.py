#!/usr/bin/env python3
"""Capture the M2.5.3 text editor frame from a headless QEMU boot.

This is a verification harness, not part of the kernel. It boots the ISO with
no display and no USB tablet, runs `editor shot`, waits for its
`editor: ready` line (the editor window with an opened source file
and its menu, status line and scrollbar), and writes the guest framebuffer to a PNG
through QMP `screendump`. If the QEMU build rejects the PNG format, it dumps a
PPM and converts it with PIL.

Usage:
    python3 scripts/editor_screenshot_qmp.py build/igis-shell.iso
    python3 scripts/editor_screenshot_qmp.py build/igis-shell.iso \
        --output /tmp/igis-editor.png

The script never opens the image for viewing: it only reads its size and
dimensions so the path can be reported. The proof holds the live frame for
about five seconds after `ready`; the harness captures as soon as it sees the
line and then terminates QEMU.
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

DEFAULT_OUTPUT = "/home/sinan/Desktop/igis-editor.png"


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

    def wait_for(self, pattern, timeout=30.0):
        compiled = re.compile(pattern)
        deadline = time.time() + timeout
        with self.condition:
            index = 0
            while True:
                while index < len(self.lines):
                    match = compiled.search(self.lines[index])
                    index += 1
                    if match:
                        return match
                remaining = deadline - time.time()
                if remaining <= 0:
                    raise TimeoutError(f"serial: no match for {pattern!r}")
                self.condition.wait(remaining)


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

    def close(self):
        try:
            self.file.close()
            self.socket.close()
        except OSError:
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


def capture(qmp, output):
    """Writes a PNG to `output`; returns the path actually produced."""
    try:
        qmp.command("screendump", {"filename": output, "format": "png"})
        return output
    except RuntimeError as error:
        print(f"[harness] screendump PNG refused ({error}); using PPM")
    fd, ppm = tempfile.mkstemp(suffix=".ppm", prefix="igis-editor-")
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
    parser.add_argument("iso", help="path to build/igis-shell.iso")
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
        tempfile.gettempdir(), f"igis-editor-{os.getpid()}.qmp"
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
        process.stdin.write(b"run editor shot\n")
        process.stdin.flush()
        serial.wait_for(r"editor: ready", timeout=args.timeout)
        # Give the first full present time to reach the framebuffer.
        time.sleep(0.4)

        path = capture(qmp, output)
        size = os.path.getsize(path)
        dimensions = image_dimensions(path)
        if dimensions is None:
            print(f"[harness] screenshot {path} ({size} bytes, dimensions unknown)")
            return 1
        width, height = dimensions
        print(
            f"[harness] screenshot {path} ({size} bytes, {width}x{height})"
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
