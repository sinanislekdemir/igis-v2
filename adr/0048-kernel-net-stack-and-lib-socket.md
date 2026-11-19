# 0048: Keep the IPv4 wire stack in the kernel and expose a `/lib/socket` unit

- **Status:** Accepted
- **Date:** 2026-11-19
- **Deciders:** igis-v2 kernel/runtime
- **Related specs:** 0020 (networking), 0003 (language), 0005 (host interface), 0006
  (streams), 0007 (scheduler), 0019 (home filesystem), 0018 (modules)
- **Related ADRs:** 0001 (ring 0), 0002 (interpreted apps), 0004/0007 (static memory),
  0019 (source-only delivery), 0023 (IRQ drain rule), 0026 (`stdlib` may use `syslib`),
  0039 (package-wide types)

## Context

The system needs basic internet: ping, UDP, a TCP client, DNS and HTTP. The project's
constraints remove the usual options:

- There is no allocator (ADR 0004/0007). Packet buffers, retransmission state and
  socket queues must be fixed static objects.
- Every application is interpreted source (ADR 0002/0019). A TCP state machine,
  checksum loops and a 1 ms timer in `.ig` would be slow, would consume a task's CPU
  budget, and would need bitwise operations the language does not have (`div`/`mod`
  only).
- The host interface is closed and enumerated (spec 0005 H1/H3). A socket must cross
  it as an opaque handle with caller-owned buffers.
- There is no privilege boundary (ADR 0001, vision R14), so the kernel is not
  protecting applications from each other's network access. It is only keeping the
  wire work in the trusted core.
- The owner's direction for this milestone is explicit: the `socket` unit must live
  on the program/interpreter side, as a `/lib` module that applications import.

The question is therefore not "kernel vs user space" in a conventional sense. It is
where to draw the line between the Rust wire stack and the source-level application
surface, and what stays testable without a kernel.

## Decision

We will keep Ethernet, ARP, IPv4, ICMP, UDP and TCP in a new allocation-free `net`
crate used by the kernel, and put the application-facing surface in
`samples/apps/lib/socket.ig` as an ordinary `.ig` unit.

### The line

- **`net` crate (Rust, `no_std`, `#![forbid(unsafe_code)]`):** frame parsing and
  construction, checksums, ARP, IPv4, ICMP, UDP, TCP, the socket table and protocol
  timers. It depends only on `abi` and a `Link`/clock seam, so it is host-tested
  against a simulated device exactly as `ext2w` is host-tested against a
  `SectorDevice`.
- **Kernel (`kernel/src/net`):** the device seam, virtio-net/e1000 drivers, static
  DMA, the service-opportunity call, the interrupt drain and the scheduler bridge.
- **Interpreter/ABI:** a small set of `core.net_*`/`core.socket_*` host builtins,
  opaque handles, `WaitFor::Net`/`WaitFor::Deadline` and the `socket_source` tag.
- **`/lib` source:** `socket.ig` (the unit), `dns.ig` and `http.ig`. These are
  ordinary modules compiled into the application package (unit ids 2..=16), with no
  `main`, no top-level `var` and no stream. They keep no protocol state.

### Why the socket unit is source

The unit is the part an application author sees and the part the thesis is about: a
personal system whose applications are source. It also keeps the kernel's ABI small
(functions can be added to the unit without a new builtin) and lets a host mock run
the same code on `langhost`. It is not a second implementation of TCP: every
function maps to one builtin.

### Why the wire stack is Rust

Checksums, sequence arithmetic, retransmission deadlines and packet parsing are
exactly the kind of code that must be bounded, allocation-free and unit-testable
without a kernel. Making them source would either duplicate them in every
application or create a "network daemon" with a stream protocol and a liveness
dependency, which is the design ADR 0020/0047 already rejected for windows and
input.

### DNS and HTTP stay in source

DNS message parsing and HTTP request/response handling are not performance-critical
wire work. They sit on top of the socket unit and prove that the unit is usable from
ordinary applications. A future `lib/http` can grow without touching the kernel.

## Consequences

### Positive

- The kernel gains one host-testable protocol crate instead of a large kernel module;
  packet code can be tested on the host with no QEMU.
- Applications get a stable, source-level `socket` unit; adding a helper does not
  change the ABI or the kernel.
- The wire stack has one implementation, one static memory budget and one set of
  counters, so a malformed packet has one place to be rejected.
- DNS and HTTP are ordinary modules, consistent with `file_dialog.ig`, `math.ig` and
  the rest of `/lib`.

### Negative

- The ABI still grows: the builtins, the wait conditions and the source tag are
  permanent surface. The unit cannot hide a missing builtin.
- The `net` crate must stay `no_std` and allocation-free, so every table size is a
  compile-time constant and a full table is a runtime `Full`/`WouldBlock`.
- Protocol bugs are kernel bugs: a bad sequence calculation can corrupt a connection
  or panic the kernel if a slice is unchecked. The crate's `forbid(unsafe_code)` and
  checked slices are the mitigation, not a proof.
- The source units have fixed capacities (Q46), so a 1024-byte `socket.recv` helper
  cannot serve every destination; callers with larger buffers use `core.socket_recv`.
- An application that wants a new transport feature still needs a kernel/runtime
  change; the "programmable system" claim stops at the wire.

### Neutral / follow-up work

- `cargo test -p net` joins the host suite; the kernel transcript remains the
  end-to-end proof.
- Spec 0005 gains the networking primitives when M10.3 lands; spec 0020 governs the
  slice until then.
- The socket unit is at `samples/apps/lib/socket.ig`; the DNS and HTTP units are
  siblings. A future `lib/net` namespace would need a directory, not a new decision.

## Alternatives considered

| Alternative | Why not |
|---|---|
| The whole stack in `/lib` `.ig` | Interpreter cost, missing bitwise operations, duplicated in every app, and a task budget spent on retransmission loops. The device seam and the source units already make the system programmable where it matters. |
| The whole stack plus DNS/HTTP in the kernel | Ignores the owner's explicit "socket unit on the program/interpreter side", bloats the trusted core, and turns every HTTP tweak into a kernel change. |
| A network server task with a stream protocol | Adds a hop, a second scheduler dependency and a liveness failure mode (the daemon must be alive for any app to use the network), with no isolation benefit at ring 0. |
| A separate `netd` in Rust linked into the kernel but with its own entry point | Same code, extra boot coupling; the service-opportunity call is simpler. |
| Keep the wire stack in the kernel but expose raw packets to `/lib` | Raw packet APIs need an address-crossing model and checksum/ARP logic in every client; the socket unit would then be a protocol implementation, not a thin wrapper. |
