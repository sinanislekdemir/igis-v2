//! The stop-and-wait TCP client (spec 0020, N9; ADR 0050).
//!
//! One outstanding segment per connection: `send` accepts at most [`MSS`]
//! bytes, keeps them in the slot's TX arena and returns `WouldBlock` until the
//! peer acknowledges them. Timers (SYN retry, data/FIN retransmission and the
//! connect deadline) run in [`Stack::service`]; nothing here blocks or spins.

use abi::Status;

use crate::checksum;
use crate::ethernet;
use crate::ipv4::PROTO_TCP;
use crate::{
    Stack, Socket, TcpState, CONNECT_TIMEOUT_NS, ETH_HEADER, FRAME_MAX, INITIAL_RTO_NS,
    IPV4_HEADER, KIND_LISTEN, LISTEN_BACKLOG, MAX_ATTEMPTS, MAX_SOCKETS, MSS, RX_CAP,
    SYN_RETRY_NS,
};

/// FIN flag.
pub(crate) const FIN: u8 = 0x01;
/// SYN flag.
pub(crate) const SYN: u8 = 0x02;
/// RST flag.
pub(crate) const RST: u8 = 0x04;
/// PSH flag.
pub(crate) const PSH: u8 = 0x08;
/// ACK flag.
pub(crate) const ACK: u8 = 0x10;

const HEADER_LEN: usize = 20;
const SYN_OPTIONS_LEN: usize = 4;

/// Where a segment's payload comes from.
pub(crate) enum Payload {
    /// No payload (SYN, ACK, FIN, RST).
    Empty,
    /// The first `n` bytes of the socket's TX arena.
    Slot(usize),
}

struct Parsed<'a> {
    src_port: u16,
    dst_port: u16,
    seq: u32,
    ack: u32,
    flags: u8,
    payload: &'a [u8],
}

/// Serial-number comparison: true when `a` precedes `b`.
fn seq_lt(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) < 0
}

/// Retransmission timeout after `attempts` transmissions (1 s, 2 s, 4 s, 8 s, 16 s).
fn retransmit_ns(attempts: u8) -> u64 {
    let shift = attempts.saturating_sub(1).min(4);
    INITIAL_RTO_NS << u32::from(shift)
}

fn parse(segment: &[u8]) -> Option<Parsed<'_>> {
    if segment.len() < HEADER_LEN {
        return None;
    }
    let data_offset = usize::from(segment[12] >> 4) * 4;
    if data_offset < HEADER_LEN || data_offset > segment.len() {
        return None;
    }
    Some(Parsed {
        src_port: ethernet::be16(segment, 0),
        dst_port: ethernet::be16(segment, 2),
        seq: ethernet::be32(segment, 4),
        ack: ethernet::be32(segment, 8),
        flags: segment[13],
        payload: &segment[data_offset..],
    })
}

impl Stack {
    fn find_tcp(&self, src_ip: u32, src_port: u16, dst_port: u16) -> Option<usize> {
        for index in 0..MAX_SOCKETS {
            let socket = &self.sockets[index];
            if socket.active
                && socket.kind == crate::KIND_TCP
                && socket.local_port == dst_port
                && socket.remote_ip == src_ip
                && socket.remote_port == src_port
            {
                return Some(index);
            }
        }
        None
    }

    /// Finds a listener bound to `local_port` and owned by `owner`.
    fn find_listener(&self, local_port: u16, owner: u64) -> Option<usize> {
        for index in 0..MAX_SOCKETS {
            let socket = &self.sockets[index];
            if socket.active
                && socket.kind == KIND_LISTEN
                && socket.local_port == local_port
                && socket.owner == owner
            {
                return Some(index);
            }
        }
        None
    }

    /// Answers an inbound SYN on a listening port (spec 0020, N10).
    ///
    /// Allocates a connection slot in `SynReceived`, records the peer and the
    /// client's sequence number and sends `SYN|ACK` (MSS 1460, window 4096).
    /// With no listener or no free slot the segment is dropped and counted; no
    /// RST is sent, so a stray SYN cannot be used to probe the host.
    fn listener_syn(
        &mut self,
        src_ip: u32,
        src_port: u16,
        dst_port: u16,
        seq: u32,
        now_ns: u64,
        tx: &mut dyn FnMut(&[u8]) -> bool,
    ) {
        let Some(listener) = self
            .sockets
            .iter()
            .position(|socket| {
                socket.active && socket.kind == KIND_LISTEN && socket.local_port == dst_port
            })
        else {
            self.count_drop();
            return;
        };
        let owner = self.sockets[listener].owner;
        let Some(index) = self.sockets.iter().position(|socket| !socket.active) else {
            // The socket table is shared by listeners and connections; with no
            // free slot the SYN is dropped and the client retries. Sending a
            // RST would need a connection slot to answer from in the first
            // place, which is exactly what is missing.
            self.count_drop();
            return;
        };
        let generation = self.next_generation();
        let isn = self.next_isn();
        {
            let socket = &mut self.sockets[index];
            *socket = Socket::EMPTY;
            socket.active = true;
            socket.generation = generation;
            socket.owner = owner;
            socket.kind = crate::KIND_TCP;
            socket.local_port = dst_port;
            socket.remote_ip = src_ip;
            socket.remote_port = src_port;
            socket.state = TcpState::SynReceived;
            socket.snd_una = isn;
            socket.snd_nxt = isn.wrapping_add(1);
            socket.rcv_nxt = seq.wrapping_add(1);
            socket.syn_started = true;
            socket.syn_next = now_ns.saturating_add(SYN_RETRY_NS);
            socket.connect_deadline = now_ns.saturating_add(CONNECT_TIMEOUT_NS);
        }
        let ack = self.sockets[index].rcv_nxt;
        let _ = self.transmit_tcp(index, isn, ack, SYN | ACK, Payload::Empty, now_ns, tx);
    }

    /// Applies one segment to a half-open `SynReceived` connection.
    ///
    /// Returns true when the caller must stop processing (the normal case):
    /// the segment was a retransmitted SYN, an unexpected acknowledgement, the
    /// final ACK that established and enqueued the connection, or a failure
    /// that released the slot.
    fn tcp_syn_received(
        &mut self,
        index: usize,
        parsed: &Parsed<'_>,
        now_ns: u64,
        tx: &mut dyn FnMut(&[u8]) -> bool,
    ) -> bool {
        if parsed.flags & SYN != 0 && parsed.flags & ACK == 0 {
            // Our SYN-ACK was lost: repeat it instead of answering a bare ACK.
            let (seq, ack) = {
                let socket = &self.sockets[index];
                (socket.snd_una, socket.rcv_nxt)
            };
            let _ = self.transmit_tcp(index, seq, ack, SYN | ACK, Payload::Empty, now_ns, tx);
            return true;
        }
        if parsed.flags & ACK == 0 || parsed.ack != self.sockets[index].snd_nxt {
            return true;
        }

        let (port, owner) = {
            let socket = &self.sockets[index];
            (socket.local_port, socket.owner)
        };
        let Some(listener) = self.find_listener(port, owner) else {
            // The listener disappeared between the SYN and the ACK.
            let _ = self.tcp_send_reset(index, now_ns, tx);
            self.release(index);
            return true;
        };
        {
            let socket = &mut self.sockets[index];
            socket.state = TcpState::Established;
            socket.snd_una = socket.snd_nxt;
        }
        if self.sockets[listener].pending_len >= LISTEN_BACKLOG {
            // A third completed connection is refused; `accept` never sees it
            // (spec 0020, N10's two-entry queue).
            let _ = self.tcp_send_reset(index, now_ns, tx);
            self.release(index);
            return true;
        }
        let handle = (self.sockets[index].generation << 8) | index as u64;
        let listener_socket = &mut self.sockets[listener];
        listener_socket.pending[listener_socket.pending_len] = handle;
        listener_socket.pending_len += 1;
        true
    }

    /// Builds and transmits one TCP segment for the socket at `index`.
    pub(crate) fn transmit_tcp(
        &mut self,
        index: usize,
        seq: u32,
        ack: u32,
        flags: u8,
        payload: Payload,
        now_ns: u64,
        tx: &mut dyn FnMut(&[u8]) -> bool,
    ) -> Status {
        let (dst_ip, src_port, dst_port, window) = {
            let socket = &self.sockets[index];
            (
                socket.remote_ip,
                socket.local_port,
                socket.remote_port,
                (RX_CAP - socket.rx_len) as u16,
            )
        };
        let Some(dst_mac) = self.resolve_dst(dst_ip, now_ns, tx) else {
            return Status::WouldBlock;
        };
        let payload_len = match payload {
            Payload::Empty => 0,
            Payload::Slot(len) => len,
        };
        if payload_len > MSS {
            return Status::Invalid;
        }
        let options = if flags & SYN != 0 {
            SYN_OPTIONS_LEN
        } else {
            0
        };
        let segment_len = HEADER_LEN + options + payload_len;
        let total_len = IPV4_HEADER + segment_len;
        if ETH_HEADER + total_len > FRAME_MAX {
            return Status::Invalid;
        }

        let mut frame = [0u8; FRAME_MAX];
        let offset = self.write_ipv4_header(&mut frame, dst_mac, dst_ip, PROTO_TCP, segment_len);
        {
            let header = &mut frame[offset..offset + HEADER_LEN];
            header[0..2].copy_from_slice(&src_port.to_be_bytes());
            header[2..4].copy_from_slice(&dst_port.to_be_bytes());
            header[4..8].copy_from_slice(&seq.to_be_bytes());
            header[8..12].copy_from_slice(&ack.to_be_bytes());
            header[12] = (((HEADER_LEN + options) / 4) as u8) << 4;
            header[13] = flags;
            header[14..16].copy_from_slice(&window.to_be_bytes());
            header[16] = 0;
            header[17] = 0;
            header[18] = 0;
            header[19] = 0;
        }
        if options == SYN_OPTIONS_LEN {
            let at = offset + HEADER_LEN;
            frame[at] = 2; // MSS option
            frame[at + 1] = 4;
            frame[at + 2..at + 4].copy_from_slice(&(MSS as u16).to_be_bytes());
        }
        let data_at = offset + HEADER_LEN + options;
        match payload {
            Payload::Empty => {}
            Payload::Slot(len) => {
                frame[data_at..data_at + len].copy_from_slice(&self.sockets[index].tx[..len]);
            }
        }

        let mut summer = checksum::pseudo(self.config.ip, dst_ip, PROTO_TCP, segment_len as u16);
        summer.add(&frame[offset..offset + segment_len]);
        let sum = summer.finish();
        frame[offset + 16..offset + 18].copy_from_slice(&sum.to_be_bytes());
        self.emit(&mut frame, ETH_HEADER + total_len, tx)
    }

    /// Sends one application segment. `WouldBlock` until the connection is
    /// established and the previous segment is acknowledged.
    pub(crate) fn tcp_send(
        &mut self,
        index: usize,
        data: &[u8],
        now_ns: u64,
        tx: &mut dyn FnMut(&[u8]) -> bool,
    ) -> Result<usize, Status> {
        {
            let socket = &self.sockets[index];
            if socket.error {
                return Err(Status::Closed);
            }
            if socket.state != TcpState::Established || socket.tx_sent || socket.fin_sent {
                return Err(Status::WouldBlock);
            }
        }
        if data.is_empty() {
            return Ok(0);
        }
        let len = data.len().min(MSS);
        if len > crate::TX_CAP {
            return Err(Status::CapacityExceeded);
        }
        self.sockets[index].tx[..len].copy_from_slice(&data[..len]);
        let (seq, ack) = {
            let socket = &self.sockets[index];
            (socket.snd_nxt, socket.rcv_nxt)
        };
        let status = self.transmit_tcp(index, seq, ack, PSH | ACK, Payload::Slot(len), now_ns, tx);
        if status != Status::Ok {
            self.sockets[index].tx_len = 0;
            return Err(status);
        }
        let socket = &mut self.sockets[index];
        socket.tx_len = len;
        socket.tx_sent = true;
        socket.snd_una = seq;
        socket.snd_nxt = seq.wrapping_add(len as u32);
        socket.tx_attempts = 1;
        socket.tx_deadline = now_ns.saturating_add(INITIAL_RTO_NS);
        Ok(len)
    }

    fn tcp_reset_received(&mut self, index: usize) {
        let socket = &mut self.sockets[index];
        socket.error = true;
        socket.state = TcpState::Closed;
        socket.tx_sent = false;
        socket.tx_len = 0;
        socket.fin_sent = false;
        socket.rx_len = 0;
        socket.udp_ready = false;
        socket.eof = false;
    }

    /// Applies one ACK; returns true when the slot is finished and must be
    /// released.
    fn tcp_ack(&mut self, index: usize, ack: u32) -> bool {
        let socket = &mut self.sockets[index];
        if socket.fin_sent && ack == socket.snd_nxt {
            socket.fin_sent = false;
            socket.tx_sent = false;
            socket.tx_len = 0;
            socket.snd_una = socket.snd_nxt;
            socket.tx_attempts = 0;
            return match socket.state {
                TcpState::FinWait1 => {
                    socket.state = TcpState::FinWait2;
                    false
                }
                TcpState::LastAck => {
                    socket.state = TcpState::Closed;
                    true
                }
                _ => false,
            };
        }
        if socket.tx_sent && ack == socket.snd_una.wrapping_add(socket.tx_len as u32) {
            socket.tx_sent = false;
            socket.tx_len = 0;
            socket.snd_una = ack;
            socket.tx_attempts = 0;
        }
        false
    }

    /// Handles one TCP segment delivered by IPv4.
    pub(crate) fn tcp_receive(
        &mut self,
        src_ip: u32,
        dst_ip: u32,
        segment: &[u8],
        now_ns: u64,
        tx: &mut dyn FnMut(&[u8]) -> bool,
    ) {
        let Some(parsed) = parse(segment) else {
            self.count_drop();
            return;
        };
        // Validate the checksum before any state is touched: a damaged segment
        // must not open a connection or enqueue anything.
        let mut summer = checksum::pseudo(src_ip, dst_ip, PROTO_TCP, segment.len() as u16);
        summer.add(segment);
        if !summer.verify() {
            self.count_error();
            return;
        }
        let Some(index) = self.find_tcp(src_ip, parsed.src_port, parsed.dst_port) else {
            // No connection matches. A bare SYN may open one on a listening
            // port; everything else belongs to no socket and is dropped.
            if parsed.flags & SYN != 0 && parsed.flags & ACK == 0 {
                self.listener_syn(
                    src_ip,
                    parsed.src_port,
                    parsed.dst_port,
                    parsed.seq,
                    now_ns,
                    tx,
                );
            } else {
                self.count_drop();
            }
            return;
        };

        // A reset ends the connection immediately; queued data is discarded.
        if parsed.flags & RST != 0 {
            if self.unpend(index) {
                // The application never accepted it, so there is nothing to
                // observe: free the slot rather than leak a pending ghost.
                self.release(index);
            } else if self.sockets[index].state == TcpState::SynReceived {
                // The handshake never completed; the listener never saw it.
                self.release(index);
            } else {
                self.tcp_reset_received(index);
            }
            return;
        }
        // A socket that already failed (connect timeout, exhausted
        // retransmission) ignores late traffic; the owner closes it.
        if self.sockets[index].error {
            return;
        }

        if self.sockets[index].state == TcpState::SynSent {
            if parsed.flags & (SYN | ACK) == (SYN | ACK)
                && parsed.ack == self.sockets[index].snd_nxt
            {
                {
                    let socket = &mut self.sockets[index];
                    socket.rcv_nxt = parsed.seq.wrapping_add(1);
                    socket.snd_una = socket.snd_nxt;
                    socket.state = TcpState::Established;
                }
                let (seq, ack) = {
                    let socket = &self.sockets[index];
                    (socket.snd_nxt, socket.rcv_nxt)
                };
                let _ = self.transmit_tcp(index, seq, ack, ACK, Payload::Empty, now_ns, tx);
            }
            // A wrong acknowledgement is ignored; the SYN is retried.
            return;
        }

        if self.sockets[index].state == TcpState::SynReceived {
            if self.tcp_syn_received(index, &parsed, now_ns, tx) {
                return;
            }
        }

        if parsed.flags & ACK != 0 && self.tcp_ack(index, parsed.ack) {
            self.release(index);
            return;
        }

        let mut ack_needed = false;
        let mut duplicate = false;
        let mut release = false;
        {
            let socket = &mut self.sockets[index];
            let payload_len = parsed.payload.len();
            if payload_len > 0 {
                if parsed.seq == socket.rcv_nxt {
                    let space = RX_CAP - socket.rx_len;
                    if payload_len <= space {
                        socket.rx[socket.rx_len..socket.rx_len + payload_len]
                            .copy_from_slice(parsed.payload);
                        socket.rx_len += payload_len;
                        socket.rcv_nxt = socket.rcv_nxt.wrapping_add(payload_len as u32);
                        ack_needed = true;
                    }
                    // A full arena is not acknowledged: the peer retransmits.
                } else if seq_lt(parsed.seq, socket.rcv_nxt) {
                    duplicate = true;
                } else {
                    ack_needed = true;
                }
            } else if parsed.seq != socket.rcv_nxt {
                if seq_lt(parsed.seq, socket.rcv_nxt) {
                    duplicate = true;
                } else {
                    ack_needed = true;
                }
            }

            if parsed.flags & FIN != 0 {
                let fin_seq = parsed.seq.wrapping_add(payload_len as u32);
                if fin_seq == socket.rcv_nxt {
                    socket.rcv_nxt = socket.rcv_nxt.wrapping_add(1);
                    socket.eof = true;
                    ack_needed = true;
                    match socket.state {
                        TcpState::Established => socket.state = TcpState::CloseWait,
                        TcpState::FinWait1 | TcpState::FinWait2 => {
                            release = socket.close_requested;
                        }
                        _ => {}
                    }
                } else if !socket.eof {
                    ack_needed = true;
                }
            }
        }

        if ack_needed || duplicate {
            let (seq, ack) = {
                let socket = &self.sockets[index];
                (socket.snd_nxt, socket.rcv_nxt)
            };
            let _ = self.transmit_tcp(index, seq, ack, ACK, Payload::Empty, now_ns, tx);
        }
        if release {
            self.release(index);
        }
    }

    /// Sends the FIN for `index` and enters the closing state.
    pub(crate) fn tcp_send_fin(
        &mut self,
        index: usize,
        now_ns: u64,
        tx: &mut dyn FnMut(&[u8]) -> bool,
    ) -> Status {
        let (seq, ack, state) = {
            let socket = &self.sockets[index];
            (socket.snd_nxt, socket.rcv_nxt, socket.state)
        };
        {
            let socket = &mut self.sockets[index];
            socket.snd_nxt = socket.snd_nxt.wrapping_add(1);
            socket.fin_sent = true;
            socket.fin_attempts = 1;
            socket.fin_deadline = now_ns.saturating_add(INITIAL_RTO_NS);
            socket.close_requested = true;
            match state {
                TcpState::Established => socket.state = TcpState::FinWait1,
                TcpState::CloseWait => socket.state = TcpState::LastAck,
                _ => {}
            }
        }
        self.transmit_tcp(index, seq, ack, FIN | ACK, Payload::Empty, now_ns, tx)
    }

    /// Best-effort reset for a socket that never completed its handshake.
    pub(crate) fn tcp_send_reset(
        &mut self,
        index: usize,
        now_ns: u64,
        tx: &mut dyn FnMut(&[u8]) -> bool,
    ) -> Status {
        let (seq, ack) = {
            let socket = &self.sockets[index];
            (socket.snd_nxt, socket.rcv_nxt)
        };
        self.transmit_tcp(index, seq, ack, RST | ACK, Payload::Empty, now_ns, tx)
    }

    /// Closes one TCP socket: FIN when connected, immediate release otherwise.
    pub(crate) fn tcp_close(
        &mut self,
        index: usize,
        now_ns: u64,
        tx: &mut dyn FnMut(&[u8]) -> bool,
    ) -> Status {
        match self.sockets[index].state {
            TcpState::SynSent => {
                self.release(index);
                Status::Ok
            }
            TcpState::Established | TcpState::CloseWait => {
                // The app is done whether or not the device took the frame;
                // `service` retransmits while `fin_sent` is set.
                let _ = self.tcp_send_fin(index, now_ns, tx);
                Status::Ok
            }
            TcpState::FinWait1 | TcpState::FinWait2 | TcpState::LastAck => {
                self.sockets[index].close_requested = true;
                Status::Ok
            }
            TcpState::Listen | TcpState::SynReceived | TcpState::Closed => {
                self.release(index);
                Status::Ok
            }
        }
    }

    fn tcp_fail(&mut self, index: usize) {
        let socket = &mut self.sockets[index];
        socket.state = TcpState::Closed;
        socket.error = true;
        socket.tx_sent = false;
        socket.tx_len = 0;
        socket.fin_sent = false;
    }

    /// Drives the SYN retry and the connect deadline.
    pub(crate) fn tcp_service_syn(
        &mut self,
        index: usize,
        now_ns: u64,
        tx: &mut dyn FnMut(&[u8]) -> bool,
    ) {
        if !self.sockets[index].syn_started {
            let socket = &mut self.sockets[index];
            socket.syn_started = true;
            socket.connect_deadline = now_ns.saturating_add(crate::CONNECT_TIMEOUT_NS);
            socket.syn_next = now_ns;
        }
        if now_ns >= self.sockets[index].connect_deadline {
            self.tcp_fail(index);
            return;
        }
        if now_ns >= self.sockets[index].syn_next {
            let seq = self.sockets[index].snd_una;
            let _ = self.transmit_tcp(index, seq, 0, SYN, Payload::Empty, now_ns, tx);
            self.sockets[index].syn_next = now_ns.saturating_add(crate::SYN_RETRY_NS);
        }
    }

    /// Drives a half-open listener connection: retransmits the SYN-ACK on the
    /// SYN retry interval and releases the slot when the handshake deadline
    /// expires (a client that never sends the final ACK).
    pub(crate) fn tcp_service_syn_received(
        &mut self,
        index: usize,
        now_ns: u64,
        tx: &mut dyn FnMut(&[u8]) -> bool,
    ) {
        if now_ns >= self.sockets[index].connect_deadline {
            self.release(index);
            return;
        }
        if now_ns >= self.sockets[index].syn_next {
            let (seq, ack) = {
                let socket = &self.sockets[index];
                (socket.snd_una, socket.rcv_nxt)
            };
            let _ = self.transmit_tcp(index, seq, ack, SYN | ACK, Payload::Empty, now_ns, tx);
            self.sockets[index].syn_next = now_ns.saturating_add(SYN_RETRY_NS);
        }
    }

    /// Retransmits an unacknowledged data segment or FIN, and reopens a
    /// receive window that `recv` drained.
    pub(crate) fn tcp_service_data(
        &mut self,
        index: usize,
        now_ns: u64,
        tx: &mut dyn FnMut(&[u8]) -> bool,
    ) {
        let (sent, deadline, attempts, len, una, rcv) = {
            let socket = &self.sockets[index];
            (
                socket.tx_sent,
                socket.tx_deadline,
                socket.tx_attempts,
                socket.tx_len,
                socket.snd_una,
                socket.rcv_nxt,
            )
        };
        if sent && now_ns >= deadline {
            if attempts >= MAX_ATTEMPTS {
                self.tcp_fail(index);
                return;
            }
            let _ = self.transmit_tcp(index, una, rcv, PSH | ACK, Payload::Slot(len), now_ns, tx);
            let socket = &mut self.sockets[index];
            socket.tx_attempts = attempts.saturating_add(1);
            socket.tx_deadline = now_ns.saturating_add(retransmit_ns(attempts.saturating_add(1)));
        }

        let (fin, fin_deadline, fin_attempts, snd_nxt, rcv) = {
            let socket = &self.sockets[index];
            (
                socket.fin_sent,
                socket.fin_deadline,
                socket.fin_attempts,
                socket.snd_nxt,
                socket.rcv_nxt,
            )
        };
        if fin && now_ns >= fin_deadline {
            if fin_attempts >= MAX_ATTEMPTS {
                self.tcp_fail(index);
                return;
            }
            let seq = snd_nxt.wrapping_sub(1);
            let _ = self.transmit_tcp(index, seq, rcv, FIN | ACK, Payload::Empty, now_ns, tx);
            let socket = &mut self.sockets[index];
            socket.fin_attempts = fin_attempts.saturating_add(1);
            socket.fin_deadline =
                now_ns.saturating_add(retransmit_ns(fin_attempts.saturating_add(1)));
        }

        if self.sockets[index].window_dirty {
            let (seq, ack) = {
                let socket = &self.sockets[index];
                (socket.snd_nxt, socket.rcv_nxt)
            };
            let status = self.transmit_tcp(index, seq, ack, ACK, Payload::Empty, now_ns, tx);
            if status == Status::Ok {
                self.sockets[index].window_dirty = false;
            }
        }
    }
}
