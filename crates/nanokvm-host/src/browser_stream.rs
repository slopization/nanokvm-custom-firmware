use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use axum::body::Body;
use axum::http::header::CONTENT_TYPE;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use tokio::sync::{Mutex, watch};
use tokio_stream::wrappers::ReceiverStream;

use crate::audio::{AudioGenerator, USB_AUDIO_CHANNELS, USB_AUDIO_SAMPLE_RATE};
use crate::cli::{AudioSettings, AudioSource, StreamCodec};
use crate::session_arbiter::{SessionArbiter, SessionOwner};
use crate::settings::SettingsStore;
use crate::v4l2_provider::{V4l2Session, V4l2Settings, output_fps};
use moonlink_core::{AudioSession, MediaSession, MediaSessionConfig, VideoControlMessage};
use moonlink_protocol::VideoCodec;

const STREAM_MAGIC: &[u8; 8] = b"NKHEVC01";
const STREAM_TIMESCALE: u32 = 90_000;
const STREAM_QUEUE_FRAMES: usize = 4;
const MAX_ACCESS_UNIT_SIZE: usize = 32 * 1024 * 1024;
const WEB_AUDIO_FRAME_SAMPLES: u32 = 960;
const OPUS_STREAM_MAGIC: &[u8; 8] = b"NKOPUS01";
const OPUS_STREAM_QUEUE_FRAMES: usize = 4;

#[derive(Clone)]
pub struct BrowserCapture {
    arbiter: SessionArbiter,
    settings: SettingsStore,
    v4l2: Option<V4l2Settings>,
    audio: AudioSettings,
    audio_active: Arc<AtomicBool>,
    active_video: Arc<Mutex<Option<ActiveVideoStream>>>,
}

impl BrowserCapture {
    pub fn new(
        arbiter: SessionArbiter,
        settings: SettingsStore,
        v4l2: Option<V4l2Settings>,
        audio: AudioSettings,
    ) -> Self {
        Self {
            arbiter,
            settings,
            v4l2,
            audio,
            audio_active: Arc::new(AtomicBool::new(false)),
            active_video: Arc::new(Mutex::new(None)),
        }
    }

    pub async fn stream(&self) -> Response {
        match self.start().await {
            Ok(response) => response,
            Err((status, message)) => (status, message).into_response(),
        }
    }

    pub async fn audio_stream(&self) -> Response {
        if self.arbiter.owner() != Some(SessionOwner::Web) {
            return (
                StatusCode::CONFLICT,
                "web video capture must be active before audio",
            )
                .into_response();
        }
        let mut audio_settings = self.audio.clone();
        self.settings.get().await.apply_audio(&mut audio_settings);
        if audio_settings.source == AudioSource::None {
            return (StatusCode::NOT_IMPLEMENTED, "browser audio is disabled").into_response();
        }
        if self
            .audio_active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return (StatusCode::CONFLICT, "browser audio is already active").into_response();
        }
        let audio_lease = AudioStreamLease(self.audio_active.clone());
        let mut audio = match AudioGenerator::new(audio_settings, WEB_AUDIO_FRAME_SAMPLES) {
            Ok(audio) => audio,
            Err(error) => {
                return (StatusCode::SERVICE_UNAVAILABLE, error.to_string()).into_response();
            }
        };
        let (sender, receiver) = tokio::sync::mpsc::channel::<Result<Bytes, Infallible>>(
            OPUS_STREAM_QUEUE_FRAMES * 2 + 1,
        );
        sender
            .try_send(Ok(Bytes::copy_from_slice(&opus_stream_preamble())))
            .expect("new audio channel");
        let arbiter = self.arbiter.clone();
        tokio::spawn(async move {
            let _audio_lease = audio_lease;
            loop {
                let frame = tokio::select! {
                    _ = sender.closed() => break,
                    _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {
                        if arbiter.owner() != Some(SessionOwner::Web) {
                            break;
                        }
                        continue;
                    }
                    frame = audio.next_audio() => frame,
                };
                let frame = match frame {
                    Ok(Some(frame)) => frame,
                    Ok(None) => continue,
                    Err(error) => {
                        eprintln!("browser audio stopped: {error}");
                        break;
                    }
                };
                let (header, payload) = match opus_stream_packet(frame.data, frame.pts) {
                    Ok(packet) => packet,
                    Err(error) => {
                        eprintln!("browser Opus stream stopped: {error}");
                        break;
                    }
                };
                match sender.try_reserve_many(2) {
                    Ok(mut permits) => {
                        permits.next().expect("two reserved slots").send(Ok(header));
                        permits
                            .next()
                            .expect("two reserved slots")
                            .send(Ok(payload));
                    }
                    Err(tokio::sync::mpsc::error::TrySendError::Full(())) => {}
                    Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => break,
                }
                tokio::task::yield_now().await;
            }
        });

        let mut response = Response::new(Body::from_stream(ReceiverStream::new(receiver)));
        let headers = response.headers_mut();
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/vnd.nanokvm.opus"),
        );
        headers.insert(
            "cache-control",
            HeaderValue::from_static("no-store, no-transform"),
        );
        headers.insert("x-accel-buffering", HeaderValue::from_static("no"));
        response
    }

    async fn start(&self) -> Result<Response, (StatusCode, String)> {
        let Some(mut settings) = self.v4l2.clone() else {
            return Err((
                StatusCode::NOT_IMPLEMENTED,
                "browser capture requires the V4L2 source".to_owned(),
            ));
        };
        if self.arbiter.owner() == Some(SessionOwner::Moonlight) {
            return Err((
                StatusCode::CONFLICT,
                "capture is owned by Moonlight".to_owned(),
            ));
        }
        let mut active_video = self.active_video.lock().await;
        if let Some(previous) = active_video.take() {
            previous.cancel_and_wait().await;
        }
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
        let (active, mut cancellation, completion) = ActiveVideoStream::new();
        *active_video = Some(active);
        tokio::spawn(async move {
            let mut waiting_for_keyframe = false;
            let mut stream_timestamp = 0_u32;
            let timestamp_step = STREAM_TIMESCALE / fps.max(1);
            loop {
                let frame = match next_frame(&mut session, &sender, &mut cancellation).await {
                    None => break,
                    Some(frame) => frame,
                };
                let frame = match frame {
                    Ok(Some(frame)) => frame,
                    Ok(None) => continue,
                    Err(err) => {
                        eprintln!("browser capture stopped: {err}");
                        break;
                    }
                };
                if waiting_for_keyframe && !frame.keyframe {
                    tokio::task::yield_now().await;
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
                        if !waiting_for_keyframe {
                            let _ = session
                                .handle_video_control(VideoControlMessage::RequestIdrFrame)
                                .await;
                        }
                        waiting_for_keyframe = true;
                    }
                    Err(tokio::sync::mpsc::error::TrySendError::Closed(())) => break,
                }
                tokio::task::yield_now().await;
            }
            let _ = session.stop().await;
            drop(lease);
            drop(completion);
        });
        drop(active_video);

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

struct ActiveVideoStream {
    cancellation: watch::Sender<bool>,
    completion: watch::Receiver<bool>,
}

impl ActiveVideoStream {
    fn new() -> (Self, watch::Receiver<bool>, VideoStreamCompletion) {
        let (cancellation, cancellation_rx) = watch::channel(false);
        let (completion_tx, completion) = watch::channel(false);
        (
            Self {
                cancellation,
                completion,
            },
            cancellation_rx,
            VideoStreamCompletion(completion_tx),
        )
    }

    async fn cancel_and_wait(mut self) {
        let _ = self.cancellation.send(true);
        while !*self.completion.borrow() && self.completion.changed().await.is_ok() {}
    }
}

struct VideoStreamCompletion(watch::Sender<bool>);

impl Drop for VideoStreamCompletion {
    fn drop(&mut self) {
        let _ = self.0.send(true);
    }
}

async fn next_frame(
    session: &mut V4l2Session,
    sender: &tokio::sync::mpsc::Sender<Result<Bytes, Infallible>>,
    cancellation: &mut watch::Receiver<bool>,
) -> Option<Result<Option<crate::v4l2_provider::EncodedFrame>, moonlink_core::MediaSessionError>> {
    let frame = session.next_encoded();
    tokio::pin!(frame);
    loop {
        tokio::select! {
            biased;
            result = cancellation.changed() => {
                if result.is_err() || *cancellation.borrow() {
                    return None;
                }
            }
            _ = sender.closed() => return None,
            result = &mut frame => return Some(result),
        }
    }
}

struct AudioStreamLease(Arc<AtomicBool>);

impl Drop for AudioStreamLease {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

fn opus_stream_preamble() -> [u8; 16] {
    let mut bytes = [0; 16];
    bytes[..8].copy_from_slice(OPUS_STREAM_MAGIC);
    bytes[8..12].copy_from_slice(&USB_AUDIO_SAMPLE_RATE.to_be_bytes());
    bytes[12..14].copy_from_slice(&u16::from(USB_AUDIO_CHANNELS).to_be_bytes());
    bytes[14..16].copy_from_slice(&(WEB_AUDIO_FRAME_SAMPLES as u16).to_be_bytes());
    bytes
}

fn opus_stream_packet(
    payload: Vec<u8>,
    timestamp: Option<i64>,
) -> Result<(Bytes, Bytes), &'static str> {
    let length = u32::try_from(payload.len()).map_err(|_| "Opus packet exceeds u32 limit")?;
    let timestamp = timestamp.unwrap_or(0);
    let timestamp = u32::try_from(timestamp).map_err(|_| "invalid Opus timestamp")?;
    let mut header = [0; 8];
    header[..4].copy_from_slice(&length.to_be_bytes());
    header[4..8].copy_from_slice(&timestamp.to_be_bytes());
    Ok((Bytes::copy_from_slice(&header), Bytes::from(payload)))
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

    #[test]
    fn opus_stream_is_timestamped_and_describes_its_format() {
        let preamble = opus_stream_preamble();
        assert_eq!(&preamble[..8], OPUS_STREAM_MAGIC);
        assert_eq!(
            u32::from_be_bytes(preamble[8..12].try_into().unwrap()),
            48_000
        );
        assert_eq!(u16::from_be_bytes(preamble[12..14].try_into().unwrap()), 2);
        assert_eq!(
            u16::from_be_bytes(preamble[14..16].try_into().unwrap()),
            960
        );

        let (header, payload) = opus_stream_packet(vec![1, 2, 3], Some(960)).unwrap();
        assert_eq!(u32::from_be_bytes(header[..4].try_into().unwrap()), 3);
        assert_eq!(u32::from_be_bytes(header[4..8].try_into().unwrap()), 960);
        assert_eq!(&payload[..], &[1, 2, 3]);
    }
}
