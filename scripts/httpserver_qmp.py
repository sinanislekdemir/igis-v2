#!/usr/bin/env python3
"""Prove the `httpserver` application end to end over QEMU user networking.

The shell-entry ISO boots headless with host port 7080 forwarded to guest port
8080. The harness runs `run httpserver 8080` (the explicit form serves the image
root), then checks:

  * a regular file is served byte-for-byte;
  * a directory serves its `index.html`;
  * a query string is ignored;
  * a file larger than one 256-byte read chunk streams completely;
  * `..`, `%2e%2e` and a leading `~` traversal attempt answer 403;
  * a missing file answers 404 and an unsupported method answers 405;
  * `HEAD` returns the length without a body;
  * Ctrl+C stops the server and returns to the shell;
  * a bare `httpserver` from `/base` serves the shell's current directory.

Both core counts run by default, each in a fresh QEMU.

Usage:
    python3 scripts/httpserver_qmp.py                 # -smp 4 then -smp 1
    python3 scripts/httpserver_qmp.py --cores 1
    python3 scripts/httpserver_qmp.py --net-device e1000

Exit status: 0 when every case passes on every requested core count.
"""

import argparse
import os
import socket
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
sys.dont_write_bytecode = True

from tablet_qmp import Serial, apps_drive  # noqa: E402

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DEFAULT_ISO = os.path.join("build", "igis-shell.iso")
GUEST_PORT = 8080
DEFAULT_HOST_PORT = 7080
TAIL_LINES = 40

FIXTURES = "samples/tests/apps/system/site"


def read_fixture(name):
    with open(os.path.join(ROOT, FIXTURES, name), "rb") as handle:
        return handle.read()


def net_device_args(net_device, host_port):
    netdev = f"user,id=n0,hostfwd=tcp::{host_port}-:{GUEST_PORT}"
    if net_device == "virtio":
        device = "virtio-net-pci,netdev=n0,disable-modern=on"
    elif net_device in ("e1000", "e1000e"):
        device = f"{net_device},netdev=n0"
    else:
        raise ValueError(f"unknown net device {net_device}")
    return ["-netdev", netdev, "-device", device]


def qemu_command(iso, cores, net_device, host_port):
    return [
        "qemu-system-x86_64",
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
        *net_device_args(net_device, host_port),
    ]


def send(process, data):
    try:
        process.stdin.write(data)
        process.stdin.flush()
        return True
    except (BrokenPipeError, ValueError) as error:
        print(f"[http] cannot write to QEMU stdin: {error}")
        return False


def http_request(host_port, method, path, timeout=10.0):
    """One HTTP/1.0 request, returning (status, headers, body)."""
    deadline = time.time() + timeout
    last_error = None
    while time.time() < deadline:
        try:
            with socket.create_connection(("127.0.0.1", host_port), timeout=3.0) as client:
                request = f"{method} {path} HTTP/1.0\r\nHost: test\r\n\r\n"
                client.sendall(request.encode())
                data = b""
                while True:
                    chunk = client.recv(4096)
                    if not chunk:
                        break
                    data += chunk
                head, _, body = data.partition(b"\r\n\r\n")
                status_line = head.split(b"\r\n", 1)[0].decode("latin1")
                status = int(status_line.split(" ")[1])
                return status, head.decode("latin1"), body
        except (OSError, ValueError) as error:
            last_error = error
            time.sleep(0.2)
    raise TimeoutError(f"no HTTP response on 127.0.0.1:{host_port}: {last_error}")


def header_value(headers, name):
    prefix = name.lower() + ":"
    for line in headers.split("\r\n")[1:]:
        if line.lower().startswith(prefix):
            return line.split(":", 1)[1].strip()
    return None


def print_tail(serial, count=TAIL_LINES):
    tail = serial.lines[-count:]
    print(f"[http] last {len(tail)} serial lines:")
    for line in tail:
        print(f"[http] | {line}")


def check(condition, message, serial):
    if condition:
        print(f"[http] ok: {message}")
        return 0
    print(f"[http] FAIL: {message}")
    print_tail(serial)
    return 1


def expect(host_port, method, path, status, body, serial):
    got_status, headers, got_body = http_request(host_port, method, path)
    if got_status != status:
        return check(False, f"{method} {path} status {got_status}, wanted {status}", serial)
    if body is not None and got_body != body:
        return check(
            False,
            f"{method} {path} body {len(got_body)} bytes, wanted {len(body)}",
            serial,
        )
    if status == 200:
        length = header_value(headers, "Content-Length")
        wanted = len(got_body) if method != "HEAD" else None
        if wanted is not None and length != str(wanted):
            return check(False, f"{method} {path} Content-Length {length}", serial)
    return check(True, f"{method} {path} -> {status}", serial)


def run_case(iso, cores, timeout, net_device, host_port):
    command = qemu_command(iso, cores, net_device, host_port)
    print(f"[http] core count {cores}")
    print(f"[http] {' '.join(command)}")
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
        if not send(process, b"run httpserver 8080\n"):
            print_tail(serial)
            return 1
        serial.wait_for(r"httpserver: serving", timeout=timeout, start=start)
        serial.wait_for(r"httpserver: press Ctrl\+C to stop", timeout=timeout, start=start)
        print(f"[http] core {cores}: server is ready")

        failures = 0
        index_body = read_fixture("index.html")
        hello_body = read_fixture("hello.txt")
        big_body = read_fixture("big.txt")
        child_body = open(
            os.path.join(ROOT, "samples/apps/base/child.ig"), "rb"
        ).read()

        failures += expect(host_port, "GET", "/system/site/hello.txt", 200, hello_body, serial)
        failures += expect(host_port, "GET", "/system/site/hello.txt?x=1", 200, hello_body, serial)
        failures += expect(host_port, "GET", "/system/site/", 200, index_body, serial)
        failures += expect(host_port, "GET", "/system/site", 200, index_body, serial)
        failures += expect(host_port, "GET", "/system/site/big.txt", 200, big_body, serial)
        failures += expect(host_port, "GET", "/base/child", 200, child_body, serial)
        failures += expect(host_port, "GET", "/system/site/../child", 403, None, serial)
        failures += expect(host_port, "GET", "/%2e%2e/home/readme.txt", 403, None, serial)
        failures += expect(host_port, "GET", "/~", 403, None, serial)
        failures += expect(host_port, "GET", "/%7e/home/readme.txt", 403, None, serial)
        failures += expect(host_port, "GET", "/system/site/missing.txt", 404, None, serial)
        failures += expect(host_port, "POST", "/system/site/hello.txt", 405, None, serial)

        status, headers, body = http_request(host_port, "HEAD", "/system/site/big.txt")
        failures += check(status == 200, "HEAD big.txt -> 200", serial)
        failures += check(
            header_value(headers, "Content-Length") == str(len(big_body)),
            "HEAD big.txt Content-Length",
            serial,
        )
        failures += check(body == b"", "HEAD big.txt has no body", serial)

        if not send(process, b"\x03"):
            print_tail(serial)
            return 1
        serial.wait_for(r"httpserver: stopped", timeout=timeout)
        serial.wait_for(r"(?:shell> |igis:/[^>]*> )", timeout=min(timeout, 20.0))
        print(f"[http] core {cores}: stopped cleanly")

        # The shell prepends its current directory to a bare `httpserver`, so
        # `/base` is the served root and the old image-root path is not found.
        if not send(process, b"cd /base\n"):
            print_tail(serial)
            return 1
        serial.wait_for(r"igis:/base> ", timeout=min(timeout, 20.0))
        start = len(serial.lines)
        if not send(process, b"httpserver 8080\n"):
            print_tail(serial)
            return 1
        serial.wait_for(
            r"httpserver: serving /base on port 8080", timeout=timeout, start=start
        )
        print(f"[http] core {cores}: bare server serves /base")
        failures += expect(host_port, "GET", "/child", 200, child_body, serial)
        failures += expect(host_port, "GET", "/base/child", 404, None, serial)

        if not send(process, b"\x03"):
            print_tail(serial)
            return 1
        serial.wait_for(r"httpserver: stopped", timeout=timeout)
        serial.wait_for(r"igis:/base> ", timeout=min(timeout, 20.0))
        print(f"[http] core {cores}: stopped cleanly again")

        if failures:
            print(f"[http] FAIL core {cores}: {failures} case(s) failed")
            return 1
        return 0
    except TimeoutError as error:
        print(f"[http] FAIL core {cores}: {error}")
        if process.poll() is not None:
            print(f"[http] QEMU exited with status {process.returncode}")
        print_tail(serial)
        return 1
    finally:
        stop_process(process)


def stop_process(process):
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
            "Boot the shell ISO with QEMU user networking, run `httpserver`, and "
            "check file, index, traversal, error and HEAD behavior."
        ),
        formatter_class=argparse.RawDescriptionHelpFormatter,
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
        default=120.0,
        help="seconds allowed for boot and for the whole probe (default: 120)",
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
        help=f"host port forwarded to guest {GUEST_PORT} (default: {DEFAULT_HOST_PORT})",
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
            print(f"[http] note: ISO does not exist yet: {iso}")
        for cores in cores_list:
            print(" ".join(qemu_command(iso, cores, args.net_device, args.host_port)))
        return 0

    if not os.path.exists(iso):
        sys.exit(f"missing ISO: {iso} (run `cargo xtask image --shell`)")

    probe = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    probe.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    try:
        probe.bind(("127.0.0.1", args.host_port))
    except OSError as error:
        sys.exit(f"host port {args.host_port} is already in use: {error}")
    finally:
        probe.close()

    status = 0
    for cores in cores_list:
        status |= run_case(iso, cores, args.timeout, args.net_device, args.host_port)

    if status == 0:
        print(
            f"[http] proof passed on all {len(cores_list)} core count(s) "
            f"({', '.join(str(cores) for cores in cores_list)})"
        )
    else:
        print("[http] FAIL: httpserver did not pass on every core count")
    return 0 if status == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
