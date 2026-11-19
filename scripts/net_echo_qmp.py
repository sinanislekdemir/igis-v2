#!/usr/bin/env python3
"""Prove the M10.7 in-guest TCP echo server end to end over QEMU user networking.

This is a verification harness, not part of the kernel. It boots the shell-entry
ISO headless with one user-mode NIC whose host port 7007 is forwarded to guest
port 7, waits for the shell prompt (`igis:/> ` or the older `shell> `),
runs `run net_echo`, then opens three
sequential Python TCP connections to 127.0.0.1:7007 and requires each line to
come back byte-for-byte. `net_echo: ok` and the prompt must follow.

Spec 0020 N20 asks for proofs on `-smp 4` and `-smp 1`, so both core counts run
by default, each in a fresh QEMU.

Usage:
    python3 scripts/net_echo_qmp.py                 # both core counts, virtio
    python3 scripts/net_echo_qmp.py --cores 1
    python3 scripts/net_echo_qmp.py --net-device e1000
    python3 scripts/net_echo_qmp.py --no-net        # only the no-device path

Exit status: 0 when every requested core count echoes all three lines and
prints `net_echo: ok` (or, with `--no-net`, prints `net_echo: no device`), 1 on
any boot/probe failure. The last 40 serial lines are printed on failure.
"""

import argparse
import os
import socket
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
# Importing the sibling harness must not leave a __pycache__ in the repo.
sys.dont_write_bytecode = True

from tablet_qmp import Serial  # noqa: E402

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DEFAULT_ISO = os.path.join("build", "igis-shell.iso")
GUEST_PORT = 7
DEFAULT_HOST_PORT = 7007
TAIL_LINES = 40
LINES = ["hello-1\n", "hello-2\n", "hello-3\n"]

VERDICT_RE = (
    r"net_echo: ok"
    r"|net_echo: (?:timeout|failed|failure|error)"
    r"|net_echo: no device"
    r"|run failed:"
    r"|child: (?:faulted|killed)"
)


def net_device_args(net_device, host_port):
    """The `-netdev`/`-device` pair for the requested NIC."""
    netdev = f"user,id=n0,hostfwd=tcp::{host_port}-:{GUEST_PORT}"
    if net_device == "virtio":
        device = "virtio-net-pci,netdev=n0,disable-modern=on"
    elif net_device in ("e1000", "e1000e"):
        device = f"{net_device},netdev=n0"
    else:
        raise ValueError(f"unknown net device {net_device}")
    return ["-netdev", netdev, "-device", device]


def qemu_command(iso, cores, net_device, no_net, host_port):
    """The `xtask run --headless` shape plus one forwarded user-mode NIC."""
    command = [
        "qemu-system-x86_64",
        "-cdrom",
        iso,
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
    ]
    if no_net:
        # QEMU adds a default NIC when none is given; `-nic none` omits it.
        command += ["-nic", "none"]
    else:
        command += net_device_args(net_device, host_port)
    return command


def send(process, text):
    """Writes one line to the guest's serial stdin, tolerating a dead QEMU."""
    try:
        process.stdin.write(text.encode())
        process.stdin.flush()
        return True
    except (BrokenPipeError, ValueError) as error:
        print(f"[echo] cannot write to QEMU stdin: {error}")
        return False


def echo_once(host_port, payload, timeout):
    """Opens one TCP connection, sends `payload`, and reads the same bytes back."""
    deadline = time.time() + timeout
    last_error = None
    while time.time() < deadline:
        try:
            with socket.create_connection(("127.0.0.1", host_port), timeout=3.0) as client:
                client.sendall(payload.encode())
                received = b""
                while len(received) < len(payload):
                    chunk = client.recv(4096)
                    if not chunk:
                        break
                    received += chunk
                return received.decode("utf-8", "replace")
        except OSError as error:
            last_error = error
            time.sleep(0.2)
    raise TimeoutError(f"no echo on 127.0.0.1:{host_port}: {last_error}")


def print_tail(serial, count=TAIL_LINES):
    """Prints the last few serial lines so a failure is diagnosable."""
    tail = serial.lines[-count:]
    print(f"[echo] last {len(tail)} serial lines:")
    for line in tail:
        print(f"[echo] | {line}")


def run_case(iso, cores, timeout, net_device, no_net, host_port):
    command = qemu_command(iso, cores, net_device, no_net, host_port)
    print(f"[echo] core count {cores}")
    print(f"[echo] {' '.join(command)}")
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
        if not send(process, "run net_echo\n"):
            print_tail(serial)
            return 1

        if no_net:
            match, verdict_index = serial.wait_for(
                r"net_echo: no device|run failed:|child: (?:faulted|killed)",
                timeout=timeout,
                start=start,
            )
            if match.group(0) != "net_echo: no device":
                print(f"[echo] FAIL core {cores}: guest verdict {match.group(0)!r}")
                print_tail(serial)
                return 1
            serial.wait_for(r"(?:shell> |igis:/[^>]*> )", timeout=timeout, start=verdict_index)
            print(f"[echo] core {cores}: net_echo: no device")
            return 0

        _, ready_index = serial.wait_for(r"net_echo: ready", timeout=timeout, start=start)
        for line in LINES:
            echoed = echo_once(host_port, line, timeout)
            if echoed != line:
                print(
                    f"[echo] FAIL core {cores}: sent {line!r}, received {echoed!r}"
                )
                print_tail(serial)
                return 1
            print(f"[echo] core {cores}: echoed {line.strip()!r}")

        match, verdict_index = serial.wait_for(VERDICT_RE, timeout=timeout, start=ready_index)
        if match.group(0) != "net_echo: ok":
            print(f"[echo] FAIL core {cores}: guest verdict {match.group(0)!r}")
            print_tail(serial)
            return 1

        # The app must have exited and the shell must take the next command.
        serial.wait_for(r"(?:shell> |igis:/[^>]*> )", timeout=min(timeout, 20.0), start=verdict_index)
        print(f"[echo] core {cores}: net_echo: ok")
        for line in serial.lines:
            if line.startswith("net_echo:"):
                print(f"[echo]   {line}")
        return 0
    except TimeoutError as error:
        print(f"[echo] FAIL core {cores}: {error}")
        if process.poll() is not None:
            print(f"[echo] QEMU exited with status {process.returncode}")
        print_tail(serial)
        return 1
    finally:
        stop_process(process)


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


def main():
    parser = argparse.ArgumentParser(
        description=(
            "Boot the shell ISO with QEMU user networking and require `run "
            "net_echo` to echo three host TCP lines and print `net_echo: ok`."
        ),
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=(
            "examples:\n"
            "  python3 scripts/net_echo_qmp.py                # -smp 4 then -smp 1\n"
            "  python3 scripts/net_echo_qmp.py --cores 1\n"
            "  python3 scripts/net_echo_qmp.py --net-device e1000\n"
            "  python3 scripts/net_echo_qmp.py --no-net\n"
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
        help="seconds allowed for boot and for the whole echo run (default: 90)",
    )
    parser.add_argument(
        "--net-device",
        choices=["virtio", "e1000", "e1000e"],
        default="virtio",
        help="QEMU NIC model (default: virtio)",
    )
    parser.add_argument(
        "--host-port",
        type=int,
        default=DEFAULT_HOST_PORT,
        help=f"host port forwarded to guest 7 (default: {DEFAULT_HOST_PORT})",
    )
    parser.add_argument(
        "--no-net",
        action="store_true",
        help="boot with no NIC and require `net_echo: no device`",
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
    if args.dry_run:
        if not os.path.exists(iso):
            print(f"[echo] note: ISO does not exist yet: {iso}")
        for cores in cores_list:
            print(
                " ".join(
                    qemu_command(iso, cores, args.net_device, args.no_net, args.host_port)
                )
            )
        return 0

    if not os.path.exists(iso):
        sys.exit(f"missing ISO: {iso} (run `cargo xtask image --shell`)")

    # A stale host process on the forwarded port would receive the client
    # connections, so refuse to start rather than report a false success.
    if not args.no_net:
        probe = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        try:
            probe.bind(("127.0.0.1", args.host_port))
        except OSError as error:
            sys.exit(f"host port {args.host_port} is already in use: {error}")
        finally:
            probe.close()

    status = 0
    for cores in cores_list:
        status |= run_case(
            iso,
            cores,
            args.timeout,
            args.net_device,
            args.no_net,
            args.host_port,
        )

    if status == 0:
        print(
            f"[echo] proof passed on all {len(cores_list)} core count(s) "
            f"({', '.join(str(cores) for cores in cores_list)})"
        )
    else:
        print("[echo] FAIL: net_echo did not pass on every core count")
    return 0 if status == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
