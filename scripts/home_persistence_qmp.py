#!/usr/bin/env python3
"""Prove that the writable home disk survives a reboot (spec 0019, V2/V3).

This is a verification harness, not part of the kernel. It boots the shell
image twice with the *same* `build/home.img`, writes a file and a directory
from `home_persist` in the first boot, power-cycles QEMU, and checks the bytes
and the tree with `home_verify` in the second boot. Afterwards it runs
`e2fsck -fn` on the host image and requires exit status 0.

Why two QEMU processes: the proof is persistence across a power cycle, not a
guest reboot. The writes are synchronous (spec 0019, H4), so quitting QEMU
after `home_persist: ok` leaves the blocks on disk.

Usage:
    python3 scripts/home_persistence_qmp.py --cores 4 --reset
    python3 scripts/home_persistence_qmp.py --cores 1
    python3 scripts/home_persistence_qmp.py --reset --keep

`--reset` runs `cargo xtask image --reset-home` first, so the test starts from
the initial tree. `--keep` leaves the image as the test left it; without it the
image is reset again at the end (after the fsck) so a normal run leaves the
repository clean. Both boots wait for `ext2w: home ok` and the shell prompt, so
a boot without the disk fails instead of silently verifying nothing.
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

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ISO = os.path.join(ROOT, "build", "igis-shell.iso")
IMAGE = os.path.join(ROOT, "build", "home.img")


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

    def wait_for(self, pattern, timeout=60.0):
        compiled = re.compile(pattern)
        deadline = time.time() + timeout
        index = 0
        with self.condition:
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

    def __init__(self, path, timeout=15.0):
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


def reset_home():
    """Rebuilds build/home.img from samples/home (spec 0019, H1/H2)."""
    print("[harness] resetting the home image", flush=True)
    result = subprocess.run(
        ["cargo", "xtask", "image", "--reset-home"],
        cwd=ROOT,
        text=True,
        capture_output=True,
    )
    if result.stdout:
        print(result.stdout, end="", flush=True)
    if result.returncode != 0:
        if result.stderr:
            print(result.stderr, end="", file=sys.stderr, flush=True)
        raise RuntimeError("cargo xtask image --reset-home failed")


def qemu_command(cores, socket_path):
    return [
        "qemu-system-x86_64",
        "-drive",
        f"file={IMAGE},format=raw,if=ide",
        "-cdrom",
        ISO,
        "-m",
        "256M",
        "-smp",
        str(cores),
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


def boot_and_run(cores, command, verdict_label, timeout):
    """One QEMU power-on: wait for the disk and prompt, run, wait for verdict.

    Returns (verdict, boot_seconds, command_seconds). The process is quit
    through QMP once the application prints its verdict; the write path is
    synchronous, so this is a clean power cycle rather than a flush.
    """
    socket_path = os.path.join(
        tempfile.gettempdir(), f"igis-home-persist-{os.getpid()}.qmp"
    )
    try:
        os.unlink(socket_path)
    except FileNotFoundError:
        pass

    process = subprocess.Popen(
        qemu_command(cores, socket_path),
        cwd=ROOT,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    serial = Serial(process.stdout)
    qmp = None
    verdict_pattern = (
        rf"{re.escape(verdict_label)}: (ok|FAILED|no writable home)"
    )
    try:
        qmp = Qmp(socket_path, timeout=timeout)
        boot_start = time.time()
        serial.wait_for(r"ext2w: home ok", timeout=timeout)
        serial.wait_for(r"(?:shell> |igis:/[^>]*> )", timeout=timeout)
        boot_seconds = time.time() - boot_start

        command_start = time.time()
        process.stdin.write((command + "\n").encode())
        process.stdin.flush()
        match = serial.wait_for(verdict_pattern, timeout=timeout)
        command_seconds = time.time() - command_start

        verdict = match.group(0)
        # Quit through QMP: a power cycle, not a guest shutdown. The reply can
        # race the socket closing, so a missing reply is not a failure.
        try:
            qmp.command("quit")
        except (RuntimeError, OSError):
            pass
        return verdict, boot_seconds, command_seconds
    finally:
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
        if qmp is not None:
            qmp.close()
        try:
            os.unlink(socket_path)
        except FileNotFoundError:
            pass


def e2fsck():
    """Runs the read-only host check and returns (ok, summary text)."""
    print(f"[harness] e2fsck -fn {IMAGE}", flush=True)
    result = subprocess.run(
        ["e2fsck", "-fn", IMAGE],
        cwd=ROOT,
        text=True,
        capture_output=True,
    )
    output = (result.stdout or "") + (result.stderr or "")
    if output:
        print(output, end="", flush=True)
    summary = ""
    for line in output.splitlines():
        if re.search(r"\d+/\d+ files", line):
            summary = line.strip()
    return result.returncode == 0, summary


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--cores", default="4")
    parser.add_argument("--reset", action="store_true")
    parser.add_argument("--keep", action="store_true")
    parser.add_argument("--timeout", type=float, default=120.0)
    args = parser.parse_args()

    if not os.path.exists(ISO):
        sys.exit(f"missing ISO: {ISO} (run `cargo xtask image --shell`)")
    if not os.path.exists(IMAGE):
        if not args.reset:
            sys.exit(f"missing home image: {IMAGE} (use --reset)")
    if args.reset:
        try:
            reset_home()
        except RuntimeError as error:
            sys.exit(str(error))
    if not os.path.exists(IMAGE):
        sys.exit(f"missing home image after reset: {IMAGE}")

    print(f"[harness] image {IMAGE}")
    print(f"[harness] iso   {ISO}")
    print(f"[harness] cores {args.cores}", flush=True)

    started = time.time()
    try:
        verdict1, boot1, cmd1 = boot_and_run(
            args.cores, "run home_persist", "home_persist", args.timeout
        )
        print(
            f"[harness] boot 1: {verdict1} "
            f"(boot {boot1:.2f}s, command {cmd1:.2f}s)",
            flush=True,
        )
        if verdict1 != "home_persist: ok":
            print("[harness] FAIL: the write boot did not report ok")
            return 1

        verdict2, boot2, cmd2 = boot_and_run(
            args.cores, "run home_verify", "home_verify", args.timeout
        )
        print(
            f"[harness] boot 2: {verdict2} "
            f"(boot {boot2:.2f}s, command {cmd2:.2f}s)",
            flush=True,
        )
        if verdict2 != "home_verify: ok":
            print("[harness] FAIL: the verification boot did not report ok")
            return 1

        clean, summary = e2fsck()
        if not clean:
            print("[harness] FAIL: e2fsck did not exit 0")
            return 1
        if summary:
            print(f"[harness] fsck summary: {summary}")
        print(
            f"[harness] persistence ok on {args.cores} core(s) "
            f"({time.time() - started:.1f}s total)"
        )
        return 0
    except (TimeoutError, RuntimeError, OSError) as error:
        print(f"[harness] FAIL: {error}")
        return 1
    finally:
        if not args.keep:
            try:
                reset_home()
            except RuntimeError as error:
                print(f"[harness] could not reset the image: {error}")
        else:
            print(f"[harness] kept {IMAGE}")


if __name__ == "__main__":
    sys.exit(main())
