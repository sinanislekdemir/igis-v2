# 0031: WAV playback and the music player

- **Status:** Implemented
- **Scope:** `samples/apps/lib/wav.ig`, `samples/apps/apps/accessories/music_player`,
  `samples/apps/apps/productivity/browser`, `samples/audio`, `xtask`, `scripts`,
  `langhost`
- **Supersedes:** none
- **Superseded by:** none
- **ADRs:** 0063, 0064
- **Updated:** 2026-11-28

## Context

Spec 0030 gives the system an HDA playback stream and the `syslib.sound_*`
builtins. The first application use is a player for the shipped public-domain
classical excerpt: a RIFF/WAVE file whose PCM data is streamed to the kernel
ring.

The constraints are the same as the rest of the application layer: no
allocation, interpreted source, a closed host interface, and no per-sample DSP
in `.ig` (spec 0003/0004, ADR 0002/0019). The easiest format is therefore
**uncompressed PCM WAV already at a supported rate** — the application moves
bytes and never decodes. Compressed formats and resampling are explicitly out of
scope; the kernel formats are 44.1 kHz and 48 kHz, 16-bit stereo (spec 0030 A8,
ADR 0064).

The shipped corpus is one 10 s public-domain Chopin excerpt, 44.1 kHz/16-bit/
stereo (spec 0031 W6); its provenance and license are in
`samples/audio/readme.txt`.

**Non-goals:** compressed audio (MP3/OGG/FLAC), resampling, playlists, seeking,
metadata display, recording, and a kernel-side file player.

## Requirements

### The WAV reader

- **W1 (MUST):** `lib.wav` parses RIFF/WAVE, walks the chunk list (8-byte
  header, `size`, a pad byte after an odd size), and accepts exactly one `fmt `
  chunk with format tag 1 (PCM), 16 bits, 2 channels, and a 44.1 kHz or 48 kHz
  rate, plus a `data` chunk. Any other tag, width, channel count, rate or a
  missing chunk is `Invalid`; a missing file is `NotFound`. `open` publishes the
  record only on `Ok`.
- **W2 (MUST):** The reader never holds the file. `open` records the absolute
  `data_offset` and `data_size` (clamped to the file); `read` returns one
  256-byte `core.fs_read` piece and clamps `got` to the data end. No allocation,
  no full-file buffer, no new builtin or opcode.
- **W3 (MUST):** The module has no `main`, no top-level `var` state and no stream
  endpoint (the `/lib` rules, spec 0018).

### The application

- **W4 (MUST):** `apps/accessories/music_player` accepts:
  - no argument → play the shipped sample at `audio/chopin-nocturne-21.wav`;
  - one path → play that file;
  - `check` → headless: open the sample, assert the format, play one second,
    drain and close, and print `music_player: ok`. On `langhost` (no `audio/`
    tree) it prints `music_player: host ok`; on a kernel without an HDA device it
    prints `music_player: no audio`.
- **W5 (MUST):** The window shows the file basename, a progress bar and the
  playing/paused/finished state. Space pauses/resumes (closing and reopening the
  stream, keeping the unplayed staging bytes), `+`/`=` and `-` change the
  volume, `M` mutes, Escape and the close box end the window. A finished file
  stops the stream and keeps the window open until closed. A failed open (no
  device, busy, unreadable) keeps the window open with a one-line reason
  instead of closing it.
- **W6 (MUST):** Audio is fed in a 16,384-byte staging buffer built from
  256-byte `core.fs_read` pieces; a full or final buffer goes to the blocking
  `syslib.sound_write`. Before a close the application waits for
  `syslib.sound_queued()` to reach zero (bounded, 20 × 20 ms) so the ring tail
  is not cut. The window is painted once per staging flush, not per read, and
  the read loop blocks for 1 ms per 4 KiB of input: filling the 32 KiB ring is
  otherwise a long continuous run that can exhaust the normal-task per-run
  budget (spec 0061) before the first write blocks.
- **W7 (MUST):** The browser maps a `.wav` name to
  `apps/accessories/music_player` and spawns it detached with the full path
  (spec 0029's open-with table).

### Corpus and proof

- **W8 (MUST):** `samples/audio/` holds the public-domain excerpt at 44.1 kHz/
  16-bit/stereo, staged read-only as `audio/`; `readme.txt` records the source
  URL, performer, license, excerpt range and the conversion command. The
  excerpt trims the recording's digital-silence lead-in.
- **W9 (MUST):** `scripts/audio_qmp.py` boots the `--shell` image with the HDA
  controller and a WAV backend, runs `music_player check`, quits QEMU cleanly and
  runs `scripts/audio_wav_check.py`; it passes on `-smp 4` and `-smp 1` and a
  silent capture fails.
- **W10 (MUST):** `langhost` compiles the player and `lib.wav` and prints
  `music_player: host ok`; a host test pins that.

## Design

```
fs_read 256 B  ->  staging bytes[16384]  ->  syslib.sound_write (blocking)
                                                    |
                                      kernel 32 KiB ring -> HDA -> device
```

The progress bar is `cursor / data_size`; the staging buffer is one 16 KiB
block because it is half the kernel ring: the first two writes fill the ring,
the third blocks until the device consumes a period, which paces the loop and
leaves the CPU to the scheduler.

## Failure modes

- **Malformed/unsupported WAV:** `music_player: cannot open`, no window, no
  device touched.
- **No device:** `sound_open` is `IoError`; the check prints `no audio`; the
  normal mode shows `No audio device` in the window and stays open until the
  user closes it.
- **Another task owns the stream:** `sound_open` is `Full`; the application
  reports `no audio` for now (one stream, spec 0030 B1).
- **Owner exit / close:** the kernel stops and zeroes the ring (spec 0030 B4).

## Known limitation

`mke2fs -d` writes an all-zero 1 KiB block as an ext2 **hole**, and `ext2w`'s
strict subset treats a zero block pointer as `Invalid` (spec 0025 B19). A WAV
whose data contains a full 1 KiB block of digital silence therefore fails its
first read at that block. The shipped excerpt trims the lead-in and contains no
such block, so the proof is green; the follow-up is to make `ext2w` read a hole
as zeros (and state it in spec 0025), or to avoid holes in the staged tree.
This is a filesystem limitation, not a WAV or player rule.

## Alternatives considered

- **Decode FLAC/MP3 in `.ig`:** per-sample interpreted work; the reason spec 0030
  accepts two native rates instead.
- **A kernel-side `sound_play_file(path)`:** removes the application from the
  data path but moves file policy into the kernel and opens a new ABI; 192 KB/s
  of copying does not justify it.
- **Resample in the application:** the same per-sample cost; the corpus is
  stored at a device rate instead.
- **`core.fs_read` directly into the ring:** the file API writes at the start of
  a caller buffer and the ring is kernel memory; the staging buffer is the
  smallest bridge.

## Changelog

| Date | Change |
|------|--------|
| 2026-11-28 | Initial spec and implementation: `lib/wav.ig`, the player, the browser mapping, the corpus and the `audio_qmp.py` proof. |
