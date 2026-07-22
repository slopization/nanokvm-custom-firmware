use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use usb_gadget::function::audio::{Channel, Uac2};
use usb_gadget::function::hid::Hid;
use usb_gadget::{Class, Config, Gadget, Id, RegGadget, Strings, Udc, UsbVersion};

use crate::audio::{UAC2_CHANNELS, UAC2_SAMPLE_RATE, UAC2_SAMPLE_SIZE};
use crate::cli::Args;
use crate::hid_report::{HID_REPORT_LEN, report_descriptor};

#[derive(Debug, Clone)]
pub struct GadgetConfig {
    pub gadget_dir: PathBuf,
    pub udc: Option<String>,
    pub vid: String,
    pub pid: String,
    pub serial: String,
    pub manufacturer: String,
    pub product: String,
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
        }
    }
}

pub fn setup_usb_gadget(config: &GadgetConfig) -> io::Result<RegGadget> {
    ensure_configfs()?;
    remove_existing_gadget(config)?;

    let mut hid = Hid::builder();
    hid.sub_class = 0;
    hid.protocol = 0;
    hid.report_len = HID_REPORT_LEN as u8;
    hid.report_desc = report_descriptor();
    let (hid, function) = hid.build();

    let channel_mask = (1_u32 << UAC2_CHANNELS) - 1;
    let (uac2, audio_function) = Uac2::new(
        Channel::new(channel_mask, UAC2_SAMPLE_RATE, UAC2_SAMPLE_SIZE.into()),
        Channel::new(0, UAC2_SAMPLE_RATE, UAC2_SAMPLE_SIZE.into()),
    );

    let mut usb_config = Config::new("HID + UAC2")
        .with_function(function)
        .with_function(audio_function);
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
        "UAC2 host playback input: {} channels, {} Hz, {}-bit, status {:?}",
        UAC2_CHANNELS,
        UAC2_SAMPLE_RATE,
        u32::from(UAC2_SAMPLE_SIZE) * 8,
        uac2.status()
    );
    Ok(reg)
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
