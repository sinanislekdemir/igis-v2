#!/usr/bin/env python3
"""Capture the terminal running the network commands over the System Log.

Boots `build/igis-shell.iso`, starts the terminal emulator from the serial
shell, waits for its inner shell prompt, runs `ifconfig dhcp`,
`nslookup example.com` and `net_probe dns`, and captures the complete
framebuffer. The desktop shell is not involved, so the System Log and the
terminal chrome are the background, matching the other full-screen captures.

Usage:
    cargo xtask image --shell
    python3 scripts/network_screenshot_qmp.py build/igis-shell.iso
    python3 scripts/network_screenshot_qmp.py build/igis-shell.iso --cores 1
"""

import argparse
import os
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
sys.dont_write_bytecode = True

from tablet_qmp import Qmp, Serial  # noqa: E402

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT_DIR = os.path.join(ROOT, "docs", "screenshots")
PROMPT = r"(?:shell> |igis:/[^>]*> )"


def type_text(process, text):
    for byte in text.encode():
        process.stdin.write(bytes([byte]))
        process.stdin.flush()
        time.sleep(0.01)


def capture(qmp, path):
    try:
        qmp.command("screendump", {"filename": path, "format": "png"})
        return path
    except RuntimeError as error:
        print(f"[net-shot] screendump PNG refused ({error}); using PPM")
    fd, ppm = tempfile.mkstemp(suffix=".ppm", prefix="igis-net-shot-")
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


def run(args):
    iso = os.path.abspath(args.iso)
    if not os.path.exists(iso):
        sys.exit(f"missing ISO: {iso}")
    output = os.path.abspath(args.output)
    os.makedirs(os.path.dirname(output) or ".", exist_ok=True)

    socket_path = os.path.join(tempfile.gettempdir(), f"igis-net-shot-{os.getpid()}.qmp")
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
        _, cursor = serial.wait_for(PROMPT, timeout=args.timeout)
        print("[net-shot] starting the terminal emulator")
        type_text(process, "run apps/utilities/terminal\n")
        _, cursor = serial.wait_for(
            r"repl: apps/utilities/terminal: spawned as task", timeout=args.timeout, start=cursor
        )
        _, cursor = serial.wait_for(
            r"repl: base/shell: spawned as task", timeout=args.timeout, start=cursor
        )
        _, cursor = serial.wait_for(PROMPT, timeout=args.timeout, start=cursor)
        time.sleep(0.5)

        print("[net-shot] ifconfig dhcp, nslookup, net_probe dns")
        type_text(process, "ifconfig dhcp\n")
        _, cursor = serial.wait_for(
            r"ifconfig: dhcp 10\.0\.2\.15/24", timeout=args.timeout, start=cursor
        )
        type_text(process, "nslookup example.com\n")
        _, cursor = serial.wait_for(r"Address:", timeout=args.timeout, start=cursor)
        type_text(process, "net_probe dns\n")
        _, cursor = serial.wait_for(r"net_probe: ok", timeout=args.timeout, start=cursor)
        _, cursor = serial.wait_for(PROMPT, timeout=args.timeout, start=cursor)
        time.sleep(0.6)

        capture(qmp, output)
        print(f"[net-shot] screenshot {output}")
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
    parser.add_argument("iso", help="path to build/igis-shell.iso")
    parser.add_argument("--output", default=os.path.join(OUT_DIR, "network.png"))
    parser.add_argument("--cores", default="4")
    parser.add_argument("--timeout", type=float, default=90.0)
    args = parser.parse_args()
    return run(args)


if __name__ == "__main__":
    sys.exit(main())
