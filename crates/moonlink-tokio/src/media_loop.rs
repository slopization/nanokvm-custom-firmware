use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use moonlink_core::{
    AudioFrame, AudioSession, MediaProvider, MediaSession, MediaSessionConfig, ServerError,
    VideoControlMessage, VideoFrame, VideoMode, access_unit_timestamp_90khz,
    audio_frame_timestamp_48khz,
};
use moonlink_protocol::{
    AudioPacketizerState, PacketizerConfig, PacketizerState, packetize_access_unit, packetize_opus,
};
use openssl::symm::{Cipher, Crypter, Mode};
use tokio::net::UdpSocket;
use tokio::sync::{broadcast, watch};

use crate::shared::{AudioCryptoState, ControlCrypto, StreamPeer};
use crate::udp_sender::AsyncUdpSender;

pub(crate) async fn stream_media<P>(
    provider: &P,
    config: MediaSessionConfig,
    video_socket: Arc<UdpSocket>,
    audio_socket: Arc<UdpSocket>,
    video_peer: StreamPeer,
    audio_peer: StreamPeer,
    mut video_control: broadcast::Receiver<VideoControlMessage>,
    mut stream_stop: broadcast::Receiver<()>,
    control_crypto: ControlCrypto,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), ServerError>
where
    P: MediaProvider,
{
    eprintln!("media loop starting: {config:?}");
    let audio_crypto = resolve_audio_crypto(config.audio_encrypted, &control_crypto).await?;
    let mut session = provider.start_session(config.clone()).await?;
    let detached_audio = session.take_audio();
    eprintln!("media session started");
    let result = if let Some(audio) = detached_audio {
        let mut audio_stop = stream_stop.resubscribe();
        let mut audio_shutdown = shutdown.clone();
        let detached_audio_crypto = audio_crypto.clone();
        tokio::select! {
            result = run_media_session(
                &mut session,
                config.clone(),
                video_socket,
                audio_socket.clone(),
                video_peer,
                audio_peer.clone(),
                audio_crypto.as_ref(),
                false,
                &mut video_control,
                &mut stream_stop,
                &mut shutdown,
            ) => result,
            result = run_audio_session(
                audio,
                audio_socket,
                audio_peer,
                detached_audio_crypto,
                &mut audio_stop,
                &mut audio_shutdown,
            ) => result,
        }
    } else {
        run_media_session(
            &mut session,
            config,
            video_socket,
            audio_socket,
            video_peer,
            audio_peer,
            audio_crypto.as_ref(),
            true,
            &mut video_control,
            &mut stream_stop,
            &mut shutdown,
        )
        .await
    };
    let stop_result = session.stop().await;
    match (result, stop_result) {
        (Err(err), _) => Err(err),
        (Ok(()), Err(err)) => Err(ServerError::Media(err)),
        (Ok(()), Ok(())) => Ok(()),
    }
}

async fn run_media_session<S>(
    session: &mut S,
    config: MediaSessionConfig,
    video_socket: Arc<UdpSocket>,
    audio_socket: Arc<UdpSocket>,
    video_peer: StreamPeer,
    audio_peer: StreamPeer,
    audio_crypto: Option<&AudioCryptoState>,
    inline_audio: bool,
    video_control: &mut broadcast::Receiver<VideoControlMessage>,
    stream_stop: &mut broadcast::Receiver<()>,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<(), ServerError>
where
    S: MediaSession,
{
    let mut video_packetizer = PacketizerState::default();
    let mut audio_packetizer = AudioPacketizerState::default();
    let video_sender = AsyncUdpSender::new(video_socket.as_ref());
    let audio_period = Duration::from_millis(u64::from(config.audio_packet_duration_ms));
    let mut next_audio_at = None;
    let mut logged_video = false;
    let mut logged_audio = false;
    loop {
        tokio::select! {
            frame = session.next_video() => {
                let Some(frame) = frame? else {
                    return Ok(());
                };
                let Some(peer) = video_peer.lock().await.peer() else {
                    continue;
                };
                send_video_frame(
                    &video_sender,
                    peer,
                    &mut video_packetizer,
                    config.video_mode,
                    config.video_packetizer_config(),
                    frame,
                ).await?;
                if !logged_video {
                    eprintln!("sent first video frame to {peer}");
                    logged_video = true;
                }
                if inline_audio && config.audio_codec.is_some() {
                    let now = tokio::time::Instant::now();
                    let due = next_audio_at.get_or_insert(now);
                    while *due <= now {
                        if let Some(frame) = session.next_audio().await? {
                            if let Some(peer) = audio_peer.lock().await.peer() {
                                send_audio_frame(&audio_socket, peer, &mut audio_packetizer, audio_crypto, frame).await?;
                                if !logged_audio {
                                    eprintln!("sent first audio frame to {peer}");
                                    logged_audio = true;
                                }
                            }
                        }
                        *due += audio_period;
                    }
                }
            }
            message = video_control.recv() => {
                match message {
                    Ok(message) => session.handle_video_control(message).await?,
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        session.handle_video_control(VideoControlMessage::RequestIdrFrame).await?;
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        return Ok(());
                    }
                }
            }
            stop = stream_stop.recv() => {
                match stop {
                    Ok(()) | Err(broadcast::error::RecvError::Lagged(_)) | Err(broadcast::error::RecvError::Closed) => {
                        return Ok(());
                    }
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

async fn run_audio_session(
    mut session: Box<dyn AudioSession>,
    socket: Arc<UdpSocket>,
    peer: StreamPeer,
    crypto: Option<AudioCryptoState>,
    stream_stop: &mut broadcast::Receiver<()>,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<(), ServerError> {
    let mut packetizer = AudioPacketizerState::default();
    let mut logged_audio = false;
    loop {
        tokio::select! {
            frame = session.next_audio() => {
                let Some(destination) = peer.lock().await.peer() else {
                    continue;
                };
                if let Some(frame) = frame? {
                    send_audio_frame(&socket, destination, &mut packetizer, crypto.as_ref(), frame).await?;
                    if !logged_audio {
                        eprintln!("sent first audio frame to {destination}");
                        logged_audio = true;
                    }
                }
            }
            stop = stream_stop.recv() => {
                match stop {
                    Ok(()) | Err(broadcast::error::RecvError::Lagged(_)) | Err(broadcast::error::RecvError::Closed) => return Ok(()),
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

async fn send_video_frame(
    sender: &AsyncUdpSender<'_>,
    peer: SocketAddr,
    packetizer: &mut PacketizerState,
    mode: VideoMode,
    packetizer_config: PacketizerConfig,
    frame: VideoFrame,
) -> Result<(), ServerError> {
    match frame {
        VideoFrame::FullRtpPacket(packet) if mode == VideoMode::FullRtpPackets => {
            sender.send_packet(&packet.bytes, peer).await?;
        }
        VideoFrame::AccessUnit(access_unit) if mode == VideoMode::EncodedAccessUnits => {
            let timestamp_90khz = access_unit_timestamp_90khz(&access_unit);
            let marker = access_unit.flags.keyframe || access_unit.flags.unit_boundary;
            let packets = packetize_access_unit(
                packetizer,
                packetizer_config,
                &access_unit.data,
                timestamp_90khz,
                marker,
            )
            .map_err(|err| ServerError::InvalidConfig(err.to_string()))?;
            sender.send_packets(peer, &packets.packets).await?;
        }
        VideoFrame::RtpPayload(payload) if mode == VideoMode::RtpPayloads => {
            let packets = packetize_access_unit(
                packetizer,
                packetizer_config,
                &payload.payload,
                payload.timestamp_90khz,
                payload.marker,
            )
            .map_err(|err| ServerError::InvalidConfig(err.to_string()))?;
            sender.send_packets(peer, &packets.packets).await?;
        }
        _ => {
            return Err(ServerError::Media(
                moonlink_core::MediaSessionError::ModeMismatch,
            ));
        }
    }
    Ok(())
}

async fn resolve_audio_crypto(
    audio_encrypted: bool,
    control_crypto: &ControlCrypto,
) -> Result<Option<AudioCryptoState>, ServerError> {
    if !audio_encrypted {
        return Ok(None);
    }
    control_crypto
        .lock()
        .await
        .as_ref()
        .map(|crypto| AudioCryptoState::from(crypto.material.clone()))
        .map(Some)
        .ok_or_else(|| {
            ServerError::InvalidConfig("audio encryption requested before launch rikey".to_owned())
        })
}

async fn send_audio_frame(
    socket: &UdpSocket,
    peer: SocketAddr,
    packetizer: &mut AudioPacketizerState,
    audio_crypto: Option<&AudioCryptoState>,
    frame: AudioFrame,
) -> Result<(), ServerError> {
    let timestamp_48khz = audio_frame_timestamp_48khz(&frame);
    let mut packet = packetize_opus(packetizer, &frame.data, timestamp_48khz);
    if let Some(crypto) = audio_crypto {
        encrypt_audio_payload(&mut packet, crypto)?;
    }
    socket.send_to(&packet, peer).await?;
    Ok(())
}

fn encrypt_audio_payload(
    packet: &mut Vec<u8>,
    crypto: &AudioCryptoState,
) -> Result<(), ServerError> {
    let payload = packet
        .get(12..)
        .ok_or_else(|| ServerError::InvalidConfig("audio RTP packet is too short".to_owned()))?;
    let sequence = u16::from_be_bytes(
        packet[2..4]
            .try_into()
            .expect("audio RTP sequence slice length"),
    );
    let mut iv = [0_u8; 16];
    iv[..4].copy_from_slice(
        &crypto
            .material
            .ri_key_id
            .wrapping_add(u32::from(sequence))
            .to_be_bytes(),
    );

    let cipher = Cipher::aes_128_cbc();
    let mut crypter = Crypter::new(cipher, Mode::Encrypt, &crypto.material.key, Some(&iv))
        .map_err(|err| ServerError::InvalidConfig(err.to_string()))?;
    let mut encrypted = vec![0_u8; payload.len() + cipher.block_size()];
    let mut count = crypter
        .update(payload, &mut encrypted)
        .map_err(|err| ServerError::InvalidConfig(err.to_string()))?;
    count += crypter
        .finalize(&mut encrypted[count..])
        .map_err(|err| ServerError::InvalidConfig(err.to_string()))?;
    encrypted.truncate(count);
    packet.truncate(12);
    packet.extend_from_slice(&encrypted);
    Ok(())
}
