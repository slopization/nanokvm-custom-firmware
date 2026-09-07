use moonlink_protocol::ScrollEvent;

use super::{PointerMode, UsbGadgetInputSink, clamp_i8, write_report};
use crate::hid_report::{
    AbsolutePointerReport, RelativeMouseReport, encode_absolute_pointer, encode_relative_mouse,
};

impl UsbGadgetInputSink {
    pub(super) async fn write_scroll(&self, event: ScrollEvent) -> std::io::Result<()> {
        match event {
            ScrollEvent::Vertical { amount_1, .. } => {
                write_wheel(self, scroll_units(amount_1)).await
            }
            ScrollEvent::Horizontal { amount } => {
                println!("horizontal scroll is not mapped by the HID mouse report: {amount}");
                Ok(())
            }
        }
    }
}

pub(super) async fn write_wheel(sink: &UsbGadgetInputSink, dy: i8) -> std::io::Result<()> {
    let mode;
    let report = {
        let state = sink.state.lock().await;
        mode = state.pointer_mode;
        match mode {
            PointerMode::Relative => encode_relative_mouse(&RelativeMouseReport {
                buttons: state.buttons(PointerMode::Relative),
                x: 0,
                y: 0,
                wheel: dy,
            })?
            .to_vec(),
            PointerMode::Absolute => encode_absolute_pointer(&AbsolutePointerReport {
                buttons: state.buttons(PointerMode::Absolute),
                x: state.pointer_x,
                y: state.pointer_y,
                wheel: dy,
            })?
            .to_vec(),
        }
    };
    let Some(file) = sink.endpoint(mode) else {
        println!("wheel event without {mode:?} HID path");
        return Ok(());
    };
    write_report(file, &report).await
}

fn scroll_units(amount: i16) -> i8 {
    let scaled = amount / 120;
    if scaled != 0 {
        clamp_i8(scaled)
    } else {
        amount.signum() as i8
    }
}
