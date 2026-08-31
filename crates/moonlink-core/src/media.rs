use async_trait::async_trait;
use bytes::Bytes;
use moonlink_protocol::{
    AudioCodec, DEFAULT_VIDEO_FEC_PERCENTAGE, DEFAULT_VIDEO_MIN_PARITY_SHARDS,
    DEFAULT_VIDEO_PACKET_SIZE, VideoCodec,
};
use oxideav_core::{Packet, TimeBase};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MediaSessionId(pub Uuid);

impl MediaSessionId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for MediaSessionId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaSessionConfig {
    pub session_id: MediaSessionId,
    pub app_id: u32,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub video_codec: VideoCodec,
    pub audio_codec: Option<AudioCodec>,
    pub video_mode: VideoMode,
    pub video_packet_size: usize,
    pub video_fec_percentage: u8,
    pub video_min_parity_shards: usize,
    pub video_encrypted: bool,
    pub audio_encrypted: bool,
    pub audio_packet_duration_ms: u32,
}

impl MediaSessionConfig {
    pub fn video_packetizer_config(&self) -> moonlink_protocol::PacketizerConfig {
        moonlink_protocol::PacketizerConfig {
            max_payload_size: self.video_packet_size + moonlink_protocol::VIDEO_RTP_EXTENSION_LEN,
            fec_percentage: self.video_fec_percentage,
            min_parity_shards: self.video_min_parity_shards,
        }
    }
}

impl Default for MediaSessionConfig {
    fn default() -> Self {
        Self {
            session_id: MediaSessionId::default(),
            app_id: 1,
            width: 1280,
            height: 720,
            fps: 60,
            video_codec: VideoCodec::H264,
            audio_codec: Some(AudioCodec::Opus),
            video_mode: VideoMode::EncodedAccessUnits,
            video_packet_size: DEFAULT_VIDEO_PACKET_SIZE,
            video_fec_percentage: DEFAULT_VIDEO_FEC_PERCENTAGE,
            video_min_parity_shards: DEFAULT_VIDEO_MIN_PARITY_SHARDS,
            video_encrypted: false,
            audio_encrypted: false,
            audio_packet_duration_ms: 5,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoMode {
    EncodedAccessUnits,
    RtpPayloads,
    FullRtpPackets,
}

pub type AccessUnit = Packet;

pub fn create_access_unit(data: Vec<u8>, timestamp_90khz: u32, keyframe: bool) -> AccessUnit {
    Packet::new(0, TimeBase::from_rate(90_000), data)
        .with_pts(i64::from(timestamp_90khz))
        .with_dts(i64::from(timestamp_90khz))
        .with_keyframe(keyframe)
        .with_unit_boundary(true)
}

pub fn access_unit_timestamp_90khz(access_unit: &AccessUnit) -> u32 {
    access_unit.pts.unwrap_or_default() as u32
}

pub type AudioFrame = Packet;

pub fn create_audio_frame(data: Vec<u8>, timestamp_48khz: u32, duration_48khz: u32) -> AudioFrame {
    Packet::new(0, TimeBase::from_rate(48_000), data)
        .with_pts(i64::from(timestamp_48khz))
        .with_dts(i64::from(timestamp_48khz))
        .with_duration(i64::from(duration_48khz))
        .with_unit_boundary(true)
}

pub fn audio_frame_timestamp_48khz(frame: &AudioFrame) -> u32 {
    frame.pts.unwrap_or_default() as u32
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtpPayload {
    pub payload: Bytes,
    pub timestamp_90khz: u32,
    pub marker: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FullRtpPacket {
    pub bytes: Bytes,
}

#[derive(Debug, Clone)]
pub enum VideoFrame {
    AccessUnit(AccessUnit),
    RtpPayload(RtpPayload),
    FullRtpPacket(FullRtpPacket),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VideoControlMessage {
    RequestIdrFrame,
    InvalidateReferenceFrames { first_frame: u32, last_frame: u32 },
}

#[derive(Debug, Error)]
pub enum MediaSessionError {
    #[error("requested media mode does not match supplied frame")]
    ModeMismatch,
    #[error("media provider has no more frames")]
    EndOfStream,
    #[error("media provider failed: {0}")]
    Provider(String),
}

#[async_trait]
pub trait MediaProvider: Send + Sync + 'static {
    type Session: MediaSession;

    async fn start_session(
        &self,
        config: MediaSessionConfig,
    ) -> Result<Self::Session, MediaSessionError>;
}

#[async_trait]
pub trait MediaSession: Send + 'static {
    async fn next_video(&mut self) -> Result<Option<VideoFrame>, MediaSessionError>;

    fn take_audio(&mut self) -> Option<Box<dyn AudioSession>> {
        None
    }

    async fn next_audio(&mut self) -> Result<Option<AudioFrame>, MediaSessionError> {
        Ok(None)
    }

    async fn handle_video_control(
        &mut self,
        _message: VideoControlMessage,
    ) -> Result<(), MediaSessionError> {
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), MediaSessionError> {
        Ok(())
    }
}

#[async_trait]
pub trait AudioSession: Send + 'static {
    async fn next_audio(&mut self) -> Result<Option<AudioFrame>, MediaSessionError>;
}
