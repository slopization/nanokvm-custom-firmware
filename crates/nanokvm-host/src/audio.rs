use async_trait::async_trait;
use moonlink_core::{AudioFrame, AudioSession, MediaSessionError, create_audio_frame};
use opus::{Application, Bitrate, MSEncoder};
use std::f32::consts::TAU;

use crate::alsa_pcm::PcmCapture;
use crate::cli::{AudioSettings, AudioSource};

const CHANNELS: usize = 2;
const MAX_OPUS_PACKET_SIZE: usize = 1275;
pub const USB_AUDIO_SAMPLE_RATE: u32 = 48_000;
pub const USB_AUDIO_CHANNELS: u8 = 2;
pub const USB_AUDIO_SAMPLE_SIZE: u8 = 2;
pub const USB_AUDIO_VOLUME_MIN_DB_256: i16 = -25_600;
pub const USB_AUDIO_VOLUME_MAX_DB_256: i16 = 0;
pub const USB_AUDIO_VOLUME_RES_DB_256: i16 = 256;

pub enum AudioGenerator {
    None,
    Silence {
        opus: OpusAudioGenerator,
        interval: tokio::time::Interval,
    },
    Usb(UsbAudioGenerator),
    Tone(ToneAudioGenerator),
}

pub struct UsbAudioGenerator {
    capture: PcmCapture,
    opus: OpusAudioGenerator,
    frames_encoded: u64,
}

pub struct ToneAudioGenerator {
    opus: OpusAudioGenerator,
    phase: f32,
    interval: tokio::time::Interval,
}

pub(crate) struct OpusAudioGenerator {
    encoder: MSEncoder,
    timestamp_48khz: u32,
    frame_samples: u32,
}

impl AudioGenerator {
    pub fn new(settings: AudioSettings, frame_samples: u32) -> Result<Self, MediaSessionError> {
        match settings.source {
            AudioSource::None => Ok(Self::None),
            AudioSource::Silence => Ok(Self::Silence {
                opus: OpusAudioGenerator::new(&settings, frame_samples)?,
                interval: audio_interval(frame_samples),
            }),
            AudioSource::Tone => Ok(Self::Tone(ToneAudioGenerator {
                opus: OpusAudioGenerator::new(&settings, frame_samples)?,
                phase: 0.0,
                interval: audio_interval(frame_samples),
            })),
            AudioSource::Usb => {
                let capture =
                    PcmCapture::open(settings.pcm_device.as_deref(), frame_samples as usize)
                        .map_err(|err| provider_error(format!("opening USB audio PCM: {err}")))?;
                Ok(Self::Usb(UsbAudioGenerator {
                    capture,
                    opus: OpusAudioGenerator::new(&settings, frame_samples)?,
                    frames_encoded: 0,
                }))
            }
        }
    }

    pub async fn next_frame(&mut self) -> Result<Option<AudioFrame>, MediaSessionError> {
        match self {
            Self::None => std::future::pending().await,
            Self::Silence { opus, interval } => {
                interval.tick().await;
                opus.encode(&vec![0; opus.frame_samples as usize * CHANNELS])
                    .map(Some)
            }
            Self::Usb(audio) => audio.next_frame().await,
            Self::Tone(audio) => audio.next_frame().await.map(Some),
        }
    }
}

#[async_trait]
impl AudioSession for AudioGenerator {
    async fn next_audio(&mut self) -> Result<Option<AudioFrame>, MediaSessionError> {
        self.next_frame().await
    }
}

impl UsbAudioGenerator {
    async fn next_frame(&mut self) -> Result<Option<AudioFrame>, MediaSessionError> {
        let samples = self
            .capture
            .read_frame()
            .await
            .map_err(|err| provider_error(format!("reading USB audio PCM: {err}")))?;
        let peak = samples
            .iter()
            .map(|sample| sample.unsigned_abs())
            .max()
            .unwrap_or(0);
        let frame = self.opus.encode(&samples)?;
        self.frames_encoded += 1;
        let log_interval =
            (USB_AUDIO_SAMPLE_RATE as u64 * 5 / u64::from(self.opus.frame_samples)).max(1);
        if self.frames_encoded == 1 || self.frames_encoded.is_multiple_of(log_interval) {
            println!(
                "USB audio PCM: frames={}, peak={}, opus_bytes={}",
                self.frames_encoded,
                peak,
                frame.data.len()
            );
        }
        Ok(Some(frame))
    }
}

impl ToneAudioGenerator {
    async fn next_frame(&mut self) -> Result<AudioFrame, MediaSessionError> {
        self.interval.tick().await;
        let step = TAU * 440.0 / USB_AUDIO_SAMPLE_RATE as f32;
        let mut samples = Vec::with_capacity(self.opus.frame_samples as usize * CHANNELS);
        for _ in 0..self.opus.frame_samples {
            let sample = (self.phase.sin() * 8192.0) as i16;
            samples.extend_from_slice(&[sample, sample]);
            self.phase = (self.phase + step) % TAU;
        }
        self.opus.encode(&samples)
    }
}

fn audio_interval(frame_samples: u32) -> tokio::time::Interval {
    let period = std::time::Duration::from_micros(
        u64::from(frame_samples) * 1_000_000 / u64::from(USB_AUDIO_SAMPLE_RATE),
    );
    let mut interval = tokio::time::interval(period);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    interval
}

impl OpusAudioGenerator {
    fn new(settings: &AudioSettings, frame_samples: u32) -> Result<Self, MediaSessionError> {
        let mut encoder =
            MSEncoder::new(USB_AUDIO_SAMPLE_RATE, 1, 1, &[0, 1], Application::LowDelay)
                .map_err(|err| provider_error(format!("creating Opus encoder: {err}")))?;
        encoder
            .set_bitrate(Bitrate::Bits(
                settings.opus_bitrate_kbps.saturating_mul(1000) as i32,
            ))
            .map_err(|err| provider_error(format!("setting Opus bitrate: {err}")))?;
        encoder
            .set_complexity(settings.opus_complexity.into())
            .map_err(|err| provider_error(format!("setting Opus complexity: {err}")))?;
        encoder
            .set_vbr(false)
            .map_err(|err| provider_error(format!("disabling Opus VBR: {err}")))?;
        println!(
            "Opus encoder: 48000 Hz stereo, {} ms, {} kbps CBR, complexity {}",
            frame_samples / 48,
            settings.opus_bitrate_kbps,
            settings.opus_complexity
        );
        Ok(Self {
            encoder,
            timestamp_48khz: 0,
            frame_samples,
        })
    }

    fn encode(&mut self, samples: &[i16]) -> Result<AudioFrame, MediaSessionError> {
        let mut packet = vec![0; MAX_OPUS_PACKET_SIZE];
        let size = self
            .encoder
            .encode(samples, &mut packet)
            .map_err(|err| provider_error(format!("encoding Opus: {err}")))?;
        packet.truncate(size);
        let frame = create_audio_frame(packet, self.timestamp_48khz, self.frame_samples);
        self.timestamp_48khz = self.timestamp_48khz.wrapping_add(self.frame_samples);
        Ok(frame)
    }
}

fn provider_error(message: String) -> MediaSessionError {
    MediaSessionError::Provider(message)
}
