use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use tokio::io::unix::AsyncFd;

use crate::audio::{
    USB_AUDIO_VOLUME_MAX_DB_256, USB_AUDIO_VOLUME_MIN_DB_256, USB_AUDIO_VOLUME_RES_DB_256,
};

const CHANNELS: usize = 2;
const ACCESS_RW_INTERLEAVED: usize = 3;
const FORMAT_S16_LE: usize = 2;
const HW_PARAM_CHANNELS: usize = 10;
const HW_PARAM_RATE: usize = 11;
const HW_PARAM_PERIOD_SIZE: usize = 13;
const HW_PARAM_PERIODS: usize = 15;
const FIRST_INTERVAL: usize = 8;
const BUFFER_PERIODS: u32 = 16;
const POLL_RETRY: std::time::Duration = std::time::Duration::from_millis(5);

const IOCTL_HW_PARAMS: libc::c_ulong = ioctl_read_write(0x11, 608);
const IOCTL_SW_PARAMS: libc::c_ulong = ioctl_read_write(0x13, 136);
const IOCTL_PREPARE: libc::c_ulong = ioctl_none(0x40);
const IOCTL_START: libc::c_ulong = ioctl_none(0x42);
const IOCTL_READI_FRAMES: libc::c_ulong = ioctl_read(0x51, 24);
const IOCTL_CTL_ELEM_READ: libc::c_ulong = ioctl_ctl_read_write(0x12, 1224);
const CTL_ELEM_IFACE_MIXER: i32 = 2;
const CAPTURE_SWITCH: &str = "PCM Capture Switch";
const CAPTURE_VOLUME: &str = "PCM Capture Volume";

pub struct PcmCapture {
    file: AsyncFd<File>,
    samples: Vec<i16>,
    frames_filled: usize,
    frame_samples: usize,
    gain: Option<UsbGainControl>,
}

impl PcmCapture {
    pub fn open(path: Option<&Path>, frame_samples: usize) -> io::Result<Self> {
        let path = match path {
            Some(path) => path.to_owned(),
            None => discover_usb_capture_device()?,
        };
        let gain = match UsbGainControl::open_for_pcm(&path) {
            Ok(gain) => Some(gain),
            Err(err) => {
                eprintln!("USB host volume control unavailable: {err}");
                None
            }
        };
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&path)?;
        configure(&file, frame_samples)?;
        let file = AsyncFd::new(file)?;
        println!("capturing USB audio from {}", path.display());
        Ok(Self {
            file,
            samples: vec![0; frame_samples * CHANNELS],
            frames_filled: 0,
            frame_samples,
            gain,
        })
    }

    pub async fn read_frame(&mut self) -> io::Result<Vec<i16>> {
        let file = &self.file;
        let samples = &mut self.samples;
        let frames_filled = &mut self.frames_filled;
        let frame_samples = self.frame_samples;
        loop {
            match read_samples(file.get_ref(), samples, frames_filled, frame_samples) {
                Ok(()) => {
                    if *frames_filled == frame_samples {
                        *frames_filled = 0;
                        let mut completed =
                            std::mem::replace(samples, vec![0; frame_samples * CHANNELS]);
                        if let Some(gain) = &mut self.gain {
                            gain.apply(&mut completed)?;
                        }
                        return Ok(completed);
                    }
                }
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                    if let Ok(ready) = tokio::time::timeout(POLL_RETRY, file.readable()).await {
                        let mut ready = ready?;
                        ready.clear_ready();
                    }
                }
                Err(err) => return Err(err),
            }
        }
    }
}

struct UsbGainControl {
    file: File,
    switch_id: SndCtlElemId,
    volume_id: SndCtlElemId,
    last: Option<(bool, i64)>,
}

impl UsbGainControl {
    fn open_for_pcm(pcm_path: &Path) -> io::Result<Self> {
        let control_path = control_path_for_pcm(pcm_path).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "cannot derive ALSA control device from {}",
                    pcm_path.display()
                ),
            )
        })?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(control_path)?;
        let mut gain = Self {
            file,
            switch_id: SndCtlElemId::named(CAPTURE_SWITCH),
            volume_id: SndCtlElemId::named(CAPTURE_VOLUME),
            last: None,
        };
        let mut switch_id = gain.switch_id.clone();
        gain.read(&mut switch_id)?;
        gain.switch_id = switch_id;
        let mut volume_id = gain.volume_id.clone();
        gain.read(&mut volume_id)?;
        gain.volume_id = volume_id;
        Ok(gain)
    }

    fn apply(&mut self, samples: &mut [i16]) -> io::Result<()> {
        let mut switch_id = self.switch_id.clone();
        let enabled = self.read(&mut switch_id)? != 0;
        self.switch_id = switch_id;
        let mut volume_id = self.volume_id.clone();
        let volume_step = self.read(&mut volume_id)?;
        self.volume_id = volume_id;

        if self.last != Some((enabled, volume_step)) {
            println!(
                "USB host volume: enabled={enabled}, step={volume_step}, gain_db={:.1}",
                gain_db(volume_step)
            );
            self.last = Some((enabled, volume_step));
        }
        apply_host_gain(samples, enabled, volume_step);
        Ok(())
    }

    fn read(&self, id: &mut SndCtlElemId) -> io::Result<i64> {
        let mut value = SndCtlElemValue::new(id.clone());
        ioctl_mut(&self.file, IOCTL_CTL_ELEM_READ, &mut value)?;
        *id = value.id;
        Ok(value.values[0])
    }
}

fn control_path_for_pcm(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?.to_str()?;
    let card = name.strip_prefix("pcmC")?.split_once('D')?.0;
    if card.is_empty() || !card.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some(path.with_file_name(format!("controlC{card}")))
}

fn gain_db(volume_step: i64) -> f32 {
    let max_step = i64::from(
        (USB_AUDIO_VOLUME_MAX_DB_256 - USB_AUDIO_VOLUME_MIN_DB_256) / USB_AUDIO_VOLUME_RES_DB_256,
    );
    let step = volume_step.clamp(0, max_step);
    (f32::from(USB_AUDIO_VOLUME_MIN_DB_256) + step as f32 * f32::from(USB_AUDIO_VOLUME_RES_DB_256))
        / 256.0
}

fn apply_host_gain(samples: &mut [i16], enabled: bool, volume_step: i64) {
    if !enabled {
        samples.fill(0);
        return;
    }
    let db = gain_db(volume_step);
    if db >= 0.0 {
        return;
    }
    let gain_q16 = (10.0_f32.powf(db / 20.0) * 65_536.0).round() as i32;
    for sample in samples {
        *sample = ((i32::from(*sample) * gain_q16) >> 16) as i16;
    }
}

fn read_samples(
    file: &File,
    samples: &mut [i16],
    frames_filled: &mut usize,
    frame_samples: usize,
) -> io::Result<()> {
    let remaining = frame_samples - *frames_filled;
    let sample_offset = *frames_filled * CHANNELS;
    let mut transfer = SndXferI {
        result: 0,
        buf: samples[sample_offset..].as_mut_ptr().cast(),
        frames: remaining as u64,
    };
    if unsafe { libc::ioctl(file.as_raw_fd(), IOCTL_READI_FRAMES, &mut transfer) } < 0 {
        let err = io::Error::last_os_error();
        return match err.raw_os_error() {
            Some(libc::EPIPE) | Some(libc::ESTRPIPE) => {
                recover_xrun(file, frames_filled)?;
                Err(io::Error::from(io::ErrorKind::WouldBlock))
            }
            _ => Err(err),
        };
    }
    if transfer.result < 0 {
        let errno = (-transfer.result) as i32;
        if matches!(errno, libc::EPIPE | libc::ESTRPIPE) {
            recover_xrun(file, frames_filled)?;
            return Err(io::Error::from(io::ErrorKind::WouldBlock));
        }
        return Err(io::Error::from_raw_os_error(errno));
    }
    *frames_filled += transfer.result as usize;
    if *frames_filled < frame_samples {
        Err(io::Error::from(io::ErrorKind::WouldBlock))
    } else {
        Ok(())
    }
}

fn recover_xrun(file: &File, frames_filled: &mut usize) -> io::Result<()> {
    ioctl_no_arg(file, IOCTL_PREPARE)?;
    ioctl_no_arg(file, IOCTL_START)?;
    *frames_filled = 0;
    Ok(())
}

fn discover_usb_capture_device() -> io::Result<PathBuf> {
    for entry in std::fs::read_dir("/sys/class/sound")? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(card) = name.to_str().and_then(|name| name.strip_prefix("card")) else {
            continue;
        };
        let id = std::fs::read_to_string(entry.path().join("id")).unwrap_or_default();
        if matches!(id.trim(), "UAC1Gadget" | "UAC2Gadget") {
            let path = PathBuf::from(format!("/dev/snd/pcmC{card}D0c"));
            if path.exists() {
                return Ok(path);
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "USB audio gadget ALSA capture device was not found",
    ))
}

fn configure(file: &File, frame_samples: usize) -> io::Result<()> {
    let mut params = SndPcmHwParams::any();
    params.set_mask(0, ACCESS_RW_INTERLEAVED);
    params.set_mask(1, FORMAT_S16_LE);
    params.set_interval(HW_PARAM_CHANNELS, CHANNELS as u32);
    params.set_interval(HW_PARAM_RATE, 48_000);
    params.set_interval(HW_PARAM_PERIOD_SIZE, frame_samples as u32);
    params.set_interval(HW_PARAM_PERIODS, BUFFER_PERIODS);
    ioctl_mut(file, IOCTL_HW_PARAMS, &mut params)?;

    let mut software = SndPcmSwParams {
        tstamp_mode: 1,
        period_step: 1,
        avail_min: frame_samples as u64,
        xfer_align: (frame_samples / 2) as u64,
        start_threshold: 1,
        stop_threshold: (frame_samples as u64) * u64::from(BUFFER_PERIODS),
        ..Default::default()
    };
    ioctl_mut(file, IOCTL_SW_PARAMS, &mut software)?;
    ioctl_no_arg(file, IOCTL_PREPARE)
}

fn ioctl_mut<T>(file: &File, request: libc::c_ulong, value: &mut T) -> io::Result<()> {
    if unsafe { libc::ioctl(file.as_raw_fd(), request, value) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn ioctl_no_arg(file: &File, request: libc::c_ulong) -> io::Result<()> {
    if unsafe { libc::ioctl(file.as_raw_fd(), request) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct SndMask {
    bits: [u32; 8],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct SndInterval {
    min: u32,
    max: u32,
    flags: u32,
}

#[repr(C)]
struct SndPcmHwParams {
    flags: u32,
    masks: [SndMask; 3],
    reserved_masks: [SndMask; 5],
    intervals: [SndInterval; 12],
    reserved_intervals: [SndInterval; 9],
    rmask: u32,
    cmask: u32,
    info: u32,
    msbits: u32,
    rate_num: u32,
    rate_den: u32,
    fifo_size: u64,
    sync: [u8; 16],
    reserved: [u8; 48],
}

impl SndPcmHwParams {
    fn any() -> Self {
        Self {
            flags: 0,
            masks: [SndMask {
                bits: [u32::MAX; 8],
            }; 3],
            reserved_masks: [SndMask::default(); 5],
            intervals: [SndInterval {
                min: 0,
                max: u32::MAX,
                flags: 0,
            }; 12],
            reserved_intervals: [SndInterval::default(); 9],
            rmask: u32::MAX,
            cmask: 0,
            info: u32::MAX,
            msbits: 0,
            rate_num: 0,
            rate_den: 0,
            fifo_size: 0,
            sync: [0; 16],
            reserved: [0; 48],
        }
    }

    fn set_mask(&mut self, parameter: usize, value: usize) {
        self.masks[parameter].bits = [0; 8];
        self.masks[parameter].bits[value / 32] = 1 << (value % 32);
    }

    fn set_interval(&mut self, parameter: usize, value: u32) {
        self.intervals[parameter - FIRST_INTERVAL] = SndInterval {
            min: value,
            max: value,
            flags: 1 << 2,
        };
    }
}

#[repr(C)]
struct SndPcmSwParams {
    tstamp_mode: i32,
    period_step: u32,
    sleep_min: u32,
    padding: u32,
    avail_min: u64,
    xfer_align: u64,
    start_threshold: u64,
    stop_threshold: u64,
    silence_threshold: u64,
    silence_size: u64,
    boundary: u64,
    proto: u32,
    tstamp_type: u32,
    reserved: [u8; 56],
}

impl Default for SndPcmSwParams {
    fn default() -> Self {
        Self {
            tstamp_mode: 0,
            period_step: 0,
            sleep_min: 0,
            padding: 0,
            avail_min: 0,
            xfer_align: 0,
            start_threshold: 0,
            stop_threshold: 0,
            silence_threshold: 0,
            silence_size: 0,
            boundary: 0,
            proto: 0,
            tstamp_type: 0,
            reserved: [0; 56],
        }
    }
}

#[repr(C)]
struct SndXferI {
    result: i64,
    buf: *mut libc::c_void,
    frames: u64,
}

#[repr(C)]
#[derive(Clone)]
struct SndCtlElemId {
    numid: u32,
    interface: i32,
    device: u32,
    subdevice: u32,
    name: [u8; 44],
    index: u32,
}

impl SndCtlElemId {
    fn named(name: &str) -> Self {
        assert!(name.len() < 44, "ALSA control name is too long");
        let mut id = Self {
            numid: 0,
            interface: CTL_ELEM_IFACE_MIXER,
            device: 0,
            subdevice: 0,
            name: [0; 44],
            index: 0,
        };
        id.name[..name.len()].copy_from_slice(name.as_bytes());
        id
    }
}

#[repr(C)]
struct SndCtlElemValue {
    id: SndCtlElemId,
    indirect: u32,
    values: [i64; 128],
    reserved: [u8; 128],
}

impl SndCtlElemValue {
    fn new(id: SndCtlElemId) -> Self {
        Self {
            id,
            indirect: 0,
            values: [0; 128],
            reserved: [0; 128],
        }
    }
}

const fn ioctl_none(number: u32) -> libc::c_ulong {
    ((b'A' as u32) << 8 | number) as libc::c_ulong
}

const fn ioctl_read(number: u32, size: u32) -> libc::c_ulong {
    ioctl(2, number, size)
}

const fn ioctl_read_write(number: u32, size: u32) -> libc::c_ulong {
    ioctl(3, number, size)
}

const fn ioctl_ctl_read_write(number: u32, size: u32) -> libc::c_ulong {
    ((3 << 30) | (size << 16) | ((b'U' as u32) << 8) | number) as libc::c_ulong
}

const fn ioctl(direction: u32, number: u32, size: u32) -> libc::c_ulong {
    ((direction << 30) | (size << 16) | ((b'A' as u32) << 8) | number) as libc::c_ulong
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alsa_pcm_abi_matches_64_bit_linux() {
        assert_eq!(std::mem::size_of::<SndPcmHwParams>(), 608);
        assert_eq!(std::mem::size_of::<SndPcmSwParams>(), 136);
        assert_eq!(std::mem::size_of::<SndXferI>(), 24);
        assert_eq!(std::mem::size_of::<SndCtlElemId>(), 64);
        assert_eq!(std::mem::size_of::<SndCtlElemValue>(), 1224);
        assert_eq!(IOCTL_HW_PARAMS, 0xc260_4111);
        assert_eq!(IOCTL_SW_PARAMS, 0xc088_4113);
        assert_eq!(IOCTL_PREPARE, 0x4140);
        assert_eq!(IOCTL_READI_FRAMES, 0x8018_4151);
        assert_eq!(IOCTL_CTL_ELEM_READ, 0xc4c8_5512);
    }

    #[test]
    fn applies_usb_mute_and_decibel_gain() {
        let mut muted = [i16::MAX, i16::MIN];
        apply_host_gain(&mut muted, false, 100);
        assert_eq!(muted, [0, 0]);

        let mut full = [12_345, -12_345];
        apply_host_gain(&mut full, true, 100);
        assert_eq!(full, [12_345, -12_345]);

        let mut minus_six_db = [20_000, -20_000];
        apply_host_gain(&mut minus_six_db, true, 94);
        assert!((minus_six_db[0] - 10_024).abs() <= 1);
        assert!((minus_six_db[1] + 10_024).abs() <= 1);
    }
}
