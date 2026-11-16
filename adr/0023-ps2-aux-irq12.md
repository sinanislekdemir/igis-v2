# 0023: Drain PS/2 auxiliary data on IRQ12 through the I/O APIC, into a lock-free ring

- **Status:** Accepted
- **Date:** 2026-11-16
- **Deciders:** igis-v2 design
- **Related specs:** `0002-architecture.md`, `0007-scheduler.md`, `0010-input-and-events.md`

## Context

The PS/2 auxiliary device has been read by the service opportunity since M2.2.0
(`mouse::poll` from `sched::service` and `sched::idle`). That works, but it has three
defects:

1. **Latency and buffering.** The controller has a one-byte output buffer and the device
   a small queue. Between service opportunities a burst arrives into the emulated
   controller; Q49 measured 300 × +2 host pixels landing as +346 guest pixels through a
   host window. The absolute USB tablet (ADR 0022) made the PS/2 path a fallback, but a
   machine without USB still relies on it.
2. **A multi-reader race.** `mouse::poll` can run on two cores at once. The status read
   and the data-port read are not atomic, and the decoder lock is taken only after the
   byte has been read, so two cores can read two bytes and push them in the opposite
   order. That desynchronises the 3-byte packet stream and silently drops motion.
3. **No interrupt plumbing exists.** The legacy PICs are masked at boot and only the
   local APIC timer, the wake IPI and the spurious vector have IDT gates.

The constraints are severe: handlers must be short (`specs/0002`), all code runs at ring 0
in one address space, and there is no allocator. A handler that took the decoder lock
could interrupt a core already holding it (in `mouse::poll`) and deadlock the machine; a
handler that took the graphics lock could interrupt a task in the middle of `present`.

## Decision

Route **IRQ12 through the I/O APIC** to the boot core and split the path into a short
handler and a bottom half:

- `arch/x86_64/src/ioapic.rs` maps the conventional MMIO window at `0xFEC00000`
  (`IOREGSEL` 0x00, `IOWIN` 0x10). `boot.s` identity-maps the first 4 GiB with cacheable
  2 MiB pages, so the containing 2 MiB page is split once and the register page is marked
  `PWT | PCD`; CR3 is reloaded before the first register access. The IOAPIC version is
  probed (`0xFFFFFFFF`, 0, or fewer than 12 redirections means absent) and redirection
  entry 12 is programmed **fixed delivery, physical destination, active-high,
  level-triggered, unmasked**, destination = the boot core's APIC id, vector `0x2C`.
- `arch/x86_64/src/interrupts.s` gains an `irq_aux` stub in the same shape as
  `irq_timer`/`irq_wake`, and `idt::init` installs its gate before the entry is unmasked.
- `kernel/src/mouse.rs` owns a **single-producer/single-consumer byte ring**: 1024
  `AtomicU8` slots with an atomic head (handler-only) and tail (consumer-only, under the
  decoder lock). The handler drains at most 64 bytes whose status says auxiliary, pushes
  them, counts the interrupts and bytes, and EOIs the **local APIC only**. It prints
  nothing and takes no kernel lock.
- `mouse::poll` drains the ring under the existing decoder lock and applies packets. The
  old direct controller drain stays as a **bounded backstop** when the I/O APIC, the local
  APIC or the controller configuration write is unavailable. The two modes are exclusive:
  while IRQ12 is active the handler is the only aux reader, which also removes the
  multi-reader race.
- The controller's configuration bit 1 is set in a separate `mouse::enable_irq` step after
  the IDT and APIC exist. Reporting is stopped (`0xF5`), the configuration is re-read, and
  reporting is re-enabled (`0xF4`) **before** `config | 0x02` is written, because once bit
  1 is set the handler steals the `0xF4` ACK. The value is written to the data port; the
  previous read-modify-write wrote it to the command port, so bit 1 had never persisted.

## Consequences

### Positive

- The controller buffer is emptied on arrival instead of once per service opportunity,
  and the auxiliary line's bytes are never read by two cores, so the packet stream cannot
  desynchronise.
- The handler is safe under every lock the kernel holds: it touches only port I/O, two
  atomics and the LAPIC EOI.
- Level triggering makes the 64-byte bound safe. The i8042 holds its line high while it
  still has buffered data; an edge entry would stall after a partial drain, while a level
  entry re-delivers after the EOI.
- The polling path is untouched as a fallback, and an absent IOAPIC is a boot log line,
  not a hang. `samples/apps/mouse`, `events`, `desktop` and the absolute tablet path are
  unaffected.
- No allocation, no new opcode, no language surface: the ring is 1 KiB of `.bss`, the
  IOAPIC table 4 KiB, and the two new registers are written once at boot.

### Negative

- The kernel now owns an I/O APIC driver: one MMIO window, one redirection entry, one
  vector and one more way to lose an interrupt if it is misprogrammed.
- The handler is the single producer, so IRQ12 is pinned to the boot core. That is fine
  for an input device at 100 reports/s, but a future per-core or per-device IRQ policy
  would need a different queue design.
- The device/controller bring-up is now two-phase and its order is load-bearing (gate,
  route, arm; reporting before bit 1). The failure log names the step, and an absent path
  falls back to polling.
- The `0xFEC00000` page is mapped uncached but the rest of that 2 MiB frame is also split
  into 4 KiB entries, costing one table page for the machine's lifetime. The guard table
  cache is not involved.
- An emulated PS/2 model may retain deltas (QEMU 10.2.2 accumulates them in `mouse_dx`),
  so the headless burst harness cannot demonstrate the historical host-window loss. It
  still proves one-for-one delivery and that IRQ12 (not polling) drained the bytes.

### Neutral / follow-up work

- IRQ1 for the keyboard, USB endpoint interrupts and a generic IRQ vector allocator are
  not part of this ticket; the keyboard stays polled and `core.read_line` is unchanged.
- ACPI/MADT parsing is still absent: the IOAPIC base and the ISA GSI number are assumed
  to be the PC defaults, which is the only machine this milestone targets (ADR 0013).
- `scripts/mouse_burst_qmp.py` is the permanent relative-burst regression; the absolute
  tablet stays the primary pointer (ADR 0022).

## Alternatives considered

| Alternative | Why not |
|-------------|---------|
| Keep 1 kHz polling and enlarge the drain bound | Removes neither the service-opportunity latency nor the multi-reader race; the status/data read is still not atomic across cores |
| Decode and move the pointer in the interrupt handler | The decoder and graphics locks may already be held by the interrupted code, which is a self-deadlock at ring 0; the handler must not block |
| A `SpinLock` around the handler's queue | Same problem: the handler can interrupt a core holding that lock (the decoder side takes it) |
| An edge-triggered IRQ12 entry | The i8042 keeps its line high while more data is buffered; a bounded drain would leave it high with no new edge |
| Route IRQ12 through the legacy 8259 PIC | The PICs are masked at boot and the kernel already owns the local APIC; adding PIC EOI/remap plumbing duplicates the interrupt path |
| Poll the PS/2 controller from the APIC timer interrupt | Still a second reader racing `mouse::poll`, and it adds controller work to the clock tick |
| A virtio input device / USB-only pointer | A different device protocol; the PS/2 fallback must work on a machine with no USB (ADR 0022) |
| ACPI/MADT IOAPIC discovery now | One PC machine is the target; the conventional base and GSI are sufficient until a second machine exists (ADR 0013) |
| Decode the ring on every core with per-core decoders | The pointer state is machine-wide and report order matters; one decoder under one lock is the simpler correctness argument |
