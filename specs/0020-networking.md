# 0020: Networking — IPv4, UDP, TCP and the `socket` unit

- **Status:** Implemented
- **Scope:** `net` (new crate), `kernel` (pci, net, sched, task, main), `arch/x86_64`
  (PCI, I/O APIC, IDT), `abi`, `compiler`, `interpreter`, `langhost`, `xtask`,
  `samples/apps/lib`, `samples/apps/base`, `scripts`
- **Supersedes:** none
- **Superseded by:** none
- **ADRs:** 0048, 0049, 0050
- **Updated:** 2026-11-19

## Context

The system has no network path: no NIC driver, no link layer, no IP, no transport,
and no application-visible socket. The personal-system goal needs "basic internet":
resolve a name, open a TCP connection, speak HTTP, and exchange UDP datagrams.

The ordinary choices do not survive the project's constraints unchanged:

- **No allocation, ever** (ADR 0004/0007). Packet queues, socket state, ARP entries
  and TCP retransmission state must live in fixed static tables and arenas sized at
  build time.
- **No privilege boundary** (ADR 0001, vision R14). Any application may configure
  the NIC, open sockets and read any socket it owns; there is no capability check.
- **Applications are interpreted source** (ADR 0002/0019). A TCP state machine
  written in `.ig` would pay interpreter costs for checksum loops, timers and
  retransmission, and would need bitwise operations the language does not have.
  The wire work therefore belongs in Rust.
- **The host interface is closed and enumerated** (spec 0005 H1/H3/H7). A socket
  crosses it as an opaque handle plus caller-owned buffers, never as an address.
- **Device interrupts are short and take no kernel lock** (spec 0002, ADR 0023).
  A NIC handler may drain device state into a lock-free ring; protocol work stays
  in the service opportunity.
- **QEMU/TCG is the deterministic test target**, but the owner also wants a future
  path to VMware and other hardware. VMware guests typically expose `e1000e` or
  `VMXNET3`, not `virtio-net`; the device seam must not assume virtio.

The owner's direction is explicit: the **`socket` unit lives on the program /
interpreter side** (`samples/apps/lib/socket.ig`), the wire stack lives in the
kernel, and the NIC layer must leave room for future drivers.

**Non-goals for M10.** IPv6, TLS/HTTPS, 802.11/Wi-Fi, PPP, NAT or routing between
interfaces, multiple simultaneous NICs, raw sockets, broadcast/multicast groups,
IP fragmentation and reassembly, TCP options beyond MSS (no window scaling, SACK,
timestamps or urgent data), congestion control beyond a fixed small window,
zero-copy, and a userspace network stack. DHCP and a second NIC driver are planned
as late M10 tickets, not prerequisites.

## Requirements

### The device seam

- **N1 (MUST):** The kernel reaches a NIC through one device seam
  (`net::device::NetDevice`) that exposes the MAC address, link state, one frame
  transmit and one frame receive. The IP/transport layers never touch a register.
- **N2 (MUST):** The first driver is `virtio-net` over the legacy (transitional)
  I/O BAR on PCI. A second driver (`e1000`/`e1000e`, later `VMXNET3`) MUST be
  addable behind N1 without changing the Ethernet/IP/UDP/TCP code.
- **N3 (MUST):** Device buffers, virtqueues, descriptor tables and DMA addresses
  are static `.bss` objects, page-aligned where the device requires it, and
  identity-mapped (virtual address equals physical address, as `ata.rs` and
  `usb.rs` already rely on). No allocation and no `alloc` crate.
- **N4 (MUST):** The driver is polled in the service opportunity. M10.6 adds one
  PCI INTx line routed through the I/O APIC to the boot core as vector `0x2D`.
  The handler is bounded and lock-free: it sets a pending flag, acknowledges the
  device with one read-to-clear register access (virtio ISR / e1000 `ICR`) and
  EOIs the local APIC. It does **not** walk the rings; the used-ring drain and
  every protocol step stay in the service opportunity, because the handler may
  interrupt a core that already holds the network lock and the device rings are
  not safe to walk from two contexts. Polling therefore remains both the
  fallback when the route is unavailable (or the boot asks for it) and a
  harmless second chance: a missed interrupt costs at most one service period.

### The wire stack

- **N5 (MUST):** Ethernet II framing with the device MAC, a 16-entry ARP cache
  (60 s aging, three requests, one queued packet per unresolved next hop).
- **N6 (MUST):** IPv4 with one machine-wide address, netmask and default gateway;
  TTL 64, header and total-length validation, header checksum, and a drop counter.
  A packet that is not IPv4, is fragmented, has a bad checksum, or is not for the
  local address or a broadcast address is dropped without touching a socket.
- **N7 (MUST):** ICMP echo request/reply (type 0/8) with checksum, so the stack is
  provable with a ping before any socket exists.
- **N8 (MUST):** UDP with a fixed socket table, per-socket receive queues, port
  demultiplexing, checksum validation (a zero checksum is accepted as "not
  supplied"), and connected sockets only (one peer address/port per socket). A UDP
  socket may bind an explicit local port (DHCP needs 68) and may send to the
  broadcast address; broadcast datagrams are accepted for a socket bound to the
  matching port, and a socket opened to the broadcast address accepts a reply from
  any source (the DHCP exchange validates the transaction id). There is no
  unconnected `sendto`/`recvfrom` surface in M10.
- **N9 (MUST):** TCP client with active open, the three-way handshake, sequence
  and acknowledgement numbers, MSS 1460, a fixed receive window (4096 or 8192,
  no scaling), one outstanding segment per connection (stop-and-wait), a 1 s
  initial retransmission timeout with exponential backoff and at most five
  attempts, RST handling, and FIN close. Out-of-order segments are dropped and
  recovered by retransmission; there is no reassembly queue.
- **N10 (MUST):** TCP listen/accept is added in M10.7: a listener has a two-entry
  pending queue; `accept` returns a connected socket or `WouldBlock`. A client
  connection and a listener may not share one slot.
- **N11 (MUST):** Checksums (IPv4, UDP, TCP pseudo-header) are computed in Rust
  with one's-complement arithmetic. Hardware offload is not negotiated.

### The application surface

- **N12 (MUST):** Sockets are opaque handles owned by the task that opened them.
  A handle is `(generation << 8) | index`, `0` is invalid, and a stale handle can
  never reach a recycled slot. A foreign or stale handle is `Invalid`/`NotFound`.
- **N13 (MUST):** The blocked operation is a new wait condition
  (`WaitFor::Net(handle)`); a timed block is `WaitFor::Deadline(ns)`. Both wake
  through the existing scheduler, and a socket close, EOF, error or timeout wakes
  every waiter. `next_any`/`poll_any` accept a socket source returned by
  `core.socket_source(id)` (`1 << 63 | handle`), and `Host` gains
  `socket_readable`.
  **Implemented:** a host `WouldBlock` from `socket_send`/`recv`/`accept` makes
  the interpreter return `Outcome::Blocked` and retry the same instruction after
  the wake. The stack records a per-socket deadline (`core.socket_timeout`,
  builtin 71; `0` = none, default 30 s; the TCP connect keeps its 5 s `Closed`);
  `net::service` expires it and calls `task::wake_net` after dropping the network
  lock, and the scheduler's post-block probe uses the recorded direction.
  `WaitFor::Deadline(ns)` is a generic timer-woken arm; socket timeouts use the
  per-socket arm. `kill_stress` and `net_select` are the concurrency proofs.
- **N14 (MUST):** A task's sockets close when the task terminates, on every
  destruction path, before its regions are freed. A blocked peer is woken with
  `Closed`; queued data is discarded.
- **N15 (MUST):** `samples/apps/lib/socket.ig` is the program-side unit
  (`unit socket;`, `uses core, stdlib;`, no `main`, no top-level `var`, no
  stream). It wraps the `core.socket_*` builtins; it does not reimplement
  protocol logic and does not depend on the kernel beyond the host interface.
- **N16 (MUST):** DNS A-record resolution (`samples/apps/lib/dns.ig`) and a
  minimal HTTP/1.0 GET (`samples/apps/lib/http.ig`) are ordinary `.ig` source on
  top of the socket unit, not kernel features.
- **N17 (MUST):** No opcode is added. The ABI v1 opcode table stays at 75; the
  socket primitives are host builtins with new ids starting at 62.

### Configuration, proofs and regressions

- **N18 (MUST):** One machine-wide static configuration (address, netmask,
  gateway, DNS server) is readable and writable from source through
  `core.net_config`/`core.net_info` (plus `core.net_mac` for the device address).
  M10 uses static values (QEMU: `10.0.2.15/24`, gateway `10.0.2.2`, DNS
  `10.0.2.3`); a DHCP client follows in M10.8.
- **N19 (MUST):** `xtask run` attaches one user-mode NIC by default and accepts
  `--no-net` (no device, graceful absence), `--net-device
  virtio|e1000|e1000e` and `--net-irq intx|poll` (M10.6). `--net-irq intx|poll` (M10.6) selects the
  interrupt path: `poll` writes `igis.net_irq=poll` into the boot loader's
  command line, so the same kernel image skips the INTx route and proves the
  polling fallback. The deterministic harness starts a host
  TCP service and uses a guest-forwarded address; external internet access is an
  optional run, not part of the regression suite.
- **N20 (MUST):** Proofs run on `-smp 4` and `-smp 1`: `net_probe check` (stack up,
  MAC, config, checksum vectors), `net_probe ping` (gateway), `net_probe udp`
  (host echo or a DNS query), `net_probe http` (host-forwarded HTTP/1.0 GET),
  `net_probe dns` (A record), `net_echo` (M10.7 listener), `net_probe dhcp`
  (M10.8) and the e1000 run (M10.9). No existing sample regresses and the visual
  golden is unchanged.
- **N21 (MUST):** A machine without a NIC, with link down, or with a refused
  route behaves deterministically: `core.net_*` returns `Invalid`/`IoError`, a
  blocking call times out with `Closed`/`WouldBlock`, the boot log says so, and
  nothing panics or spins.
- **N22 (SHOULD):** `net` exposes bounded counters (RX/TX frames, drops, protocol
  errors) readable through `core.net_stats(var rx, var tx, var drops, var
  errors) -> Status` (builtin 73, M10.6), so failures are diagnosable without a
  debugger and an application can prove that traffic moved. `base/net_perf` is
  the proof application: ten timed HTTP gets with the counters before and after.

## Design

### Layers and ownership

```text
application (.ig source)
  uses lib.http; lib.dns; lib.socket;
        │
        ▼
  lib/socket.ig   (unit socket)      ← the program/interpreter side
  lib/dns.ig      (unit dns)
  lib/http.ig     (unit http)
        │  core.socket_* / core.net_*   (host builtins)
        ▼
  interpreter Host trait  ──►  kernel::sched::KernelHost
        │                         │
        │                         ▼
        │                    kernel::net  (stack facade, static table)
        │                         │
        │                    net crate: ethernet → arp → ipv4 → icmp
        │                              └────────► udp / tcp (socket table)
        │                         │
        │                    NetDevice seam
        │                    ├── virtio-net (M10.1)
        │                    └── e1000/e1000e (M10.9)
        ▼
  scheduler: WaitFor::Net / WaitFor::Deadline
```

The stack lives in a new `net` crate (`no_std`, `#![forbid(unsafe_code)]`,
allocation-free, depending only on `abi`). It owns protocol state and packet
parsing/building; it is host-testable against a simulated `Link` and a virtual
clock, exactly as `ext2w` is host-tested against a `SectorDevice`. The kernel
owns the device, DMA and the scheduler bridge. The application surface is the
`/lib` units and the enumerated builtins.

### Static budget (initial sizes)

| Object | Count × size | Bytes |
|---|---|---|
| Socket slots | 8 × (state + 4 KiB RX + 4 KiB TX) | ~64 KiB |
| TCP connections / listeners | 4 / 2 (inside the 8 slots) | — |
| RX frames | 32 × 2 KiB | 64 KiB |
| TX frames | 16 × 2 KiB | 32 KiB |
| Virtqueue descriptors | 128 | 2 KiB |
| ARP cache | 16 entries | ~512 B |
| `net` protocol state | one `Stack` | ~8 KiB |

The exact sizes are implementation constants recorded beside the arrays; M10.1
prints the static cost and the remaining free frames at boot. Socket payload
buffers are per-slot arenas, never mapped into an application region.

### Device seam

```rust
// illustrative; the kernel selects one device at boot
pub trait NetDevice {
    fn mac(&self) -> [u8; 6];
    fn link_up(&self) -> bool;
    fn transmit(&mut self, frame: &[u8]) -> Status;
    /// Copies one received frame into `out`; `None` when the ring is empty.
    fn receive(&mut self, out: &mut [u8]) -> Option<usize>;
    fn poll(&mut self);
    fn stats(&self) -> DeviceStats;
}
```

The kernel keeps one `Device` value in a `SpinLock` (concrete enum dispatch is
acceptable and avoids a trait-object vtable in the static path). `net::service()`
runs in the service opportunity next to `mouse::poll`: it drains the device,
passes frames to the `Stack`, runs protocol timers, and wakes blocked tasks. The
NIC handler (M10.6) sets a pending flag, acknowledges the device with one
read-to-clear register access and EOIs; the used-ring drain stays in the service
opportunity (ADR 0051).

### Packet flow

- **Transmit:** a socket write copies from the caller's buffer into the slot's TX
  arena, the transport layer writes the header/checksum, ARP resolves the next
  hop into a 6-byte MAC, and the device transmits one frame. A full device TX
  ring returns `WouldBlock`; the service opportunity retries.
- **Receive:** the device delivers one Ethernet frame; `ethernet` checks the
  destination MAC (local, broadcast or the ARP request path); `arp` consumes ARP;
  `ipv4` validates and dispatches to `icmp`, `udp` or `tcp`; the transport copies
  the payload into the owning socket's RX arena and wakes its waiters. A frame
  that names no live socket is dropped and counted; ICMP unreachable is not sent.

### Socket model

- `Kind::Tcp` (active open), `Kind::Udp` (connected datagram), `Kind::Listen`
  (M10.7). `open` for TCP is a blocking connect with a 5 s default deadline;
  `open` for UDP returns immediately; `listen` binds a port.
- A slot stores owner task id, generation, kind, local/remote address and port,
  state, `rx`/`tx` arenas, sequence variables, deadline and flags.
- TCP: one unacknowledged segment, `snd_nxt`/`snd_una`, `rcv_nxt`, fixed receive
  window, no delayed ACK (ACK every segment), no Nagle (send immediately). A FIN
  in either direction moves through the minimal close states; a RST is `Closed`.
- UDP: one datagram per send; a received datagram is copied whole or truncated to
  the destination capacity, with `read` reporting the copied length and the rest
  discarded (the datagram boundary is preserved).

### Blocking and readiness

`WaitFor::Net(handle)` is recorded by the interpreter when a blocking socket
operation returns `WouldBlock`; the kernel's `wake_net(handle)` wakes matching
tasks. `WaitFor::Deadline(ns)` is the timeout arm, serviced by the existing 1 kHz
tick (the tick only marks expiry; the service opportunity performs the work).
Readiness for `next_any` is a snapshot, not a reservation: the following
`socket_recv` is authoritative, the same contract streams already use.

A socket is **selector-ready** when it has queued data, reached EOF, failed,
timed out, or (for a listener) has a pending connection. A bare completed
connect is deliberately not a selector source: the first blocking `recv` on an
empty connection still waits for data, so reporting the connect would spin a
selector and, in a console loop like `nc`, block the task where a console byte
can no longer wake it. `Stack::readable` still reports connect completion for
callers that ask for it. A socket is writable when the TX arena has space;
`poll_any`/`next_any` model read sources only.

### Configuration and name resolution

`core.net_config(ip, mask, gateway, dns)` sets the machine-wide values;
`core.net_info(var ip, var mask, var gateway, var dns)` reads them, and
`core.net_mac() -> uint` reads the device address (the six bytes packed big-endian,
`52:54:00:12:34:56` is `0x5254_0012_3456`; `0` when there is no device). IPv4
addresses are
`uint` in host order with the first octet in bits 24..31 (`10.0.2.15` is
`0x0A00_020F`); ports are `uint` in `0..=65535`. There is no routing table: a
destination is local when `(dst & mask) == (ip & mask)`, otherwise the gateway is
used; the gateway itself must be local.

`base/ifconfig` is the command-line view of this surface: no argument prints the
interface block (MAC, IPv4/prefix/netmask, gateway, DNS and the wire counters),
`dhcp` acquires a lease with `lib/dhcp` and applies it with `core.net_config`, and
`<ip> <netmask> <gateway> [dns]` applies a static configuration. `base/nslookup`
resolves one A record through `lib/dns`, defaulting the server to the configured
DNS and accepting an explicit second argument; a numeric name is answered
locally. Neither command needs a new builtin.

`lib/dns.ig` builds a standard query (type A, class IN, recursion desired), sends
it to the configured server over a UDP socket, validates the transaction id and
parses the first A record, with two retries and a 2 s deadline. It returns a
`uint` address; CNAME chains are followed up to four links. `lib/http.ig` sends
`GET <path> HTTP/1.0`, `Host: <host>`, `Connection: close`, reads until FIN or a
capacity limit, and returns the body after the first `\r\n\r\n`. Redirects,
chunked encoding and TLS are out of scope.

### The `socket` unit

The program-side unit is deliberately thin; every function maps to one builtin
and keeps no state. Its fixed capacities are the language's answer to the
no-runtime-capacity-buffer rule (Q46): a caller that needs another size calls
`core.socket_recv` directly with its own `bytes[N]`.

```text
unit socket;
uses core, stdlib;

function tcp(ip: uint; port: uint; var id: int) -> Status;
function udp(ip: uint; port: uint; var id: int) -> Status;
function udp_from(local: uint; ip: uint; port: uint; var id: int) -> Status;
function listen(port: uint; var id: int) -> Status;              { M10.7 }
function accept(id: int; var peer: int) -> Status;              { M10.7 }
function close(id: int) -> Status;
function set_timeout(id: int; ms: uint) -> Status;
function send(id: int; data: const bytes[0]; var sent: int) -> Status;
function send_text(id: int; text: const string[0]; var sent: int) -> Status;
function recv(id: int; var dst: bytes[1024]; var read: int) -> Status;
function recv_line(id: int; var dst: string[256]) -> Status;
function source(id: int) -> uint;
```

`send_text` stages the whole string into a local `bytes[1024]`; a larger send
uses a caller-owned `bytes[N]` and `core.socket_send`. `recv_line` accumulates
until `\n`, strips a trailing `\r`, and returns `CapacityExceeded` while keeping
the stream synchronised (the `stdlib.recv_line_into` rule). The module has no
`main`, no top-level `var` and no stream endpoint, so it follows the M6 module
rules and links like any other `/lib` unit.

## Interfaces

### `core` builtins (no opcode; ABI v1 stays at 75)

| ID | Signature | Milestone | Notes |
|---|---|---|---|
| 62 | `core.net_config(ip, mask, gateway, dns) -> Status` | M10.2 | machine-wide, last writer wins |
| 63 | `core.net_info(var ip, var mask, var gateway, var dns) -> Status` | M10.2 | untouched on failure |
| 64 | `core.net_mac() -> uint` | M10.1 | packed MAC, `0` when absent |
| 65 | `core.socket_open(kind, ip, port, local, var id) -> Status` | M10.3 | kind 1 TCP (M10.4), 2 UDP, 3 listen (M10.7); `local` 0 = ephemeral, listener binds it |
| 66 | `core.socket_send(id, data, var sent) -> Status` | M10.3 | blocking sends all; UDP is one datagram; the try variant may send a prefix |
| 67 | `core.socket_recv(id, var dst, var read) -> Status` | M10.3 | non-blocking first slice; `Ok`/0 is EOF; the compiler stages `dst` capacity as a hidden scalar |
| 68 | `core.socket_try_send(id, data, var sent) -> Status` | M10.3 | `WouldBlock`, never blocks |
| 69 | `core.socket_try_recv(id, var dst, var read) -> Status` | M10.3 | `WouldBlock` when empty |
| 70 | `core.socket_close(id) -> Status` | M10.3 | owner-only |
| 71 | `core.socket_timeout(id, ms) -> Status` | M10.4 | `0` = no timeout; default per operation |
| 72 | `core.socket_accept(id, var peer) -> Status` | M10.7 | listener only |
| 73 | `core.net_stats(var rx, var tx, var drops, var errors) -> Status` | M10.6 | optional but SHOULD |

`socket_send` accepts a `string` or a `bytes` buffer (the kind is staged in the
operand word, like `SUBSTR`); `socket_recv` accepts any `bytes[N]` destination and
the compiler emits its capacity. `core.socket_source(id)` is compiler-lowered
(`ref` + `load_ref`), like `core.stream_source`, returning
`abi::wait::SOURCE_SOCKET_BASE | handle`.

Defaults: connect 5 s; send/recv 30 s once M10.4 adds deadlines; `ms = 0` means
block indefinitely. A timeout is `Closed` for a connection that never opened and
`WouldBlock` for a send/recv that can be retried.

### Status mapping

`Ok` success; `WouldBlock` no progress on a try/partial operation; `Closed` peer
FIN/RST, refused connect, or timeout; `Invalid` foreign/stale handle, bad kind,
port or address; `NotFound` unknown socket; `Full` all slots in use; `IoError`
no device or link down; `CapacityExceeded` a datagram or send larger than the
slot's TX arena.

### `net` crate (host-tested)

```rust
pub struct Stack { /* static tables */ }
impl Stack {
    pub fn new(config: Config) -> Self;
    pub fn open(&mut self, kind: Kind, ip: u32, port: u16, local: u16, owner: u64)
        -> Result<Handle, Status>;
    pub fn send(&mut self, h: Handle, data: &[u8], now_ns: u64) -> Result<usize, Status>;
    pub fn recv(&mut self, h: Handle, dst: &mut [u8]) -> Result<usize, Status>;
    pub fn close(&mut self, h: Handle) -> Status;
    pub fn readable(&self, h: Handle) -> bool;
    pub fn poll(&mut self, link: &mut dyn Link, now_ns: u64);
    pub fn tick(&mut self, now_ns: u64);
}
```

Host tests cover header construction/parsing, checksums (including the `0xFFFF`
fold), ARP cache behaviour, UDP demultiplexing, the TCP handshake/retransmit/FIN
transitions against a simulated peer, and the timeout arithmetic. The `/lib` DNS
parser and HTTP request builder are tested through `langhost` with canned packets,
not in the wire crate. `cargo test -p net` joins the host suite.

### `xtask` and harness

`xtask run` adds `-netdev user,id=n0` and
`-device virtio-net-pci,netdev=n0,disable-modern=on` by default; `--no-net` omits
both and proves the graceful-absence path. The deterministic harness
`scripts/net_qmp.py` starts a host TCP service and passes a guest-forwarded
address (the exact `guestfwd` form is fixed by the M10.4 ticket), boots
`build/igis-shell.iso`, runs the probe, and checks the serial verdict on both
core counts. `net_probe internet` is an optional manual run that resolves and
fetches a real name; it is never part of the regression suite.

Boot lines: `pci: ...` (existing), `net: virtio-net 1af4:1000 mac ...`,
`net: link up`, `net: config 10.0.2.15/24 gw 10.0.2.2`; a missing device is
`net: no device` and everything stays `Invalid`/`IoError`.

## Failure modes

- **No NIC / link down:** `core.socket_open` and `net_config` return `IoError`;
  `net_info`/`net_mac` report zero/absent; the boot log names the condition.
- **Malformed frame:** dropped before any socket, counted, never a panic.
- **ARP unresolved:** one packet queued per next hop, three requests, then drop
  and the blocking call times out.
- **Socket table full:** `Full`; no slot is reused while another task owns it.
- **TX/RX arena full:** `WouldBlock` (try) or a blocking wait; no overwrite.
- **TCP retransmit exhausted:** `Closed`; the slot returns to `Closed` and is
  reusable after `close`.
- **Task death:** every owned socket closes, waiters wake with `Closed`, buffers
  are not freed to the heap (there is none) but return to the slot table.
- **Foreign handle:** `Invalid`; a stale generation can never alias a new socket.
- **No time source / timer stopped:** a blocking call with a deadline never
  completes; the boot log and the task's normal CPU budget bound the damage. No
  busy-wait is added.

## Alternatives considered

| Alternative | Why not |
|---|---|
| The whole stack in `/lib` `.ig` | Checksums, timers and retransmission in the interpreter are slow and need bitwise operations the language lacks; every app would link the stack. The thesis is served by the source-level socket/DNS/HTTP units and the programmable device seam, not by moving the wire code. |
| The whole stack plus DNS/HTTP in the kernel | Contradicts the owner's "socket unit on the program/interpreter side" and bloats the trusted core; HTTP and DNS are ordinary parsing and belong in source. |
| A userspace network task | There is no process boundary; a task that owns the NIC would need a stream protocol and would add a hop and a liveness dependency, the same reasoning as ADR 0020/0047. |
| `e1000` first | More infrastructure (MMIO attributes, descriptor rings) before the first frame; virtio-net legacy is the shortest path on the primary QEMU target. e1000/e1000e follows behind N1 (M10.9). |
| Full TCP (congestion control, SACK, reassembly) | Not needed for a basic client and much harder to verify in a static, single-connection design; the stop-and-wait subset is honest about its limits. |
| One socket per task / per-task queues | A fixed table with ownership is simpler, matches streams/windows, and keeps the foreign-handle rule. |

## Open questions

| # | Question | Trigger / owner |
|---|----------|-----------------|
| 1 | TLS/HTTPS (needs crypto, entropy, certificates, a trust store) | first real web application; OPEN-QUESTIONS Q52 |
| 2 | IPv6 and dual stack | when an IPv6-only peer matters; Q53 |
| 3 | Wi-Fi/802.11 (firmware, association) | a laptop target; Q54 |
| 4 | Multiple NICs and a routing table | a second device or a real gateway; Q55 |
| 5 | TCP server backlog and multiple listeners | first server application; Q56 |
| 6 | DHCP lease renewal, DNS caching, ICMP errors | M10.8 follow-up; Q57 |
| 7 | A writable-source socket source for `poll_any` | when a proof needs send-space readiness |
| 8 | Exact `guestfwd` syntax and UDP-to-host behaviour under QEMU 10.2.2 | M10.3/M10.4 ticket |
| 9 | Network trust model: any task may reconfigure the NIC; sockets are owner-checked but there is no raw/promiscuous mode | first multi-user or remote-service target; Q58 |

## Changelog

| Date | Change |
|------|--------|
| 2026-11-19 | Initial draft for the M10 planning session: kernel wire stack, `net` crate, `/lib/socket` unit, virtio-net first behind a `NetDevice` seam, static configuration, DNS/HTTP in source, M10 ticket split. |
| 2026-11-19 | Accepted; implementation starts at M10.1. The first M10.3–M10.5 slice may use the non-blocking socket form (N13 staging) and `core.net_mac() -> uint`; blocking waits, `socket_source`, DNS, DHCP, the listener and e1000 follow in their tickets. |
| 2026-11-19 | M10.1–M10.4 first slice implemented (`e08745d`): legacy virtio-net, the `net` crate (Ethernet/ARP/IPv4/ICMP/UDP/TCP client), builtins 62–68, `lib/socket.ig`, `lib/http.ig` and `base/net_probe`; a real HTTP/1.0 GET to `10.0.2.2:6666` prints `net_probe: ok` on `-smp 4` and `-smp 1`. Non-blocking sockets; 553 host tests. |
| 2026-11-19 | M10.5 DNS in source (`2b8bd70`): `lib/dns.ig` builds/sends a standard A query over UDP and parses the response (id/QR/opcode/TC/RCODE, labels/compression, first A, CNAME chain, NXDOMAIN, timeouts); `net_probe dns` resolves `example.com` via `10.0.2.3` on `-smp 4`/`-smp 1`. No kernel/ABI change. |
| 2026-11-19 | M10.8 DHCP in source (`3752d17`): `lib/dhcp.ig` (DISCOVER/OFFER/REQUEST/ACK, local port 68, broadcast, options 1/3/6/50/53/54/55/51, 6 s cap, NAK restart) and `socket.udp_from`; `net_probe dhcp` applies `10.0.2.15/24`/`10.0.2.2`/`10.0.2.3` and then fetches the HTTP page on `-smp 4`/`-smp 1`. A broadcast send is allowed before an address is configured. |
| 2026-11-19 | M10.9 e1000/e1000e behind `NetDevice` (`c7989c4`): polling 82540EM/82574L drivers with uncached MMIO BAR0, 32 RX + 16 TX legacy rings and static buffers; `xtask --net-device virtio\|e1000\|e1000e`. HTTP, DNS and DHCP+HTTP pass over both on `-smp 4`/`-smp 1`; virtio and `--no-net` are unchanged. |
| 2026-11-19 | M10.6 INTx, the polling fallback and `core.net_stats`: one PCI INTx route (QEMU GSI 11) to the boot core as vector `0x2D`, a lock-free handler (pending flag, one read-to-clear device acknowledge, EOI) and the used-ring drain left in the service opportunity; `--net-irq intx\|poll` through the multiboot2 command line; builtin 73 `core.net_stats`; `base/net_perf` (ten timed HTTP gets with the counters before and after). Note for legacy virtio: the boot-time test frame can leave the PCI INTx level latched high, so `net::enable_irq` reads the ISR once to drop it — a level that never changes would swallow every later notification. |
| 2026-11-19 | M10.7 TCP listener/`accept` and the `net_echo` proof: `KIND_LISTEN` (open kind 3, nonzero bind port), a two-handle pending queue, inbound SYN → `SynReceived` + SYN-ACK, final ACK → `Established`/enqueue, backlog-full RST, SYN-ACK retry/5 s expiry, RST cleanup and listener close/owner cleanup; accepted connections use the normal TCP path. `core.socket_accept` is builtin 72; `lib/socket.listen/accept` and `samples/apps/base/net_echo.ig` are the program side; `scripts/net_echo_qmp.py` proves three host clients through `hostfwd` on `-smp 4`/`-smp 1`. 590 host tests; no TIME-WAIT, backlog 2. |
| 2026-11-19 | Blocking sockets and `socket_source`: `WaitFor::Net(handle)`/`WaitFor::Deadline(ns)`, `Outcome::Blocked` retry, per-socket `core.socket_timeout` (71; 0 = none, default 30 s, connect keeps 5 s `Closed`), compiler-lowered `core.socket_source` (`1 << 63 \| handle`) in `next_any`/`poll_any`, and the readiness-event/wake path that drops `NET` before `task::wake_net`. `net_select` proves the selector and 300 ms recv/select timeouts; 605 host tests; `kill_stress` 300/300. |
| 2026-11-19 | DNS host test: `samples/lang/dns_check.ig` (host-only, socket-free) exercises the query builder and `parse_response` on canned A/NXDOMAIN/CNAME/TC/truncated/overrun packets and `langhost/tests/e2e.rs` pins `dns_check: ok`; 606 host tests. M10 is complete. |
| 2026-11-19 | Selector readiness correction: `core.socket_source`/`next_any` uses `select_ready` (data/EOF/error/pending listener/timeout), not a bare completed connect; `Stack::readable` still reports connect completion for other callers. This prevents a selector/console loop (`nc`) from selecting an empty established socket and then blocking where a console byte cannot wake it. |
