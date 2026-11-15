# 0020: The window server is a kernel `syslib` service over one shadow screen

- **Status:** Accepted
- **Date:** 2026-11-16
- **Deciders:** igis-v2 design
- **Related specs:** `0002-architecture.md`, `0003-language.md`, `0005-host-interface.md`, `0006-streams.md`, `0009-graphics-and-windows.md`

## Context

The next milestone is a retro desktop. A desktop needs a window server: something that
owns the screen, decides which window is on top, tells applications to repaint, and
delivers pointer and keyboard events. The obvious Unix answer is a user-space process
talking to clients over a socket. That answer does not fit this system:

- there is **no shared memory**: applications are interpreter-owned regions in one
  address space, and streams are byte FIFOs copied through the kernel (`0006-streams.md`);
- there is **no window canvas** an application could write into directly, and a
  protocol that ships pixels through a byte FIFO would copy every pixel twice more;
- the kernel already owns the framebuffer and the font, and the console already draws
  into it;
- the language has no generics, no pointers and no way to map a buffer, so a practical
  canvas protocol would need a new host interface anyway;
- the system runs at ring 0 in one address space (ADR 0001): a "process boundary" here
  buys no isolation, only protocol cost.

The drawing layer that M2.1 delivers is deliberately small: a full-screen shadow buffer,
a clip rectangle and nine primitives behind `syslib`.

## Decision

We will implement the window server as a **kernel service exposed through `syslib`**,
not as an application. It owns one full-screen **shadow screen** allocated from the
frame allocator at boot; applications draw into it through clipped `syslib` primitives;
the service presents rectangles to the real framebuffer with a pitch-aware row copy.

Windows are described by the service, not by application memory. When a window needs to
be redrawn (because it was uncovered, moved or resized), the service narrows the clip
rectangle to that window's content and asks the application to repaint through an
`expose` notification; the application redraws its content with the same `syslib` calls;
the service then presents the union of the affected rectangles. There is exactly one
image at any time.

M2.1 (this slice) implements the shadow, the clip region, `present`, the console path and
the `syslib` primitives. M2.2 adds the mouse and events. M2.3 adds windows, z-order and
`expose`.

## Consequences

### Positive

- One copy into the shadow and one copy out per presented rectangle; no canvas per
  window, no double buffering per client.
- `get_pixel` and the console read and write the same image, so a self-check is
  meaningful and the screen can never disagree with what an application drew.
- The application-facing surface is small, typed and mockable; host tests see a 0×0
  screen through the trait defaults and need no framebuffer.
- The kernel already serialises drawing with a spin lock, so SMP needs no new protocol.
- Resolution independence is free: the shadow is allocated from the returned mode.

### Negative

- The window server is in the trusted computing base: a drawing bug can corrupt the
  screen or the kernel, with no recovery boundary.
- A client cannot keep a backing store in its own memory (the language has no
  shared mappings), so repaint cost is paid by the client on every `expose`.
- The screen is a global resource; a misbehaving application can paint outside its
  window when the clip is not narrowed. `syslib` has no capability model (vision R14),
  and the window manager's clip is advisory until M2.3 makes it the only path.
- Screen latency includes the shadow copy; a full 1024×768 present is about 786k
  pixel writes.

### Neutral / follow-up work

- Dirty-rectangle tracking is an optimisation, not a correctness requirement.
- A soft cursor (painted into the shadow) is the simplest M2.2 shape; a hardware cursor
  is unnecessary under QEMU and would add device-specific state.
- The clip region is internal in M2.1. Exposing a set/reset pair to applications is an
  M2.3 decision.

## Alternatives considered

| Alternative | Why not |
|-------------|---------|
| User-space window server over `in_stream`/`out_stream` | Pixels through a byte FIFO cost a copy per byte, there is no shared canvas, and the protocol would need a new host interface larger than the service it replaces |
| Per-window canvases in kernel memory | More copies and several images to keep consistent; no benefit without shared memory or per-window isolation |
| Draw directly to the framebuffer, keep no shadow | `get_pixel`, clipping, repaint and window content all become impossible or need a shadow anyway |
| One opcode per primitive, rasterised in bytecode | Rasterisation is host work with hardware-adjacent structure; `BUILTIN` plus `Host` defaults already provides the mockable staging point |
| Application provides its own framebuffer address | Raw addresses are forbidden (`0005-host-interface.md` H3) and the address space model has no view/aliasing |
