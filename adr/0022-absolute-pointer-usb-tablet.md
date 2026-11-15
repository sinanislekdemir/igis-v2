# 0022: Use an absolute USB HID tablet for the pointer, not a scaled relative mouse

- **Status:** Accepted
- **Date:** 2026-11-16
- **Deciders:** igis-v2 design
- **Related specs:** `0002-architecture.md`, `0003-language.md`, `0005-host-interface.md`, `0009-graphics-and-windows.md`, `0010-input-and-events.md`

## Context

Q49 measured two independent defects in the relative PS/2 pointer:

1. **Burst loss.** A burst of 300 relative QMP events of +2 host pixels (600 px
   requested) moved the guest only +346 px. The PS/2 controller has a small queue and the
   kernel drains it in the service opportunity about once per millisecond; between drains
   the queue overflows.
2. **Host scale.** Relative motion is measured in host pixels. When the QEMU window is
   scaled, host pixels are not guest pixels, so the guest cursor never reaches the screen
   edges.

The decoder itself is correct: a single relative event moves the guest exactly 1:1. The
loss is a delivery and coordinate-space problem, not a decoding bug.

Three responses are possible:

- **Keep relative motion and scale it.** A host-to-guest scale factor could be estimated
  or configured, and a faster drain (aux IRQ12) could reduce burst loss. The scale remains
  a guess that changes with the window, and a dropped relative event still loses motion
  permanently.
- **Use a different virtual device class.** virtio-input/tablet has an absolute pointer,
  but it adds a second device protocol (virtqueue, config space) and is not the plain USB
  tablet a historical PC exposes. QEMU's `usb-tablet` is the device a real USB stack is
  expected to drive.
- **Use an absolute HID tablet.** An absolute report carries the current position, so a
  dropped or repeated report is idempotent: the next report restates the truth. The
  coordinate range is the device's own `0..=32767`, independent of the host window scale.
  Reaching the guest edges is an axis mapping, not an accumulator.

The absolute option implies a small USB stack: PCI discovery, a UHCI host controller, a
control pipe for enumeration, and one interrupt-IN endpoint for reports. QEMU's default
PC machine already provides the PIIX3 UHCI controller (`8086:7020` at `0:1.2`) and
`-usb -device usb-tablet` exposes the HID device.

## Decision

We will use QEMU's **USB HID tablet** as the primary pointer and keep the PS/2 auxiliary
device as the fallback. The kernel owns the whole path:

- scan PCI for the UHCI controller, enable I/O and bus mastering, take BAR4;
- reset the controller, install a static 4 KiB frame list and static queue head/TDs;
- enumerate the tablet at address 1, verify that its HID report is an absolute 16-bit X/Y
  tablet, configure it and `SET_IDLE(0)`;
- poll one self-linked interrupt-IN TD from the service opportunity, scale `0..=32767` to
  `0..=w-1`/`0..=h-1`, and set the pointer absolutely;
- publish `syslib.pointer_source()` (`0` none, `1` PS/2, `2` tablet) next to the pointer.

The driver starts in the kernel (`kernel/src/pci.rs`, `kernel/src/usb.rs`) because the
pointer state is machine-wide and the service opportunity already polls devices. ADR 0013
puts machine-specific code behind `arch`/`platform`; there is one ISA and one machine in
this milestone, so moving the module is deferred rather than duplicating the port I/O and
DMA layout behind an abstraction with one implementation.

The PS/2 relative path stays: it is the fallback on a machine without the tablet, and it
keeps `samples/apps/mouse` and the existing self-checks meaningful. When the tablet is
ready, PS/2 motion is decoded but dropped; the tablet is authoritative for position and
buttons.

## Consequences

### Positive

- A dropped report cannot lose motion: the next absolute report restates the position.
  The measured +346-of-600 px burst failure disappears.
- The position is in the device's own range, so host window scaling cannot keep the guest
  away from the edges.
- The same path is deterministic headless: QMP `input-send-event` with absolute values
  lands on exact guest pixels, so the proof needs no display or real mouse.
- The PS/2 decoder, fallback and debug injection are unchanged; a machine without USB
  loses nothing.
- The whole USB path allocates nothing and uses 4336 bytes of static `.bss` (4096 frame
  list + 240 QH/TD/buffers).

### Negative

- The kernel gains a PCI scan, a UHCI driver and a HID control path: more trusted code,
  and a schedule bug is a system bug.
- The control path is deliberately narrow: one full-speed HID tablet, one interrupt-IN
  endpoint, one data TD per control stage. The measured QEMU device returns a whole
  descriptor in one TD; a real device with an 8-byte control pipe would short-transfer and
  be refused (open question in spec 0010), not misparsed.
- The driver is x86-specific and sits outside `arch`/`platform` until a second machine
  exists (ADR 0013 debt, recorded here).
- Polling the endpoint adds a short controller walk per service opportunity; the UHCI
  frame timer makes a NAK cheap and the endpoint is not interrupt-driven in this slice.
- USB keyboards, hubs, mass storage and IRQ12 are not covered; IRQ12 remains M2.2.7.

### Neutral / follow-up work

- M2.2.7 drains the PS/2 aux device on IRQ12, so the relative fallback also stops losing
  bursts. The absolute tablet is the primary path regardless.
- M2.2.4's event queue will carry pointer events from the same state; `pointer_source`
  stays a low-level read.
- A second machine or architecture moves `pci`/`usb` behind `platform`/`arch` and adds the
  multi-packet control stages and USB keyboard if real hardware becomes a target.

## Alternatives considered

| Alternative | Why not |
|-------------|---------|
| PS/2 with a host-to-guest scale factor | The scale is window-dependent and unobservable from the guest; a dropped relative event still loses motion (Q49) |
| Aux IRQ12 alone, relative pointer | Fixes burst loss but not the host-pixel scale; the pointer still stops short of the edges on a scaled display |
| virtio-input / virtio-tablet | An absolute pointer with a different device protocol and queue, while the PC machine already has UHCI; it would not exercise the USB stack a historical machine exposes |
| A user-space USB driver over streams | The pointer is machine-wide and the kernel already owns the polling opportunity; a byte protocol and a per-report copy add no isolation (ADR 0021) |
| A hardware/emulator-side workaround (host mouse scaling) | Does not fix the guest for a scaled or bursty host, and is not under the system's control |
| No absolute pointer | Leaves two measured defects: lost motion in bursts and unreachable screen edges |
