# 0049: Start with virtio-net behind a device seam, not a virtio-specific stack

- **Status:** Accepted
- **Date:** 2026-11-19
- **Deciders:** igis-v2 kernel/drivers
- **Related specs:** 0020 (networking), 0002 (architecture), 0010 (PCI/UHCI precedent),
  0019 (ATA/`SectorDevice` precedent)
- **Related ADRs:** 0013 (x86_64-first), 0022 (UHCI tablet), 0023 (IRQ drain rule),
  0040 (`SectorDevice`), 0048 (kernel wire stack)

## Context

The first network milestone needs one working NIC on the deterministic QEMU target,
and the owner wants future drivers (e1000 and others) to be added without rewriting
the stack. The system has:

- an x86 port-I/O pair (`syslib.port_in8`/`out8`, `arch_x86_64::port`) used by the
  PS/2 controller, ATA and UHCI;
- an identity map of the first 4 GiB, with a 4 KiB split/uncached-PTE helper used by
  the I/O APIC;
- no allocator, so every queue and buffer must be a static `.bss` object;
- one UHCI driver already doing static DMA (`kernel/src/usb.rs`) with identity-mapped
  physical addresses, which is the precedent for a NIC;
- one I/O APIC route (IRQ12, vector `0x2C`) whose handler only drains into a lock-free
  ring (ADR 0023).

QEMU's user-mode network is the natural test backend: `10.0.2.0/24`, guest
`10.0.2.15`, gateway `10.0.2.2`, DNS `10.0.2.3`. The question is which emulated NIC to
drive first and how much of the device model to expose above the driver.

**Hardware reality.** `virtio-net` is the QEMU/KVM default and is also available on
other hypervisors, but VMware Workstation/Fusion/ESXi expose `e1000`, `e1000e` or
`VMXNET3`, not `virtio-net`. A VMware path therefore needs one of those drivers
later; the seam, not the first driver, determines whether that is cheap.

## Decision

We will define one `NetDevice` seam and implement **virtio-net (legacy/transitional,
I/O BAR0)** as the first driver. e1000/e1000e (and, if ever needed, VMXNET3) will be
added behind the same seam; no protocol code will know which device is present.

### Device choice

- Use the **transitional** virtio PCI device with legacy 0.9.5 registers reached
  through I/O BAR0. `xtask` requests it explicitly with
  `-device virtio-net-pci,netdev=n0,disable-modern=on`, so the device id and register
  layout are deterministic rather than dependent on a hypervisor default.
- Negotiate only the features the stack needs: `VIRTIO_NET_F_MAC` (and optionally
  `VIRTIO_NET_F_STATUS`). **No** checksum offload, GSO, mergeable RX buffers,
  multiqueue or MSI-X. Every frame carries the 10-byte legacy `virtio_net_hdr`; the
  stack writes a zero header and computes checksums itself.
- Use one RX and one TX virtqueue (queue 0/1), 32 RX buffers and 16 TX buffers of
  2 KiB, static descriptor/avail/used tables, page-aligned. Buffers live in `.bss`;
  their virtual addresses equal their physical addresses through the existing
  identity map. `compiler_fence` orders descriptor writes against the avail/used
  index updates; x86 DMA is coherent, so no cache flush is needed.
- Enable I/O space and bus mastering in the PCI command register. Read the MAC from
  the device configuration; if the feature is absent, use the fixed QEMU default and
  log the fallback.

### Interrupts and polling

- The driver is **polled first** (M10.1–M10.5): `net::service()` drains the used RX
  ring in the service opportunity, next to `mouse::poll` and the keyboard. This
  avoids adding interrupt surface before a frame has ever been received.
- M10.6 adds one PCI INTx line (QEMU GSI 11) routed through the I/O APIC to the boot
  core as vector `0x2D`, level-triggered. The handler only drains the device's used
  ring into the same lock-free SPSC ring the mouse uses and EOIs; protocol work stays
  in the service opportunity (ADR 0023). An absent I/O APIC or a failed route logs a
  line and keeps polling.
- No MSI/MSI-X in M10; INTx is enough for one device and one core.

### The seam

The stack talks to a `NetDevice` (MAC, link state, transmit one frame, receive one
frame, poll, stats). The kernel selects one concrete device at boot (an enum is
acceptable; a static trait object is not required). `net` never includes a virtio or
e1000 header.

### Future drivers

- **e1000/e1000e (M10.9):** MMIO BAR0 with uncached PTEs, TX/RX descriptor rings,
  INTx. This is the VMware path and a second proof that the seam is real.
- **VMXNET3:** only if a VMware target demands it; it is a larger paravirtual device
  with its own descriptor format.
- **RTL8139:** not planned; it is a legacy I/O+DMA device with no current target.

### What is rejected now

- **virtio modern (MMIO BAR4, queue notifications, packed rings):** more registers
  and a different feature negotiation for no gain on the first target. The
  transitional device is the compatibility path QEMU still provides.
- **A userspace driver:** no process boundary, no isolation benefit, and DMA would
  still cross the same host interface; it adds a liveness dependency.
- **A NIC abstraction with dynamic registration:** there is no allocator and one
  device; a boot-time selection is enough. The seam is the trait, not a registry.

## Consequences

### Positive

- The shortest path to a first frame on the primary target, using the existing port
  I/O, identity map and IRQ-drain patterns.
- The protocol crate and the socket unit never depend on virtio, so e1000/e1000e and
  future hardware reuse them unchanged.
- Static DMA keeps the no-allocation rule and makes the memory budget visible at
  boot; the same model as UHCI and ATA.
- Polling first keeps the first slices small; the interrupt is an optimization with a
  proven fallback.

### Negative

- Legacy virtio is a compatibility interface; a future hypervisor may expose only
  modern virtio, which will need a second device path.
- No offloads means every checksum is computed in software, and the fixed 2 KiB
  buffers cap the MTU path (1500 is fine; jumbo frames are out of scope).
- One queue pair and one device means no multiqueue, no RSS and no NIC teaming.
- The PCI code grows beyond the UHCI-only scan; it stays x86/machine-specific until
  the `platform` split (ADR 0013 debt, already recorded for USB).
- INTx is shared and level-triggered; a stuck line or a missed EOI would hang the
  boot core, so the handler must stay minimal and the poll path must remain.

### Neutral / follow-up work

- `xtask` gains `--no-net` and `--net-device virtio|e1000`; the default is one
  user-mode virtio NIC.
- Boot logs the device id, MAC, link state and the static DMA cost; a missing device
  is a normal line, not an error.
- M10.9 adds e1000/e1000e and re-runs the same `net_probe` suite to prove the seam.
- The driver modules remain in `kernel/src/net` until a second machine exists, then
  move behind `platform`/`drivers`, like `pci.rs`/`usb.rs`.

## Alternatives considered

| Alternative | Why not |
|---|---|
| e1000 first | Closer to VMware, but needs MMIO attributes and descriptor-ring work before the first frame; virtio legacy is simpler on the primary QEMU target, and e1000 follows in M10.9. |
| RTL8139 first | Legacy I/O+DMA and simple registers, but no current hypervisor target and no path to VMware; it would be throwaway work. |
| virtio modern first | More register surface and a different negotiation; no benefit for one queue pair on QEMU. |
| Use QEMU's default NIC without specifying a model | The default varies by QEMU version/machine type; an explicit device keeps the test deterministic. |
| A userspace driver task | No isolation benefit at ring 0, adds a hop and a liveness dependency, and still needs DMA/physical addresses. |
| Poll only, forever | Works for proofs but adds latency under load; M10.6 adds INTx with polling as the fallback, matching the mouse precedent. |
