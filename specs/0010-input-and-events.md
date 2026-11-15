# 0010: Input, the pointer and events

- **Status:** Accepted
- **Scope:** `kernel` (`mouse`, `usb`, `pci`, `gfx`, `console`, `sched`),
  `arch/x86_64` (i8042, keyboard, ports), `abi`, `compiler`, `interpreter`,
  `syslib`; M2.2 of the desktop milestone
- **Supersedes:** none
- **Superseded by:** none
- **ADRs:** 0021, 0022
- **Updated:** 2026-11-16

## Context

The desktop (M2) has one image — the shadow screen of spec 0009 — and applications draw
into it through `syslib`. What it does not have is input. A desktop needs a pointer that
moves and clicks, a cursor the user can see, and eventually a keyboard event stream that a
window manager can route. Today the only input path is the kernel's staged line discipline:
the service opportunity polls COM1 and the PS/2 keyboard into a byte FIFO, and
`core.read_line` blocks the shell until a line is complete (spec 0005, Q44).

This spec covers the first half of that work:

- the **PS/2 auxiliary device** (mouse) as the pointer, polled by the kernel;
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

Non-goals for this slice (deferred or rejected):

- no event queue, no `Event` record, no `next_event`/`poll_event`, no `WaitFor::Input`;
- no keyboard key codes, modifiers or arrows in the language: the console keeps its byte
  stream and `core.read_line` for the shell (M2.2.3 changes that);
- no pointer acceleration, no wheel, no double-click detection, no pointer grab;
- no per-application cursor and no hardware cursor sprite;
- no window focus or routing: until M2.3, input is machine-wide and no application "owns"
  a window;
- no new bytecode opcode: the input surface is host builtins behind `BUILTIN`.
- no USB keyboard, mass storage, hub or IRQ12 in this slice (IRQ12 is M2.2.7); the UHCI
  driver serves the interrupt-IN endpoint of one full-speed HID tablet.
- no real-hardware control-pipe packetisation: the measured QEMU device returns a whole
  descriptor in one data TD. A device with a smaller control pipe would short-transfer
  and be refused as a descriptor error, never guessed at.

## Requirements

Coordinates are screen pixels, origin top-left, `x` right and `y` down, as in spec 0009.
Buttons are a bit mask: bit 0 left, bit 1 right, bit 2 middle.

### The device

- **I1 (MUST):** The kernel initialises the i8042 auxiliary device at boot with a bounded
  wait on the controller's input and output buffer bits: enable the aux interface
  (`0xA8`), read the controller configuration and clear the aux interrupt bit (`0x20` /
  `0x60`), then send `0xF6` (defaults), `0xF3 100` (100 reports/s), `0xE8 3`
  (4 counts/mm), `0xE6` (1:1 scaling) and `0xF4` (enable reporting) through the `0xD4`
  prefix, waiting for each ACK. Every step has a timeout; a missing controller or device
  is a boot log line, not a hang.
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

### Polling and ownership

- **I14 (MUST):** Input is polled by whichever core reaches the service opportunity:
  `console::poll_input` drains COM1 and the keyboard, and `mouse::poll` drains the aux
  device, before every scheduler claim and in the idle loop. There is no input IRQ.
- **I15 (MUST):** The keyboard path and the mouse path never consume each other's bytes.
  The controller status bit 5 says which device a waiting byte came from; the keyboard
  path stops at an aux byte, the mouse path stops at a keyboard byte, and both are polled
  in the same opportunity, so a byte is deferred by one poll at most and never lost.
- **I16 (MUST):** The decoder and the pointer update are serialised by a kernel lock, so
  two cores polling at once cannot interleave packet assembly or tear the position.
  Presenting is serialised by the graphics lock (spec 0009).
- **I17 (MUST):** Pointer and cursor work allocates nothing and keeps no per-application
  state: one static decoder, one static pointer, one static cursor table.

### Forward references (specified here, implemented later)

- **I18 (MUST, M2.2.3):** The keyboard grows a **key event** layer separate from the line
  discipline: scan-code set 1 is decoded to a key code plus an ASCII/text field with
  Shift/Ctrl/Alt/Caps Lock state, arrows and function keys, into a bounded queue that
  does not drop under fast typing. `core.read_line` stays for the shell.
- **I19 (MUST, M2.2.4):** Events are delivered through a predeclared `Event` record
  `(kind, x, y, code, window)` and two primitives: `syslib.next_event(var ev) -> Status`
  blocks (via a new `WaitFor::Input`, SMP-safe wake) until one is queued, and
  `syslib.poll_event(var ev) -> Status` returns `WouldBlock` when the queue is empty.
  Overflow policy: the queue is a bounded ring; when it is full the **newest** event is
  dropped and a drop counter is kept, so a slow reader loses recent input rather than the
  event it is about to process. Delivery is machine-wide (focus-independent) until M2.3
  routes events to windows.

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
        └─ aux byte ──▶ mouse::Decoder (3-byte assembly)
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
Both paths are polled in the same service opportunity, so a report is applied at most a
few milliseconds after it arrives.

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
keyboard clock bit are preserved; only the aux interrupt is cleared, because the kernel
polls (I14). The keyboard interface itself is enabled by the existing
`keyboard::init` (`0xAE`).

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
```

The compiler checks arity and the staged types (`int` deltas, `uint` buttons and source);
the interpreter rejects `buttons > 0b111` with `Status.Invalid` before calling the host,
like the drawing primitives reject a non-positive extent (spec 0009, G9).

### Host contract

```rust
fn pointer_x(&mut self) -> i64 { 0 }
fn pointer_y(&mut self) -> i64 { 0 }
fn pointer_buttons(&mut self) -> u64 { 0 }
fn pointer_source(&mut self) -> u64 { 0 }   // 0 none, 1 PS/2, 2 tablet
fn inject_pointer(&mut self, _dx: i64, _dy: i64, _buttons: u32) -> Status { Status::Invalid }
```

The defaults are a machine with no pointer device and no screen (I11). The kernel
implements them over `kernel/src/mouse.rs`, `kernel/src/usb.rs` and `kernel/src/gfx.rs`.

### Kernel contract

`mouse::init()` initialises the controller, runs a mode-independent decoder self-check
(canned packets through the same decoder and the same clamp arithmetic, on scratch state,
so the real pointer is untouched), logs `mouse: PS/2 aux ready` or `mouse: no aux device`,
and prints `mouse: decoder ok` or halts loudly with the failing step. `mouse::poll()`
drains the aux device; `gfx::update_pointer(dx, dy, buttons)` applies one move; `gfx`
owns the pointer state and the cursor table. `pci::init()` scans bus 0 and returns the
UHCI controller, `usb::init()` enumerates the tablet and arms the interrupt endpoint, and
`usb::poll()` parses a completed report and calls `gfx::set_pointer_absolute`.

## Failure modes

| Situation | Behaviour |
|-----------|-----------|
| No i8042 controller or no aux device | `mouse: no aux device`; no cursor; the keyboard and console keep working |
| A controller command times out | Treated as "no aux device"; init stops, nothing else is affected |
| Byte stream desync (first byte without bit 3) | Bytes are dropped until bit 3 is set; assembly restarts |
| Packet with X or Y overflow | Packet ignored whole, including its buttons; the next packet re-states them |
| Controller output buffer full of keyboard bytes | The mouse stops at a keyboard byte; the keyboard path consumes it at the same opportunity (I15) |
| Pointer delta runs past an edge | Clamped to the edge; no wrap, no error |
| `inject_pointer` with `buttons > 0b111` | `Status.Invalid`, position and buttons unchanged |
| Graphics disabled (`screen_width == 0`) | Pointer primitives report 0/no buttons, `inject_pointer` is `Invalid`; the device is still polled, but there is nothing to clamp to and no cursor |
| Two cores poll at once | The decoder lock serialises assembly and the update; the graphics lock serialises present; the USB lock serialises the endpoint state machine |
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

## Alternatives considered

| Alternative | Why not |
|-------------|---------|
| Mouse as an `in_stream` owned by a driver application | The device is polled by the kernel service opportunity and the pointer state is machine-wide; a byte FIFO adds a protocol and a second copy for no isolation (ADR 0021) |
| Applications poll the i8042 themselves | Every application would fight over one controller buffer; the line discipline would lose bytes; pointer state would be per-application |
| Save-under the cursor in the shadow | `get_pixel` would then see the cursor, the framebuffer and the shadow would disagree, and every drawing primitive would have to restore it first (ADR 0021) |
| A hardware cursor sprite | Device-specific state, no proof under QEMU, and it would not compose with the window clip later |
| Per-application cursors | No per-window ownership exists until M2.3 and the visual identity is the system's |
| Acceleration in this slice | Needs tuning constants and a clock; 1:1 is enough to prove the path |
| Reading the mouse from the timer interrupt | Input would then need a lock usable from interrupt context; polling in the service opportunity is already where the keyboard lives |
| A PS/2 scale factor / acceleration | Relative motion is in host pixels and its loss is burst-dependent; a scale can never reach the guest edge and needs tuning per host window (Q49, ADR 0022) |
| virtio-input / virtio-tablet | A second virtual device class and a queue protocol, while the historical PC machine already has UHCI; the goal is a plain USB tablet QEMU exposes (ADR 0022) |
| A user-space USB driver over streams | The pointer state is machine-wide and the kernel already polls the device; a driver application would add a byte protocol and a copy per report (ADR 0021, ADR 0022) |
| Relative-only pointer | Measured motion loss (+346 of 600 px) and host scaling leave the pointer short of the edges (Q49) |

## Open questions

| # | Question | Trigger / owner |
|---|----------|-----------------|
| 1 | Pointer acceleration curve and a "pixels per count" scale | when the desktop is actually used with a mouse |
| 2 | Wheel, extra buttons and double-click detection | when a control needs them (M2.4) |
| 3 | Event queue capacity and drop accounting | M2.2.4 |
| 4 | A hardware cursor / double buffering if the present cost matters | M2.6.2 measurement |
| 5 | Whether injection survives into a "debug build" flag | if `syslib.inject_pointer` is ever considered a production surface |
| 6 | Multi-packet control stages on real hardware | the measured QEMU tablet returns a whole descriptor in one data TD; a device whose control pipe is smaller than 64 bytes would need chained data TDs and would currently be refused |
| 7 | USB keyboard, hubs and mass storage | not in this slice; the UHCI driver serves one interrupt-IN endpoint and IRQ12 is M2.2.7 |

## Changelog

| Date | Change |
|------|--------|
| 2026-11-16 | Initial accepted version: PS/2 aux init and decoding, machine-wide pointer state with clamping, cursor composited at present time, the four `syslib` pointer primitives, debug injection, polling ownership, and forward references for key events (M2.2.3) and the event queue (M2.2.4). |
| 2026-11-16 | M2.2.6 absolute pointer (ADR 0022): PCI scan + UHCI + HID `usb-tablet` enumeration (`GET_DESCRIPTOR`/`SET_ADDRESS`/`SET_CONFIGURATION`/`SET_IDLE`), the measured report layout (buttons, absolute 16-bit LE X/Y, wheel), 0..=32767 → `0..=w-1` scaling with round-to-nearest, tablet-over-PS/2 precedence, the polled interrupt-IN endpoint, `syslib.pointer_source()` (builtin 26, no new opcode), and the `usb:` boot lines. Q49's burst loss and host-scale mismatch are fixed by the absolute report; IRQ12 remains M2.2.7. |
