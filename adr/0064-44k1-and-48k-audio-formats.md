# 0064: Accept 44.1 kHz as well as 48 kHz on the HDA playback stream

- **Status:** Accepted
- **Date:** 2026-11-28
- **Deciders:** project owner
- **Related specs:** 0030

## Context

ADR 0063 chose a single 48 kHz/16-bit/stereo stream as the smallest format set
every HDA codec supports. That is still the right default, but the music corpus
the owner wants is mostly 44.1 kHz. The two ways to play it with a 48 kHz-only
stream are bad:

- **Resample in the application**, sample by sample, in interpreted `.ig` code.
  That is exactly the per-sample cost the blocking, chunk-based design was meant
  to avoid, and it would run for the whole track.
- **Resample the files offline** when building the corpus. Cheap and fine for a
  fixed demo set, but every user-supplied file would need a host tool before it
  could play, which contradicts "the application is source and the filesystem is
  the user's".

The hardware supports both rates natively: the HDA format word selects
`44.1 kHz` with base bit 14 (`0x4011` for 16-bit stereo) and `48 kHz` with bit
14 clear (`0x0011`); `AC_PAR_PCM` bit 6 advertises 44.1 kHz on QEMU's codec and
on ordinary Intel/Realtek codecs. The 32 KiB ring, the 8-entry BDL and the
`SD_LPIB` accounting are all byte-based, so a rate change alters only period
duration (21.3 ms at 48 kHz, 23.2 ms at 44.1 kHz), not one line of accounting.

## Decision

`syslib.sound_open` accepts **44100** and **48000** with 16-bit stereo. The
driver stores the codec format word in the stream (instead of the compile-time
48 kHz constant), programs it on both the DAC and the stream descriptor, and
starts the stream at the selected rate. Any other rate is `Invalid`. There is no
resampling in the kernel and none in `.ig`.

The default, the boot self-test and the initial proof corpus stay 48 kHz; 44.1k
is an additional accepted format, not a replacement.

## Consequences

### Positive

- Most music plays as stored, with no host-side conversion and no per-sample
  work in the interpreted application.
- The change is one stored `u16`, one `SET_STREAM_FORMAT` command and one
  descriptor write; the ring, BDL, ownership and blocking rules are untouched.
- A future rate (32 kHz, 96 kHz, 88.2 kHz) is the same table entry plus codec
  capability discovery, not a redesign.

### Negative

- The driver now has two format constants to test instead of one; the self-test
  stays at 48 kHz, so 44.1k needs its own proof (the `audio_tone` application
  and a 44.1k WAV in the corpus).
- A file whose rate is neither accepted value still needs an offline conversion
  or a future resampler; the error is explicit rather than silent.

### Neutral / follow-up work

- `AC_PAR_PCM` range parsing would let the driver reject a rate the codec does
  not advertise instead of relying on the two known-good values. Deferred until
  real hardware is tested.
- A kernel-side resampler is explicitly not planned: the application layer owns
  file formats, and the kernel owns the device format.

## Alternatives considered

| Alternative | Why not |
|-------------|---------|
| Keep 48 kHz only and convert the corpus offline | Fine for a fixed demo, but user files would not play without a host tool. |
| Resample in `.ig` | Per-sample interpreted work for the whole track; defeats the chunked design. |
| Add a kernel resampler | Moves format policy into the kernel and adds a per-sample DSP path for no gain over two native rates. |
| Only 44.1 kHz | The reverse problem: 48 kHz material (and the existing self-test) would need conversion. |
| Add every rate in `AC_PAR_PCM` now | Range parsing and per-rate proof are more than the corpus needs; two rates cover the target material. |
