//! The ARP cache and request/reply exchange (spec 0020, N5).
//!
//! The cache is a fixed 16-entry table. An entry keeps its MAC, the time it was
//! last refreshed and the number of unanswered requests, so an unresolved next
//! hop is asked for at most three times, one second apart, and then left alone
//! until the caller gives up.

use crate::ethernet::{self, BROADCAST_MAC};
use crate::{
    Stack, ARP_ENTRIES, ARP_MAX_REQUESTS, ARP_RETRY_NS, ARP_TTL_NS, ETHERTYPE_ARP, ETHERTYPE_IPV4,
};

const ARP_LEN: usize = 28;
const HARDWARE_ETHERNET: u16 = 1;
const OPER_REQUEST: u16 = 1;
const OPER_REPLY: u16 = 2;

/// One ARP cache entry.
#[derive(Clone, Copy)]
pub(crate) struct ArpEntry {
    pub(crate) ip: u32,
    pub(crate) mac: [u8; 6],
    pub(crate) updated_ns: u64,
    pub(crate) valid: bool,
    pub(crate) requests: u8,
    pub(crate) last_request_ns: u64,
}

impl ArpEntry {
    pub(crate) const EMPTY: Self = Self {
        ip: 0,
        mac: [0; 6],
        updated_ns: 0,
        valid: false,
        requests: 0,
        last_request_ns: 0,
    };
}

impl Stack {
    /// Returns the cached MAC for `ip` when it is still fresh.
    pub(crate) fn arp_lookup(&self, ip: u32, now_ns: u64) -> Option<[u8; 6]> {
        for entry in &self.arp {
            if entry.valid
                && entry.ip == ip
                && now_ns.saturating_sub(entry.updated_ns) <= ARP_TTL_NS
            {
                return Some(entry.mac);
            }
        }
        None
    }

    /// Records a sender's address, inserting or evicting as needed.
    pub(crate) fn arp_learn(&mut self, ip: u32, mac: [u8; 6], now_ns: u64) {
        if ip == 0 {
            return;
        }
        let mut free = None;
        let mut oldest = 0usize;
        let mut oldest_ns = u64::MAX;
        for index in 0..ARP_ENTRIES {
            let entry = &self.arp[index];
            if entry.ip == ip {
                let entry = &mut self.arp[index];
                entry.ip = ip;
                entry.mac = mac;
                entry.updated_ns = now_ns;
                entry.valid = true;
                entry.requests = 0;
                entry.last_request_ns = 0;
                return;
            }
            if !entry.valid && free.is_none() {
                free = Some(index);
            }
            if entry.valid && entry.updated_ns < oldest_ns {
                oldest_ns = entry.updated_ns;
                oldest = index;
            }
        }
        let index = free.unwrap_or(oldest);
        self.arp[index] = ArpEntry {
            ip,
            mac,
            updated_ns: now_ns,
            valid: true,
            requests: 0,
            last_request_ns: 0,
        };
    }

    /// Sends one ARP request for `ip` if the retry budget allows it.
    pub(crate) fn arp_request(&mut self, ip: u32, now_ns: u64, tx: &mut dyn FnMut(&[u8]) -> bool) {
        let mut index = None;
        let mut free = None;
        let mut oldest = 0usize;
        let mut oldest_ns = u64::MAX;
        for candidate in 0..ARP_ENTRIES {
            let entry = &self.arp[candidate];
            if entry.ip == ip {
                index = Some(candidate);
                break;
            }
            if !entry.valid && free.is_none() {
                free = Some(candidate);
            }
            if entry.valid && entry.updated_ns < oldest_ns {
                oldest_ns = entry.updated_ns;
                oldest = candidate;
            }
        }
        let index = index.or(free).unwrap_or(oldest);

        let entry = &mut self.arp[index];
        if entry.ip != ip || entry.valid {
            // A new next hop, or a cached entry the caller already found
            // stale: start a fresh request cycle.
            entry.ip = ip;
            entry.valid = false;
            entry.requests = 0;
            entry.last_request_ns = 0;
        }
        if entry.requests >= ARP_MAX_REQUESTS {
            return;
        }
        if entry.requests > 0 && now_ns.saturating_sub(entry.last_request_ns) < ARP_RETRY_NS {
            return;
        }
        entry.requests = entry.requests.saturating_add(1);
        entry.last_request_ns = now_ns;

        let mut frame = [0u8; ethernet::MIN_FRAME_LEN];
        ethernet::write_header(&mut frame, BROADCAST_MAC, self.mac, ETHERTYPE_ARP);
        let arp = &mut frame[ethernet::HEADER_LEN..];
        arp[0..2].copy_from_slice(&HARDWARE_ETHERNET.to_be_bytes());
        arp[2..4].copy_from_slice(&ETHERTYPE_IPV4.to_be_bytes());
        arp[4] = 6;
        arp[5] = 4;
        arp[6..8].copy_from_slice(&OPER_REQUEST.to_be_bytes());
        arp[8..14].copy_from_slice(&self.mac);
        arp[14..18].copy_from_slice(&self.config.ip.to_be_bytes());
        arp[18..24].copy_from_slice(&[0; 6]);
        arp[24..28].copy_from_slice(&ip.to_be_bytes());
        let _ = self.emit(&mut frame, ethernet::HEADER_LEN + ARP_LEN, tx);
    }

    /// Expires entries that have not been refreshed for [`ARP_TTL_NS`].
    pub(crate) fn arp_age(&mut self, now_ns: u64) {
        for entry in self.arp.iter_mut() {
            if entry.valid && now_ns.saturating_sub(entry.updated_ns) > ARP_TTL_NS {
                entry.valid = false;
                entry.requests = 0;
            }
        }
    }

    /// Parses one ARP payload behind an Ethernet header, learning the sender
    /// and answering a request aimed at our address.
    pub(crate) fn arp_receive(
        &mut self,
        payload: &[u8],
        src_mac: &[u8; 6],
        now_ns: u64,
        tx: &mut dyn FnMut(&[u8]) -> bool,
    ) {
        if payload.len() < ARP_LEN {
            self.count_drop();
            return;
        }
        let hardware = ethernet::be16(payload, 0);
        let protocol = ethernet::be16(payload, 2);
        let hardware_len = payload[4];
        let protocol_len = payload[5];
        let operation = ethernet::be16(payload, 6);
        if hardware != HARDWARE_ETHERNET
            || protocol != ETHERTYPE_IPV4
            || hardware_len != 6
            || protocol_len != 4
        {
            self.count_drop();
            return;
        }

        let sender_mac = ethernet::mac_at(payload, 8);
        let sender_ip = ethernet::be32(payload, 14);
        let target_ip = ethernet::be32(payload, 24);

        // A zero sender address is a probe; there is nothing to learn.
        if sender_ip != 0 {
            self.arp_learn(sender_ip, sender_mac, now_ns);
        }

        match operation {
            OPER_REQUEST => {
                if target_ip != self.config.ip || self.config.ip == 0 {
                    return;
                }
                let mut frame = [0u8; ethernet::MIN_FRAME_LEN];
                ethernet::write_header(&mut frame, sender_mac, self.mac, ETHERTYPE_ARP);
                let arp = &mut frame[ethernet::HEADER_LEN..];
                arp[0..2].copy_from_slice(&HARDWARE_ETHERNET.to_be_bytes());
                arp[2..4].copy_from_slice(&ETHERTYPE_IPV4.to_be_bytes());
                arp[4] = 6;
                arp[5] = 4;
                arp[6..8].copy_from_slice(&OPER_REPLY.to_be_bytes());
                arp[8..14].copy_from_slice(&self.mac);
                arp[14..18].copy_from_slice(&self.config.ip.to_be_bytes());
                arp[18..24].copy_from_slice(&sender_mac);
                arp[24..28].copy_from_slice(&sender_ip.to_be_bytes());
                let _ = self.emit(&mut frame, ethernet::HEADER_LEN + ARP_LEN, tx);
            }
            OPER_REPLY => {
                // The sender was learned above; a reply for another target is
                // still useful only as a cache refresh, which already happened.
                let _ = src_mac;
            }
            _ => self.count_drop(),
        }
    }
}
