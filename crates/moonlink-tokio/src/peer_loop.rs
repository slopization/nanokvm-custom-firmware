use std::sync::Arc;

use moonlink_core::ServerError;
use tokio::net::UdpSocket;
use tokio::sync::watch;

use crate::shared::{StreamPeer, StreamPeerPolicy};

pub(crate) async fn learn_stream_peer(
    socket: Arc<UdpSocket>,
    stream_peer: StreamPeer,
    label: &'static str,
    policy: StreamPeerPolicy,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), ServerError> {
    let mut buffer = [0_u8; 2048];
    loop {
        tokio::select! {
            result = socket.recv_from(&mut buffer) => {
                let (len, peer) = result?;
                if is_wake_on_lan_packet(&buffer[..len]) {
                    continue;
                }
                let mut stream_peer = stream_peer.lock().await;
                if stream_peer.learn_from_udp(peer, policy) {
                    eprintln!("learned {label} peer from UDP traffic: {peer}");
                }
            }
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    return Ok(());
                }
            }
        }
    }
}

fn is_wake_on_lan_packet(packet: &[u8]) -> bool {
    packet.len() >= 102 && packet[..6].iter().all(|byte| *byte == 0xff)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_wake_on_lan_magic_packet() {
        let packet = [0xff_u8; 102];
        assert!(is_wake_on_lan_packet(&packet));
        assert!(!is_wake_on_lan_packet(&packet[..101]));
        assert!(!is_wake_on_lan_packet(&[0, 0, 0, 0, 0, 0, 1, 2]));
    }
}
