use std::fs::{self, File};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use moonlink_core::{ServerRuntimeConfig, VideoStream};
use moonlink_protocol::{CodecCapabilities, VideoCodec};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::cli::{Args, AudioSettings, AudioSource, StreamCodec, StreamEncryptionMode};
use crate::v4l2_provider::V4l2Settings;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MoonlinkSettings {
    pub name: String,
    pub codec: StreamCodec,
    pub audio: AudioSource,
    pub opus_bitrate_kbps: u32,
    pub opus_complexity: u8,
    pub capture_width: u32,
    pub capture_height: u32,
    pub advertised_fps: u32,
    pub frame_divider: u32,
    pub bitrate_kbps: Option<u32>,
    pub gop: u32,
    pub video_fec_percentage: u8,
    pub video_min_parity_shards: usize,
    #[serde(default)]
    pub encryption_mode: StreamEncryptionMode,
    pub hevc_loop_filter: Option<i32>,
    pub repeat_headers: bool,
    pub aud: bool,
}

impl MoonlinkSettings {
    pub fn changes_stream(&self, previous: &Self) -> bool {
        let mut current = self.clone();
        let mut previous = previous.clone();
        current.name.clear();
        previous.name.clear();
        current != previous
    }

    pub fn from_args(args: &Args) -> Self {
        Self {
            name: args.name.clone(),
            codec: args.codec,
            audio: args.audio,
            opus_bitrate_kbps: args.opus_bitrate_kbps,
            opus_complexity: args.opus_complexity,
            capture_width: args.capture_width,
            capture_height: args.capture_height,
            advertised_fps: args.capture_fps,
            frame_divider: args.frame_divider,
            bitrate_kbps: args.bitrate_kbps,
            gop: args.gop,
            video_fec_percentage: args.video_fec_percentage,
            video_min_parity_shards: args.video_min_parity_shards,
            encryption_mode: args.encryption_mode,
            hevc_loop_filter: args.hevc_loop_filter,
            repeat_headers: !args.no_repeat_headers,
            aud: !args.no_aud,
        }
    }

    pub fn apply(&self, args: &mut Args) {
        args.name.clone_from(&self.name);
        args.codec = self.codec;
        args.audio = self.audio;
        args.opus_bitrate_kbps = self.opus_bitrate_kbps;
        args.opus_complexity = self.opus_complexity;
        args.capture_width = self.capture_width;
        args.capture_height = self.capture_height;
        args.capture_fps = self.advertised_fps;
        args.frame_divider = self.frame_divider;
        args.bitrate_kbps = self.bitrate_kbps;
        args.gop = self.gop;
        args.video_fec_percentage = self.video_fec_percentage;
        args.video_min_parity_shards = self.video_min_parity_shards;
        args.encryption_mode = self.encryption_mode;
        args.hevc_loop_filter = self.hevc_loop_filter;
        args.no_repeat_headers = !self.repeat_headers;
        args.no_aud = !self.aud;
    }

    pub fn apply_v4l2(&self, settings: &mut V4l2Settings) {
        settings.codec = self.codec;
        settings.capture_width = self.capture_width;
        settings.capture_height = self.capture_height;
        settings.capture_fps = self.advertised_fps;
        settings.frame_divider = self.frame_divider;
        settings.bitrate_kbps = self.bitrate_kbps;
        settings.gop = self.gop;
        settings.hevc_loop_filter = self.hevc_loop_filter;
        settings.repeat_headers = self.repeat_headers;
        settings.aud = self.aud;
    }

    pub fn apply_audio(&self, settings: &mut AudioSettings) {
        settings.source = self.audio;
        settings.opus_bitrate_kbps = self.opus_bitrate_kbps;
        settings.opus_complexity = self.opus_complexity;
    }

    pub fn runtime_config(&self) -> Result<ServerRuntimeConfig, &'static str> {
        if self.advertised_fps % self.frame_divider != 0 {
            return Err("capture FPS must be divisible by the frame divider");
        }
        let video = vec![match self.codec {
            StreamCodec::Hevc => VideoCodec::Hevc,
            StreamCodec::H264 => VideoCodec::H264,
        }];
        let capabilities = if self.audio == AudioSource::None {
            CodecCapabilities::video_only(video)
        } else {
            CodecCapabilities::with_opus(video)
        };
        Ok(ServerRuntimeConfig {
            name: self.name.clone(),
            capabilities,
            video_stream: VideoStream::new(
                self.capture_width,
                self.capture_height,
                self.advertised_fps / self.frame_divider,
            )
            .with_fec(self.video_fec_percentage, self.video_min_parity_shards),
            encryption_mode: self.encryption_mode.into(),
        })
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if self.name.trim().is_empty() || self.name.len() > 64 {
            return Err("name must contain 1 to 64 characters");
        }
        if self.capture_width == 0 || self.capture_height == 0 {
            return Err("capture dimensions must be non-zero");
        }
        if self.advertised_fps == 0 || self.advertised_fps > 240 {
            return Err("advertised FPS must be between 1 and 240");
        }
        if self.frame_divider == 0 {
            return Err("frame divider must be at least 1");
        }
        if self.advertised_fps % self.frame_divider != 0 {
            return Err("capture FPS must be divisible by the frame divider");
        }
        if self.gop == 0 {
            return Err("GOP must be at least 1");
        }
        if self.video_fec_percentage > 100 {
            return Err("FEC percentage cannot exceed 100");
        }
        if self.opus_bitrate_kbps < 16 || self.opus_bitrate_kbps > 512 {
            return Err("Opus bitrate must be between 16 and 512 kbps");
        }
        if self.opus_complexity > 10 {
            return Err("Opus complexity must be between 0 and 10");
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct SettingsStore {
    path: PathBuf,
    current: Arc<RwLock<MoonlinkSettings>>,
}

impl SettingsStore {
    pub fn open(path: PathBuf, defaults: MoonlinkSettings) -> io::Result<Self> {
        let current = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other)?,
            Err(err) if err.kind() == io::ErrorKind::NotFound => defaults,
            Err(err) => return Err(err),
        };
        current.validate().map_err(io::Error::other)?;
        Ok(Self {
            path,
            current: Arc::new(RwLock::new(current)),
        })
    }

    pub async fn get(&self) -> MoonlinkSettings {
        self.current.read().await.clone()
    }

    pub async fn save(&self, settings: MoonlinkSettings) -> io::Result<()> {
        settings.validate().map_err(io::Error::other)?;
        let bytes = serde_json::to_vec_pretty(&settings).map_err(io::Error::other)?;
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || write_atomic(&path, &bytes))
            .await
            .map_err(io::Error::other)??;
        *self.current.write().await = settings;
        Ok(())
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("settings"),
        std::process::id()
    ));
    let mut file = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(temporary, path)?;
    File::open(parent)?.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn rejects_invalid_frame_divider() {
        let args = Args::parse_from(["nanokvm-host"]);
        let mut settings = MoonlinkSettings::from_args(&args);
        settings.frame_divider = 0;
        assert!(settings.validate().is_err());
    }

    #[test]
    fn defaults_missing_encryption_mode_to_opportunistic() {
        let args = Args::parse_from(["nanokvm-host"]);
        let settings = MoonlinkSettings::from_args(&args);
        let mut json = serde_json::to_value(settings).unwrap();
        json.as_object_mut().unwrap().remove("encryption_mode");

        let migrated: MoonlinkSettings = serde_json::from_value(json).unwrap();
        assert_eq!(
            migrated.encryption_mode,
            StreamEncryptionMode::Opportunistic
        );
    }
}
