use std::net::SocketAddr;

use moonlink_protocol::{
    CodecCapabilities, DEFAULT_VIDEO_FEC_PERCENTAGE, DEFAULT_VIDEO_MIN_PARITY_SHARDS,
};
use thiserror::Error;

use crate::PairingPin;

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub name: String,
    pub bind_http: SocketAddr,
    pub bind_https: SocketAddr,
    pub bind_discovery: SocketAddr,
    pub bind_rtsp: SocketAddr,
    pub bind_video: SocketAddr,
    pub bind_audio: SocketAddr,
    pub bind_control: SocketAddr,
    pub apps: Vec<App>,
    pub capabilities: CodecCapabilities,
    pub input_backpressure: BackpressurePolicy,
    pub pairing_pin: PairingPin,
    pub app_version: String,
    pub video_stream: VideoStream,
}

impl ServerConfig {
    pub fn new(
        name: impl Into<String>,
        bind_http: SocketAddr,
        bind_discovery: SocketAddr,
        bind_rtsp: SocketAddr,
        capabilities: CodecCapabilities,
    ) -> Self {
        Self {
            name: name.into(),
            bind_http,
            bind_https: ephemeral_same_host(bind_http),
            bind_discovery,
            bind_rtsp,
            bind_video: "0.0.0.0:47998".parse().expect("static socket address"),
            bind_audio: "0.0.0.0:48000".parse().expect("static socket address"),
            bind_control: "0.0.0.0:47999".parse().expect("static socket address"),
            apps: vec![App::desktop()],
            capabilities,
            input_backpressure: BackpressurePolicy::DropOldestMotion,
            pairing_pin: PairingPin::static_pin("0000"),
            app_version: "9.1.0.0".to_owned(),
            video_stream: VideoStream::default(),
        }
    }

    pub fn with_https_bind(mut self, https: SocketAddr) -> Self {
        self.bind_https = https;
        self
    }

    pub fn with_stream_ports(
        mut self,
        video: SocketAddr,
        audio: SocketAddr,
        control: SocketAddr,
    ) -> Self {
        self.bind_video = video;
        self.bind_audio = audio;
        self.bind_control = control;
        self
    }

    pub fn with_video_stream(mut self, video_stream: VideoStream) -> Self {
        self.video_stream = video_stream;
        self
    }
}

fn ephemeral_same_host(addr: SocketAddr) -> SocketAddr {
    SocketAddr::new(addr.ip(), 0)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct App {
    pub id: u32,
    pub name: String,
}

impl App {
    pub fn desktop() -> Self {
        Self {
            id: 1,
            name: "Desktop".to_owned(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackpressurePolicy {
    DropNewest,
    DropOldestMotion,
    Disconnect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoStream {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub fec_percentage: u8,
    pub min_parity_shards: usize,
}

impl VideoStream {
    pub fn new(width: u32, height: u32, fps: u32) -> Self {
        Self {
            width,
            height,
            fps,
            fec_percentage: DEFAULT_VIDEO_FEC_PERCENTAGE,
            min_parity_shards: DEFAULT_VIDEO_MIN_PARITY_SHARDS,
        }
    }

    pub fn with_fec(mut self, percentage: u8, min_parity_shards: usize) -> Self {
        self.fec_percentage = percentage;
        self.min_parity_shards = min_parity_shards;
        self
    }

    pub fn max_luma_pixels(self) -> u32 {
        self.width.saturating_mul(self.height)
    }
}

impl Default for VideoStream {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 720,
            fps: 60,
            fec_percentage: DEFAULT_VIDEO_FEC_PERCENTAGE,
            min_parity_shards: DEFAULT_VIDEO_MIN_PARITY_SHARDS,
        }
    }
}

#[derive(Debug, Error)]
pub enum ServerError {
    #[error("server configuration is invalid: {0}")]
    InvalidConfig(String),
    #[error("pairing failed: {0}")]
    Pairing(#[from] crate::pairing::PairingError),
    #[error("media session failed: {0}")]
    Media(#[from] crate::media::MediaSessionError),
    #[error("I/O failed: {0}")]
    Io(#[from] std::io::Error),
}
