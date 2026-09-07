use std::io;

use serde::Serialize;

pub const KEYBOARD_REPORT_LEN: usize = 8;
pub const RELATIVE_MOUSE_REPORT_LEN: usize = 4;
pub const ABSOLUTE_MOUSE_REPORT_LEN: usize = 6;
pub const POINTER_AXIS_MAX: u16 = 32_767;

#[derive(Serialize)]
pub struct KeyboardReport {
    pub modifiers: u8,
    pub reserved: u8,
    pub keys: [u8; 6],
}

#[derive(Serialize)]
pub struct RelativeMouseReport {
    pub buttons: u8,
    pub x: i8,
    pub y: i8,
    pub wheel: i8,
}

#[derive(Serialize)]
pub struct AbsolutePointerReport {
    pub buttons: u8,
    pub x: u16,
    pub y: u16,
    pub wheel: i8,
}

pub fn keyboard_report_descriptor() -> Vec<u8> {
    vec![
        0x05, 0x01, // Usage Page (Generic Desktop)
        0x09, 0x06, // Usage (Keyboard)
        0xa1, 0x01, // Collection (Application)
        0x05, 0x07, // Usage Page (Keyboard/Keypad)
        0x19, 0xe0, // Usage Minimum (Left Control)
        0x29, 0xe7, // Usage Maximum (Right GUI)
        0x15, 0x00, // Logical Minimum (0)
        0x25, 0x01, // Logical Maximum (1)
        0x75, 0x01, // Report Size (1)
        0x95, 0x08, // Report Count (8)
        0x81, 0x02, // Input (Data, Variable, Absolute)
        0x95, 0x01, // Report Count (1)
        0x75, 0x08, // Report Size (8)
        0x81, 0x03, // Input (Constant, Variable, Absolute)
        0x95, 0x05, // Report Count (5)
        0x75, 0x01, // Report Size (1)
        0x05, 0x08, // Usage Page (LEDs)
        0x19, 0x01, // Usage Minimum (Num Lock)
        0x29, 0x05, // Usage Maximum (Kana)
        0x91, 0x02, // Output (Data, Variable, Absolute)
        0x95, 0x01, // Report Count (1)
        0x75, 0x03, // Report Size (3)
        0x91, 0x03, // Output (Constant, Variable, Absolute)
        0x95, 0x06, // Report Count (6)
        0x75, 0x08, // Report Size (8)
        0x15, 0x00, // Logical Minimum (0)
        0x25, 0x65, // Logical Maximum (101)
        0x05, 0x07, // Usage Page (Keyboard/Keypad)
        0x19, 0x00, // Usage Minimum (0)
        0x29, 0x65, // Usage Maximum (101)
        0x81, 0x00, // Input (Data, Array, Absolute)
        0xc0, // End Collection
    ]
}

pub fn relative_mouse_report_descriptor() -> Vec<u8> {
    vec![
        0x05, 0x01, // Usage Page (Generic Desktop)
        0x09, 0x02, // Usage (Mouse)
        0xa1, 0x01, // Collection (Application)
        0x09, 0x01, // Usage (Pointer)
        0xa1, 0x00, // Collection (Physical)
        0x05, 0x09, // Usage Page (Button)
        0x19, 0x01, // Usage Minimum (Button 1)
        0x29, 0x05, // Usage Maximum (Button 5)
        0x15, 0x00, // Logical Minimum (0)
        0x25, 0x01, // Logical Maximum (1)
        0x95, 0x05, // Report Count (5)
        0x75, 0x01, // Report Size (1)
        0x81, 0x02, // Input (Data, Variable, Absolute)
        0x95, 0x01, // Report Count (1)
        0x75, 0x03, // Report Size (3)
        0x81, 0x03, // Input (Constant, Variable, Absolute)
        0x05, 0x01, // Usage Page (Generic Desktop)
        0x09, 0x30, // Usage (X)
        0x09, 0x31, // Usage (Y)
        0x09, 0x38, // Usage (Wheel)
        0x15, 0x81, // Logical Minimum (-127)
        0x25, 0x7f, // Logical Maximum (127)
        0x75, 0x08, // Report Size (8)
        0x95, 0x03, // Report Count (3)
        0x81, 0x06, // Input (Data, Variable, Relative)
        0xc0, // End Collection
        0xc0, // End Collection
    ]
}

pub fn absolute_mouse_report_descriptor() -> Vec<u8> {
    vec![
        0x05, 0x01, // Usage Page (Generic Desktop)
        0x09, 0x02, // Usage (Mouse)
        0xa1, 0x01, // Collection (Application)
        0x09, 0x01, // Usage (Pointer)
        0xa1, 0x00, // Collection (Physical)
        0x05, 0x09, // Usage Page (Button)
        0x19, 0x01, // Usage Minimum (Button 1)
        0x29, 0x05, // Usage Maximum (Button 5)
        0x15, 0x00, // Logical Minimum (0)
        0x25, 0x01, // Logical Maximum (1)
        0x95, 0x05, // Report Count (5)
        0x75, 0x01, // Report Size (1)
        0x81, 0x02, // Input (Data, Variable, Absolute)
        0x95, 0x01, // Report Count (1)
        0x75, 0x03, // Report Size (3)
        0x81, 0x01, // Input (Constant, Variable, Absolute)
        0x05, 0x01, // Usage Page (Generic Desktop)
        0x09, 0x30, // Usage (X)
        0x09, 0x31, // Usage (Y)
        0x15, 0x00, // Logical Minimum (0)
        0x26, 0xff, 0x7f, // Logical Maximum (32767)
        0x35, 0x00, // Physical Minimum (0)
        0x46, 0xff, 0x7f, // Physical Maximum (32767)
        0x75, 0x10, // Report Size (16)
        0x95, 0x02, // Report Count (2)
        0x81, 0x02, // Input (Data, Variable, Absolute)
        0x05, 0x01, // Usage Page (Generic Desktop)
        0x09, 0x38, // Usage (Wheel)
        0x15, 0x81, // Logical Minimum (-127)
        0x25, 0x7f, // Logical Maximum (127)
        0x35, 0x00, // Physical Minimum (0)
        0x45, 0x00, // Physical Maximum (0)
        0x75, 0x08, // Report Size (8)
        0x95, 0x01, // Report Count (1)
        0x81, 0x06, // Input (Data, Variable, Relative)
        0xc0, // End Collection
        0xc0, // End Collection
    ]
}

pub fn encode_keyboard(report: &KeyboardReport) -> io::Result<[u8; KEYBOARD_REPORT_LEN]> {
    encode_report(report)
}

pub fn encode_relative_mouse(
    report: &RelativeMouseReport,
) -> io::Result<[u8; RELATIVE_MOUSE_REPORT_LEN]> {
    encode_report(report)
}

pub fn encode_absolute_pointer(
    report: &AbsolutePointerReport,
) -> io::Result<[u8; ABSOLUTE_MOUSE_REPORT_LEN]> {
    encode_report(report)
}

fn encode_report<T: Serialize, const N: usize>(report: &T) -> io::Result<[u8; N]> {
    let mut bytes = [0; N];
    ssmarshal::serialize(&mut bytes, report)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_standard_hid_reports_without_report_ids() {
        assert_eq!(
            encode_keyboard(&KeyboardReport {
                modifiers: 2,
                reserved: 0,
                keys: [4, 0, 0, 0, 0, 0],
            })
            .unwrap(),
            [2, 0, 4, 0, 0, 0, 0, 0]
        );
        assert_eq!(
            encode_relative_mouse(&RelativeMouseReport {
                buttons: 1,
                x: -2,
                y: 3,
                wheel: -1,
            })
            .unwrap(),
            [1, 0xfe, 3, 0xff]
        );
        assert_eq!(
            encode_absolute_pointer(&AbsolutePointerReport {
                buttons: 1,
                x: 0x1234,
                y: 0x5678,
                wheel: -1,
            })
            .unwrap(),
            [1, 0x34, 0x12, 0x78, 0x56, 0xff]
        );
    }

    #[test]
    fn descriptors_start_with_expected_usages() {
        assert_eq!(
            &keyboard_report_descriptor()[..4],
            &[0x05, 0x01, 0x09, 0x06]
        );
        assert_eq!(
            &relative_mouse_report_descriptor()[..6],
            &[0x05, 0x01, 0x09, 0x02, 0xa1, 0x01]
        );
        assert_eq!(
            &absolute_mouse_report_descriptor()[..6],
            &[0x05, 0x01, 0x09, 0x02, 0xa1, 0x01]
        );
    }
}
