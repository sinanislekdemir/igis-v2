# 0063: Intel HDA with a cyclic BDL and polled period accounting for audio output

- **Status:** Accepted
- **Date:** 2026-11-27
- **Deciders:** project owner
- **Related specs:** 0030

## Context

The system has no audio device at all. The device must live within the existing
constraints: no allocation (ADR 0004/0007), one address space with an identity
map below 4 GiB (ADR 0001/0002), interpreted applications (ADR 0002), a closed
enumerated host interface (spec 0005), and a kernel that already polls its NIC
from the scheduler service opportunity (spec 0020, N4).

The candidate devices on the QEMU target are the PC speaker, AC'97, Sound
Blaster 16 and Intel HDA. The owner wants real music playback, not only beeps,
and the same code should have a plausible path to the Intel/AMD HDA controllers
found on real machines.

HDA brings its own complexity: a DMA command ring (CORB) and response ring
(RIRB), codec verb encoding, a widget graph, amplifier capabilities and a buffer
descriptor list. It also has two behaviours that are easy to get wrong:

- Only controller stream descriptors **4–7** are playback descriptors in QEMU's
  `intel-hda` model; a codec output stream tag is matched against those four.
  Programming descriptor 0 configures a valid stream that never receives data.
- The descriptor register layout is not the obvious one: `FORMAT` sits at
  `0x12` and `BDLPL/BDLPU` at `0x18/0x1C`, after `FIFOSIZE` at `0x10`.

## Decision

We will drive **Intel HDA** (PCI class `04.03`, ICH6/ICH9) as the first audio
device, with:

- capability discovery through `GET_PARAMETER` (node count, widget type, amp
  capability) rather than hardcoded node ids;
- one static, 128-byte-aligned 32 KiB ring split into 8 × 4 KiB periods, a
  BDL of 8 entries with `IOC` on each, and the engine looping the ring;
- fixed 48 kHz/16-bit/stereo (`0x0011`), because it is the one format every HDA
  codec and QEMU support without range negotiation;
- **polling** `SD_LPIB` from the existing scheduler service opportunity and
  accounting whole periods from the position delta, not an interrupt; a
  one-bit completion status is not a counter when QEMU pre-buffers 8 KiB;
- one machine-wide stream owned by the task that opened it, exposed as six
  `syslib.sound_*` builtins (spec 0030).

The device choice is recorded here; the behavioral requirements are in spec
0030.

## Consequences

### Positive

- Real PCM playback with a codec graph that generalizes to real Intel/AMD
  controllers: the driver never assumes QEMU's node ids.
- The cyclic ring streams without rewriting the BDL; a period boundary is the
  natural unit for accounting, silence and wake-ups.
- Polling reuses the proven service-opportunity pattern and avoids a new IOAPIC
  route, a lock-free IRQ hand-off and a second source of concurrency in the
  first slice.
- The 32 KiB ring is static `.bss`; no allocator, no fragmentation and no
  lifetime question. Physical addresses come from the identity map.

### Negative

- Polling adds one MMIO read per service opportunity (~1 kHz) even when idle;
  the cost is a single uncached read and is accepted for now.
- 48 kHz/16-bit/stereo only: 44.1 kHz material will need resampling in `.ig` or
  a later format extension.
- One stream only: a second sound source must mix in an application or wait for
  a later milestone.
- HDA is the most complex of the four candidates; the first bring-up is more
  code than AC'97 would have been.

### Neutral / follow-up work

- The output-descriptor rule (4–7) and the register offsets are recorded in
  spec 0030 and in the driver module docs; they are the two facts that cost the
  most debugging time when rediscovered.
- An interrupt path, additional formats and real-hardware validation are
  follow-ups, not prerequisites.
- The PC speaker can still become a separate `syslib.beep` without touching this
  decision.

## Alternatives considered

| Alternative | Why not |
|-------------|---------|
| PC speaker (PIT channel 2) | Monophonic square-wave beeps only; cannot play music. Optional later beep, not the product path. |
| AC'97 | Simpler register set, but a legacy standard with fixed legacy I/O and no useful format negotiation; no future on real machines. |
| Sound Blaster 16 | Requires the 8237 ISA DMA controller and IRQ 5; contradicts the move away from ISA-era DMA. |
| USB audio class | Needs a USB device stack beyond the current UHCI/HID tablet scope; larger than HDA for the same result. |
| virtio-sound | No such device on the QEMU target, and it would prove nothing about real hardware. |
| Interrupt-driven period completion | Adds an IOAPIC route and a lock-free hand-off for no measurable gain at a 1 kHz service period. Deferred. |
| One-entry BDL looping a small buffer | Cannot stream; replays the same buffer forever. The 8-period ring is the smallest streaming design. |
