#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VideoCodec {
    H264,
    Hevc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AudioCodec {
    Opus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioPacketizerState {
    pub sequence: u16,
    pub ssrc: u32,
}

impl Default for AudioPacketizerState {
    fn default() -> Self {
        Self {
            sequence: 0,
            ssrc: 0,
        }
    }
}

pub fn packetize_opus(
    state: &mut AudioPacketizerState,
    opus: &[u8],
    timestamp_48khz: u32,
) -> Vec<u8> {
    const RTP_VERSION_2: u8 = 0x80;
    const OPUS_PAYLOAD_TYPE: u8 = 97;

    let mut packet = Vec::with_capacity(12 + opus.len());
    packet.push(RTP_VERSION_2);
    packet.push(OPUS_PAYLOAD_TYPE);
    packet.extend_from_slice(&state.sequence.to_be_bytes());
    packet.extend_from_slice(&timestamp_48khz.to_be_bytes());
    packet.extend_from_slice(&state.ssrc.to_be_bytes());
    packet.extend_from_slice(opus);
    state.sequence = state.sequence.wrapping_add(1);
    packet
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MediaDirection {
    SendOnly,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodecCapabilities {
    pub video: Vec<VideoCodec>,
    pub audio: Vec<AudioCodec>,
}

impl CodecCapabilities {
    pub fn video_only(video: impl Into<Vec<VideoCodec>>) -> Self {
        Self {
            video: video.into(),
            audio: Vec::new(),
        }
    }

    pub fn with_opus(video: impl Into<Vec<VideoCodec>>) -> Self {
        Self {
            video: video.into(),
            audio: vec![AudioCodec::Opus],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packetizes_opus_as_rtp_payload_type_97() {
        let mut state = AudioPacketizerState {
            sequence: 0xffff,
            ssrc: 0x1234_5678,
        };

        let first = packetize_opus(&mut state, &[0xf8, 0xff, 0xfe], 960);
        assert_eq!(&first[..2], &[0x80, 97]);
        assert_eq!(&first[2..4], &[0xff, 0xff]);
        assert_eq!(&first[4..8], &960_u32.to_be_bytes());
        assert_eq!(&first[8..12], &0x1234_5678_u32.to_be_bytes());
        assert_eq!(&first[12..], &[0xf8, 0xff, 0xfe]);

        let second = packetize_opus(&mut state, &[0xf8, 0xff, 0xfe], 1920);
        assert_eq!(&second[2..4], &[0, 0]);
        assert_eq!(state.sequence, 1);
    }
}
