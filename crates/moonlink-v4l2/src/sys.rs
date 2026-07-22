use crate::buffer::{BufferIndex, BufferMemory, BufferType};
use crate::error::{V4l2Error, V4l2Result};
use crate::format::{Field, FrameInterval, PixelFormat};

const IOC_NRBITS: u64 = 8;
const IOC_TYPEBITS: u64 = 8;
const IOC_SIZEBITS: u64 = 14;

const IOC_NRSHIFT: u64 = 0;
const IOC_TYPESHIFT: u64 = IOC_NRSHIFT + IOC_NRBITS;
const IOC_SIZESHIFT: u64 = IOC_TYPESHIFT + IOC_TYPEBITS;
const IOC_DIRSHIFT: u64 = IOC_SIZESHIFT + IOC_SIZEBITS;

const IOC_WRITE: u64 = 1;
const IOC_READ: u64 = 2;

const fn ioc(dir: u64, type_: u64, nr: u64, size: u64) -> libc::c_ulong {
    ((dir << IOC_DIRSHIFT)
        | (type_ << IOC_TYPESHIFT)
        | (nr << IOC_NRSHIFT)
        | (size << IOC_SIZESHIFT)) as libc::c_ulong
}

const fn ior<T>(type_: u8, nr: u8) -> libc::c_ulong {
    ioc(
        IOC_READ,
        type_ as u64,
        nr as u64,
        std::mem::size_of::<T>() as u64,
    )
}

const fn iow<T>(type_: u8, nr: u8) -> libc::c_ulong {
    ioc(
        IOC_WRITE,
        type_ as u64,
        nr as u64,
        std::mem::size_of::<T>() as u64,
    )
}

const fn iowr<T>(type_: u8, nr: u8) -> libc::c_ulong {
    ioc(
        IOC_READ | IOC_WRITE,
        type_ as u64,
        nr as u64,
        std::mem::size_of::<T>() as u64,
    )
}

pub(crate) fn vidioc_querycap() -> libc::c_ulong {
    ior::<v4l2_capability>(b'V', 0)
}

pub(crate) fn vidioc_s_fmt() -> libc::c_ulong {
    iowr::<v4l2_format>(b'V', 5)
}

pub(crate) fn vidioc_reqbufs() -> libc::c_ulong {
    iowr::<v4l2_requestbuffers>(b'V', 8)
}

pub(crate) fn vidioc_querybuf() -> libc::c_ulong {
    iowr::<v4l2_buffer>(b'V', 9)
}

pub(crate) fn vidioc_qbuf() -> libc::c_ulong {
    iowr::<v4l2_buffer>(b'V', 15)
}

pub(crate) fn vidioc_dqbuf() -> libc::c_ulong {
    iowr::<v4l2_buffer>(b'V', 17)
}

pub(crate) fn vidioc_streamon() -> libc::c_ulong {
    iow::<libc::c_int>(b'V', 18)
}

pub(crate) fn vidioc_streamoff() -> libc::c_ulong {
    iow::<libc::c_int>(b'V', 19)
}

pub(crate) fn vidioc_s_parm() -> libc::c_ulong {
    iowr::<v4l2_streamparm>(b'V', 22)
}

pub(crate) fn vidioc_s_ctrl() -> libc::c_ulong {
    iowr::<v4l2_control>(b'V', 28)
}

pub(crate) fn vidioc_g_ctrl() -> libc::c_ulong {
    iowr::<v4l2_control>(b'V', 27)
}

pub(crate) fn vidioc_expbuf() -> libc::c_ulong {
    iowr::<v4l2_exportbuffer>(b'V', 16)
}

pub(crate) const V4L2_CID_CODEC_BASE: u32 = 0x0099_0900;
pub(crate) const V4L2_CID_MPEG_VIDEO_BASE: u32 = V4L2_CID_CODEC_BASE;
pub(crate) const V4L2_CID_MPEG_VIDEO_BITRATE: u32 = V4L2_CID_MPEG_VIDEO_BASE + 207;
pub(crate) const V4L2_CID_MPEG_VIDEO_GOP_SIZE: u32 = V4L2_CID_MPEG_VIDEO_BASE + 203;
pub(crate) const V4L2_CID_MPEG_VIDEO_FRAME_RC_ENABLE: u32 = V4L2_CID_MPEG_VIDEO_BASE + 215;
pub(crate) const V4L2_CID_MPEG_VIDEO_FORCE_KEY_FRAME: u32 = V4L2_CID_MPEG_VIDEO_BASE + 229;
pub(crate) const V4L2_CID_MPEG_VIDEO_REPEAT_SEQ_HEADER: u32 = V4L2_CID_MPEG_VIDEO_BASE + 226;
pub(crate) const V4L2_CID_MPEG_VIDEO_HEVC_LOOP_FILTER_MODE: u32 = V4L2_CID_MPEG_VIDEO_BASE + 620;
pub(crate) const V4L2_CID_MPEG_VIDEO_HEVC_REFRESH_TYPE: u32 = V4L2_CID_MPEG_VIDEO_BASE + 623;
pub(crate) const V4L2_CID_MPEG_VIDEO_PREPEND_SPSPPS_TO_IDR: u32 = V4L2_CID_MPEG_VIDEO_BASE + 644;

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct v4l2_capability {
    pub driver: [u8; 16],
    pub card: [u8; 32],
    pub bus_info: [u8; 32],
    pub version: u32,
    pub capabilities: u32,
    pub device_caps: u32,
    pub reserved: [u32; 3],
}

impl Default for v4l2_capability {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct v4l2_plane_pix_format {
    pub sizeimage: u32,
    pub bytesperline: u32,
    pub reserved: [u16; 6],
}

impl Default for v4l2_plane_pix_format {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct v4l2_pix_format_mplane {
    pub width: u32,
    pub height: u32,
    pub pixelformat: u32,
    pub field: u32,
    pub colorspace: u32,
    pub plane_fmt: [v4l2_plane_pix_format; 8],
    pub num_planes: u8,
    pub flags: u8,
    pub ycbcr_enc: u8,
    pub quantization: u8,
    pub xfer_func: u8,
    pub reserved: [u8; 7],
}

impl Default for v4l2_pix_format_mplane {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct v4l2_pix_format {
    pub width: u32,
    pub height: u32,
    pub pixelformat: u32,
    pub field: u32,
    pub bytesperline: u32,
    pub sizeimage: u32,
    pub colorspace: u32,
    pub priv_: u32,
    pub flags: u32,
    pub ycbcr_enc: u32,
    pub quantization: u32,
    pub xfer_func: u32,
}

impl Default for v4l2_pix_format {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

#[repr(C, align(8))]
pub(crate) union v4l2_format_union {
    pub pix: v4l2_pix_format,
    pub pix_mp: v4l2_pix_format_mplane,
    pub raw: [u8; 200],
}

#[repr(C)]
pub(crate) struct v4l2_format {
    pub type_: u32,
    pub fmt: v4l2_format_union,
}

impl v4l2_format {
    pub(crate) fn from_pixel_format(buffer_type: BufferType, format: PixelFormat) -> Self {
        if buffer_type.is_multiplanar() {
            let mut pix_mp = v4l2_pix_format_mplane::default();
            pix_mp.width = format.size.width;
            pix_mp.height = format.size.height;
            pix_mp.pixelformat = format.fourcc.0;
            pix_mp.field = format.field.raw();
            pix_mp.num_planes = format.num_planes.max(1);
            Self {
                type_: buffer_type.raw(),
                fmt: v4l2_format_union { pix_mp },
            }
        } else {
            let mut pix = v4l2_pix_format::default();
            pix.width = format.size.width;
            pix.height = format.size.height;
            pix.pixelformat = format.fourcc.0;
            pix.field = format.field.raw();
            Self {
                type_: buffer_type.raw(),
                fmt: v4l2_format_union { pix },
            }
        }
    }

    pub(crate) fn to_pixel_format(&self, buffer_type: BufferType) -> V4l2Result<PixelFormat> {
        if buffer_type.is_multiplanar() {
            let pix = unsafe { self.fmt.pix_mp };
            Ok(PixelFormat {
                size: crate::format::FrameSize {
                    width: pix.width,
                    height: pix.height,
                },
                fourcc: crate::format::FourCc(pix.pixelformat),
                field: field_from_raw(pix.field)?,
                num_planes: pix.num_planes,
            })
        } else {
            let pix = unsafe { self.fmt.pix };
            Ok(PixelFormat {
                size: crate::format::FrameSize {
                    width: pix.width,
                    height: pix.height,
                },
                fourcc: crate::format::FourCc(pix.pixelformat),
                field: field_from_raw(pix.field)?,
                num_planes: 1,
            })
        }
    }
}

fn field_from_raw(value: u32) -> V4l2Result<Field> {
    match value {
        0 => Ok(Field::Any),
        1 => Ok(Field::None),
        other => Err(V4l2Error::InvalidValue(format!(
            "unsupported field value {other}"
        ))),
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct v4l2_fract {
    pub numerator: u32,
    pub denominator: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct v4l2_captureparm {
    pub capability: u32,
    pub capturemode: u32,
    pub timeperframe: v4l2_fract,
    pub extendedmode: u32,
    pub readbuffers: u32,
    pub reserved: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct v4l2_outputparm {
    pub capability: u32,
    pub outputmode: u32,
    pub timeperframe: v4l2_fract,
    pub extendedmode: u32,
    pub writebuffers: u32,
    pub reserved: [u32; 4],
}

#[repr(C)]
pub(crate) union v4l2_streamparm_union {
    pub capture: v4l2_captureparm,
    pub output: v4l2_outputparm,
    pub raw: [u8; 200],
}

#[repr(C)]
pub(crate) struct v4l2_streamparm {
    pub type_: u32,
    pub parm: v4l2_streamparm_union,
}

impl v4l2_streamparm {
    pub(crate) fn from_interval(buffer_type: BufferType, interval: FrameInterval) -> Self {
        let timeperframe = v4l2_fract {
            numerator: interval.numerator,
            denominator: interval.denominator,
        };
        match buffer_type {
            BufferType::VideoCapture | BufferType::VideoCaptureMplane => Self {
                type_: buffer_type.raw(),
                parm: v4l2_streamparm_union {
                    capture: v4l2_captureparm {
                        capability: 0,
                        capturemode: 0,
                        timeperframe,
                        extendedmode: 0,
                        readbuffers: 0,
                        reserved: [0; 4],
                    },
                },
            },
            BufferType::VideoOutput | BufferType::VideoOutputMplane => Self {
                type_: buffer_type.raw(),
                parm: v4l2_streamparm_union {
                    output: v4l2_outputparm {
                        capability: 0,
                        outputmode: 0,
                        timeperframe,
                        extendedmode: 0,
                        writebuffers: 0,
                        reserved: [0; 4],
                    },
                },
            },
        }
    }
}

#[repr(C)]
pub(crate) struct v4l2_requestbuffers {
    pub count: u32,
    pub type_: u32,
    pub memory: u32,
    pub capabilities: u32,
    pub flags: u8,
    pub reserved: [u8; 1],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) union v4l2_plane_m {
    pub mem_offset: u32,
    pub userptr: libc::c_ulong,
    pub fd: libc::c_int,
}

impl Default for v4l2_plane_m {
    fn default() -> Self {
        Self { mem_offset: 0 }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct v4l2_plane {
    pub bytesused: u32,
    pub length: u32,
    pub m: v4l2_plane_m,
    pub data_offset: u32,
    pub reserved: [u32; 11],
}

impl Default for v4l2_plane {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct timeval {
    pub tv_sec: libc::c_long,
    pub tv_usec: libc::c_long,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct v4l2_timecode {
    pub type_: u32,
    pub flags: u32,
    pub frames: u8,
    pub seconds: u8,
    pub minutes: u8,
    pub hours: u8,
    pub userbits: [u8; 4],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) union v4l2_buffer_m {
    pub offset: u32,
    pub userptr: libc::c_ulong,
    pub planes: *mut v4l2_plane,
    pub fd: libc::c_int,
}

impl Default for v4l2_buffer_m {
    fn default() -> Self {
        Self { offset: 0 }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct v4l2_buffer {
    pub index: u32,
    pub type_: u32,
    pub bytesused: u32,
    pub flags: u32,
    pub field: u32,
    pub timestamp: timeval,
    pub timecode: v4l2_timecode,
    pub sequence: u32,
    pub memory: u32,
    pub m: v4l2_buffer_m,
    pub length: u32,
    pub reserved2: u32,
    pub request_fd: i32,
}

impl v4l2_buffer {
    pub(crate) fn new(
        buffer_type: BufferType,
        memory: BufferMemory,
        index: BufferIndex,
        planes: u8,
        plane_ptr: *mut v4l2_plane,
    ) -> Self {
        let mut raw: Self = unsafe { std::mem::zeroed() };
        raw.index = index.0;
        raw.type_ = buffer_type.raw();
        raw.memory = memory.raw();
        if buffer_type.is_multiplanar() {
            raw.length = u32::from(planes.max(1));
            raw.m.planes = plane_ptr;
        }
        raw
    }
}

#[repr(C)]
pub(crate) struct v4l2_exportbuffer {
    pub type_: u32,
    pub index: u32,
    pub plane: u32,
    pub flags: u32,
    pub fd: i32,
    pub reserved: [u32; 11],
}

#[repr(C)]
pub(crate) struct v4l2_control {
    pub id: u32,
    pub value: i32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn v4l2_format_matches_64_bit_linux_abi() {
        assert_eq!(std::mem::size_of::<v4l2_format>(), 208);
        assert_eq!(vidioc_s_fmt(), 0xc0d0_5605);
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn common_buffer_ioctls_match_64_bit_linux_abi() {
        assert_eq!(std::mem::size_of::<v4l2_requestbuffers>(), 20);
        assert_eq!(vidioc_reqbufs(), 0xc014_5608);
        assert_eq!(std::mem::size_of::<v4l2_buffer>(), 88);
        assert_eq!(vidioc_qbuf(), 0xc058_560f);
    }

    #[test]
    fn codec_control_ids_match_linux_uapi() {
        assert_eq!(V4L2_CID_MPEG_VIDEO_GOP_SIZE, 0x0099_09cb);
        assert_eq!(V4L2_CID_MPEG_VIDEO_BITRATE, 0x0099_09cf);
        assert_eq!(V4L2_CID_MPEG_VIDEO_FRAME_RC_ENABLE, 0x0099_09d7);
        assert_eq!(V4L2_CID_MPEG_VIDEO_REPEAT_SEQ_HEADER, 0x0099_09e2);
        assert_eq!(V4L2_CID_MPEG_VIDEO_FORCE_KEY_FRAME, 0x0099_09e5);
        assert_eq!(V4L2_CID_MPEG_VIDEO_HEVC_LOOP_FILTER_MODE, 0x0099_0b6c);
        assert_eq!(V4L2_CID_MPEG_VIDEO_HEVC_REFRESH_TYPE, 0x0099_0b6f);
        assert_eq!(V4L2_CID_MPEG_VIDEO_PREPEND_SPSPPS_TO_IDR, 0x0099_0b84);
    }
}
