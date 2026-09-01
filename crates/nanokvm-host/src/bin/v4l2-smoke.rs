use std::path::PathBuf;

use clap::{Parser, ValueEnum};
use moonlink_v4l2::{
    BufferCount, BufferIndex, BufferMemory, BufferPlane, BufferType, Device, FourCc, FrameInterval,
    PixelFormat, Queue, QueuedBuffer,
};

#[derive(Debug, Parser)]
struct Args {
    #[arg(long)]
    capture_device: PathBuf,
    #[arg(long)]
    codec_device: PathBuf,
    #[arg(long, default_value_t = 640)]
    width: u32,
    #[arg(long, default_value_t = 480)]
    height: u32,
    #[arg(long, default_value_t = 30)]
    fps: u32,
    #[arg(long, default_value = "UYVY")]
    raw_fourcc: FourCc,
    #[arg(long, default_value = "FWHT")]
    encoded_fourcc: FourCc,
    #[arg(long, value_enum, default_value = "capture")]
    capture_type: QueueTypeArg,
    #[arg(long, value_enum, default_value = "output")]
    codec_output_type: QueueTypeArg,
    #[arg(long, value_enum, default_value = "capture")]
    codec_capture_type: QueueTypeArg,
    #[arg(long, default_value_t = 4)]
    buffer_count: u32,
    #[arg(long, default_value_t = 30)]
    frames: u32,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum QueueTypeArg {
    Capture,
    Output,
    CaptureMplane,
    OutputMplane,
}

impl QueueTypeArg {
    fn buffer_type(self) -> BufferType {
        match self {
            Self::Capture => BufferType::VideoCapture,
            Self::Output => BufferType::VideoOutput,
            Self::CaptureMplane => BufferType::VideoCaptureMplane,
            Self::OutputMplane => BufferType::VideoOutputMplane,
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let capture = Device::open(&args.capture_device)?;
    let codec = Device::open(&args.codec_device)?;
    println!("capture: {:?}", capture.query_capability()?);
    println!("codec: {:?}", codec.query_capability()?);

    let capture_queue = Queue::new(args.capture_type.buffer_type(), BufferMemory::Mmap, 1);
    let codec_output_queue = Queue::new(
        args.codec_output_type.buffer_type(),
        BufferMemory::DmaBuf,
        1,
    );
    let codec_capture_queue =
        Queue::new(args.codec_capture_type.buffer_type(), BufferMemory::Mmap, 1);

    let interval = FrameInterval::from_fps(args.fps)?;
    let raw_format = PixelFormat::single_plane(args.width, args.height, args.raw_fourcc);
    let encoded_format = PixelFormat::single_plane(args.width, args.height, args.encoded_fourcc);

    println!(
        "capture format: {:?}",
        capture_queue.set_format(&capture, raw_format)?
    );
    capture_queue.set_frame_interval(&capture, interval)?;
    println!(
        "codec input format: {:?}",
        codec_output_queue.set_format(&codec, raw_format)?
    );
    codec_output_queue.set_frame_interval(&codec, interval)?;
    println!(
        "codec output format: {:?}",
        codec_capture_queue.set_format(&codec, encoded_format)?
    );

    let requested = BufferCount(args.buffer_count.max(2));
    let capture_count = capture_queue.request_buffers(&capture, requested)?;
    codec_output_queue.request_buffers(&codec, capture_count)?;
    let encoded_count = codec_capture_queue.request_buffers(&codec, requested)?;

    let mut capture_dma = Vec::new();
    for index in 0..capture_count.0 {
        let index = BufferIndex(index);
        let exported = capture_queue.export_buffer(&capture, index)?;
        capture_dma.push(exported.planes);
        capture_queue
            .queue_buffer(&capture, empty_buffer(index))
            .await?;
    }

    let mut encoded_maps = Vec::new();
    for index in 0..encoded_count.0 {
        let index = BufferIndex(index);
        encoded_maps.push(codec_capture_queue.map_buffer(&codec, index)?);
        codec_capture_queue
            .queue_buffer(&codec, empty_buffer(index))
            .await?;
    }

    codec_capture_queue.start_streaming(&codec)?;
    codec_output_queue.start_streaming(&codec)?;
    capture_queue.start_streaming(&capture)?;

    let mut encoded_total = 0_u64;
    for frame in 0..args.frames {
        let captured = capture_queue.dequeue_buffer(&capture).await?;
        let capture_index = captured.index;
        let planes = capture_dma
            .get(capture_index.0 as usize)
            .ok_or("capture index out of range")?
            .iter()
            .zip(captured.planes.iter())
            .map(|(dma, plane)| BufferPlane::from_dma(dma, plane.bytes_used))
            .collect();
        codec_output_queue
            .queue_buffer(
                &codec,
                QueuedBuffer {
                    index: capture_index,
                    planes,
                },
            )
            .await?;

        let (encoded, returned) = tokio::try_join!(
            codec_capture_queue.dequeue_buffer(&codec),
            codec_output_queue.dequeue_buffer(&codec),
        )?;

        capture_queue
            .queue_buffer(&capture, empty_buffer(returned.index))
            .await?;

        let bytes_used = encoded
            .planes
            .first()
            .map(|plane| plane.bytes_used)
            .unwrap_or(encoded.bytes_used) as usize;
        let mapped = encoded_maps
            .get(encoded.index.0 as usize)
            .and_then(|planes| planes.first())
            .ok_or("encoded index out of range")?;
        let usable = bytes_used.min(mapped.as_slice().len());
        encoded_total += usable as u64;
        println!(
            "frame={} capture_index={} encoded_index={} bytes={}",
            frame, capture_index.0, encoded.index.0, usable
        );

        codec_capture_queue
            .queue_buffer(&codec, empty_buffer(encoded.index))
            .await?;
    }

    let _ = capture_queue.stop_streaming(&capture);
    let _ = codec_output_queue.stop_streaming(&codec);
    let _ = codec_capture_queue.stop_streaming(&codec);
    println!("encoded_total={encoded_total}");
    Ok(())
}

fn empty_buffer(index: BufferIndex) -> QueuedBuffer {
    QueuedBuffer {
        index,
        planes: Vec::new(),
    }
}
