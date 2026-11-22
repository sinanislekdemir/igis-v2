#!/usr/bin/env python3
"""Drive the Q39 multiplex proof in a headless shell boot.

Boots `build/igis-shell.iso` (the pre-desktop image, so no boot desktop
competes for the machine-wide event queue), runs `multiplex check` for the
deterministic stream + input arms, then runs `multiplex` and types the console
line that completes the third arm. Every arm prints its own line, so a timeout
names the missing step.

Usage:
    cargo xtask image --shell
    python3 scripts/multiplex_qmp.py build/igis-shell.iso
    python3 scripts/multiplex_qmp.py build/igis-shell.iso --cores 1

The script writes nothing and needs no QMP socket: it only types into the
serial console and checks the kernel's transcript.
"""

import argparse
import os
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
# Importing the sibling harness must not leave a __pycache__ in the repo.
sys.dont_write_bytecode = True

from tablet_qmp import Serial, apps_drive  # noqa: E402

PROMPT = r"(?:shell> |igis:/[^>]*> )"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("iso", help="path to build/igis-shell.iso")
    parser.add_argument("--cores", default="4")
    parser.add_argument("--timeout", type=float, default=90.0)
    args = parser.parse_args()

    iso = os.path.abspath(args.iso)
    if not os.path.exists(iso):
        sys.exit(f"missing ISO: {iso}")

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
    ]

    process = subprocess.Popen(
        command,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    serial = Serial(process.stdout)
    try:
        serial.wait_for(PROMPT, timeout=args.timeout)
        print("[harness] shell ready")

        # Deterministic arms: stream (spawned sender) then injected input.
        process.stdin.write(b"run multiplex check\n")
        process.stdin.flush()
        serial.wait_for(r"multiplex: stream source ready ok", timeout=args.timeout)
        serial.wait_for(r"multiplex: input source ready ok", timeout=args.timeout)
        serial.wait_for(r"multiplex: ok", timeout=args.timeout)
        serial.wait_for(PROMPT, timeout=args.timeout)
        print("[harness] check mode ok")

        # Full mode: the console arm waits for a typed line.
        process.stdin.write(b"run multiplex\n")
        process.stdin.flush()
        serial.wait_for(r"multiplex: stream source ready ok", timeout=args.timeout)
        serial.wait_for(r"multiplex: input source ready ok", timeout=args.timeout)
        serial.wait_for(r"multiplex: waiting for console", timeout=args.timeout)
        time.sleep(0.2)
        process.stdin.write(b"tick\n")
        process.stdin.flush()
        serial.wait_for(r"multiplex: console source ready ok", timeout=args.timeout)
        serial.wait_for(r"multiplex: console line ok", timeout=args.timeout)
        serial.wait_for(r"multiplex: ok", timeout=args.timeout)
        serial.wait_for(PROMPT, timeout=args.timeout)
        print(f"[harness] multiplex ok on -smp {args.cores}")
        return 0
    finally:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()


if __name__ == "__main__":
    sys.exit(main())
