//! One's-complement checksums (spec 0020, N11).
//!
//! Every checksum in the stack is the same 16-bit one's-complement sum of
//! big-endian words, taken over the header or over a pseudo-header plus the
//! transport segment. The summer is incremental so a pseudo-header and a
//! payload can be fed as separate slices without a scratch copy; an odd-length
//! slice carries its last byte into the next one.

/// Incremental one's-complement summer.
#[derive(Clone, Copy)]
pub(crate) struct Checksum {
    sum: u32,
    pending: Option<u8>,
}

impl Checksum {
    /// Starts an empty sum.
    pub(crate) const fn new() -> Self {
        Self {
            sum: 0,
            pending: None,
        }
    }

    /// Adds one slice of bytes, remembering an odd trailing byte across calls.
    pub(crate) fn add(&mut self, bytes: &[u8]) {
        let mut index = 0;
        if let Some(high) = self.pending.take() {
            match bytes.first() {
                Some(&low) => {
                    self.sum += u32::from(u16::from_be_bytes([high, low]));
                    index = 1;
                }
                None => {
                    self.pending = Some(high);
                    return;
                }
            }
        }
        while index + 2 <= bytes.len() {
            self.sum += u32::from(u16::from_be_bytes([bytes[index], bytes[index + 1]]));
            index += 2;
        }
        if index < bytes.len() {
            self.pending = Some(bytes[index]);
        }
    }

    /// Folds the carries and any pending byte into a 16-bit sum.
    pub(crate) fn fold(self) -> u16 {
        let mut sum = self.sum;
        if let Some(high) = self.pending {
            // An odd trailing byte is the high half of a zero-padded word
            // (RFC 1071: pad on the right).
            sum += u32::from(u16::from_be_bytes([high, 0]));
        }
        while sum >> 16 != 0 {
            sum = (sum & 0xFFFF) + (sum >> 16);
        }
        sum as u16
    }

    /// The value to store in a zeroed checksum field.
    ///
    /// The complement of `0xFFFF` is `0x0000`, which is the required fold on
    /// transmit (spec 0020, N11).
    pub(crate) fn finish(self) -> u16 {
        !self.fold()
    }

    /// True when the data already contains a valid checksum field.
    pub(crate) fn verify(self) -> bool {
        self.fold() == 0xFFFF
    }
}

/// Computes the checksum of `bytes` with its checksum field zeroed.
pub(crate) fn checksum(bytes: &[u8]) -> u16 {
    let mut summer = Checksum::new();
    summer.add(bytes);
    summer.finish()
}

/// Verifies a header or segment that already carries its checksum.
pub(crate) fn verify(bytes: &[u8]) -> bool {
    let mut summer = Checksum::new();
    summer.add(bytes);
    summer.verify()
}

/// Seeds a checksum with the IPv4 pseudo-header for a transport segment.
pub(crate) fn pseudo(src: u32, dst: u32, proto: u8, len: u16) -> Checksum {
    let mut summer = Checksum::new();
    summer.add(&src.to_be_bytes());
    summer.add(&dst.to_be_bytes());
    summer.add(&[0, proto]);
    summer.add(&len.to_be_bytes());
    summer
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real IPv4 header with `b8 61` in its checksum field (the classic
    /// worked example): the complement of the total sum is zero.
    const IPV4_HEADER: [u8; 20] = [
        0x45, 0x00, 0x00, 0x73, 0x00, 0x00, 0x40, 0x00, 0x40, 0x11, 0xb8, 0x61, 0xc0, 0xa8, 0x00,
        0x01, 0xc0, 0xa8, 0x00, 0xc7,
    ];

    #[test]
    fn a_known_ipv4_header_verifies() {
        assert!(verify(&IPV4_HEADER));
        let mut zeroed = IPV4_HEADER;
        zeroed[10] = 0;
        zeroed[11] = 0;
        assert_eq!(checksum(&zeroed), 0xB861);
    }

    #[test]
    fn a_known_icmp_echo_computes() {
        let mut echo = [0u8; 12];
        echo[0] = 8;
        echo[4] = 0;
        echo[5] = 1;
        echo[6] = 0;
        echo[7] = 1;
        echo[8..12].copy_from_slice(b"abcd");
        let sum = checksum(&echo);
        assert_eq!(sum, 0x3337);
        echo[2..4].copy_from_slice(&sum.to_be_bytes());
        assert!(verify(&echo));
    }

    #[test]
    fn a_known_udp_pseudo_header_computes() {
        let payload = b"hello";
        let mut datagram = [0u8; 13];
        datagram[0..2].copy_from_slice(&0xC350u16.to_be_bytes());
        datagram[2..4].copy_from_slice(&6666u16.to_be_bytes());
        datagram[4..6].copy_from_slice(&13u16.to_be_bytes());
        datagram[8..13].copy_from_slice(payload);
        let mut summer = pseudo(0x0A00_020F, 0x0A00_0202, 17, 13);
        summer.add(&datagram);
        let sum = summer.finish();
        assert_eq!(sum, 0xC696);
        datagram[6..8].copy_from_slice(&sum.to_be_bytes());
        let mut check = pseudo(0x0A00_020F, 0x0A00_0202, 17, 13);
        check.add(&datagram);
        assert!(check.verify());
    }

    #[test]
    fn a_known_tcp_pseudo_header_computes() {
        let payload = b"hello";
        let mut segment = [0u8; 25];
        segment[0..2].copy_from_slice(&0xC350u16.to_be_bytes());
        segment[2..4].copy_from_slice(&6666u16.to_be_bytes());
        segment[4..8].copy_from_slice(&0x1000_0000u32.to_be_bytes());
        segment[8..12].copy_from_slice(&0x1000_0001u32.to_be_bytes());
        segment[12] = 0x50;
        segment[13] = 0x18;
        segment[14..16].copy_from_slice(&4096u16.to_be_bytes());
        segment[20..25].copy_from_slice(payload);
        let mut summer = pseudo(0x0A00_020F, 0x0A00_0202, 6, 25);
        summer.add(&segment);
        let sum = summer.finish();
        assert_eq!(sum, 0x4689);
        segment[16..18].copy_from_slice(&sum.to_be_bytes());
        let mut check = pseudo(0x0A00_020F, 0x0A00_0202, 6, 25);
        check.add(&segment);
        assert!(check.verify());
    }

    #[test]
    fn the_complement_folds_ffff_to_zero() {
        // A plain sum of 0xFFFF complements to 0x0000.
        assert_eq!(checksum(&[0xFF, 0xFF]), 0x0000);
        // A plain sum of zero complements to 0xFFFF.
        assert_eq!(checksum(&[0x00, 0x00]), 0xFFFF);
        // Carries fold back in: 0xFFFF + 0x0002 = 0x10001 -> 0x0002.
        assert_eq!(checksum(&[0xFF, 0xFF, 0x00, 0x02]), 0xFFFD);
        // An odd trailing byte is padded on the right: 0x1234 + 0x5600.
        assert_eq!(checksum(&[0x12, 0x34, 0x56]), 0x97CB);
    }

    #[test]
    fn an_odd_length_byte_carries_across_slices() {
        let mut split = Checksum::new();
        split.add(&[0x12]);
        split.add(&[0x34, 0x56, 0x78]);
        let mut whole = Checksum::new();
        whole.add(&[0x12, 0x34, 0x56, 0x78]);
        assert_eq!(split.fold(), whole.fold());
    }
}
