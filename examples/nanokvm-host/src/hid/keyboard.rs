use moonlink_protocol::KeyboardEvent;

use super::{UsbGadgetInputSink, write_report};
use crate::hid_report::{KeyboardReport, encode_keyboard};

#[derive(Debug, Clone, Copy)]
enum KeyboardUsage {
    Key(u8),
    Modifier(u8),
}

impl UsbGadgetInputSink {
    pub(super) async fn write_keyboard(&self, event: KeyboardEvent) -> std::io::Result<()> {
        let Some(file) = &self.hid else {
            println!("keyboard event without HID path: {event:?}");
            return Ok(());
        };

        let mut state = self.state.lock().await;
        let Some(usage) = map_keyboard_usage(event.key_code) else {
            println!("unmapped key code: {}", event.key_code);
            return Ok(());
        };

        match usage {
            KeyboardUsage::Modifier(bit) => {
                if event.pressed {
                    state.keyboard.modifiers |= bit;
                } else {
                    state.keyboard.modifiers &= !bit;
                }
            }
            KeyboardUsage::Key(code) => {
                if event.pressed {
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
}
