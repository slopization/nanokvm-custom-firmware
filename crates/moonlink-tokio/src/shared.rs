use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use openssl::cipher_ctx::CipherCtx;
use tokio::sync::Mutex;

use crate::pairing::PairingSession;

#[derive(Debug, Clone, Copy)]
pub(crate) struct StreamPorts {
    pub(crate) https: u16,
    pub(crate) rtsp: u16,
    pub(crate) video: u16,
    pub(crate) audio: u16,
    pub(crate) control: u16,
}

pub(crate) type PairingSessions = Arc<Mutex<HashMap<String, PairingSession>>>;
pub(crate) type StreamPeer = Arc<Mutex<StreamPeerTracker>>;

pub(crate) async fn clear_stream_peer(peer: &StreamPeer) {
    peer.lock().await.clear();
}
pub(crate) type ControlCrypto = Arc<Mutex<Option<ControlCryptoState>>>;

const MOONLIGHT_RTSP_CLIENT_PORT: u16 = 50000;

#[derive(Debug, Clone)]
pub(crate) struct RiKeyMaterial {
    pub(crate) key: [u8; 16],
    pub(crate) ri_key_id: u32,
}

impl RiKeyMaterial {
    pub(crate) fn new(key: [u8; 16], ri_key_id: u32) -> Self {
        Self { key, ri_key_id }
    }
}

pub(crate) struct ControlCryptoState {
    pub(crate) material: RiKeyMaterial,
    pub(crate) next_host_sequence: u32,
    pub(crate) mode: ControlEncryptionMode,
    pub(crate) encrypt_context: Option<CipherCtx>,
    pub(crate) encrypt_context_mode: Option<ControlEncryptionMode>,
    pub(crate) decrypt_context: Option<CipherCtx>,
    pub(crate) decrypt_context_mode: Option<ControlEncryptionMode>,
}

impl ControlCryptoState {
    pub(crate) fn new(material: RiKeyMaterial) -> Self {
        Self {
            material,
            next_host_sequence: 0,
            mode: ControlEncryptionMode::New,
            encrypt_context: None,
            encrypt_context_mode: None,
            decrypt_context: None,
            decrypt_context_mode: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ControlEncryptionMode {
    New,
    Old,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct StreamPeerPolicy {
    rtsp_client_port: u16,
}

impl StreamPeerPolicy {
    pub(crate) fn moonlight_nat() -> Self {
        Self {
            rtsp_client_port: MOONLIGHT_RTSP_CLIENT_PORT,
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct StreamPeerTracker {
    peer: Option<SocketAddr>,
}

impl StreamPeerTracker {
    pub(crate) fn peer(&self) -> Option<SocketAddr> {
        self.peer
    }

    pub(crate) fn set_from_rtsp(&mut self, peer: SocketAddr) -> bool {
        self.set_peer(peer)
    }

    pub(crate) fn clear(&mut self) {
        self.peer = None;
    }

    pub(crate) fn learn_from_udp(&mut self, peer: SocketAddr, policy: StreamPeerPolicy) -> bool {
        if self.peer == Some(peer) {
            return false;
        }
        if self
            .peer
            .is_some_and(|current| current.port() != policy.rtsp_client_port)
        {
            return false;
        }
        self.set_peer(peer)
    }

    fn set_peer(&mut self, peer: SocketAddr) -> bool {
        if self.peer == Some(peer) {
            return false;
        }
        self.peer = Some(peer);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_rtsp_client_port_once() {
        let rtsp_peer = "192.0.2.10:50000".parse().unwrap();
        let learned_peer = "192.0.2.10:63589".parse().unwrap();
        let later_peer = "192.0.2.10:63590".parse().unwrap();
        let policy = StreamPeerPolicy::moonlight_nat();

        let mut tracker = StreamPeerTracker::default();
        assert!(tracker.set_from_rtsp(rtsp_peer));
        assert!(tracker.learn_from_udp(learned_peer, policy));
        assert!(!tracker.learn_from_udp(later_peer, policy));
        assert_eq!(tracker.peer(), Some(learned_peer));
    }
}
