use std::io;

use hidreport::hid::{CollectionItem, ItemBuilder, ReportDescriptorBuilder};
use hidreport::types::{
    LogicalMaximum, LogicalMinimum, ReportCount, ReportId, ReportSize, UsageId, UsageMaximum,
    UsageMinimum, UsagePage,
};
use serde::Serialize;
use usbd_hid::descriptor::generator_prelude::*;

pub const HID_REPORT_LEN: usize = 9;
pub const POINTER_AXIS_MAX: u16 = 32_767;

const KEYBOARD_REPORT_ID: u8 = 1;
const RELATIVE_MOUSE_REPORT_ID: u8 = 2;
const ABSOLUTE_POINTER_REPORT_ID: u8 = 3;
const TOUCHSCREEN_REPORT_ID: u8 = 4;

#[gen_hid_descriptor(
    (collection = APPLICATION, usage_page = GENERIC_DESKTOP, usage = KEYBOARD) = {
        (report_id = 1,) = {
            (usage_page = KEYBOARD, usage_min = 0xE0, usage_max = 0xE7) = {
                #[packed_bits = 8] #[item_settings(data,variable,absolute)] modifiers=input;
            };
            #[item_settings(constant,variable,absolute)] reserved=input;
            (usage_page = KEYBOARD, usage_min = 0x00, usage_max = 0x65) = {
                #[item_settings(data,array,absolute)] keys=input;
            };
        };
    }
)]
#[derive(Serialize)]
pub struct KeyboardReport {
    pub modifiers: u8,
    pub reserved: u8,
    pub keys: [u8; 6],
}

#[gen_hid_descriptor(
    (collection = APPLICATION, usage_page = GENERIC_DESKTOP, usage = MOUSE) = {
        (collection = PHYSICAL, usage = POINTER) = {
            (report_id = 2,) = {
                (usage_page = BUTTON, usage_min = BUTTON_1, usage_max = BUTTON_8) = {
                    #[packed_bits = 8] #[item_settings(data,variable,absolute)] buttons=input;
                };
                (usage_page = GENERIC_DESKTOP,) = {
                    (usage = X,) = { #[item_settings(data,variable,relative)] x=input; };
                    (usage = Y,) = { #[item_settings(data,variable,relative)] y=input; };
                    (usage = WHEEL,) = { #[item_settings(data,variable,relative)] wheel=input; };
                };
            };
        };
    }
)]
#[derive(Serialize)]
pub struct RelativeMouseReport {
    pub buttons: u8,
    pub x: i8,
    pub y: i8,
    pub wheel: i8,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct AbsolutePointerReport {
    pub buttons: u8,
    pub x: u16,
    pub y: u16,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct TouchscreenReport {
    pub flags: u8,
    pub contact_id: u8,
    pub x: u16,
    pub y: u16,
    pub contact_count: u8,
}

pub fn report_descriptor() -> Vec<u8> {
    [
        KeyboardReport::desc(),
        RelativeMouseReport::desc(),
        &absolute_pointer_descriptor(),
        &touchscreen_descriptor(),
    ]
    .concat()
}

fn touchscreen_descriptor() -> Vec<u8> {
    ReportDescriptorBuilder::new()
        .append(UsagePage::from(0x0d).into())
        .append(UsageId::from(0x04).into())
        .open_collection(CollectionItem::Application)
        .append(ReportId::from(TOUCHSCREEN_REPORT_ID).into())
        .append(UsageId::from(0x22).into())
        .open_collection(CollectionItem::Logical)
        .append(UsageId::from(0x42).into())
        .append(UsageId::from(0x32).into())
        .append(LogicalMinimum::from(0).into())
        .append(LogicalMaximum::from(1).into())
        .append(ReportSize::from(1).into())
        .append(ReportCount::from(2).into())
        .input(ItemBuilder::new().variable().absolute().input())
        .append(ReportCount::from(6).into())
        .input(ItemBuilder::new().constant().input())
        .append(UsageId::from(0x51).into())
        .append(LogicalMaximum::from(255).into())
        .append(ReportSize::from(8).into())
        .append(ReportCount::from(1).into())
        .input(ItemBuilder::new().variable().absolute().input())
        .append(UsagePage::from(0x01).into())
        .append(UsageId::from(0x30).into())
        .append(UsageId::from(0x31).into())
        .append(LogicalMaximum::from(i32::from(POINTER_AXIS_MAX)).into())
        .append(ReportSize::from(16).into())
        .append(ReportCount::from(2).into())
        .input(ItemBuilder::new().variable().absolute().input())
        .close_collection()
        .append(UsagePage::from(0x0d).into())
        .append(UsageId::from(0x54).into())
        .append(LogicalMaximum::from(1).into())
        .append(ReportSize::from(8).into())
        .append(ReportCount::from(1).into())
        .input(ItemBuilder::new().variable().absolute().input())
        .close_collection()
        .build()
}

fn absolute_pointer_descriptor() -> Vec<u8> {
    ReportDescriptorBuilder::new()
        .append(UsagePage::from(0x01).into())
        .append(UsageId::from(0x02).into())
        .open_collection(CollectionItem::Application)
        .append(UsageId::from(0x01).into())
        .open_collection(CollectionItem::Physical)
        .append(ReportId::from(ABSOLUTE_POINTER_REPORT_ID).into())
        .append(UsagePage::from(0x09).into())
        .append(UsageMinimum::from(1).into())
        .append(UsageMaximum::from(8).into())
        .append(LogicalMinimum::from(0).into())
        .append(LogicalMaximum::from(1).into())
        .append(ReportSize::from(1).into())
        .append(ReportCount::from(8).into())
        .input(ItemBuilder::new().variable().absolute().input())
        .append(UsagePage::from(0x01).into())
        .append(UsageId::from(0x30).into())
        .append(UsageId::from(0x31).into())
        .append(LogicalMinimum::from(0).into())
        .append(LogicalMaximum::from(i32::from(POINTER_AXIS_MAX)).into())
        .append(ReportSize::from(16).into())
        .append(ReportCount::from(2).into())
        .input(ItemBuilder::new().variable().absolute().input())
        .close_collection()
        .close_collection()
        .build()
}

pub fn encode_keyboard(report: &KeyboardReport) -> io::Result<[u8; HID_REPORT_LEN]> {
    encode(KEYBOARD_REPORT_ID, report)
}

pub fn encode_relative_mouse(report: &RelativeMouseReport) -> io::Result<[u8; HID_REPORT_LEN]> {
    encode(RELATIVE_MOUSE_REPORT_ID, report)
}

pub fn encode_absolute_pointer(report: &AbsolutePointerReport) -> io::Result<[u8; HID_REPORT_LEN]> {
    encode(ABSOLUTE_POINTER_REPORT_ID, report)
}

pub fn encode_touchscreen(report: &TouchscreenReport) -> io::Result<[u8; HID_REPORT_LEN]> {
    encode(TOUCHSCREEN_REPORT_ID, report)
}

fn encode<T: Serialize>(report_id: u8, report: &T) -> io::Result<[u8; HID_REPORT_LEN]> {
    let mut bytes = [0; HID_REPORT_LEN];
    bytes[0] = report_id;
    ssmarshal::serialize(&mut bytes[1..], report)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_all_reports_with_their_ids() {
        assert_eq!(
            encode_keyboard(&KeyboardReport {
                modifiers: 2,
                reserved: 0,
                keys: [4, 0, 0, 0, 0, 0],
            })
            .unwrap(),
            [1, 2, 0, 4, 0, 0, 0, 0, 0]
        );
        assert_eq!(
            encode_relative_mouse(&RelativeMouseReport {
                buttons: 1,
                x: -2,
                y: 3,
                wheel: -1,
            })
            .unwrap(),
            [2, 1, 0xfe, 3, 0xff, 0, 0, 0, 0]
        );
        assert_eq!(
            encode_absolute_pointer(&AbsolutePointerReport {
                buttons: 1,
                x: 0x1234,
                y: 0x5678,
            })
            .unwrap(),
            [3, 1, 0x34, 0x12, 0x78, 0x56, 0, 0, 0]
        );
        assert_eq!(
            encode_touchscreen(&TouchscreenReport {
                flags: 3,
                contact_id: 7,
                x: 0x1234,
                y: 0x5678,
                contact_count: 1,
            })
            .unwrap(),
            [4, 3, 7, 0x34, 0x12, 0x78, 0x56, 1, 0]
        );
    }
}
