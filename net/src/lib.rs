//! The allocation-free IPv4 wire stack (spec 0020, ADR 0048).
//!
//! This crate owns protocol state: Ethernet framing, ARP, IPv4, ICMP, UDP, TCP
//! and the socket table. It never touches a device register; frames and
//! transmission go through the caller's closures, so the same code runs in the
//! kernel over virtio-net and on the host against a simulated link.
//!
//! # Slices
//!
//! - **M10.1** defined the [`Stack`] type, the machine-wide [`Config`]/[`Stats`]
//!   values and the socket-table shape.
//! - **M10.2** added Ethernet, ARP, IPv4 and ICMP echo.
//! - **M10.3** added UDP sockets and the generation/owner handle rules.
//! - **M10.4** added the stop-and-wait TCP client used by the HTTP proof.
//! - **M10.7** added the TCP listener, the two-entry pending queue and `accept`.
//!
//! Every table is a fixed-size array sized at compile time, nothing here
//! allocates, and no malformed frame can panic: bad input is dropped and
//! counted in [`Stats`].
//!
//! # Deliberate limits
//!
//! One TCP segment may be unacknowledged at a time (stop-and-wait), so
//! [`Stack::send`] accepts at most [`MSS`] bytes and returns `WouldBlock`
//! afterwards. There is no reassembly queue: an out-of-order segment is
//! dropped and answered with an ACK for the expected sequence, so the peer
//! retransmits. A UDP socket holds exactly one datagram; a second datagram
//! arriving before `recv` is dropped and counted. A TCP listener accepts at most
//! two completed connections before `accept` drains one. TCP has no window
//! scaling, no SACK and no delayed ACK; it acknowledges every segment.

#![no_std]
#![forbid(unsafe_code)]

use abi::Status;

mod arp;
mod checksum;
mod ethernet;
mod icmp;
mod ipv4;
mod tcp;
mod udp;

use arp::ArpEntry;

/// Number of socket slots in the fixed table (spec 0020, "Static budget").
pub const MAX_SOCKETS: usize = 8;
/// TCP maximum segment size for the single fixed window (spec 0020, N9).
pub const MSS: usize = 1460;
/// Per-socket receive arena capacity.
pub const RX_CAP: usize = 4096;
/// Per-socket transmit arena capacity.
pub const TX_CAP: usize = 4096;
/// Active-open TCP socket kind.
pub const KIND_TCP: u64 = 1;
/// Connected UDP socket kind.
pub const KIND_UDP: u64 = 2;
/// Passive-open TCP listener kind (spec 0020, N10). `open` binds `local`; `ip`
/// and `port` are ignored.
pub const KIND_LISTEN: u64 = 3;
/// Completed connections a listener holds before `accept` drains one.
pub const LISTEN_BACKLOG: usize = 2;

/// Ethernet II header length.
pub(crate) const ETH_HEADER: usize = ethernet::HEADER_LEN;
/// IPv4 header length.
pub(crate) const IPV4_HEADER: usize = 20;
/// Largest frame this stack builds: headers plus one MSS segment.
pub(crate) const FRAME_MAX: usize = ETH_HEADER + IPV4_HEADER + 24 + MSS;
/// ARP ethertype.
pub(crate) const ETHERTYPE_ARP: u16 = 0x0806;
/// IPv4 ethertype.
pub(crate) const ETHERTYPE_IPV4: u16 = 0x0800;
/// The limited IPv4 broadcast address.
pub(crate) const BROADCAST_IP: u32 = 0xFFFF_FFFF;

/// ARP cache entries.
pub(crate) const ARP_ENTRIES: usize = 16;
/// How long a learned MAC stays fresh.
pub(crate) const ARP_TTL_NS: u64 = 60_000_000_000;
/// Minimum spacing between ARP requests for one next hop.
pub(crate) const ARP_RETRY_NS: u64 = 1_000_000_000;
/// Unanswered ARP requests before the stack gives up.
pub(crate) const ARP_MAX_REQUESTS: u8 = 3;
/// SYN retry interval while connecting.
pub(crate) const SYN_RETRY_NS: u64 = 200_000_000;
/// Deadline for the three-way handshake.
pub(crate) const CONNECT_TIMEOUT_NS: u64 = 5_000_000_000;
/// First retransmission timeout for data and FIN.
pub(crate) const INITIAL_RTO_NS: u64 = 1_000_000_000;
/// Transmissions of one segment before the connection is declared dead.
pub(crate) const MAX_ATTEMPTS: u8 = 5;
/// Default deadline for a blocking send/receive/accept when the application did
/// not call `core.socket_timeout` (spec 0020, "Blocking and readiness"). The
/// connect handshake keeps its own [`CONNECT_TIMEOUT_NS`].
pub const DEFAULT_OP_TIMEOUT_NS: u64 = 30_000_000_000;
/// First ephemeral local port.
const EPHEMERAL_START: u16 = 49152;
/// Number of ephemeral ports.
const EPHEMERAL_PORTS: u32 = 65536 - EPHEMERAL_START as u32;

/// One machine-wide IPv4 configuration (host order, first octet in bits 24..31).
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Config {
    pub ip: u32,
    pub mask: u32,
    pub gateway: u32,
    pub dns: u32,
}

/// Bounded protocol counters (spec 0020, N22).
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Stats {
    pub rx_frames: u64,
    pub tx_frames: u64,
    pub drops: u64,
    pub errors: u64,
}

/// Minimal TCP connection state (spec 0020, N9).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TcpState {
    /// Never connected, or finished.
    Closed,
    /// SYN sent, waiting for the SYN-ACK.
    SynSent,
    /// Bound and waiting for an inbound SYN.
    Listen,
    /// An inbound SYN was answered; waiting for the final ACK.
    SynReceived,
    /// The handshake completed.
    Established,
    /// Our FIN was sent; waiting for its ACK.
    FinWait1,
    /// Our FIN was acknowledged; waiting for the peer's FIN.
    FinWait2,
    /// The peer sent its FIN; our side is still open.
    CloseWait,
    /// We sent the FIN after the peer's; waiting for its ACK.
    LastAck,
}

/// One socket-table slot.
#[derive(Clone, Copy)]
struct Socket {
    active: bool,
    generation: u64,
    owner: u64,
    kind: u64,
    remote_ip: u32,
    remote_port: u16,
    local_port: u16,

    /// Receive arena and queued length (TCP bytes or one UDP datagram).
    rx: [u8; RX_CAP],
    rx_len: usize,
    udp_ready: bool,
    udp_len: usize,
    eof: bool,
    error: bool,

    state: TcpState,
    snd_una: u32,
    snd_nxt: u32,
    rcv_nxt: u32,
    /// The one unacknowledged segment.
    tx: [u8; TX_CAP],
    tx_len: usize,
    tx_sent: bool,
    tx_deadline: u64,
    tx_attempts: u8,
    /// The FIN, which is tracked separately so a data segment can precede it.
    fin_sent: bool,
    fin_deadline: u64,
    fin_attempts: u8,
    syn_started: bool,
    syn_next: u64,
    connect_deadline: u64,
    close_requested: bool,
    /// `recv` drained the arena; `service` re-advertises the window.
    window_dirty: bool,
    /// Completed connections waiting for `accept`: listener handles, oldest
    /// first. Only a [`KIND_LISTEN`] slot fills this.
    pending: [u64; LISTEN_BACKLOG],
    pending_len: usize,

    /// Configured per-operation deadline in nanoseconds (0 = block
    /// indefinitely). Set by [`Stack::set_timeout`].
    timeout_ns: u64,
    /// True once the application called `core.socket_timeout`, so an explicit
    /// `0` (indefinite) is distinguishable from the default.
    timeout_set: bool,
    /// Absolute deadline armed by the first `WouldBlock` of the operation in
    /// progress; 0 = not armed.
    op_deadline_ns: u64,
    /// Direction of the operation that armed the deadline, so the scheduler's
    /// post-block re-check probes the same readiness.
    wait_op: WaitOp,
    /// A deadline expired while the task was blocked. The interpreter consumes
    /// this with `socket_timed_out` and reports `WouldBlock` without blocking.
    timed_out: bool,
}

/// Which readiness a blocked socket operation is waiting for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum WaitOp {
    /// No blocking operation armed (a `next_any` read source, or idle).
    None,
    /// Data, EOF, an error or a pending listener connection.
    Read,
    /// Space in the transmit path and a resolved next hop.
    Write,
}

impl Socket {
    const EMPTY: Self = Self {
        active: false,
        generation: 0,
        owner: 0,
        kind: 0,
        remote_ip: 0,
        remote_port: 0,
        local_port: 0,
        rx: [0; RX_CAP],
        rx_len: 0,
        udp_ready: false,
        udp_len: 0,
        eof: false,
        error: false,
        state: TcpState::Closed,
        snd_una: 0,
        snd_nxt: 0,
        rcv_nxt: 0,
        tx: [0; TX_CAP],
        tx_len: 0,
        tx_sent: false,
        tx_deadline: 0,
        tx_attempts: 0,
        fin_sent: false,
        fin_deadline: 0,
        fin_attempts: 0,
        syn_started: false,
        syn_next: 0,
        connect_deadline: 0,
        close_requested: false,
        window_dirty: false,
        pending: [0; LISTEN_BACKLOG],
        pending_len: 0,
        timeout_ns: 0,
        timeout_set: false,
        op_deadline_ns: 0,
        wait_op: WaitOp::None,
        timed_out: false,
    };
}

/// The wire stack: one static instance, embedded by the kernel in a lock.
pub struct Stack {
    mac: [u8; 6],
    config: Config,
    stats: Stats,
    /// Next generation stamped into a reused slot (`(generation << 8) | index`).
    generation: u64,
    sockets: [Socket; MAX_SOCKETS],
    arp: [ArpEntry; ARP_ENTRIES],
    ip_id: u16,
    isn: u32,
    next_port: u16,
    /// Monotonic readiness-event counter: bumped whenever a frame changed a
    /// socket, a connection failed, or a deadline expired. The kernel compares
    /// it across service opportunities so it wakes waiters only after activity.
    events: u64,
}

impl Stack {
    /// Const constructor: the kernel embeds this in a static `SpinLock`.
    ///
    /// Every field is zero, so the 64 KiB of socket arenas live in `.bss` and
    /// do not enlarge the kernel image. The sequence-number base and the
    /// ephemeral-port cursor are seeded lazily on first use.
    pub const fn new() -> Self {
        Self {
            mac: [0; 6],
            config: Config {
                ip: 0,
                mask: 0,
                gateway: 0,
                dns: 0,
            },
            stats: Stats {
                rx_frames: 0,
                tx_frames: 0,
                drops: 0,
                errors: 0,
            },
            generation: 0,
            sockets: [Socket::EMPTY; MAX_SOCKETS],
            arp: [ArpEntry::EMPTY; ARP_ENTRIES],
            ip_id: 0,
            isn: 0,
            next_port: 0,
            events: 0,
        }
    }

    /// Stores the device address reported by the NIC driver.
    pub fn set_mac(&mut self, mac: [u8; 6]) {
        self.mac = mac;
    }

    /// Returns the device address; all zeros when no device is present.
    pub fn mac(&self) -> [u8; 6] {
        self.mac
    }

    /// Returns the current machine-wide configuration.
    pub fn config(&self) -> Config {
        self.config
    }

    /// Replaces the machine-wide configuration (last writer wins).
    pub fn set_config(&mut self, config: Config) {
        self.config = config;
    }

    /// Returns a snapshot of the bounded protocol counters.
    pub fn stats(&self) -> Stats {
        self.stats
    }

    /// Number of readiness events seen so far (frames that touched a socket,
    /// connection failures and expired deadlines). The kernel uses it to avoid
    /// walking the wake path on every service opportunity.
    pub fn event_seq(&self) -> u64 {
        self.events
    }

    /// Records one readiness-relevant transition.
    fn note_event(&mut self) {
        self.events = self.events.wrapping_add(1);
    }

    /// Sets the per-operation timeout in milliseconds (`0` = block
    /// indefinitely) and clears any armed deadline (spec 0020, N13).
    ///
    /// The timeout applies to the next blocking operation and is re-armed
    /// each time a send/receive/accept has to wait; an operation that completes
    /// clears it. `now_ns` is accepted so the same signature serves a call that
    /// wants to observe the clock, but the deadline is armed on the first
    /// `WouldBlock` (see [`Stack::arm_wait`]).
    pub fn set_timeout(
        &mut self,
        handle: u64,
        owner: u64,
        ms: u64,
        _now_ns: u64,
    ) -> Status {
        let index = match self.lookup(handle, owner) {
            Ok(index) => index,
            Err(status) => return status,
        };
        let socket = &mut self.sockets[index];
        socket.timeout_set = true;
        socket.timeout_ns = ms
            .min(abi::net::MAX_TIMEOUT_MS)
            .saturating_mul(1_000_000);
        socket.op_deadline_ns = 0;
        socket.timed_out = false;
        Status::Ok
    }

    /// Arms the deadline for a blocking operation that just returned
    /// `WouldBlock`.
    ///
    /// `write` selects the send (space) direction; a receive or accept waits
    /// for read readiness. The first call of an operation arms the deadline;
    /// later retries keep it. An explicit `core.socket_timeout(id, 0)` leaves
    /// the operation unbounded, as does the absence of a clock.
    pub fn arm_wait(&mut self, handle: u64, owner: u64, now_ns: u64, write: bool) -> Status {
        let index = match self.lookup(handle, owner) {
            Ok(index) => index,
            Err(status) => return status,
        };
        let socket = &mut self.sockets[index];
        socket.wait_op = if write { WaitOp::Write } else { WaitOp::Read };
        if socket.op_deadline_ns != 0 {
            return Status::Ok;
        }
        let timeout = if socket.timeout_set {
            socket.timeout_ns
        } else {
            DEFAULT_OP_TIMEOUT_NS
        };
        if timeout != 0 {
            socket.op_deadline_ns = now_ns.saturating_add(timeout);
        }
        Status::Ok
    }

    /// Ends the operation in progress: clears the armed deadline and the
    /// expiry flag. A completed or terminally failed call must not leave a
    /// stale timeout for the next one.
    pub fn finish_wait(&mut self, handle: u64, owner: u64) {
        if let Ok(index) = self.lookup(handle, owner) {
            let socket = &mut self.sockets[index];
            socket.op_deadline_ns = 0;
            socket.wait_op = WaitOp::None;
            socket.timed_out = false;
        }
    }

    /// Consumes an expired deadline for `handle`.
    ///
    /// Returns `true` once for an expiry, clearing the flag so the retry is
    /// allowed to block again if the application ignores the `WouldBlock`.
    pub fn take_timed_out(&mut self, handle: u64, owner: u64) -> bool {
        let index = match self.lookup(handle, owner) {
            Ok(index) => index,
            Err(_) => return false,
        };
        let socket = &mut self.sockets[index];
        let expired = socket.timed_out;
        if expired {
            socket.timed_out = false;
            socket.op_deadline_ns = 0;
            socket.wait_op = WaitOp::None;
        }
        expired
    }

    /// Readiness of the operation that armed the deadline: the scheduler's
    /// post-block re-check. Expiry and a terminal error count as ready so the
    /// retry can report them.
    pub fn wait_ready(&self, handle: u64, owner: u64, now_ns: u64) -> bool {
        let Ok(index) = self.lookup(handle, owner) else {
            return true;
        };
        self.slot_wait_ready(&self.sockets[index], now_ns)
    }

    /// True when the armed operation would not block: the read direction uses
    /// [`Stack::slot_recv_ready`] (not a bare established connect); the write
    /// direction is sendable, expired, or failed (a `Closed`/error wake must
    /// reach the blocked sender).
    fn slot_wait_ready(&self, socket: &Socket, now_ns: u64) -> bool {
        if socket.timed_out {
            return true;
        }
        match socket.wait_op {
            WaitOp::Write => {
                socket.error || self.slot_sendable(socket, now_ns)
            }
            WaitOp::Read => self.slot_recv_ready(socket),
            WaitOp::None => self.slot_readable(socket),
        }
    }

    /// Fills `out` with the handles whose armed operation is ready (or whose
    /// deadline expired), so the kernel can wake exactly those waiters after a
    /// service opportunity. Returns the number written.
    pub fn ready_handles(&self, now_ns: u64, out: &mut [u64; MAX_SOCKETS]) -> usize {
        let mut count = 0;
        for (index, socket) in self.sockets.iter().enumerate() {
            if !socket.active {
                continue;
            }
            let ready = self.slot_wait_ready(socket, now_ns);
            if ready {
                out[count] = (socket.generation << 8) | index as u64;
                count += 1;
            }
        }
        count
    }

    /// Read readiness of one slot, without the owner check.
    fn slot_readable(&self, socket: &Socket) -> bool {
        if socket.kind == KIND_LISTEN {
            return socket.pending_len > 0;
        }
        if socket.udp_ready || socket.rx_len > 0 || socket.eof || socket.error {
            return true;
        }
        socket.kind == KIND_TCP && socket.state == TcpState::Established
    }

    /// True when a `recv` on this slot would not block.
    ///
    /// Unlike [`Stack::slot_readable`], a bare completed connect is not enough:
    /// the first receive on an established, empty connection still waits for
    /// data. The distinction matters because the scheduler's post-block probe
    /// uses this predicate; treating `Established` as "receive ready" would
    /// wake the task forever.
    fn slot_recv_ready(&self, socket: &Socket) -> bool {
        if socket.kind == KIND_LISTEN {
            return socket.pending_len > 0;
        }
        if socket.udp_ready || socket.rx_len > 0 || socket.eof || socket.error {
            return true;
        }
        socket.kind == KIND_TCP && socket.state == TcpState::Closed
    }

    /// True when a send on this socket can make progress: the TCP handshake is
    /// complete with no unacknowledged segment, and the next hop's MAC is
    /// resolved (or the destination is broadcast). Pure: it never sends an ARP
    /// request, so it is safe in a readiness probe.
    fn slot_sendable(&self, socket: &Socket, now_ns: u64) -> bool {
        if socket.error || socket.kind == KIND_LISTEN {
            return false;
        }
        if socket.kind == KIND_TCP
            && (socket.state != TcpState::Established || socket.tx_sent || socket.fin_sent)
        {
            return false;
        }
        self.next_hop_resolved(socket.remote_ip, now_ns)
    }

    /// Pure form of the ARP next-hop check used by [`Stack::resolve_dst`].
    fn next_hop_resolved(&self, dst_ip: u32, now_ns: u64) -> bool {
        if dst_ip == BROADCAST_IP
            || (self.config.mask != 0 && dst_ip == self.broadcast_ip())
        {
            return true;
        }
        if self.config.ip == 0 {
            return false;
        }
        let next_hop = if self.on_link(dst_ip) {
            dst_ip
        } else {
            self.config.gateway
        };
        next_hop != 0 && self.arp_lookup(next_hop, now_ns).is_some()
    }

    /// Opens a socket. `kind` is [`KIND_TCP`] (active open), [`KIND_UDP`] or
    /// [`KIND_LISTEN`] (passive open); `ip`/`port` are the remote peer and
    /// `local` 0 asks for an ephemeral port. TCP returns immediately in
    /// SYN-SENT and `service` drives the handshake; UDP is ready at once. A
    /// listener requires a nonzero `local` bind port, ignores `ip`/`port` and
    /// waits for inbound SYNs on that port.
    pub fn open(
        &mut self,
        kind: u64,
        ip: u32,
        port: u16,
        local: u16,
        owner: u64,
    ) -> Result<u64, Status> {
        if kind != KIND_TCP && kind != KIND_UDP && kind != KIND_LISTEN {
            return Err(Status::Invalid);
        }
        if kind == KIND_LISTEN && local == 0 {
            return Err(Status::Invalid);
        }
        let mut free = None;
        for (index, socket) in self.sockets.iter().enumerate() {
            if !socket.active {
                free = Some(index);
                break;
            }
        }
        let index = free.ok_or(Status::Full)?;
        let local_port = self.alloc_port(kind, local)?;
        let generation = self.next_generation();
        let isn = if kind == KIND_TCP { self.next_isn() } else { 0 };

        let socket = &mut self.sockets[index];
        *socket = Socket::EMPTY;
        socket.active = true;
        socket.generation = generation;
        socket.owner = owner;
        socket.kind = kind;
        socket.local_port = local_port;
        if kind == KIND_LISTEN {
            // A listener has no peer and no remote state; each accepted
            // connection takes its own sequence number in `listener_syn`.
            socket.state = TcpState::Listen;
        } else {
            socket.remote_ip = ip;
            socket.remote_port = port;
        }
        if kind == KIND_TCP {
            socket.state = TcpState::SynSent;
            socket.snd_una = isn;
            socket.snd_nxt = isn.wrapping_add(1);
        }
        Ok((generation << 8) | index as u64)
    }

    /// Copies at most one TCP segment ([`MSS`]) or one UDP datagram.
    ///
    /// `WouldBlock` when the socket is not connected, the previous TCP segment
    /// is unacknowledged, the next hop is not resolved yet, or the device
    /// refused the frame. `Ok(n)` reports the accepted byte count. A listener
    /// is `Invalid`: it carries no byte stream.
    pub fn send(
        &mut self,
        handle: u64,
        owner: u64,
        data: &[u8],
        now_ns: u64,
        tx: &mut dyn FnMut(&[u8]) -> bool,
    ) -> Result<usize, Status> {
        let index = self.lookup(handle, owner)?;
        match self.sockets[index].kind {
            KIND_UDP => self.udp_send(index, data, now_ns, tx),
            KIND_TCP => self.tcp_send(index, data, now_ns, tx),
            _ => Err(Status::Invalid),
        }
    }

    /// Non-blocking receive. `Ok(n)` with `n > 0` is data; `Ok(0)` is EOF
    /// (TCP FIN); `WouldBlock` when nothing is queued and the connection is
    /// live.
    pub fn recv(&mut self, handle: u64, owner: u64, dst: &mut [u8]) -> Result<usize, Status> {
        let index = self.lookup(handle, owner)?;
        let socket = &mut self.sockets[index];
        if socket.kind == KIND_LISTEN {
            // A listener is not a datagram or byte stream; use `accept`.
            return Err(Status::Invalid);
        }
        if socket.kind == KIND_UDP {
            if !socket.udp_ready {
                return Err(Status::WouldBlock);
            }
            let copied = dst.len().min(socket.udp_len);
            dst[..copied].copy_from_slice(&socket.rx[..copied]);
            // The datagram boundary is preserved: anything past the
            // destination capacity is discarded.
            socket.udp_ready = false;
            socket.udp_len = 0;
            socket.rx_len = 0;
            return Ok(copied);
        }

        if socket.rx_len > 0 {
            let copied = dst.len().min(socket.rx_len);
            dst[..copied].copy_from_slice(&socket.rx[..copied]);
            socket.rx.copy_within(copied..socket.rx_len, 0);
            socket.rx_len -= copied;
            if matches!(
                socket.state,
                TcpState::Established
                    | TcpState::FinWait1
                    | TcpState::FinWait2
                    | TcpState::CloseWait
                    | TcpState::LastAck
            ) {
                socket.window_dirty = true;
            }
            return Ok(copied);
        }
        if socket.error {
            return Err(Status::Closed);
        }
        if socket.eof {
            return Ok(0);
        }
        if socket.state == TcpState::Closed {
            return Err(Status::Closed);
        }
        Err(Status::WouldBlock)
    }

    /// Pops the oldest completed connection from a listener (spec 0020, N10).
    ///
    /// The connection was created by the stack, is already in `Established`,
    /// and belongs to the listener's owner, so the returned handle is a normal
    /// [`KIND_TCP`] socket for `send`/`recv`/`close`. `WouldBlock` when the
    /// pending queue is empty; `Invalid` for a live handle that is not a
    /// listener; `NotFound`/`Invalid` for the ordinary stale/foreign rules.
    pub fn accept(&mut self, handle: u64, owner: u64) -> Result<u64, Status> {
        let index = self.lookup(handle, owner)?;
        if self.sockets[index].kind != KIND_LISTEN {
            return Err(Status::Invalid);
        }
        loop {
            if self.sockets[index].pending_len == 0 {
                return Err(Status::WouldBlock);
            }
            let peer = self.sockets[index].pending[0];
            let socket = &mut self.sockets[index];
            for slot in 0..(socket.pending_len - 1) {
                socket.pending[slot] = socket.pending[slot + 1];
            }
            socket.pending_len -= 1;
            socket.pending[socket.pending_len] = 0;
            // A pending connection is released on a listener close or a peer
            // reset, and those paths remove it; a dead entry is skipped rather
            // than handed out with a stale generation.
            if self.lookup(peer, owner).is_ok() {
                return Ok(peer);
            }
        }
    }

    /// Sends FIN for a connected TCP socket (or frees a UDP socket). The slot
    /// is released once the close handshake completes; UDP and a socket that
    /// never connected are released immediately. Closing a listener releases
    /// its completed and half-open connections.
    pub fn close(
        &mut self,
        handle: u64,
        owner: u64,
        now_ns: u64,
        tx: &mut dyn FnMut(&[u8]) -> bool,
    ) -> Status {
        let index = match self.lookup(handle, owner) {
            Ok(index) => index,
            Err(status) => return status,
        };
        match self.sockets[index].kind {
            KIND_UDP => {
                self.release(index);
                Status::Ok
            }
            KIND_TCP => self.tcp_close(index, now_ns, tx),
            KIND_LISTEN => {
                self.release_listener(index);
                Status::Ok
            }
            _ => {
                self.release(index);
                Status::Ok
            }
        }
    }

    /// Called from `Task::reclaim`: closes every socket owned by `owner`.
    pub fn close_owner(&mut self, owner: u64, now_ns: u64, tx: &mut dyn FnMut(&[u8]) -> bool) {
        for index in 0..MAX_SOCKETS {
            if !self.sockets[index].active || self.sockets[index].owner != owner {
                continue;
            }
            if self.sockets[index].kind == KIND_LISTEN {
                self.release_listener(index);
                continue;
            }
            if self.sockets[index].kind == KIND_TCP {
                match self.sockets[index].state {
                    TcpState::Established | TcpState::CloseWait => {
                        let _ = self.tcp_send_fin(index, now_ns, tx);
                    }
                    TcpState::SynSent | TcpState::SynReceived => {
                        let _ = self.tcp_send_reset(index, now_ns, tx);
                    }
                    _ => {}
                }
            }
            self.release(index);
        }
    }

    /// Parses one Ethernet frame delivered by the device; may emit frames.
    pub fn receive(&mut self, frame: &[u8], now_ns: u64, tx: &mut dyn FnMut(&[u8]) -> bool) {
        if frame.len() < ETH_HEADER {
            self.count_drop();
            return;
        }
        self.stats.rx_frames = self.stats.rx_frames.saturating_add(1);

        let dst = ethernet::mac_at(frame, 0);
        if dst != self.mac && !ethernet::is_broadcast(&dst) {
            self.count_drop();
            return;
        }
        let src = ethernet::mac_at(frame, 6);
        match ethernet::be16(frame, 12) {
            ETHERTYPE_ARP => {
                self.arp_receive(&frame[ETH_HEADER..], &src, now_ns, tx);
                self.note_event();
            }
            ETHERTYPE_IPV4 => {
                self.ipv4_receive(&frame[ETH_HEADER..], &src, now_ns, tx);
                self.note_event();
            }
            _ => self.count_drop(),
        }
    }

    /// Drives handshake retransmission and the TCP/ARP timers.
    pub fn service(&mut self, now_ns: u64, tx: &mut dyn FnMut(&[u8]) -> bool) {
        self.arp_age(now_ns);
        let mut expired = false;
        for index in 0..MAX_SOCKETS {
            let socket = &mut self.sockets[index];
            if socket.active && socket.op_deadline_ns != 0 && now_ns >= socket.op_deadline_ns {
                socket.op_deadline_ns = 0;
                socket.timed_out = true;
                expired = true;
            }
            if !socket.active || socket.kind != KIND_TCP {
                continue;
            }
            match socket.state {
                TcpState::SynSent => self.tcp_service_syn(index, now_ns, tx),
                TcpState::SynReceived => self.tcp_service_syn_received(index, now_ns, tx),
                TcpState::Established
                | TcpState::FinWait1
                | TcpState::FinWait2
                | TcpState::CloseWait
                | TcpState::LastAck => self.tcp_service_data(index, now_ns, tx),
                TcpState::Listen | TcpState::Closed => {}
            }
        }
        if expired {
            self.note_event();
        }
    }

    /// Reports whether a socket is readable: queued data, EOF, an error, a
    /// completed TCP connect or, for a listener, a pending connection.
    /// Owner-checked.
    pub fn readable(&self, handle: u64, owner: u64) -> bool {
        let Ok(index) = self.lookup(handle, owner) else {
            return false;
        };
        let socket = &self.sockets[index];
        // An expired deadline is reported as readable so a `next_any`/read
        // wait returns and the following operation can observe the timeout.
        socket.timed_out || self.slot_readable(socket)
    }

    /// Readiness for a **selector** (`core.next_any`/`poll_any`): data, EOF,
    /// error, a pending listener connection or an expired deadline. A bare
    /// completed TCP connect is deliberately excluded; the first blocking
    /// `recv` on an empty connection still waits for data, so reporting the
    /// connect as a select-ready source would spin the selector (and, in a
    /// console loop like `nc`, would block the task on the socket where a
    /// console byte can no longer wake it).
    pub fn select_ready(&self, handle: u64, owner: u64) -> bool {
        let Ok(index) = self.lookup(handle, owner) else {
            return false;
        };
        let socket = &self.sockets[index];
        socket.timed_out || self.slot_recv_ready(socket)
    }

    /// Finds a live slot for `handle`, checking the generation and the owner.
    fn lookup(&self, handle: u64, owner: u64) -> Result<usize, Status> {
        let generation = handle >> 8;
        let index = (handle & 0xFF) as usize;
        if handle == 0 || generation == 0 || index >= MAX_SOCKETS {
            return Err(Status::NotFound);
        }
        let socket = &self.sockets[index];
        if !socket.active || socket.generation != generation {
            return Err(Status::NotFound);
        }
        if socket.owner != owner {
            return Err(Status::Invalid);
        }
        Ok(index)
    }

    /// Frees a slot; the generation is not reused, so old handles stay stale.
    fn release(&mut self, index: usize) {
        self.sockets[index] = Socket::EMPTY;
    }

    /// Releases a listener: the slot, its completed connections and any
    /// half-open `SynReceived` connection bound to the same port and owner.
    fn release_listener(&mut self, index: usize) {
        let owner = self.sockets[index].owner;
        let port = self.sockets[index].local_port;
        let mut pending = [0u64; LISTEN_BACKLOG];
        let pending_len = self.sockets[index].pending_len;
        pending[..pending_len]
            .copy_from_slice(&self.sockets[index].pending[..pending_len]);
        self.release(index);
        for handle in &pending[..pending_len] {
            if let Ok(peer) = self.lookup(*handle, owner) {
                self.release(peer);
            }
        }
        for slot in 0..MAX_SOCKETS {
            let socket = &self.sockets[slot];
            if socket.active
                && socket.owner == owner
                && socket.kind == KIND_TCP
                && socket.local_port == port
                && socket.state == TcpState::SynReceived
            {
                self.release(slot);
            }
        }
    }

    /// Removes the socket at `index` from every listener's pending queue.
    /// Returns whether it was queued, which is how a reset distinguishes a
    /// connection the application never accepted from an accepted one.
    pub(crate) fn unpend(&mut self, index: usize) -> bool {
        let handle = (self.sockets[index].generation << 8) | index as u64;
        for listener in 0..MAX_SOCKETS {
            if !self.sockets[listener].active || self.sockets[listener].kind != KIND_LISTEN {
                continue;
            }
            let socket = &mut self.sockets[listener];
            let mut found = None;
            for slot in 0..socket.pending_len {
                if socket.pending[slot] == handle {
                    found = Some(slot);
                    break;
                }
            }
            if let Some(position) = found {
                for slot in position..(socket.pending_len - 1) {
                    socket.pending[slot] = socket.pending[slot + 1];
                }
                socket.pending_len -= 1;
                socket.pending[socket.pending_len] = 0;
                return true;
            }
        }
        false
    }

    fn next_generation(&mut self) -> u64 {
        if self.generation == 0 {
            self.generation = 1;
        }
        let generation = self.generation;
        self.generation = self.generation.wrapping_add(1);
        if self.generation == 0 {
            self.generation = 1;
        }
        generation
    }

    fn next_isn(&mut self) -> u32 {
        if self.isn == 0 {
            self.isn = 0x1000_0000;
        }
        let isn = self.isn;
        self.isn = self.isn.wrapping_add(0x1_0000);
        isn
    }

    fn alloc_port(&mut self, kind: u64, local: u16) -> Result<u16, Status> {
        if local != 0 {
            if self.port_in_use(kind, local) {
                return Err(Status::Invalid);
            }
            return Ok(local);
        }
        if self.next_port < EPHEMERAL_START {
            self.next_port = EPHEMERAL_START;
        }
        for _ in 0..EPHEMERAL_PORTS {
            let port = self.next_port;
            self.next_port = if port == u16::MAX {
                EPHEMERAL_START
            } else {
                port + 1
            };
            if !self.port_in_use(kind, port) {
                return Ok(port);
            }
        }
        Err(Status::Full)
    }

    fn port_in_use(&self, kind: u64, port: u16) -> bool {
        self.sockets
            .iter()
            .any(|socket| socket.active && socket.kind == kind && socket.local_port == port)
    }

    /// True when `ip` is on our subnet.
    fn on_link(&self, ip: u32) -> bool {
        self.config.mask != 0 && (ip & self.config.mask) == (self.config.ip & self.config.mask)
    }

    /// Our subnet's directed broadcast address.
    fn broadcast_ip(&self) -> u32 {
        self.config.ip | !self.config.mask
    }

    /// Resolves the MAC for `dst_ip`, sending one ARP request on a miss.
    ///
    /// `None` means "not resolved yet"; the caller returns `WouldBlock` and
    /// retries, and `service` retries its own pending frames.
    fn resolve_dst(
        &mut self,
        dst_ip: u32,
        now_ns: u64,
        tx: &mut dyn FnMut(&[u8]) -> bool,
    ) -> Option<[u8; 6]> {
        // A broadcast needs no next-hop lookup, so it also works before an
        // address is configured: the DHCP client's DISCOVER leaves from
        // 0.0.0.0 to 255.255.255.255 (spec 0020, N8/N18).
        if dst_ip == BROADCAST_IP || (self.config.mask != 0 && dst_ip == self.broadcast_ip()) {
            return Some(ethernet::BROADCAST_MAC);
        }
        if self.config.ip == 0 {
            return None;
        }
        let next_hop = if self.on_link(dst_ip) {
            dst_ip
        } else {
            self.config.gateway
        };
        if next_hop == 0 {
            return None;
        }
        if let Some(mac) = self.arp_lookup(next_hop, now_ns) {
            return Some(mac);
        }
        self.arp_request(next_hop, now_ns, tx);
        None
    }

    /// Transmits one frame, padding it to the Ethernet minimum.
    fn emit(&mut self, frame: &mut [u8], len: usize, tx: &mut dyn FnMut(&[u8]) -> bool) -> Status {
        if len > frame.len() {
            self.count_error();
            return Status::Invalid;
        }
        let total = len.max(ethernet::MIN_FRAME_LEN).min(frame.len());
        for byte in frame[len..total].iter_mut() {
            *byte = 0;
        }
        if tx(&frame[..total]) {
            self.stats.tx_frames = self.stats.tx_frames.saturating_add(1);
            Status::Ok
        } else {
            self.count_error();
            Status::WouldBlock
        }
    }

    fn count_drop(&mut self) {
        self.stats.drops = self.stats.drops.saturating_add(1);
    }

    fn count_error(&mut self) {
        self.stats.errors = self.stats.errors.saturating_add(1);
    }
}

impl Default for Stack {
    fn default() -> Self {
        Self::new()
    }
}
