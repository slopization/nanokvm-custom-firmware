mod builder;
mod control_loop;
mod discovery_loop;
mod error;
pub mod file_pairing_store;
mod handle;
mod http_loop;
mod https_loop;
mod media_loop;
mod media_session_manager;
mod pairing;
mod peer_loop;
mod rtsp_loop;
mod server_controller;
mod shared;
mod tls;
mod udp_sender;
mod xml;

pub use builder::ServerBuilder;
pub use error::TokioServerError;
pub use file_pairing_store::FilePairingStore;
pub use handle::ServerHandle;
pub use server_controller::ServerController;

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use bytes::Bytes;
    use moonlink_core::{
        InMemoryPairingStore, MediaProvider, MediaSession, MediaSessionConfig, MediaSessionError,
        ServerConfig, ServerRuntimeConfig, VideoStream, create_access_unit,
    };
    use moonlink_protocol::{CodecCapabilities, VideoCodec};
    use std::net::SocketAddr;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpStream, UdpSocket};

    struct EmptyProvider;

    #[async_trait]
    impl MediaProvider for EmptyProvider {
        type Session = EmptySession;

        async fn start_session(
            &self,
            _config: MediaSessionConfig,
        ) -> Result<Self::Session, MediaSessionError> {
            Ok(EmptySession)
        }
    }

    struct EmptySession;

    #[async_trait]
    impl MediaSession for EmptySession {
        async fn next_video(
            &mut self,
        ) -> Result<Option<moonlink_core::VideoFrame>, MediaSessionError> {
            Ok(None)
        }
    }

    struct OneFrameProvider;

    #[async_trait]
    impl MediaProvider for OneFrameProvider {
        type Session = OneFrameSession;

        async fn start_session(
            &self,
            _config: MediaSessionConfig,
        ) -> Result<Self::Session, MediaSessionError> {
            Ok(OneFrameSession { sent: false })
        }
    }

    struct OneFrameSession {
        sent: bool,
    }

    #[async_trait]
    impl MediaSession for OneFrameSession {
        async fn next_video(
            &mut self,
        ) -> Result<Option<moonlink_core::VideoFrame>, MediaSessionError> {
            if self.sent {
                return Ok(None);
            }
            self.sent = true;
            Ok(Some(moonlink_core::VideoFrame::AccessUnit(
                create_access_unit(
                    Bytes::from_static(&[0, 0, 0, 1, 0x67, 1, 2, 3, 0, 0, 0, 1, 0x65, 4, 5])
                        .to_vec(),
                    90_000,
                    true,
                ),
            )))
        }
    }

    #[tokio::test]
    async fn starts_and_stops_server() {
        let config = ServerConfig::new(
            "test",
            "127.0.0.1:0".parse().unwrap(),
            "127.0.0.1:0".parse().unwrap(),
            "127.0.0.1:0".parse().unwrap(),
            CodecCapabilities::video_only(vec![VideoCodec::H264]),
        )
        .with_stream_ports(
            "127.0.0.1:0".parse().unwrap(),
            "127.0.0.1:0".parse().unwrap(),
            "127.0.0.1:0".parse().unwrap(),
        );
        let handle = ServerBuilder::new(config)
            .media_provider(EmptyProvider)
            .input_sink(|_: moonlink_core::InputContext, _: moonlink_protocol::InputEvent| async {})
            .pairing_store(InMemoryPairingStore::default())
            .serve()
            .await
            .unwrap();

        assert_ne!(handle.http_addr().port(), 0);
        assert_ne!(handle.discovery_addr().port(), 0);
        assert_ne!(handle.rtsp_addr().port(), 0);
        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn applies_runtime_config_to_discovery() {
        let config = ServerConfig::new(
            "before",
            "127.0.0.1:0".parse().unwrap(),
            "127.0.0.1:0".parse().unwrap(),
            "127.0.0.1:0".parse().unwrap(),
            CodecCapabilities::video_only(vec![VideoCodec::H264]),
        )
        .with_stream_ports(
            "127.0.0.1:0".parse().unwrap(),
            "127.0.0.1:0".parse().unwrap(),
            "127.0.0.1:0".parse().unwrap(),
        );
        let handle = ServerBuilder::new(config)
            .media_provider(EmptyProvider)
            .input_sink(|_: moonlink_core::InputContext, _: moonlink_protocol::InputEvent| async {})
            .pairing_store(InMemoryPairingStore::default())
            .serve()
            .await
            .unwrap();
        handle
            .controller()
            .apply_runtime_config(ServerRuntimeConfig {
                name: "after".to_owned(),
                capabilities: CodecCapabilities::video_only(vec![VideoCodec::H264]),
                video_stream: VideoStream::default(),
                encryption_mode: moonlink_core::EncryptionMode::Opportunistic,
            });

        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        socket
            .send_to(b"probe", handle.discovery_addr())
            .await
            .unwrap();
        let mut response = [0_u8; 256];
        let read = socket.recv(&mut response).await.unwrap();
        assert!(String::from_utf8_lossy(&response[..read]).ends_with("after"));
        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn rtsp_handshake_starts_video_packets() {
        let client_video = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let client_video_port = client_video.local_addr().unwrap().port();
        let config = ServerConfig::new(
            "test",
            "127.0.0.1:0".parse().unwrap(),
            "127.0.0.1:0".parse().unwrap(),
            "127.0.0.1:0".parse().unwrap(),
            CodecCapabilities::video_only(vec![VideoCodec::H264]),
        )
        .with_stream_ports(
            "127.0.0.1:0".parse().unwrap(),
            "127.0.0.1:0".parse().unwrap(),
            "127.0.0.1:0".parse().unwrap(),
        );
        let handle = ServerBuilder::new(config)
            .media_provider(OneFrameProvider)
            .input_sink(|_: moonlink_core::InputContext, _: moonlink_protocol::InputEvent| async {})
            .pairing_store(InMemoryPairingStore::default())
            .serve()
            .await
            .unwrap();

        transact(
            handle.rtsp_addr(),
            "OPTIONS rtsp://127.0.0.1 RTSP/1.0\r\nCSeq: 1\r\n\r\n",
        )
        .await;
        transact(
            handle.rtsp_addr(),
            "DESCRIBE rtsp://127.0.0.1 RTSP/1.0\r\nCSeq: 2\r\nAccept: application/sdp\r\n\r\n",
        )
        .await;
        let setup = format!(
            "SETUP streamid=video/0/0 RTSP/1.0\r\nCSeq: 3\r\nTransport: unicast;X-GS-ClientPort={client_video_port}-{}\r\n\r\n",
            client_video_port + 1
        );
        transact(handle.rtsp_addr(), &setup).await;
        let body = concat!(
            "v=0\r\n",
            "s=Moonlight\r\n",
            "a=x-nv-video[0].clientViewportWd:1280\r\n",
            "a=x-nv-video[0].clientViewportHt:720\r\n",
            "a=x-nv-video[0].maxFPS:60\r\n",
            "a=x-nv-vqos[0].bitStreamFormat:0\r\n"
        );
        let announce = format!(
            "ANNOUNCE streamid=video RTSP/1.0\r\nCSeq: 4\r\nSession: DEADBEEFCAFE\r\nContent-Type: application/sdp\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        transact(handle.rtsp_addr(), &announce).await;
        transact(
            handle.rtsp_addr(),
            "PLAY / RTSP/1.0\r\nCSeq: 5\r\nSession: DEADBEEFCAFE\r\n\r\n",
        )
        .await;

        let mut packet = [0_u8; 1500];
        let received =
            tokio::time::timeout(Duration::from_secs(2), client_video.recv_from(&mut packet))
                .await
                .unwrap()
                .unwrap()
                .0;
        assert!(received > 32);
        assert_eq!(packet[0], 0x90);
        assert_eq!(packet[1], 96);

        handle.shutdown().await.unwrap();
    }

    async fn transact(addr: SocketAddr, request: &str) -> String {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut response = vec![0_u8; 8192];
        let read = stream.read(&mut response).await.unwrap();
        let response = String::from_utf8_lossy(&response[..read]).to_string();
        assert!(response.starts_with("RTSP/1.0 200 OK"), "{response}");
        response
    }
}
