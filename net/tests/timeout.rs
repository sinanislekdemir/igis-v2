//! Host tests for the per-socket deadline and readiness wake path
//! (spec 0020, N13; ADR 0050).
//!
//! `arm_wait` records the direction of a blocking call that returned
//! `WouldBlock`; `service` marks an expired deadline, and `ready_handles` /
//! `wait_ready` tell the kernel which waiter to wake. None of this touches the
//! protocol state machine, so the tests use a minimal simulated link.

use abi::Status;
use net::{Config, Stack, DEFAULT_OP_TIMEOUT_NS, KIND_TCP, KIND_UDP, MAX_SOCKETS};

const OUR_MAC: [u8; 6] = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
const PEER_MAC: [u8; 6] = [0x52, 0x55, 0x0A, 0x00, 0x02, 0x02];
const OUR_IP: u32 = 0x0A00_020F;
const MASK: u32 = 0xFFFF_FF00;
const PEER_IP: u32 = 0x0A00_0202;
const T0: u64 = 1_000_000_000;
const MS: u64 = 1_000_000;

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

fn eth(dst: [u8; 6], src: [u8; 6], ethertype: u16, payload: &[u8]) -> Vec<u8> {
    let mut frame = vec![0u8; 14];
    frame[0..6].copy_from_slice(&dst);
    frame[6..12].copy_from_slice(&src);
    frame[12..14].copy_from_slice(&ethertype.to_be_bytes());
    frame.extend_from_slice(payload);
    frame
}

fn arp_reply() -> Vec<u8> {
    let mut arp = vec![0u8; 28];
    arp[0..2].copy_from_slice(&1u16.to_be_bytes());
    arp[2..4].copy_from_slice(&0x0800u16.to_be_bytes());
    arp[4] = 6;
    arp[5] = 4;
    arp[6..8].copy_from_slice(&2u16.to_be_bytes());
    arp[8..14].copy_from_slice(&PEER_MAC);
    arp[14..18].copy_from_slice(&PEER_IP.to_be_bytes());
    arp[18..24].copy_from_slice(&OUR_MAC);
    arp[24..28].copy_from_slice(&OUR_IP.to_be_bytes());
    eth(OUR_MAC, PEER_MAC, 0x0806, &arp)
}

#[test]
fn a_configured_deadline_expires_once_and_names_the_handle() {
    let mut stack = configured();
    let mut tx = |_: &[u8]| true;
    let handle = stack
        .open(KIND_UDP, PEER_IP, 53, 0, 1)
        .expect("a UDP socket opens");
    let mut dst = [0u8; 16];

    assert_eq!(stack.set_timeout(handle, 1, 500, T0), Status::Ok);
    assert_eq!(stack.recv(handle, 1, &mut dst), Err(Status::WouldBlock));
    assert_eq!(stack.arm_wait(handle, 1, T0, false), Status::Ok);

    // Before the deadline: not ready, no expiry, no event.
    let before = stack.event_seq();
    stack.service(T0 + 499 * MS, &mut tx);
    assert_eq!(stack.event_seq(), before, "no event before the deadline");
    assert!(!stack.wait_ready(handle, 1, T0 + 499 * MS));
    assert!(!stack.take_timed_out(handle, 1));

    // At the deadline the stack marks the socket and bumps the event counter.
    stack.service(T0 + 500 * MS, &mut tx);
    assert!(stack.event_seq() > before, "expiry is a readiness event");
    assert!(stack.readable(handle, 1), "expiry reads as ready");
    assert!(stack.wait_ready(handle, 1, T0 + 500 * MS));

    let mut out = [0u64; MAX_SOCKETS];
    let count = stack.ready_handles(T0 + 500 * MS, &mut out);
    assert_eq!(count, 1);
    assert_eq!(out[0], handle);

    // The interpreter consumes the expiry exactly once.
    assert!(stack.take_timed_out(handle, 1));
    assert!(!stack.take_timed_out(handle, 1));
}

#[test]
fn zero_timeout_blocks_indefinitely() {
    let mut stack = configured();
    let mut tx = |_: &[u8]| true;
    let handle = stack.open(KIND_UDP, PEER_IP, 53, 0, 1).expect("open");
    let mut dst = [0u8; 8];

    assert_eq!(stack.set_timeout(handle, 1, 0, T0), Status::Ok);
    assert_eq!(stack.recv(handle, 1, &mut dst), Err(Status::WouldBlock));
    assert_eq!(stack.arm_wait(handle, 1, T0, false), Status::Ok);

    stack.service(T0 + 60_000 * MS, &mut tx);
    assert!(!stack.take_timed_out(handle, 1), "0 means no deadline");
    assert!(!stack.wait_ready(handle, 1, T0 + 60_000 * MS));
}

#[test]
fn the_default_deadline_is_thirty_seconds() {
    let mut stack = configured();
    let mut tx = |_: &[u8]| true;
    let handle = stack.open(KIND_UDP, PEER_IP, 53, 0, 1).expect("open");
    let mut dst = [0u8; 8];

    assert_eq!(DEFAULT_OP_TIMEOUT_NS, 30_000 * MS);
    assert_eq!(stack.recv(handle, 1, &mut dst), Err(Status::WouldBlock));
    assert_eq!(stack.arm_wait(handle, 1, T0, false), Status::Ok);

    stack.service(T0 + 29_999 * MS, &mut tx);
    assert!(!stack.take_timed_out(handle, 1));
    assert_eq!(stack.recv(handle, 1, &mut dst), Err(Status::WouldBlock));
    assert_eq!(stack.arm_wait(handle, 1, T0, false), Status::Ok);
    stack.service(T0 + 30_000 * MS, &mut tx);
    assert!(stack.take_timed_out(handle, 1));
}

#[test]
fn a_send_wait_is_readied_by_the_next_hop_resolution() {
    let mut stack = configured();
    let mut tx = |_: &[u8]| true;
    let handle = stack.open(KIND_UDP, PEER_IP, 80, 0, 1).expect("open");

    // A fresh UDP socket has no ARP entry for the gateway, so a send blocks.
    assert_eq!(stack.send(handle, 1, b"x", T0, &mut tx), Err(Status::WouldBlock));
    assert_eq!(stack.arm_wait(handle, 1, T0, true), Status::Ok);
    assert!(!stack.wait_ready(handle, 1, T0));

    // The ARP reply is a readiness event and makes the send possible.
    let before = stack.event_seq();
    stack.receive(&arp_reply(), T0 + MS, &mut tx);
    assert!(stack.event_seq() > before);
    assert!(stack.wait_ready(handle, 1, T0 + MS));

    let mut out = [0u64; MAX_SOCKETS];
    assert_eq!(stack.ready_handles(T0 + MS, &mut out), 1);
    assert_eq!(out[0], handle);
    assert_eq!(stack.send(handle, 1, b"x", T0 + MS, &mut tx), Ok(1));
}

#[test]
fn a_tcp_connect_wait_is_readied_when_the_handshake_fails() {
    let mut stack = configured();
    let mut tx = |_: &[u8]| true;
    let handle = stack.open(KIND_TCP, PEER_IP, 80, 0, 1).expect("open");

    assert_eq!(stack.send(handle, 1, b"x", T0, &mut tx), Err(Status::WouldBlock));
    assert_eq!(stack.arm_wait(handle, 1, T0, true), Status::Ok);
    assert!(!stack.wait_ready(handle, 1, T0));

    // The first service starts the SYN and arms the 5 s connect deadline; the
    // second reaches it, which is a protocol failure, not the armed op
    // deadline: the socket errors and the event counter moves.
    stack.service(T0, &mut tx);
    let before = stack.event_seq();
    stack.service(T0 + 5_000 * MS, &mut tx);
    assert!(stack.event_seq() > before);
    assert!(stack.wait_ready(handle, 1, T0 + 5_000 * MS));
    assert_eq!(
        stack.send(handle, 1, b"x", T0 + 5_000 * MS, &mut tx),
        Err(Status::Closed)
    );
}

#[test]
fn set_timeout_checks_the_owner_and_the_handle() {
    let mut stack = configured();
    let handle = stack.open(KIND_UDP, PEER_IP, 53, 0, 1).expect("open");
    assert_eq!(stack.set_timeout(handle, 2, 500, T0), Status::Invalid);
    assert_eq!(stack.set_timeout(handle + 0x100, 1, 500, T0), Status::NotFound);
    assert_eq!(stack.set_timeout(0, 1, 500, T0), Status::NotFound);
}
