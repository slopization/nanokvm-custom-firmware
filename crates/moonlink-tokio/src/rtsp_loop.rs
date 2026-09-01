use std::net::SocketAddr;
use std::sync::Arc;

use moonlink_core::{
    EncryptionMode, MediaProvider, MediaSessionConfig, MediaSessionId, ServerError,
    ServerRuntimeConfig, VideoMode,
};
use moonlink_protocol::{
    AudioCodec, DEFAULT_VIDEO_PACKET_SIZE, FEATURE_FLAG_AUDIO_ENCRYPTION, RtspResponse,
    STREAM_ENCRYPTION_ALL, STREAM_ENCRYPTION_AUDIO, STREAM_ENCRYPTION_CONTROL_V2,
    STREAM_ENCRYPTION_MEDIA, STREAM_ENCRYPTION_VIDEO, VideoCodec, parse_rtsp_request,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::{Mutex, broadcast, watch};

use crate::media_loop::stream_media;
use crate::media_session_manager::MediaSessionManager;
use crate::shared::{ControlCrypto, StreamPeer, StreamPorts};

pub(crate) async fn serve_rtsp<P>(
    listener: TcpListener,
    runtime_config: watch::Receiver<ServerRuntimeConfig>,
    media_provider: Arc<P>,
    video_socket: Arc<UdpSocket>,
    audio_socket: Arc<UdpSocket>,
    ports: StreamPorts,
    video_peer: StreamPeer,
    audio_peer: StreamPeer,
    video_control: broadcast::Sender<moonlink_core::VideoControlMessage>,
    stream_stop: broadcast::Sender<()>,
    control_crypto: ControlCrypto,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), ServerError>
where
    P: MediaProvider,
{
    let session_state = Arc::new(Mutex::new(RtspSessionState::default()));
    let media_sessions = Arc::new(Mutex::new(MediaSessionManager::default()));
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, peer) = accepted?;
                let runtime_config = runtime_config.clone();
                let media_provider = Arc::clone(&media_provider);
                let video_socket = Arc::clone(&video_socket);
                let audio_socket = Arc::clone(&audio_socket);
                let video_peer = Arc::clone(&video_peer);
                let audio_peer = Arc::clone(&audio_peer);
                let video_control = video_control.clone();
                let stream_stop = stream_stop.clone();
                let control_crypto = Arc::clone(&control_crypto);
                let session_state = Arc::clone(&session_state);
                let media_sessions = Arc::clone(&media_sessions);
                let shutdown = shutdown.clone();
                tokio::spawn(async move {
                    if let Err(err) = process_rtsp_connection(stream, peer, runtime_config, media_provider, video_socket, audio_socket, ports, video_peer, audio_peer, video_control, stream_stop, control_crypto, session_state, media_sessions, shutdown).await {
                        eprintln!("rtsp connection from {peer} failed: {err}");
                    }
                });
            }
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    return Ok(());
                }
            }
        }
    }
}

async fn process_rtsp_connection<P>(
    mut stream: TcpStream,
    peer: SocketAddr,
    runtime_config: watch::Receiver<ServerRuntimeConfig>,
    media_provider: Arc<P>,
    video_socket: Arc<UdpSocket>,
    audio_socket: Arc<UdpSocket>,
    ports: StreamPorts,
    video_peer: StreamPeer,
    audio_peer: StreamPeer,
    video_control: broadcast::Sender<moonlink_core::VideoControlMessage>,
    stream_stop: broadcast::Sender<()>,
    control_crypto: ControlCrypto,
    session_state: Arc<Mutex<RtspSessionState>>,
    media_sessions: Arc<Mutex<MediaSessionManager>>,
    shutdown: watch::Receiver<bool>,
) -> Result<(), ServerError>
where
    P: MediaProvider,
{
    let mut buffer = Vec::new();
    let mut read_buffer = [0_u8; 8192];
    loop {
        let read = stream.read(&mut read_buffer).await?;
        if read == 0 {
            return Ok(());
        }
        buffer.extend_from_slice(&read_buffer[..read]);
        let Some((request, _used)) = parse_rtsp_request(&buffer)
            .map_err(|err| ServerError::InvalidConfig(err.to_string()))?
        else {
            continue;
        };

        let cseq = request.cseq().map(str::to_owned);
        let runtime_config = runtime_config.borrow().clone();
        let mut response = RtspResponse::ok(cseq.as_deref());
        eprintln!(
            "rtsp request from {peer}: method={} target={}",
            request.method, request.target
        );
        match request.method.as_str() {
            "OPTIONS" => {
                clear_stream_peers(&video_peer, &audio_peer).await;
                response.headers.insert(
                    "Public".to_owned(),
                    "OPTIONS, DESCRIBE, SETUP, ANNOUNCE, PLAY, TEARDOWN".to_owned(),
                );
            }
            "DESCRIBE" => {
                response
                    .headers
                    .insert("Content-Type".to_owned(), "application/sdp".to_owned());
                response.body = describe_sdp(&runtime_config).into_bytes();
            }
            "SETUP" => {
                let stream_kind = detect_stream_kind(&request.target);
                if matches!(stream_kind, Some("video" | "audio"))
                    && let Some(port) = request
                        .headers
                        .get("transport")
                        .and_then(|transport| parse_client_port(transport))
                {
                    eprintln!(
                        "rtsp setup {:?}: transport={:?} client_port={port}",
                        stream_kind,
                        request.headers.get("transport")
                    );
                    let stream_peer = match stream_kind {
                        Some("audio") => &audio_peer,
                        _ => &video_peer,
                    };
                    let peer = SocketAddr::new(peer.ip(), port);
                    stream_peer.lock().await.set_from_rtsp(peer);
                    eprintln!("rtsp setup {:?}: peer={peer}", stream_kind);
                }
                response
                    .headers
                    .insert("Session".to_owned(), "DEADBEEFCAFE".to_owned());
                response
                    .headers
                    .insert("Transport".to_owned(), setup_transport(stream_kind, ports));
            }
            "ANNOUNCE" => {
                let mut state = session_state.lock().await;
                apply_announce(&mut state, &request.body);
                response
                    .headers
                    .insert("Session".to_owned(), "DEADBEEFCAFE".to_owned());
            }
            "PLAY" => {
                if !request.target.contains("audio") {
                    let state = session_state.lock().await.clone();
                    let media_encryption = negotiated_media_encryption(
                        runtime_config.encryption_mode,
                        state.encryption_flags,
                    );
                    if media_encryption.is_none() {
                        response.status = 403;
                        response.reason = "Forbidden".to_owned();
                        eprintln!(
                            "rejecting client without mandatory media encryption: enabled={:#x}",
                            state.encryption_flags
                        );
                    } else {
                        let (session_shutdown_tx, session_shutdown_rx) = watch::channel(false);
                        let Some(media_encryption) = media_encryption else {
                            unreachable!("mandatory media encryption was validated");
                        };
                        let session_config = MediaSessionConfig {
                            session_id: MediaSessionId::new(),
                            app_id: 1,
                            // Moonlight announces its UI-selected mode even when a fixed-capture
                            // host only supports the mode advertised in serverinfo and RTSP SDP.
                            width: runtime_config.video_stream.width,
                            height: runtime_config.video_stream.height,
                            fps: runtime_config.video_stream.fps,
                            video_codec: state.video_codec.unwrap_or(VideoCodec::H264),
                            audio_codec: Some(AudioCodec::Opus)
                                .filter(|_| !runtime_config.capabilities.audio.is_empty()),
                            video_mode: VideoMode::EncodedAccessUnits,
                            video_packet_size: state
                                .video_packet_size
                                .unwrap_or(DEFAULT_VIDEO_PACKET_SIZE),
                            video_fec_percentage: runtime_config.video_stream.fec_percentage,
                            video_min_parity_shards: runtime_config.video_stream.min_parity_shards,
                            video_encrypted: media_encryption & STREAM_ENCRYPTION_VIDEO != 0,
                            audio_encrypted: media_encryption & STREAM_ENCRYPTION_AUDIO != 0,
                            audio_packet_duration_ms: state.audio_packet_duration_ms.unwrap_or(5),
                        };
                        let mut media_sessions = media_sessions.lock().await;
                        media_sessions.stop().await;
                        let provider = Arc::clone(&media_provider);
                        eprintln!("rtsp play: starting media session {session_config:?}");
                        let socket = Arc::clone(&video_socket);
                        let audio_socket = Arc::clone(&audio_socket);
                        let video_peer = Arc::clone(&video_peer);
                        let audio_peer = Arc::clone(&audio_peer);
                        let video_control = video_control.subscribe();
                        let stream_stop = stream_stop.subscribe();
                        let mut server_shutdown = shutdown.clone();
                        let media_shutdown_tx = session_shutdown_tx.clone();
                        let task = tokio::spawn(async move {
                            let media = stream_media(
                                provider.as_ref(),
                                session_config,
                                socket,
                                audio_socket,
                                video_peer,
                                audio_peer,
                                video_control,
                                stream_stop,
                                control_crypto,
                                session_shutdown_rx,
                            );
                            tokio::pin!(media);
                            let result = tokio::select! {
                                result = &mut media => result,
                                changed = server_shutdown.changed() => {
                                    if changed.is_ok() && *server_shutdown.borrow() {
                                        let _ = media_shutdown_tx.send(true);
                                    }
                                    media.await
                                }
                            };
                            if let Err(err) = result {
                                eprintln!("media stream stopped with error: {err}");
                            }
                        });
                        media_sessions.start(session_shutdown_tx, task);
                    }
                }
                response
                    .headers
                    .insert("Session".to_owned(), "DEADBEEFCAFE".to_owned());
            }
            "TEARDOWN" => {
                if !request.target.contains("audio") {
                    media_sessions.lock().await.stop().await;
                    clear_stream_peers(&video_peer, &audio_peer).await;
                }
                response
                    .headers
                    .insert("Session".to_owned(), "DEADBEEFCAFE".to_owned());
            }
            _ => {}
        }

        stream.write_all(&response.serialize()).await?;
        stream.shutdown().await?;
        // Moonlight-common-c waits for RTSP response EOF, so this server closes after one request.
        return Ok(());
    }
}

async fn clear_stream_peers(video_peer: &StreamPeer, audio_peer: &StreamPeer) {
    video_peer.lock().await.clear();
    audio_peer.lock().await.clear();
}

#[derive(Default, Clone)]
struct RtspSessionState {
    video_codec: Option<VideoCodec>,
    video_packet_size: Option<usize>,
    encryption_flags: u32,
    audio_packet_duration_ms: Option<u32>,
}

fn describe_sdp(config: &ServerRuntimeConfig) -> String {
    let (encryption_supported, encryption_requested) = match config.encryption_mode {
        EncryptionMode::Never => (STREAM_ENCRYPTION_CONTROL_V2, STREAM_ENCRYPTION_CONTROL_V2),
        EncryptionMode::Opportunistic => (STREAM_ENCRYPTION_ALL, STREAM_ENCRYPTION_CONTROL_V2),
        EncryptionMode::Mandatory => (STREAM_ENCRYPTION_ALL, STREAM_ENCRYPTION_ALL),
    };
    let video_attributes = if config.capabilities.video.contains(&VideoCodec::Hevc) {
        "a=rtpmap:96 H264/90000\r\n\
         a=fmtp:96 packetization-mode=1; sprop-parameter-sets=AAAAAU\r\n"
    } else {
        "a=rtpmap:96 H264/90000\r\n\
         a=fmtp:96 packetization-mode=1\r\n"
    };
    format!(
        "v=0\r\n\
         o=android 0 9 IN IPv4 127.0.0.1\r\n\
         s=NVIDIA Streaming Client\r\n\
         t=0 0\r\n\
         m=video 47998  \r\n\
         {video_attributes}\
         a=x-ss-general.encryptionSupported:{encryption_supported}\r\n\
         a=x-ss-general.encryptionRequested:{encryption_requested}\r\n\
         a=x-nv-video[0].packetSize:{packet_size}\r\n\
         a=x-nv-video[0].clientViewportWd:{width}\r\n\
         a=x-nv-video[0].clientViewportHt:{height}\r\n\
         a=x-nv-video[0].maxFPS:{fps}\r\n\
         a=x-nv-vqos[0].fec.enable:{fec_enabled}\r\n\
         a=x-nv-vqos[0].fec.repairPercent:{fec_percentage}\r\n\
         a=x-nv-vqos[0].fec.minRequiredFecPackets:{min_parity_shards}\r\n\
         m=audio 48000  \r\n\
         a=rtpmap:97 opus/48000/2\r\n",
        width = config.video_stream.width,
        height = config.video_stream.height,
        fps = config.video_stream.fps,
        packet_size = DEFAULT_VIDEO_PACKET_SIZE,
        fec_enabled = u8::from(config.video_stream.fec_percentage != 0),
        fec_percentage = config.video_stream.fec_percentage,
        min_parity_shards = config.video_stream.min_parity_shards
    )
}

fn negotiated_media_encryption(mode: EncryptionMode, enabled: u32) -> Option<u32> {
    match mode {
        EncryptionMode::Never => Some(0),
        EncryptionMode::Opportunistic => Some(enabled & STREAM_ENCRYPTION_MEDIA),
        EncryptionMode::Mandatory
            if enabled & STREAM_ENCRYPTION_MEDIA == STREAM_ENCRYPTION_MEDIA =>
        {
            Some(STREAM_ENCRYPTION_MEDIA)
        }
        EncryptionMode::Mandatory => None,
    }
}

fn detect_stream_kind(target: &str) -> Option<&'static str> {
    if target.contains("video") {
        Some("video")
    } else if target.contains("audio") {
        Some("audio")
    } else if target.contains("control") {
        Some("control")
    } else {
        None
    }
}

fn setup_transport(stream_kind: Option<&str>, ports: StreamPorts) -> String {
    let port = match stream_kind {
        Some("audio") => ports.audio,
        Some("control") => ports.control,
        _ => ports.video,
    };
    format!("unicast;server_port={port}-{port}")
}

fn parse_client_port(transport: &str) -> Option<u16> {
    let ports = transport.split(';').find_map(|part| {
        let (name, value) = part.trim().split_once('=')?;
        matches!(
            name.to_ascii_lowercase().as_str(),
            "x-gs-clientport" | "client_port"
        )
        .then_some(value)
    })?;
    let first = ports.split_once('-').map_or(ports, |(first, _)| first);
    first.parse().ok()
}

fn apply_announce(state: &mut RtspSessionState, body: &[u8]) {
    state.encryption_flags = 0;
    let body = String::from_utf8_lossy(body);
    for line in body.lines() {
        if let Some(value) = line.strip_prefix("a=x-nv-vqos[0].bitStreamFormat:") {
            state.video_codec = if value.trim() == "1" {
                Some(VideoCodec::Hevc)
            } else {
                Some(VideoCodec::H264)
            };
        } else if let Some(value) = line.strip_prefix("a=x-nv-video[0].packetSize:") {
            state.video_packet_size = parse_video_packet_size(value.trim());
        } else if let Some(value) = line.strip_prefix("a=x-nv-general.featureFlags:") {
            if parse_feature_flags(value)
                .is_some_and(|flags| flags & FEATURE_FLAG_AUDIO_ENCRYPTION != 0)
            {
                state.encryption_flags |= STREAM_ENCRYPTION_AUDIO;
            }
        } else if let Some(value) = line.strip_prefix("a=x-ss-general.encryptionEnabled:") {
            state.encryption_flags |=
                parse_feature_flags(value).unwrap_or(0) & STREAM_ENCRYPTION_ALL;
        } else if let Some(value) = line.strip_prefix("a=x-nv-aqos.packetDuration:") {
            state.audio_packet_duration_ms = value
                .trim()
                .parse()
                .ok()
                .filter(|duration| matches!(duration, 5 | 10 | 20));
        }
    }
}

fn parse_feature_flags(value: &str) -> Option<u32> {
    let value = value.trim();
    value.parse().ok().or_else(|| {
        value
            .strip_prefix("0x")
            .or_else(|| value.strip_prefix("0X"))
            .and_then(|hex| u32::from_str_radix(hex, 16).ok())
    })
}

fn parse_video_packet_size(value: &str) -> Option<usize> {
    let packet_size = value.parse::<usize>().ok()?;
    (packet_size >= 512).then_some(packet_size - packet_size % 16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_moonlight_transport_client_ports() {
        assert_eq!(
            parse_client_port("unicast;X-GS-ClientPort=47998-47999"),
            Some(47998)
        );
        assert_eq!(
            parse_client_port("RTP/AVP;unicast;client_port=47998-47999"),
            Some(47998)
        );
    }

    #[test]
    fn parses_audio_encryption_feature_flag() {
        let mut state = RtspSessionState::default();
        apply_announce(&mut state, b"a=x-nv-general.featureFlags:160\r\n");
        assert_ne!(state.encryption_flags & STREAM_ENCRYPTION_AUDIO, 0);
        apply_announce(&mut state, b"a=x-nv-general.featureFlags:128\r\n");
        assert_eq!(state.encryption_flags, 0);
    }

    #[test]
    fn parses_audio_packet_duration() {
        let mut state = RtspSessionState::default();
        apply_announce(&mut state, b"a=x-nv-aqos.packetDuration:5\r\n");
        assert_eq!(state.audio_packet_duration_ms, Some(5));
    }

    #[test]
    fn advertises_encryption_policy() {
        let mut config = ServerRuntimeConfig {
            name: "test".to_owned(),
            capabilities: moonlink_protocol::CodecCapabilities::video_only(vec![VideoCodec::H264]),
            video_stream: moonlink_core::VideoStream::default(),
            encryption_mode: EncryptionMode::Never,
        };
        let never = describe_sdp(&config);
        assert!(never.contains("encryptionSupported:1\r\n"));
        assert!(never.contains("encryptionRequested:1\r\n"));

        config.encryption_mode = EncryptionMode::Opportunistic;
        let opportunistic = describe_sdp(&config);
        assert!(opportunistic.contains("encryptionSupported:7\r\n"));
        assert!(opportunistic.contains("encryptionRequested:1\r\n"));

        config.encryption_mode = EncryptionMode::Mandatory;
        let mandatory = describe_sdp(&config);
        assert!(mandatory.contains("encryptionSupported:7\r\n"));
        assert!(mandatory.contains("encryptionRequested:7\r\n"));
    }

    #[test]
    fn enforces_media_encryption_policy() {
        assert_eq!(
            negotiated_media_encryption(EncryptionMode::Never, STREAM_ENCRYPTION_MEDIA),
            Some(0)
        );
        assert_eq!(
            negotiated_media_encryption(EncryptionMode::Opportunistic, STREAM_ENCRYPTION_VIDEO),
            Some(STREAM_ENCRYPTION_VIDEO)
        );
        assert_eq!(
            negotiated_media_encryption(EncryptionMode::Mandatory, STREAM_ENCRYPTION_VIDEO),
            None
        );
        assert_eq!(
            negotiated_media_encryption(EncryptionMode::Mandatory, STREAM_ENCRYPTION_MEDIA),
            Some(STREAM_ENCRYPTION_MEDIA)
        );
    }
}
