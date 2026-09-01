use std::collections::BTreeMap;
use std::ffi::CString;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use usb_gadget::function::msd::Msd;

#[derive(Debug, Clone)]
pub struct MassStorageController {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    function: Msd,
    lun_dir: PathBuf,
    storage_dir: PathBuf,
    state_path: PathBuf,
    state: Mutex<PersistentState>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct PersistentState {
    selected: Option<String>,
    connected: bool,
    cdrom: bool,
    read_write: bool,
    #[serde(skip)]
    uploading: Option<UploadState>,
}

#[derive(Debug, Clone)]
struct UploadState {
    name: String,
    size: u64,
    written: u64,
}

impl MassStorageController {
    pub fn open(function: Msd, function_dir: PathBuf, storage_dir: PathBuf) -> io::Result<Self> {
        fs::create_dir_all(&storage_dir)?;
        let state_path = storage_dir
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("msd-state.json");
        let mut state = match fs::read(&state_path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => PersistentState::default(),
            Err(error) => return Err(error),
        };
        if state
            .selected
            .as_deref()
            .is_some_and(|name| validate_image_name(name).is_err())
        {
            state.selected = None;
            state.connected = false;
        }
        let controller = Self {
            inner: Arc::new(Inner {
                function,
                lun_dir: function_dir.join("lun.0"),
                storage_dir,
                state_path,
                state: Mutex::new(state),
            }),
        };
        controller.restore()?;
        Ok(controller)
    }

    pub async fn snapshot(&self) -> Value {
        let state = self.inner.state.lock().expect("MSD state poisoned").clone();
        let images = self.images();
        let selected = state
            .selected
            .as_ref()
            .and_then(|name| images.get(name).cloned());
        let (size, free) = filesystem_space(&self.inner.storage_dir).unwrap_or((0, 0));
        let uploading = state.uploading.map(|upload| {
            json!({
                "name": upload.name,
                "size": upload.size,
                "written": upload.written,
            })
        });
        json!({
            "event_type": "msd",
            "event": {
                "enabled": true,
                "online": true,
                "busy": uploading.is_some(),
                "drive": {
                    "image": selected,
                    "connected": state.connected,
                    "cdrom": state.cdrom,
                    "rw": state.read_write && !state.cdrom,
                },
                "storage": {
                    "parts": {"": {"size": size, "free": free, "writable": true}},
                    "images": images,
                    "uploading": uploading,
                    "downloading": null,
                },
            },
        })
    }

    pub async fn set_image(&self, image: Option<&str>) -> io::Result<()> {
        let image = image.filter(|name| !name.is_empty());
        if let Some(name) = image {
            validate_image_name(name)?;
            if !self.image_path(name)?.is_file() {
                return Err(io::Error::new(io::ErrorKind::NotFound, "image not found"));
            }
        }
        let mut state = self.inner.state.lock().expect("MSD state poisoned");
        require_disconnected(&state)?;
        state.selected = image.map(str::to_owned);
        self.save_state(&state)
    }

    pub async fn set_cdrom(&self, enabled: bool) -> io::Result<()> {
        let mut state = self.inner.state.lock().expect("MSD state poisoned");
        require_disconnected(&state)?;
        state.cdrom = enabled;
        if enabled {
            state.read_write = false;
        }
        self.apply_mode(&state)?;
        self.save_state(&state)
    }

    pub async fn set_read_write(&self, enabled: bool) -> io::Result<()> {
        let mut state = self.inner.state.lock().expect("MSD state poisoned");
        require_disconnected(&state)?;
        if enabled && state.cdrom {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "CD-ROM images cannot be writable",
            ));
        }
        state.read_write = enabled;
        self.apply_mode(&state)?;
        self.save_state(&state)
    }

    pub async fn set_connected(&self, connected: bool) -> io::Result<()> {
        let mut state = self.inner.state.lock().expect("MSD state poisoned");
        if connected == state.connected {
            return Ok(());
        }
        if connected {
            let name = state
                .selected
                .as_deref()
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no image selected"))?;
            let path = self.image_path(name)?;
            if !path.is_file() {
                return Err(io::Error::new(io::ErrorKind::NotFound, "image not found"));
            }
            self.apply_mode(&state)?;
            self.inner.function.set_file(0, Some(&path))?;
        } else {
            self.detach()?;
        }
        state.connected = connected;
        self.save_state(&state)
    }

    pub async fn reset(&self) -> io::Result<()> {
        let state = self.inner.state.lock().expect("MSD state poisoned");
        if !state.connected {
            return Ok(());
        }
        let name = state
            .selected
            .as_deref()
            .ok_or_else(|| io::Error::other("connected drive has no selected image"))?;
        let path = self.image_path(name)?;
        self.detach()?;
        self.apply_mode(&state)?;
        self.inner.function.set_file(0, Some(path))
    }

    pub async fn remove_image(&self, name: &str) -> io::Result<()> {
        let path = self.image_path(name)?;
        let mut state = self.inner.state.lock().expect("MSD state poisoned");
        require_disconnected(&state)?;
        fs::remove_file(path)?;
        if state.selected.as_deref() == Some(name) {
            state.selected = None;
            self.save_state(&state)?;
        }
        Ok(())
    }

    pub fn image_path(&self, name: &str) -> io::Result<PathBuf> {
        validate_image_name(name)?;
        Ok(self.inner.storage_dir.join(name))
    }

    pub fn temporary_image_path(&self, name: &str) -> io::Result<PathBuf> {
        validate_image_name(name)?;
        Ok(self.inner.storage_dir.join(format!(".{name}.upload")))
    }

    pub async fn begin_upload(&self, name: &str, size: u64) -> io::Result<()> {
        validate_image_name(name)?;
        let mut state = self.inner.state.lock().expect("MSD state poisoned");
        require_disconnected(&state)?;
        if state.uploading.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "another upload is active",
            ));
        }
        state.uploading = Some(UploadState {
            name: name.to_owned(),
            size,
            written: 0,
        });
        Ok(())
    }

    pub async fn upload_progress(&self, written: u64) {
        if let Some(upload) = &mut self
            .inner
            .state
            .lock()
            .expect("MSD state poisoned")
            .uploading
        {
            upload.written = written;
        }
    }

    pub async fn finish_upload(&self, name: &str, succeeded: bool) -> io::Result<()> {
        let mut state = self.inner.state.lock().expect("MSD state poisoned");
        state.uploading = None;
        if succeeded {
            state.selected = Some(name.to_owned());
            self.save_state(&state)?;
        }
        Ok(())
    }

    fn restore(&self) -> io::Result<()> {
        let mut state = self.inner.state.lock().expect("MSD state poisoned");
        self.apply_mode(&state)?;
        if state.connected {
            let path = state
                .selected
                .as_deref()
                .map(|name| self.image_path(name))
                .transpose()?;
            if let Some(path) = path.filter(|path| path.is_file()) {
                self.inner.function.set_file(0, Some(path))?;
            } else {
                state.connected = false;
                self.save_state(&state)?;
            }
        }
        Ok(())
    }

    fn apply_mode(&self, state: &PersistentState) -> io::Result<()> {
        fs::write(
            self.inner.lun_dir.join("cdrom"),
            u8::from(state.cdrom).to_string(),
        )?;
        fs::write(
            self.inner.lun_dir.join("ro"),
            u8::from(state.cdrom || !state.read_write).to_string(),
        )
    }

    fn detach(&self) -> io::Result<()> {
        self.inner.function.force_eject(0)?;
        self.inner.function.set_file(0, None::<&Path>)
    }

    fn save_state(&self, state: &PersistentState) -> io::Result<()> {
        let bytes = serde_json::to_vec_pretty(state).map_err(io::Error::other)?;
        let temporary = self.inner.state_path.with_extension("json.tmp");
        fs::write(&temporary, bytes)?;
        fs::rename(temporary, &self.inner.state_path)
    }

    fn images(&self) -> BTreeMap<String, Value> {
        let mut images = BTreeMap::new();
        let Ok(entries) = fs::read_dir(&self.inner.storage_dir) else {
            return images;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if name.starts_with('.') || validate_image_name(name).is_err() {
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if !metadata.is_file() {
                continue;
            }
            let mod_ts = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |duration| duration.as_secs());
            images.insert(
                name.to_owned(),
                json!({
                    "name": name,
                    "size": metadata.len(),
                    "mod_ts": mod_ts,
                    "writable": !metadata.permissions().readonly(),
                    "complete": true,
                    "removable": true,
                    "in_storage": true,
                }),
            );
        }
        images
    }
}

fn require_disconnected(state: &PersistentState) -> io::Result<()> {
    if state.connected {
        Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "disconnect the mass-storage drive first",
        ))
    } else {
        Ok(())
    }
}

fn validate_image_name(name: &str) -> io::Result<()> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.starts_with('.')
        || name.contains(['/', '\\', '\0'])
        || name.len() > 255
    {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid image name",
        ))
    } else {
        Ok(())
    }
}

fn filesystem_space(path: &Path) -> io::Result<(u64, u64)> {
    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "storage path contains NUL"))?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    if unsafe { libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let stat = unsafe { stat.assume_init() };
    let block_size = stat.f_frsize;
    Ok((
        stat.f_blocks.saturating_mul(block_size),
        stat.f_bavail.saturating_mul(block_size),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsafe_image_names() {
        for name in ["", ".", "..", ".hidden", "../disk.img", "a/b.img"] {
            assert!(validate_image_name(name).is_err(), "accepted {name:?}");
        }
        assert!(validate_image_name("installer.iso").is_ok());
    }
}
