# 0051: Acknowledge the NIC on its own INTx vector and keep the ring drain in the service opportunity

- **Status:** Accepted
- **Date:** 2026-11-19
- **Deciders:** igis-v2 kernel/network
- **Related specs:** 0020 (networking, N4/N19/N22), 0002 (architecture), 0005 (host
  interface), 0007 (scheduler)
- **Related ADRs:** 0023 (IRQ handler takes no lock), 0049 (virtio-net first device
  seam), 0011 (global claim loop), 0017 (kernel REPL service)

## Context

M10.1–M10.9 poll the NIC in the service opportunity, which the scheduler reaches
before every claim and in the idle loop. That works, but it costs a full device
register/ring check on every claim and gives no way to tell whether the device is
actually pushing data. M10.6 adds the one interrupt line spec 0020 N4 promises.

Two constraints dominate the design:

1. **A ring handler cannot take the network lock.** The NIC's state lives in one
   `SpinLock<NetState>`; the interrupt can arrive on a core that already holds it
   (a transmit or a service opportunity in progress). Walking the virtqueue used
   ring and calling `stack.receive` from the handler would need that lock, which is
   a guaranteed self-deadlock at ring 0. ADR 0023 reached the same conclusion for
   the PS/2 ring and solved it with a lock-free byte ring plus a bounded drain.
2. **The device rings are not SPSC-safe against the service path.** Unlike the
   i8042 byte stream, the virtio available ring is written by the driver
   (`publish`) as well as read, and the e1000 tail register moves on every
   receive. Two contexts touching them needs its own producer/consumer protocol
   and a second copy of every frame. A flag is enough: `service()` already runs on
   every claim, so a missed or coalesced interrupt costs at most one service
   period, and polling stays a correct second chance.

A third, empirical constraint surfaced while implementing it: legacy virtio-pci
raises its INTx line from `vdev->isr & 1` and the PCI layer only turns a *level
change* into an interrupt. The driver's boot-time test transmit can assert the
line before the I/O APIC entry is programmed, leaving the level latched high so
every later notification is a no-op. The handler's read of the ISR deasserts it,
so one read after the route is installed restores the edge behaviour.

## Decision

We will route the NIC's PCI INTx line (QEMU GSI 11 for 00:03.0 at both virtio-net
and e1000/e1000e) through the I/O APIC to the boot core as vector `0x2D`, and use a
bounded, lock-free handler:

- the handler sets one `AtomicBool` pending flag, increments a counter, performs
  **one** read-to-clear device register access (virtio `ISR` at `0x13`, e1000
  `ICR`) using only a base address kept in atomics, then EOIs the local APIC;
- it takes no `SpinLock`, prints nothing, allocates nothing and never calls
  `net::service`;
- `net::service()` clears the flag, drains up to `RX_DRAIN` frames and runs the
  stack timers exactly as before, so the polled path is unchanged code;
- the I/O APIC `init` already programs one redirection entry per call and splits
  its 2 MiB page only once, so IRQ12 (PS/2) and IRQ11 (NIC) coexist;
- the polled path stays the default when there is no route, when the boot asks
  for it, or when the device refuses to unmask. The selection is the multiboot2
  command line (`xtask run --net-irq poll` writes `igis.net_irq=poll`), which is
  a standard boot-loader channel and needs no second kernel image or debug
  builtin;
- `core.net_stats` (builtin 73) exposes the stack's four bounded counters.

## Consequences

- An interrupt is a latency hint, not a delivery guarantee: correctness never
  depends on it, and a spurious one costs one register read and an EOI.
- The device acknowledge lives in the handler, so a still-asserted level line is
  re-delivered instead of lost (the ADR 0023 level-trigger argument).
- The handler cannot see the frame; a service opportunity must follow. The
  scheduler's claim loop and idle loop guarantee one, which is the same
  requirement the polled path already had.
- `net::enable_irq` must clear a stale cause after the route is installed
  (legacy virtio's latched INTx level). This is device-specific and documented in
  the driver.
- e1000 must unmask its receive causes (`ICR.RXT0`/`RXO`/`RXDMTO`/`LSC`) and leave
  the transmit causes masked, or a send would interrupt for every frame. That
  required fixing the old `IMC = u32::MAX` at reset, which *unmasked* everything
  (the I/O APIC entry was masked, so polling hid it).
- No MSI/MSI-X. INTx keeps one vector, one EOI and no per-queue vector table; a
  device without a usable INTx line still runs at full speed.

## Alternatives considered

| Alternative | Why not |
|---|---|
| Drain the used ring in the handler | Needs the `NET` lock (self-deadlock at ring 0) or a second copy of the ring and frame buffers |
| A lock-free SPSC frame ring between handler and service | The virtio available ring is written by the driver too, and e1000's tail register moves on receive; a second protocol is more state to prove for no gain when `service()` already runs on every claim |
| Blocking `WaitFor::Net` wake from the handler | The wake path takes scheduler locks; the handler must not |
| MSI/MSI-X | Per-queue vectors, message programming and masking for no benefit at this traffic level; INTx is one entry |
| `xtask --net-irq poll` via a patched ISO / `-append` / a device property | No channel the kernel reads; the multiboot2 command line is already delivered to `kmain` and costs one tag walk |
| A debug builtin to disable the route at run time | Spec 0020 forbids new debug primitives, and a boot-time choice is deterministic per run |
| Program the route from inside the driver | The kernel keeps all interrupt routing in `main.rs`, visible in the boot log, next to the IRQ12 line |
