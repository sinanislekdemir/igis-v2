//! Host tests for the M10.1 `net` skeleton (spec 0020, ADR 0048).
//!
//! Only the machine-wide values and the stub contract are covered here; the
//! protocol behaviour arrives with M10.2–M10.4 and gets its own tests then.

use abi::Status;
use net::{Config, Stack, Stats, KIND_TCP, MAX_SOCKETS, MSS, RX_CAP, TX_CAP};

#[test]
fn constants_match_the_static_budget() {
    assert_eq!(MAX_SOCKETS, 8);
    assert_eq!(MSS, 1460);
    assert_eq!(RX_CAP, 4096);
    assert_eq!(TX_CAP, 4096);
    assert_eq!(KIND_TCP, 1);
}

#[test]
fn a_new_stack_reports_no_device_and_zero_config() {
    let stack = Stack::new();
    assert_eq!(stack.mac(), [0; 6]);
    assert_eq!(stack.config(), Config::default());
    assert_eq!(stack.stats(), Stats::default());
}

#[test]
fn the_mac_round_trips() {
    let mut stack = Stack::new();
    let mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
    stack.set_mac(mac);
    assert_eq!(stack.mac(), mac);
}

#[test]
fn the_configuration_round_trips() {
    let mut stack = Stack::new();
    let config = Config {
        ip: 0x0A00_020F,
        mask: 0xFFFF_FF00,
        gateway: 0x0A00_0202,
        dns: 0x0A00_0203,
    };
    stack.set_config(config);
    assert_eq!(stack.config(), config);
}

#[test]
fn the_protocol_methods_are_live_in_this_slice() {
    let mut stack = Stack::new();
    stack.set_mac([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
    stack.set_config(Config {
        ip: 0x0A00_020F,
        mask: 0xFFFF_FF00,
        gateway: 0x0A00_0202,
        dns: 0x0A00_0203,
    });
    let mut tx = |_: &[u8]| true;
    let mut dst = [0u8; 16];

    // A TCP open is immediate and returns a generation handle in SYN-SENT.
    let handle = stack.open(KIND_TCP, 0x0A00_0202, 80, 0, 1).expect("open");
    assert_ne!(handle, 0);
    assert_eq!(handle >> 8, 1);
    assert!(!stack.readable(handle, 1));
    assert_eq!(
        stack.send(handle, 1, b"hello", 0, &mut tx),
        Err(Status::WouldBlock)
    );
    assert_eq!(stack.recv(handle, 1, &mut dst), Err(Status::WouldBlock));

    // A malformed frame is dropped and counted, never fatal.
    let before = stack.stats();
    stack.receive(&[0u8; 8], 0, &mut tx);
    assert_eq!(stack.stats().drops, before.drops + 1);

    // A foreign owner is Invalid; the owner may close; the handle is stale.
    assert_eq!(stack.close(handle, 2, 0, &mut tx), Status::Invalid);
    assert_eq!(stack.close(handle, 1, 0, &mut tx), Status::Ok);
    assert_eq!(stack.recv(handle, 1, &mut dst), Err(Status::NotFound));

    stack.close_owner(1, 0, &mut tx);
    stack.service(0, &mut tx);
}

#[test]
fn default_is_the_const_constructor() {
    let stack = Stack::default();
    assert_eq!(stack.mac(), [0; 6]);
    assert_eq!(stack.config(), Config::default());
}
