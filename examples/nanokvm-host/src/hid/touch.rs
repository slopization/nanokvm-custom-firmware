use moonlink_protocol::TouchEvent;

use super::{UsbGadgetInputSink, write_report};
use crate::hid_report::{TouchscreenReport, encode_touchscreen};

const TOUCH_DOWN: u8 = 0x01;
const TOUCH_MOVE: u8 = 0x03;
const TOUCH_CANCEL_ALL: u8 = 0x07;

impl UsbGadgetInputSink {
    pub(super) async fn write_touch(&self, event: TouchEvent) -> std::io::Result<()> {
        let Some(file) = &self.hid else {
            println!("touch event without HID path: {event:?}");
            return Ok(());
        };
        if self.trace_pointer_input {
            println!("pointer input: {event:?}");
        }
        let mut state = self.state.lock().await;
        if matches!(event.event_type, TOUCH_DOWN | TOUCH_MOVE) {
            state.set_pointer_from_unit_float(event.x, event.y);
            state.touch_contact_id = event.pointer_id as u8;
        }
        let active = matches!(event.event_type, TOUCH_DOWN | TOUCH_MOVE);
        let contact_id = if event.event_type == TOUCH_CANCEL_ALL {
            state.touch_contact_id
        } else {
            event.pointer_id as u8
        };
        let report = encode_touchscreen(&TouchscreenReport {
            flags: if active { 0x03 } else { 0 },
            contact_id,
            x: state.pointer_x,
            y: state.pointer_y,
            contact_count: u8::from(active),
        })?;
        drop(state);
        write_report(file, &report).await
    }
}
