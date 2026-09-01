use moonlink_protocol::MouseEvent;

use super::{PointerMode, UsbGadgetInputSink, clamp_i8, write_report};
use crate::hid_report::{
    AbsolutePointerReport, RelativeMouseReport, encode_absolute_pointer, encode_relative_mouse,
};

impl UsbGadgetInputSink {
    pub(super) async fn write_mouse(&self, event: MouseEvent) -> std::io::Result<()> {
        let Some(file) = &self.hid else {
            println!("mouse event without HID path: {event:?}");
            return Ok(());
        };

        if self.trace_pointer_input {
            println!("pointer input: {event:?}");
        }
        match event {
            MouseEvent::Relative { dx, dy } => {
                let mut state = self.state.lock().await;
                state.pointer_mode = PointerMode::Relative;
                let report = encode_relative_mouse(&RelativeMouseReport {
                    buttons: state.mouse_buttons,
                    x: clamp_i8(dx),
                    y: clamp_i8(dy),
                    wheel: 0,
                })?;
                drop(state);
                write_report(file, &report).await
            }
            MouseEvent::Button { pressed, button } => {
                let mut state = self.state.lock().await;
                let mask = map_mouse_button(button);
                if pressed {
                    state.mouse_buttons |= mask;
                } else {
                    state.mouse_buttons &= !mask;
                }
                let report = match state.pointer_mode {
                    PointerMode::Relative => encode_relative_mouse(&RelativeMouseReport {
                        buttons: state.mouse_buttons,
                        x: 0,
                        y: 0,
                        wheel: 0,
                    })?,
                    PointerMode::Absolute => encode_absolute_pointer(&AbsolutePointerReport {
                        buttons: state.mouse_buttons,
                        x: state.pointer_x,
                        y: state.pointer_y,
                    })?,
                };
                drop(state);
                write_report(file, &report).await
            }
            MouseEvent::Absolute {
                x,
                y,
                width,
                height,
            } => {
                let mut state = self.state.lock().await;
                state.set_pointer_from_absolute(x, y, width, height);
                state.pointer_mode = PointerMode::Absolute;
                let report = encode_absolute_pointer(&AbsolutePointerReport {
                    buttons: state.mouse_buttons,
                    x: state.pointer_x,
                    y: state.pointer_y,
                })?;
                drop(state);
                write_report(file, &report).await
            }
        }
    }
}

fn map_mouse_button(button: u8) -> u8 {
    match button {
        1 => 0x01,
        2 => 0x04,
        3 => 0x02,
        4..=8 => 1 << (button - 1),
        _ => 0,
    }
}

pub(super) async fn write_web_button(
    sink: &UsbGadgetInputSink,
    button: &str,
    pressed: bool,
) -> std::io::Result<()> {
    let button = match button {
        "left" => 1,
        "middle" => 2,
        "right" => 3,
        "back" => 4,
        "forward" => 5,
        _ => return Ok(()),
    };
    sink.write_mouse(MouseEvent::Button { pressed, button })
        .await
}

pub(super) async fn write_web_absolute(
    sink: &UsbGadgetInputSink,
    x: u16,
    y: u16,
) -> std::io::Result<()> {
    let Some(file) = &sink.hid else {
        return Ok(());
    };
    let mut state = sink.state.lock().await;
    state.pointer_x = (u32::from(x) * u32::from(crate::hid_report::POINTER_AXIS_MAX)
        / u32::from(u16::MAX)) as u16;
    state.pointer_y = (u32::from(y) * u32::from(crate::hid_report::POINTER_AXIS_MAX)
        / u32::from(u16::MAX)) as u16;
    state.pointer_mode = PointerMode::Absolute;
    let report = encode_absolute_pointer(&AbsolutePointerReport {
        buttons: state.mouse_buttons,
        x: state.pointer_x,
        y: state.pointer_y,
    })?;
    drop(state);
    write_report(file, &report).await
}

pub(super) async fn write_web_relative(
    sink: &UsbGadgetInputSink,
    dx: i8,
    dy: i8,
) -> std::io::Result<()> {
    sink.write_mouse(MouseEvent::Relative {
        dx: i16::from(dx),
        dy: i16::from(dy),
    })
    .await
}
