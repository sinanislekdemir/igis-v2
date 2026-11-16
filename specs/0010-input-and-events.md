# 0010: Input, the pointer and events

- **Status:** Accepted
- **Scope:** `kernel` (`event`, `mouse`, `usb`, `pci`, `gfx`, `console`, `sched`,
  `task`), `arch/x86_64` (i8042, keyboard, ports), `abi`, `compiler`, `interpreter`,
  `syslib`; M2.2 of the desktop milestone
- **Supersedes:** none
- **Superseded by:** none
- **ADRs:** 0021, 0022, 0023
- **Updated:** 2026-11-16

## Context

The desktop (M2) has one image — the shadow screen of spec 0009 — and applications draw
into it through `syslib`. What it does not have is input. A desktop needs a pointer that
moves and clicks, a cursor the user can see, and eventually a keyboard event stream that a
window manager can route. Today the only input path is the kernel's staged line discipline:
the service opportunity polls COM1 and the PS/2 keyboard into a byte FIFO, and
`core.read_line` blocks the shell until a line is complete (spec 0005, Q44).

This spec covers the first half of that work:

- the **PS/2 auxiliary device** (mouse) as the pointer, drained by the kernel (IRQ12,
  with a polling backstop since M2.2.7);
- **pointer state** (`x`, `y`, buttons) as one machine-wide value with 1:1 motion;
- the **software cursor** as a compositor overlay, drawn at present time and never stored
  in the shadow;
- the **debug injection** primitive that makes all of it testable without a display or a
  real mouse;
- forward references for the two slices that follow: keyboard **key events** (M2.2.3) and
  the **event queue** with `next_event`/`poll_event` (M2.2.4).

M2.2.6 adds a second pointer source, because relative PS/2 motion has two measured
host-side failure modes (Q49): QEMU's PS/2 queue overflows in bursts between the kernel's
1 ms drains, and relative motion is measured in host pixels, so a scaled display never
reaches the guest edges. An **absolute** USB HID tablet (`usb-tablet`) reports the current
position, so a dropped report is harmless and the position is independent of host scaling.
The PS/2 device stays as the fallback for a machine with no USB tablet.

M2.2.7 completes that fallback: IRQ12 is routed through the I/O APIC to the boot core and
the handler drains the controller into a lock-free byte ring, with the 1 kHz poll kept as
the backstop for a machine where the route cannot be set up. This removes the service
opportunity's drain latency and, more importantly, makes the interrupt handler the only
reader of aux bytes, so two cores can no longer take a byte out of order. The historical
Q49 figure (346 of 600 px) was measured through a host window; the headless QMP burst
harness on QEMU 10.2.2 reports 600/600 on both the polled and the IRQ build, because its
PS/2 model accumulates deltas and defers packets instead of dropping them. The harness is
kept as the regression that the IRQ path is one-for-one and that the ring reports the
interrupts it drained.

Non-goals for this slice (deferred or rejected):

- no event queue, no `Event` record, no `next_event`/`poll_event`, no `WaitFor::Input`
  *in the original M2.2.0 slice*; M2.2.3/M2.2.4 add them below (I18/I19).
- no keyboard key codes, modifiers or arrows in the language *in the original slice*: the
  console keeps its byte stream and `core.read_line` for the shell. M2.2.3 adds the key
  event layer alongside it (I18).
- no pointer acceleration, no wheel, no double-click detection, no pointer grab;
- no per-application cursor and no hardware cursor sprite;
- no window focus or routing: until M2.3, input is machine-wide and no application "owns"
  a window;
- no new bytecode opcode: the input surface is host builtins behind `BUILTIN`.
- no USB keyboard, mass storage or hub; the UHCI driver serves the interrupt-IN endpoint
  of one full-speed HID tablet. IRQ12 is **not** deferred any more: M2.2.7 implements it
  for the PS/2 auxiliary device (I32).
- no real-hardware control-pipe packetisation: the measured QEMU device returns a whole
  descriptor in one data TD. A device with a smaller control pipe would short-transfer
  and be refused as a descriptor error, never guessed at.
- no ACPI/MADT parsing: the I/O APIC is assumed at the conventional `0xFEC00000` and the
  legacy ISA IRQ number is the redirection entry (12), as on the PC machine this milestone
  targets.

## Requirements

Coordinates are screen pixels, origin top-left, `x` right and `y` down, as in spec 0009.
Buttons are a bit mask: bit 0 left, bit 1 right, bit 2 middle.

### The device

- **I1 (MUST):** The kernel initialises the i8042 auxiliary device at boot with a bounded
  wait on the controller's input and output buffer bits: enable the aux interface
  (`0xA8`), read the controller configuration and write it back with the aux interrupt
  bit clear, then send `0xF6` (defaults), `0xF3 100` (100 reports/s), `0xE8 3`
  (4 counts/mm), `0xE6` (1:1 scaling) and `0xF4` (enable reporting) through the `0xD4`
  prefix, waiting for each ACK. The configuration value is written to the **data** port
  after the `0x60` command; a value written to the command port is a command, not
  configuration. Once the IDT gate and the I/O APIC route exist (I32), a second bounded
  step stops reporting, re-reads the configuration, re-enables reporting, and writes the
  configuration back with the aux interrupt bit set; the keyboard translation and clock
  bits are preserved in every read-modify-write. Every step has a timeout; a missing
  controller or device is a boot log line, not a hang.
- **I2 (MUST):** Report decoding is a pure 3-byte state machine. Byte 0 carries the three
  buttons in bits 0–2, bit 3 is always 1 (sync), bit 4 is the X sign, bit 5 the Y sign,
  bit 6 the X overflow and bit 7 the Y overflow. A first byte without bit 3 is a desync:
  it is dropped and assembly restarts. A packet with either overflow bit set is ignored
  whole. The deltas are 9-bit two's-complement (the sign bit from byte 0 plus the 8-bit
  byte), so the range is `-256..=255`.
- **I3 (MUST):** The Y axis is inverted: a positive packet delta means "up", so the screen
  `y` decreases. Motion is 1:1 in pixels (acceleration is future work, see the open
  questions).
- **I4 (MUST):** Pointer state is machine-wide: one `x`, one `y` and one button mask for
  the whole system. `x` is clamped to `0..=width-1` and `y` to `0..=height-1`; a delta
  that would leave the screen simply stops at the edge. A packet with no movement and no
  button change updates nothing.
- **I5 (MUST):** The pointer state is written only by the kernel decoder and by
  `syslib.inject_pointer` (I9). An application cannot set the position or the buttons
  directly, and cannot read raw device state (I12).

### The cursor

- **I6 (MUST):** The cursor is a 16×16 monochrome arrow with a black outline, a white
  fill and hotspot `(0, 0)`, defined in the kernel. There is exactly one cursor and no
  application chooses its shape.
- **I7 (MUST):** The cursor is **composited at present time and never stored in the
  shadow**. `present(x, y, w, h)` copies the shadow rectangle and then overwrites the
  pixels covered by the cursor rectangle (black outline, white fill, transparent pixels
  leave the shadow pixel alone), in that order. `get_pixel` therefore always returns the
  uncomposited shadow pixel, which is what makes a headless self-check meaningful.
- **I8 (MUST):** Moving the pointer re-presents only the old and the new cursor
  rectangles, not the screen. The old rectangle comes back from the shadow (with any
  cursor overlap at the new position), the new one gets the overlay. The console
  presents each glyph cell through the same path, so a cell under the cursor is presented
  with the cursor on top of the text and no cell is ever left with a stale cursor.
- **I9 (MUST):** The cursor is visible from the moment the pointer device initialises
  successfully (or the first injection) and is hidden when the device never answered. A
  machine without a pointer device shows no cursor.

### The `syslib` surface

- **I10 (MUST):** The pointer surface is exactly four host builtins:

  ```text
  syslib.pointer_x() -> int
  syslib.pointer_y() -> int
  syslib.pointer_buttons() -> uint      // bit 0 left, bit 1 right, bit 2 middle
  syslib.inject_pointer(dx: int, dy: int, buttons: uint) -> Status
  ```

  `inject_pointer` applies the deltas with the same clamping and the same Y direction as a
  real packet (`dy` positive moves down), sets the button mask, and repaints the cursor.
  A `buttons` value above `0b111` is `Status.Invalid` and changes nothing.
- **I11 (MUST):** The four primitives are host builtins with safe defaults: a pointer at
  `(0, 0)`, no buttons, and `Status.Invalid` for `inject_pointer`. `langhost`, host tests
  and a kernel without graphics therefore agree, exactly as the drawing primitives do
  (spec 0009, G15/G16).
- **I12 (MUST):** `inject_pointer` is **debug-only**. It exists so headless verification
  can move the pointer and set buttons deterministically; it is documented as a test hook,
  not as a device abstraction. It does not grant port access, and it is the only way an
  application can change the pointer or button state. M2.2.4 will add `inject_event` for
  keyboard and queue tests.
- **I13 (MUST):** A task cannot read raw device state beyond this documented surface. The
  i8042 data and status ports are not exposed as input; an application may still touch
  them through `syslib.port_in8`/`port_out8` (there is no capability model, H4), but that
  is the hardware escape hatch, not an input API, and the kernel does not promise what a
  competing reader sees.

### The auxiliary interrupt and ownership (M2.2.7)

- **I14 (MUST):** The PS/2 auxiliary device is **interrupt-driven**: IRQ12 is routed
  through the I/O APIC to the boot core on vector `0x2C`, and the handler drains the
  controller's output buffer into a lock-free byte ring. Decoding and the pointer update
  stay in the service opportunity (`mouse::poll`), which drains that ring before every
  scheduler claim and in the idle loop. Polling the controller directly is **kept as a
  bounded backstop** for a machine where the I/O APIC or the local APIC is absent, or
  where the controller refuses the configuration write; in that mode `mouse::poll` drains
  the controller itself. The two modes are exclusive: while IRQ12 is active the handler is
  the only reader of aux bytes and the direct drain is not executed, so no byte can be
  consumed twice or taken out of order.
- **I15 (MUST):** The keyboard path and the mouse path never consume each other's bytes.
  The controller status bit 5 says which device a byte came from. With IRQ12 active the
  handler reads only bytes whose status says auxiliary and the keyboard path reads only
  bytes whose status says keyboard; the controller's one-byte output buffer makes those
  conditions mutually exclusive, so neither consumes the other's byte. In the polling
  backstop the same rule applies: the keyboard path stops at an aux byte and the mouse
  path stops at a keyboard byte, and both are polled in the same opportunity.
- **I16 (MUST):** The decoder and the pointer update are serialised by a kernel lock, so
  two cores draining the ring cannot interleave packet assembly or tear the position.
  Presenting is serialised by the graphics lock (spec 0009).
- **I17 (MUST):** Pointer and cursor work allocates nothing and keeps no per-application
  state: one static decoder, one static pointer, one static cursor table, one static byte
  ring.
- **I32 (MUST):** The interrupt mechanism is bounded and allocation-free:

  - the I/O APIC is programmed at the conventional MMIO window `0xFEC00000`
    (`IOREGSEL` 0x00, `IOWIN` 0x10) and the containing 2 MiB identity-mapped page is
    marked uncacheable (`PWT | PCD`) before the registers are touched;
  - the redirection entry for IRQ12 is fixed delivery, physical destination, active-high,
    **level-triggered** and unmasked, with the boot core's APIC id as the destination.
    Level triggering is required so a partial drain is re-delivered after the EOI instead
    of waiting for a new edge that the controller cannot produce while its line is
    already high;
  - the ring is single-producer/single-consumer: only the handler writes the head and only
    the decoder (holding the decoder lock) writes the tail. The handler takes **no kernel
    lock and prints nothing**, because it may interrupt code that holds the decoder,
    graphics or console lock;
  - the handler drains at most 64 bytes per interrupt, pushes them into the ring, counts
    the interrupt and the bytes, and acknowledges the **local APIC only** (the legacy PIC
    remains masked; no PIC EOI is sent);
  - a full ring discards the newest byte and increments a drop counter, reported once in
    the boot log. Overflow is a bounded failure, never a stall: the handler keeps draining
    the controller, so the level-triggered line still falls;
  - an unexpected or spurious `0x2C` with no aux byte waiting acknowledges and returns.
- **I33 (MUST):** Bring-up is fail-safe and visible. The boot log names the route
  (`ioapic: version ..., IRQ12 -> vector 0x2c on apic N (level)` and
  `mouse: aux IRQ12 enabled (config ...)`) or the reason polling remains
  (`ioapic: absent or unusable; PS/2 polling only`, `... no local APIC; PS/2 polling
  only`, or a controller-step failure). The first interrupt that delivers bytes is
  reported once as `mouse: aux IRQ12 delivered N interrupt(s), M byte(s)`, printed from
  the decoder context rather than the handler. An absent device, an absent IOAPIC and a
  failed configuration write all leave the polling path working.

### Key events and the event queue (M2.2.3–M2.2.5)

These slices are part of the normative surface; the design sub-sections below ("Key
events", "The event queue") record how they were implemented.

- **I18 (MUST):** The keyboard grows a **key event** layer separate from the line
  discipline: scan-code set 1 is decoded to a key code plus an ASCII/text field with
  Shift/Ctrl/Alt/Caps Lock state, arrows and function keys, into a bounded queue that
  does not drop under fast typing. `core.read_line` stays for the shell. The same byte
  feeds both consumers: a key that produces console text still produces its event.
- **I19 (MUST):** Events are delivered through a predeclared `Event` record
  `(kind, x, y, code, window)` and two primitives: `syslib.next_event(var ev) -> Status`
  blocks (via a new `WaitFor::Input`, SMP-safe wake) until one is queued, and
  `syslib.poll_event(var ev) -> Status` returns `WouldBlock` when the queue is empty.
  Overflow policy: the queue is a bounded ring; when it is full the **newest** event is
  dropped and a drop counter is kept, so a slow reader loses recent input rather than the
  event it is about to process. The queue itself is machine-wide and keeps
  `Event.window = 0`; the kernel host fills the target at **poll time** (spec 0011, W16):
  a pointer-down is hit-tested and raises/focuses the topmost window, other pointer and
  key events go to the focused window, and an `Expose` (kind 5) is a per-window, owner-only
  pending notification that is returned before the queue.
- **I29 (MUST):** `syslib.inject_event(ev: Event) -> Status` appends a caller-built
  record to the queue, and `syslib.event_drops() -> uint` / `syslib.event_flush() -> uint`
  expose the drop counter and discard the queue (returning the count). Injection is
  **debug-only**, like `inject_pointer`: it validates the `kind` ordinal (`Invalid`
  otherwise) and does not move the pointer. The drop counter is monotonic since boot and
  is not reset by a flush.
- **I30 (MUST):** A task blocked in `next_event` is woken by any insertion on any core.
  The producer publishes the event first and then wakes, and the scheduler re-checks the
  queue after recording the block, so an event that arrives in the window between the
  empty read and the block is not lost.
- **I31 (MUST):** The queue is one machine-wide FIFO: two tasks that call `next_event`
  compete for the same queued events and each queued event is delivered once. Window
  routing is applied to the record when it is polled (never to a shared queue entry), and
  an `Expose` is not a queue entry at all: it is one coalesced pending flag per damaged
  window, readable only by that window's owner. A non-zero `window` on a queued event (an
  injection) is passed through unchanged.

### Absolute pointer (M2.2.6)

- **I20 (MUST):** There are two pointer sources. The PS/2 auxiliary device is the
  relative fallback (I1–I5). A USB HID tablet, when present, is the absolute source: one
  report carries the current position, so a dropped report is harmless and no motion is
  accumulated across reports.
- **I21 (MUST):** The tablet takes precedence when it is ready. While it is the source,
  decoded PS/2 motion is ignored (the bytes are still drained so the controller cannot
  fill), and the tablet supplies both position and buttons. If the tablet is absent or
  its bring-up fails, the source stays PS/2; if the PS/2 device did not initialise either,
  the source is none.
- **I22 (MUST):** Bring-up is bounded and allocation-free. The kernel scans PCI bus 0
  (slots 0–31, functions 0–7), finds the class `0C.03` prog-if `00` UHCI controller,
  enables its I/O space and bus mastering, and takes BAR4 as the 32-byte I/O region. The
  schedule is one static 4 KiB-aligned 1024-entry frame list and static queue heads;
  control and interrupt transfers use static 16-byte TDs. Every DMA structure lives in
  `.bss`, is physically contiguous, and is handed to the controller as
  `virtual - KERNEL_BASE` because the kernel is linked high and aliased into the identity
  map.
- **I23 (MUST):** Enumeration is `GET_DESCRIPTOR(Device)` → `SET_ADDRESS(1)` →
  `GET_DESCRIPTOR(Configuration)` → `SET_CONFIGURATION(1)` → HID `SET_IDLE(0)`, with
  `GET_DESCRIPTOR(HID Report)` used to verify the report shape. The configuration must be
  one HID interface (class 3, subclass 0, protocol 0 — a boot mouse/keyboard is not the
  absolute pointer) with one interrupt-IN endpoint. Every step has a timeout; a failure is
  a boot log line and the PS/2 fallback, never a hang.
- **I24 (MUST):** The accepted HID report is the QEMU tablet shape measured on 2026-11-16:
  one buttons byte (bit 0 left, bit 1 right, bit 2 middle), X and Y as absolute 16-bit
  little-endian values with logical range `0..=32767`, and a wheel byte. A descriptor that
  does not declare that absolute 16-bit X/Y input is refused; the driver never guesses an
  absolute mapping.
- **I25 (MUST):** Axis scaling maps HID `0..=32767` to screen `0..=w-1` / `0..=h-1` with
  round-to-nearest: `(2 * v * (w - 1) + 32767) / 65534`. So `0 → 0`,
  `16384 → (w-1)/2` (512 at 1024, 384 at 768), and `32767 → w-1` / `h-1` (1023 / 767).
  Y has the same top-left origin as the screen (no inversion, unlike the PS/2 relative
  packet). The button byte is masked to `0b111`.
- **I26 (MUST):** The interrupt-IN endpoint is **polled**, not interrupt-driven, from the
  same service opportunity and idle loop as the PS/2 device. One TD is kept armed; a NAK
  leaves it active. On completion the report is parsed, the pointer is set with
  `gfx.set_pointer_absolute`, and the TD is rearmed with the next data toggle. A transfer
  error disables the tablet and falls back to PS/2.
- **I27 (MUST):** `syslib.pointer_source() -> uint` reports the current source: `0` none,
  `1` PS/2, `2` tablet. It is a machine-wide read-only view. The debug-only
  `inject_pointer` still moves the pointer regardless of source.
- **I28 (MUST):** The boot log names the USB path: one `pci:` line per function found, the
  UHCI I/O base with `USBSTS` and both `PORTSC`s, the device VID:PID and report descriptor
  length, and either `usb: tablet ...` or `usb: no tablet (...); PS/2 only`. A machine
  without a UHCI controller logs `usb: no UHCI; PS/2 only`.

## Design

### Data flow

```
i8042 ── status 0x64 / data 0x60 ──▶ keyboard (byte stream, console)
        │
        ├─ status bit 5 (aux) ──▶ IRQ12 ──▶ I/O APIC ──▶ handler (boot core)
        │                                                 │ drain ≤ 64 bytes
        │                                                 ▼
        │                                         lock-free byte ring
        │                                                 │ service opportunity
        │                                                 ▼
        └─ fallback: mouse::poll reads the controller ──▶ mouse::Decoder
                                                              │ Packet { dx, dy, buttons }
                                                              ▼
PCI 0C.03 ──▶ UHCI ──▶ tablet HID report { buttons, x, y }
                              │ absolute 0..=32767, scaled
                              ▼
                        gfx pointer state ──▶ present() composites the cursor
                              ▲                        │
                              └── inject_pointer ──────┘   (debug hook)

application ── syslib.pointer_x/y/buttons/source ──▶ pointer state (read only)
application ── syslib.get_pixel ─────────────────────▶ shadow (never the cursor)
```

The PS/2 path is the fallback; while the tablet is ready its reports are ignored (I21).
The handler only moves aux bytes; the service opportunity decodes them, so a report is
applied within one service opportunity of arriving. The tablet endpoint is still polled in
the same opportunity, and both paths update one pointer state.

### Controller initialisation

| Step | Write | Wait | On timeout |
|------|-------|------|------------|
| 1 | — | input buffer clear, then drain stale output bytes | device absent |
| 2 | `0x64 ← 0xA8` | input buffer clear | device absent |
| 3 | `0x64 ← 0x20`, read `0x60` | output full | device absent |
| 4 | `0x64 ← 0x60`, `0x60 ← config & !0x02` | input buffer clear | device absent |
| 5 | `0x64 ← 0xD4`, `0x60 ← 0xF6` | ACK `0xFA` | no aux device |
| 6 | `0x64 ← 0xD4`, `0x60 ← 0xF3`, `0x60 ← 100` | two ACKs | no aux device |
| 7 | `0x64 ← 0xD4`, `0x60 ← 0xE8`, `0x60 ← 3` | two ACKs | no aux device |
| 8 | `0x64 ← 0xD4`, `0x60 ← 0xE6` | ACK | no aux device |
| 9 | `0x64 ← 0xD4`, `0x60 ← 0xF4` | ACK | no aux device |

The configuration byte is read-modify-written, so the keyboard translation bit and the
keyboard clock bit are preserved. At step 4 the aux interrupt is left clear because the
IDT gate and the IOAPIC route do not exist yet; the second bring-up step (below) sets it.
The keyboard interface itself is enabled by the existing `keyboard::init` (`0xAE`).

### The auxiliary interrupt (M2.2.7)

`kernel/src/mouse.rs` and `arch/x86_64/src/ioapic.rs` split the path in two: a short
handler that moves bytes, and the service opportunity that decodes them.

| Step | What | Bound |
|------|------|-------|
| Route | split the `0xFEC00000` 2 MiB page once, mark the register page `PWT \| PCD`, reload CR3 | 1 table |
| Probe | read `IOAPICVER`; reject `0xFFFFFFFF`, 0 or a redirection count below 12 | 1 read |
| Program | entry 12: fixed, physical, active-high, level, unmasked, destination = boot core APIC id | 3 writes |
| Gate | `idt::init` installs `irq_aux` (vector `0x2C`) before the route exists | boot order |
| Arm | stop reporting (`0xF5`), re-read the configuration, re-enable reporting (`0xF4`), write `config \| 0x02` last | 5 commands |
| Drain | handler reads status + data while bit 5 says aux; pushes to the ring; EOIs the LAPIC | 64 bytes/IRQ |
| Decode | `mouse::poll` drains the ring under the decoder lock and applies packets | service opportunity |

The order in "Arm" is load-bearing: once bit 1 is set, an ACK byte is aux data, and the
handler would steal the `0xF4` ACK from `wait_ack`; so reporting is re-enabled before the
configuration byte is written. That last write has no ACK, and any byte arriving after it
is drained by the handler. The configuration byte is not read back after bit 1 is set,
because the read response would race a pending report; the boot log states the value that
was written, and the one-time `mouse: aux IRQ12 delivered ...` line proves the route.

The ring holds 1024 bytes (341 reports), more than the 300-event burst in
`scripts/mouse_burst_qmp.py`, so a burst that reaches the handler cannot overflow before
the decoder runs. A full ring drops the newest byte and counts it; the handler still
consumes the controller byte, so a level-triggered line does not stay asserted.

Level triggering is deliberate. The i8042 keeps IRQ12 asserted while its output buffer
holds another byte; with an edge-triggered entry a handler that stopped at the 64-byte
bound would leave the line high with no new edge and stall until the next unrelated
interrupt. With a level entry the IOAPIC re-delivers after the EOI, so the bound is safe.

The handler allocates nothing, prints nothing and takes no kernel lock. It runs on the
boot core only (the redirection entry names that APIC id), which is what makes the ring a
single-producer structure. The decoder side may run on any core; the decoder lock makes it
a single consumer at a time.

The QMP burst harness injects 20 × `rel x=-100` to clamp at the left edge, then 300 ×
`rel x=+2` as one `input-send-event` batch, and requires exactly +600 guest pixels. On
QEMU 10.2.2 the polled build also reports 600/600: the emulated PS/2 model accumulates
deltas in `mouse_dx` and defers packets until the guest reads, so the historical
host-window 346/600 is not reproducible headlessly. The IRQ build's transcript additionally
shows the `IRQ12 delivered` line and no direct-poll fallback line.

### Packet decoding

```
byte 0:  7 Y overflow | 6 X overflow | 5 Y sign | 4 X sign | 3 always 1 | 2 middle | 1 right | 0 left
byte 1:  X delta, low 8 bits
byte 2:  Y delta, low 8 bits

dx = sign ? byte1 - 256 : byte1          (-256 ..= 255)
dy = sign ? byte2 - 256 : byte2          (-256 ..= 255), positive is up
```

The decoder drops a byte while it is not expecting a first byte and bit 3 is clear; while
assembling it accepts any byte (bytes 1 and 2 carry data in every bit position). An
overflow packet is discarded whole, but its buttons are **not** applied either: the
report is untrustworthy, and the next packet re-states the buttons.

### Pointer state and clamping

`x`, `y` and `buttons` live in the graphics module next to the shadow, because the cursor
is composited there. Keeping them in one place means `present` can read the position under
the lock it already holds, with no second lock and therefore no lock-order question.

```text
new_x = clamp(x + dx, 0, width - 1)
new_y = clamp(y + dy, 0, height - 1)     // dy already screen-oriented
```

The update and the repaint happen under the graphics lock: compute the old cursor
rectangle, store the new position and buttons, present the old rectangle (shadow only,
unless it overlaps the new one) and present the new rectangle (with the overlay). Two
cores therefore cannot leave a ghost cursor behind; an application present that runs
between them sees the new position, which is the eventual image anyway.

### Cursor overlay

The cursor occupies the 16×16 rectangle whose top-left corner is the pointer position.
It is clipped to the screen like any other rectangle. `present` does:

1. copy the clipped shadow rectangle to the framebuffer, row by row, pitch-aware;
2. for the intersection of that rectangle with the cursor rectangle, write the black
   outline and white fill pixels **to the framebuffer** (not the shadow).

`get_pixel` reads the shadow only, so the cursor is invisible to it. The console's cell
present goes through the same `present`, so text under the cursor is drawn and then
covered by it; when the cursor moves away the cell is presented again from the shadow and
the text is back.

### Key events (M2.2.3)

`arch/x86_64::keyboard::poll` drains controller bytes once and reports both outputs of
each byte through one callback: the console byte (for the line discipline) and the key
event (for the queue). A byte that produces both is delivered to both, so the shell and an
event-driven application never steal each other's keys.

| Scan-code set 1 | Console byte | `KeyEvent` |
|---|---|---|
| printable key | the character | stable key code + text under Shift/Caps/Ctrl |
| Enter (0x1C, or E0 1C) | `\n` | `Enter` |
| Backspace (0x0E) | `0x7F` | `Backspace` |
| Tab (0x0F) | `\t` | `Tab` |
| Ctrl+C | `0x03` | `C` with `Ctrl` and text `0x03` |
| Shift/Ctrl/Alt/Caps make/break | — | the modifier key itself, with the new state |
| arrows, Home/End/PgUp/PgDn/Insert/Delete (plain or E0) | — | the `0x110..=0x119` codes |
| F1–F12 | — | the `0x120..=0x12B` codes |
| E1 Pause sequence | — | dropped whole (the skip counter prevents fake modifiers) |

A key event packs the three things a consumer needs into `Event.code`, because the record
has one code field (the values are `abi::key`):

```text
bits  0..=7   text: ASCII/control byte, 0 for a non-text key or a release
bits  8..=11  modifiers after the transition: Shift 1, Ctrl 2, Alt 4, Caps 8
bits 16..=31  key code: unshifted ASCII (letters uppercase) or a 0x100-range code
```

The key code is the stable physical identity, so `Shift+a` and `a` share key code `'A'`
but differ in text and modifiers. Modifier transitions are themselves events, so a
consumer can track the modifier state without a separate field. `Shift+A` with Caps Lock
on produces text `'A'` with both `Shift` and `Caps` set.

`keyboard::decoder_self_check()` feeds canned scancodes through the same pure `decode`
function at boot and prints `keyboard: decoder ok` (a mismatch halts with the step
number), covering make/break, Shift, Caps toggling, Ctrl+C, an extended arrow, F1, Enter,
Backspace and the Pause sequence.

### The event queue (M2.2.4)

One machine-wide FIFO in `kernel/src/event.rs`:

```text
Event record (40 bytes, abi::layout::event)
  0   kind    EventKind enum ordinal: 0 PointerMove | 1 PointerDown |
              2 PointerUp | 3 KeyDown | 4 KeyUp | 5 Expose
  8   x       int, pointer x in pixels (0 for key events and Expose)
  16  y       int, pointer y in pixels (0 for key events and Expose)
  24  code    uint, packed key value (key events) or button mask (pointer)
  32  window  int, filled at poll time: the routed window id, or 0
```

An `Expose` asks the owner to repaint the **whole content rectangle** of window `window`
(spec 0011, W12); `x`, `y` and `code` are 0.

The queue holds **64** events. A full queue drops the incoming event and increments a
monotonic drop counter; `event_flush` empties the queue and returns the discarded count
without resetting the counter. The pointer producers emit `PointerMove` on a position
change, `PointerDown` with the newly pressed bits and `PointerUp` with the released bits;
a repeated absolute report is idempotent and emits nothing.

An insertion wakes every task blocked on `WaitFor::Input` after the queue lock is
dropped; the scheduler records that wait only after the empty read, then re-checks the
queue, so the insert-between-read-and-block race is closed. `next_event` restores its
operand stack and `pc` exactly like the stream and child waits, so the retry sees an
unchanged instruction.

`syslib.inject_event` validates the record's kind through `abi::layout::event::read` and
appends it; it is the deterministic test hook and does not change the pointer (use
`inject_pointer` for that). `samples/apps/events` and the helper `events_poke` prove the
whole surface, including a genuinely blocked `next_event` woken by another task.

### Injection

`syslib.inject_pointer(dx, dy, buttons)` calls the same pointer update as a decoded
packet. It is the only deterministic input path in this slice and it is what the proof app
uses; it will be joined by `inject_event` in M2.2.4. The primitive is documented as
debug-only in spec 0003 and in the boot log's `mouse:` line.

### Absolute pointer bring-up (M2.2.6)

The USB path is deliberately small: one full-speed HID tablet on the PIIX3 UHCI
controller, polled with one interrupt TD, no interrupts and no allocation. `kernel/src/pci.rs`
finds the controller and `kernel/src/usb.rs` owns the UHCI schedule. Both are x86-specific
for now and move behind `arch`/`platform` when a second machine exists (ADR 0022).

| Step | What | Bound |
|------|------|-------|
| PCI | scan bus 0, slots 0–31, functions 0–7; class `0C.03`, prog-if `00`; enable I/O + bus master; BAR4 | 256 functions |
| UHCI | `USBCMD.HCRESET`, wait for self-clear; `USBSTS` clear; `FLBASEADD` = frame list; `FRNUM` 0; `USBCMD` run + configured + max packet 64 | 20 ms each |
| Port | find `CCS` on port 1 or 2; drive `PR` 50 ms; clear; set `EN`; acknowledge change bits with `EN` set | 70 ms + read-back |
| Enumeration | `GET_DESCRIPTOR(Device)` (18) → `SET_ADDRESS(1)` → `GET_DESCRIPTOR(Configuration)` (128) → `SET_CONFIGURATION(1)` → HID `SET_IDLE(0)` → `GET_DESCRIPTOR(HID Report)` (64) | 200 ms per transfer |
| Interrupt | one TD, self-linked with depth-first; `Active` remains set across NAKs; rearm after completion | polled |

The configuration is checked before the report descriptor is trusted: interface class 3,
subclass 0, protocol 0, one interrupt-IN endpoint. That excludes a boot mouse (1/2) and a
boot keyboard (1/1). No `SET_PROTOCOL` is sent: the tablet is a non-boot HID interface and
QEMU stalls that request for it; `SET_IDLE(0)` is enough to keep reports flowing. The HID
report descriptor must contain the absolute X/Y shape:
`Usage X`/`Usage Y`, logical maximum `0x7fff`, report size 16, report count 2, and an
`Input (Data,Var,Abs)` item, plus a one-byte five-button field. Anything else is
`usb: no tablet (not an absolute tablet); PS/2 only`.

The report layout measured through QMP `input-send-event` absolute injection:

| Offset | Field | Notes |
|--------|-------|-------|
| 0 | buttons | bit 0 left, bit 1 right, bit 2 middle; bits 3–4 (side/extra) are masked off |
| 1–2 | X | little-endian `u16`, `0..=32767`, 0 at the left edge |
| 3–4 | Y | little-endian `u16`, `0..=32767`, 0 at the top edge (no inversion) |
| 5 | wheel | unused; the endpoint packet is 8 bytes and the report is 6 |

`gfx.set_pointer_absolute(x, y, buttons)` is separate from the relative
`update_pointer`: it writes the position, clamps to the screen, and repaints only the old
and new cursor rectangles. An identical report changes nothing, so a dropped or repeated
report is idempotent. `syslib.pointer_source()` is the only new language surface; the
three read views and `inject_pointer` are unchanged.

The static DMA cost is 4096 bytes for the frame list plus 240 bytes for the two queue
heads, four TDs, the 8-byte setup packet and the 128-byte data buffer (4336 bytes of
`.bss`). The UHCI schedule takes physical addresses, computed as `virtual - KERNEL_BASE`;
the frame list is 4 KiB aligned by its type and every TD/QH is 16-byte aligned.

## Interfaces

### Language surface (normative)

```text
syslib.pointer_x() -> int
syslib.pointer_y() -> int
syslib.pointer_buttons() -> uint      // bit 0 left, bit 1 right, bit 2 middle
syslib.pointer_source() -> uint       // 0 none, 1 PS/2, 2 tablet
syslib.inject_pointer(dx: int, dy: int, buttons: uint) -> Status

syslib.next_event(var ev: Event) -> Status    // blocks on WaitFor::Input
syslib.poll_event(var ev: Event) -> Status    // WouldBlock when empty
syslib.inject_event(ev: Event) -> Status      // debug-only test hook
syslib.event_drops() -> uint
syslib.event_flush() -> uint
```

The compiler checks arity and the staged types (`int` deltas, `uint` buttons and source);
the interpreter rejects `buttons > 0b111` with `Status.Invalid` before calling the host,
like the drawing primitives reject a non-positive extent (spec 0009, G9). `Event` and
`EventKind` are predeclared; `next_event`/`poll_event` require a writable `var ev: Event`,
while `inject_event` takes a read-only `ev: Event`.

### Host contract

```rust
fn pointer_x(&mut self) -> i64 { 0 }
fn pointer_y(&mut self) -> i64 { 0 }
fn pointer_buttons(&mut self) -> u64 { 0 }
fn pointer_source(&mut self) -> u64 { 0 }   // 0 none, 1 PS/2, 2 tablet
fn inject_pointer(&mut self, _dx: i64, _dy: i64, _buttons: u32) -> Status { Status::Invalid }
fn poll_event(&mut self, _ev: &mut [u8]) -> Status { Status::WouldBlock }
fn inject_event(&mut self, _ev: &[u8]) -> Status { Status::Invalid }
fn event_drops(&mut self) -> u64 { 0 }
fn event_flush(&mut self) -> u64 { 0 }
```

The defaults are a machine with no pointer device and no event queue (I11). The kernel
implements them over `kernel/src/mouse.rs`, `kernel/src/usb.rs`, `kernel/src/gfx.rs` and
`kernel/src/event.rs`.

### Kernel contract

`mouse::init()` initialises the controller, runs a mode-independent decoder self-check
(canned packets through the same decoder and the same clamp arithmetic, on scratch state,
so the real pointer is untouched), logs `mouse: PS/2 aux ready` or `mouse: no aux device`,
and prints `mouse: decoder ok` or halts loudly with the failing step. `mouse::enable_irq`
is the later, optional second step that arms IRQ12 (above). `mouse::poll()` drains the
byte ring, or the controller when IRQ12 is inactive; `gfx::update_pointer(dx, dy, buttons)`
applies one move; `gfx` owns the pointer state and the cursor table. `pci::init()` scans bus 0 and returns the
UHCI controller, `usb::init()` enumerates the tablet and arms the interrupt endpoint, and
`usb::poll()` parses a completed report and calls `gfx::set_pointer_absolute`.

`keyboard::decoder_self_check()` runs the same pure `decode` the live path uses and prints
`keyboard: decoder ok` or halts with the step. `console::poll_input` passes each drained
byte to both consumers. `event::push` owns the 64-slot queue and wakes `WaitFor::Input`;
`event::poll`, `event::inject`, `event::drops` and `event::flush` are the host contract.

`ioapic::init(irq, vector, destination)` maps the register page uncached, probes
`IOAPICVER` and programs one level-triggered redirection entry; it returns `None` for an
absent or implausible IOAPIC. `mouse::enable_irq(routed)` is the second bring-up step:
it stops reporting, re-reads the configuration, re-enables reporting, writes
`config | 0x02` and marks IRQ12 active. `mouse::on_irq()` is called by the `irq_aux` stub:
it drains at most 64 aux bytes into the ring and EOIs the local APIC. `mouse::poll()`
drains the ring (and, only while IRQ12 is inactive, the controller) and decodes it.
`idt::init` installs the `0x2C` gate; `apic::AUX_VECTOR` is the one vector constant.

## Failure modes

| Situation | Behaviour |
|-----------|-----------|
| No i8042 controller or no aux device | `mouse: no aux device`; no cursor; the keyboard and console keep working |
| A controller command times out | Treated as "no aux device"; init stops, nothing else is affected |
| No I/O APIC, or `IOAPICVER` is implausible | `ioapic: absent or unusable; PS/2 polling only`; the polling drain stays; no hang |
| No local APIC | `ioapic: no local APIC; PS/2 polling only`; the timer path is already absent, polling stays |
| Configuration read/write or the reporting commands time out | `mouse: aux IRQ12 unavailable (...); polling`; bit 1 stays clear and the direct drain is used |
| Byte stream desync (first byte without bit 3) | Bytes are dropped until bit 3 is set; assembly restarts |
| Packet with X or Y overflow | Packet ignored whole, including its buttons; the next packet re-states them |
| Controller output buffer full of keyboard bytes | The handler stops at a keyboard byte and the keyboard path consumes it in the same opportunity (I15) |
| Aux byte arrives while the handler is running | The line stays high; the level-triggered entry re-delivers after the EOI |
| Handler reaches its 64-byte bound with the line still high | The level entry re-fires after the EOI; the next pass continues the drain |
| The 1024-byte ring is full | The newest byte is dropped and counted; the handler still consumes the controller byte, so the line falls; `mouse: aux ring overflow ...` is printed once |
| A spurious `0x2C` with no aux byte waiting | The handler EOIs and returns; nothing else changes |
| Pointer delta runs past an edge | Clamped to the edge; no wrap, no error |
| `inject_pointer` with `buttons > 0b111` | `Status.Invalid`, position and buttons unchanged |
| Graphics disabled (`screen_width == 0`) | Pointer primitives report 0/no buttons, `inject_pointer` is `Invalid`; the device is still drained, but there is nothing to clamp to and no cursor |
| Two cores reach the service opportunity at once | The decoder lock serialises ring consumption and the update; the handler never takes it, so it cannot deadlock with a held lock |
| Cursor over text | The cell is presented with the cursor on top; moving away restores the text from the shadow |
| Application reads `get_pixel` under the cursor | It gets the uncomposited shadow pixel (I7) |
| Application drives the i8042 through `syslib.port_out8` | Allowed (no capability model); the kernel does not promise input still works |
| No PCI UHCI function | `usb: no UHCI; PS/2 only`; no tablet, PS/2 pointer keeps working |
| No device on either UHCI port | `usb: no tablet (no device); PS/2 only` |
| Port reset/enable fails, or the device is low-speed | `usb: no tablet (port enable)` / `low-speed device`; PS/2 fallback |
| A control transfer times out or STALLs | `usb: no tablet (transfer timeout/error); PS/2 only`; no hang |
| Descriptor is not an absolute HID tablet | `usb: no tablet (not an absolute tablet); PS/2 only`; never guessed |
| HID report descriptor differs from the measured shape | Refused at enumeration; PS/2 fallback |
| Interrupt-IN NAK | The TD stays Active and is retried by the controller next frame; no bus error |
| Interrupt-IN completion with an error | `usb: tablet report error ...; PS/2 fallback`; the source returns to PS/2 |
| Absolute report outside `0..=32767` | Clamped before scaling; the axis stops at the edge |
| Repeated/duplicate absolute report | Idempotent: the same position and buttons repaint nothing |
| Tablet ready, PS/2 packet arrives | Decoded and dropped (bytes drained); the tablet is authoritative |
| Event queue full | The incoming (newest) event is dropped and the drop counter increments; older events stay |
| `next_event` with an empty queue | The task blocks on `WaitFor::Input`; any insertion on any core wakes it |
| `inject_event` with an unknown `kind` ordinal | `Status.Invalid`; nothing is queued |
| `event_flush` with an empty queue | Returns 0; the drop counter is unchanged |
| Two tasks call `next_event` | They compete for one machine-wide FIFO; each event is delivered once |
| A keyboard byte arrives | It produces its console byte and its key event, or only one of them; neither path consumes the other's |
| A Pause (`0xE1`) sequence | The whole five-byte tail is skipped, so it cannot fake Ctrl/NumLock events |
| Pressing a key with no console meaning | No console byte; the key event is still delivered |
| A button transition while the pointer also moves | One `PointerMove` followed by `PointerDown`/`PointerUp` with only the changed bits |

## Alternatives considered

| Alternative | Why not |
|-------------|---------|
| Mouse as an `in_stream` owned by a driver application | The device is polled by the kernel service opportunity and the pointer state is machine-wide; a byte FIFO adds a protocol and a second copy for no isolation (ADR 0021) |
| Applications poll the i8042 themselves | Every application would fight over one controller buffer; the line discipline would lose bytes; pointer state would be per-application |
| Save-under the cursor in the shadow | `get_pixel` would then see the cursor, the framebuffer and the shadow would disagree, and every drawing primitive would have to restore it first (ADR 0021) |
| A hardware cursor sprite | Device-specific state, no proof under QEMU, and it would not compose with the window clip later |
| Per-application cursors | No per-window ownership exists until M2.3 and the visual identity is the system's |
| Acceleration in this slice | Needs tuning constants and a clock; 1:1 is enough to prove the path |
| Reading and decoding the mouse in the timer interrupt | The decoder and the pointer update take the decoder and graphics locks, and the interrupted thread may already hold either; decoding stays in the service opportunity and the ISR only moves bytes into a lock-free ring (ADR 0023) |
| An edge-triggered IRQ12 entry | The i8042 holds its line high while more data is buffered, so a bounded drain could leave the line asserted with no new edge; a level entry re-delivers after the EOI (ADR 0023) |
| A locked queue between the handler and the decoder | The handler may interrupt a core holding the queue lock (it can hold the decoder lock in `poll`), which is a self-deadlock; a single-producer/single-consumer atomic ring needs no lock in the handler (ADR 0023) |
| A PS/2 scale factor / acceleration | Relative motion is in host pixels and its loss is burst-dependent; a scale can never reach the guest edge and needs tuning per host window (Q49, ADR 0022) |
| virtio-input / virtio-tablet | A second virtual device class and a queue protocol, while the historical PC machine already has UHCI; the goal is a plain USB tablet QEMU exposes (ADR 0022) |
| A user-space USB driver over streams | The pointer state is machine-wide and the kernel already polls the device; a driver application would add a byte protocol and a copy per report (ADR 0021, ADR 0022) |
| Relative-only pointer | Measured motion loss (+346 of 600 px) and host scaling leave the pointer short of the edges (Q49) |

## Open questions

| # | Question | Trigger / owner |
|---|----------|-----------------|
| 1 | Pointer acceleration curve and a "pixels per count" scale | when the desktop is actually used with a mouse |
| 2 | Wheel, extra buttons and double-click detection | when a control needs them (M2.4) |
| 3 | ~~Event queue capacity and drop accounting~~ **Resolved (2026-11-16)**: fixed 64-event queue, newest dropped when full, monotonic drop counter, `event_flush` returns the discarded count and does not reset the counter | M2.2.4 |
| 4 | A hardware cursor / double buffering if the present cost matters | M2.6.2 measurement |
| 5 | Whether injection survives into a "debug build" flag | if `syslib.inject_pointer` is ever considered a production surface |
| 6 | Multi-packet control stages on real hardware | the measured QEMU tablet returns a whole descriptor in one data TD; a device whose control pipe is smaller than 64 bytes would need chained data TDs and would currently be refused |
| 7 | USB keyboard, hubs and mass storage | not in this slice; the UHCI driver serves one interrupt-IN endpoint. PS/2 IRQ12 is implemented (M2.2.7); USB endpoints are still polled |

## Changelog

| Date | Change |
|------|--------|
| 2026-11-16 | Initial accepted version: PS/2 aux init and decoding, machine-wide pointer state with clamping, cursor composited at present time, the four `syslib` pointer primitives, debug injection, polling ownership, and forward references for key events (M2.2.3) and the event queue (M2.2.4). |
| 2026-11-16 | M2.2.6 absolute pointer (ADR 0022): PCI scan + UHCI + HID `usb-tablet` enumeration (`GET_DESCRIPTOR`/`SET_ADDRESS`/`SET_CONFIGURATION`/`SET_IDLE`), the measured report layout (buttons, absolute 16-bit LE X/Y, wheel), 0..=32767 → `0..=w-1` scaling with round-to-nearest, tablet-over-PS/2 precedence, the polled interrupt-IN endpoint, `syslib.pointer_source()` (builtin 26, no new opcode), and the `usb:` boot lines. Q49's burst loss and host-scale mismatch are fixed by the absolute report; IRQ12 remains M2.2.7. |
| 2026-11-16 | M2.2.7 PS/2 aux IRQ12 (ADR 0023): the IDT gate `0x2C` and `arch/x86_64::ioapic` route IRQ12 through the I/O APIC to the boot core as a level-triggered fixed/physical/active-high entry, with the `0xFEC00000` page split and marked `PWT \| PCD`. The handler drains at most 64 aux bytes into a 1024-byte single-producer/single-consumer atomic ring and EOIs only the local APIC; `mouse::poll` decodes the ring in the service opportunity, and the direct controller drain remains the bounded backstop when the route is unavailable (the two modes are exclusive, so an aux byte has one reader). The controller configuration value is now written to the data port (the old code wrote it to the command port, so bit 1 never persisted); `enable_irq` stops reporting, re-reads the configuration, re-enables reporting and writes `config \| 0x02` last, because once bit 1 is set the handler would steal the `0xF4` ACK. Boot lines: `ioapic: version ...`, `mouse: aux IRQ12 enabled ...`, one-time `mouse: aux IRQ12 delivered ...`, and the `absent/unavailable ... polling` fallbacks. `scripts/mouse_burst_qmp.py` injects a 300×(+2) burst after clamping at x=0 and requires +600; on QEMU 10.2.2 the polled build is also 600/600 because the emulated PS/2 model retains deltas, so the historical 346/600 is a host-window result and is not reproducible headlessly. Verified `-smp 4` and `-smp 1` with all regressions and QMP absolute 7/7. |
| 2026-11-16 | M2.2.3–M2.2.5 key events and the event queue: `arch/x86_64::keyboard` decodes set 1 into a console byte and a `KeyEvent` (stable key code, text, Shift/Ctrl/Alt/Caps state, arrows, F1–F12, E1 Pause skipped) with a boot self-check. Predeclared `EventKind` (`PointerMove`/`PointerDown`/`PointerUp`/`KeyDown`/`KeyUp`) and `Event` (`kind,x,y,code,window`, 40 bytes at `abi::layout::event`); key events pack text/modifiers/key code into `code` (`abi::key`). `kernel/src/event.rs` owns a 64-event queue that drops the newest on overflow with a monotonic counter, and pointer transitions enqueue move/down/up. `syslib.next_event` (blocks on `WaitFor::Input`, SMP-safe wake), `poll_event`, `inject_event` (debug-only), `event_drops` and `event_flush` are builtins 27–31 with no new opcode. `samples/apps/events` + `events_poke` prove the API, the overflow policy and a genuinely blocked wake; `keyboard: decoder ok` and `events: ok` run on `-smp 4` and `-smp 1`. |
| 2026-11-16 | M2.3.3 routing and `Expose` (spec 0011, W12/W16/W17): `EventKind` gains `Expose` (ordinal 5). Producers still enqueue `window = 0`; the kernel host routes on poll — a pending owner-only `Expose` first, then a queued event, with a pointer-down hit-tested against the topmost frame and raising/focusing it before delivery and other pointer/key events naming the focused window. An injected non-zero `window` is passed through. Exposes are per-window coalesced flags marked by a repair and are not queue entries, so a foreign task cannot steal them; the scheduler's input-block re-check includes pending exposes. |
