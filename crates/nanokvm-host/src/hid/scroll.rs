use moonlink_protocol::ScrollEvent;

use super::{UsbGadgetInputSink, clamp_i8, write_report};
use crate::hid_report::{RelativeMouseReport, encode_relative_mouse};

impl UsbGadgetInputSink {
    pub(super) async fn write_scroll(&self, event: ScrollEvent) -> std::io::Result<()> {
        let Some(file) = &self.hid else {
            println!("scroll event without HID path: {event:?}");
            return Ok(());
        };
        let buttons = self.state.lock().await.mouse_buttons;
        match event {
            ScrollEvent::Vertical { amount_1, .. } => {
                let report = encode_relative_mouse(&RelativeMouseReport {
                    buttons,
                    x: 0,
                    y: 0,
                    wheel: scroll_units(amount_1),
                })?;
                write_report(file, &report).await
            }
            ScrollEvent::Horizontal { amount } => {
                println!("horizontal scroll is not mapped to the boot mouse report: {amount}");
                Ok(())
            }
        }
    }
}

fn scroll_units(amount: i16) -> i8 {
    let scaled = amount / 120;
    if scaled != 0 {
        clamp_i8(scaled)
    } else {
        amount.signum() as i8
    }
}

pub(super) async fn write_web_wheel(sink: &UsbGadgetInputSink, dy: i8) -> std::io::Result<()> {
    let Some(file) = &sink.hid else {
        return Ok(());
    };
    let buttons = sink.state.lock().await.mouse_buttons;
    let report = encode_relative_mouse(&RelativeMouseReport {
        buttons,
        x: 0,
        y: 0,
        wheel: dy,
    })?;
    write_report(file, &report).await
}
