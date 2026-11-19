//! Host tests for the M10.2–M10.4 wire protocols (spec 0020, ADR 0048/0050).
//!
//! The simulated link is the same shape as the kernel's facade: every transmit
//! closure captures the frame, and received frames are built by hand with an
//! independent checksum implementation, so the stack is never validated
//! against itself.

use abi::Status;
use net::{
    Config, Stack, KIND_LISTEN, KIND_TCP, KIND_UDP, LISTEN_BACKLOG, MAX_SOCKETS, MSS,
};

/// The stack's SYN retry interval and passive handshake deadline (spec 0020,
/// N9); private tuning constants, pinned here independently.
const SYN_RETRY_NS: u64 = 200_000_000;
const CONNECT_TIMEOUT_NS: u64 = 5_000_000_000;

const OUR_MAC: [u8; 6] = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
const PEER_MAC: [u8; 6] = [0x52, 0x55, 0x0A, 0x00, 0x02, 0x02];
const OTHER_MAC: [u8; 6] = [0x52, 0x55, 0x0A, 0x00, 0x02, 0x99];
const OUR_IP: u32 = 0x0A00_020F;
const MASK: u32 = 0xFFFF_FF00;
const PEER_IP: u32 = 0x0A00_0202;
const PEER_PORT: u16 = 6666;
const T0: u64 = 1_000_000_000;

const FIN: u8 = 0x01;
const SYN: u8 = 0x02;
const RST: u8 = 0x04;
const PSH: u8 = 0x08;
const ACK: u8 = 0x10;

// ---------------------------------------------------------------------------
// The simulated link
// ---------------------------------------------------------------------------

struct Wire {
    frames: Vec<Vec<u8>>,
    accept: bool,
}

impl Wire {
    fn new() -> Self {
        Self {
            frames: Vec::new(),
            accept: true,
        }
    }

    fn drive(&mut self, run: impl FnOnce(&mut dyn FnMut(&[u8]) -> bool)) {
        let accept = self.accept;
        let frames = &mut self.frames;
        run(&mut |frame: &[u8]| {
            if !accept {
                return false;
            }
            frames.push(frame.to_vec());
            true
        });
    }

    fn last(&self) -> &[u8] {
        self.frames.last().expect("a frame").as_slice()
    }

    fn clear(&mut self) {
        self.frames.clear();
    }

    fn receive(&mut self, stack: &mut Stack, now: u64, frame: &[u8]) {
        self.drive(|tx| stack.receive(frame, now, tx));
    }

    fn service(&mut self, stack: &mut Stack, now: u64) {
        self.drive(|tx| stack.service(now, tx));
    }

    fn send(
        &mut self,
        stack: &mut Stack,
        owner: u64,
        handle: u64,
        data: &[u8],
        now: u64,
    ) -> Result<usize, Status> {
        let mut out = None;
        self.drive(|tx| out = Some(stack.send(handle, owner, data, now, tx)));
        out.expect("send ran")
    }

    fn close(&mut self, stack: &mut Stack, owner: u64, handle: u64, now: u64) -> Status {
        let mut out = Status::Ok;
        self.drive(|tx| out = stack.close(handle, owner, now, tx));
        out
    }
}

fn configured() -> Stack {
    let mut stack = Stack::new();
    stack.set_mac(OUR_MAC);
    stack.set_config(Config {
        ip: OUR_IP,
        mask: MASK,
        gateway: PEER_IP,
        dns: 0x0A00_0203,
    });
    stack
}

/// Feeds an ARP reply so the gateway's MAC is cached before a test sends.
fn learn_gateway(stack: &mut Stack, wire: &mut Wire, now: u64) {
    let reply = arp_frame(2, PEER_MAC, PEER_IP, OUR_MAC, OUR_IP, OUR_MAC, PEER_MAC);
    wire.receive(stack, now, &reply);
    assert!(wire.frames.is_empty(), "a reply is not answered");
}

/// Opens a TCP socket, drives the SYN and completes the handshake.
fn connect(stack: &mut Stack, wire: &mut Wire, owner: u64) -> (u64, u16, u32) {
    learn_gateway(stack, wire, T0);
    let handle = stack
        .open(KIND_TCP, PEER_IP, PEER_PORT, 0, owner)
        .expect("open");

    wire.clear();
    wire.service(stack, T0);
    let syn_frame = wire.last().to_vec();
    let syn_ip = parse_ip(&syn_frame);
    let syn = parse_tcp(&syn_ip);
    assert_eq!(syn.flags, SYN);
    assert_eq!(syn.ack, 0);
    assert_eq!(syn.window, 4096);
    assert_eq!(
        syn.options.as_slice(),
        &[2, 4, (MSS >> 8) as u8, MSS as u8],
        "the SYN offers MSS"
    );
    assert_eq!(syn_ip.src, OUR_IP);
    assert_eq!(syn_ip.dst, PEER_IP);
    assert_eq!(syn.dst_port, PEER_PORT);
    assert!(syn.src_port >= 49152, "an ephemeral local port");
    let local = syn.src_port;
    let isn = syn.seq;

    wire.clear();
    let next = isn.wrapping_add(1);
    let syn_ack = tcp_packet(
        PEER_IP,
        OUR_IP,
        PEER_PORT,
        local,
        5000,
        next,
        SYN | ACK,
        &[],
        &[],
    );
    wire.receive(stack, T0, &from_peer(&syn_ack));
    assert!(stack.readable(handle, owner), "the connect completed");
    let handshake = parse_tcp(&parse_ip(&wire.last().to_vec()));
    assert_eq!(handshake.flags, ACK);
    assert_eq!(handshake.seq, next);
    assert_eq!(handshake.ack, 5001);
    (handle, local, isn)
}

/// An established connection with an empty receive arena reports a completed
/// connect for `next_any`, but a blocked `recv` must not be readied by that
/// alone: doing so would wake it forever (spec 0020, N13; ADR 0050).
#[test]
fn an_established_empty_socket_is_not_receive_ready() {
    let mut stack = configured();
    let mut wire = Wire::new();
    let (handle, _, _) = connect(&mut stack, &mut wire, 1);
    assert!(stack.readable(handle, 1), "the connect completed");

    let mut buf = [0u8; 8];
    assert_eq!(stack.recv(handle, 1, &mut buf), Err(Status::WouldBlock));
    assert_eq!(stack.arm_wait(handle, 1, T0, false), Status::Ok);
    assert!(
        !stack.wait_ready(handle, 1, T0),
        "an empty established socket is not receive-ready"
    );

    let mut out = [0u64; MAX_SOCKETS];
    assert_eq!(stack.ready_handles(T0, &mut out), 0);
}

// ---------------------------------------------------------------------------
// Packet builders (an independent reference implementation)
// ---------------------------------------------------------------------------

fn checksum(parts: &[&[u8]]) -> u16 {
    let mut data = Vec::new();
    for part in parts {
        data.extend_from_slice(part);
    }
    if data.len() % 2 != 0 {
        data.push(0);
    }
    let mut sum: u32 = 0;
    for pair in data.chunks(2) {
        sum += u32::from(u16::from_be_bytes([pair[0], pair[1]]));
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

fn pseudo_header(src: u32, dst: u32, protocol: u8, len: u16) -> Vec<u8> {
    let mut header = Vec::with_capacity(12);
    header.extend_from_slice(&src.to_be_bytes());
    header.extend_from_slice(&dst.to_be_bytes());
    header.push(0);
    header.push(protocol);
    header.extend_from_slice(&len.to_be_bytes());
    header
}

fn eth(dst: [u8; 6], src: [u8; 6], ethertype: u16, payload: &[u8]) -> Vec<u8> {
    let mut frame = vec![0u8; 14 + payload.len()];
    frame[0..6].copy_from_slice(&dst);
    frame[6..12].copy_from_slice(&src);
    frame[12..14].copy_from_slice(&ethertype.to_be_bytes());
    frame[14..].copy_from_slice(payload);
    if frame.len() < 60 {
        frame.resize(60, 0);
    }
    frame
}

fn from_peer(packet: &[u8]) -> Vec<u8> {
    eth(OUR_MAC, PEER_MAC, 0x0800, packet)
}

fn arp_frame(
    operation: u16,
    sender_mac: [u8; 6],
    sender_ip: u32,
    target_mac: [u8; 6],
    target_ip: u32,
    dst_mac: [u8; 6],
    src_mac: [u8; 6],
) -> Vec<u8> {
    let mut arp = vec![0u8; 28];
    arp[0..2].copy_from_slice(&1u16.to_be_bytes());
    arp[2..4].copy_from_slice(&0x0800u16.to_be_bytes());
    arp[4] = 6;
    arp[5] = 4;
    arp[6..8].copy_from_slice(&operation.to_be_bytes());
    arp[8..14].copy_from_slice(&sender_mac);
    arp[14..18].copy_from_slice(&sender_ip.to_be_bytes());
    arp[18..24].copy_from_slice(&target_mac);
    arp[24..28].copy_from_slice(&target_ip.to_be_bytes());
    eth(dst_mac, src_mac, 0x0806, &arp)
}

fn ipv4_packet(src: u32, dst: u32, protocol: u8, payload: &[u8], id: u16) -> Vec<u8> {
    let mut header = vec![0u8; 20];
    header[0] = 0x45;
    header[2..4].copy_from_slice(&((20 + payload.len()) as u16).to_be_bytes());
    header[4..6].copy_from_slice(&id.to_be_bytes());
    header[6..8].copy_from_slice(&0x4000u16.to_be_bytes());
    header[8] = 64;
    header[9] = protocol;
    header[12..16].copy_from_slice(&src.to_be_bytes());
    header[16..20].copy_from_slice(&dst.to_be_bytes());
    let sum = checksum(&[&header]);
    header[10..12].copy_from_slice(&sum.to_be_bytes());
    let mut packet = header;
    packet.extend_from_slice(payload);
    packet
}

/// Recomputes the IPv4 header checksum after a test edits flags or addresses.
fn fix_ipv4_checksum(packet: &mut [u8]) {
    packet[10] = 0;
    packet[11] = 0;
    let sum = checksum(&[&packet[..20]]);
    packet[10..12].copy_from_slice(&sum.to_be_bytes());
}

fn icmp_echo_request(identifier: u16, sequence: u16, data: &[u8]) -> Vec<u8> {
    let mut message = vec![0u8; 8 + data.len()];
    message[0] = 8;
    message[4..6].copy_from_slice(&identifier.to_be_bytes());
    message[6..8].copy_from_slice(&sequence.to_be_bytes());
    message[8..].copy_from_slice(data);
    let sum = checksum(&[&message]);
    message[2..4].copy_from_slice(&sum.to_be_bytes());
    message
}

fn udp_packet(src_ip: u32, dst_ip: u32, src_port: u16, dst_port: u16, payload: &[u8]) -> Vec<u8> {
    let mut datagram = vec![0u8; 8 + payload.len()];
    datagram[0..2].copy_from_slice(&src_port.to_be_bytes());
    datagram[2..4].copy_from_slice(&dst_port.to_be_bytes());
    datagram[4..6].copy_from_slice(&((8 + payload.len()) as u16).to_be_bytes());
    datagram[8..].copy_from_slice(payload);
    let pseudo = pseudo_header(src_ip, dst_ip, 17, datagram.len() as u16);
    let mut sum = checksum(&[&pseudo, &datagram]);
    if sum == 0 {
        sum = 0xFFFF;
    }
    datagram[6..8].copy_from_slice(&sum.to_be_bytes());
    ipv4_packet(src_ip, dst_ip, 17, &datagram, 8)
}

fn tcp_packet(
    src_ip: u32,
    dst_ip: u32,
    src_port: u16,
    dst_port: u16,
    seq: u32,
    ack: u32,
    flags: u8,
    options: &[u8],
    payload: &[u8],
) -> Vec<u8> {
    let header_len = 20 + options.len();
    let mut segment = vec![0u8; header_len + payload.len()];
    segment[0..2].copy_from_slice(&src_port.to_be_bytes());
    segment[2..4].copy_from_slice(&dst_port.to_be_bytes());
    segment[4..8].copy_from_slice(&seq.to_be_bytes());
    segment[8..12].copy_from_slice(&ack.to_be_bytes());
    segment[12] = ((header_len / 4) as u8) << 4;
    segment[13] = flags;
    segment[14..16].copy_from_slice(&4096u16.to_be_bytes());
    if !options.is_empty() {
        segment[20..20 + options.len()].copy_from_slice(options);
    }
    segment[header_len..].copy_from_slice(payload);
    let pseudo = pseudo_header(src_ip, dst_ip, 6, segment.len() as u16);
    let sum = checksum(&[&pseudo, &segment]);
    segment[16..18].copy_from_slice(&sum.to_be_bytes());
    ipv4_packet(src_ip, dst_ip, 6, &segment, 7)
}

// ---------------------------------------------------------------------------
// Packet parsers
// ---------------------------------------------------------------------------

struct EthFrame<'a> {
    dst: [u8; 6],
    src: [u8; 6],
    ethertype: u16,
    payload: &'a [u8],
}

fn parse_eth(frame: &[u8]) -> EthFrame<'_> {
    assert!(frame.len() >= 14, "a complete Ethernet header");
    let mut dst = [0u8; 6];
    dst.copy_from_slice(&frame[0..6]);
    let mut src = [0u8; 6];
    src.copy_from_slice(&frame[6..12]);
    EthFrame {
        dst,
        src,
        ethertype: u16::from_be_bytes([frame[12], frame[13]]),
        payload: &frame[14..],
    }
}

struct Ipv4 {
    src: u32,
    dst: u32,
    protocol: u8,
    id: u16,
    header: Vec<u8>,
    payload: Vec<u8>,
}

fn parse_ip(frame: &[u8]) -> Ipv4 {
    let eth = parse_eth(frame);
    assert_eq!(eth.ethertype, 0x0800, "an IPv4 frame");
    let packet = eth.payload;
    let header_len = usize::from(packet[0] & 0x0F) * 4;
    let total_len = usize::from(u16::from_be_bytes([packet[2], packet[3]]));
    assert_eq!(packet[0] >> 4, 4);
    let mut src = [0u8; 4];
    src.copy_from_slice(&packet[12..16]);
    let mut dst = [0u8; 4];
    dst.copy_from_slice(&packet[16..20]);
    Ipv4 {
        src: u32::from_be_bytes(src),
        dst: u32::from_be_bytes(dst),
        protocol: packet[9],
        id: u16::from_be_bytes([packet[4], packet[5]]),
        header: packet[..header_len].to_vec(),
        payload: packet[header_len..total_len].to_vec(),
    }
}

struct Tcp {
    src_port: u16,
    dst_port: u16,
    seq: u32,
    ack: u32,
    flags: u8,
    window: u16,
    options: Vec<u8>,
    payload: Vec<u8>,
    checksum_valid: bool,
}

fn parse_tcp(ip: &Ipv4) -> Tcp {
    assert_eq!(ip.protocol, 6, "a TCP segment");
    let segment = &ip.payload;
    let data_offset = usize::from(segment[12] >> 4) * 4;
    let pseudo = pseudo_header(ip.src, ip.dst, 6, segment.len() as u16);
    let checksum_valid = checksum(&[&pseudo, segment]) == 0;
    Tcp {
        src_port: u16::from_be_bytes([segment[0], segment[1]]),
        dst_port: u16::from_be_bytes([segment[2], segment[3]]),
        seq: u32::from_be_bytes([segment[4], segment[5], segment[6], segment[7]]),
        ack: u32::from_be_bytes([segment[8], segment[9], segment[10], segment[11]]),
        flags: segment[13],
        window: u16::from_be_bytes([segment[14], segment[15]]),
        options: segment[20..data_offset].to_vec(),
        payload: segment[data_offset..].to_vec(),
        checksum_valid,
    }
}

struct Udp {
    src_port: u16,
    dst_port: u16,
    payload: Vec<u8>,
    checksum_valid: bool,
}

fn parse_udp(ip: &Ipv4) -> Udp {
    assert_eq!(ip.protocol, 17, "a UDP datagram");
    let datagram = &ip.payload;
    let len = usize::from(u16::from_be_bytes([datagram[4], datagram[5]]));
    let pseudo = pseudo_header(ip.src, ip.dst, 17, len as u16);
    let checksum_valid =
        len >= 8 && len <= datagram.len() && checksum(&[&pseudo, &datagram[..len]]) == 0;
    Udp {
        src_port: u16::from_be_bytes([datagram[0], datagram[1]]),
        dst_port: u16::from_be_bytes([datagram[2], datagram[3]]),
        payload: datagram[8..len].to_vec(),
        checksum_valid,
    }
}

// ---------------------------------------------------------------------------
// Ethernet and IPv4 validation
// ---------------------------------------------------------------------------

#[test]
fn short_and_foreign_frames_are_dropped() {
    let mut stack = configured();
    let mut wire = Wire::new();
    let mut drops = stack.stats().drops;

    wire.receive(&mut stack, T0, &[0u8; 13]);
    assert_eq!(stack.stats().drops, drops + 1, "shorter than a header");

    drops = stack.stats().drops;
    let foreign = eth(OTHER_MAC, PEER_MAC, 0x0800, &[0u8; 40]);
    wire.receive(&mut stack, T0, &foreign);
    assert_eq!(stack.stats().drops, drops + 1, "another station's MAC");

    drops = stack.stats().drops;
    let unknown = eth(OUR_MAC, PEER_MAC, 0x86DD, &[0u8; 40]);
    wire.receive(&mut stack, T0, &unknown);
    assert_eq!(stack.stats().drops, drops + 1, "not ARP or IPv4");
    assert!(wire.frames.is_empty());
}

#[test]
fn malformed_ipv4_is_dropped_or_counted() {
    let mut stack = configured();
    let mut wire = Wire::new();
    let echo = icmp_echo_request(1, 1, b"ping");

    // A bad header checksum is an error.
    let mut packet = ipv4_packet(PEER_IP, OUR_IP, 1, &echo, 1);
    packet[10] ^= 0xFF;
    let errors = stack.stats().errors;
    wire.receive(&mut stack, T0, &from_peer(&packet));
    assert_eq!(stack.stats().errors, errors + 1);
    assert!(wire.frames.is_empty());

    // A fragment (MF set) is a drop.
    let mut packet = ipv4_packet(PEER_IP, OUR_IP, 1, &echo, 2);
    packet[6] |= 0x20;
    fix_ipv4_checksum(&mut packet);
    let drops = stack.stats().drops;
    wire.receive(&mut stack, T0, &from_peer(&packet));
    assert_eq!(stack.stats().drops, drops + 1);

    // A fragment with an offset set is a drop too.
    let mut packet = ipv4_packet(PEER_IP, OUR_IP, 1, &echo, 3);
    packet[7] = 1;
    fix_ipv4_checksum(&mut packet);
    let drops = stack.stats().drops;
    wire.receive(&mut stack, T0, &from_peer(&packet));
    assert_eq!(stack.stats().drops, drops + 1);

    // Another address's packet is a drop.
    let packet = ipv4_packet(0x0A00_0299, 0x0A00_0299, 1, &echo, 4);
    let drops = stack.stats().drops;
    wire.receive(&mut stack, T0, &from_peer(&packet));
    assert_eq!(stack.stats().drops, drops + 1);
    assert!(wire.frames.is_empty(), "nothing was answered");
}

// ---------------------------------------------------------------------------
// ARP
// ---------------------------------------------------------------------------

#[test]
fn arp_answers_a_request_for_our_address() {
    let mut stack = configured();
    let mut wire = Wire::new();
    let request = arp_frame(1, PEER_MAC, PEER_IP, [0; 6], OUR_IP, [0xFF; 6], PEER_MAC);
    wire.receive(&mut stack, T0, &request);

    assert_eq!(wire.frames.len(), 1);
    let frame = wire.last().to_vec();
    assert_eq!(frame.len(), 60, "padded to the Ethernet minimum");
    let eth = parse_eth(&frame);
    assert_eq!(eth.ethertype, 0x0806);
    assert_eq!(eth.dst, PEER_MAC);
    assert_eq!(eth.src, OUR_MAC);
    let arp = eth.payload;
    assert_eq!(u16::from_be_bytes([arp[6], arp[7]]), 2, "a reply");
    assert_eq!(&arp[8..14], &OUR_MAC);
    assert_eq!(
        u32::from_be_bytes([arp[14], arp[15], arp[16], arp[17]]),
        OUR_IP
    );
    assert_eq!(&arp[18..24], &PEER_MAC);
    assert_eq!(
        u32::from_be_bytes([arp[24], arp[25], arp[26], arp[27]]),
        PEER_IP
    );
}

#[test]
fn an_arp_reply_lets_ipv4_out_without_a_request() {
    let mut stack = configured();
    let mut wire = Wire::new();
    learn_gateway(&mut stack, &mut wire, T0);

    let handle = stack.open(KIND_UDP, PEER_IP, 53, 0, 1).unwrap();
    wire.clear();
    assert_eq!(wire.send(&mut stack, 1, handle, b"x", T0), Ok(1));
    let frame = wire.last().to_vec();
    assert_eq!(parse_eth(&frame).ethertype, 0x0800);
    assert_eq!(parse_eth(&frame).dst, PEER_MAC);
    assert_eq!(wire.frames.len(), 1, "no ARP request was needed");
}

#[test]
fn an_unresolved_next_hop_asks_once_per_second() {
    let mut stack = configured();
    let mut wire = Wire::new();
    let handle = stack.open(KIND_UDP, PEER_IP, 53, 0, 1).unwrap();

    wire.clear();
    assert_eq!(
        wire.send(&mut stack, 1, handle, b"x", T0),
        Err(Status::WouldBlock)
    );
    assert_eq!(wire.frames.len(), 1);
    let eth = parse_eth(wire.last());
    assert_eq!(eth.ethertype, 0x0806);
    assert_eq!(u16::from_be_bytes([eth.payload[6], eth.payload[7]]), 1);

    // A second send inside the retry window does not flood the link.
    wire.clear();
    assert_eq!(
        wire.send(&mut stack, 1, handle, b"x", T0 + 100_000_000),
        Err(Status::WouldBlock)
    );
    assert!(wire.frames.is_empty());
}

#[test]
fn arp_entries_age_after_sixty_seconds() {
    let mut stack = configured();
    let mut wire = Wire::new();
    learn_gateway(&mut stack, &mut wire, T0);
    let handle = stack.open(KIND_UDP, PEER_IP, 53, 0, 1).unwrap();
    wire.clear();
    wire.send(&mut stack, 1, handle, b"x", T0).unwrap();
    assert_eq!(parse_eth(wire.last()).ethertype, 0x0800);

    wire.clear();
    wire.service(&mut stack, T0 + 61_000_000_000);
    assert_eq!(
        wire.send(&mut stack, 1, handle, b"y", T0 + 61_000_000_000),
        Err(Status::WouldBlock)
    );
    assert_eq!(wire.frames.len(), 1);
    let eth = parse_eth(wire.last());
    assert_eq!(eth.ethertype, 0x0806, "the cache entry expired");
    assert_eq!(u16::from_be_bytes([eth.payload[6], eth.payload[7]]), 1);
}

// ---------------------------------------------------------------------------
// ICMP
// ---------------------------------------------------------------------------

#[test]
fn icmp_echo_is_answered_with_the_same_payload() {
    let mut stack = configured();
    let mut wire = Wire::new();
    learn_gateway(&mut stack, &mut wire, T0);
    wire.clear();

    let request = icmp_echo_request(7, 1, b"abcdefgh");
    let packet = ipv4_packet(PEER_IP, OUR_IP, 1, &request, 42);
    wire.receive(&mut stack, T0, &from_peer(&packet));

    let frame = wire.last().to_vec();
    let ip = parse_ip(&frame);
    assert_eq!(ip.src, OUR_IP);
    assert_eq!(ip.dst, PEER_IP);
    assert_eq!(ip.protocol, 1);
    assert_ne!(ip.id, 42);
    assert_eq!(checksum(&[&ip.header]), 0, "the reply header is valid");
    let message = ip.payload;
    assert_eq!(message[0], 0, "an echo reply");
    assert_eq!(message[1], 0);
    assert_eq!(
        &message[4..8],
        &[0, 7, 0, 1],
        "same identifier and sequence"
    );
    assert_eq!(&message[8..], b"abcdefgh");
    assert_eq!(checksum(&[&message]), 0, "the reply checksum is valid");
}

#[test]
fn icmp_rejects_a_bad_checksum_and_other_types() {
    let mut stack = configured();
    let mut wire = Wire::new();
    learn_gateway(&mut stack, &mut wire, T0);
    wire.clear();

    let mut request = icmp_echo_request(7, 1, b"abcd");
    request[2] ^= 0xFF;
    let packet = ipv4_packet(PEER_IP, OUR_IP, 1, &request, 43);
    let errors = stack.stats().errors;
    wire.receive(&mut stack, T0, &from_peer(&packet));
    assert_eq!(stack.stats().errors, errors + 1);
    assert!(wire.frames.is_empty());

    // An echo reply sent to us is not answered.
    let mut reply = icmp_echo_request(7, 1, b"abcd");
    reply[0] = 0;
    reply[2] = 0;
    reply[3] = 0;
    let sum = checksum(&[&reply]);
    reply[2..4].copy_from_slice(&sum.to_be_bytes());
    let packet = ipv4_packet(PEER_IP, OUR_IP, 1, &reply, 44);
    wire.receive(&mut stack, T0, &from_peer(&packet));
    assert!(wire.frames.is_empty());
}

// ---------------------------------------------------------------------------
// UDP
// ---------------------------------------------------------------------------

#[test]
fn udp_send_builds_a_valid_datagram() {
    let mut stack = configured();
    let mut wire = Wire::new();
    learn_gateway(&mut stack, &mut wire, T0);
    let handle = stack.open(KIND_UDP, PEER_IP, 53, 0, 1).unwrap();

    wire.clear();
    assert_eq!(wire.send(&mut stack, 1, handle, b"ping", T0), Ok(4));
    let frame = wire.last().to_vec();
    assert_eq!(frame.len(), 60);
    let ip = parse_ip(&frame);
    assert_eq!(ip.src, OUR_IP);
    assert_eq!(ip.dst, PEER_IP);
    assert_eq!(checksum(&[&ip.header]), 0);
    let udp = parse_udp(&ip);
    assert!(udp.src_port >= 49152);
    assert_eq!(udp.dst_port, 53);
    assert_eq!(udp.payload.as_slice(), b"ping");
    assert!(udp.checksum_valid);
}

#[test]
fn udp_receive_truncates_and_preserves_the_boundary() {
    let mut stack = configured();
    let mut wire = Wire::new();
    let handle = stack.open(KIND_UDP, PEER_IP, 9000, 40000, 1).unwrap();

    let packet = udp_packet(PEER_IP, OUR_IP, 9000, 40000, b"hello world");
    wire.receive(&mut stack, T0, &from_peer(&packet));

    let mut buf = [0u8; 5];
    assert_eq!(stack.recv(handle, 1, &mut buf), Ok(5));
    assert_eq!(&buf, b"hello");
    assert_eq!(
        stack.recv(handle, 1, &mut buf),
        Err(Status::WouldBlock),
        "the rest of the datagram is discarded"
    );

    // A second datagram before `recv` is dropped, one at a time only.
    let first = udp_packet(PEER_IP, OUR_IP, 9000, 40000, b"one");
    let second = udp_packet(PEER_IP, OUR_IP, 9000, 40000, b"two");
    wire.receive(&mut stack, T0, &from_peer(&first));
    let drops = stack.stats().drops;
    wire.receive(&mut stack, T0, &from_peer(&second));
    assert_eq!(stack.stats().drops, drops + 1);
    let mut buf = [0u8; 16];
    assert_eq!(stack.recv(handle, 1, &mut buf), Ok(3));
    assert_eq!(&buf[..3], b"one");
}

#[test]
fn udp_drops_wrong_ports_bad_checksums_and_unbound_traffic() {
    let mut stack = configured();
    let mut wire = Wire::new();
    let handle = stack.open(KIND_UDP, PEER_IP, 9000, 40000, 1).unwrap();

    let mut buf = [0u8; 16];
    let wrong_port = udp_packet(PEER_IP, OUR_IP, 9000, 40001, b"x");
    let drops = stack.stats().drops;
    wire.receive(&mut stack, T0, &from_peer(&wrong_port));
    assert_eq!(stack.stats().drops, drops + 1);
    assert_eq!(stack.recv(handle, 1, &mut buf), Err(Status::WouldBlock));

    let wrong_source = udp_packet(PEER_IP, OUR_IP, 9001, 40000, b"x");
    let drops = stack.stats().drops;
    wire.receive(&mut stack, T0, &from_peer(&wrong_source));
    assert_eq!(stack.stats().drops, drops + 1);

    // A corrupted UDP checksum is an error and is dropped.
    let mut packet = udp_packet(PEER_IP, OUR_IP, 9000, 40000, b"x");
    let ip_header = 20;
    packet[ip_header + 6] ^= 0xFF;
    let errors = stack.stats().errors;
    wire.receive(&mut stack, T0, &from_peer(&packet));
    assert_eq!(stack.stats().errors, errors + 1);
    assert_eq!(stack.recv(handle, 1, &mut buf), Err(Status::WouldBlock));
}

#[test]
fn udp_accepts_an_explicit_port_and_rejects_a_collision() {
    let mut stack = configured();
    let mut wire = Wire::new();
    learn_gateway(&mut stack, &mut wire, T0);

    let first = stack.open(KIND_UDP, PEER_IP, 67, 68, 1).unwrap();
    wire.clear();
    assert_eq!(wire.send(&mut stack, 1, first, b"discover", T0), Ok(8));
    assert_eq!(parse_udp(&parse_ip(&wire.last().to_vec())).src_port, 68);

    assert_eq!(
        stack.open(KIND_UDP, PEER_IP, 67, 68, 1),
        Err(Status::Invalid),
        "the local port is taken"
    );
    // TCP may use the same number: the port namespaces are per transport.
    assert!(stack.open(KIND_TCP, PEER_IP, 67, 68, 1).is_ok());
}

#[test]
fn a_broadcast_udp_socket_accepts_any_source_and_can_send() {
    let mut stack = configured();
    let mut wire = Wire::new();
    learn_gateway(&mut stack, &mut wire, T0);

    let handle = stack.open(KIND_UDP, 0xFFFF_FFFF, 67, 68, 1).unwrap();
    wire.clear();
    assert_eq!(wire.send(&mut stack, 1, handle, b"discover", T0), Ok(8));
    assert_eq!(parse_eth(wire.last()).dst, [0xFF; 6], "broadcast, no ARP");

    // A reply from the server's unicast address is accepted.
    let reply = udp_packet(PEER_IP, OUR_IP, 67, 68, b"offer");
    wire.receive(&mut stack, T0, &from_peer(&reply));
    let mut buf = [0u8; 16];
    assert_eq!(stack.recv(handle, 1, &mut buf), Ok(5));
    assert_eq!(&buf[..5], b"offer");
}

#[test]
fn a_broadcast_socket_sends_before_an_address_is_configured() {
    let mut stack = Stack::new();
    stack.set_mac(OUR_MAC);
    let mut wire = Wire::new();

    // A DHCP client has no address yet: the frame leaves from 0.0.0.0 to the
    // limited broadcast without an ARP request.
    assert_eq!(stack.config().ip, 0);
    let handle = stack.open(KIND_UDP, 0xFFFF_FFFF, 67, 68, 1).unwrap();
    assert_eq!(wire.send(&mut stack, 1, handle, b"discover", T0), Ok(8));
    let frame = wire.last().to_vec();
    assert_eq!(parse_eth(&frame).dst, [0xFF; 6], "broadcast MAC");
    let ip = parse_ip(&frame);
    assert_eq!(ip.src, 0, "an unconfigured source address");
    assert_eq!(ip.dst, 0xFFFF_FFFF);
    let udp = parse_udp(&ip);
    assert!(udp.checksum_valid, "the broadcast checksum is valid");
    assert_eq!((udp.src_port, udp.dst_port), (68, 67));

    // The server's broadcast reply is accepted from any source, and the
    // unconfigured stack still hands it to the socket.
    let reply = udp_packet(PEER_IP, 0xFFFF_FFFF, 67, 68, b"offer");
    wire.receive(&mut stack, T0, &from_peer(&reply));
    let mut buf = [0u8; 16];
    assert_eq!(stack.recv(handle, 1, &mut buf), Ok(5));
    assert_eq!(&buf[..5], b"offer");
}

#[test]
fn a_broadcast_datagram_validates_against_its_own_destination() {
    let mut stack = configured();
    let mut wire = Wire::new();
    let handle = stack.open(KIND_UDP, 0xFFFF_FFFF, 67, 68, 1).unwrap();

    // The checksum's pseudo-header names the broadcast destination, not our
    // configured address.
    let packet = udp_packet(PEER_IP, 0xFFFF_FFFF, 67, 68, b"offer");
    wire.receive(&mut stack, T0, &from_peer(&packet));
    let mut buf = [0u8; 8];
    assert_eq!(stack.recv(handle, 1, &mut buf), Ok(5));
    assert_eq!(&buf[..5], b"offer");
}

#[test]
fn udp_send_is_bounded_by_the_frame() {
    let mut stack = configured();
    let mut wire = Wire::new();
    learn_gateway(&mut stack, &mut wire, T0);
    let handle = stack.open(KIND_UDP, PEER_IP, 53, 0, 1).unwrap();
    let oversized = [0u8; 1473];
    assert_eq!(
        wire.send(&mut stack, 1, handle, &oversized, T0),
        Err(Status::CapacityExceeded)
    );
    let largest = [0u8; 1472];
    assert_eq!(wire.send(&mut stack, 1, handle, &largest, T0), Ok(1472));
}

// ---------------------------------------------------------------------------
// TCP
// ---------------------------------------------------------------------------

#[test]
fn tcp_handshake_data_fin_and_close() {
    let mut stack = configured();
    let mut wire = Wire::new();
    let (handle, local, isn) = connect(&mut stack, &mut wire, 1);

    // Send the request.
    wire.clear();
    let request = b"GET / HTTP/1.0\r\n\r\n";
    assert_eq!(
        wire.send(&mut stack, 1, handle, request, T0),
        Ok(request.len())
    );
    let sent = parse_tcp(&parse_ip(&wire.last().to_vec()));
    assert_eq!(sent.flags, PSH | ACK);
    assert_eq!(sent.seq, isn.wrapping_add(1));
    assert_eq!(sent.ack, 5001);
    assert_eq!(sent.payload.as_slice(), request);
    assert!(sent.checksum_valid);
    assert!(sent.window > 0);

    // A second send waits for the acknowledgement.
    assert_eq!(
        wire.send(&mut stack, 1, handle, b"more", T0),
        Err(Status::WouldBlock)
    );

    // The peer acknowledges the request; nothing needs to be answered.
    wire.clear();
    let ack = tcp_packet(
        PEER_IP,
        OUR_IP,
        PEER_PORT,
        local,
        5001,
        isn.wrapping_add(1 + request.len() as u32),
        ACK,
        &[],
        &[],
    );
    wire.receive(&mut stack, T0, &from_peer(&ack));
    assert!(wire.frames.is_empty());

    // Data arrives; it is queued and acknowledged.
    let body = b"HTTP/1.0 200 OK\r\n\r\nhello";
    let data = tcp_packet(
        PEER_IP,
        OUR_IP,
        PEER_PORT,
        local,
        5001,
        isn.wrapping_add(1 + request.len() as u32),
        ACK | PSH,
        &[],
        body,
    );
    wire.receive(&mut stack, T0, &from_peer(&data));
    let ack = parse_tcp(&parse_ip(&wire.last().to_vec()));
    assert_eq!(ack.flags, ACK);
    assert_eq!(ack.seq, isn.wrapping_add(1 + request.len() as u32));
    assert_eq!(ack.ack, 5001 + body.len() as u32);
    let mut buf = [0u8; 64];
    assert_eq!(stack.recv(handle, 1, &mut buf), Ok(body.len()));
    assert_eq!(&buf[..body.len()], body);
    assert_eq!(stack.recv(handle, 1, &mut buf), Err(Status::WouldBlock));

    // FIN: EOF after the queue drains.
    wire.clear();
    let fin = tcp_packet(
        PEER_IP,
        OUR_IP,
        PEER_PORT,
        local,
        5001 + body.len() as u32,
        isn.wrapping_add(1 + request.len() as u32),
        ACK | FIN,
        &[],
        &[],
    );
    wire.receive(&mut stack, T0, &from_peer(&fin));
    let ack = parse_tcp(&parse_ip(&wire.last().to_vec()));
    assert_eq!(ack.ack, 5001 + body.len() as u32 + 1);
    assert_eq!(stack.recv(handle, 1, &mut buf), Ok(0));

    // Close sends FIN and waits for the final ACK.
    wire.clear();
    assert_eq!(wire.close(&mut stack, 1, handle, T0), Status::Ok);
    let fin = parse_tcp(&parse_ip(&wire.last().to_vec()));
    assert_eq!(fin.flags, FIN | ACK);
    assert_eq!(fin.seq, isn.wrapping_add(1 + request.len() as u32));
    assert_eq!(fin.ack, 5001 + body.len() as u32 + 1);

    wire.clear();
    let final_ack = tcp_packet(
        PEER_IP,
        OUR_IP,
        PEER_PORT,
        local,
        5001 + body.len() as u32 + 1,
        isn.wrapping_add(2 + request.len() as u32),
        ACK,
        &[],
        &[],
    );
    wire.receive(&mut stack, T0, &from_peer(&final_ack));
    assert!(wire.frames.is_empty());
    assert_eq!(
        stack.recv(handle, 1, &mut buf),
        Err(Status::NotFound),
        "the slot was released after the close handshake"
    );
}

#[test]
fn tcp_send_is_bounded_by_the_mss() {
    let mut stack = configured();
    let mut wire = Wire::new();
    let (handle, _local, _isn) = connect(&mut stack, &mut wire, 1);

    wire.clear();
    let big = vec![0xABu8; 4000];
    assert_eq!(wire.send(&mut stack, 1, handle, &big, T0), Ok(MSS));
    let segment = parse_tcp(&parse_ip(&wire.last().to_vec()));
    assert_eq!(segment.payload.len(), MSS);
    assert!(segment.payload.iter().all(|byte| *byte == 0xAB));
    assert_eq!(
        wire.send(&mut stack, 1, handle, b"more", T0),
        Err(Status::WouldBlock)
    );
}

#[test]
fn tcp_retransmits_with_exponential_backoff_and_gives_up() {
    let mut stack = configured();
    let mut wire = Wire::new();
    let (handle, _local, isn) = connect(&mut stack, &mut wire, 1);
    wire.clear();
    assert_eq!(wire.send(&mut stack, 1, handle, b"GET", T0), Ok(3));

    // 1 s: the same segment is sent again.
    wire.clear();
    wire.service(&mut stack, T0 + 1_000_000_000);
    let first = parse_tcp(&parse_ip(&wire.last().to_vec()));
    assert_eq!(first.flags, PSH | ACK);
    assert_eq!(first.seq, isn.wrapping_add(1));
    assert_eq!(first.payload.as_slice(), b"GET");

    // Not again before the doubled timeout.
    wire.clear();
    wire.service(&mut stack, T0 + 2_000_000_000);
    assert!(wire.frames.is_empty());

    // 1 s + 2 s: the second retransmission.
    wire.clear();
    wire.service(&mut stack, T0 + 3_000_000_001);
    assert_eq!(wire.frames.len(), 1);
    assert_eq!(parse_tcp(&parse_ip(wire.last())).seq, isn.wrapping_add(1));

    // The third and fourth retransmissions after 4 s and 8 s.
    wire.clear();
    wire.service(&mut stack, T0 + 7_000_000_001);
    assert_eq!(wire.frames.len(), 1);
    wire.clear();
    wire.service(&mut stack, T0 + 15_000_000_001);
    assert_eq!(wire.frames.len(), 1);

    // After five attempts the connection is dead.
    wire.clear();
    wire.service(&mut stack, T0 + 31_000_000_001);
    assert!(wire.frames.is_empty());
    assert!(stack.readable(handle, 1));
    assert_eq!(stack.recv(handle, 1, &mut [0u8; 8]), Err(Status::Closed));
}

#[test]
fn tcp_connect_retries_and_times_out_after_five_seconds() {
    let mut stack = configured();
    let mut wire = Wire::new();
    learn_gateway(&mut stack, &mut wire, T0);
    let handle = stack.open(KIND_TCP, PEER_IP, PEER_PORT, 0, 1).unwrap();

    wire.clear();
    wire.service(&mut stack, T0);
    assert_eq!(parse_tcp(&parse_ip(wire.last())).flags, SYN);

    // The SYN is retried every 200 ms, not on every service call.
    wire.clear();
    wire.service(&mut stack, T0 + 100_000_000);
    assert!(wire.frames.is_empty());
    wire.service(&mut stack, T0 + 210_000_000);
    assert_eq!(wire.frames.len(), 1);
    assert_eq!(parse_tcp(&parse_ip(wire.last())).flags, SYN);

    // A wrong SYN-ACK is ignored.
    let syn = parse_tcp(&parse_ip(&wire.last().to_vec()));
    let bogus = tcp_packet(
        PEER_IP,
        OUR_IP,
        PEER_PORT,
        syn.src_port,
        5000,
        syn.seq.wrapping_add(2),
        SYN | ACK,
        &[],
        &[],
    );
    wire.clear();
    wire.receive(&mut stack, T0 + 220_000_000, &from_peer(&bogus));
    assert!(wire.frames.is_empty(), "the wrong ack changes nothing");
    assert!(!stack.readable(handle, 1));

    // Five seconds after the first attempt the connect fails with Closed.
    wire.clear();
    wire.service(&mut stack, T0 + 5_000_000_001);
    assert!(stack.readable(handle, 1));
    assert_eq!(stack.recv(handle, 1, &mut [0u8; 8]), Err(Status::Closed));
}

#[test]
fn tcp_reset_closes_the_socket() {
    let mut stack = configured();
    let mut wire = Wire::new();
    let (handle, local, isn) = connect(&mut stack, &mut wire, 1);

    let reset = tcp_packet(
        PEER_IP,
        OUR_IP,
        PEER_PORT,
        local,
        5000,
        isn.wrapping_add(2),
        RST | ACK,
        &[],
        &[],
    );
    wire.receive(&mut stack, T0, &from_peer(&reset));
    assert!(stack.readable(handle, 1));
    assert_eq!(stack.recv(handle, 1, &mut [0u8; 8]), Err(Status::Closed));
    assert_eq!(wire.close(&mut stack, 1, handle, T0), Status::Ok);
    assert_eq!(stack.recv(handle, 1, &mut [0u8; 8]), Err(Status::NotFound));
}

#[test]
fn tcp_drops_duplicates_and_out_of_order_segments() {
    let mut stack = configured();
    let mut wire = Wire::new();
    let (handle, local, isn) = connect(&mut stack, &mut wire, 1);
    let send_seq = isn.wrapping_add(1);

    // In-order data.
    let first = tcp_packet(
        PEER_IP,
        OUR_IP,
        PEER_PORT,
        local,
        5001,
        send_seq,
        ACK | PSH,
        &[],
        b"abc",
    );
    wire.receive(&mut stack, T0, &from_peer(&first));
    let mut buf = [0u8; 8];
    assert_eq!(stack.recv(handle, 1, &mut buf), Ok(3));
    assert_eq!(&buf[..3], b"abc");

    // A duplicate is acknowledged again but not queued.
    wire.clear();
    let duplicate = tcp_packet(
        PEER_IP,
        OUR_IP,
        PEER_PORT,
        local,
        5001,
        send_seq,
        ACK | PSH,
        &[],
        b"abc",
    );
    wire.receive(&mut stack, T0, &from_peer(&duplicate));
    let ack = parse_tcp(&parse_ip(&wire.last().to_vec()));
    assert_eq!(ack.flags, ACK);
    assert_eq!(ack.ack, 5004);
    assert_eq!(stack.recv(handle, 1, &mut buf), Err(Status::WouldBlock));

    // An out-of-order segment is answered for the expected sequence and dropped.
    wire.clear();
    let future = tcp_packet(
        PEER_IP,
        OUR_IP,
        PEER_PORT,
        local,
        5010,
        send_seq,
        ACK | PSH,
        &[],
        b"zzz",
    );
    wire.receive(&mut stack, T0, &from_peer(&future));
    let ack = parse_tcp(&parse_ip(&wire.last().to_vec()));
    assert_eq!(ack.flags, ACK);
    assert_eq!(ack.ack, 5004);
    assert_eq!(stack.recv(handle, 1, &mut buf), Err(Status::WouldBlock));
}

#[test]
fn tcp_receive_window_closes_and_reopens() {
    let mut stack = configured();
    let mut wire = Wire::new();
    let (handle, local, isn) = connect(&mut stack, &mut wire, 1);
    let send_seq = isn.wrapping_add(1);

    // Three segments fill the 4096-byte arena exactly.
    let mut seq = 5001;
    let mut last_ack = Vec::new();
    for size in [1460usize, 1460, 1176] {
        let chunk = vec![0x42u8; size];
        let packet = tcp_packet(
            PEER_IP,
            OUR_IP,
            PEER_PORT,
            local,
            seq,
            send_seq,
            ACK | PSH,
            &[],
            &chunk,
        );
        wire.clear();
        wire.receive(&mut stack, T0, &from_peer(&packet));
        last_ack = wire.last().to_vec();
        seq += size as u32;
    }
    let ack = parse_tcp(&parse_ip(&last_ack));
    assert_eq!(ack.ack, seq);
    assert_eq!(ack.window, 0, "the arena is full");

    // A fourth segment cannot be accepted, so it is not acknowledged.
    wire.clear();
    let extra = tcp_packet(
        PEER_IP,
        OUR_IP,
        PEER_PORT,
        local,
        seq,
        send_seq,
        ACK | PSH,
        &[],
        &[0x43u8; 100],
    );
    wire.receive(&mut stack, T0, &from_peer(&extra));
    assert!(wire.frames.is_empty());

    // Reading part of the queue reopens the window on the next service pass.
    let mut buf = vec![0u8; 1024];
    assert_eq!(stack.recv(handle, 1, &mut buf), Ok(1024));
    wire.clear();
    wire.service(&mut stack, T0);
    let update = parse_tcp(&parse_ip(&wire.last().to_vec()));
    assert_eq!(update.flags, ACK);
    assert_eq!(update.ack, seq);
    assert_eq!(update.window, 1024);

    // The retransmitted segment now fits and is queued.
    wire.clear();
    wire.receive(&mut stack, T0, &from_peer(&extra));
    let ack = parse_tcp(&parse_ip(&wire.last().to_vec()));
    assert_eq!(ack.ack, seq + 100);
    assert_eq!(ack.window, 924);
}

#[test]
fn tcp_close_in_fin_wait_one_moves_through_fin_wait_two() {
    let mut stack = configured();
    let mut wire = Wire::new();
    let (handle, local, isn) = connect(&mut stack, &mut wire, 1);

    // Close while established: FIN is sent, state FIN-WAIT-1.
    wire.clear();
    assert_eq!(wire.close(&mut stack, 1, handle, T0), Status::Ok);
    let fin = parse_tcp(&parse_ip(&wire.last().to_vec()));
    assert_eq!(fin.flags, FIN | ACK);
    let fin_seq = fin.seq;

    // ACK of the FIN moves to FIN-WAIT-2; the socket stays live.
    wire.clear();
    let ack = tcp_packet(
        PEER_IP,
        OUR_IP,
        PEER_PORT,
        local,
        5001,
        fin_seq.wrapping_add(1),
        ACK,
        &[],
        &[],
    );
    wire.receive(&mut stack, T0, &from_peer(&ack));
    assert!(wire.frames.is_empty());
    assert_eq!(
        stack.recv(handle, 1, &mut [0u8; 8]),
        Err(Status::WouldBlock)
    );

    // The peer's FIN finishes the close and releases the slot.
    let fin = tcp_packet(
        PEER_IP,
        OUR_IP,
        PEER_PORT,
        local,
        5001,
        fin_seq.wrapping_add(1),
        ACK | FIN,
        &[],
        &[],
    );
    wire.receive(&mut stack, T0, &from_peer(&fin));
    let ack = parse_tcp(&parse_ip(&wire.last().to_vec()));
    assert_eq!(ack.ack, 5002);
    assert_eq!(stack.recv(handle, 1, &mut [0u8; 8]), Err(Status::NotFound));
    let _ = isn;
}

#[test]
fn tcp_close_owner_sends_a_fin_and_releases() {
    let mut stack = configured();
    let mut wire = Wire::new();
    let (handle, _local, _isn) = connect(&mut stack, &mut wire, 1);

    wire.clear();
    wire.drive(|tx| stack.close_owner(1, T0, tx));
    let fin = parse_tcp(&parse_ip(&wire.last().to_vec()));
    assert_eq!(fin.flags, FIN | ACK);
    assert_eq!(stack.recv(handle, 1, &mut [0u8; 8]), Err(Status::NotFound));
}

// ---------------------------------------------------------------------------
// TCP listener (M10.7, spec 0020 N10)
// ---------------------------------------------------------------------------

const LISTEN_PORT: u16 = 7;
const CLIENT_ONE: u16 = 40_000;

/// Sends one SYN to a listening port and returns the server ISN from its
/// SYN-ACK, checking the acknowledgement, the MSS option and the window.
fn syn_to_listener(
    stack: &mut Stack,
    wire: &mut Wire,
    port: u16,
    client_port: u16,
    client_seq: u32,
    now: u64,
) -> u32 {
    wire.clear();
    let syn = tcp_packet(PEER_IP, OUR_IP, client_port, port, client_seq, 0, SYN, &[], &[]);
    wire.receive(stack, now, &from_peer(&syn));
    assert_eq!(wire.frames.len(), 1, "the listener answers with SYN-ACK");
    let reply = parse_tcp(&parse_ip(&wire.last().to_vec()));
    assert_eq!(reply.flags, SYN | ACK);
    assert_eq!(reply.src_port, port);
    assert_eq!(reply.dst_port, client_port);
    assert_eq!(reply.ack, client_seq.wrapping_add(1));
    assert_eq!(reply.window, 4096);
    assert_eq!(
        reply.options.as_slice(),
        &[2, 4, (MSS >> 8) as u8, MSS as u8],
        "the SYN-ACK offers MSS"
    );
    assert!(reply.checksum_valid);
    reply.seq
}

/// Sends the final ACK of the passive handshake.
fn complete_to_listener(
    stack: &mut Stack,
    wire: &mut Wire,
    port: u16,
    client_port: u16,
    client_seq: u32,
    server_isn: u32,
    now: u64,
) {
    wire.clear();
    let ack = tcp_packet(
        PEER_IP,
        OUR_IP,
        client_port,
        port,
        client_seq.wrapping_add(1),
        server_isn.wrapping_add(1),
        ACK,
        &[],
        &[],
    );
    wire.receive(stack, now, &from_peer(&ack));
}

/// Opens a listener on `port` for `owner`.
fn listener(stack: &mut Stack, owner: u64, port: u16) -> u64 {
    stack
        .open(KIND_LISTEN, 0, 0, port, owner)
        .expect("a listener opens")
}

#[test]
fn tcp_listener_accepts_and_echoes_a_line() {
    let mut stack = configured();
    let mut wire = Wire::new();
    let owner = 1;
    let listen = listener(&mut stack, owner, LISTEN_PORT);

    let mut buf = [0u8; 64];
    assert_eq!(stack.accept(listen, owner), Err(Status::WouldBlock));
    assert!(!stack.readable(listen, owner));

    let client_seq = 5000;
    let isn = syn_to_listener(&mut stack, &mut wire, LISTEN_PORT, CLIENT_ONE, client_seq, T0);
    assert_eq!(
        stack.accept(listen, owner),
        Err(Status::WouldBlock),
        "a half-open connection is not accepted"
    );
    assert!(!stack.readable(listen, owner));

    complete_to_listener(
        &mut stack,
        &mut wire,
        LISTEN_PORT,
        CLIENT_ONE,
        client_seq,
        isn,
        T0,
    );
    assert!(wire.frames.is_empty(), "the final ACK needs no answer");
    assert!(stack.readable(listen, owner), "the connection is pending");

    let peer = stack.accept(listen, owner).expect("accept");
    assert_ne!(peer, listen);
    assert_eq!(peer & 0xFF, 1, "a distinct connection slot");
    assert_eq!(stack.accept(listen, owner), Err(Status::WouldBlock));
    assert!(!stack.readable(listen, owner));
    assert!(stack.readable(peer, owner), "an established connection");

    // Data arrives, is queued, acknowledged and readable.
    wire.clear();
    let line = b"hello-1\n";
    let data = tcp_packet(
        PEER_IP,
        OUR_IP,
        CLIENT_ONE,
        LISTEN_PORT,
        client_seq.wrapping_add(1),
        isn.wrapping_add(1),
        ACK | PSH,
        &[],
        line,
    );
    wire.receive(&mut stack, T0, &from_peer(&data));
    let ack = parse_tcp(&parse_ip(&wire.last().to_vec()));
    assert_eq!(ack.flags, ACK);
    assert_eq!(ack.ack, client_seq.wrapping_add(1 + line.len() as u32));
    assert_eq!(stack.recv(peer, owner, &mut buf), Ok(line.len()));
    assert_eq!(&buf[..line.len()], line);

    // The echo is one segment on the accepted connection.
    wire.clear();
    assert_eq!(wire.send(&mut stack, owner, peer, line, T0), Ok(line.len()));
    let echo = parse_tcp(&parse_ip(&wire.last().to_vec()));
    assert_eq!(echo.flags, PSH | ACK);
    assert_eq!(echo.seq, isn.wrapping_add(1));
    assert_eq!(echo.ack, client_seq.wrapping_add(1 + line.len() as u32));
    assert_eq!(echo.payload.as_slice(), line);
    assert!(echo.checksum_valid);

    // The client acknowledges the echo.
    wire.clear();
    let ack = tcp_packet(
        PEER_IP,
        OUR_IP,
        CLIENT_ONE,
        LISTEN_PORT,
        client_seq.wrapping_add(1 + line.len() as u32),
        isn.wrapping_add(1 + line.len() as u32),
        ACK,
        &[],
        &[],
    );
    wire.receive(&mut stack, T0, &from_peer(&ack));
    assert!(wire.frames.is_empty());

    // FIN: EOF, then our close completes and releases the slot.
    wire.clear();
    let fin = tcp_packet(
        PEER_IP,
        OUR_IP,
        CLIENT_ONE,
        LISTEN_PORT,
        client_seq.wrapping_add(1 + line.len() as u32),
        isn.wrapping_add(1 + line.len() as u32),
        ACK | FIN,
        &[],
        &[],
    );
    wire.receive(&mut stack, T0, &from_peer(&fin));
    let ack = parse_tcp(&parse_ip(&wire.last().to_vec()));
    assert_eq!(ack.ack, client_seq.wrapping_add(2 + line.len() as u32));
    assert_eq!(stack.recv(peer, owner, &mut buf), Ok(0), "end of file");

    wire.clear();
    assert_eq!(wire.close(&mut stack, owner, peer, T0), Status::Ok);
    let our_fin = parse_tcp(&parse_ip(&wire.last().to_vec()));
    assert_eq!(our_fin.flags, FIN | ACK);
    let final_ack = tcp_packet(
        PEER_IP,
        OUR_IP,
        CLIENT_ONE,
        LISTEN_PORT,
        client_seq.wrapping_add(2 + line.len() as u32),
        isn.wrapping_add(2 + line.len() as u32),
        ACK,
        &[],
        &[],
    );
    wire.receive(&mut stack, T0, &from_peer(&final_ack));
    assert_eq!(
        stack.recv(peer, owner, &mut buf),
        Err(Status::NotFound),
        "the connection slot was released"
    );
}

#[test]
fn tcp_listener_backlog_is_two_and_refuses_a_third() {
    let mut stack = configured();
    let mut wire = Wire::new();
    let owner = 1;
    let listen = listener(&mut stack, owner, LISTEN_PORT);

    for slot in 0..LISTEN_BACKLOG {
        let client_port = CLIENT_ONE + slot as u16;
        let client_seq = 6000 + slot as u32 * 100;
        let isn = syn_to_listener(&mut stack, &mut wire, LISTEN_PORT, client_port, client_seq, T0);
        complete_to_listener(
            &mut stack,
            &mut wire,
            LISTEN_PORT,
            client_port,
            client_seq,
            isn,
            T0,
        );
        assert!(wire.frames.is_empty());
    }
    assert!(stack.readable(listen, owner), "two connections are pending");

    // A third completed connection is refused with RST and released while the
    // queue is still full.
    let client_port = CLIENT_ONE + 2;
    let client_seq = 6200;
    let isn = syn_to_listener(&mut stack, &mut wire, LISTEN_PORT, client_port, client_seq, T0);
    wire.clear();
    let ack = tcp_packet(
        PEER_IP,
        OUR_IP,
        client_port,
        LISTEN_PORT,
        client_seq.wrapping_add(1),
        isn.wrapping_add(1),
        ACK,
        &[],
        &[],
    );
    wire.receive(&mut stack, T0, &from_peer(&ack));
    let reset = parse_tcp(&parse_ip(&wire.last().to_vec()));
    assert_eq!(reset.flags & RST, RST, "the third connection is refused");

    // Both queued connections are valid and distinct.
    let first = stack.accept(listen, owner).expect("accept one");
    let second = stack.accept(listen, owner).expect("accept two");
    assert_ne!(first, second);
    assert_eq!(stack.accept(listen, owner), Err(Status::WouldBlock));
    let mut buf = [0u8; 8];
    assert_eq!(stack.recv(first, owner, &mut buf), Err(Status::WouldBlock));
    assert_eq!(stack.recv(second, owner, &mut buf), Err(Status::WouldBlock));
}

#[test]
fn tcp_listener_close_releases_pending_and_half_open_connections() {
    let mut stack = configured();
    let mut wire = Wire::new();
    let owner = 1;
    let listen = listener(&mut stack, owner, LISTEN_PORT);

    let client_seq = 7000;
    let isn = syn_to_listener(&mut stack, &mut wire, LISTEN_PORT, CLIENT_ONE, client_seq, T0);
    complete_to_listener(
        &mut stack,
        &mut wire,
        LISTEN_PORT,
        CLIENT_ONE,
        client_seq,
        isn,
        T0,
    );
    let peer = stack.accept(listen, owner).expect("accept");

    // A second connection is still half-open when the listener closes.
    let _ = syn_to_listener(
        &mut stack,
        &mut wire,
        LISTEN_PORT,
        CLIENT_ONE + 1,
        7100,
        T0,
    );

    assert_eq!(stack.close(listen, owner, T0, &mut |_: &[u8]| true), Status::Ok);
    assert_eq!(stack.accept(listen, owner), Err(Status::NotFound));
    assert_eq!(
        stack.recv(peer, owner, &mut [0u8; 8]),
        Err(Status::WouldBlock),
        "an accepted connection is the application's, not the listener's"
    );

    // The listener and the half-open connection are gone; the accepted
    // connection still holds one slot.
    let mut handles = Vec::new();
    for _ in 0..MAX_SOCKETS - 1 {
        handles.push(
            stack
                .open(KIND_UDP, PEER_IP, 53, 0, owner)
                .expect("a free slot"),
        );
    }
    assert_eq!(
        stack.open(KIND_UDP, PEER_IP, 53, 0, owner),
        Err(Status::Full)
    );
}

#[test]
fn tcp_listener_checks_ownership_kind_and_bind_port() {
    let mut stack = configured();
    let owner = 1;
    let listen = listener(&mut stack, owner, LISTEN_PORT);
    let client = stack.open(KIND_TCP, PEER_IP, PEER_PORT, 0, owner).unwrap();

    assert_eq!(stack.accept(listen, 2), Err(Status::Invalid));
    assert_eq!(stack.accept(client, owner), Err(Status::Invalid));
    assert_eq!(
        stack.send(listen, owner, b"x", T0, &mut |_: &[u8]| true),
        Err(Status::Invalid)
    );
    assert_eq!(stack.recv(listen, owner, &mut [0u8; 8]), Err(Status::Invalid));
    assert_eq!(
        stack.close(listen, 2, T0, &mut |_: &[u8]| true),
        Status::Invalid
    );
    // A listener must bind a real port, and one port has one listener.
    assert_eq!(
        stack.open(KIND_LISTEN, 0, 0, 0, owner),
        Err(Status::Invalid)
    );
    assert_eq!(
        stack.open(KIND_LISTEN, 0, 0, LISTEN_PORT, owner),
        Err(Status::Invalid)
    );
    // A listener handle is not a connection, and vice versa.
    assert!(!stack.readable(client, owner));
    assert_eq!(stack.accept(listen, owner), Err(Status::WouldBlock));
}

#[test]
fn tcp_listener_retransmits_the_syn_ack_and_expires_a_half_open_connection() {
    let mut stack = configured();
    let mut wire = Wire::new();
    let owner = 1;
    let listen = listener(&mut stack, owner, LISTEN_PORT);
    let client_seq = 8000;

    let isn = syn_to_listener(&mut stack, &mut wire, LISTEN_PORT, CLIENT_ONE, client_seq, T0);
    let first = parse_tcp(&parse_ip(&wire.last().to_vec()));

    wire.clear();
    wire.service(&mut stack, T0 + SYN_RETRY_NS);
    let again = parse_tcp(&parse_ip(&wire.last().to_vec()));
    assert_eq!(again.flags, SYN | ACK);
    assert_eq!(again.seq, first.seq, "the same sequence number");
    assert_eq!(again.ack, first.ack);
    assert_eq!(again.options, first.options);

    // No final ACK: the half-open slot expires and is released.
    wire.clear();
    wire.service(&mut stack, T0 + CONNECT_TIMEOUT_NS + 1);
    assert!(wire.frames.is_empty());
    // The listener plus seven new sockets fill the table; the half-open slot
    // is gone.
    for _ in 0..MAX_SOCKETS - 1 {
        assert!(stack.open(KIND_UDP, PEER_IP, 53, 0, owner).is_ok());
    }
    assert_eq!(
        stack.open(KIND_UDP, PEER_IP, 53, 0, owner),
        Err(Status::Full)
    );
    let _ = isn;
}

#[test]
fn a_syn_with_no_listener_is_dropped() {
    let mut stack = configured();
    let mut wire = Wire::new();
    let syn = tcp_packet(PEER_IP, OUR_IP, CLIENT_ONE, LISTEN_PORT, 9000, 0, SYN, &[], &[]);
    let mut drops = stack.stats().drops;
    wire.receive(&mut stack, T0, &from_peer(&syn));
    assert!(wire.frames.is_empty(), "no RST is sent");
    assert_eq!(stack.stats().drops, drops + 1);
    drops = stack.stats().drops;
    // A bare ACK for an unknown connection is dropped too.
    let ack = tcp_packet(
        PEER_IP,
        OUR_IP,
        CLIENT_ONE,
        LISTEN_PORT,
        9001,
        1,
        ACK,
        &[],
        &[],
    );
    wire.receive(&mut stack, T0, &from_peer(&ack));
    assert!(wire.frames.is_empty());
    assert_eq!(stack.stats().drops, drops + 1);
}

#[test]
fn tcp_rejects_a_foreign_owner_and_a_stale_handle() {
    let mut stack = configured();
    let mut wire = Wire::new();
    learn_gateway(&mut stack, &mut wire, T0);
    let handle = stack.open(KIND_TCP, PEER_IP, PEER_PORT, 0, 1).unwrap();

    let mut buf = [0u8; 8];
    assert_eq!(
        stack.send(handle, 2, b"x", T0, &mut |_: &[u8]| true),
        Err(Status::Invalid)
    );
    assert_eq!(stack.recv(handle, 2, &mut buf), Err(Status::Invalid));
    assert_eq!(
        stack.close(handle, 2, T0, &mut |_: &[u8]| true),
        Status::Invalid
    );
    assert!(!stack.readable(handle, 2));

    // In SYN-SENT a close frees the slot immediately; the handle is stale.
    assert_eq!(wire.close(&mut stack, 1, handle, T0), Status::Ok);
    assert_eq!(stack.recv(handle, 1, &mut buf), Err(Status::NotFound));
    assert_eq!(
        stack.send(handle, 1, b"x", T0, &mut |_: &[u8]| true),
        Err(Status::NotFound)
    );

    // The index is reused with a new generation.
    let again = stack.open(KIND_TCP, PEER_IP, PEER_PORT, 0, 1).unwrap();
    assert_eq!(again & 0xFF, handle & 0xFF);
    assert_ne!(again >> 8, handle >> 8);
    assert_eq!(stack.recv(handle, 1, &mut buf), Err(Status::NotFound));
}

#[test]
fn the_socket_table_is_full_at_eight() {
    let mut stack = configured();
    let first = stack.open(KIND_UDP, PEER_IP, 53, 0, 1).unwrap();
    for _ in 1..MAX_SOCKETS {
        assert!(stack.open(KIND_UDP, PEER_IP, 53, 0, 1).is_ok());
    }
    assert_eq!(stack.open(KIND_UDP, PEER_IP, 53, 0, 1), Err(Status::Full));
    // Freeing one makes room again.
    assert_eq!(stack.close(first, 1, T0, &mut |_: &[u8]| true), Status::Ok);
    assert!(stack.open(KIND_UDP, PEER_IP, 53, 0, 1).is_ok());
}

#[test]
fn a_bad_kind_is_rejected() {
    let mut stack = configured();
    assert_eq!(stack.open(0, PEER_IP, 53, 0, 1), Err(Status::Invalid));
    assert_eq!(stack.open(4, PEER_IP, 53, 0, 1), Err(Status::Invalid));
    // The listener kind is accepted, but it must bind a local port.
    assert_eq!(stack.open(KIND_LISTEN, PEER_IP, 53, 0, 1), Err(Status::Invalid));
    assert!(stack.open(KIND_LISTEN, PEER_IP, 53, 80, 1).is_ok());
}

#[test]
fn close_owner_only_touches_its_own_sockets() {
    let mut stack = configured();
    let mut wire = Wire::new();
    learn_gateway(&mut stack, &mut wire, T0);
    let first = stack.open(KIND_UDP, PEER_IP, 53, 0, 5).unwrap();
    let second = stack.open(KIND_UDP, PEER_IP, 53, 0, 5).unwrap();
    let other = stack.open(KIND_UDP, PEER_IP, 53, 0, 6).unwrap();

    let mut buf = [0u8; 4];
    wire.drive(|tx| stack.close_owner(5, T0, tx));
    assert_eq!(stack.recv(first, 5, &mut buf), Err(Status::NotFound));
    assert_eq!(stack.recv(second, 5, &mut buf), Err(Status::NotFound));
    assert_eq!(stack.recv(other, 6, &mut buf), Err(Status::WouldBlock));
}

#[test]
fn a_refused_device_frame_reports_would_block() {
    let mut stack = configured();
    let mut wire = Wire::new();
    learn_gateway(&mut stack, &mut wire, T0);
    let handle = stack.open(KIND_UDP, PEER_IP, 53, 0, 1).unwrap();

    wire.accept = false;
    assert_eq!(
        wire.send(&mut stack, 1, handle, b"x", T0),
        Err(Status::WouldBlock)
    );
    wire.accept = true;
    assert_eq!(wire.send(&mut stack, 1, handle, b"x", T0), Ok(1));
}

#[test]
fn statistics_move_with_traffic() {
    let mut stack = configured();
    let mut wire = Wire::new();
    learn_gateway(&mut stack, &mut wire, T0);
    let handle = stack.open(KIND_UDP, PEER_IP, 53, 0, 1).unwrap();
    let before = stack.stats();

    wire.clear();
    wire.send(&mut stack, 1, handle, b"x", T0).unwrap();
    assert_eq!(stack.stats().tx_frames, before.tx_frames + 1);

    let local = parse_udp(&parse_ip(&wire.last().to_vec())).src_port;
    let packet = udp_packet(PEER_IP, OUR_IP, 53, local, b"y");
    wire.receive(&mut stack, T0, &from_peer(&packet));
    assert_eq!(stack.stats().rx_frames, before.rx_frames + 1);
    let mut buf = [0u8; 4];
    assert_eq!(stack.recv(handle, 1, &mut buf), Ok(1));
    assert_eq!(&buf[..1], b"y");
}
