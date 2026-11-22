#!/usr/bin/env python3
"""Prove the M10 networking stack end to end through QEMU user networking.

This is a verification harness, not part of the kernel. It boots the shell-entry
ISO headless with one user-mode virtio-net NIC, waits for the shell prompt
(`igis:/> `; the older `shell> ` still matches),
runs `run net_probe http`, and requires `net_probe: ok` on the serial console
(optionally plus a response-body marker). Spec 0020 N20 asks for proofs on
`-smp 4` and `-smp 1`, so both core counts run by default, each in a fresh
QEMU.

QEMU's user networking (slirp) gives the guest `10.0.2.15/24`, maps the host to
`10.0.2.2` and reserves `10.0.2.3` for DNS. `net_probe http` fetches
`http://10.0.2.2:6666/`, which is the host's `python3 -m http.server 6666`.
The harness checks that server before booting and never starts a duplicate
unless `--serve` asks for one.

Usage:
    python3 scripts/net_qmp.py                 # both core counts
    python3 scripts/net_qmp.py --cores 1
    python3 scripts/net_qmp.py --serve --expect "Directory listing"

Exit status: 0 when every core count prints `net_probe: ok` (and the optional
marker), 1 on a boot/probe failure (the last 40 serial lines are printed), and 1
with a clear message when no host server answers and `--serve` was not given.
`--dry-run` prints the QEMU command without booting anything, so it works
before `net_probe` exists and even before the ISO is built.
"""

import argparse
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.request

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
# Importing the sibling harness must not leave a __pycache__ in the repo.
sys.dont_write_bytecode = True

from tablet_qmp import Serial, apps_drive  # noqa: E402

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DEFAULT_ISO = os.path.join("build", "igis-shell.iso")
SERVER_URL = "http://127.0.0.1:6666/"
SERVER_PORT = 6666
TAIL_LINES = 40

# The shell prints `run failed: ...` for a synchronous load failure and
# `child: ...` for an exit reason; the probe prints `net_probe: timeout` (or a
# similar status word) when its deadline expires. Any of these is a verdict,
# so a failing run is reported instead of waiting for the full timeout.
VERDICT_RE = (
    r"net_probe: ok"
    r"|net_probe: (?:timeout|failed|failure|error)"
    r"|run failed:"
    r"|child: (?:faulted|killed)"
)

# Lines worth repeating in the success summary.
NET_LINE_RE = re.compile(
    r"net_probe:|HTTP/|^GET |Directory listing|^net:"
)


def host_server_answers(timeout=2.0):
    """True when http://127.0.0.1:6666/ answers with a 2xx/3xx status."""
    try:
        with urllib.request.urlopen(SERVER_URL, timeout=timeout) as response:
            response.read(64)
            return 200 <= response.status < 400
    except Exception:
        return False


def start_host_server(timeout=5.0):
    """Starts `python3 -m http.server 6666` in a temp dir, or returns None."""
    directory = tempfile.mkdtemp(prefix="igis-net-http-")
    process = subprocess.Popen(
        [sys.executable, "-m", "http.server", str(SERVER_PORT)],
        cwd=directory,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    deadline = time.time() + timeout
    while time.time() < deadline:
        if process.poll() is not None:
            break
        if host_server_answers(timeout=0.5):
            # Another server may have won the port; never report ours when it
            # has already exited.
            if process.poll() is None:
                return process, directory
            break
        time.sleep(0.1)
    stop_process(process)
    shutil.rmtree(directory, ignore_errors=True)
    return None


def stop_process(process):
    """Terminates a child, escalating to kill after a bounded wait."""
    if process.poll() is not None:
        return
    process.terminate()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            pass


def send(process, text):
    """Writes one line to the guest's serial stdin, tolerating a dead QEMU."""
    try:
        process.stdin.write(text.encode())
        process.stdin.flush()
        return True
    except (BrokenPipeError, ValueError) as error:
        print(f"[net] cannot write to QEMU stdin: {error}")
        return False


def qemu_command(iso, cores, home):
    """The `xtask run --headless` shape plus one user-mode virtio-net NIC."""
    command = ["qemu-system-x86_64"]
    if home:
        # The home disk is irrelevant to the network proof and stays off by
        # default, but keep the same fixed drive order as `xtask run` when it
        # is requested.
        command += ["-drive", f"file={home},format=raw,if=ide"]
    command += [
        "-cdrom",
        iso,
        *apps_drive(iso),
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
        "-usb",
        "-device",
        "usb-tablet",
        "-netdev",
        "user,id=n0",
        "-device",
        "virtio-net-pci,netdev=n0,disable-modern=on",
    ]
    return command


def print_tail(serial, count=TAIL_LINES):
    """Prints the last few serial lines so a failure is diagnosable."""
    tail = serial.lines[-count:]
    print(f"[net] last {len(tail)} serial lines:")
    for line in tail:
        print(f"[net] | {line}")


def run_case(iso, cores, timeout, expect, home):
    """Boots one fresh QEMU and returns 0 when `net_probe` reports `ok`."""
    command = qemu_command(iso, cores, home)
    print(f"[net] core count {cores}")
    print(f"[net] {' '.join(command)}")
    process = subprocess.Popen(
        command,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    serial = Serial(process.stdout)
    try:
        serial.wait_for(r"(?:shell> |igis:/[^>]*> )", timeout=timeout)
        start = len(serial.lines)
        if not send(process, "run net_probe http\n"):
            print_tail(serial)
            return 1

        match, _ = serial.wait_for(VERDICT_RE, timeout=timeout, start=start)
        if match.group(0) != "net_probe: ok":
            print(f"[net] FAIL core {cores}: guest verdict {match.group(0)!r}")
            print_tail(serial)
            return 1

        if expect is not None and not any(expect in line for line in serial.lines):
            try:
                serial.wait_for(re.escape(expect), timeout=min(timeout, 5.0))
            except TimeoutError:
                print(
                    f"[net] FAIL core {cores}: `net_probe: ok` arrived but the "
                    f"response body marker {expect!r} did not"
                )
                print_tail(serial)
                return 1

        print(f"[net] core {cores}: net_probe: ok")
        if expect is not None:
            print(f"[net] core {cores}: body marker {expect!r} present")
        seen = 0
        for line in serial.lines:
            if NET_LINE_RE.search(line):
                print(f"[net]   {line}")
                seen += 1
        if seen == 0:
            print("[net]   (no net_probe/HTTP lines captured)")
        return 0
    except TimeoutError as error:
        print(f"[net] FAIL core {cores}: {error}")
        if process.poll() is not None:
            print(f"[net] QEMU exited with status {process.returncode}")
        print_tail(serial)
        return 1
    finally:
        stop_process(process)


def main():
    parser = argparse.ArgumentParser(
        description=(
            "Boot the shell ISO with QEMU user networking and require "
            "`run net_probe http` to print `net_probe: ok`."
        ),
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=(
            "examples:\n"
            "  python3 scripts/net_qmp.py                 # -smp 4 then -smp 1\n"
            "  python3 scripts/net_qmp.py --cores 1\n"
            "  python3 scripts/net_qmp.py --serve --expect \"Directory listing\"\n"
        ),
    )
    parser.add_argument(
        "--iso",
        default=DEFAULT_ISO,
        help=f"shell-entry ISO to boot (default: {DEFAULT_ISO})",
    )
    parser.add_argument(
        "--cores",
        nargs="+",
        type=int,
        default=[4, 1],
        metavar="N",
        help="core counts to prove, each in a fresh QEMU (default: 4 1)",
    )
    parser.add_argument(
        "--timeout",
        type=float,
        default=90.0,
        help="seconds allowed for boot and for the probe (default: 90)",
    )
    parser.add_argument(
        "--expect",
        default=None,
        help="response-body text that must also appear on the serial console",
    )
    parser.add_argument(
        "--serve",
        action="store_true",
        help=(
            "start `python3 -m http.server 6666` when no host server answers "
            "(never starts a duplicate)"
        ),
    )
    parser.add_argument(
        "--home",
        action="store_true",
        help="attach build/home.img (default: no home disk; irrelevant to the proof)",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="print the QEMU command and exit without booting",
    )
    args = parser.parse_args()

    cores_list = []
    for cores in args.cores:
        if cores < 1:
            parser.error("--cores values must be at least 1")
        if cores not in cores_list:
            cores_list.append(cores)

    iso = os.path.abspath(args.iso)
    home = None
    if args.home:
        home = os.path.join(ROOT, "build", "home.img")
        if not os.path.exists(home):
            sys.exit(f"missing home disk: {home} (run `cargo xtask image`)")

    if args.dry_run:
        if not os.path.exists(iso):
            print(f"[net] note: ISO does not exist yet: {iso}")
        for cores in cores_list:
            print(" ".join(qemu_command(iso, cores, home)))
        if not args.serve and not host_server_answers():
            print(
                f"[net] note: no host server answers {SERVER_URL}; "
                "a real run needs one, or pass --serve"
            )
        return 0

    if not os.path.exists(iso):
        sys.exit(f"missing ISO: {iso} (run `cargo xtask image --shell`)")

    server = None
    directory = None
    if host_server_answers():
        print(f"[net] host server {SERVER_URL} answers")
    elif args.serve:
        started = start_host_server()
        if started is None:
            sys.exit(f"could not start `python3 -m http.server {SERVER_PORT}`")
        server, directory = started
        print(f"[net] started host server {SERVER_URL} in {directory}")
    else:
        sys.exit(
            f"no host server answering {SERVER_URL}; start "
            f"`python3 -m http.server {SERVER_PORT}` or pass --serve"
        )

    status = 0
    try:
        for cores in cores_list:
            status |= run_case(iso, cores, args.timeout, args.expect, home)
    finally:
        if server is not None:
            stop_process(server)
        if directory is not None:
            shutil.rmtree(directory, ignore_errors=True)

    if status == 0:
        print(
            f"[net] net_probe: ok on all {len(cores_list)} core count(s) "
            f"({', '.join(str(cores) for cores in cores_list)})"
        )
    else:
        print("[net] FAIL: net_probe did not pass on every core count")
    return 0 if status == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
