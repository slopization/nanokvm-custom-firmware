use moonlink_protocol::CodecCapabilities;

use crate::{ServerConfig, VideoStream};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerRuntimeConfig {
    pub name: String,
    pub capabilities: CodecCapabilities,
    pub video_stream: VideoStream,
}

impl From<&ServerConfig> for ServerRuntimeConfig {
    fn from(config: &ServerConfig) -> Self {
        Self {
            name: config.name.clone(),
            capabilities: config.capabilities.clone(),
            video_stream: config.video_stream,
        }
    }
}
