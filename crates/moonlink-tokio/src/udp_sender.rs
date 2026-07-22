use std::net::SocketAddr;

use moonlink_core::ServerError;
use quinn_udp::{Transmit, UdpSockRef, UdpSocketState};
use tokio::net::UdpSocket;

const MAX_UDP_PAYLOAD_LEN: usize = 65_507;

pub(crate) struct AsyncUdpSender<'a> {
    socket: &'a UdpSocket,
    udp_state: Option<UdpSocketState>,
}

impl<'a> AsyncUdpSender<'a> {
    pub(crate) fn new(socket: &'a UdpSocket) -> Self {
        let udp_state = match UdpSocketState::new(UdpSockRef::from(socket)) {
            Ok(state) => {
                eprintln!("UDP GSO max segments: {}", state.max_gso_segments());
                Some(state)
            }
            Err(err) => {
                eprintln!("UDP GSO unavailable; using send_to fallback: {err}");
                None
            }
        };
        Self { socket, udp_state }
    }

    pub(crate) async fn send_packet(
        &self,
        packet: &[u8],
        peer: SocketAddr,
    ) -> Result<(), ServerError> {
        self.socket.send_to(packet, peer).await?;
        Ok(())
    }

    pub(crate) async fn send_packets(
        &self,
        peer: SocketAddr,
        packets: &[Vec<u8>],
    ) -> Result<(), ServerError> {
        let Some(state) = &self.udp_state else {
            return self.send_packets_fallback(peer, packets).await;
        };
        let max_segments = state.max_gso_segments();
        if max_segments <= 1 {
            return self.send_packets_fallback(peer, packets).await;
        }

        let mut index = 0;
        while index < packets.len() {
            let segment_size = packets[index].len();
            let max_batch_segments = max_batch_segments(segment_size, max_segments);
            if max_batch_segments <= 1 {
                self.send_packet(&packets[index], peer).await?;
                index += 1;
                continue;
            }
            let end = equal_sized_packet_run(packets, index, segment_size, max_batch_segments);
            if end == index + 1 {
                self.send_packet(&packets[index], peer).await?;
            } else {
                self.send_gso_batch(state, peer, segment_size, &packets[index..end])
                    .await?;
            }
            index = end;
        }
        Ok(())
    }

    async fn send_packets_fallback(
        &self,
        peer: SocketAddr,
        packets: &[Vec<u8>],
    ) -> Result<(), ServerError> {
        for packet in packets {
            self.send_packet(packet, peer).await?;
        }
        Ok(())
    }

    async fn send_gso_batch(
        &self,
        state: &UdpSocketState,
        peer: SocketAddr,
        segment_size: usize,
        packets: &[Vec<u8>],
    ) -> Result<(), ServerError> {
        let mut contents = Vec::with_capacity(segment_size * packets.len());
        for packet in packets {
            contents.extend_from_slice(packet);
        }
        let transmit = Transmit {
            destination: peer,
            ecn: None,
            contents: &contents,
            segment_size: Some(segment_size),
            src_ip: None,
        };
        loop {
            match state.try_send(UdpSockRef::from(self.socket), &transmit) {
                Ok(()) => return Ok(()),
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    self.socket.writable().await?;
                }
                Err(err) => return Err(ServerError::Io(err)),
            }
        }
    }
}

fn max_batch_segments(segment_size: usize, max_gso_segments: usize) -> usize {
    if segment_size == 0 {
        return 1;
    }
    max_gso_segments.min(MAX_UDP_PAYLOAD_LEN / segment_size)
}

fn equal_sized_packet_run(
    packets: &[Vec<u8>],
    start: usize,
    segment_size: usize,
    max_segments: usize,
) -> usize {
    let max_end = (start + max_segments).min(packets.len());
    let mut end = start + 1;
    while end < max_end && packets[end].len() == segment_size {
        end += 1;
    }
    end
}
