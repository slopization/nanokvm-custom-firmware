use moonlink_protocol::MouseEvent;

use super::{PointerMode, UsbGadgetInputSink, clamp_i8, write_report};
use crate::hid_report::{
    AbsolutePointerReport, RelativeMouseReport, encode_absolute_pointer, encode_relative_mouse,
};

impl UsbGadgetInputSink {
    pub(super) async fn write_mouse(&self, event: MouseEvent) -> std::io::Result<()> {
        if self.trace() {
            println!("pointer input: {event:?}");
        }
        match event {
            MouseEvent::Relative { dx, dy } => {
                let Some(file) = self.relative_mouse() else {
                    println!("relative mouse event without HID path: {event:?}");
                    return Ok(());
                };
                let report;
                let release = {
                    let mut state = self.state.lock().await;
                    let release = if state.pointer_mode == PointerMode::Absolute {
                        let buttons = state.take_buttons(PointerMode::Absolute);
                        if buttons != 0 {
                            Some((
                                self.endpoint(PointerMode::Absolute).cloned(),
                                encode_absolute_pointer(&AbsolutePointerReport {
                                    buttons: 0,
                                    x: state.pointer_x,
                                    y: state.pointer_y,
                                    wheel: 0,
                                })?
                                .to_vec(),
                            ))
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    state.pointer_mode = PointerMode::Relative;
                    report = encode_relative_mouse(&RelativeMouseReport {
                        buttons: state.buttons(PointerMode::Relative),
                        x: clamp_i8(dx),
                        y: clamp_i8(dy),
                        wheel: 0,
                    })?;
                    release
                };
                write_release(release).await?;
                write_report(file, &report).await
            }
            MouseEvent::Button { pressed, button } => {
                let mode;
                let mask = map_mouse_button(button);
                let report = {
                    let mut state = self.state.lock().await;
                    mode = state.pointer_mode;
                    state.set_button(mode, mask, pressed);
                    match mode {
                        PointerMode::Relative => encode_relative_mouse(&RelativeMouseReport {
                            buttons: state.buttons(PointerMode::Relative),
                            x: 0,
                            y: 0,
                            wheel: 0,
                        })?
                        .to_vec(),
                        PointerMode::Absolute => encode_absolute_pointer(&AbsolutePointerReport {
                            buttons: state.buttons(PointerMode::Absolute),
                            x: state.pointer_x,
                            y: state.pointer_y,
                            wheel: 0,
                        })?
                        .to_vec(),
                    }
                };
                let Some(file) = self.endpoint(mode) else {
                    println!("mouse button event without {mode:?} HID path: {event:?}");
                    return Ok(());
                };
                write_report(file, &report).await
            }
            MouseEvent::Absolute {
                x,
                y,
                width,
                height,
            } => {
                let Some(file) = self.absolute_mouse() else {
                    println!("absolute mouse event without HID path: {event:?}");
                    return Ok(());
                };
                let report;
                let release = {
                    let mut state = self.state.lock().await;
                    let release = if state.pointer_mode == PointerMode::Relative {
                        let buttons = state.take_buttons(PointerMode::Relative);
                        if buttons != 0 {
                            Some((
                                self.endpoint(PointerMode::Relative).cloned(),
                                encode_relative_mouse(&RelativeMouseReport {
                                    buttons: 0,
                                    x: 0,
                                    y: 0,
                                    wheel: 0,
                                })?
                                .to_vec(),
                            ))
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    state.set_pointer_from_absolute(x, y, width, height);
                    state.pointer_mode = PointerMode::Absolute;
                    report = encode_absolute_pointer(&AbsolutePointerReport {
                        buttons: state.buttons(PointerMode::Absolute),
                        x: state.pointer_x,
                        y: state.pointer_y,
                        wheel: 0,
                    })?;
                    release
                };
                write_release(release).await?;
                write_report(file, &report).await
            }
        }
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
        "back" | "up" => 4,
        "forward" | "down" => 5,
        _ => return Ok(()),
    };
    sink.write_mouse(MouseEvent::Button { pressed, button })
        .await
}

pub(super) async fn write_web_absolute(
    sink: &UsbGadgetInputSink,
    x: i16,
    y: i16,
) -> std::io::Result<()> {
    let Some(file) = sink.absolute_mouse() else {
        return Ok(());
    };
    let report;
    let release = {
        let mut state = sink.state.lock().await;
        let release = if state.pointer_mode == PointerMode::Relative {
            let buttons = state.take_buttons(PointerMode::Relative);
            if buttons != 0 {
                Some((
                    sink.endpoint(PointerMode::Relative).cloned(),
                    encode_relative_mouse(&RelativeMouseReport {
                        buttons: 0,
                        x: 0,
                        y: 0,
                        wheel: 0,
                    })?
                    .to_vec(),
                ))
            } else {
                None
            }
        } else {
            None
        };
        state.set_pointer_from_web(x, y);
        state.pointer_mode = PointerMode::Absolute;
        report = encode_absolute_pointer(&AbsolutePointerReport {
            buttons: state.buttons(PointerMode::Absolute),
            x: state.pointer_x,
            y: state.pointer_y,
            wheel: 0,
        })?;
        release
    };
    write_release(release).await?;
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

fn map_mouse_button(button: u8) -> u8 {
    match button {
        1 => 0x01,
        2 => 0x04,
        3 => 0x02,
        4..=8 => 1 << (button - 1),
        _ => 0,
    }
}

async fn write_release(
    release: Option<(Option<super::HidEndpoint>, Vec<u8>)>,
) -> std::io::Result<()> {
    let Some((Some(file), report)) = release else {
        return Ok(());
    };
    write_report(&file, &report).await
}
