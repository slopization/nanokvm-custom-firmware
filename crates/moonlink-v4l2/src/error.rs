use std::io;

use thiserror::Error;

pub type V4l2Result<T> = Result<T, V4l2Error>;

#[derive(Debug, Error)]
pub enum V4l2Error {
    #[error("{op} failed: {source}")]
    Io { op: String, source: io::Error },
    #[error("V4L2 device is not ready")]
    NotReady,
    #[error("invalid V4L2 value: {0}")]
    InvalidValue(String),
    #[error("unsupported V4L2 capability: {0}")]
    Unsupported(String),
}

impl V4l2Error {
    pub(crate) fn io(op: impl Into<String>) -> Self {
        Self::Io {
            op: op.into(),
            source: io::Error::last_os_error(),
        }
    }

    pub(crate) fn from_io(op: impl Into<String>, source: io::Error) -> Self {
        Self::Io {
            op: op.into(),
            source,
        }
    }

    pub fn raw_os_error(&self) -> Option<i32> {
        match self {
            Self::Io { source, .. } => source.raw_os_error(),
            _ => None,
        }
    }
}
