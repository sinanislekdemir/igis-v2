//! ICMP echo request/reply (spec 0020, N7).

use crate::checksum;
use crate::ipv4::PROTO_ICMP;
use crate::{Stack, ETH_HEADER, FRAME_MAX, IPV4_HEADER};

const TYPE_ECHO_REPLY: u8 = 0;
const TYPE_ECHO_REQUEST: u8 = 8;

impl Stack {
    /// Answers an ICMP echo request and drops every other ICMP message.
    pub(crate) fn icmp_receive(
        &mut self,
        src_ip: u32,
        message: &[u8],
        now_ns: u64,
        tx: &mut dyn FnMut(&[u8]) -> bool,
    ) {
        if message.len() < 8 {
            self.count_drop();
            return;
        }
        if !checksum::verify(message) {
            self.count_error();
            return;
        }
        if message[0] != TYPE_ECHO_REQUEST || message[1] != 0 {
            self.count_drop();
            return;
        }
        if ETH_HEADER + IPV4_HEADER + message.len() > FRAME_MAX {
            self.count_drop();
            return;
        }
        let Some(dst_mac) = self.resolve_dst(src_ip, now_ns, tx) else {
            self.count_drop();
            return;
        };

        let mut frame = [0u8; FRAME_MAX];
        let offset = self.write_ipv4_header(&mut frame, dst_mac, src_ip, PROTO_ICMP, message.len());
        frame[offset..offset + message.len()].copy_from_slice(message);
        // Echo reply, same identifier/sequence/payload, fresh checksum.
        frame[offset] = TYPE_ECHO_REPLY;
        frame[offset + 1] = 0;
        frame[offset + 2] = 0;
        frame[offset + 3] = 0;
        let sum = checksum::checksum(&frame[offset..offset + message.len()]);
        frame[offset + 2..offset + 4].copy_from_slice(&sum.to_be_bytes());
        let _ = self.emit(&mut frame, ETH_HEADER + IPV4_HEADER + message.len(), tx);
    }
}
