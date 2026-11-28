# 0030: Audio output — the Intel HDA stream and `syslib.sound`

- **Status:** Implemented
- **Scope:** `kernel` (platform/x86_64/hda, audio, pci, sched, main), `abi`,
  `compiler`, `interpreter`, `langhost`, `xtask`, `samples/tests`, `scripts`
- **Supersedes:** none
- **Superseded by:** none
- **ADRs:** 0063
- **Updated:** 2026-11-28

## Context

The system has no audio path: no device driver, no kernel audio service and no
application surface. The personal-system thesis wants sound, and the first
concrete use is a music tone/player and audible UI feedback.

The project constraints shape the design:

- **No allocation** (ADR 0004/0007). Stream rings, buffer descriptor lists and
  command rings are fixed `.bss` objects sized at compile time.
- **Ring 0, one address space** (ADR 0001). The driver is ordinary kernel code;
  the first 4 GiB are identity-mapped, so a static's physical address is
  `virtual - KERNEL_BASE` (the virtio/e1000 rule).
- **Applications are interpreted source** (ADR 0002/0019). Sample generation and
  file decoding belong in `.ig`; the wire/DMA work belongs in Rust.
- **The host interface is closed and enumerated** (spec 0005). Sound crosses it
  as a builtin with caller-owned buffers, never as a register or a DMA address.
- **One interpreter loop per core, tasks are migratable** (ADR 0011). The audio
  service is a machine-wide singleton behind one `SpinLock`; a blocked writer
  waits on a scheduler condition, not on a core.
- **QEMU/TCG is the deterministic test target.** The first proven device is
  QEMU's `ich9-intel-hda` with an `hda-duplex`/`hda-output` codec.
- **No capture in this milestone.** Playback only; the ADC path, microphone and
  HDMI/display audio are non-goals.

The owner's decisions (2026-11-27): playback plus volume/mute, a **blocking**
write model, and **discovery-based** codec handling (QEMU proven first, real
Intel/AMD HDA expected but not guaranteed).

**Non-goals:** recording, MIDI, USB audio, AC'97/SB16/PC-speaker drivers,
resampling, more than one stream, multiple codecs, jack detection, HDMI/DP audio,
power management/suspend, and interrupts (the first milestone polls).

## Requirements

### Device and bring-up

- **A1 (MUST):** The driver finds an HDA controller on PCI bus 0 as class
  `04.03` with vendor `8086` and device `2668` (ICH6) or `293E` (ICH9). It
  enables memory space and bus mastering and takes BAR0 as a 16 KiB MMIO
  window; a 64-bit BAR takes its upper half from BAR1. An absent device is not
  an error and MUST NOT prevent boot (spec 0019 H3 rule).
- **A2 (MUST):** Bring-up clears `GCTL.CRST`, waits at least 100 µs, sets
  `CRST`, and polls it back within a bounded spin. A controller that never
  reports ready is logged once and treated as absent.
- **A3 (MUST):** The driver waits for at least one codec in `STATESTS` before
  sending a command, and addresses codec 0.
- **A4 (MUST):** Commands travel through the CORB/RIRB rings: 256-entry CORB
  (1 KiB) and 256-entry RIRB (2 KiB), both 128-byte aligned; the read pointer is
  reset with `CORBRP = 0x8000`, the write pointer with `RIRBWP = 0x8000`, and
  both engines run (`CORBCTL.RUN`, `RIRBCTL.DMA_EN`). The CORB write pointer
  names the last entry written and the device reads from `read pointer + 1`, so
  the first command lands at index 1, not 0. A response is matched by polling
  `RIRBWP`; every send has a bounded timeout. The ring-level response interrupt
  is enabled and `RIRBSTS` is cleared after each response, because the
  controller stops processing the CORB once `RINTCNT` responses have arrived;
  `INTCTL` stays clear, so no PCI interrupt is raised.
- **A5 (MUST):** A command is encoded as
  `(cad << 28) | (nid << 20) | (verb << 8) | payload`. A verb whose top three
  bits of the verb field are `0b111` carries an 8-bit payload; every other verb
  carries a 16-bit payload (the QEMU/HDA split).
- **A6 (MUST):** The codec is discovered, never hardcoded: `GET_PARAMETER`
  (`0xF00`) parameter `0x04` returns the first node id in the low 16 bits and
  the child count in the high 16 bits; parameter `0x09` returns the widget
  capabilities with the type in bits `20..24`. The driver selects the first
  audio-output widget (type 0, DAC) of an audio function group and an
  output-capable pin complex (type 4, `PIN_CAP.OUT`) reachable from it.
- **A7 (MUST):** Playback uses a controller **output** stream descriptor in
  `SD4..=SD7` (QEMU matches the codec's output stream tag only against those
  descriptors). `SD4` is preferred; a controller with fewer output descriptors
  reports audio unavailable.
- **A8 (MUST):** The stream format is 16-bit, 2 channels, at 44.1 kHz
  (`0x4011`) or 48 kHz (`0x0011`), selected by `sound_open` and written to both
  the DAC and the stream descriptor. Any other rate, width or channel count is
  `Invalid`, not silently converted; the kernel never resamples.
- **A9 (MUST):** The buffer is one static, 128-byte-aligned 32 KiB ring split
  into 8 periods of 4 KiB. The BDL holds 8 entries
  `{ addr: u64, len: u32, flags: u32 }` with `IOC` (bit 0) on every entry;
  `CBL = 32768`, `LVI = 7`. The engine loops the ring, so streaming rewrites
  sample bytes in place and never rewrites the BDL.
- **A10 (MUST):** Position is observed by polling `SD_LPIB` from
  `audio::service()` once per scheduler service opportunity. The driver keeps
  the last observed position, takes the byte delta modulo `CBL`, advances the
  consumed position by `delta / 4096` whole periods, and leaves the remainder
  for the next poll; `SD_STS.BCIS` is cleared opportunistically but is **not**
  the counter, because the one-bit status cannot report two completions between
  polls (and QEMU pre-buffers 8 KiB at start). Each consumed period is zeroed
  so a later wrap plays silence rather than stale samples. No interrupts are
  used in this milestone.
- **A11 (MUST):** A device, codec, DAC or pin that is missing or rejects a
  command leaves audio unavailable: `audio::init` returns `None`, exactly one
  boot line reports it, and every sound builtin returns `IoError`. Nothing
  hangs and no other subsystem changes behavior.

### Kernel audio service

- **B1 (MUST):** One machine-wide stream exists behind one `SpinLock`. The
  first successful `sound_open` records the calling task as the owner. A second
  open by another task is `Full`; a repeat open by the owner is `Ok` and keeps
  the current position, volume and mute.
- **B2 (MUST):** `sound_write` copies the caller's whole buffer into the ring.
  The buffer length MUST be `1..=32768`; a larger buffer is
  `CapacityExceeded`, an empty one `Invalid`. If the ring has less free space
  than the buffer, the caller blocks until enough periods are consumed and the
  same instruction is retried; a wake is a hint and the retry re-checks.
- **B3 (MUST):** The kernel accounts queued bytes as
  `written - consumed` (both free-running byte counters, wrap-safe). Free space
  is `32768 - queued`. `consumed` advances only in whole periods, so the
  accounting is exact while the engine runs.
- **B4 (MUST):** `sound_close` stops the stream (`SD_CTL.RUN = 0`), zeroes the
  ring and the counters, and clears the owner. The task that owns a stream and
  exits closes it; the scheduler's teardown path does this without taking the
  interpreter lock.
- **B5 (MUST):** The stream starts (`RUN = 1`) on the first successful write
  that leaves at least one full period queued, after the DAC has its stream id
  and format and the pin is output-enabled. Later writes only fill the ring.
- **B6 (MUST):** `sound_volume(percent)` accepts `0..=100` and maps linearly to
  the DAC's output-amplifier gain range from `AMP_OUT_CAP` (offset in bits
  `0..7`, step count in bits `8..15`); `100` is the codec's maximum step. Both
  channels are set. `sound_mute(on)` accepts `0` or `1`; both channels are
  muted/unmuted. Volume and mute survive `sound_close`/`sound_open`.
- **B7 (MUST):** A codec whose DAC advertises no output amplifier accepts the
  volume/mute calls as `Ok` no-ops, so an application does not need to detect
  the codec model.

### Application surface

- **C1 (MUST):** The surface is six builtins, no opcode:

  | id | name | operands | result |
  |----|------|----------|--------|
  | 98 | `syslib.sound_open(rate, channels, bits)` | 3 | `Status` |
  | 99 | `syslib.sound_write(samples)` | 1 buffer | `Status` (blocks) |
  | 100 | `syslib.sound_close()` | 0 | `Status` |
  | 101 | `syslib.sound_volume(percent)` | 1 | `Status` |
  | 102 | `syslib.sound_mute(on)` | 1 | `Status` |
  | 103 | `syslib.sound_queued()` | 0 | `uint` |

  `syslib.sound_write` accepts a `string` or a `bytes` buffer, like
  `core.write`/`syslib.blit_rgb`.
- **C2 (MUST):** Owner checks: `sound_write` and `sound_close` are `Invalid`
  for a task that does not own the stream or when no stream is open.
  `sound_queued` has no owner check and returns `0` when no stream is open.
- **C3 (MUST):** `sound_open` validates the triplet against A8 before touching
  the device and pushes `Ok`, `Invalid`, `Full` or `IoError` (A11).
- **C4 (MUST):** A blocked `sound_write` reports `WaitKind::Audio`; the kernel
  maps it to `WaitFor::Audio` and `audio::service` calls `task::wake_audio()`
  after it consumes at least one period.
- **C5 (MUST):** The `Host` trait gains `sound_open`, `sound_write`,
  `sound_close`, `sound_volume`, `sound_mute` and `sound_queued` with defaults
  that return `Invalid`/`0`, so `langhost` and test hosts compile and report the
  disabled service without special cases.
- **C6 (MUST):** The compiler recognizes the six names with the same arity and
  operand-type diagnostics as the other `syslib` builtins. They are
  compiler-recognised host builtins; no `stdlib` declaration is needed (the
  table in C1 is the contract).

### Verification

- **V1 (MUST):** With `igis.audio_selftest=<rate>` on the multiboot2 command
  line (`1` or `48000` for 48 kHz, `44100` for 44.1 kHz), the kernel streams
  three rings (about half a second) of a 440 Hz square wave at that rate after
  bring-up, logs the codec vendor id, DAC node, pin node, stream descriptor,
  rate, period count and total underruns, and stops the stream. Without the flag,
  boot behavior is unchanged.
- **V2 (MUST):** `scripts/audio_qmp.py` boots with
  `-audiodev wav,id=snd0,path=build/audio.wav` and
  `-device ich9-intel-hda -device hda-output,audiodev=snd0` (the WAV backend is
  output-only, so the playback-only codec is the one under test), runs
  `apps/accessories/music_player check` (spec 0031, W4/W9), and asserts the
  captured WAV is 48 kHz, 16-bit, 2 channels with a non-trivial RMS. A silent
  run fails.
- **V3 (MUST):** Pure helpers (command encoding, format constant, volume to
  gain mapping) live in a host-testable crate and have unit tests; the kernel
  crate keeps no test harness.
- **V4 (MUST):** Boot without the audio device (`cargo xtask run`) still logs
  the absent path and passes the existing transcripts unchanged.

## Design

```
application  --syslib.sound_write-->  interpreter (Host)  --+--> kernel audio facade
                                                            |      (ring, owner, wait)
                                                            +--> platform/x86_64/hda
                                                                 (CORB/RIRB, codec,
                                                                  BDL, stream)
```

The driver is a platform module (`kernel/src/platform/x86_64/hda.rs`); the
facade is `kernel/src/audio/mod.rs`, mirroring `net/mod.rs`. The facade owns the
one `Sink`, enforces ownership, filters the builtin calls and calls into the
driver; the driver owns every register and the codec verb sequence.

### Invariants

- Exactly one playback stream exists. Ownership is a task id; teardown clears it.
- The ring is the only place sample bytes live. A write either copies the whole
  buffer into free space or changes nothing and blocks.
- `consumed` is monotonic modulo `2^64`; `queued` never exceeds the ring.
- The BDL is written once before `RUN` and never during playback.
- A period is zeroed only after it is consumed, so the application's unplayed
  data is never clobbered.
- No audio code takes the console lock; the only lock is the audio `SpinLock`,
  and `service()` releases it before calling `task::wake_audio()`.

### QEMU/HDA facts the implementation depends on

| Fact | Value |
|------|-------|
| SD descriptor offsets | `CTL 0x00`, `STS 0x03`, `LPIB 0x04`, `CBL 0x08`, `LVI 0x0C`, `FORMAT 0x12`, `BDLPL 0x18`, `BDLPU 0x1C` |
| SD base | `0x80 + index * 0x20`; playback descriptors are indices 4–7 |
| SD control bits | `SRST 0`, `RUN 1`, `IOCE 2`, `DEIE 3`, `FEIE 4`, stream tag `20..24` |
| SD status | `BCIS` bit 2, `FIFO_READY` bit 5 |
| CORB/RIRB | `CORBLBASE 0x40`, `CORBWP 0x48`, `CORBRP 0x4A`, `CORBCTL 0x4C`; `RIRBLBASE 0x50`, `RIRBWP 0x58`, `RIRBCTL 0x5C` |
| `GCTL` | `CRST` bit 0 |
| Verbs | `SET_STREAM_FORMAT 0x200`, `SET_AMP_GAIN_MUTE 0x300`, `SET_CHANNEL_STREAMID 0x706`, `SET_PIN_WIDGET_CONTROL 0x707`, `GET_AMP_GAIN_MUTE 0xB00`, `GET_PARAMETERS 0xF00` |
| Parameters | `NODE_COUNT 0x04`, `WIDGET_CAP 0x09`, `AMP_OUT_CAP 0x12`, `CONNLIST_LEN 0x0E` |
| Amp payload | output `1<<15`, left `1<<13`, right `1<<12`, index `8..12`, mute `1<<7`, gain `0..7` bits |
| QEMU codec | node 1 = AFG, 2 = DAC, 3 = line-out pin (duplex adds 4 = ADC, 5 = line-in) |
| QEMU amp | 74 steps, offset 74, 0.25 dB per step; unmuted by default |

## Interfaces

```rust
// kernel/src/platform/x86_64/hda.rs
pub struct Hda {
    mmio: u64,
    /// The 32 KiB sample ring (static, 128-byte aligned).
    ring: &'static mut [u8; RING_BYTES],
    /// Filled-in codec facts.
    dac: u8,
    pin: u8,
    stream: u8,          // descriptor index 4..=7
    amp: Option<AmpCaps>, // None => volume/mute are no-ops
}

impl Hda {
    pub fn init(controller: &pci::HdaController) -> Option<Hda>;
    pub fn start(&mut self) -> bool;
    pub fn stop(&mut self);
    pub fn period_completed(&mut self) -> bool;
    pub fn set_volume(&mut self, percent: u8);
    pub fn set_mute(&mut self, on: bool);
}

// kernel/src/audio/mod.rs
pub fn init();
pub fn service();
pub fn open(rate: u64, channels: u64, bits: u64, owner: u64) -> Status;
pub fn write(owner: u64, bytes: &[u8]) -> Result<(), Status>; // WouldBlock
pub fn close(owner: u64) -> Status;
pub fn volume(owner: u64, percent: u64) -> Status;
pub fn mute(owner: u64, on: u64) -> Status;
pub fn queued() -> u64;
```

The application buffer is resolved by the machine (`resolve_text`), so the host
receives bytes, not references.

## Failure modes

- **No controller / no codec / no DAC / no output pin:** `init` logs one line and
  returns `None`; every builtin is `IoError`; boot and every other sample are
  unaffected.
- **Command timeout:** treated as the same unavailable path for the command's
  stage; bring-up fails rather than using a partly configured codec.
- **Ring full:** the writer blocks; it never spins in the interpreter, so the
  normal-task watchdog is not involved (blocked time does not count).
- **Underrun:** zeroed periods play silence; `audio_selftest` logs the count, and
  the app can observe `sound_queued` dropping to `0`.
- **Owner exits without close:** teardown stops the stream and zeroes the ring.
- **Wrong format / wrong arity / wrong type:** `Invalid`/compiler diagnostic;
  nothing reaches the device.

## Alternatives considered

- **PC speaker (PIT channel 2):** trivial and always present, but monophonic
  beeps only; rejected as the product path, kept as an optional later beep.
- **AC'97:** simpler codec register set, but a dead 1997 standard with legacy
  fixed I/O and no format negotiation worth keeping.
- **SB16 / ISA DMA:** needs the 8237 controller and IRQ 5; rejected with the
  rest of ISA-era DMA.
- **Interrupt-driven periods:** lower poll cost, but adds an IOAPIC route and a
  lock-free hand-off for no benefit at 1 kHz service and 4 KiB periods. Deferred.
- **One BDL entry looping a small buffer:** simplest, but cannot stream; the
  8-period ring is the smallest design that streams without rewriting the BDL.

## Open questions

| # | Question | Trigger / owner |
|---|----------|-----------------|
| 1 | Add an interrupt path for period completion? | If service polling shows measurable cost, or if a future app needs < 1 ms latency. |
| 2 | Add 44.1 kHz and more channel counts? | When a real music file corpus needs it; requires `AC_PAR_PCM` range parsing. |
| 3 | A `WaitFor::Audio` deadline or timeout for a stuck writer? | If an app can block forever on a stopped device; `sound_close` by the owner is the current answer. |
| 4 | Real-hardware codec support (Realtek ALC, AMD)? | When the owner runs the image on metal; depends on amp/pin discovery only. |

## Changelog

| Date | Change |
|------|--------|
| 2026-11-27 | Initial spec. Owner decisions: playback + volume/mute, blocking write, discovery-based codec. |
| 2026-11-28 | A8 accepts **44.1 kHz** as well as 48 kHz (owner decision; ADR 0064): the codec format word is `0x4011` for 44.1k and `0x0011` for 48k, selected by `sound_open` and programmed on both the DAC and the stream descriptor. The ring and BDL are byte-based, so period timing changes but no accounting code does. No resampling in the kernel or in `.ig`. |
| 2026-11-28 | Implemented: builtins 98–103, ownership, blocking `sound_write` (`WaitKind::Audio`/`WaitFor::Audio`), close/teardown, volume/mute, `sound_queued`, and the compiler names. V2's proof is the `music_player check` (spec 0031) through `scripts/audio_qmp.py` (1.027 s captured, stereo, 4 and 1 cores); the boot self-test still covers both rates. C6 clarified: the six names are compiler-recognised, with no `stdlib` declaration. |
