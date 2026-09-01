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
use openssl::cipher::Cipher;
use openssl::cipher_ctx::CipherCtx;
use tokio::net::UdpSocket;
use tokio::sync::{broadcast, watch};

use crate::shared::{ControlCrypto, RiKeyMaterial, StreamPeer};
use crate::udp_sender::AsyncUdpSender;

const VIDEO_ENCRYPTION_PREFIX_LEN: usize = 32;
const VIDEO_GCM_TAG_LEN: usize = 16;
const VIDEO_FRAME_INDEX_OFFSET: usize = 20;

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
    let crypto_material = resolve_crypto_material(
        config.video_encrypted || config.audio_encrypted,
        &control_crypto,
    )
    .await?;
    let mut video_crypto = config
        .video_encrypted
        .then(|| VideoEncryptor::new(required_material(&crypto_material)))
        .transpose()?;
    let mut audio_crypto = config
        .audio_encrypted
        .then(|| AudioEncryptor::new(required_material(&crypto_material)))
        .transpose()?;
    let mut session = provider.start_session(config.clone()).await?;
    let detached_audio = session.take_audio();
    eprintln!("media session started");
    let result = if let Some(audio) = detached_audio {
        let mut audio_stop = stream_stop.resubscribe();
        let mut audio_shutdown = shutdown.clone();
        tokio::select! {
            result = run_media_session(
                &mut session,
                config.clone(),
                video_socket,
                audio_socket.clone(),
                video_peer,
                audio_peer.clone(),
                video_crypto.as_mut(),
                None,
                false,
                &mut video_control,
                &mut stream_stop,
                &mut shutdown,
            ) => result,
            result = run_audio_session(
                audio,
                audio_socket,
                audio_peer,
                audio_crypto,
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
            video_crypto.as_mut(),
            audio_crypto.as_mut(),
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
    mut video_crypto: Option<&mut VideoEncryptor>,
    mut audio_crypto: Option<&mut AudioEncryptor>,
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
                    video_crypto.as_deref_mut(),
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
                        if let Some(frame) = session.next_audio().await?
                            && let Some(peer) = audio_peer.lock().await.peer()
                        {
                            send_audio_frame(&audio_socket, peer, &mut audio_packetizer, audio_crypto.as_deref_mut(), frame).await?;
                            if !logged_audio {
                                eprintln!("sent first audio frame to {peer}");
                                logged_audio = true;
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
    mut crypto: Option<AudioEncryptor>,
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
                    send_audio_frame(&socket, destination, &mut packetizer, crypto.as_mut(), frame).await?;
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
    mut crypto: Option<&mut VideoEncryptor>,
    frame: VideoFrame,
) -> Result<(), ServerError> {
    match frame {
        VideoFrame::FullRtpPacket(packet) if mode == VideoMode::FullRtpPackets => {
            if let Some(crypto) = crypto.as_mut() {
                let mut packet = packet.bytes.to_vec();
                crypto.encrypt_packet(&mut packet)?;
                sender.send_packet(&packet, peer).await?;
            } else {
                sender.send_packet(&packet.bytes, peer).await?;
            }
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
            let mut packets = packets.packets;
            encrypt_video_packets(&mut packets, crypto)?;
            sender.send_packets(peer, &packets).await?;
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
            let mut packets = packets.packets;
            encrypt_video_packets(&mut packets, crypto)?;
            sender.send_packets(peer, &packets).await?;
        }
        _ => {
            return Err(ServerError::Media(
                moonlink_core::MediaSessionError::ModeMismatch,
            ));
        }
    }
    Ok(())
}

async fn resolve_crypto_material(
    encrypted: bool,
    control_crypto: &ControlCrypto,
) -> Result<Option<RiKeyMaterial>, ServerError> {
    if !encrypted {
        return Ok(None);
    }
    control_crypto
        .lock()
        .await
        .as_ref()
        .map(|crypto| crypto.material.clone())
        .map(Some)
        .ok_or_else(|| {
            ServerError::InvalidConfig("media encryption requested before launch rikey".to_owned())
        })
}

fn required_material(material: &Option<RiKeyMaterial>) -> &RiKeyMaterial {
    material
        .as_ref()
        .expect("encrypted media has validated RI key material")
}

async fn send_audio_frame(
    socket: &UdpSocket,
    peer: SocketAddr,
    packetizer: &mut AudioPacketizerState,
    audio_crypto: Option<&mut AudioEncryptor>,
    frame: AudioFrame,
) -> Result<(), ServerError> {
    let timestamp_48khz = audio_frame_timestamp_48khz(&frame);
    let mut packet = packetize_opus(packetizer, &frame.data, timestamp_48khz);
    if let Some(crypto) = audio_crypto {
        crypto.encrypt_packet(&mut packet)?;
    }
    socket.send_to(&packet, peer).await?;
    Ok(())
}

fn encrypt_video_packets(
    packets: &mut [Vec<u8>],
    crypto: Option<&mut VideoEncryptor>,
) -> Result<(), ServerError> {
    let Some(crypto) = crypto else {
        return Ok(());
    };
    for packet in packets {
        crypto.encrypt_packet(packet)?;
    }
    Ok(())
}

struct VideoEncryptor {
    context: CipherCtx,
    next_iv: u64,
}

impl VideoEncryptor {
    fn new(material: &RiKeyMaterial) -> Result<Self, ServerError> {
        let mut context = CipherCtx::new().map_err(crypto_error)?;
        context
            .encrypt_init(
                Some(Cipher::aes_128_gcm()),
                Some(&material.key),
                Some(&[0; 12]),
            )
            .map_err(crypto_error)?;
        context.set_padding(false);
        Ok(Self {
            context,
            next_iv: 0,
        })
    }

    fn encrypt_packet(&mut self, packet: &mut Vec<u8>) -> Result<(), ServerError> {
        let frame_number = u32::from_le_bytes(
            packet
                .get(VIDEO_FRAME_INDEX_OFFSET..VIDEO_FRAME_INDEX_OFFSET + 4)
                .ok_or_else(|| {
                    ServerError::InvalidConfig("video packet is too short to encrypt".to_owned())
                })?
                .try_into()
                .expect("video frame number slice length"),
        );
        let next_iv = self.next_iv.checked_add(1).ok_or_else(|| {
            ServerError::InvalidConfig("video encryption IV space exhausted".to_owned())
        })?;
        let mut iv = [0_u8; 12];
        iv[..8].copy_from_slice(&self.next_iv.to_le_bytes());
        iv[11] = b'V';
        self.next_iv = next_iv;

        let plaintext_len = packet.len();
        packet.reserve(VIDEO_ENCRYPTION_PREFIX_LEN);
        packet.resize(plaintext_len + VIDEO_ENCRYPTION_PREFIX_LEN, 0);
        packet.copy_within(0..plaintext_len, VIDEO_ENCRYPTION_PREFIX_LEN);
        packet[..12].copy_from_slice(&iv);
        packet[12..16].copy_from_slice(&frame_number.to_le_bytes());

        self.context
            .encrypt_init(None, None, Some(&iv))
            .map_err(crypto_error)?;
        let encrypted = self
            .context
            .cipher_update_inplace(&mut packet[VIDEO_ENCRYPTION_PREFIX_LEN..], plaintext_len)
            .map_err(crypto_error)?;
        let finalized = self
            .context
            .cipher_final(&mut packet[VIDEO_ENCRYPTION_PREFIX_LEN + encrypted..])
            .map_err(crypto_error)?;
        if encrypted + finalized != plaintext_len {
            return Err(ServerError::InvalidConfig(
                "AES-GCM changed the video packet length".to_owned(),
            ));
        }
        self.context
            .tag(&mut packet[16..16 + VIDEO_GCM_TAG_LEN])
            .map_err(crypto_error)
    }
}

struct AudioEncryptor {
    context: CipherCtx,
    ri_key_id: u32,
}

impl AudioEncryptor {
    fn new(material: &RiKeyMaterial) -> Result<Self, ServerError> {
        let mut context = CipherCtx::new().map_err(crypto_error)?;
        context
            .encrypt_init(
                Some(Cipher::aes_128_cbc()),
                Some(&material.key),
                Some(&[0; 16]),
            )
            .map_err(crypto_error)?;
        Ok(Self {
            context,
            ri_key_id: material.ri_key_id,
        })
    }

    fn encrypt_packet(&mut self, packet: &mut Vec<u8>) -> Result<(), ServerError> {
        let sequence = u16::from_be_bytes(
            packet
                .get(2..4)
                .ok_or_else(|| {
                    ServerError::InvalidConfig("audio RTP packet is too short".to_owned())
                })?
                .try_into()
                .expect("audio RTP sequence slice length"),
        );
        let payload_len = packet.len().checked_sub(12).ok_or_else(|| {
            ServerError::InvalidConfig("audio RTP packet is too short".to_owned())
        })?;
        let mut iv = [0_u8; 16];
        iv[..4].copy_from_slice(
            &self
                .ri_key_id
                .wrapping_add(u32::from(sequence))
                .to_be_bytes(),
        );

        self.context
            .encrypt_init(None, None, Some(&iv))
            .map_err(crypto_error)?;
        packet.resize(packet.len() + 16, 0);
        let encrypted = self
            .context
            .cipher_update_inplace(&mut packet[12..], payload_len)
            .map_err(crypto_error)?;
        let finalized = self
            .context
            .cipher_final(&mut packet[12 + encrypted..])
            .map_err(crypto_error)?;
        packet.truncate(12 + encrypted + finalized);
        Ok(())
    }
}

fn crypto_error(error: openssl::error::ErrorStack) -> ServerError {
    ServerError::InvalidConfig(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use openssl::symm::{Cipher as SymmetricCipher, Crypter, Mode};

    fn material() -> RiKeyMaterial {
        RiKeyMaterial::new([0x3c; 16], 0x1020_3040)
    }

    #[test]
    fn encrypts_video_fec_shards_in_moonlight_format() {
        let mut packetizer = PacketizerState::default();
        let plaintext = packetize_access_unit(
            &mut packetizer,
            PacketizerConfig::default(),
            &[0x65; 2_400],
            90_000,
            true,
        )
        .unwrap()
        .packets;
        let mut encrypted = plaintext.clone();
        let mut encryptor = VideoEncryptor::new(&material()).unwrap();
        encrypt_video_packets(&mut encrypted, Some(&mut encryptor)).unwrap();

        for (index, (encrypted, plaintext)) in encrypted.iter().zip(&plaintext).enumerate() {
            assert_eq!(
                encrypted.len(),
                plaintext.len() + VIDEO_ENCRYPTION_PREFIX_LEN
            );
            assert_eq!(encrypted[11], b'V');
            assert_eq!(
                u64::from_le_bytes(encrypted[..8].try_into().unwrap()),
                index as u64
            );
            assert_eq!(
                u32::from_le_bytes(encrypted[12..16].try_into().unwrap()),
                u32::from_le_bytes(
                    plaintext[VIDEO_FRAME_INDEX_OFFSET..VIDEO_FRAME_INDEX_OFFSET + 4]
                        .try_into()
                        .unwrap()
                )
            );
            assert_eq!(decrypt_video_packet(encrypted), *plaintext);
        }
    }

    #[test]
    fn reuses_audio_cbc_context() {
        let material = material();
        let mut encryptor = AudioEncryptor::new(&material).unwrap();
        let mut packetizer = AudioPacketizerState::default();
        for timestamp in [960, 1_920] {
            let mut packet = packetize_opus(&mut packetizer, &[0xf8, 0xff, 0xfe], timestamp);
            let plaintext = packet[12..].to_vec();
            let sequence = u16::from_be_bytes(packet[2..4].try_into().unwrap());
            encryptor.encrypt_packet(&mut packet).unwrap();

            let mut iv = [0_u8; 16];
            iv[..4].copy_from_slice(
                &material
                    .ri_key_id
                    .wrapping_add(u32::from(sequence))
                    .to_be_bytes(),
            );
            let decrypted = decrypt(
                SymmetricCipher::aes_128_cbc(),
                &material.key,
                &iv,
                &packet[12..],
                None,
            );
            assert_eq!(decrypted, plaintext);
        }
    }

    fn decrypt_video_packet(packet: &[u8]) -> Vec<u8> {
        decrypt(
            SymmetricCipher::aes_128_gcm(),
            &material().key,
            &packet[..12],
            &packet[VIDEO_ENCRYPTION_PREFIX_LEN..],
            Some(&packet[16..32]),
        )
    }

    fn decrypt(
        cipher: SymmetricCipher,
        key: &[u8],
        iv: &[u8],
        ciphertext: &[u8],
        tag: Option<&[u8]>,
    ) -> Vec<u8> {
        let mut crypter = Crypter::new(cipher, Mode::Decrypt, key, Some(iv)).unwrap();
        if let Some(tag) = tag {
            crypter.pad(false);
            crypter.set_tag(tag).unwrap();
        }
        let mut plaintext = vec![0; ciphertext.len() + cipher.block_size()];
        let mut written = crypter.update(ciphertext, &mut plaintext).unwrap();
        written += crypter.finalize(&mut plaintext[written..]).unwrap();
        plaintext.truncate(written);
        plaintext
    }
}
