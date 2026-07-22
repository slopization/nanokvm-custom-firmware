use std::fmt;
use std::str::FromStr;

use crate::error::{V4l2Error, V4l2Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FourCc(pub u32);

impl FourCc {
    pub const UYVY: Self = Self::from_bytes(*b"UYVY");
    pub const YUYV: Self = Self::from_bytes(*b"YUYV");
    pub const NV12: Self = Self::from_bytes(*b"NV12");
    pub const HEVC: Self = Self::from_bytes(*b"HEVC");
    pub const H264: Self = Self::from_bytes(*b"H264");
    pub const FWHT: Self = Self::from_bytes(*b"FWHT");

    pub const fn from_bytes(bytes: [u8; 4]) -> Self {
        Self(u32::from_le_bytes(bytes))
    }

    pub fn bytes(self) -> [u8; 4] {
        self.0.to_le_bytes()
    }
}

impl fmt::Display for FourCc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let bytes = self.bytes();
        let text = String::from_utf8_lossy(&bytes);
        f.write_str(&text)
    }
}

impl FromStr for FourCc {
    type Err = V4l2Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let bytes = value.as_bytes();
        if bytes.len() != 4 {
            return Err(V4l2Error::InvalidValue(format!(
                "FourCC must be four bytes, got {value:?}"
            )));
        }
        Ok(Self::from_bytes(
            bytes.try_into().expect("checked FourCC length"),
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameSize {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameInterval {
    pub numerator: u32,
    pub denominator: u32,
}

impl FrameInterval {
    pub fn from_fps(fps: u32) -> V4l2Result<Self> {
        if fps == 0 {
            return Err(V4l2Error::InvalidValue("fps must be non-zero".to_owned()));
        }
        Ok(Self {
            numerator: 1,
            denominator: fps,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Any,
    None,
}

impl Field {
    pub(crate) fn raw(self) -> u32 {
        match self {
            Self::Any => 0,
            Self::None => 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelFormat {
    pub size: FrameSize,
    pub fourcc: FourCc,
    pub field: Field,
    pub num_planes: u8,
}

impl PixelFormat {
    pub fn single_plane(width: u32, height: u32, fourcc: FourCc) -> Self {
        Self {
            size: FrameSize { width, height },
            fourcc,
            field: Field::None,
            num_planes: 1,
        }
    }
}
