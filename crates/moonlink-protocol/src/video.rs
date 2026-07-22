use std::sync::OnceLock;

use thiserror::Error;

pub const RTP_HEADER_EXTENSION_FLAG: u8 = 0x10;
pub const RTP_MARKER_FLAG: u8 = 0x80;
pub const RTP_VERSION_2: u8 = 0x80;
pub const VIDEO_PAYLOAD_TYPE: u8 = 96;
pub const FLAG_CONTAINS_PIC_DATA: u8 = 0x01;
pub const FLAG_EOF: u8 = 0x02;
pub const FLAG_SOF: u8 = 0x04;
pub const VIDEO_RTP_HEADER_LEN: usize = 12;
pub const VIDEO_RTP_EXTENSION_LEN: usize = 4;
pub const NV_VIDEO_PACKET_LEN: usize = 16;
pub const VIDEO_PACKET_HEADER_LEN: usize =
    VIDEO_RTP_HEADER_LEN + VIDEO_RTP_EXTENSION_LEN + NV_VIDEO_PACKET_LEN;
pub const DEFAULT_VIDEO_PACKET_SIZE: usize = 1184;
pub const DEFAULT_VIDEO_DATAGRAM_SIZE: usize = DEFAULT_VIDEO_PACKET_SIZE + VIDEO_RTP_EXTENSION_LEN;
pub const DEFAULT_VIDEO_FEC_PERCENTAGE: u8 = 20;
pub const DEFAULT_VIDEO_MIN_PARITY_SHARDS: usize = 2;
const SHORT_FRAME_HEADER_LEN: usize = 8;
const MIN_MEDIA_BYTES: usize = 32;
const MAX_RS_SHARDS: usize = 255;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PacketizedVideoFrame {
    pub packets: Vec<Vec<u8>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PacketizerState {
    pub sequence: u16,
    pub frame_index: u32,
    pub stream_packet_index: u32,
    pub ssrc: u32,
}

impl Default for PacketizerState {
    fn default() -> Self {
        Self {
            sequence: 1,
            frame_index: 1,
            stream_packet_index: 1,
            ssrc: 0x4d4c_0001,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PacketizerConfig {
    pub max_payload_size: usize,
    pub fec_percentage: u8,
    pub min_parity_shards: usize,
}

impl Default for PacketizerConfig {
    fn default() -> Self {
        Self {
            max_payload_size: DEFAULT_VIDEO_DATAGRAM_SIZE,
            fec_percentage: DEFAULT_VIDEO_FEC_PERCENTAGE,
            min_parity_shards: DEFAULT_VIDEO_MIN_PARITY_SHARDS,
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum PacketizeError {
    #[error("max payload size is too small")]
    MaxPayloadTooSmall,
    #[error("video frame requires too many FEC shards")]
    TooManyShards,
    #[error("reed-solomon FEC failed: {0}")]
    Fec(String),
}

pub fn packetize_access_unit(
    state: &mut PacketizerState,
    config: PacketizerConfig,
    access_unit: &[u8],
    timestamp_90khz: u32,
    idr: bool,
) -> Result<PacketizedVideoFrame, PacketizeError> {
    if config.max_payload_size <= VIDEO_PACKET_HEADER_LEN + SHORT_FRAME_HEADER_LEN + MIN_MEDIA_BYTES
    {
        return Err(PacketizeError::MaxPayloadTooSmall);
    }

    let frame_len = SHORT_FRAME_HEADER_LEN + access_unit.len();
    let media_capacity = config.max_payload_size - VIDEO_PACKET_HEADER_LEN;
    let last_payload_len = match frame_len % media_capacity {
        0 => media_capacity,
        len => len,
    } as u16;

    let mut frame = Vec::with_capacity(frame_len);
    frame.push(0x01);
    frame.extend_from_slice(&0_u16.to_le_bytes());
    frame.push(if idr { 2 } else { 1 });
    frame.extend_from_slice(&last_payload_len.to_le_bytes());
    frame.extend_from_slice(&[0, 0]);
    frame.extend_from_slice(access_unit);

    let data_shards = frame.len().div_ceil(media_capacity);
    if data_shards > 0x3ff {
        return Err(PacketizeError::TooManyShards);
    }
    let fec_percentage = if data_shards > MAX_RS_SHARDS {
        0
    } else {
        fec_percentage_for_frame(data_shards, config.fec_percentage, config.min_parity_shards)
    };
    let parity_shards = parity_shard_count(data_shards, fec_percentage);

    let mut shards = Vec::with_capacity(data_shards + parity_shards);
    for index in 0..data_shards {
        let mut shard = vec![0; config.max_payload_size];
        let start = index * media_capacity;
        let end = (start + media_capacity).min(frame.len());
        shard[VIDEO_PACKET_HEADER_LEN..VIDEO_PACKET_HEADER_LEN + end - start]
            .copy_from_slice(&frame[start..end]);
        write_data_video_header(
            &mut shard,
            state,
            index,
            data_shards,
            timestamp_90khz,
            fec_percentage,
        );
        shards.push(shard);
    }

    for _ in 0..parity_shards {
        shards.push(vec![0; config.max_payload_size]);
    }

    if parity_shards > 0 {
        encode_nanors_parity(
            &mut shards,
            data_shards,
            parity_shards,
            config.max_payload_size,
        );
    }

    for (index, shard) in shards.iter_mut().enumerate() {
        write_common_video_header(
            shard,
            state,
            index,
            timestamp_90khz,
            data_shards,
            fec_percentage,
        );
    }

    state.sequence = state.sequence.wrapping_add(shards.len() as u16);
    state.stream_packet_index = state.stream_packet_index.wrapping_add(data_shards as u32);
    state.frame_index = state.frame_index.wrapping_add(1);
    Ok(PacketizedVideoFrame { packets: shards })
}

fn write_data_video_header(
    packet: &mut [u8],
    state: &PacketizerState,
    index: usize,
    data_shards: usize,
    timestamp_90khz: u32,
    fec_percentage: u8,
) {
    write_common_video_header(
        packet,
        state,
        index,
        timestamp_90khz,
        data_shards,
        fec_percentage,
    );

    let nv = VIDEO_RTP_HEADER_LEN + VIDEO_RTP_EXTENSION_LEN;
    let mut flags = FLAG_CONTAINS_PIC_DATA;
    if index == 0 {
        flags |= FLAG_SOF;
    }
    if index == data_shards.saturating_sub(1) {
        flags |= FLAG_EOF;
    }

    packet[nv..nv + 4].copy_from_slice(
        &(state.stream_packet_index.wrapping_add(index as u32) << 8).to_le_bytes(),
    );
    packet[nv + 8] = flags;
    packet[nv + 9] = 0;
}

fn write_common_video_header(
    packet: &mut [u8],
    state: &PacketizerState,
    index: usize,
    timestamp_90khz: u32,
    data_shards: usize,
    fec_percentage: u8,
) {
    packet[0] = RTP_VERSION_2 | RTP_HEADER_EXTENSION_FLAG;
    packet[1] = VIDEO_PAYLOAD_TYPE;
    packet[2..4].copy_from_slice(&state.sequence.wrapping_add(index as u16).to_be_bytes());
    packet[4..8].copy_from_slice(&timestamp_90khz.to_be_bytes());
    packet[8..12].copy_from_slice(&state.ssrc.to_be_bytes());
    packet[12..16].copy_from_slice(&[0, 0, 0, 0]);

    let nv = VIDEO_RTP_HEADER_LEN + VIDEO_RTP_EXTENSION_LEN;
    packet[nv + 4..nv + 8].copy_from_slice(&state.frame_index.to_le_bytes());
    packet[nv + 10] = 0x10;
    packet[nv + 11] = 0;
    packet[nv + 12..nv + 16]
        .copy_from_slice(&fec_info(index as u32, data_shards as u32, fec_percentage).to_le_bytes());
}

fn fec_info(index: u32, data_shards: u32, fec_percentage: u8) -> u32 {
    let data_shards = data_shards.clamp(1, 0x3ff);
    ((fec_percentage as u32) << 4) | ((index & 0x3ff) << 12) | ((data_shards & 0x3ff) << 22)
}

fn parity_shard_count(data_shards: usize, fec_percentage: u8) -> usize {
    (data_shards * fec_percentage as usize).div_ceil(100)
}

fn fec_percentage_for_frame(
    data_shards: usize,
    fec_percentage: u8,
    min_parity_shards: usize,
) -> u8 {
    if fec_percentage == 0 {
        return 0;
    }
    let mut fec_percentage = fec_percentage;
    let max_parity_shards = MAX_RS_SHARDS.saturating_sub(data_shards);
    while parity_shard_count(data_shards, fec_percentage) > max_parity_shards {
        fec_percentage = fec_percentage.saturating_sub(1);
    }

    let parity_shards = parity_shard_count(data_shards, fec_percentage);
    if parity_shards >= min_parity_shards {
        return fec_percentage;
    }
    let min_percentage = ((min_parity_shards * 100) / data_shards).clamp(1, u8::MAX as usize) as u8;
    if parity_shard_count(data_shards, min_percentage) <= max_parity_shards {
        return min_percentage;
    }
    fec_percentage
}

fn encode_nanors_parity(
    shards: &mut [Vec<u8>],
    data_shards: usize,
    parity_shards: usize,
    shard_len: usize,
) {
    let table = gf_mul_table();
    for parity_index in 0..parity_shards {
        let (data, parity) = shards.split_at_mut(data_shards);
        for (data_index, source) in data.iter().enumerate() {
            let coefficient = gf_inv((parity_shards + data_index) as u8 ^ parity_index as u8);
            xor_scaled(
                &mut parity[parity_index],
                source,
                coefficient,
                shard_len,
                table,
            );
        }
    }
}

fn xor_scaled(target: &mut [u8], source: &[u8], coefficient: u8, len: usize, table: &[u8; 65536]) {
    if coefficient == 0 {
        return;
    }
    if coefficient == 1 {
        for index in 0..len {
            target[index] ^= source[index];
        }
        return;
    }
    let row = coefficient as usize * 256;
    for index in 0..len {
        target[index] ^= table[row + source[index] as usize];
    }
}

fn gf_mul_table() -> &'static [u8; 65536] {
    static TABLE: OnceLock<[u8; 65536]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = [0; 65536];
        for a in 0..=255 {
            for b in 0..=255 {
                table[a * 256 + b] = gf_mul_slow(a as u8, b as u8);
            }
        }
        table
    })
}

fn gf_inv(value: u8) -> u8 {
    debug_assert_ne!(value, 0);
    gf_pow(value, 254)
}

fn gf_pow(mut value: u8, mut exponent: u16) -> u8 {
    let mut result = 1;
    while exponent != 0 {
        if exponent & 1 == 1 {
            result = gf_mul_slow(result, value);
        }
        value = gf_mul_slow(value, value);
        exponent >>= 1;
    }
    result
}

fn gf_mul_slow(mut a: u8, mut b: u8) -> u8 {
    let mut product = 0;
    while b != 0 {
        if b & 1 != 0 {
            product ^= a;
        }
        let carry = a & 0x80 != 0;
        a <<= 1;
        if carry {
            a ^= 0x1d;
        }
        b >>= 1;
    }
    product
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packetizes_with_gamestream_headers() {
        let mut state = PacketizerState::default();
        let frame = packetize_access_unit(
            &mut state,
            PacketizerConfig::default(),
            &[0, 0, 0, 1, 0x65, 1, 2, 3],
            90_000,
            true,
        )
        .unwrap();

        assert_eq!(frame.packets.len(), 3);
        assert_eq!(
            frame.packets[0][0],
            RTP_VERSION_2 | RTP_HEADER_EXTENSION_FLAG
        );
        assert_eq!(frame.packets[0][1], VIDEO_PAYLOAD_TYPE);
        assert_eq!(
            frame.packets[0][16 + 8],
            FLAG_CONTAINS_PIC_DATA | FLAG_SOF | FLAG_EOF
        );
    }

    #[test]
    fn appends_reed_solomon_parity_shards() {
        let mut state = PacketizerState::default();
        let config = PacketizerConfig {
            min_parity_shards: 2,
            ..PacketizerConfig::default()
        };
        let frame = packetize_access_unit(&mut state, config, &[1; 2400], 90_000, false).unwrap();

        let data_shards = (SHORT_FRAME_HEADER_LEN + 2400)
            .div_ceil(config.max_payload_size - VIDEO_PACKET_HEADER_LEN);
        assert_eq!(frame.packets.len(), data_shards + 2);
        for (index, packet) in frame.packets.iter().enumerate() {
            let fec = u32::from_le_bytes(packet[28..32].try_into().unwrap());
            assert_eq!((fec & 0x3ff000) >> 12, index as u32);
            assert_eq!((fec & 0xff0) >> 4, 66);
            assert_eq!((fec & 0xffc00000) >> 22, data_shards as u32);
        }
    }

    #[test]
    fn reduces_fec_percentage_for_large_frames() {
        let mut state = PacketizerState::default();
        let config = PacketizerConfig {
            max_payload_size: 1392 + VIDEO_RTP_EXTENSION_LEN,
            fec_percentage: 20,
            min_parity_shards: 2,
        };
        let access_unit = vec![1; 303_437];
        let frame = packetize_access_unit(&mut state, config, &access_unit, 90_000, true).unwrap();
        let data_shards = (SHORT_FRAME_HEADER_LEN + access_unit.len())
            .div_ceil(config.max_payload_size - VIDEO_PACKET_HEADER_LEN);

        assert!(frame.packets.len() <= MAX_RS_SHARDS);
        assert!(frame.packets.len() > data_shards);
        let fec = u32::from_le_bytes(frame.packets[0][28..32].try_into().unwrap());
        let fec_percentage = ((fec & 0xff0) >> 4) as u8;
        assert_eq!(
            frame.packets.len(),
            data_shards + parity_shard_count(data_shards, fec_percentage)
        );
        assert!(fec_percentage < config.fec_percentage);
    }

    #[test]
    fn disables_fec_when_frame_exceeds_reed_solomon_limit() {
        let mut state = PacketizerState::default();
        let config = PacketizerConfig {
            max_payload_size: 1392 + VIDEO_RTP_EXTENSION_LEN,
            fec_percentage: 20,
            min_parity_shards: 2,
        };
        let access_unit = vec![1; 360_000];
        let frame = packetize_access_unit(&mut state, config, &access_unit, 90_000, true).unwrap();
        let data_shards = (SHORT_FRAME_HEADER_LEN + access_unit.len())
            .div_ceil(config.max_payload_size - VIDEO_PACKET_HEADER_LEN);

        assert!(data_shards > MAX_RS_SHARDS);
        assert_eq!(frame.packets.len(), data_shards);
        let fec = u32::from_le_bytes(frame.packets[0][28..32].try_into().unwrap());
        assert_eq!((fec & 0xff0) >> 4, 0);
        assert_eq!((fec & 0xffc00000) >> 22, data_shards as u32);
    }
}
