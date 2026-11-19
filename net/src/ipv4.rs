//! IPv4 header validation and construction (spec 0020, N6).

use crate::checksum;
use crate::ethernet;
use crate::{Stack, BROADCAST_IP, ETHERTYPE_IPV4, ETH_HEADER, IPV4_HEADER};

/// ICMP protocol number.
pub(crate) const PROTO_ICMP: u8 = 1;
/// TCP protocol number.
pub(crate) const PROTO_TCP: u8 = 6;
/// UDP protocol number.
pub(crate) const PROTO_UDP: u8 = 17;

/// Don't-fragment flag in the flags/fragment word.
const FLAG_DF: u16 = 0x4000;
/// Multicast-fragment and fragment-offset bits; any of them means fragmentation.
const FRAGMENT_MASK: u16 = 0x3FFF;
/// Time to live on every frame this stack emits.
const TTL: u8 = 64;

impl Stack {
    /// Validates one IPv4 packet and hands its payload to the next layer.
    pub(crate) fn ipv4_receive(
        &mut self,
        packet: &[u8],
        src_mac: &[u8; 6],
        now_ns: u64,
        tx: &mut dyn FnMut(&[u8]) -> bool,
    ) {
        if packet.len() < IPV4_HEADER {
            self.count_drop();
            return;
        }
        let version = packet[0] >> 4;
        let header_len = usize::from(packet[0] & 0x0F) * 4;
        if version != 4 || header_len < IPV4_HEADER || header_len > packet.len() {
            self.count_drop();
            return;
        }
        let total_len = usize::from(ethernet::be16(packet, 2));
        if total_len < header_len || total_len > packet.len() {
            self.count_drop();
            return;
        }
        if !checksum::verify(&packet[..header_len]) {
            self.count_error();
            return;
        }
        if ethernet::be16(packet, 6) & FRAGMENT_MASK != 0 {
            self.count_drop();
            return;
        }

        let dst = ethernet::be32(packet, 16);
        let for_us = dst == self.config.ip
            || dst == BROADCAST_IP
            || (self.config.mask != 0 && dst == self.broadcast_ip());
        if !for_us {
            self.count_drop();
            return;
        }

        let src = ethernet::be32(packet, 12);
        if src != 0 {
            // Any IPv4 frame refreshes the sender's cache entry (spec 0020, N5).
            self.arp_learn(src, *src_mac, now_ns);
        }

        let payload = &packet[header_len..total_len];
        match packet[9] {
            PROTO_ICMP => self.icmp_receive(src, payload, now_ns, tx),
            PROTO_UDP => self.udp_receive(src, dst, payload, now_ns, tx),
            PROTO_TCP => self.tcp_receive(src, dst, payload, now_ns, tx),
            _ => self.count_drop(),
        }
    }

    /// Writes the Ethernet and IPv4 headers for a transport payload and
    /// returns the offset where that payload begins.
    pub(crate) fn write_ipv4_header(
        &mut self,
        frame: &mut [u8],
        dst_mac: [u8; 6],
        dst_ip: u32,
        protocol: u8,
        payload_len: usize,
    ) -> usize {
        ethernet::write_header(frame, dst_mac, self.mac, ETHERTYPE_IPV4);
        let total_len = IPV4_HEADER + payload_len;
        self.ip_id = self.ip_id.wrapping_add(1);
        let header = &mut frame[ETH_HEADER..ETH_HEADER + IPV4_HEADER];
        header[0] = 0x45;
        header[1] = 0;
        header[2..4].copy_from_slice(&(total_len as u16).to_be_bytes());
        header[4..6].copy_from_slice(&self.ip_id.to_be_bytes());
        header[6..8].copy_from_slice(&FLAG_DF.to_be_bytes());
        header[8] = TTL;
        header[9] = protocol;
        header[10] = 0;
        header[11] = 0;
        header[12..16].copy_from_slice(&self.config.ip.to_be_bytes());
        header[16..20].copy_from_slice(&dst_ip.to_be_bytes());
        let sum = checksum::checksum(header);
        header[10..12].copy_from_slice(&sum.to_be_bytes());
        ETH_HEADER + IPV4_HEADER
    }
}
