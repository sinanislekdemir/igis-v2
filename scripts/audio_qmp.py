#!/usr/bin/env python3
"""Play the shipped WAV sample and verify the captured audio.

This is a verification harness, not part of the kernel. It boots the `--shell`
ISO (the text shell, so no application competes for input), types

    run apps/accessories/music_player check

waits for `music_player: ok`, quits QEMU cleanly (so the WAV backend writes its
RIFF sizes), and runs `scripts/audio_wav_check.py` on `build/audio.wav`.

Usage:
    cargo xtask image --shell
    python3 scripts/audio_qmp.py
    python3 scripts/audio_qmp.py build/igis-shell.iso --cores 1
    python3 scripts/audio_qmp.py --wav /tmp/igis-audio.wav

The player's check mode parses the 44.1 kHz Chopin sample, opens the HDA stream
at 44.1 kHz, plays one second through the builtins and closes it. QEMU's WAV
backend is configured at 48 kHz, so the capture proves the codec resampled a
real 44.1 kHz stream rather than staying silent.
"""

import argparse
import os
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
# Importing the sibling harness must not leave a __pycache__ in the repo.
sys.dont_write_bytecode = True

from tablet_qmp import Qmp, Serial, apps_drive  # noqa: E402

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DEFAULT_ISO = os.path.join(ROOT, "build", "igis-shell.iso")
DEFAULT_HOME = os.path.join(ROOT, "build", "home.img")
DEFAULT_WAV = os.path.join(ROOT, "build", "audio.wav")
PROMPT_RE = r"(?:shell> |igis:/[^>]*> )"
COMMAND = "run apps/accessories/music_player check\n"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("iso", nargs="?", default=DEFAULT_ISO)
    parser.add_argument("--cores", default="4")
    parser.add_argument("--wav", default=DEFAULT_WAV)
    parser.add_argument("--timeout", type=float, default=180.0)
    args = parser.parse_args()

    iso = os.path.abspath(args.iso)
    if not os.path.exists(iso):
        sys.exit(f"missing ISO: {iso} (run `cargo xtask image --shell`)")
    if not os.path.exists(DEFAULT_HOME):
        sys.exit(f"missing home disk: {DEFAULT_HOME}")
    wav = os.path.abspath(args.wav)
    os.makedirs(os.path.dirname(wav) or ".", exist_ok=True)
    try:
        os.unlink(wav)
    except FileNotFoundError:
        pass

    socket_path = os.path.join(tempfile.gettempdir(), f"igis-audio-{os.getpid()}.qmp")
    try:
        os.unlink(socket_path)
    except FileNotFoundError:
        pass

    command = [
        "qemu-system-x86_64",
        "-cdrom",
        iso,
        *apps_drive(iso),
        "-drive",
        f"file={DEFAULT_HOME},format=raw,if=ide,index=0",
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
        "-nic",
        "none",
        "-qmp",
        f"unix:{socket_path},server,nowait",
        # QEMU's WAV backend is output-only and captures at 48 kHz; the HDA
        # codec is driven at 44.1 kHz by the sample.
        "-audiodev",
        f"wav,id=snd0,path={wav},out.frequency=48000,out.channels=2,out.format=s16",
        "-device",
        "ich9-intel-hda",
        "-device",
        "hda-output,audiodev=snd0",
    ]

    process = subprocess.Popen(
        command,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    serial = Serial(process.stdout)
    qmp = None
    ok = False
    try:
        qmp = Qmp(socket_path, timeout=args.timeout)
        serial.wait_for(PROMPT_RE, timeout=args.timeout)
        # One byte at a time, like the other harnesses: a burst can be dropped
        # or reordered by the serial poll under load.
        for byte in COMMAND.encode():
            process.stdin.write(bytes([byte]))
            process.stdin.flush()
            time.sleep(0.02)
        serial.wait_for(r"music_player: ok", timeout=args.timeout)
        ok = True
        # Let the close drain into the backend before the clean quit.
        time.sleep(0.5)
        try:
            qmp.command("quit")
        except RuntimeError:
            pass
    except TimeoutError as error:
        print(f"audio_qmp: FAIL: {error}")
        return 1
    finally:
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
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

    if not ok:
        print("audio_qmp: FAIL: no music_player: ok")
        return 1

    check = subprocess.run(
        [sys.executable, os.path.join(ROOT, "scripts", "audio_wav_check.py"), wav],
        text=True,
    )
    if check.returncode != 0:
        print("audio_qmp: FAIL: captured WAV did not verify")
        return 1
    print(f"audio_qmp: PASS {wav} ({os.path.getsize(wav)} bytes, {args.cores} core(s))")
    return 0


if __name__ == "__main__":
    sys.exit(main())
