# 0032: Visual regression uses exact host-side framebuffer goldens

- **Status:** Accepted
- **Date:** 2026-11-17
- **Deciders:** igis-v2 kernel/UI
- **Related specs:** 0009, 0011, 0015, 0016

## Context

M2.1.2 asked for a screenshot/debug dump so the UI can be verified without a display.
By M2.6.3 every windowed proof already runs headless and checks selected pixels with
`syslib.get_pixel`, and the QMP screenshot harnesses (`scripts/*_screenshot_qmp.py`)
already write PNGs by booting QEMU with no display and issuing `screendump`. What was
missing is a **comparison**: a frame that is deterministic, a stored reference, and a
compare that fails loudly when a pixel changes.

Three constraints shape the choice:

- The framebuffer is already fully observable from the host through QEMU's display
  surface. A kernel-side dump would be a second path to the same bytes.
- A kernel serial dump is expensive and fragile: it needs a new debug builtin, a bounded
  RLE/hex encoding, a host parser, and a large amount of serial traffic for a 1024×768
  frame. It would also have to be tested separately from the pixels it claims to prove.
- Most desktop frames are **not** deterministic: the boot desktop and the windowed shell
  show live clocks, the terminal and system-log windows contain boot text, and the
  pointer cursor is composited at present time. A golden of an arbitrary frame would be
  permanently flaky.

## Decision

We will prove the framebuffer with **host-side QMP `screendump` of one deterministic
specimen and an exact decoded-pixel golden**.

- `samples/apps/visual` opens a window whose frame is exactly the screen. The service
  chrome plus the specimen therefore cover both kernel text windows and every earlier
  boot line; nothing outside the specimen can reach the capture. The specimen has no
  clock, animation, event or host input, draws in a fixed order at fixed coordinates,
  and holds after `visual: ready` without changing a pixel. The pointer remains at its
  boot position.
- `scripts/visual_regression.py` boots `build/igis-shell.iso` headless, runs
  `visual shot`, waits for the complete present, captures through QMP, decodes to RGB,
  and compares **exactly** with `tests/golden/visual.png`. The manifest
  `tests/golden/visual.json` records the SHA-256 of the raw RGB bytes and one hash per
  64×64 tile; a mismatch reports the differing pixel count, bounding box, first pixel
  and differing tiles.
- The golden is the **decoded RGB image**, not the PNG encoder's bytes, so an encoder
  change does not invalidate it. It is tied to the bootloader's mode (1024×768 here) and
  to the QEMU display path, so updating it is a deliberate, reviewed `--update`, never a
  side effect of a normal run.
- The same golden must match on `-smp 4` and `-smp 1`. `--tamper` (corrupt one captured
  pixel in memory) and `--expect-mismatch` exist to prove that the comparator rejects a
  difference rather than passing trivially.
- M2.1.2 is resolved by this host-side dump. **No kernel serial dump or debug builtin is
  added.**

## Consequences

### Positive

- One small app, one harness and two committed files prove the whole framebuffer
  end-to-end: the interpreter's drawing calls, the shadow, the service chrome, `present`
  and the QEMU display conversion.
- The comparison is exact and fast, works without a display, and localizes a change to a
  64×64 tile. The specimen self-check (`visual: ok`) still catches a broken drawing
  primitive before the golden is consulted.
- No new opcode, builtin, `syslib` primitive, kernel table or serial protocol; the
  framebuffer has one producer and the host has one reader.

### Negative

- A golden is environment-scoped. A QEMU display-format change, a bootloader mode change
  or an intentional visual change requires a reviewed `--update`, and a stale golden fails
  the harness until it is updated. This is the intended cost of exact pixels.
- The specimen is a synthetic full-screen composition, not the live desktop. It pins the
  drawing surface and the service chrome; it does not prove that a live app's choice of
  pixels is unchanged.
- A full-screen window covers the kernel text windows during the shot. That is the
  mechanism that makes the frame deterministic, and it means the golden says nothing
  about console text.

## Alternatives considered

| Alternative | Why not |
|-------------|---------|
| A kernel `syslib` dump (RLE/hex) over serial | Needs a new debug builtin, an encoding and a host parser; large serial traffic; duplicates a path the host already sees; would itself need a test. The QMP dump is already the display surface. |
| Golden the live boot desktop | Not deterministic: live clock, boot text in the terminal/log windows, and a composited cursor. |
| Golden only the visual window's content rectangle | Weaker and unnecessary: the full-screen specimen already isolates the frame, and the full frame also pins the service chrome and the cursor position. |
| Perceptual/fuzzy image compare | Hides the small regressions the golden exists to catch; the specimen removes the noise a tolerance would tolerate. |
| Compare the PNG file bytes | Encoder settings and chunk order are incidental; the pixel image is the contract. |
| A host-side screenshot without a golden | Already existed (the M2.2–M2.6 screenshot harnesses); it cannot fail a regression. |
