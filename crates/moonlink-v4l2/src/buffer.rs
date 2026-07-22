use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::ptr::NonNull;

use crate::error::{V4l2Error, V4l2Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BufferIndex(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BufferCount(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BufferType {
    VideoCapture,
    VideoOutput,
    VideoCaptureMplane,
    VideoOutputMplane,
}

impl BufferType {
    pub(crate) fn raw(self) -> u32 {
        match self {
            Self::VideoCapture => 1,
            Self::VideoOutput => 2,
            Self::VideoCaptureMplane => 9,
            Self::VideoOutputMplane => 10,
        }
    }

    pub fn is_multiplanar(self) -> bool {
        matches!(self, Self::VideoCaptureMplane | Self::VideoOutputMplane)
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::VideoCapture => "VideoCapture",
            Self::VideoOutput => "VideoOutput",
            Self::VideoCaptureMplane => "VideoCaptureMplane",
            Self::VideoOutputMplane => "VideoOutputMplane",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BufferMemory {
    Mmap,
    DmaBuf,
}

impl BufferMemory {
    pub(crate) fn raw(self) -> u32 {
        match self {
            Self::Mmap => 1,
            Self::DmaBuf => 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlaneLayout {
    pub length: u32,
    pub bytes_used: u32,
    pub offset: u32,
}

#[derive(Debug)]
pub struct DmaBuffer {
    pub fd: OwnedFd,
    pub length: u32,
}

impl DmaBuffer {
    pub(crate) fn from_raw(fd: RawFd, length: u32) -> V4l2Result<Self> {
        if fd < 0 {
            return Err(V4l2Error::io("VIDIOC_EXPBUF"));
        }
        Ok(Self {
            fd: unsafe { OwnedFd::from_raw_fd(fd) },
            length,
        })
    }
}

#[derive(Debug)]
pub struct ExportedBuffer {
    pub index: BufferIndex,
    pub planes: Vec<DmaBuffer>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferPlane {
    pub fd: RawFd,
    pub bytes_used: u32,
    pub length: u32,
}

impl BufferPlane {
    pub fn from_dma(buffer: &DmaBuffer, bytes_used: u32) -> Self {
        Self {
            fd: buffer.fd.as_raw_fd(),
            bytes_used,
            length: buffer.length,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedBuffer {
    pub index: BufferIndex,
    pub planes: Vec<BufferPlane>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DequeuedBuffer {
    pub index: BufferIndex,
    pub bytes_used: u32,
    pub planes: Vec<PlaneLayout>,
    pub sequence: u32,
    pub flags: u32,
}

impl DequeuedBuffer {
    pub fn is_keyframe(&self) -> bool {
        self.flags & BufferFlag::KEYFRAME.bits() != 0
    }

    pub fn is_last(&self) -> bool {
        self.flags & BufferFlag::LAST.bits() != 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BufferFlag(u32);

impl BufferFlag {
    pub const KEYFRAME: Self = Self(0x0000_0008);
    pub const PFRAME: Self = Self(0x0000_0010);
    pub const BFRAME: Self = Self(0x0000_0020);
    pub const LAST: Self = Self(0x0010_0000);

    pub fn bits(self) -> u32 {
        self.0
    }
}

#[derive(Debug)]
pub struct MappedBuffer {
    ptr: NonNull<u8>,
    len: usize,
}

unsafe impl Send for MappedBuffer {}

impl MappedBuffer {
    pub(crate) fn map(fd: RawFd, length: u32, offset: u32) -> V4l2Result<Self> {
        if length == 0 {
            return Err(V4l2Error::InvalidValue(
                "cannot map zero-length V4L2 buffer".to_owned(),
            ));
        }
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                length as usize,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                offset as libc::off_t,
            )
        };
        if ptr == libc::MAP_FAILED {
            return Err(V4l2Error::io("mmap"));
        }
        Ok(Self {
            ptr: NonNull::new(ptr.cast::<u8>()).expect("mmap returned non-null"),
            len: length as usize,
        })
    }

    pub fn as_slice(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }
}

impl Drop for MappedBuffer {
    fn drop(&mut self) {
        let _ = unsafe { libc::munmap(self.ptr.as_ptr().cast(), self.len) };
    }
}
