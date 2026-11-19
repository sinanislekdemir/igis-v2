//! The network API's fixed limits and range rules (spec 0020).
//!
//! Nine host builtins are the whole application surface: three for the
//! machine-wide configuration (`net_config`, `net_info`, `net_mac`), one for
//! the protocol counters (`net_stats`) and five for sockets (`socket_open`,
//! `socket_send`, `socket_recv`, `socket_close`, `socket_accept`).
//! The wire stack itself lives in the kernel's `net` crate; this module holds
//! only the constants and the operand rules the compiler and the interpreter
//! must agree on, so a bad kind, port or address means the same thing
//! everywhere.
//!
//! Addresses are `uint` in host order with the first octet in bits 24..31
//! (`10.0.2.15` is `0x0A00_020F`); ports are `uint` in `0..=`[`MAX_PORT`]. A
//! socket is an opaque owner-checked `int` handle where `0` is never valid.

use crate::Status;

/// `core.socket_open` kind for a TCP client connection (spec 0020, N12). `open`
/// returns immediately with the handshake in progress; the first send may
/// report `WouldBlock` until it completes.
pub const KIND_TCP: u64 = 1;

/// `core.socket_open` kind for a UDP socket: one datagram per send, no
/// connection state.
pub const KIND_UDP: u64 = 2;

/// `core.socket_open` kind for a passive-open TCP listener (spec 0020, N10).
/// `local` is the bind port and must be nonzero; `ip`/`port` are ignored.
pub const KIND_LISTEN: u64 = 3;

/// The largest port number, inclusive. `local` port `0` means "ephemeral".
pub const MAX_PORT: u64 = 65535;

/// The largest IPv4 address as an operand: any `u32` bit pattern is a valid
/// address, so only the 64-bit range is checked.
pub const MAX_ADDRESS: u64 = u32::MAX as u64;

/// The most bytes one `core.socket_recv` call copies, and the size of the
/// interpreter's staging chunk. A smaller destination is filled up to its own
/// capacity; the caller reads more with another call.
pub const RECV_CHUNK: usize = 2048;

/// The largest accepted `core.socket_timeout` value in milliseconds (one hour).
/// A larger operand is clamped rather than rejected, so an arithmetic slip in
/// an application cannot wrap a deadline back into the past.
pub const MAX_TIMEOUT_MS: u64 = 3_600_000;

/// Validates the operands of `core.socket_open` (spec 0020, N12): `kind` must
/// be [`KIND_TCP`], [`KIND_UDP`] or [`KIND_LISTEN`], `port` and `local` must be
/// at most [`MAX_PORT`], and a listener must bind a nonzero `local` port.
///
/// A rejected operand is [`Status::Invalid`]; the caller pushes it without a
/// host call, so a malformed open never touches the socket table.
pub fn validate_open(kind: u64, port: u64, local: u64) -> Status {
    if kind != KIND_TCP && kind != KIND_UDP && kind != KIND_LISTEN {
        return Status::Invalid;
    }
    if port > MAX_PORT || local > MAX_PORT {
        return Status::Invalid;
    }
    if kind == KIND_LISTEN && local == 0 {
        return Status::Invalid;
    }
    Status::Ok
}

/// Validates one host-order IPv4 address operand: at most [`MAX_ADDRESS`],
/// which is every `u32`. A wider value is [`Status::Invalid`] and is rejected
/// before the host call.
pub fn validate_address(address: u64) -> Status {
    if address > MAX_ADDRESS {
        Status::Invalid
    } else {
        Status::Ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_are_the_spec_constants() {
        assert_eq!((KIND_TCP, KIND_UDP, KIND_LISTEN), (1, 2, 3));
        assert_eq!(MAX_PORT, 65535);
        assert_eq!(MAX_ADDRESS, 0xFFFF_FFFF);
        assert_eq!(RECV_CHUNK, 2048);
        assert_eq!(MAX_TIMEOUT_MS, 3_600_000);
    }

    #[test]
    fn an_open_needs_a_client_kind_and_a_port_range() {
        assert_eq!(validate_open(KIND_TCP, 6666, 0), Status::Ok);
        assert_eq!(validate_open(KIND_UDP, 0, 5353), Status::Ok);
        assert_eq!(validate_open(KIND_TCP, MAX_PORT, MAX_PORT), Status::Ok);
        assert_eq!(validate_open(KIND_LISTEN, 0, 7), Status::Ok);
        assert_eq!(validate_open(KIND_LISTEN, MAX_PORT, MAX_PORT), Status::Ok);
        // A listener must bind a local port; other kinds cannot use kind 3's
        // rules, and an unknown kind stays invalid.
        assert_eq!(validate_open(KIND_LISTEN, 0, 0), Status::Invalid);
        assert_eq!(validate_open(KIND_LISTEN, 7, 0), Status::Invalid);
        for kind in [0, 4, 99] {
            assert_eq!(validate_open(kind, 80, 0), Status::Invalid, "kind {kind}");
        }
        for (port, local) in [(MAX_PORT + 1, 0), (0, MAX_PORT + 1), (0x1_0000, 0x1_0000)] {
            assert_eq!(
                validate_open(KIND_TCP, port, local),
                Status::Invalid,
                "port {port} local {local}"
            );
        }
    }

    #[test]
    fn an_address_is_any_u32_bit_pattern() {
        for address in [0, 1, 0x0A00_020F, MAX_ADDRESS] {
            assert_eq!(validate_address(address), Status::Ok, "{address:#x}");
        }
        assert_eq!(validate_address(MAX_ADDRESS + 1), Status::Invalid);
        assert_eq!(validate_address(u64::MAX), Status::Invalid);
    }
}
