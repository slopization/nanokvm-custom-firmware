use std::time::Duration;

use moonlink_core::{InputContext, InputSink, MediaSessionId, ServerError, VideoControlMessage};
use moonlink_protocol::{
    ENCRYPTED_CONTROL_PACKET_TYPE, START_ACK_PACKET_TYPE, is_input_data_packet_type,
    is_invalidate_reference_packet_type, is_request_idr_packet_type, parse_input_packet,
};
use openssl::symm::{Cipher, Crypter, Mode};
use tokio::sync::{broadcast, mpsc, watch};
use tokio_enet::{
    Event as EnetEvent, Host as EnetHost, Packet as EnetPacket, PacketMode as EnetPacketMode,
};

use crate::server_controller::ControlCommand;
use crate::shared::{ControlCrypto, ControlCryptoState, ControlEncryptionMode};

const AES_GCM_TAG_LEN: usize = 16;

pub(crate) async fn serve_control(
    mut host: EnetHost,
    input_sink: std::sync::Arc<impl InputSink>,
    control_crypto: ControlCrypto,
    video_control: broadcast::Sender<VideoControlMessage>,
    stream_stop: broadcast::Sender<()>,
    mut commands: mpsc::Receiver<ControlCommand>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), ServerError> {
    let mut decode_failures = DecodeFailureLogLimiter::default();
    loop {
        tokio::select! {
            result = host.service(Duration::from_millis(20)) => {
                if let Some(event) = result.map_err(|err| ServerError::InvalidConfig(err.to_string()))? {
                    if let Err(err) = process_control_event(&mut host, input_sink.as_ref(), &control_crypto, &video_control, &stream_stop, &mut decode_failures, event).await {
                        eprintln!("control event failed: {err}");
                    }
                }
            }
            command = commands.recv() => {
                match command {
                    Some(ControlCommand::DisconnectClients) => {
                        let peers = host.peers().map(|peer| peer.id()).collect::<Vec<_>>();
                        for peer in peers {
                            host.disconnect_now(peer, 0);
                        }
                    }
                    None => return Ok(()),
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

async fn process_control_event<I>(
    host: &mut EnetHost,
    input_sink: &I,
    control_crypto: &ControlCrypto,
    video_control: &broadcast::Sender<VideoControlMessage>,
    stream_stop: &broadcast::Sender<()>,
    decode_failures: &mut DecodeFailureLogLimiter,
    event: EnetEvent,
) -> Result<(), ServerError>
where
    I: InputSink,
{
    match event {
        EnetEvent::Connect { peer_id, .. } => {
            eprintln!("control peer connected: {peer_id:?}");
            send_control_packet(host, control_crypto, peer_id, 0, START_ACK_PACKET_TYPE, &[])
                .await?;
        }
        EnetEvent::Receive {
            peer_id,
            channel_id,
            packet,
        } => {
            process_control_packet(
                host,
                input_sink,
                control_crypto,
                video_control,
                peer_id,
                channel_id,
                packet,
                decode_failures,
            )
            .await?;
        }
        EnetEvent::Disconnect { peer_id, .. } => {
            eprintln!("control peer disconnected: {peer_id:?}");
            let _ = stream_stop.send(());
        }
    }
    Ok(())
}

async fn process_control_packet<I>(
    host: &mut EnetHost,
    input_sink: &I,
    control_crypto: &ControlCrypto,
    video_control: &broadcast::Sender<VideoControlMessage>,
    peer_id: tokio_enet::PeerId,
    channel_id: u8,
    packet: EnetPacket,
    decode_failures: &mut DecodeFailureLogLimiter,
) -> Result<(), ServerError>
where
    I: InputSink,
{
    let control_packet = match decode_control_packet(packet.data(), control_crypto).await {
        Ok(packet) => packet,
        Err(err) => {
            if decode_failures.record() {
                eprintln!(
                    "failed to decode control packet from {peer_id:?} channel={channel_id} len={} count={}: {err}",
                    packet.data().len(),
                    decode_failures.count()
                );
            }
            return Ok(());
        }
    };
    if let Some(message) = parse_video_control_message(&control_packet) {
        let _ = video_control.send(message);
    }
    let packet_type = u16::from_le_bytes(
        control_packet
            .get(..2)
            .ok_or_else(|| ServerError::InvalidConfig("control packet is too short".to_owned()))?
            .try_into()
            .expect("slice length"),
    );
    let payload = control_packet.get(2..).unwrap_or_default();
    if is_input_data_packet_type(packet_type) {
        match parse_input_packet(payload) {
            Ok(event) => {
                input_sink
                    .on_input(
                        InputContext {
                            client_id: format!("{peer_id:?}"),
                            session_id: MediaSessionId::new(),
                        },
                        event,
                    )
                    .await;
            }
            Err(err) => eprintln!("failed to parse input control packet: {err}"),
        }
    }
    send_control_packet(
        host,
        control_crypto,
        peer_id,
        channel_id,
        packet_type,
        payload,
    )
    .await?;
    Ok(())
}

#[derive(Debug, Default)]
struct DecodeFailureLogLimiter {
    count: u64,
}

impl DecodeFailureLogLimiter {
    fn record(&mut self) -> bool {
        self.count = self.count.saturating_add(1);
        self.count <= 5 || self.count.is_power_of_two()
    }

    fn count(&self) -> u64 {
        self.count
    }
}

async fn decode_control_packet(
    data: &[u8],
    control_crypto: &ControlCrypto,
) -> Result<Vec<u8>, ServerError> {
    if u16::from_le_bytes(
        data.get(..2)
            .ok_or_else(|| ServerError::InvalidConfig("control packet is too short".to_owned()))?
            .try_into()
            .expect("slice length"),
    ) != ENCRYPTED_CONTROL_PACKET_TYPE
    {
        return Ok(data.to_vec());
    }

    let mut crypto = control_crypto.lock().await;
    let crypto = crypto.as_mut().ok_or_else(|| {
        ServerError::InvalidConfig(
            "received encrypted control packet before launch rikey".to_owned(),
        )
    })?;
    decrypt_control_packet(data, crypto)
}

fn decrypt_control_packet(
    data: &[u8],
    crypto: &mut ControlCryptoState,
) -> Result<Vec<u8>, ServerError> {
    if data.len() < 8 + AES_GCM_TAG_LEN {
        return Err(ServerError::InvalidConfig(
            "encrypted control packet is too short".to_owned(),
        ));
    }
    let length = u16::from_le_bytes(data[2..4].try_into().expect("slice length")) as usize;
    if data.len() != length + 4 {
        return Err(ServerError::InvalidConfig(
            "encrypted control packet length mismatch".to_owned(),
        ));
    }
    if length < 4 + AES_GCM_TAG_LEN + 4 {
        return Err(ServerError::InvalidConfig(
            "encrypted control packet payload is too short".to_owned(),
        ));
    }

    let sequence = u32::from_le_bytes(data[4..8].try_into().expect("slice length"));
    let tag = &data[8..8 + AES_GCM_TAG_LEN];
    let ciphertext = &data[8 + AES_GCM_TAG_LEN..];
    let plaintext = match decrypt_aes_gcm(
        &crypto.material.key,
        &control_v2_iv(sequence, b'C', b'C'),
        tag,
        ciphertext,
    ) {
        Ok(plaintext) => {
            crypto.mode = ControlEncryptionMode::New;
            plaintext
        }
        Err(new_err) => match decrypt_aes_gcm(
            &crypto.material.key,
            &control_v1_iv(sequence),
            tag,
            ciphertext,
        ) {
            Ok(plaintext) => {
                crypto.mode = ControlEncryptionMode::Old;
                plaintext
            }
            Err(_) => return Err(new_err),
        },
    };
    if plaintext.len() < 4 {
        return Err(ServerError::InvalidConfig(
            "decrypted control packet is too short".to_owned(),
        ));
    }
    let payload_len =
        u16::from_le_bytes(plaintext[2..4].try_into().expect("slice length")) as usize;
    if plaintext.len() != 4 + payload_len {
        return Err(ServerError::InvalidConfig(
            "decrypted control packet payload length mismatch".to_owned(),
        ));
    }
    let mut v1 = Vec::with_capacity(2 + payload_len);
    v1.extend_from_slice(&plaintext[..2]);
    v1.extend_from_slice(&plaintext[4..]);
    Ok(v1)
}

async fn send_control_packet(
    host: &mut EnetHost,
    control_crypto: &ControlCrypto,
    peer_id: tokio_enet::PeerId,
    channel_id: u8,
    packet_type: u16,
    payload: &[u8],
) -> Result<(), ServerError> {
    let Some(peer) = host.peer_mut(peer_id) else {
        return Ok(());
    };
    let data = {
        let mut guard = control_crypto.lock().await;
        match guard.as_mut() {
            Some(crypto) => encrypt_control_packet(packet_type, payload, crypto)?,
            None => encode_plain_control_packet(packet_type, payload),
        }
    };
    peer.send(
        channel_id,
        EnetPacket::new(&data, EnetPacketMode::ReliableSequenced),
    )
    .map_err(|err| ServerError::InvalidConfig(err.to_string()))
}

fn encode_plain_control_packet(packet_type: u16, payload: &[u8]) -> Vec<u8> {
    let mut packet = Vec::with_capacity(2 + payload.len());
    packet.extend_from_slice(&packet_type.to_le_bytes());
    packet.extend_from_slice(payload);
    packet
}

fn encrypt_control_packet(
    packet_type: u16,
    payload: &[u8],
    crypto: &mut ControlCryptoState,
) -> Result<Vec<u8>, ServerError> {
    let payload_len = u16::try_from(payload.len())
        .map_err(|_| ServerError::InvalidConfig("control payload is too large".to_owned()))?;
    let sequence = crypto.next_host_sequence;
    crypto.next_host_sequence = crypto.next_host_sequence.wrapping_add(1);

    let mut plaintext = Vec::with_capacity(4 + payload.len());
    plaintext.extend_from_slice(&packet_type.to_le_bytes());
    plaintext.extend_from_slice(&payload_len.to_le_bytes());
    plaintext.extend_from_slice(payload);

    let (tag, ciphertext) = encrypt_aes_gcm(
        &crypto.material.key,
        &control_iv(sequence, b'H', b'C', crypto.mode),
        &plaintext,
    )?;
    let length = u16::try_from(4 + AES_GCM_TAG_LEN + ciphertext.len()).map_err(|_| {
        ServerError::InvalidConfig("encrypted control packet is too large".to_owned())
    })?;
    let mut packet = Vec::with_capacity(4 + length as usize);
    packet.extend_from_slice(&ENCRYPTED_CONTROL_PACKET_TYPE.to_le_bytes());
    packet.extend_from_slice(&length.to_le_bytes());
    packet.extend_from_slice(&sequence.to_le_bytes());
    packet.extend_from_slice(&tag);
    packet.extend_from_slice(&ciphertext);
    Ok(packet)
}

fn control_iv(sequence: u32, origin: u8, stream: u8, mode: ControlEncryptionMode) -> Vec<u8> {
    match mode {
        ControlEncryptionMode::New => control_v2_iv(sequence, origin, stream).to_vec(),
        ControlEncryptionMode::Old => control_v1_iv(sequence).to_vec(),
    }
}

fn control_v2_iv(sequence: u32, origin: u8, stream: u8) -> [u8; 12] {
    let mut iv = [0_u8; 12];
    iv[..4].copy_from_slice(&sequence.to_le_bytes());
    iv[10] = origin;
    iv[11] = stream;
    iv
}

fn control_v1_iv(sequence: u32) -> [u8; 16] {
    let mut iv = [0_u8; 16];
    iv[0] = sequence as u8;
    iv
}

fn decrypt_aes_gcm(
    key: &[u8; 16],
    iv: &[u8],
    tag: &[u8],
    ciphertext: &[u8],
) -> Result<Vec<u8>, ServerError> {
    let mut crypter = Crypter::new(Cipher::aes_128_gcm(), Mode::Decrypt, key, Some(iv))
        .map_err(|err| ServerError::InvalidConfig(err.to_string()))?;
    crypter.pad(false);
    crypter
        .set_tag(tag)
        .map_err(|err| ServerError::InvalidConfig(err.to_string()))?;
    let mut plaintext = vec![0_u8; ciphertext.len() + Cipher::aes_128_gcm().block_size()];
    let mut count = crypter
        .update(ciphertext, &mut plaintext)
        .map_err(|err| ServerError::InvalidConfig(err.to_string()))?;
    count += crypter
        .finalize(&mut plaintext[count..])
        .map_err(|err| ServerError::InvalidConfig(err.to_string()))?;
    plaintext.truncate(count);
    Ok(plaintext)
}

fn encrypt_aes_gcm(
    key: &[u8; 16],
    iv: &[u8],
    plaintext: &[u8],
) -> Result<([u8; AES_GCM_TAG_LEN], Vec<u8>), ServerError> {
    let mut crypter = Crypter::new(Cipher::aes_128_gcm(), Mode::Encrypt, key, Some(iv))
        .map_err(|err| ServerError::InvalidConfig(err.to_string()))?;
    crypter.pad(false);
    let mut ciphertext = vec![0_u8; plaintext.len() + Cipher::aes_128_gcm().block_size()];
    let mut count = crypter
        .update(plaintext, &mut ciphertext)
        .map_err(|err| ServerError::InvalidConfig(err.to_string()))?;
    count += crypter
        .finalize(&mut ciphertext[count..])
        .map_err(|err| ServerError::InvalidConfig(err.to_string()))?;
    ciphertext.truncate(count);
    let mut tag = [0_u8; AES_GCM_TAG_LEN];
    crypter
        .get_tag(&mut tag)
        .map_err(|err| ServerError::InvalidConfig(err.to_string()))?;
    Ok((tag, ciphertext))
}

fn parse_video_control_message(data: &[u8]) -> Option<VideoControlMessage> {
    let packet_type = u16::from_le_bytes(data.get(..2)?.try_into().ok()?);
    match packet_type {
        packet_type if is_request_idr_packet_type(packet_type) => {
            Some(VideoControlMessage::RequestIdrFrame)
        }
        packet_type if is_invalidate_reference_packet_type(packet_type) => {
            let payload = data.get(2..)?;
            let first_frame = read_u32_le(payload, 0).or_else(|| read_u64_low_le(payload, 0))?;
            let last_frame = read_u32_le(payload, 8)
                .or_else(|| read_u64_low_le(payload, 8))
                .unwrap_or(first_frame);
            Some(VideoControlMessage::InvalidateReferenceFrames {
                first_frame,
                last_frame,
            })
        }
        _ => None,
    }
}

fn read_u32_le(data: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        data.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn read_u64_low_le(data: &[u8], offset: usize) -> Option<u32> {
    Some(u64::from_le_bytes(data.get(offset..offset + 8)?.try_into().ok()?) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_idr_request_packets() {
        assert_eq!(
            parse_video_control_message(&[0x02, 0x03, 0, 0]),
            Some(VideoControlMessage::RequestIdrFrame)
        );
        assert_eq!(
            parse_video_control_message(&[0x07, 0x14, 0, 0]),
            Some(VideoControlMessage::RequestIdrFrame)
        );
    }

    #[test]
    fn parses_reference_frame_invalidation_packets() {
        let mut packet = vec![0x01, 0x03];
        packet.extend_from_slice(&7_u32.to_le_bytes());
        packet.extend_from_slice(&0_u32.to_le_bytes());
        packet.extend_from_slice(&11_u32.to_le_bytes());
        packet.extend_from_slice(&[0; 12]);

        assert_eq!(
            parse_video_control_message(&packet),
            Some(VideoControlMessage::InvalidateReferenceFrames {
                first_frame: 7,
                last_frame: 11,
            })
        );
    }
}
