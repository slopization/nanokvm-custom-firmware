use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use usb_gadget::function::Handle;
use usb_gadget::function::audio::{Channel, Uac1, Uac1Config, Uac2, Uac2Config};
use usb_gadget::function::hid::Hid;
use usb_gadget::function::msd::{Lun, Msd};
use usb_gadget::{Class, Config, Gadget, Id, RegGadget, Strings, Udc, UsbVersion};

use crate::audio::{
    USB_AUDIO_CHANNELS, USB_AUDIO_SAMPLE_RATE, USB_AUDIO_SAMPLE_SIZE, USB_AUDIO_VOLUME_MAX_DB_256,
    USB_AUDIO_VOLUME_MIN_DB_256, USB_AUDIO_VOLUME_RES_DB_256,
};
use crate::cli::{Args, UsbAudioClass};
use crate::hid_report::{HID_REPORT_LEN, report_descriptor};
use crate::msd::MassStorageController;

#[derive(Debug, Clone)]
pub struct GadgetConfig {
    pub gadget_dir: PathBuf,
    pub udc: Option<String>,
    pub vid: String,
    pub pid: String,
    pub serial: String,
    pub manufacturer: String,
    pub product: String,
    pub audio_class: UsbAudioClass,
    pub msd_dir: PathBuf,
}

impl GadgetConfig {
    pub fn from_args(args: &Args) -> Self {
        Self {
            gadget_dir: args.gadget_dir.clone(),
            udc: args.udc.clone(),
            vid: args.usb_vid.clone(),
            pid: args.usb_pid.clone(),
            serial: args.usb_serial.clone(),
            manufacturer: args.usb_manufacturer.clone(),
            product: args.usb_product.clone(),
            audio_class: args.usb_audio_class,
            msd_dir: args.msd_dir.clone(),
        }
    }
}

#[derive(Debug)]
pub struct RegisteredGadget {
    _registration: RegGadget,
    pub mass_storage: MassStorageController,
}

pub fn setup_usb_gadget(config: &GadgetConfig) -> io::Result<RegisteredGadget> {
    ensure_configfs()?;
    remove_existing_gadget(config)?;

    let mut hid = Hid::builder();
    hid.sub_class = 0;
    hid.protocol = 0;
    hid.report_len = HID_REPORT_LEN as u8;
    hid.report_desc = report_descriptor();
    let (hid, function) = hid.build();

    let (audio_function, audio_class) = match config.audio_class {
        UsbAudioClass::Uac1 => (build_uac1(), "UAC1"),
        UsbAudioClass::Uac2 => (build_uac2(), "UAC2"),
    };

    let mut lun = Lun::empty();
    lun.read_only = true;
    lun.inquiry_string = "NanoKVM Virtual Media".to_owned();
    let (mass_storage, mass_storage_function) = Msd::builder().with_lun(lun).build();

    let mut usb_config = Config::new(format!("HID + {audio_class} + MSD"))
        .with_function(function)
        .with_function(audio_function)
        .with_function(mass_storage_function);
    usb_config.max_power = 250;

    let mut gadget = Gadget::new(
        Class::INTERFACE_SPECIFIC,
        Id::new(parse_hex_u16(&config.vid)?, parse_hex_u16(&config.pid)?),
        Strings::new(&config.manufacturer, &config.product, &config.serial),
    )
    .with_config(usb_config);
    gadget.name = gadget_name(config).map(|name| name.to_owned());
    gadget.device_release = 0x0100;
    gadget.usb_version = UsbVersion::V20;

    let udc = match config.udc.as_deref() {
        Some(name) => find_udc(name)?,
        None => usb_gadget::default_udc()?,
    };
    let reg = gadget.bind(&udc)?;
    println!(
        "registered {} on UDC {} at {} ({})",
        config.product,
        udc.name().to_string_lossy(),
        reg.path().display(),
        hid.device_path()?.display()
    );
    println!(
        "{audio_class} host playback input: {} channels, {} Hz, {}-bit",
        USB_AUDIO_CHANNELS,
        USB_AUDIO_SAMPLE_RATE,
        u32::from(USB_AUDIO_SAMPLE_SIZE) * 8,
    );
    let mass_storage = MassStorageController::open(
        mass_storage,
        find_function_path(reg.path(), "mass_storage")?,
        config.msd_dir.clone(),
    )?;
    Ok(RegisteredGadget {
        _registration: reg,
        mass_storage,
    })
}

fn find_function_path(gadget: &Path, driver: &str) -> io::Result<PathBuf> {
    let prefix = format!("{driver}.");
    std::fs::read_dir(gadget.join("functions"))?
        .filter_map(Result::ok)
        .find(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
        .map(|entry| entry.path())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("{driver} function missing"),
            )
        })
}

fn build_uac1() -> Handle {
    let channel_mask = (1_u32 << USB_AUDIO_CHANNELS) - 1;
    let mut capture = Uac1Config::default();
    capture.channel = Channel::new(
        channel_mask,
        USB_AUDIO_SAMPLE_RATE,
        USB_AUDIO_SAMPLE_SIZE.into(),
    );
    capture.mute_present = Some(true);
    capture.volume_present = Some(true);
    capture.volume_min = Some(USB_AUDIO_VOLUME_MIN_DB_256);
    capture.volume_max = Some(USB_AUDIO_VOLUME_MAX_DB_256);
    capture.volume_resolution = Some(USB_AUDIO_VOLUME_RES_DB_256);
    let mut playback = Uac1Config::default();
    playback.channel = Channel::new(0, USB_AUDIO_SAMPLE_RATE, USB_AUDIO_SAMPLE_SIZE.into());
    let (_, function) = Uac1::builder()
        .with_capture_config(capture)
        .with_playback_config(playback)
        .build();
    function
}

fn build_uac2() -> Handle {
    let channel_mask = (1_u32 << USB_AUDIO_CHANNELS) - 1;
    let mut capture = Uac2Config::default();
    capture.channel = Channel::new(
        channel_mask,
        USB_AUDIO_SAMPLE_RATE,
        USB_AUDIO_SAMPLE_SIZE.into(),
    );
    capture.mute_present = Some(true);
    capture.volume_present = Some(true);
    capture.volume_min = Some(USB_AUDIO_VOLUME_MIN_DB_256);
    capture.volume_max = Some(USB_AUDIO_VOLUME_MAX_DB_256);
    capture.volume_resolution = Some(USB_AUDIO_VOLUME_RES_DB_256);
    capture.volume_name = Some("NanoKVM Capture Volume".to_owned());
    capture.input_terminal_name = Some("NanoKVM USB Input".to_owned());
    capture.input_terminal_channel_name = Some("NanoKVM Stereo".to_owned());
    capture.output_terminal_name = Some("NanoKVM Capture".to_owned());

    let mut playback = Uac2Config::default();
    playback.channel = Channel::new(0, USB_AUDIO_SAMPLE_RATE, USB_AUDIO_SAMPLE_SIZE.into());

    let mut builder = Uac2::builder()
        .with_capture_config(capture)
        .with_playback_config(playback);
    builder.function_name = Some("NanoKVM Audio".to_owned());
    builder.control_name = Some("NanoKVM Audio Control".to_owned());
    builder.clock_source_out_name = Some("NanoKVM Audio Clock".to_owned());
    let (_, function) = builder.build();
    function
}

fn ensure_configfs() -> io::Result<()> {
    let root = Path::new("/sys/kernel/config/usb_gadget");
    if root.exists() {
        return Ok(());
    }
    let status = Command::new("mount")
        .args(["-t", "configfs", "none", "/sys/kernel/config"])
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other("failed to mount configfs"))
    }
}

fn remove_existing_gadget(config: &GadgetConfig) -> io::Result<()> {
    let Some(name) = gadget_name(config) else {
        return Ok(());
    };
    for gadget in usb_gadget::registered()? {
        if gadget.name() == name {
            gadget.remove()?;
            return Ok(());
        }
    }
    Ok(())
}

fn gadget_name(config: &GadgetConfig) -> Option<&str> {
    config.gadget_dir.file_name().and_then(|name| name.to_str())
}

fn find_udc(name: &str) -> io::Result<Udc> {
    usb_gadget::udcs()?
        .into_iter()
        .find(|udc| udc.name() == name)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("UDC {name} not found")))
}

fn parse_hex_u16(value: &str) -> io::Result<u16> {
    let value = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    u16::from_str_radix(value, 16).map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))
}
