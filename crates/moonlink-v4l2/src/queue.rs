use crate::buffer::{
    BufferCount, BufferIndex, BufferMemory, BufferType, DequeuedBuffer, DmaBuffer, ExportedBuffer,
    MappedBuffer, PlaneLayout, QueuedBuffer,
};
use crate::device::Device;
use crate::error::{V4l2Error, V4l2Result};
use crate::format::{FrameInterval, PixelFormat};
use crate::sys;

const MAX_PLANES: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueueConfig {
    pub buffer_type: BufferType,
    pub memory: BufferMemory,
    pub buffers: BufferCount,
    pub planes: u8,
}

#[derive(Debug)]
pub struct Queue {
    pub buffer_type: BufferType,
    pub memory: BufferMemory,
    pub planes: u8,
}

impl Queue {
    pub fn new(buffer_type: BufferType, memory: BufferMemory, planes: u8) -> Self {
        Self {
            buffer_type,
            memory,
            planes: planes.max(1),
        }
    }

    pub fn set_format(&self, device: &Device, format: PixelFormat) -> V4l2Result<PixelFormat> {
        let mut raw = sys::v4l2_format::from_pixel_format(self.buffer_type, format);
        let ret = unsafe { libc::ioctl(device.raw_fd(), sys::vidioc_s_fmt(), &mut raw) };
        if ret < 0 {
            return Err(V4l2Error::from_io(
                format!(
                    "VIDIOC_S_FMT {} {}x{} {}",
                    self.buffer_type.name(),
                    format.size.width,
                    format.size.height,
                    format.fourcc
                ),
                std::io::Error::last_os_error(),
            ));
        }
        raw.to_pixel_format(self.buffer_type)
    }

    pub fn set_frame_interval(&self, device: &Device, interval: FrameInterval) -> V4l2Result<()> {
        let mut parm = sys::v4l2_streamparm::from_interval(self.buffer_type, interval);
        let ret = unsafe { libc::ioctl(device.raw_fd(), sys::vidioc_s_parm(), &mut parm) };
        if ret < 0 {
            return Err(V4l2Error::io("VIDIOC_S_PARM"));
        }
        Ok(())
    }

    pub fn request_buffers(&self, device: &Device, count: BufferCount) -> V4l2Result<BufferCount> {
        let mut req = sys::v4l2_requestbuffers {
            count: count.0,
            type_: self.buffer_type.raw(),
            memory: self.memory.raw(),
            capabilities: 0,
            flags: 0,
            reserved: [0; 1],
        };
        let ret = unsafe { libc::ioctl(device.raw_fd(), sys::vidioc_reqbufs(), &mut req) };
        if ret < 0 {
            return Err(V4l2Error::io("VIDIOC_REQBUFS"));
        }
        Ok(BufferCount(req.count))
    }

    pub fn query_buffer(&self, device: &Device, index: BufferIndex) -> V4l2Result<DequeuedBuffer> {
        let mut planes = [sys::v4l2_plane::default(); MAX_PLANES];
        let mut raw = sys::v4l2_buffer::new(
            self.buffer_type,
            self.memory,
            index,
            self.planes,
            planes.as_mut_ptr(),
        );
        let ret = unsafe { libc::ioctl(device.raw_fd(), sys::vidioc_querybuf(), &mut raw) };
        if ret < 0 {
            return Err(V4l2Error::io("VIDIOC_QUERYBUF"));
        }
        Ok(dequeued_from_raw(&raw, &planes, self.buffer_type))
    }

    pub fn export_buffer(&self, device: &Device, index: BufferIndex) -> V4l2Result<ExportedBuffer> {
        let mut planes = Vec::new();
        let plane_count = if self.buffer_type.is_multiplanar() {
            self.planes
        } else {
            1
        };
        for plane in 0..plane_count {
            let mut export = sys::v4l2_exportbuffer {
                type_: self.buffer_type.raw(),
                index: index.0,
                plane: u32::from(plane),
                flags: libc::O_CLOEXEC as u32,
                fd: -1,
                reserved: [0; 11],
            };
            let ret = unsafe { libc::ioctl(device.raw_fd(), sys::vidioc_expbuf(), &mut export) };
            if ret < 0 {
                return Err(V4l2Error::io("VIDIOC_EXPBUF"));
            }
            let layout = self.query_buffer(device, index)?;
            let length = layout
                .planes
                .get(plane as usize)
                .map(|plane| plane.length)
                .unwrap_or(layout.bytes_used);
            planes.push(DmaBuffer::from_raw(export.fd, length)?);
        }
        Ok(ExportedBuffer { index, planes })
    }

    pub fn map_buffer(&self, device: &Device, index: BufferIndex) -> V4l2Result<Vec<MappedBuffer>> {
        let layout = self.query_buffer(device, index)?;
        layout
            .planes
            .iter()
            .map(|plane| MappedBuffer::map(device.raw_fd(), plane.length, plane.offset))
            .collect()
    }

    pub async fn queue_buffer(&self, device: &Device, buffer: QueuedBuffer) -> V4l2Result<()> {
        loop {
            match self.try_queue_buffer(device, &buffer) {
                Err(V4l2Error::NotReady) => device.wait_writable().await?,
                result => return result,
            }
        }
    }

    pub fn try_queue_buffer(&self, device: &Device, buffer: &QueuedBuffer) -> V4l2Result<()> {
        let mut planes = [sys::v4l2_plane::default(); MAX_PLANES];
        for (index, source) in buffer.planes.iter().enumerate() {
            planes[index].bytesused = source.bytes_used;
            planes[index].length = source.length;
            planes[index].m.fd = source.fd;
        }
        let mut raw = sys::v4l2_buffer::new(
            self.buffer_type,
            self.memory,
            buffer.index,
            self.planes,
            planes.as_mut_ptr(),
        );
        if !self.buffer_type.is_multiplanar() {
            raw.bytesused = buffer
                .planes
                .first()
                .map(|plane| plane.bytes_used)
                .unwrap_or(0);
            raw.length = buffer.planes.first().map(|plane| plane.length).unwrap_or(0);
            raw.m.fd = buffer.planes.first().map(|plane| plane.fd).unwrap_or(-1);
        }
        let ret = unsafe { libc::ioctl(device.raw_fd(), sys::vidioc_qbuf(), &mut raw) };
        if ret < 0 {
            return errno_result("VIDIOC_QBUF");
        }
        Ok(())
    }

    pub async fn dequeue_buffer(&self, device: &Device) -> V4l2Result<DequeuedBuffer> {
        loop {
            match self.try_dequeue_buffer(device) {
                Err(V4l2Error::NotReady) => device.wait_readable().await?,
                result => return result,
            }
        }
    }

    pub fn try_dequeue_buffer(&self, device: &Device) -> V4l2Result<DequeuedBuffer> {
        let mut planes = [sys::v4l2_plane::default(); MAX_PLANES];
        let mut raw = sys::v4l2_buffer::new(
            self.buffer_type,
            self.memory,
            BufferIndex(0),
            self.planes,
            planes.as_mut_ptr(),
        );
        let ret = unsafe { libc::ioctl(device.raw_fd(), sys::vidioc_dqbuf(), &mut raw) };
        if ret < 0 {
            return errno_result("VIDIOC_DQBUF");
        }
        Ok(dequeued_from_raw(&raw, &planes, self.buffer_type))
    }

    pub fn start_streaming(&self, device: &Device) -> V4l2Result<()> {
        stream(device, self.buffer_type, true)
    }

    pub fn stop_streaming(&self, device: &Device) -> V4l2Result<()> {
        stream(device, self.buffer_type, false)
    }
}

fn stream(device: &Device, buffer_type: BufferType, enabled: bool) -> V4l2Result<()> {
    let mut type_ = buffer_type.raw() as libc::c_int;
    let request = if enabled {
        sys::vidioc_streamon()
    } else {
        sys::vidioc_streamoff()
    };
    let ret = unsafe { libc::ioctl(device.raw_fd(), request, &mut type_) };
    if ret < 0 {
        return Err(V4l2Error::io(if enabled {
            format!("VIDIOC_STREAMON {:?}", buffer_type)
        } else {
            format!("VIDIOC_STREAMOFF {:?}", buffer_type)
        }));
    }
    Ok(())
}

fn dequeued_from_raw(
    raw: &sys::v4l2_buffer,
    raw_planes: &[sys::v4l2_plane],
    buffer_type: BufferType,
) -> DequeuedBuffer {
    let planes = if buffer_type.is_multiplanar() {
        raw_planes
            .iter()
            .take(raw.length as usize)
            .map(|plane| PlaneLayout {
                length: plane.length,
                bytes_used: plane.bytesused,
                offset: unsafe { plane.m.mem_offset },
            })
            .collect()
    } else {
        vec![PlaneLayout {
            length: raw.length,
            bytes_used: raw.bytesused,
            offset: unsafe { raw.m.offset },
        }]
    };
    DequeuedBuffer {
        index: BufferIndex(raw.index),
        bytes_used: raw.bytesused,
        planes,
        sequence: raw.sequence,
        flags: raw.flags,
    }
}

fn errno_result<T>(op: &'static str) -> V4l2Result<T> {
    let err = std::io::Error::last_os_error();
    if err.raw_os_error() == Some(libc::EAGAIN) {
        Err(V4l2Error::NotReady)
    } else {
        Err(V4l2Error::from_io(op, err))
    }
}
