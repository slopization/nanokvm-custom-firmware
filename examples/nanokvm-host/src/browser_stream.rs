use std::convert::Infallible;

use axum::body::Body;
use axum::http::header::CONTENT_TYPE;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use tokio_stream::wrappers::ReceiverStream;

use crate::cli::{AudioSettings, AudioSource, StreamCodec};
use crate::session_arbiter::{SessionArbiter, SessionOwner};
use crate::settings::SettingsStore;
use crate::v4l2_provider::{V4l2Session, V4l2Settings, output_fps};
use moonlink_core::{MediaSession, MediaSessionConfig, VideoControlMessage};
use moonlink_protocol::VideoCodec;

const STREAM_MAGIC: &[u8; 8] = b"NKHEVC01";
const STREAM_TIMESCALE: u32 = 90_000;
const STREAM_QUEUE_FRAMES: usize = 4;
const MAX_ACCESS_UNIT_SIZE: usize = 32 * 1024 * 1024;

#[derive(Clone)]
pub struct BrowserCapture {
    arbiter: SessionArbiter,
    settings: SettingsStore,
    v4l2: Option<V4l2Settings>,
}

impl BrowserCapture {
    pub fn new(
        arbiter: SessionArbiter,
        settings: SettingsStore,
        v4l2: Option<V4l2Settings>,
    ) -> Self {
        Self {
            arbiter,
            settings,
            v4l2,
        }
    }

    pub async fn stream(&self) -> Response {
        match self.start().await {
            Ok(response) => response,
            Err((status, message)) => (status, message).into_response(),
        }
    }

    async fn start(&self) -> Result<Response, (StatusCode, String)> {
        let Some(mut settings) = self.v4l2.clone() else {
            return Err((
                StatusCode::NOT_IMPLEMENTED,
                "browser capture requires the V4L2 source".to_owned(),
            ));
        };
        let lease = self.arbiter.acquire(SessionOwner::Web).map_err(|owner| {
            (
                StatusCode::CONFLICT,
                format!("capture is owned by {owner:?}"),
            )
        })?;
        self.settings.get().await.apply_v4l2(&mut settings);
        settings.codec = StreamCodec::Hevc;
        let fps =
            output_fps(&settings).map_err(|err| (StatusCode::BAD_REQUEST, err.to_string()))?;
        let config = MediaSessionConfig {
            width: settings.capture_width,
            height: settings.capture_height,
            fps,
            video_codec: VideoCodec::Hevc,
            audio_codec: None,
            ..MediaSessionConfig::default()
        };
        let audio = AudioSettings {
            source: AudioSource::None,
            pcm_device: None,
            opus_bitrate_kbps: 128,
            opus_complexity: 0,
        };
        let mut session = V4l2Session::start(settings.clone(), audio, config)
            .await
            .map_err(|err| (StatusCode::SERVICE_UNAVAILABLE, err.to_string()))?;

        let (sender, receiver) =
            tokio::sync::mpsc::channel::<Result<Bytes, Infallible>>(STREAM_QUEUE_FRAMES * 2 + 1);
        sender
            .try_send(Ok(Bytes::copy_from_slice(&stream_preamble(
                settings.capture_width,
                settings.capture_height,
                fps,
            ))))
            .expect("new stream channel");
        tokio::spawn(async move {
            let _lease = lease;
            let mut waiting_for_keyframe = false;
            let mut stream_timestamp = 0_u32;
            let timestamp_step = STREAM_TIMESCALE / fps.max(1);
            loop {
                let frame = match session.next_encoded().await {
                    Ok(Some(frame)) => frame,
                    Ok(None) => continue,
                    Err(err) => {
                        eprintln!("browser capture stopped: {err}");
                        break;
                    }
                };
                if waiting_for_keyframe && !frame.keyframe {
                    continue;
                }
                match sender.try_reserve_many(2) {
                    Ok(mut permits) => {
                        let (header, payload) =
                            match stream_frame(frame.bytes, stream_timestamp, frame.keyframe) {
                                Ok(chunks) => chunks,
                                Err(err) => {
                                    eprintln!("browser frame dropped: {err}");
                                    continue;
                                }
                            };
                        permits.next().expect("two reserved slots").send(Ok(header));
                        permits
                            .next()
                            .expect("two reserved slots")
                            .send(Ok(payload));
                        stream_timestamp = stream_timestamp.wrapping_add(timestamp_step);
                        waiting_for_keyframe = false;
                    }
                    Err(tokio::sync::mpsc::error::TrySendError::Full(())) => {
                        let request_keyframe = !waiting_for_keyframe || frame.keyframe;
                        waiting_for_keyframe = true;
                        if request_keyframe {
                            let _ = session
                                .handle_video_control(VideoControlMessage::RequestIdrFrame)
                                .await;
                        }
                    }
                    Err(tokio::sync::mpsc::error::TrySendError::Closed(())) => break,
                }
            }
            let _ = session.stop().await;
        });

        let mut response = Response::new(Body::from_stream(ReceiverStream::new(receiver)));
        let headers = response.headers_mut();
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/vnd.nanokvm.hevc"),
        );
        headers.insert(
            "cache-control",
            HeaderValue::from_static("no-store, no-transform"),
        );
        headers.insert("x-accel-buffering", HeaderValue::from_static("no"));
        Ok(response)
    }
}

fn stream_preamble(width: u32, height: u32, fps: u32) -> [u8; 24] {
    let mut bytes = [0; 24];
    bytes[..8].copy_from_slice(STREAM_MAGIC);
    bytes[8..12].copy_from_slice(&width.to_be_bytes());
    bytes[12..16].copy_from_slice(&height.to_be_bytes());
    bytes[16..20].copy_from_slice(&fps.to_be_bytes());
    bytes[20..24].copy_from_slice(&STREAM_TIMESCALE.to_be_bytes());
    bytes
}

fn stream_frame(
    payload: Vec<u8>,
    timestamp: u32,
    keyframe: bool,
) -> Result<(Bytes, Bytes), &'static str> {
    if payload.len() > MAX_ACCESS_UNIT_SIZE {
        return Err("encoded frame exceeds protocol limit");
    }
    let length = u32::try_from(payload.len()).map_err(|_| "encoded frame exceeds u32 limit")?;
    let mut header = [0; 12];
    header[..4].copy_from_slice(&length.to_be_bytes());
    header[4..8].copy_from_slice(&timestamp.to_be_bytes());
    header[8] = u8::from(keyframe);
    Ok((Bytes::copy_from_slice(&header), Bytes::from(payload)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_format_is_network_order_and_self_delimiting() {
        let preamble = stream_preamble(1920, 1080, 60);
        assert_eq!(&preamble[..8], STREAM_MAGIC);
        assert_eq!(
            u32::from_be_bytes(preamble[8..12].try_into().unwrap()),
            1920
        );
        assert_eq!(
            u32::from_be_bytes(preamble[20..24].try_into().unwrap()),
            STREAM_TIMESCALE
        );

        let (header, payload) = stream_frame(vec![1, 2, 3], 1500, true).unwrap();
        assert_eq!(u32::from_be_bytes(header[..4].try_into().unwrap()), 3);
        assert_eq!(u32::from_be_bytes(header[4..8].try_into().unwrap()), 1500);
        assert_eq!(&header[8..], &[1, 0, 0, 0]);
        assert_eq!(&payload[..], &[1, 2, 3]);
    }

    #[test]
    fn rejects_oversized_access_units() {
        assert!(stream_frame(vec![0; MAX_ACCESS_UNIT_SIZE + 1], 0, false).is_err());
    }
}
