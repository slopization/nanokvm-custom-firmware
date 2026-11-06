use std::net::SocketAddr;
use std::path::PathBuf;

use clap::{Parser, ValueEnum};
use moonlink_core::EncryptionMode;
use moonlink_v4l2::FourCc;
use serde::{Deserialize, Serialize};

use crate::display::DisplayTransform;
use crate::hid::InputDevicePaths;
use crate::v4l2_provider::V4l2Settings;

#[derive(Debug, Clone, Parser)]
pub struct Args {
    #[arg(long, default_value = "nanokvm-moonlink")]
    pub name: String,
    #[arg(long)]
    pub pin: Option<String>,
    #[arg(long, default_value = "/var/lib/moonlink/pairing.json")]
    pub pairing_store: PathBuf,
    #[arg(long, default_value = "/var/lib/moonlink/settings.json")]
    pub settings_store: PathBuf,
    #[arg(long, default_value = "0.0.0.0:8080")]
    pub bind_web: SocketAddr,
    #[arg(long, default_value = "/dev/dri/card0")]
    pub display_device: PathBuf,
    #[arg(long, value_enum, default_value = "mirror-x")]
    pub display_transform: DisplayTransform,
    #[arg(long, default_value = "0.0.0.0:47989")]
    pub bind_http: SocketAddr,
    #[arg(long, default_value = "0.0.0.0:47984")]
    pub bind_https: SocketAddr,
    #[arg(long, default_value = "0.0.0.0:48010")]
    pub bind_rtsp: SocketAddr,
    #[arg(long, default_value = "0.0.0.0:47998")]
    pub bind_video: SocketAddr,
    #[arg(long, default_value = "0.0.0.0:48000")]
    pub bind_audio: SocketAddr,
    #[arg(long, default_value = "0.0.0.0:47999")]
    pub bind_control: SocketAddr,
    #[arg(long, value_enum, default_value = "hevc")]
    pub codec: StreamCodec,
    #[arg(long, value_enum, default_value = "usb")]
    pub audio: AudioSource,
    #[arg(long, value_enum, default_value = "uac1")]
    pub usb_audio_class: UsbAudioClass,
    #[arg(long, alias = "uac1-pcm-device", alias = "uac2-pcm-device")]
    pub usb_pcm_device: Option<PathBuf>,
    #[arg(long, default_value_t = 128)]
    pub opus_bitrate_kbps: u32,
    #[arg(long, default_value_t = 5, value_parser = clap::value_parser!(u8).range(0..=10))]
    pub opus_complexity: u8,
    #[arg(long)]
    pub video_command: Option<String>,
    #[arg(long)]
    pub capture_device: Option<PathBuf>,
    #[arg(long)]
    pub encoder_device: Option<PathBuf>,
    #[arg(long, default_value = "UYVY")]
    pub raw_fourcc: FourCc,
    #[arg(long, default_value_t = 4)]
    pub buffer_count: u32,
    #[arg(long, default_value_t = 1920)]
    pub capture_width: u32,
    #[arg(long, default_value_t = 1080)]
    pub capture_height: u32,
    #[arg(long, default_value_t = 30)]
    pub capture_fps: u32,
    #[arg(long, default_value_t = 1)]
    pub frame_divider: u32,
    #[arg(long, default_value_t = 3840)]
    pub max_width: u32,
    #[arg(long, default_value_t = 2160)]
    pub max_height: u32,
    #[arg(long, default_value_t = 60)]
    pub max_fps: u32,
    #[arg(long, default_value = "1000")]
    pub bitrate_kbps: Option<u32>,
    #[arg(long, default_value_t = 30)]
    pub gop: u32,
    #[arg(long)]
    pub wavefront: bool,
    #[arg(long)]
    pub no_seek: bool,
    #[arg(long, default_value_t = 0)]
    pub video_fec_percentage: u8,
    #[arg(long, default_value_t = 2)]
    pub video_min_parity_shards: usize,
    #[arg(long, value_enum, default_value = "opportunistic")]
    pub encryption_mode: StreamEncryptionMode,
    #[arg(long)]
    pub hevc_loop_filter: Option<i32>,
    #[arg(long)]
    pub no_repeat_headers: bool,
    #[arg(long)]
    pub no_aud: bool,
    #[arg(long)]
    pub keyboard_hid: Option<PathBuf>,
    #[arg(long)]
    pub mouse_hid: Option<PathBuf>,
    #[arg(long)]
    pub absolute_mouse_hid: Option<PathBuf>,
    #[arg(long)]
    pub trace_pointer_input: bool,
    #[arg(long)]
    pub setup_usb_gadget: bool,
    #[arg(long, default_value = "/sys/kernel/config/usb_gadget/moonlink")]
    pub gadget_dir: PathBuf,
    #[arg(long, default_value = "/var/lib/moonlink/msd")]
    pub msd_dir: PathBuf,
    #[arg(long)]
    pub udc: Option<String>,
    #[arg(long, default_value = "0x1d6b")]
    pub usb_vid: String,
    #[arg(long, default_value = "0x0108")]
    pub usb_pid: String,
    #[arg(long, default_value = "moonlink-nanokvm")]
    pub usb_serial: String,
    #[arg(long, default_value = "moonlink")]
    pub usb_manufacturer: String,
    #[arg(long, default_value = "NanoKVM Moonlight")]
    pub usb_product: String,
}

impl Args {
    pub fn input_device_paths(&self) -> Option<InputDevicePaths> {
        if self.keyboard_hid.is_none()
            && self.mouse_hid.is_none()
            && self.absolute_mouse_hid.is_none()
        {
            return None;
        }
        let keyboard = self
            .keyboard_hid
            .clone()
            .unwrap_or_else(|| PathBuf::from("/dev/hidg0"));
        let relative_mouse = self
            .mouse_hid
            .clone()
            .unwrap_or_else(|| PathBuf::from("/dev/hidg1"));
        let absolute_mouse = self
            .absolute_mouse_hid
            .clone()
            .unwrap_or_else(|| PathBuf::from("/dev/hidg2"));
        Some(InputDevicePaths {
            keyboard: Some(keyboard),
            relative_mouse: Some(relative_mouse),
            absolute_mouse: Some(absolute_mouse),
        })
    }

    pub fn source_kind(&self) -> Result<SourceKind, String> {
        if let Some(command) = &self.video_command {
            return Ok(SourceKind::Command(command.clone()));
        }
        let capture_device = self
            .capture_device
            .clone()
            .ok_or_else(|| "either --video-command or --capture-device is required".to_owned())?;
        let encoder_device = self
            .encoder_device
            .clone()
            .ok_or_else(|| "--encoder-device is required with --capture-device".to_owned())?;
        Ok(SourceKind::V4l2(V4l2Settings {
            capture_device,
            encoder_device,
            raw_fourcc: self.raw_fourcc,
            codec: self.codec,
            buffer_count: self.buffer_count,
            capture_width: self.capture_width,
            capture_height: self.capture_height,
            capture_fps: self.capture_fps,
            frame_divider: self.frame_divider,
            max_width: self.max_width,
            max_height: self.max_height,
            max_fps: self.max_fps,
            bitrate_kbps: self.bitrate_kbps,
            gop: self.gop,
            wavefront: self.wavefront,
            hevc_loop_filter: self.hevc_loop_filter,
            repeat_headers: !self.no_repeat_headers,
            aud: !self.no_aud,
        }))
    }

    pub fn audio_settings(&self) -> AudioSettings {
        AudioSettings {
            source: self.audio,
            pcm_device: self.usb_pcm_device.clone(),
            opus_bitrate_kbps: self.opus_bitrate_kbps,
            opus_complexity: self.opus_complexity,
        }
    }
}

#[derive(Debug, Clone)]
pub enum SourceKind {
    Command(String),
    V4l2(V4l2Settings),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StreamCodec {
    Hevc,
    H264,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StreamEncryptionMode {
    Never,
    #[default]
    Opportunistic,
    Mandatory,
}

impl From<StreamEncryptionMode> for EncryptionMode {
    fn from(mode: StreamEncryptionMode) -> Self {
        match mode {
            StreamEncryptionMode::Never => Self::Never,
            StreamEncryptionMode::Opportunistic => Self::Opportunistic,
            StreamEncryptionMode::Mandatory => Self::Mandatory,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioSource {
    None,
    Silence,
    Tone,
    #[value(alias = "uac1", alias = "uac2")]
    #[serde(alias = "uac1", alias = "uac2")]
    Usb,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UsbAudioClass {
    #[default]
    Uac1,
    Uac2,
}

#[derive(Debug, Clone)]
pub struct AudioSettings {
    pub source: AudioSource,
    pub pcm_device: Option<PathBuf>,
    pub opus_bitrate_kbps: u32,
    pub opus_complexity: u8,
}
