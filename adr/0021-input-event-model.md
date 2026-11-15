# 0021: Input is one machine-wide pointer and event queue owned by the kernel, with a composited cursor

- **Status:** Accepted
- **Date:** 2026-11-16
- **Deciders:** igis-v2 design
- **Related specs:** `0002-architecture.md`, `0003-language.md`, `0005-host-interface.md`, `0006-streams.md`, `0009-graphics-and-windows.md`, `0010-input-and-events.md`

## Context

The desktop needs input: a pointer that moves and clicks, a visible cursor, and later a
keyboard event stream the window manager can route. Three shapes are possible, and they
are not interchangeable:

1. **Per-application device access.** An application polls the i8042 ports itself, as it
   can today through `syslib.port_in8`. The controller has one output buffer and no
   arbitration; two applications reading it steal each other's bytes, and the kernel line
   discipline would lose keystrokes.
2. **A driver application over streams.** A user-space driver reads the device (still
   needing a kernel polling hook) and publishes events on an `in_stream`. Streams are byte
   FIFOs copied through the kernel (spec 0006), so every event would be encoded, copied
   and decoded again, and the pointer position would live in one application rather than
   in the system.
3. **A kernel input service.** The kernel polls the device in the service opportunity it
   already runs for the keyboard, keeps the state machine and the pointer state, and
   publishes a typed surface through `syslib`.

The cursor has a similar fork. It can be painted into the shadow (save-under/restore
around every draw) or composited onto the framebuffer at present time. The shadow is
specified as the single source of truth and `get_pixel` is a documented self-check surface
(spec 0009, G5/G9); a cursor stored in it would make the check see something the
application never drew, and every drawing primitive would have to know about it.

## Decision

We will keep **input in the kernel**: the PS/2 auxiliary device is polled next to the
keyboard, the kernel owns one machine-wide pointer state and (in M2.2.4) one machine-wide
event queue, and applications see only the typed `syslib` surface (spec 0010). The cursor
is a **compositor overlay**: `present` copies the shadow and then writes the cursor pixels
to the framebuffer, so the shadow never contains the cursor and `get_pixel` stays
uncomposited.

We will also keep a **debug-only injection primitive** (`syslib.inject_pointer`, later
`inject_event`), because every other verification path in this system is headless: there is
no display in CI and no mouse in QEMU's serial-only run.

## Consequences

### Positive

- One reader per device: keyboard bytes and aux bytes are separated by the controller's
  own status bit, and neither path can starve the other (spec 0010, I15).
- The pointer and the event queue belong to the system, not to a window: until M2.3 there
  is no focus to route to, and later the window service can route from the same queue.
- `get_pixel` remains a meaningful self-check, because the cursor is invisible to it. The
  proof app can assert "the cursor is not in the shadow" directly.
- Moving the cursor repaints two 16×16 rectangles, not the screen; text under the cursor
  is restored from the shadow by re-presenting its cell.
- No protocol, no per-event copy, no second state machine and no new opcode: the four
  primitives are host builtins with safe defaults, so `langhost` and host tests need no
  device.
- Host-only tests can drive the whole path deterministically through injection.

### Negative

- The kernel gains a device driver, a pointer state machine and (later) an event queue:
  more trusted code, and a decoder bug is a system bug.
- The cursor is drawn twice — once into the shadow's cover and once as an overlay — and a
  present that contains the cursor writes those pixels to the framebuffer outside the
  row copy. That is one extra pass over at most 256 pixels per present.
- Injection is an application-reachable primitive that changes system input state. It is
  documented as debug-only, but with no capability model (vision R14) it cannot be
  restricted without one.
- A single 16×16 cursor and 1:1 motion are deliberately simplistic; acceleration, wheel
  and per-window cursors are future work.

### Neutral / follow-up work

- Keyboard **key events** and the **event queue** are specified in spec 0010 (I18/I19)
  and implemented in M2.2.3/M2.2.4; this ADR fixes their ownership, not their shape.
- The window service (M2.3) consumes the same queue and adds focus routing; the pointer
  primitives stay as a low-level read of the machine state.
- A second cursor shape or a hotspot per window is an M2.4 decision.

## Alternatives considered

| Alternative | Why not |
|-------------|---------|
| Mouse as an application driver over `in_stream` | Needs a kernel polling hook anyway, adds a byte protocol and a copy per event, and puts machine-wide pointer state in one application |
| Applications poll the i8042 ports directly | One controller buffer, no arbitration, the console line discipline loses bytes, and each application would keep its own pointer |
| Save-under the cursor in the shadow | `get_pixel` would see the cursor, the framebuffer and the shadow would disagree, and every primitive would need restore logic before drawing (spec 0009, G5) |
| A hardware cursor sprite | No portability, no benefit under QEMU, device-specific state, and it cannot compose with a window clip |
| Per-application cursors | No window ownership exists until M2.3; the system cursor is the identity of the machine |
| Deliver injected events through a stream instead of a builtin | The queue must be readable without an application pairing endpoints, and the event record is typed data, not a byte stream |
