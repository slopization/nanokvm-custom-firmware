use thiserror::Error;

const HEADER_LEN: usize = 8;

pub const KEY_DOWN_EVENT_MAGIC: u32 = 0x0000_0003;
pub const KEY_UP_EVENT_MAGIC: u32 = 0x0000_0004;
pub const MOUSE_MOVE_ABS_MAGIC: u32 = 0x0000_0005;
pub const MOUSE_MOVE_REL_MAGIC: u32 = 0x0000_0006;
pub const MOUSE_MOVE_REL_MAGIC_GEN5: u32 = 0x0000_0007;
pub const MOUSE_BUTTON_DOWN_EVENT_MAGIC_GEN5: u32 = 0x0000_0008;
pub const MOUSE_BUTTON_UP_EVENT_MAGIC_GEN5: u32 = 0x0000_0009;
pub const SCROLL_MAGIC: u32 = 0x0000_0009;
pub const SCROLL_MAGIC_GEN5: u32 = 0x0000_000a;
pub const CONTROLLER_MAGIC: u32 = 0x0000_000a;
pub const MULTI_CONTROLLER_MAGIC: u32 = 0x0000_000d;
pub const MULTI_CONTROLLER_MAGIC_GEN5: u32 = 0x0000_000c;
pub const ENABLE_HAPTICS_MAGIC: u32 = 0x0000_000d;
pub const UTF8_TEXT_EVENT_MAGIC: u32 = 0x0000_0017;
pub const SS_HSCROLL_MAGIC: u32 = 0x5500_0001;
pub const SS_TOUCH_MAGIC: u32 = 0x5500_0002;
pub const SS_PEN_MAGIC: u32 = 0x5500_0003;
pub const SS_CONTROLLER_ARRIVAL_MAGIC: u32 = 0x5500_0004;
pub const SS_CONTROLLER_TOUCH_MAGIC: u32 = 0x5500_0005;
pub const SS_CONTROLLER_MOTION_MAGIC: u32 = 0x5500_0006;
pub const SS_CONTROLLER_BATTERY_MAGIC: u32 = 0x5500_0007;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum InputParseError {
    #[error("input packet is shorter than header")]
    ShortHeader,
    #[error("declared packet length {declared} does not match actual payload length {actual}")]
    LengthMismatch { declared: usize, actual: usize },
    #[error("packet 0x{magic:08x} is too short: expected at least {expected}, got {actual}")]
    ShortPacket {
        magic: u32,
        expected: usize,
        actual: usize,
    },
    #[error("unknown input packet magic 0x{0:08x}")]
    UnknownMagic(u32),
    #[error("text packet is not valid UTF-8")]
    InvalidUtf8,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InputEvent {
    pub kind: InputEventKind,
    pub raw_packet: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum InputEventKind {
    Keyboard(KeyboardEvent),
    Text(String),
    Mouse(MouseEvent),
    Scroll(ScrollEvent),
    Controller(ControllerStateEvent),
    ControllerArrival(ControllerArrivalEvent),
    ControllerTouch(ControllerTouchEvent),
    ControllerMotion(ControllerMotionEvent),
    ControllerBattery(ControllerBatteryEvent),
    Touch(TouchEvent),
    Pen(PenEvent),
    HapticsEnabled(bool),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyboardEvent {
    pub pressed: bool,
    pub flags: u8,
    pub key_code: u16,
    pub modifiers: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MouseEvent {
    Relative {
        dx: i16,
        dy: i16,
    },
    Absolute {
        x: i16,
        y: i16,
        width: i16,
        height: i16,
    },
    Button {
        pressed: bool,
        button: u8,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScrollEvent {
    Vertical { amount_1: i16, amount_2: i16 },
    Horizontal { amount: i16 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerStateEvent {
    pub controller_number: Option<u16>,
    pub active_gamepad_mask: Option<u16>,
    pub button_flags: u16,
    pub button_flags_2: u16,
    pub left_trigger: u8,
    pub right_trigger: u8,
    pub left_stick_x: i16,
    pub left_stick_y: i16,
    pub right_stick_x: i16,
    pub right_stick_y: i16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerArrivalEvent {
    pub controller_number: u8,
    pub controller_type: u8,
    pub capabilities: u16,
    pub supported_button_flags: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ControllerTouchEvent {
    pub controller_number: u8,
    pub event_type: u8,
    pub touchpad_index: u8,
    pub pointer_id: u32,
    pub x: f32,
    pub y: f32,
    pub pressure: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ControllerMotionEvent {
    pub controller_number: u8,
    pub motion_type: u8,
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerBatteryEvent {
    pub controller_number: u8,
    pub battery_state: u8,
    pub battery_percentage: u8,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TouchEvent {
    pub event_type: u8,
    pub rotation: u16,
    pub pointer_id: u32,
    pub x: f32,
    pub y: f32,
    pub pressure_or_distance: f32,
    pub contact_area_major: f32,
    pub contact_area_minor: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PenEvent {
    pub event_type: u8,
    pub tool_type: u8,
    pub pen_buttons: u8,
    pub x: f32,
    pub y: f32,
    pub pressure_or_distance: f32,
    pub rotation: u16,
    pub tilt: u8,
    pub contact_area_major: f32,
    pub contact_area_minor: f32,
}

pub fn parse_input_packet(raw: &[u8]) -> Result<InputEvent, InputParseError> {
    if raw.len() < HEADER_LEN {
        return Err(InputParseError::ShortHeader);
    }

    let declared = u32::from_be_bytes(raw[0..4].try_into().expect("slice length")) as usize;
    let actual = raw.len() - 4;
    if declared != actual {
        return Err(InputParseError::LengthMismatch { declared, actual });
    }

    let magic = u32::from_le_bytes(raw[4..8].try_into().expect("slice length"));
    let body = &raw[8..];
    let kind = match magic {
        KEY_DOWN_EVENT_MAGIC | KEY_UP_EVENT_MAGIC => {
            require(magic, body, 6)?;
            InputEventKind::Keyboard(KeyboardEvent {
                pressed: magic == KEY_DOWN_EVENT_MAGIC,
                flags: body[0],
                key_code: read_u16_le(body, 1),
                modifiers: body[3],
            })
        }
        UTF8_TEXT_EVENT_MAGIC => {
            let end = body.iter().position(|b| *b == 0).unwrap_or(body.len());
            let text = std::str::from_utf8(&body[..end])
                .map_err(|_| InputParseError::InvalidUtf8)?
                .to_owned();
            InputEventKind::Text(text)
        }
        MOUSE_MOVE_REL_MAGIC | MOUSE_MOVE_REL_MAGIC_GEN5 => {
            require(magic, body, 4)?;
            InputEventKind::Mouse(MouseEvent::Relative {
                dx: read_i16_be(body, 0),
                dy: read_i16_be(body, 2),
            })
        }
        MOUSE_MOVE_ABS_MAGIC => {
            require(magic, body, 10)?;
            InputEventKind::Mouse(MouseEvent::Absolute {
                x: read_i16_be(body, 0),
                y: read_i16_be(body, 2),
                width: read_i16_be(body, 6),
                height: read_i16_be(body, 8),
            })
        }
        MOUSE_BUTTON_DOWN_EVENT_MAGIC_GEN5 | MOUSE_BUTTON_UP_EVENT_MAGIC_GEN5 => {
            require(magic, body, 1)?;
            InputEventKind::Mouse(MouseEvent::Button {
                pressed: magic == MOUSE_BUTTON_DOWN_EVENT_MAGIC_GEN5,
                button: body[0],
            })
        }
        SCROLL_MAGIC_GEN5 => parse_scroll_or_controller(magic, body)?,
        SS_HSCROLL_MAGIC => {
            require(magic, body, 2)?;
            InputEventKind::Scroll(ScrollEvent::Horizontal {
                amount: read_i16_be(body, 0),
            })
        }
        MULTI_CONTROLLER_MAGIC | MULTI_CONTROLLER_MAGIC_GEN5 => {
            if body.len() == 2 {
                InputEventKind::HapticsEnabled(read_u16_le(body, 0) != 0)
            } else {
                require(magic, body, 22)?;
                InputEventKind::Controller(ControllerStateEvent {
                    controller_number: Some(read_u16_le(body, 2)),
                    active_gamepad_mask: Some(read_u16_le(body, 4)),
                    button_flags: read_u16_le(body, 8),
                    button_flags_2: read_u16_le(body, 18),
                    left_trigger: body[10],
                    right_trigger: body[11],
                    left_stick_x: read_i16_le(body, 12),
                    left_stick_y: read_i16_le(body, 14),
                    right_stick_x: read_i16_le(body, 16),
                    right_stick_y: read_i16_le(body, 18),
                })
            }
        }
        SS_TOUCH_MAGIC => {
            require(magic, body, 30)?;
            InputEventKind::Touch(TouchEvent {
                event_type: body[0],
                rotation: read_u16_le(body, 2),
                pointer_id: read_u32_le(body, 4),
                x: read_f32_le(body, 8),
                y: read_f32_le(body, 12),
                pressure_or_distance: read_f32_le(body, 16),
                contact_area_major: read_f32_le(body, 20),
                contact_area_minor: read_f32_le(body, 24),
            })
        }
        SS_PEN_MAGIC => {
            require(magic, body, 31)?;
            InputEventKind::Pen(PenEvent {
                event_type: body[0],
                tool_type: body[1],
                pen_buttons: body[2],
                x: read_f32_le(body, 4),
                y: read_f32_le(body, 8),
                pressure_or_distance: read_f32_le(body, 12),
                rotation: read_u16_le(body, 16),
                tilt: body[18],
                contact_area_major: read_f32_le(body, 20),
                contact_area_minor: read_f32_le(body, 24),
            })
        }
        SS_CONTROLLER_ARRIVAL_MAGIC => {
            require(magic, body, 8)?;
            InputEventKind::ControllerArrival(ControllerArrivalEvent {
                controller_number: body[0],
                controller_type: body[1],
                capabilities: read_u16_le(body, 2),
                supported_button_flags: read_u32_le(body, 4),
            })
        }
        SS_CONTROLLER_TOUCH_MAGIC => {
            require(magic, body, 20)?;
            InputEventKind::ControllerTouch(ControllerTouchEvent {
                controller_number: body[0],
                event_type: body[1],
                touchpad_index: body[3],
                pointer_id: read_u32_le(body, 4),
                x: read_f32_le(body, 8),
                y: read_f32_le(body, 12),
                pressure: read_f32_le(body, 16),
            })
        }
        SS_CONTROLLER_MOTION_MAGIC => {
            require(magic, body, 16)?;
            InputEventKind::ControllerMotion(ControllerMotionEvent {
                controller_number: body[0],
                motion_type: body[1],
                x: read_f32_le(body, 4),
                y: read_f32_le(body, 8),
                z: read_f32_le(body, 12),
            })
        }
        SS_CONTROLLER_BATTERY_MAGIC => {
            require(magic, body, 4)?;
            InputEventKind::ControllerBattery(ControllerBatteryEvent {
                controller_number: body[0],
                battery_state: body[1],
                battery_percentage: body[2],
            })
        }
        other => return Err(InputParseError::UnknownMagic(other)),
    };

    Ok(InputEvent {
        kind,
        raw_packet: raw.to_vec(),
    })
}

fn parse_scroll_or_controller(magic: u32, body: &[u8]) -> Result<InputEventKind, InputParseError> {
    if body.len() >= 18 {
        Ok(InputEventKind::Controller(ControllerStateEvent {
            controller_number: None,
            active_gamepad_mask: None,
            button_flags: read_u16_le(body, 2),
            button_flags_2: 0,
            left_trigger: body[4],
            right_trigger: body[5],
            left_stick_x: read_i16_le(body, 6),
            left_stick_y: read_i16_le(body, 8),
            right_stick_x: read_i16_le(body, 10),
            right_stick_y: read_i16_le(body, 12),
        }))
    } else {
        require(magic, body, 6)?;
        Ok(InputEventKind::Scroll(ScrollEvent::Vertical {
            amount_1: read_i16_be(body, 0),
            amount_2: read_i16_be(body, 2),
        }))
    }
}

fn require(magic: u32, body: &[u8], expected: usize) -> Result<(), InputParseError> {
    if body.len() < expected {
        Err(InputParseError::ShortPacket {
            magic,
            expected,
            actual: body.len(),
        })
    } else {
        Ok(())
    }
}

fn read_u16_le(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().expect("slice length"))
}

fn read_i16_le(bytes: &[u8], offset: usize) -> i16 {
    i16::from_le_bytes(bytes[offset..offset + 2].try_into().expect("slice length"))
}

fn read_i16_be(bytes: &[u8], offset: usize) -> i16 {
    i16::from_be_bytes(bytes[offset..offset + 2].try_into().expect("slice length"))
}

fn read_u32_le(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("slice length"))
}

fn read_f32_le(bytes: &[u8], offset: usize) -> f32 {
    f32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("slice length"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(magic: u32, body: &[u8]) -> Vec<u8> {
        let size = (4 + body.len()) as u32;
        let mut out = Vec::new();
        out.extend_from_slice(&size.to_be_bytes());
        out.extend_from_slice(&magic.to_le_bytes());
        out.extend_from_slice(body);
        out
    }

    #[test]
    fn parses_keyboard_down() {
        let raw = packet(KEY_DOWN_EVENT_MAGIC, &[0, 42, 0, 2, 0, 0]);
        let event = parse_input_packet(&raw).unwrap();
        assert_eq!(
            event.kind,
            InputEventKind::Keyboard(KeyboardEvent {
                pressed: true,
                flags: 0,
                key_code: 42,
                modifiers: 2,
            })
        );
        assert_eq!(event.raw_packet, raw);
    }

    #[test]
    fn parses_relative_mouse_gen5() {
        let raw = packet(MOUSE_MOVE_REL_MAGIC_GEN5, &[0, 5, 0xff, 0xfe]);
        let event = parse_input_packet(&raw).unwrap();
        assert_eq!(
            event.kind,
            InputEventKind::Mouse(MouseEvent::Relative { dx: 5, dy: -2 })
        );
    }

    #[test]
    fn parses_absolute_mouse_in_network_order() {
        let raw = packet(
            MOUSE_MOVE_ABS_MAGIC,
            &[0x03, 0xc0, 0x02, 0x1c, 0, 0, 0x07, 0x7f, 0x04, 0x37],
        );
        let event = parse_input_packet(&raw).unwrap();
        assert_eq!(
            event.kind,
            InputEventKind::Mouse(MouseEvent::Absolute {
                x: 960,
                y: 540,
                width: 1919,
                height: 1079,
            })
        );
    }

    #[test]
    fn rejects_length_mismatch() {
        let raw = [0, 0, 0, 5, 1, 0, 0, 0];
        assert!(matches!(
            parse_input_packet(&raw),
            Err(InputParseError::LengthMismatch {
                declared: 5,
                actual: 4
            })
        ));
    }
}
