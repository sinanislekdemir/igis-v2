#!/usr/bin/env python3
"""Relative-motion burst proof for the PS/2 fallback (spec 0010, M2.2.7; Q49).

This is a verification harness, not part of the kernel. It boots the ISO with
the USB controller but **without** `usb-tablet`, so the PS/2 auxiliary device
is the pointer source, runs the `pointer` diagnostic app, clamps the pointer to
the left edge and then injects a burst of relative QMP events. It compares the
guest's final x with the total requested motion.

With polling (before M2.2.7) Q49 measured +346 of 600 px through a host window,
because the kernel drained the controller about once per millisecond. With IRQ12
draining, the handler empties the controller as bytes arrive. On QEMU 10.2.2 the
emulated PS/2 model accumulates deltas in `mouse_dx` and defers packets instead of
dropping them, so the polled build also reports 600/600 headlessly; the historical
host-window number is not reproducible through QMP. The harness is still the
regression that the IRQ path is one-for-one, and the serial transcript's
`mouse: aux IRQ12 delivered ...` line proves the interrupt (not polling) drained it.

Usage:
    python3 scripts/mouse_burst_qmp.py build/igis.iso
    python3 scripts/mouse_burst_qmp.py build/igis.iso --cores 1

Exit status is 0 when the guest moved exactly the requested distance.
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

POINTER_RE = r"pointer: (-?\d+) (-?\d+) (\d+)"


def last_pointer(serial):
    """The most recent `pointer: x y buttons` line, or None."""
    with serial.condition:
        for line in reversed(serial.lines):
            match = re.search(POINTER_RE, line)
            if match:
                return (
                    int(match.group(1)),
                    int(match.group(2)),
                    int(match.group(3)),
                )
    return None


def wait_settled(serial, settle=0.4, timeout=8.0):
    """Waits until the pointer report stops changing for `settle` seconds."""
    deadline = time.time() + timeout
    value = last_pointer(serial)
    stable_since = time.time()
    while time.time() < deadline:
        time.sleep(0.1)
        latest = last_pointer(serial)
        if latest != value:
            value = latest
            stable_since = time.time()
        elif time.time() - stable_since >= settle:
            break
    return value


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("iso", help="path to build/igis.iso")
    parser.add_argument("--cores", default="4")
    parser.add_argument("--events", type=int, default=300)
    parser.add_argument("--delta", type=int, default=2)
    parser.add_argument(
        "--batch",
        type=int,
        default=0,
        help="events per QMP command (0: all in one command)",
    )
    parser.add_argument("--timeout", type=float, default=90.0)
    args = parser.parse_args()

    iso = os.path.abspath(args.iso)
    if not os.path.exists(iso):
        sys.exit(f"missing ISO: {iso}")

    socket_path = os.path.join(
        tempfile.gettempdir(), f"igis-burst-{os.getpid()}.qmp"
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
        # The UHCI controller is present but no tablet is attached, so the
        # PS/2 auxiliary device is the pointer source (pointer_source == 1).
        "-usb",
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
        process.stdin.write(b"run pointer\n")
        process.stdin.flush()

        source_match, _ = serial.wait_for(r"pointer: source\s+(\d+)", timeout=30)
        source = int(source_match.group(1))
        wait_settled(serial, settle=0.2, timeout=5)
        baseline = last_pointer(serial)
        print(f"\n[harness] pointer_source={source} baseline={baseline}")

        # Clamp to the left edge: enough negative relative motion that the
        # kernel's clamp definitely stops at 0. QEMU's per-event delta is
        # limited to one PS/2 axis byte, so this is a small burst of -100.
        for _ in range(20):
            qmp.command(
                "input-send-event",
                {"events": [{"type": "rel", "data": {"axis": "x", "value": -100}}]},
            )
        clamped = wait_settled(serial)
        print(f"[harness] after clamp: {clamped}")
        if clamped is None or clamped[0] != 0:
            print(f"[harness] FAIL clamp did not reach x=0 ({clamped})")
            return 1

        # The burst: one QMP rel event per 2 guest pixels, sent as fast as the
        # socket accepts them (the shape Q49 measured). A batch is one command,
        # so QEMU applies all events before the vCPU runs.
        total = args.events * args.delta
        batch = args.batch if args.batch > 0 else args.events
        started = time.time()
        remaining = args.events
        while remaining > 0:
            count = min(batch, remaining)
            events = [
                {"type": "rel", "data": {"axis": "x", "value": args.delta}}
                for _ in range(count)
            ]
            qmp.command("input-send-event", {"events": events})
            remaining -= count
        elapsed = time.time() - started
        final = wait_settled(serial)
        print(
            f"[harness] burst {args.events}x{args.delta:+d} (batch {batch}) "
            f"in {elapsed:.2f}s -> {final}"
        )

        if final is None:
            print("[harness] FAIL no pointer report after the burst")
            return 1
        moved = final[0] - clamped[0]
        percent = 100.0 * moved / total if total else 0.0
        ok = moved == total and final[1] == clamped[1]
        print(
            f"[harness] moved {moved} of {total} px ({percent:.1f}%) "
            f"[{'ok' if ok else 'FAIL'}]"
        )

        process.stdin.write(b"exit\n")
        process.stdin.flush()
        try:
            serial.wait_for(r"bye", timeout=20)
        except TimeoutError:
            pass
        print(
            f"\n[harness] burst {'passed' if ok else 'FAILED'} "
            f"({moved}/{total} px, pointer_source={source})"
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


if __name__ == "__main__":
    sys.exit(main())
