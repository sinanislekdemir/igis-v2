#!/usr/bin/env python3
"""Check a WAV captured from the HDA self-test or an audio application.

Spec 0030, V2. QEMU's `wav` backend writes the RIFF and `data` sizes when the
file is closed cleanly; a harness that kills QEMU with a timeout leaves both at
zero. This checker tolerates that: a zero `data` size means "the rest of the
file", and the RIFF size is not needed once the chunks are walked.

Usage: scripts/audio_wav_check.py build/audio.wav
Exit status 0 and one `audio wav: ok ...` line on success; a diagnostic and
status 1 otherwise.
"""

import math
import struct
import sys


def fail(message):
    print(f"audio wav: {message}", file=sys.stderr)
    raise SystemExit(1)


def main():
    if len(sys.argv) != 2:
        fail("usage: audio_wav_check.py <capture.wav>")
    path = sys.argv[1]
    try:
        blob = open(path, "rb").read()
    except OSError as error:
        fail(f"cannot read {path}: {error}")

    if len(blob) < 12 or blob[:4] != b"RIFF" or blob[8:12] != b"WAVE":
        fail("not a RIFF/WAVE file")

    fmt = None
    data = None
    position = 12
    while position + 8 <= len(blob):
        chunk = blob[position:position + 4]
        size = struct.unpack_from("<I", blob, position + 4)[0]
        body = position + 8
        if chunk == b"fmt ":
            if size < 16:
                fail("short fmt chunk")
            fmt = struct.unpack_from("<HHIIHH", blob, body)
        elif chunk == b"data":
            # A killed QEMU leaves the size at zero.
            length = size if size else len(blob) - body
            data = (body, min(length, len(blob) - body))
            break
        position = body + size + (size & 1)

    if fmt is None:
        fail("no fmt chunk")
    if data is None:
        fail("no data chunk")

    format_tag, channels, rate, _, _, bits = fmt
    if format_tag != 1:
        fail(f"expected PCM, found format tag {format_tag}")
    if channels != 2:
        fail(f"expected 2 channels, found {channels}")
    if rate != 48000:
        fail(f"expected 48000 Hz, found {rate}")
    if bits != 16:
        fail(f"expected 16-bit samples, found {bits}")

    offset, length = data
    frames = length // 4
    if frames < 1024:
        fail(f"too few frames: {frames}")
    samples = struct.unpack_from(f"<{frames * 2}h", blob, offset)
    left = samples[0::2]
    right = samples[1::2]

    rms = math.sqrt(sum(value * value for value in left) / frames)
    peak = max(abs(value) for value in left)
    crossings = sum(1 for a, b in zip(left, left[1:]) if (a < 0) != (b < 0))
    seconds = frames / rate
    hertz = crossings / 2 / seconds if seconds else 0.0

    if rms < 1000:
        fail(f"signal too quiet: rms {rms:.0f}, peak {peak}")
    same = "mono" if left == right else "stereo"
    print(
        "audio wav: ok "
        f"({seconds:.3f} s, 48 kHz {same} s16, rms {rms:.0f}, peak {peak}, "
        f"~{hertz:.0f} Hz)"
    )


if __name__ == "__main__":
    main()
