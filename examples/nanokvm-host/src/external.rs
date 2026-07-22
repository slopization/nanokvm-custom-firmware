use std::process::Stdio;

use async_trait::async_trait;
use bytes::BytesMut;
use moonlink_core::{
    MediaSession, MediaSessionConfig, MediaSessionError, VideoFrame, create_access_unit,
};
use moonlink_protocol::VideoCodec;
use tokio::{
    io::AsyncReadExt,
    process::{Child, ChildStdout, Command},
};

use crate::audio::AudioGenerator;
use crate::cli::{AudioSettings, StreamCodec};
use crate::hevc;

pub struct ExternalSession {
    child: Child,
    stdout: ChildStdout,
    buffer: BytesMut,
    timestamp_90khz: u32,
    timestamp_step: u32,
    codec: StreamCodec,
    audio: Option<AudioGenerator>,
}

impl ExternalSession {
    pub async fn start(
        command: &str,
        codec: StreamCodec,
        audio: AudioSettings,
        config: MediaSessionConfig,
    ) -> Result<Self, MediaSessionError> {
        validate_codec(codec, config.video_codec)?;
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(command)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|err| MediaSessionError::Provider(err.to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| MediaSessionError::Provider("encoder stdout unavailable".to_owned()))?;

        Ok(Self {
            child,
            stdout,
            buffer: BytesMut::new(),
            timestamp_90khz: 0,
            timestamp_step: 90_000 / config.fps.max(1),
            codec,
            audio: Some(AudioGenerator::new(
                audio,
                config.audio_packet_duration_ms * 48,
            )?),
        })
    }

    fn take_access_unit(&mut self) -> Option<(bytes::Bytes, bool)> {
        match self.codec {
            StreamCodec::Hevc => hevc::take_access_unit(&mut self.buffer),
            StreamCodec::H264 => None,
        }
    }
}

#[async_trait]
impl MediaSession for ExternalSession {
    async fn next_video(&mut self) -> Result<Option<VideoFrame>, MediaSessionError> {
        loop {
            if let Some((bytes, marker)) = self.take_access_unit() {
                let timestamp = self.timestamp_90khz;
                self.timestamp_90khz = self.timestamp_90khz.wrapping_add(self.timestamp_step);
                return Ok(Some(VideoFrame::AccessUnit(create_access_unit(
                    bytes.to_vec(),
                    timestamp,
                    marker,
                ))));
            }

            let mut chunk = [0_u8; 32 * 1024];
            let read = self
                .stdout
                .read(&mut chunk)
                .await
                .map_err(|err| MediaSessionError::Provider(err.to_string()))?;
            if read == 0 {
                return Ok(
                    hevc::finish_access_unit(&mut self.buffer).map(|(bytes, marker)| {
                        VideoFrame::AccessUnit(create_access_unit(
                            bytes.to_vec(),
                            self.timestamp_90khz,
                            marker,
                        ))
                    }),
                );
            }
            self.buffer.extend_from_slice(&chunk[..read]);
        }
    }

    async fn stop(&mut self) -> Result<(), MediaSessionError> {
        let _ = self.child.kill().await;
        Ok(())
    }

    fn take_audio(&mut self) -> Option<Box<dyn moonlink_core::AudioSession>> {
        self.audio
            .take()
            .map(|audio| Box::new(audio) as Box<dyn moonlink_core::AudioSession>)
    }
}

fn validate_codec(requested: StreamCodec, moonlight: VideoCodec) -> Result<(), MediaSessionError> {
    let expected = match requested {
        StreamCodec::Hevc => VideoCodec::Hevc,
        StreamCodec::H264 => VideoCodec::H264,
    };
    if expected != moonlight {
        return Err(MediaSessionError::Provider(format!(
            "provider supplies {expected:?}, Moonlight requested {moonlight:?}"
        )));
    }
    if requested == StreamCodec::H264 {
        return Err(MediaSessionError::Provider(
            "H.264 access-unit parsing is reserved for the next encoder implementation".to_owned(),
        ));
    }
    Ok(())
}
