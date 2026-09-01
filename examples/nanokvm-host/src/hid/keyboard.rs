use moonlink_protocol::KeyboardEvent;

use super::{UsbGadgetInputSink, write_report};
use crate::hid_report::{KeyboardReport, encode_keyboard};

#[derive(Debug, Clone, Copy)]
pub(super) enum KeyboardUsage {
    Key(u8),
    Modifier(u8),
}

impl UsbGadgetInputSink {
    pub(super) async fn write_keyboard(&self, event: KeyboardEvent) -> std::io::Result<()> {
        let Some(usage) = map_keyboard_usage(event.key_code) else {
            println!("unmapped key code: {}", event.key_code);
            return Ok(());
        };

        self.write_keyboard_usage(Some(usage), event.pressed).await
    }

    pub(super) async fn write_keyboard_usage(
        &self,
        usage: Option<KeyboardUsage>,
        pressed: bool,
    ) -> std::io::Result<()> {
        let Some(file) = &self.hid else {
            return Ok(());
        };
        let Some(usage) = usage else {
            return Ok(());
        };
        let mut state = self.state.lock().await;
        match usage {
            KeyboardUsage::Modifier(bit) => {
                if pressed {
                    state.keyboard.modifiers |= bit;
                } else {
                    state.keyboard.modifiers &= !bit;
                }
            }
            KeyboardUsage::Key(code) => {
                if pressed {
                    if !state.keyboard.keys.contains(&code) && state.keyboard.keys.len() < 6 {
                        state.keyboard.keys.push(code);
                    }
                } else {
                    state.keyboard.keys.retain(|key| *key != code);
                }
            }
        }

        let mut keys = [0; 6];
        for (slot, key) in state.keyboard.keys.iter().take(6).enumerate() {
            keys[slot] = *key;
        }
        let report = encode_keyboard(&KeyboardReport {
            modifiers: state.keyboard.modifiers,
            reserved: 0,
            keys,
        })?;
        drop(state);
        write_report(file, &report).await
    }

    pub async fn write_text(&self, text: &str, delay: std::time::Duration) -> std::io::Result<()> {
        let strokes = text
            .chars()
            .map(map_text_character)
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "text contains characters unavailable in the en-us keymap",
                )
            })?;
        for (usage, shifted) in strokes {
            if shifted {
                self.write_keyboard_usage(Some(KeyboardUsage::Modifier(0x02)), true)
                    .await?;
            }
            self.write_keyboard_usage(Some(KeyboardUsage::Key(usage)), true)
                .await?;
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            self.write_keyboard_usage(Some(KeyboardUsage::Key(usage)), false)
                .await?;
            if shifted {
                self.write_keyboard_usage(Some(KeyboardUsage::Modifier(0x02)), false)
                    .await?;
            }
        }
        Ok(())
    }
}

fn map_text_character(character: char) -> Option<(u8, bool)> {
    let mapped = match character {
        'a'..='z' => (0x04 + character as u8 - b'a', false),
        'A'..='Z' => (0x04 + character as u8 - b'A', true),
        '1'..='9' => (0x1e + character as u8 - b'1', false),
        '0' => (0x27, false),
        '\n' | '\r' => (0x28, false),
        '\t' => (0x2b, false),
        ' ' => (0x2c, false),
        '-' => (0x2d, false),
        '_' => (0x2d, true),
        '=' => (0x2e, false),
        '+' => (0x2e, true),
        '[' => (0x2f, false),
        '{' => (0x2f, true),
        ']' => (0x30, false),
        '}' => (0x30, true),
        '\\' => (0x31, false),
        '|' => (0x31, true),
        ';' => (0x33, false),
        ':' => (0x33, true),
        '\'' => (0x34, false),
        '"' => (0x34, true),
        '`' => (0x35, false),
        '~' => (0x35, true),
        ',' => (0x36, false),
        '<' => (0x36, true),
        '.' => (0x37, false),
        '>' => (0x37, true),
        '/' => (0x38, false),
        '?' => (0x38, true),
        '!' => (0x1e, true),
        '@' => (0x1f, true),
        '#' => (0x20, true),
        '$' => (0x21, true),
        '%' => (0x22, true),
        '^' => (0x23, true),
        '&' => (0x24, true),
        '*' => (0x25, true),
        '(' => (0x26, true),
        ')' => (0x27, true),
        _ => return None,
    };
    Some(mapped)
}

pub(super) fn map_dom_code(code: &str) -> Option<KeyboardUsage> {
    let usage = match code {
        "ControlLeft" => return Some(KeyboardUsage::Modifier(0x01)),
        "ShiftLeft" => return Some(KeyboardUsage::Modifier(0x02)),
        "AltLeft" => return Some(KeyboardUsage::Modifier(0x04)),
        "MetaLeft" => return Some(KeyboardUsage::Modifier(0x08)),
        "ControlRight" => return Some(KeyboardUsage::Modifier(0x10)),
        "ShiftRight" => return Some(KeyboardUsage::Modifier(0x20)),
        "AltRight" => return Some(KeyboardUsage::Modifier(0x40)),
        "MetaRight" => return Some(KeyboardUsage::Modifier(0x80)),
        "Enter" => 0x28,
        "Escape" => 0x29,
        "Backspace" => 0x2a,
        "Tab" => 0x2b,
        "Space" => 0x2c,
        "Minus" => 0x2d,
        "Equal" => 0x2e,
        "BracketLeft" => 0x2f,
        "BracketRight" => 0x30,
        "Backslash" | "IntlBackslash" => 0x31,
        "Semicolon" => 0x33,
        "Quote" => 0x34,
        "Backquote" => 0x35,
        "Comma" => 0x36,
        "Period" => 0x37,
        "Slash" => 0x38,
        "CapsLock" => 0x39,
        "PrintScreen" => 0x46,
        "ScrollLock" => 0x47,
        "Pause" => 0x48,
        "Insert" => 0x49,
        "Home" => 0x4a,
        "PageUp" => 0x4b,
        "Delete" => 0x4c,
        "End" => 0x4d,
        "PageDown" => 0x4e,
        "ArrowRight" => 0x4f,
        "ArrowLeft" => 0x50,
        "ArrowDown" => 0x51,
        "ArrowUp" => 0x52,
        "NumLock" => 0x53,
        "NumpadDivide" => 0x54,
        "NumpadMultiply" => 0x55,
        "NumpadSubtract" => 0x56,
        "NumpadAdd" => 0x57,
        "NumpadEnter" => 0x58,
        "Numpad1" => 0x59,
        "Numpad2" => 0x5a,
        "Numpad3" => 0x5b,
        "Numpad4" => 0x5c,
        "Numpad5" => 0x5d,
        "Numpad6" => 0x5e,
        "Numpad7" => 0x5f,
        "Numpad8" => 0x60,
        "Numpad9" => 0x61,
        "Numpad0" => 0x62,
        "NumpadDecimal" => 0x63,
        "ContextMenu" => 0x65,
        _ if code.len() == 4 && code.starts_with("Key") => {
            let letter = code.as_bytes()[3];
            if !letter.is_ascii_uppercase() {
                return None;
            }
            0x04 + letter - b'A'
        }
        _ if code.len() == 6 && code.starts_with("Digit") => {
            let digit = code.as_bytes()[5];
            if !digit.is_ascii_digit() {
                return None;
            }
            [0x27, 0x1e, 0x1f, 0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26][(digit - b'0') as usize]
        }
        _ if let Some(number) = code
            .strip_prefix('F')
            .and_then(|value| value.parse::<u8>().ok())
            && (1..=12).contains(&number) =>
        {
            0x39 + number
        }
        _ => return None,
    };
    Some(KeyboardUsage::Key(usage))
}

fn map_keyboard_usage(key_code: u16) -> Option<KeyboardUsage> {
    let key_code = match key_code.to_le_bytes() {
        [key_code, 0x80] => u16::from(key_code),
        _ => key_code,
    };
    let usage = match key_code {
        0x08 => 0x2a,
        0x09 => 0x2b,
        0x0d => 0x28,
        0x1b => 0x29,
        0x20 => 0x2c,
        0x30..=0x39 => {
            [0x27, 0x1e, 0x1f, 0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26][(key_code - 0x30) as usize]
        }
        0x41..=0x5a => 0x04 + (key_code - 0x41) as u8,
        0x70..=0x7b => 0x3a + (key_code - 0x70) as u8,
        0x25 => 0x50,
        0x26 => 0x52,
        0x27 => 0x4f,
        0x28 => 0x51,
        0x2d => 0x49,
        0x2e => 0x4c,
        0x24 => 0x4a,
        0x23 => 0x4d,
        0xba => 0x33,
        0xbb => 0x2e,
        0xbc => 0x36,
        0xbd => 0x2d,
        0xbe => 0x37,
        0xbf => 0x38,
        0xc0 => 0x35,
        0xdb => 0x2f,
        0xdc => 0x31,
        0xdd => 0x30,
        0xde => 0x34,
        0xa0 => return Some(KeyboardUsage::Modifier(0x02)),
        0xa1 => return Some(KeyboardUsage::Modifier(0x20)),
        0xa2 => return Some(KeyboardUsage::Modifier(0x01)),
        0xa3 => return Some(KeyboardUsage::Modifier(0x10)),
        0xa4 => return Some(KeyboardUsage::Modifier(0x04)),
        0xa5 => return Some(KeyboardUsage::Modifier(0x40)),
        0x5b => return Some(KeyboardUsage::Modifier(0x08)),
        0x5c => return Some(KeyboardUsage::Modifier(0x80)),
        _ => return None,
    };
    Some(KeyboardUsage::Key(usage))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_prefixed_moonlight_key_code() {
        assert!(matches!(
            map_keyboard_usage(0x8041),
            Some(KeyboardUsage::Key(0x04))
        ));
    }

    #[test]
    fn maps_dom_codes_to_usb_usages() {
        assert!(matches!(
            map_dom_code("KeyA"),
            Some(KeyboardUsage::Key(0x04))
        ));
        assert!(matches!(
            map_dom_code("F12"),
            Some(KeyboardUsage::Key(0x45))
        ));
        assert!(matches!(
            map_dom_code("ControlRight"),
            Some(KeyboardUsage::Modifier(0x10))
        ));
    }

    #[test]
    fn maps_en_us_text_characters() {
        assert_eq!(map_text_character('a'), Some((0x04, false)));
        assert_eq!(map_text_character('A'), Some((0x04, true)));
        assert_eq!(map_text_character('?'), Some((0x38, true)));
        assert_eq!(map_text_character('\n'), Some((0x28, false)));
        assert_eq!(map_text_character('é'), None);
    }
}
