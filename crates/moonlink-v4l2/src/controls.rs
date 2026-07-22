use crate::device::Device;
use crate::error::{V4l2Error, V4l2Result};
use crate::sys;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ControlId(pub u32);

impl ControlId {
    pub const BITRATE: Self = Self(sys::V4L2_CID_MPEG_VIDEO_BITRATE);
    pub const GOP_SIZE: Self = Self(sys::V4L2_CID_MPEG_VIDEO_GOP_SIZE);
    pub const FRAME_RC_ENABLE: Self = Self(sys::V4L2_CID_MPEG_VIDEO_FRAME_RC_ENABLE);
    pub const REPEAT_SEQ_HEADER: Self = Self(sys::V4L2_CID_MPEG_VIDEO_REPEAT_SEQ_HEADER);
    pub const FORCE_KEY_FRAME: Self = Self(sys::V4L2_CID_MPEG_VIDEO_FORCE_KEY_FRAME);
    pub const HEVC_LOOP_FILTER_MODE: Self = Self(sys::V4L2_CID_MPEG_VIDEO_HEVC_LOOP_FILTER_MODE);
    pub const HEVC_REFRESH_TYPE: Self = Self(sys::V4L2_CID_MPEG_VIDEO_HEVC_REFRESH_TYPE);
    pub const PREPEND_SPSPPS_TO_IDR: Self = Self(sys::V4L2_CID_MPEG_VIDEO_PREPEND_SPSPPS_TO_IDR);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Control {
    pub id: ControlId,
    pub value: i32,
}

impl Device {
    pub fn get_control(&self, id: ControlId) -> V4l2Result<i32> {
        let mut raw = sys::v4l2_control { id: id.0, value: 0 };
        let ret = unsafe { libc::ioctl(self.raw_fd(), sys::vidioc_g_ctrl(), &mut raw) };
        if ret < 0 {
            return Err(V4l2Error::from_io(
                format!("VIDIOC_G_CTRL id=0x{:08x}", id.0),
                std::io::Error::last_os_error(),
            ));
        }
        Ok(raw.value)
    }

    pub fn set_control(&self, control: Control) -> V4l2Result<()> {
        let mut raw = sys::v4l2_control {
            id: control.id.0,
            value: control.value,
        };
        let ret = unsafe { libc::ioctl(self.raw_fd(), sys::vidioc_s_ctrl(), &mut raw) };
        if ret < 0 {
            return Err(V4l2Error::from_io(
                format!(
                    "VIDIOC_S_CTRL id=0x{:08x} value={}",
                    control.id.0, control.value
                ),
                std::io::Error::last_os_error(),
            ));
        }
        Ok(())
    }
}
