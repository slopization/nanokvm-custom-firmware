use std::fs::{File, OpenOptions};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use tokio::io::unix::AsyncFd;

use crate::error::{V4l2Error, V4l2Result};
use crate::sys;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capability {
    pub driver: String,
    pub card: String,
    pub bus_info: String,
    pub capabilities: u32,
    pub device_caps: u32,
}

#[derive(Debug)]
pub struct Device {
    path: PathBuf,
    inner: AsyncFd<File>,
}

impl Device {
    pub fn open(path: impl AsRef<Path>) -> V4l2Result<Self> {
        let path = path.as_ref();
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(path)
            .map_err(|err| V4l2Error::from_io("open", err))?;
        let inner = AsyncFd::new(file).map_err(|err| V4l2Error::from_io("AsyncFd::new", err))?;
        Ok(Self {
            path: path.to_owned(),
            inner,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn raw_fd(&self) -> RawFd {
        self.inner.get_ref().as_raw_fd()
    }

    pub fn query_capability(&self) -> V4l2Result<Capability> {
        let mut raw = sys::v4l2_capability::default();
        let ret = unsafe { libc::ioctl(self.raw_fd(), sys::vidioc_querycap(), &mut raw) };
        if ret < 0 {
            return Err(V4l2Error::io("VIDIOC_QUERYCAP"));
        }
        Ok(Capability {
            driver: c_string(&raw.driver),
            card: c_string(&raw.card),
            bus_info: c_string(&raw.bus_info),
            capabilities: raw.capabilities,
            device_caps: raw.device_caps,
        })
    }

    pub async fn wait_readable(&self) -> V4l2Result<()> {
        let mut guard = self
            .inner
            .readable()
            .await
            .map_err(|err| V4l2Error::from_io("poll readable", err))?;
        guard.clear_ready();
        Ok(())
    }

    pub async fn wait_writable(&self) -> V4l2Result<()> {
        let mut guard = self
            .inner
            .writable()
            .await
            .map_err(|err| V4l2Error::from_io("poll writable", err))?;
        guard.clear_ready();
        Ok(())
    }
}

fn c_string(bytes: &[u8]) -> String {
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}
