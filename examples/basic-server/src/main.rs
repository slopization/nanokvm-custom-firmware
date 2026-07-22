use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use moonlink_core::{
    InMemoryPairingStore, InputContext, MediaProvider, MediaSession, MediaSessionConfig,
    MediaSessionError, ServerConfig, VideoFrame, create_access_unit,
};
use moonlink_protocol::{CodecCapabilities, InputEvent, VideoCodec};
use moonlink_tokio::ServerBuilder;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, ChildStdout, Command};

struct ExternalMediaProvider;

#[async_trait]
impl MediaProvider for ExternalMediaProvider {
    type Session = ExternalMediaSession;

    async fn start_session(
        &self,
        config: MediaSessionConfig,
    ) -> Result<Self::Session, MediaSessionError> {
        println!("starting media session: {config:?}");
        let mut child = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                &format!(
                    "testsrc=size={}x{}:rate={}",
                    config.width, config.height, config.fps
                ),
                "-an",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-tune",
                "zerolatency",
                "-x264-params",
                "slices=1:repeat-headers=1",
                "-g",
                &config.fps.to_string(),
                "-keyint_min",
                &config.fps.to_string(),
                "-pix_fmt",
                "yuv420p",
                "-bsf:v",
                "h264_metadata=aud=insert",
                "-f",
                "h264",
                "-",
            ])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .map_err(|err| MediaSessionError::Provider(err.to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| MediaSessionError::Provider("ffmpeg stdout unavailable".to_owned()))?;
        Ok(ExternalMediaSession {
            child,
            stdout,
            buffer: BytesMut::new(),
            timestamp_90khz: 0,
            timestamp_step: 90_000 / config.fps.max(1),
        })
    }
}

struct ExternalMediaSession {
    child: Child,
    stdout: ChildStdout,
    buffer: BytesMut,
    timestamp_90khz: u32,
    timestamp_step: u32,
}

#[async_trait]
impl MediaSession for ExternalMediaSession {
    async fn next_video(&mut self) -> Result<Option<VideoFrame>, MediaSessionError> {
        loop {
            if let Some(access_unit) = self.take_access_unit() {
                let timestamp = self.timestamp_90khz;
                self.timestamp_90khz = self.timestamp_90khz.wrapping_add(self.timestamp_step);
                return Ok(Some(VideoFrame::AccessUnit(create_access_unit(
                    access_unit.to_vec(),
                    timestamp,
                    true,
                ))));
            }

            let mut chunk = [0_u8; 8192];
            let read = self
                .stdout
                .read(&mut chunk)
                .await
                .map_err(|err| MediaSessionError::Provider(err.to_string()))?;
            if read == 0 {
                return Ok(None);
            }
            self.buffer.extend_from_slice(&chunk[..read]);
        }
    }

    async fn stop(&mut self) -> Result<(), MediaSessionError> {
        let _ = self.child.kill().await;
        Ok(())
    }
}

impl ExternalMediaSession {
    fn take_access_unit(&mut self) -> Option<Bytes> {
        let starts = find_start_codes(&self.buffer);
        if starts.len() < 4 {
            return None;
        }

        let mut next_access_unit = None;
        let mut seen_vcl = false;
        let mut seen_first_slice = false;
        for window in starts.windows(2) {
            let start_code = window[0].0;
            let nal_start = window[0].1;
            let nal_end = window[1].0;
            if nal_start >= self.buffer.len() || nal_start >= nal_end {
                continue;
            }
            let nal_type = self.buffer[nal_start] & 0x1f;
            if nal_type == 9 && seen_vcl {
                next_access_unit = Some(start_code);
                break;
            }
            if !(1..=5).contains(&nal_type) {
                continue;
            }

            let is_first_slice = first_mb_in_slice(&self.buffer[nal_start..nal_end]) == Some(0);
            if seen_first_slice && is_first_slice {
                next_access_unit = Some(start_code);
                break;
            }
            seen_vcl = true;
            seen_first_slice |= is_first_slice;
        }

        let split = next_access_unit?;
        Some(self.buffer.split_to(split).freeze())
    }
}

fn first_mb_in_slice(nal: &[u8]) -> Option<u32> {
    if nal.len() < 2 {
        return None;
    }
    read_ue(&remove_emulation_prevention(&nal[1..]))
}

fn remove_emulation_prevention(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut zero_count = 0;
    for &byte in bytes {
        if zero_count >= 2 && byte == 0x03 {
            zero_count = 0;
            continue;
        }
        out.push(byte);
        if byte == 0 {
            zero_count += 1;
        } else {
            zero_count = 0;
        }
    }
    out
}

fn read_ue(bytes: &[u8]) -> Option<u32> {
    let mut bit = 0;
    let mut leading_zeroes = 0;
    while bit < bytes.len() * 8 {
        if read_bit(bytes, bit)? {
            break;
        }
        leading_zeroes += 1;
        bit += 1;
    }
    if bit >= bytes.len() * 8 {
        return None;
    }
    bit += 1;

    let mut value = 1_u32.checked_shl(leading_zeroes)?;
    for _ in 0..leading_zeroes {
        value = value.checked_add(u32::from(read_bit(bytes, bit)?))?;
        bit += 1;
    }
    value.checked_sub(1)
}

fn read_bit(bytes: &[u8], bit: usize) -> Option<bool> {
    let byte = *bytes.get(bit / 8)?;
    Some(byte & (0x80 >> (bit % 8)) != 0)
}

fn find_start_codes(bytes: &[u8]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 4 <= bytes.len() {
        if bytes[i..].starts_with(&[0, 0, 1]) {
            out.push((i, i + 3));
            i += 3;
        } else if bytes[i..].starts_with(&[0, 0, 0, 1]) {
            out.push((i, i + 4));
            i += 4;
        } else {
            i += 1;
        }
    }
    out
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = ServerConfig::new(
        "moonlink-example",
        "0.0.0.0:47989".parse()?,
        "0.0.0.0:47989".parse()?,
        "0.0.0.0:48010".parse()?,
        CodecCapabilities::with_opus(vec![VideoCodec::H264]),
    )
    .with_https_bind("0.0.0.0:47984".parse()?);

    let handle = ServerBuilder::new(config)
        .media_provider(ExternalMediaProvider)
        .input_sink(|context: InputContext, event: InputEvent| async move {
            println!("input from {}: {:?}", context.client_id, event.kind);
        })
        .pairing_store(InMemoryPairingStore::default())
        .serve()
        .await?;

    println!(
        "moonlink example listening: http={}, https={}, discovery={}, rtsp={}",
        handle.http_addr(),
        handle.https_addr(),
        handle.discovery_addr(),
        handle.rtsp_addr()
    );
    tokio::signal::ctrl_c().await?;
    handle.shutdown().await?;
    Ok(())
}
