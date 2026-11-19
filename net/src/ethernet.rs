//! Ethernet II framing and the shared wire parse helpers (spec 0020, N5).

/// Length of an Ethernet II header (destination, source, ethertype).
pub(crate) const HEADER_LEN: usize = 14;
/// Smallest frame put on the wire, excluding the FCS.
pub(crate) const MIN_FRAME_LEN: usize = 60;

/// Destination address of the limited broadcast.
pub(crate) const BROADCAST_MAC: [u8; 6] = [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];

/// Reads a big-endian `u16` at `offset`; the caller has already bounded it.
pub(crate) fn be16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes([bytes[offset], bytes[offset + 1]])
}

/// Reads a big-endian `u32` at `offset`; the caller has already bounded it.
pub(crate) fn be32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

/// Copies six bytes out as a MAC address; the caller has bounded `offset`.
pub(crate) fn mac_at(bytes: &[u8], offset: usize) -> [u8; 6] {
    [
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
        bytes[offset + 4],
        bytes[offset + 5],
    ]
}

/// Writes the Ethernet II header at the start of `frame`.
pub(crate) fn write_header(frame: &mut [u8], dst: [u8; 6], src: [u8; 6], ethertype: u16) {
    frame[0..6].copy_from_slice(&dst);
    frame[6..12].copy_from_slice(&src);
    frame[12..14].copy_from_slice(&ethertype.to_be_bytes());
}

/// True when `mac` is the limited broadcast address.
pub(crate) fn is_broadcast(mac: &[u8; 6]) -> bool {
    *mac == BROADCAST_MAC
}
