use std::fs::{self, File};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use moonlink_core::{ServerRuntimeConfig, VideoStream};
use moonlink_protocol::{CodecCapabilities, VideoCodec};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::cli::{
    Args, AudioSettings, AudioSource, StreamCodec, StreamEncryptionMode, UsbAudioClass,
};
use crate::v4l2_provider::V4l2Settings;

fn default_true() -> bool {
    true
}

/// Settings for the host-controlled browser KVM stream. There is deliberately
/// no shared stream section: this block is complete on its own.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WebSettings {
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
    #[serde(default)]
    pub wavefront: bool,
    #[serde(default = "default_true")]
    pub seek: bool,
    pub hevc_loop_filter: Option<i32>,
    pub repeat_headers: bool,
    pub aud: bool,
}

/// Settings for the Moonlight/GameStream host. Complete on its own, including
/// the fields the web stream does not have (FEC, encryption, host name).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MoonlightSettings {
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
    #[serde(default)]
    pub wavefront: bool,
    pub video_fec_percentage: u8,
    pub video_min_parity_shards: usize,
    #[serde(default)]
    pub encryption_mode: StreamEncryptionMode,
    pub hevc_loop_filter: Option<i32>,
    pub repeat_headers: bool,
    pub aud: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// Device-wide: the USB gadget must be rebuilt as one unit.
    #[serde(default)]
    pub usb_audio_class: UsbAudioClass,
    pub web: WebSettings,
    pub moonlight: MoonlightSettings,
}

impl WebSettings {
    pub fn output_fps(&self) -> u32 {
        self.advertised_fps / self.frame_divider.max(1)
    }

    pub fn apply_args(&self, args: &mut Args) {
        args.codec = StreamCodec::Hevc;
        args.audio = self.audio;
        args.opus_bitrate_kbps = self.opus_bitrate_kbps;
        args.opus_complexity = self.opus_complexity;
        args.capture_width = self.capture_width;
        args.capture_height = self.capture_height;
        args.capture_fps = self.advertised_fps;
        args.frame_divider = self.frame_divider;
        args.bitrate_kbps = self.bitrate_kbps;
        args.gop = self.gop;
        args.wavefront = self.wavefront;
        args.no_seek = !self.seek;
        args.hevc_loop_filter = self.hevc_loop_filter;
        args.no_repeat_headers = !self.repeat_headers;
        args.no_aud = !self.aud;
    }

    pub fn apply_v4l2(&self, settings: &mut V4l2Settings) {
        settings.codec = StreamCodec::Hevc;
        settings.capture_width = self.capture_width;
        settings.capture_height = self.capture_height;
        settings.capture_fps = self.advertised_fps;
        settings.frame_divider = self.frame_divider;
        settings.bitrate_kbps = self.bitrate_kbps;
        settings.gop = self.gop;
        settings.wavefront = self.wavefront;
        settings.hevc_loop_filter = self.hevc_loop_filter;
        settings.repeat_headers = self.repeat_headers;
        settings.aud = self.aud;
    }

    pub fn apply_audio(&self, settings: &mut AudioSettings) {
        settings.source = self.audio;
        settings.opus_bitrate_kbps = self.opus_bitrate_kbps;
        settings.opus_complexity = self.opus_complexity;
    }

    fn validate(&self) -> Result<(), &'static str> {
        if self.codec != StreamCodec::Hevc {
            return Err("the web KVM stream supports HEVC only");
        }
        validate_stream(
            self.capture_width,
            self.capture_height,
            self.advertised_fps,
            self.frame_divider,
            self.gop,
            self.bitrate_kbps,
            self.opus_bitrate_kbps,
            self.opus_complexity,
            true,
        )
    }
}

impl MoonlightSettings {
    pub fn output_fps(&self) -> u32 {
        self.advertised_fps / self.frame_divider.max(1)
    }

    pub fn apply_args(&self, args: &mut Args) {
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
        args.wavefront = self.wavefront;
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
        settings.wavefront = self.wavefront;
        settings.hevc_loop_filter = self.hevc_loop_filter;
        settings.repeat_headers = self.repeat_headers;
        settings.aud = self.aud;
    }

    pub fn apply_audio(&self, settings: &mut AudioSettings) {
        settings.source = self.audio;
        settings.opus_bitrate_kbps = self.opus_bitrate_kbps;
        settings.opus_complexity = self.opus_complexity;
    }

    pub fn stream_changed(&self, previous: &Self) -> bool {
        let mut current = self.clone();
        let mut previous = previous.clone();
        current.name.clear();
        previous.name.clear();
        current != previous
    }

    fn validate(&self) -> Result<(), &'static str> {
        if self.name.trim().is_empty() || self.name.len() > 64 {
            return Err("name must contain 1 to 64 characters");
        }
        if self.video_fec_percentage > 100 {
            return Err("FEC percentage cannot exceed 100");
        }
        validate_stream(
            self.capture_width,
            self.capture_height,
            self.advertised_fps,
            self.frame_divider,
            self.gop,
            self.bitrate_kbps,
            self.opus_bitrate_kbps,
            self.opus_complexity,
            false,
        )
    }
}

fn validate_stream(
    width: u32,
    height: u32,
    fps: u32,
    divider: u32,
    gop: u32,
    bitrate: Option<u32>,
    opus_bitrate_kbps: u32,
    opus_complexity: u8,
    allow_no_gop: bool,
) -> Result<(), &'static str> {
    if width == 0 || height == 0 {
        return Err("capture dimensions must be non-zero");
    }
    if fps == 0 || fps > 240 {
        return Err("advertised FPS must be between 1 and 240");
    }
    if divider == 0 {
        return Err("frame divider must be at least 1");
    }
    if !fps.is_multiple_of(divider) {
        return Err("capture FPS must be divisible by the frame divider");
    }
    if gop > 2047 {
        return Err("GOP must be at most 2047");
    }
    if gop == 0 && !allow_no_gop {
        return Err("GOP must be at least 1");
    }
    if bitrate == Some(0) {
        return Err("bitrate must be non-zero when set");
    }
    if !(16..=512).contains(&opus_bitrate_kbps) {
        return Err("Opus bitrate must be between 16 and 512 kbps");
    }
    if opus_complexity > 10 {
        return Err("Opus complexity must be between 0 and 10");
    }
    Ok(())
}

impl Settings {
    pub fn web_stream_changed(&self, previous: &Self) -> bool {
        self.web != previous.web
    }

    pub fn moonlight_stream_changed(&self, previous: &Self) -> bool {
        self.moonlight.stream_changed(&previous.moonlight)
    }

    pub fn requires_restart(&self, previous: &Self) -> bool {
        self.usb_audio_class != previous.usb_audio_class
    }

    pub fn from_args(args: &Args) -> Self {
        Self {
            usb_audio_class: args.usb_audio_class,
            web: WebSettings {
                codec: StreamCodec::Hevc,
                audio: args.audio,
                opus_bitrate_kbps: args.opus_bitrate_kbps,
                opus_complexity: args.opus_complexity,
                capture_width: args.capture_width,
                capture_height: args.capture_height,
                advertised_fps: args.capture_fps,
                frame_divider: args.frame_divider,
                bitrate_kbps: args.bitrate_kbps,
                gop: args.gop,
                wavefront: args.wavefront,
                seek: !args.no_seek,
                hevc_loop_filter: args.hevc_loop_filter,
                repeat_headers: !args.no_repeat_headers,
                aud: !args.no_aud,
            },
            moonlight: MoonlightSettings {
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
                wavefront: args.wavefront,
                hevc_loop_filter: args.hevc_loop_filter,
                repeat_headers: !args.no_repeat_headers,
                aud: !args.no_aud,
            },
        }
    }

    pub fn apply_device_args(&self, args: &mut Args) {
        args.usb_audio_class = self.usb_audio_class;
    }

    pub fn moonlight_runtime_config(&self) -> Result<ServerRuntimeConfig, &'static str> {
        let moonlight = &self.moonlight;
        if !moonlight
            .advertised_fps
            .is_multiple_of(moonlight.frame_divider)
        {
            return Err("capture FPS must be divisible by the frame divider");
        }
        let video = vec![match moonlight.codec {
            StreamCodec::Hevc => VideoCodec::Hevc,
            StreamCodec::H264 => VideoCodec::H264,
        }];
        let capabilities = if moonlight.audio == AudioSource::None {
            CodecCapabilities::video_only(video)
        } else {
            CodecCapabilities::with_opus(video)
        };
        Ok(ServerRuntimeConfig {
            name: moonlight.name.clone(),
            capabilities,
            video_stream: VideoStream::new(
                moonlight.capture_width,
                moonlight.capture_height,
                moonlight.output_fps(),
            )
            .with_fec(
                moonlight.video_fec_percentage,
                moonlight.video_min_parity_shards,
            ),
            encryption_mode: moonlight.encryption_mode.into(),
        })
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        self.web.validate()?;
        self.moonlight.validate()?;
        Ok(())
    }

    pub fn parse(bytes: &[u8]) -> serde_json::Result<Self> {
        let value: serde_json::Value = serde_json::from_slice(bytes)?;
        if value.get("web").is_some() || value.get("moonlight").is_some() {
            serde_json::from_value(value)
        } else {
            let flat: FlatSettings = serde_json::from_value(value)?;
            Ok(Self::from(flat))
        }
    }
}

/// The pre-split flat settings shape, kept for the existing management UI and
/// for migrating an older `settings.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlatSettings {
    pub name: String,
    pub codec: StreamCodec,
    pub audio: AudioSource,
    #[serde(default)]
    pub usb_audio_class: UsbAudioClass,
    pub opus_bitrate_kbps: u32,
    pub opus_complexity: u8,
    pub capture_width: u32,
    pub capture_height: u32,
    pub advertised_fps: u32,
    pub frame_divider: u32,
    pub bitrate_kbps: Option<u32>,
    pub gop: u32,
    #[serde(default)]
    pub wavefront: bool,
    pub video_fec_percentage: u8,
    pub video_min_parity_shards: usize,
    #[serde(default)]
    pub encryption_mode: StreamEncryptionMode,
    pub hevc_loop_filter: Option<i32>,
    pub repeat_headers: bool,
    pub aud: bool,
}

impl FlatSettings {
    pub fn from_settings(settings: &Settings) -> Self {
        let moonlight = &settings.moonlight;
        Self {
            name: moonlight.name.clone(),
            codec: moonlight.codec,
            audio: moonlight.audio,
            usb_audio_class: settings.usb_audio_class,
            opus_bitrate_kbps: moonlight.opus_bitrate_kbps,
            opus_complexity: moonlight.opus_complexity,
            capture_width: moonlight.capture_width,
            capture_height: moonlight.capture_height,
            advertised_fps: moonlight.advertised_fps,
            frame_divider: moonlight.frame_divider,
            bitrate_kbps: moonlight.bitrate_kbps,
            gop: moonlight.gop,
            wavefront: moonlight.wavefront,
            video_fec_percentage: moonlight.video_fec_percentage,
            video_min_parity_shards: moonlight.video_min_parity_shards,
            encryption_mode: moonlight.encryption_mode,
            hevc_loop_filter: moonlight.hevc_loop_filter,
            repeat_headers: moonlight.repeat_headers,
            aud: moonlight.aud,
        }
    }

    /// Merge the flat view into the Moonlight stream and the device-wide USB
    /// audio class. The web stream is intentionally left alone.
    pub fn apply_to(&self, settings: &mut Settings) {
        settings.usb_audio_class = self.usb_audio_class;
        let moonlight = &mut settings.moonlight;
        moonlight.name.clone_from(&self.name);
        moonlight.codec = self.codec;
        moonlight.audio = self.audio;
        moonlight.opus_bitrate_kbps = self.opus_bitrate_kbps;
        moonlight.opus_complexity = self.opus_complexity;
        moonlight.capture_width = self.capture_width;
        moonlight.capture_height = self.capture_height;
        moonlight.advertised_fps = self.advertised_fps;
        moonlight.frame_divider = self.frame_divider;
        moonlight.bitrate_kbps = self.bitrate_kbps;
        moonlight.gop = self.gop;
        moonlight.wavefront = self.wavefront;
        moonlight.video_fec_percentage = self.video_fec_percentage;
        moonlight.video_min_parity_shards = self.video_min_parity_shards;
        moonlight.encryption_mode = self.encryption_mode;
        moonlight.hevc_loop_filter = self.hevc_loop_filter;
        moonlight.repeat_headers = self.repeat_headers;
        moonlight.aud = self.aud;
    }
}

impl From<FlatSettings> for Settings {
    fn from(flat: FlatSettings) -> Self {
        Self {
            usb_audio_class: flat.usb_audio_class,
            web: WebSettings {
                codec: StreamCodec::Hevc,
                audio: flat.audio,
                opus_bitrate_kbps: flat.opus_bitrate_kbps,
                opus_complexity: flat.opus_complexity,
                capture_width: flat.capture_width,
                capture_height: flat.capture_height,
                advertised_fps: flat.advertised_fps,
                frame_divider: flat.frame_divider,
                bitrate_kbps: flat.bitrate_kbps,
                gop: flat.gop,
                wavefront: flat.wavefront,
                seek: true,
                hevc_loop_filter: flat.hevc_loop_filter,
                repeat_headers: flat.repeat_headers,
                aud: flat.aud,
            },
            moonlight: MoonlightSettings {
                name: flat.name,
                codec: flat.codec,
                audio: flat.audio,
                opus_bitrate_kbps: flat.opus_bitrate_kbps,
                opus_complexity: flat.opus_complexity,
                capture_width: flat.capture_width,
                capture_height: flat.capture_height,
                advertised_fps: flat.advertised_fps,
                frame_divider: flat.frame_divider,
                bitrate_kbps: flat.bitrate_kbps,
                gop: flat.gop,
                wavefront: flat.wavefront,
                video_fec_percentage: flat.video_fec_percentage,
                video_min_parity_shards: flat.video_min_parity_shards,
                encryption_mode: flat.encryption_mode,
                hevc_loop_filter: flat.hevc_loop_filter,
                repeat_headers: flat.repeat_headers,
                aud: flat.aud,
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct SettingsStore {
    path: PathBuf,
    current: Arc<RwLock<Settings>>,
}

impl SettingsStore {
    pub fn open(path: PathBuf, defaults: Settings) -> io::Result<Self> {
        let current = match fs::read(&path) {
            Ok(bytes) => Settings::parse(&bytes).map_err(io::Error::other)?,
            Err(err) if err.kind() == io::ErrorKind::NotFound => defaults,
            Err(err) => return Err(err),
        };
        current.validate().map_err(io::Error::other)?;
        Ok(Self {
            path,
            current: Arc::new(RwLock::new(current)),
        })
    }

    pub async fn get(&self) -> Settings {
        self.current.read().await.clone()
    }

    pub async fn save(&self, settings: Settings) -> io::Result<()> {
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

    fn defaults() -> Settings {
        Settings::from_args(&Args::parse_from(["nanokvm-host"]))
    }

    #[test]
    fn rejects_invalid_frame_divider() {
        let mut settings = defaults();
        settings.web.frame_divider = 0;
        assert!(settings.validate().is_err());

        let mut settings = defaults();
        settings.moonlight.frame_divider = 0;
        assert!(settings.validate().is_err());
    }

    #[test]
    fn defaults_video_bitrate_to_1000_kbps() {
        let args = Args::parse_from(["nanokvm-host"]);
        assert_eq!(args.bitrate_kbps, Some(1000));
        let settings = Settings::from_args(&args);
        assert_eq!(settings.web.bitrate_kbps, Some(1000));
        assert_eq!(settings.moonlight.bitrate_kbps, Some(1000));
    }

    #[test]
    fn defaults_missing_encryption_mode_to_opportunistic() {
        let settings = defaults();
        let mut json = serde_json::to_value(&settings.moonlight).unwrap();
        json.as_object_mut().unwrap().remove("encryption_mode");

        let migrated: MoonlightSettings = serde_json::from_value(json).unwrap();
        assert_eq!(
            migrated.encryption_mode,
            StreamEncryptionMode::Opportunistic
        );
    }

    #[test]
    fn accepts_legacy_usb_audio_names_and_serializes_usb() {
        for name in ["uac1", "uac2"] {
            let source: AudioSource = serde_json::from_value(serde_json::json!(name)).unwrap();
            assert_eq!(source, AudioSource::Usb);
            let args = Args::try_parse_from(["nanokvm-host", "--audio", name]).unwrap();
            assert_eq!(args.audio, AudioSource::Usb);
        }
        assert_eq!(
            serde_json::to_value(AudioSource::Usb).unwrap(),
            serde_json::json!("usb")
        );
    }

    #[test]
    fn defaults_missing_usb_audio_class_to_uac1() {
        let settings = defaults();
        let mut json = serde_json::to_value(&settings).unwrap();
        json.as_object_mut().unwrap().remove("usb_audio_class");

        let migrated: Settings = serde_json::from_value(json).unwrap();
        assert_eq!(migrated.usb_audio_class, UsbAudioClass::Uac1);
    }

    #[test]
    fn usb_audio_class_change_requires_restart_but_not_stream_restart() {
        let previous = defaults();
        let mut current = previous.clone();
        current.usb_audio_class = UsbAudioClass::Uac2;

        assert!(current.requires_restart(&previous));
        assert!(!current.web_stream_changed(&previous));
        assert!(!current.moonlight_stream_changed(&previous));
    }

    #[test]
    fn migrates_flat_settings_to_both_stream_sections() {
        let settings = defaults();
        let flat = FlatSettings::from_settings(&settings);
        let bytes = serde_json::to_vec(&flat).unwrap();
        let parsed = Settings::parse(&bytes).unwrap();

        assert_eq!(parsed.usb_audio_class, flat.usb_audio_class);
        assert_eq!(parsed.web.bitrate_kbps, flat.bitrate_kbps);
        assert_eq!(parsed.web.gop, flat.gop);
        assert_eq!(parsed.moonlight.name, flat.name);
        assert_eq!(parsed.moonlight.bitrate_kbps, flat.bitrate_kbps);
    }

    #[test]
    fn roundtrips_nested_settings() {
        let settings = defaults();
        let bytes = serde_json::to_vec(&settings).unwrap();
        assert_eq!(Settings::parse(&bytes).unwrap(), settings);
    }

    #[test]
    fn web_and_moonlight_settings_are_independent() {
        let mut settings = defaults();
        settings.web.bitrate_kbps = Some(111);
        settings.web.gop = 11;
        settings.web.audio = AudioSource::None;
        settings.moonlight.bitrate_kbps = Some(222);
        settings.moonlight.gop = 22;
        settings.moonlight.audio = AudioSource::Usb;

        let bytes = serde_json::to_vec(&settings).unwrap();
        let parsed = Settings::parse(&bytes).unwrap();
        assert_eq!(parsed.web.bitrate_kbps, Some(111));
        assert_eq!(parsed.web.gop, 11);
        assert_eq!(parsed.web.audio, AudioSource::None);
        assert_eq!(parsed.moonlight.bitrate_kbps, Some(222));
        assert_eq!(parsed.moonlight.gop, 22);
        assert_eq!(parsed.moonlight.audio, AudioSource::Usb);
    }

    #[test]
    fn flat_updates_leave_web_settings_alone() {
        let mut settings = defaults();
        settings.web.bitrate_kbps = Some(111);
        settings.web.gop = 11;
        let mut flat = FlatSettings::from_settings(&settings);
        flat.bitrate_kbps = Some(222);
        flat.gop = 22;
        flat.apply_to(&mut settings);

        assert_eq!(settings.web.bitrate_kbps, Some(111));
        assert_eq!(settings.web.gop, 11);
        assert_eq!(settings.moonlight.bitrate_kbps, Some(222));
        assert_eq!(settings.moonlight.gop, 22);
    }

    #[test]
    fn web_allows_no_periodic_gop() {
        let mut settings = defaults();
        settings.web.gop = 0;
        assert!(settings.validate().is_ok());
    }

    #[test]
    fn rejects_out_of_range_gop() {
        let mut settings = defaults();
        settings.web.gop = 2048;
        assert!(settings.validate().is_err());

        let mut settings = defaults();
        settings.moonlight.gop = 2048;
        assert!(settings.validate().is_err());
    }

    #[test]
    fn moonlight_rejects_zero_gop() {
        let mut settings = defaults();
        settings.moonlight.gop = 0;
        assert!(settings.validate().is_err());
    }
}
