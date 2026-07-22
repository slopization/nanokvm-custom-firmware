use std::path::PathBuf;

use async_trait::async_trait;
use moonlink_core::{
    MediaSession, MediaSessionConfig, MediaSessionError, VideoControlMessage, VideoFrame,
    create_access_unit,
};
use moonlink_protocol::VideoCodec;
use moonlink_v4l2::{
    BufferCount, BufferIndex, BufferMemory, BufferPlane, BufferType, Control, ControlId, Device,
    DmaBuffer, FourCc, FrameInterval, PixelFormat, Queue, QueuedBuffer, V4l2Error,
};

use crate::audio::AudioGenerator;
use crate::cli::{AudioSettings, StreamCodec};
use crate::hevc::{ParameterSets, describe_access_unit};

#[derive(Debug, Clone)]
pub struct V4l2Settings {
    pub capture_device: PathBuf,
    pub encoder_device: PathBuf,
    pub raw_fourcc: FourCc,
    pub codec: StreamCodec,
    pub buffer_count: u32,
    pub capture_width: u32,
    pub capture_height: u32,
    pub capture_fps: u32,
    pub frame_divider: u32,
    pub max_width: u32,
    pub max_height: u32,
    pub max_fps: u32,
    pub bitrate_kbps: Option<u32>,
    pub gop: u32,
    pub hevc_loop_filter: Option<i32>,
    pub repeat_headers: bool,
    pub aud: bool,
}

pub struct V4l2Session {
    capture: Device,
    encoder: Device,
    capture_queue: Queue,
    encoder_output_queue: Queue,
    encoder_capture_queue: Queue,
    capture_dma: Vec<Vec<DmaBuffer>>,
    encoded_maps: Vec<Vec<moonlink_v4l2::MappedBuffer>>,
    timestamp_90khz: u32,
    timestamp_step: u32,
    frame_divider: u32,
    encoder_output_started: bool,
    logged_waiting_for_frame: bool,
    logged_encoded_frame: bool,
    logged_force_keyframe_failure: bool,
    logged_requested_keyframes: u32,
    logged_truncated_frames: u64,
    hevc_parameter_sets: ParameterSets,
    audio: Option<AudioGenerator>,
}

impl V4l2Session {
    pub async fn start(
        settings: V4l2Settings,
        audio: AudioSettings,
        config: MediaSessionConfig,
    ) -> Result<Self, MediaSessionError> {
        validate_codec(&settings, config.video_codec)?;

        let capture = Device::open(&settings.capture_device).map_err(provider_error)?;
        let encoder = Device::open(&settings.encoder_device).map_err(provider_error)?;
        println!(
            "capture={:?} encoder={:?}",
            capture.query_capability().map_err(provider_error)?,
            encoder.query_capability().map_err(provider_error)?
        );

        let capture_queue = Queue::new(BufferType::VideoCapture, BufferMemory::Mmap, 1);
        let encoder_output_queue =
            Queue::new(BufferType::VideoOutputMplane, BufferMemory::DmaBuf, 1);
        let encoder_capture_queue =
            Queue::new(BufferType::VideoCaptureMplane, BufferMemory::Mmap, 1);

        let stream_mode = fixed_stream_mode(&settings, &config)?;
        let input_interval =
            FrameInterval::from_fps(settings.capture_fps).map_err(provider_error)?;
        let output_interval = FrameInterval::from_fps(stream_mode.fps).map_err(provider_error)?;
        let raw_format =
            PixelFormat::single_plane(stream_mode.width, stream_mode.height, settings.raw_fourcc);
        capture_queue
            .set_format(&capture, raw_format)
            .map_err(provider_error)?;
        set_frame_interval_if_supported(&capture_queue, &capture, input_interval)?;
        encoder_output_queue
            .set_format(&encoder, raw_format)
            .map_err(provider_error)?;
        set_frame_interval_if_supported(&encoder_output_queue, &encoder, output_interval)?;

        let encoded_format = PixelFormat::single_plane(
            stream_mode.width,
            stream_mode.height,
            match settings.codec {
                StreamCodec::Hevc => FourCc::HEVC,
                StreamCodec::H264 => FourCc::H264,
            },
        );
        encoder_capture_queue
            .set_format(&encoder, encoded_format)
            .map_err(provider_error)?;
        configure_encoder(&encoder, &settings)?;

        let buffers = BufferCount(settings.buffer_count.max(2));
        let capture_count = capture_queue
            .request_buffers(&capture, buffers)
            .map_err(provider_error)?;
        encoder_output_queue
            .request_buffers(&encoder, capture_count)
            .map_err(provider_error)?;
        let encoded_count = encoder_capture_queue
            .request_buffers(&encoder, buffers)
            .map_err(provider_error)?;

        let mut capture_dma = Vec::with_capacity(capture_count.0 as usize);
        for index in 0..capture_count.0 {
            let exported = capture_queue
                .export_buffer(&capture, BufferIndex(index))
                .map_err(provider_error)?;
            capture_dma.push(exported.planes);
            capture_queue
                .queue_buffer(&capture, empty_buffer(BufferIndex(index)))
                .await
                .map_err(provider_error)?;
        }

        let mut encoded_maps = Vec::with_capacity(encoded_count.0 as usize);
        for index in 0..encoded_count.0 {
            encoded_maps.push(
                encoder_capture_queue
                    .map_buffer(&encoder, BufferIndex(index))
                    .map_err(provider_error)?,
            );
            encoder_capture_queue
                .queue_buffer(&encoder, empty_buffer(BufferIndex(index)))
                .await
                .map_err(provider_error)?;
        }

        encoder_capture_queue
            .start_streaming(&encoder)
            .map_err(provider_error)?;
        capture_queue
            .start_streaming(&capture)
            .map_err(provider_error)?;
        force_next_frame_to_keyframe(&encoder);

        Ok(Self {
            capture,
            encoder,
            capture_queue,
            encoder_output_queue,
            encoder_capture_queue,
            capture_dma,
            encoded_maps,
            timestamp_90khz: 0,
            timestamp_step: 90_000 / stream_mode.fps.max(1),
            frame_divider: stream_mode.frame_divider,
            encoder_output_started: false,
            logged_waiting_for_frame: false,
            logged_encoded_frame: false,
            logged_force_keyframe_failure: false,
            logged_requested_keyframes: 0,
            logged_truncated_frames: 0,
            hevc_parameter_sets: ParameterSets::default(),
            audio: Some(AudioGenerator::new(
                audio,
                config.audio_packet_duration_ms * 48,
            )?),
        })
    }

    async fn encode_frame(&mut self) -> Result<Option<(Vec<u8>, bool)>, MediaSessionError> {
        if !self.logged_waiting_for_frame {
            eprintln!("waiting for first captured V4L2 frame");
            self.logged_waiting_for_frame = true;
        }
        for _ in 1..self.frame_divider {
            self.drop_capture_frame().await?;
        }
        let captured = self
            .capture_queue
            .dequeue_buffer(&self.capture)
            .await
            .map_err(provider_error)?;
        let capture_index = captured.index;
        let raw_planes = self
            .capture_dma
            .get(capture_index.0 as usize)
            .ok_or_else(|| {
                MediaSessionError::Provider("capture buffer index out of range".to_owned())
            })?;

        self.encoder_output_queue
            .queue_buffer(
                &self.encoder,
                QueuedBuffer {
                    index: capture_index,
                    planes: raw_planes
                        .iter()
                        .zip(captured.planes.iter())
                        .map(|(dma, plane)| BufferPlane::from_dma(dma, plane.bytes_used))
                        .collect(),
                },
            )
            .await
            .map_err(provider_error)?;
        if !self.encoder_output_started {
            self.encoder_output_queue
                .start_streaming(&self.encoder)
                .map_err(provider_error)?;
            self.encoder_output_started = true;
        }

        let (encoded, returned) = tokio::try_join!(
            self.encoder_capture_queue.dequeue_buffer(&self.encoder),
            self.encoder_output_queue.dequeue_buffer(&self.encoder),
        )
        .map_err(provider_error)?;

        self.capture_queue
            .queue_buffer(&self.capture, empty_buffer(returned.index))
            .await
            .map_err(provider_error)?;

        let bytes_used = encoded
            .planes
            .first()
            .map(|plane| plane.bytes_used)
            .unwrap_or(encoded.bytes_used) as usize;
        let mapped = self
            .encoded_maps
            .get(encoded.index.0 as usize)
            .and_then(|planes| planes.first())
            .ok_or_else(|| {
                MediaSessionError::Provider("encoded buffer index out of range".to_owned())
            })?;
        let mapped_len = mapped.as_slice().len();
        if bytes_used > mapped_len {
            self.encoder_capture_queue
                .queue_buffer(&self.encoder, empty_buffer(encoded.index))
                .await
                .map_err(provider_error)?;
            self.logged_truncated_frames = self.logged_truncated_frames.saturating_add(1);
            if self.logged_truncated_frames <= 5 || self.logged_truncated_frames.is_power_of_two() {
                eprintln!(
                    "dropping oversized encoded frame: bytes_used={bytes_used} mapped_len={mapped_len} count={}",
                    self.logged_truncated_frames
                );
            }
            return Ok(None);
        }
        let bytes = &mapped.as_slice()[..bytes_used];
        let mut frame = bytes.to_vec();
        let marker = encoded.is_keyframe();
        if marker {
            self.hevc_parameter_sets.update(&frame);
            frame = self
                .hevc_parameter_sets
                .prepend_if_missing(bytes::Bytes::from(frame))
                .to_vec();
        }
        if !self.logged_encoded_frame {
            eprintln!(
                "encoded first V4L2 frame: index={} bytes={} flags=0x{:08x} keyframe={} hevc_nals={}",
                encoded.index.0,
                bytes_used,
                encoded.flags,
                marker,
                describe_access_unit(&frame)
            );
            self.logged_encoded_frame = true;
        }
        if marker && self.logged_requested_keyframes > 0 && self.logged_requested_keyframes <= 5 {
            eprintln!(
                "encoded keyframe after request: bytes={} flags=0x{:08x} hevc_nals={}",
                bytes_used,
                encoded.flags,
                describe_access_unit(&frame)
            );
        }

        self.encoder_capture_queue
            .queue_buffer(&self.encoder, empty_buffer(encoded.index))
            .await
            .map_err(provider_error)?;

        Ok(Some((frame, marker)))
    }

    async fn drop_capture_frame(&mut self) -> Result<(), MediaSessionError> {
        let captured = self
            .capture_queue
            .dequeue_buffer(&self.capture)
            .await
            .map_err(provider_error)?;
        self.capture_queue
            .queue_buffer(&self.capture, empty_buffer(captured.index))
            .await
            .map_err(provider_error)
    }
}

#[async_trait]
impl MediaSession for V4l2Session {
    async fn next_video(&mut self) -> Result<Option<VideoFrame>, MediaSessionError> {
        let (bytes, marker) = loop {
            if let Some(frame) = self.encode_frame().await? {
                break frame;
            }
        };
        let timestamp = self.timestamp_90khz;
        self.timestamp_90khz = self.timestamp_90khz.wrapping_add(self.timestamp_step);
        Ok(Some(VideoFrame::AccessUnit(create_access_unit(
            bytes, timestamp, marker,
        ))))
    }

    async fn stop(&mut self) -> Result<(), MediaSessionError> {
        let _ = self.capture_queue.stop_streaming(&self.capture);
        let _ = self.encoder_output_queue.stop_streaming(&self.encoder);
        let _ = self.encoder_capture_queue.stop_streaming(&self.encoder);
        Ok(())
    }

    fn take_audio(&mut self) -> Option<Box<dyn moonlink_core::AudioSession>> {
        self.audio
            .take()
            .map(|audio| Box::new(audio) as Box<dyn moonlink_core::AudioSession>)
    }

    async fn handle_video_control(
        &mut self,
        message: VideoControlMessage,
    ) -> Result<(), MediaSessionError> {
        match message {
            VideoControlMessage::RequestIdrFrame
            | VideoControlMessage::InvalidateReferenceFrames { .. } => {}
        }

        if let Err(err) = self.encoder.set_control(Control {
            id: ControlId::FORCE_KEY_FRAME,
            value: 1,
        }) {
            if !self.logged_force_keyframe_failure {
                eprintln!("keyframe request control failed; relying on configured GOP: {err}");
                self.logged_force_keyframe_failure = true;
            }
        } else {
            self.logged_requested_keyframes = self.logged_requested_keyframes.saturating_add(1);
            if self.logged_requested_keyframes <= 5 {
                eprintln!("accepted keyframe request: {message:?}");
            }
        }
        Ok(())
    }
}

fn validate_mode(
    settings: &V4l2Settings,
    config: &MediaSessionConfig,
) -> Result<(), MediaSessionError> {
    if settings.capture_width > settings.max_width
        || settings.capture_height > settings.max_height
        || settings.capture_fps > settings.max_fps
    {
        return Err(MediaSessionError::Provider(format!(
            "configured capture mode {}x{}x{} exceeds limits {}x{}x{}",
            settings.capture_width,
            settings.capture_height,
            settings.capture_fps,
            settings.max_width,
            settings.max_height,
            settings.max_fps
        )));
    }
    if config.width > settings.max_width
        || config.height > settings.max_height
        || config.fps > settings.max_fps
    {
        return Err(MediaSessionError::Provider(format!(
            "requested mode {}x{}x{} exceeds limits {}x{}x{}",
            config.width,
            config.height,
            config.fps,
            settings.max_width,
            settings.max_height,
            settings.max_fps
        )));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct FixedStreamMode {
    width: u32,
    height: u32,
    fps: u32,
    frame_divider: u32,
}

fn fixed_stream_mode(
    settings: &V4l2Settings,
    config: &MediaSessionConfig,
) -> Result<FixedStreamMode, MediaSessionError> {
    validate_mode(settings, config)?;
    let output_fps = output_fps(settings)?;
    if config.width != settings.capture_width
        || config.height != settings.capture_height
        || config.fps != output_fps
    {
        return Err(MediaSessionError::Provider(format!(
            "unsupported stream mode {}x{}x{}; only {}x{}x{} is available",
            config.width,
            config.height,
            config.fps,
            settings.capture_width,
            settings.capture_height,
            output_fps
        )));
    }
    Ok(FixedStreamMode {
        width: settings.capture_width,
        height: settings.capture_height,
        fps: output_fps,
        frame_divider: settings.frame_divider,
    })
}

pub fn output_fps(settings: &V4l2Settings) -> Result<u32, MediaSessionError> {
    if settings.frame_divider == 0 {
        return Err(MediaSessionError::Provider(
            "frame divider must be a positive integer".to_owned(),
        ));
    }
    if settings.capture_fps % settings.frame_divider != 0 {
        return Err(MediaSessionError::Provider(format!(
            "capture FPS {} is not divisible by frame divider {}",
            settings.capture_fps, settings.frame_divider
        )));
    }
    Ok(settings.capture_fps / settings.frame_divider)
}

fn validate_codec(settings: &V4l2Settings, requested: VideoCodec) -> Result<(), MediaSessionError> {
    let expected = match settings.codec {
        StreamCodec::Hevc => VideoCodec::Hevc,
        StreamCodec::H264 => VideoCodec::H264,
    };
    if requested != expected {
        return Err(MediaSessionError::Provider(format!(
            "V4L2 provider supplies {expected:?}, Moonlight requested {requested:?}"
        )));
    }
    if settings.codec == StreamCodec::H264 {
        return Err(MediaSessionError::Provider(
            "native H.264 encoder support is reserved by the codec abstraction but not enabled yet"
                .to_owned(),
        ));
    }
    Ok(())
}

fn configure_encoder(encoder: &Device, settings: &V4l2Settings) -> Result<(), MediaSessionError> {
    if let Some(bitrate_kbps) = settings.bitrate_kbps {
        encoder
            .set_control(Control {
                id: ControlId::FRAME_RC_ENABLE,
                value: 1,
            })
            .map_err(provider_error)?;
        encoder
            .set_control(Control {
                id: ControlId::BITRATE,
                value: bitrate_kbps.saturating_mul(1000) as i32,
            })
            .map_err(provider_error)?;
    }
    encoder
        .set_control(Control {
            id: ControlId::GOP_SIZE,
            value: settings.gop as i32,
        })
        .map_err(provider_error)?;
    if settings.repeat_headers {
        let _ = encoder.set_control(Control {
            id: ControlId::REPEAT_SEQ_HEADER,
            value: 1,
        });
    }
    if settings.codec == StreamCodec::Hevc {
        if let Some(value) = settings.hevc_loop_filter {
            set_optional_encoder_control(
                encoder,
                Control {
                    id: ControlId::HEVC_LOOP_FILTER_MODE,
                    value,
                },
                "HEVC loop filter",
            );
            match encoder.get_control(ControlId::HEVC_LOOP_FILTER_MODE) {
                Ok(current) => eprintln!("HEVC loop filter mode now {current}"),
                Err(err) => eprintln!("HEVC loop filter readback failed: {err}"),
            }
        }
        set_optional_encoder_control(
            encoder,
            Control {
                id: ControlId::PREPEND_SPSPPS_TO_IDR,
                value: 1,
            },
            "prepend SPS/PPS to IDR",
        );
    }
    if settings.aud {
        println!("AUD insertion requested; use the WAVE driver control if your kernel exposes one");
    }
    Ok(())
}

fn force_next_frame_to_keyframe(encoder: &Device) {
    set_optional_encoder_control(
        encoder,
        Control {
            id: ControlId::FORCE_KEY_FRAME,
            value: 1,
        },
        "initial keyframe request",
    );
}

fn set_optional_encoder_control(encoder: &Device, control: Control, description: &str) {
    if let Err(err) = encoder.set_control(control) {
        eprintln!("{description} control failed: {err}");
    }
}

fn set_frame_interval_if_supported(
    queue: &Queue,
    device: &Device,
    interval: FrameInterval,
) -> Result<(), MediaSessionError> {
    match queue.set_frame_interval(device, interval) {
        Ok(()) => Ok(()),
        Err(err) if is_unsupported_ioctl(&err) => {
            eprintln!(
                "{} does not support VIDIOC_S_PARM; continuing without driver FPS control",
                device.path().display()
            );
            Ok(())
        }
        Err(err) => Err(provider_error(err)),
    }
}

fn is_unsupported_ioctl(err: &V4l2Error) -> bool {
    const EINVAL: i32 = 22;
    const ENOTTY: i32 = 25;
    matches!(err.raw_os_error(), Some(ENOTTY | EINVAL))
}

fn empty_buffer(index: BufferIndex) -> QueuedBuffer {
    QueuedBuffer {
        index,
        planes: Vec::new(),
    }
}

fn provider_error(err: impl std::fmt::Display) -> MediaSessionError {
    MediaSessionError::Provider(err.to_string())
}
