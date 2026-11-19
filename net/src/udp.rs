//! Connected UDP sockets and datagram demultiplexing (spec 0020, N8).

use abi::Status;

use crate::checksum;
use crate::ethernet;
use crate::ipv4::PROTO_UDP;
use crate::{
    Stack, BROADCAST_IP, ETH_HEADER, FRAME_MAX, IPV4_HEADER, KIND_UDP, MAX_SOCKETS, RX_CAP,
};

/// Largest UDP payload that fits one 1500-byte Ethernet frame.
pub(crate) const MAX_PAYLOAD: usize = 1472;
const UDP_HEADER: usize = 8;

impl Stack {
    /// Sends one datagram to the socket's connected peer.
    pub(crate) fn udp_send(
        &mut self,
        index: usize,
        data: &[u8],
        now_ns: u64,
        tx: &mut dyn FnMut(&[u8]) -> bool,
    ) -> Result<usize, Status> {
        if data.len() > MAX_PAYLOAD {
            return Err(Status::CapacityExceeded);
        }
        let (dst_ip, src_port, dst_port) = {
            let socket = &self.sockets[index];
            (socket.remote_ip, socket.local_port, socket.remote_port)
        };
        let Some(dst_mac) = self.resolve_dst(dst_ip, now_ns, tx) else {
            return Err(Status::WouldBlock);
        };

        let datagram_len = UDP_HEADER + data.len();
        let mut frame = [0u8; FRAME_MAX];
        let offset = self.write_ipv4_header(&mut frame, dst_mac, dst_ip, PROTO_UDP, datagram_len);
        {
            let header = &mut frame[offset..offset + UDP_HEADER];
            header[0..2].copy_from_slice(&src_port.to_be_bytes());
            header[2..4].copy_from_slice(&dst_port.to_be_bytes());
            header[4..6].copy_from_slice(&(datagram_len as u16).to_be_bytes());
            header[6] = 0;
            header[7] = 0;
        }
        frame[offset + UDP_HEADER..offset + datagram_len].copy_from_slice(data);

        let mut summer = checksum::pseudo(self.config.ip, dst_ip, PROTO_UDP, datagram_len as u16);
        summer.add(&frame[offset..offset + datagram_len]);
        let mut sum = summer.finish();
        if sum == 0 {
            // A transmitted zero means "no checksum"; 0xFFFF means the same sum.
            sum = 0xFFFF;
        }
        frame[offset + 6..offset + 8].copy_from_slice(&sum.to_be_bytes());

        match self.emit(&mut frame, ETH_HEADER + IPV4_HEADER + datagram_len, tx) {
            Status::Ok => Ok(data.len()),
            status => Err(status),
        }
    }

    /// Copies one received datagram into the owning socket's arena.
    pub(crate) fn udp_receive(
        &mut self,
        src_ip: u32,
        dst_ip: u32,
        segment: &[u8],
        _now_ns: u64,
        _tx: &mut dyn FnMut(&[u8]) -> bool,
    ) {
        if segment.len() < UDP_HEADER {
            self.count_drop();
            return;
        }
        let src_port = ethernet::be16(segment, 0);
        let dst_port = ethernet::be16(segment, 2);
        let datagram_len = usize::from(ethernet::be16(segment, 4));
        if datagram_len < UDP_HEADER || datagram_len > segment.len() {
            self.count_drop();
            return;
        }
        let supplied = ethernet::be16(segment, 6);
        if supplied != 0 {
            let mut summer = checksum::pseudo(src_ip, dst_ip, PROTO_UDP, datagram_len as u16);
            summer.add(&segment[..datagram_len]);
            if !summer.verify() {
                self.count_error();
                return;
            }
        }

        let payload = &segment[UDP_HEADER..datagram_len];
        if payload.len() > RX_CAP {
            self.count_drop();
            return;
        }

        let broadcast = self.broadcast_ip();
        let subnet_broadcast = self.config.mask != 0;
        for index in 0..MAX_SOCKETS {
            let socket = &self.sockets[index];
            if !socket.active || socket.kind != KIND_UDP || socket.local_port != dst_port {
                continue;
            }
            let from_any = socket.remote_ip == BROADCAST_IP
                || (subnet_broadcast && socket.remote_ip == broadcast);
            if !from_any && (socket.remote_ip != src_ip || socket.remote_port != src_port) {
                continue;
            }
            if socket.udp_ready {
                // One datagram at a time: a second one before `recv` is dropped.
                self.count_drop();
                return;
            }
            let socket = &mut self.sockets[index];
            socket.rx[..payload.len()].copy_from_slice(payload);
            socket.rx_len = payload.len();
            socket.udp_len = payload.len();
            socket.udp_ready = true;
            return;
        }
        // No socket names this port: the frame is dropped and counted.
        self.count_drop();
    }
}
