#!/usr/bin/env python3
"""Prove the M2.6.4 system-control primitives through separate QEMU runs.

This is a verification harness, not part of the kernel, and deliberately not
part of the normal regression suite: both modes stop or reset the machine.

    halt    boots `build/igis-shell.iso` with `-no-shutdown -no-reboot`, runs
            `run system/system_probe halt`, waits for the stop summary, then sends
            another command and requires that serial produces no reply: every
            core really stopped (spec 0017).
    reboot  boots without `-no-reboot`, runs `run system/system_probe reboot`, and
            requires the `igis-v2: long mode entered` banner to appear a second
            time: the platform reset restarted the machine (spec 0017).

Usage:
    cargo xtask image --shell
    python3 scripts/system_control_qmp.py build/igis-shell.iso --mode halt
    python3 scripts/system_control_qmp.py build/igis-shell.iso --mode reboot
    python3 scripts/system_control_qmp.py build/igis-shell.iso --mode both \
        --cores 1
"""

import argparse
import os
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
# Importing the sibling harness must not leave a __pycache__ in the repo.
sys.dont_write_bytecode = True

from tablet_qmp import Serial  # noqa: E402


def command(iso, cores, reboot):
    cmd = [
        "qemu-system-x86_64",
        "-cdrom",
        iso,
        "-m",
        "256M",
        "-smp",
        cores,
        "-serial",
        "stdio",
        "-monitor",
        "none",
        "-accel",
        "tcg",
        "-vga",
        "std",
        "-display",
        "none",
        "-no-shutdown",
        "-usb",
        "-device",
        "usb-tablet",
    ]
    if not reboot:
        cmd.insert(7, "-no-reboot")
    return cmd


def boot(iso, cores, reboot, timeout):
    process = subprocess.Popen(
        command(iso, cores, reboot),
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    serial = Serial(process.stdout)
    serial.wait_for(r"(?:shell> |igis:/[^>]*> )", timeout=timeout)
    return process, serial


def stop(process):
    process.terminate()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()


def run_halt(iso, cores, timeout):
    process, serial = boot(iso, cores, reboot=False, timeout=timeout)
    try:
        process.stdin.write(b"run system/system_probe halt\n")
        process.stdin.flush()
        serial.wait_for(r"system_probe: halt requested", timeout=timeout)
        serial.wait_for(r"system: halt requested", timeout=timeout)
        summary, _ = serial.wait_for(
            r"system: (\d+) of (\d+) APs stopped", timeout=timeout
        )
        stopped, expected = int(summary.group(1)), int(summary.group(2))
        print(f"[halt] {stopped} of {expected} APs acknowledged the stop")

        # Prove the whole machine stopped: no core services this line, so no
        # echo and no prompt may follow it.
        mark = len(serial.lines)
        process.stdin.write(b"echo after-halt\n")
        process.stdin.flush()
        time.sleep(2.0)
        tail = serial.lines[mark:]
        leaked = [line for line in tail if "after-halt" in line or "shell>" in line]
        if leaked:
            print(f"[halt] FAIL: serial still active after halt: {leaked[-3:]}")
            return 1
        if not any("system_probe: halt returned" in line for line in serial.lines):
            print("[halt] ok: the halt never returned to the probe")
            return 0
        print("[halt] FAIL: `core.halt` returned to the application")
        return 1
    finally:
        stop(process)


def run_reboot(iso, cores, timeout):
    process, serial = boot(iso, cores, reboot=True, timeout=timeout)
    try:
        first = len(serial.lines)
        process.stdin.write(b"run system/system_probe reboot\n")
        process.stdin.flush()
        serial.wait_for(r"system_probe: reboot requested", timeout=timeout)
        serial.wait_for(r"system: reboot requested", timeout=timeout)
        # The reset restarts the guest: the boot banner must appear again.
        serial.wait_for(r"igis-v2: long mode entered", timeout=timeout, start=first)
        print("[reboot] ok: a second `igis-v2: long mode entered` banner appeared")
        return 0
    finally:
        stop(process)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("iso", help="path to build/igis-shell.iso")
    parser.add_argument("--mode", choices=["halt", "reboot", "both"], default="both")
    parser.add_argument("--cores", default="4")
    parser.add_argument("--timeout", type=float, default=90.0)
    args = parser.parse_args()

    iso = os.path.abspath(args.iso)
    if not os.path.exists(iso):
        sys.exit(f"missing ISO: {iso}")

    status = 0
    if args.mode in ("halt", "both"):
        status |= run_halt(iso, args.cores, args.timeout)
    if args.mode in ("reboot", "both"):
        status |= run_reboot(iso, args.cores, args.timeout)
    return 1 if status else 0


if __name__ == "__main__":
    sys.exit(main())
