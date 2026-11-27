#!/usr/bin/env python3
"""Prove the editor's Save File dialog end to end through the real keyboard path.

The shell-entry ISO boots headless with the home disk in a temporary snapshot.
The harness runs `editor`, sends Ctrl+N (unnamed New File), types `hello`,
sends Ctrl+S (which must open the Save File dialog because the document is
unnamed), types `saveasprobe` into the selected name field, presses Enter,
and quits with Ctrl+Q. It then runs `ls /home` and `cat /home/saveasprobe` and
requires a five-byte file containing `hello`.

This exercises `lib.file_dialog.save_file`, the save dialog's field, listing and
Save button/key path, and the editor's temp+rename write. The home disk is
snapshotted, so `build/home.img` is never modified.

Usage:
    python3 scripts/editor_save_as_qmp.py                # -smp 4 then -smp 1
    python3 scripts/editor_save_as_qmp.py --cores 1
"""

import argparse
import os
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
sys.dont_write_bytecode = True

from tablet_qmp import Qmp, Serial, apps_drive  # noqa: E402

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DEFAULT_ISO = os.path.join("build", "igis-shell.iso")
TARGET = "saveasprobe"
BODY = "hello"


def qemu_command(iso, cores, socket_path):
    return [
        "qemu-system-x86_64",
        "-cdrom",
        iso,
        "-drive",
        "file=build/home.img,format=raw,if=ide,index=0,snapshot=on",
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
        "-nic",
        "none",
        "-qmp",
        f"unix:{socket_path},server,nowait",
    ]


def send_key(qmp, name, hold=40):
    qmp.command(
        "send-key",
        {"keys": [{"type": "qcode", "data": name}], "hold-time": hold},
    )


def chord(qmp, *names):
    qmp.command(
        "send-key",
        {
            "keys": [{"type": "qcode", "data": name} for name in names],
            "hold-time": 80,
        },
    )


def type_text(qmp, text):
    for character in text:
        send_key(qmp, character)
        time.sleep(0.06)


def send_line(process, text):
    process.stdin.write((text + "\n").encode())
    process.stdin.flush()


def print_tail(serial, count=35):
    tail = serial.lines[-count:]
    print(f"[saveas] last {len(tail)} serial lines:")
    for line in tail:
        print(f"[saveas] | {line}")


def run_case(iso, cores, timeout):
    socket_path = os.path.join(
        tempfile.gettempdir(), f"igis-saveas-{os.getpid()}-{cores}.qmp"
    )
    try:
        os.unlink(socket_path)
    except FileNotFoundError:
        pass

    command = qemu_command(iso, cores, socket_path)
    print(f"[saveas] core count {cores}")
    process = subprocess.Popen(
        command,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    serial = Serial(process.stdout)
    qmp = None
    try:
        qmp = Qmp(socket_path, timeout=timeout)
        _, index = serial.wait_for(r"igis:/> ", timeout=timeout)

        send_line(process, f"rm /home/{TARGET}")
        _, index = serial.wait_for(r"igis:/> ", timeout=timeout, start=index)
        send_line(process, "run editor")
        serial.wait_for(
            r"apps/productivity/editor: spawned", timeout=timeout, start=index
        )
        # Interactive mode prints no ready line; give the first frame time.
        time.sleep(1.5)

        chord(qmp, "ctrl", "n")
        time.sleep(0.7)
        type_text(qmp, BODY)
        time.sleep(0.3)
        chord(qmp, "ctrl", "s")
        # The unnamed document must open the Save File dialog.
        time.sleep(1.5)
        type_text(qmp, TARGET)
        time.sleep(0.3)
        send_key(qmp, "ret")
        time.sleep(1.5)
        chord(qmp, "ctrl", "q")
        _, index = serial.wait_for(r"igis:/> ", timeout=timeout, start=index)

        send_line(process, "ls /home")
        _, index = serial.wait_for(r"igis:/> ", timeout=timeout, start=index)
        ls_text = "\n".join(serial.lines)
        send_line(process, f"cat /home/{TARGET}")
        _, index = serial.wait_for(r"igis:/> ", timeout=timeout, start=index)
        cat_text = "\n".join(serial.lines)

        listed = f"f {TARGET} {len(BODY)}" in ls_text
        catted = BODY in cat_text.split(f"cat /home/{TARGET}", 1)[-1]
        if not listed or not catted:
            print(
                f"[saveas] FAIL core {cores}: listed={listed} catted={catted}"
            )
            print_tail(serial)
            return 1
        print(f"[saveas] core {cores}: saved and read back {TARGET!r}")
        return 0
    except TimeoutError as error:
        print(f"[saveas] FAIL core {cores}: {error}")
        if process.poll() is not None:
            print(f"[saveas] QEMU exited with status {process.returncode}")
        print_tail(serial)
        return 1
    finally:
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    pass
        if qmp is not None:
            qmp.close()
        try:
            os.unlink(socket_path)
        except FileNotFoundError:
            pass


def main():
    parser = argparse.ArgumentParser(
        description=(
            "Boot the shell ISO, run the editor, create an unnamed document, "
            "save it through the Save File dialog and read it back from home."
        )
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
    args = parser.parse_args()

    cores_list = []
    for cores in args.cores:
        if cores < 1:
            parser.error("--cores values must be at least 1")
        if cores not in cores_list:
            cores_list.append(cores)

    iso = os.path.abspath(args.iso)
    if not os.path.exists(iso):
        sys.exit(f"missing ISO: {iso} (run `cargo xtask image --shell`)")
    if not os.path.exists(os.path.join(os.path.dirname(iso), "home.img")):
        sys.exit("missing home.img next to the ISO (run `cargo xtask image`)")

    status = 0
    for cores in cores_list:
        status |= run_case(iso, cores, args.timeout)

    if status == 0:
        print(
            f"[saveas] proof passed on all {len(cores_list)} core count(s) "
            f"({', '.join(str(cores) for cores in cores_list)})"
        )
    else:
        print("[saveas] FAIL: the Save File dialog did not pass on every core count")
    return 0 if status == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
