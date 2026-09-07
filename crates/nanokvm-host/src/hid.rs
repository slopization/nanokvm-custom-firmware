use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use moonlink_core::InputContext;
use moonlink_protocol::{InputEvent, InputEventKind};
use tokio::io::unix::AsyncFd;
use tokio::sync::Mutex;

use crate::hid_report::POINTER_AXIS_MAX;

mod keyboard;
mod mouse;
mod scroll;

pub(super) type HidEndpoint = Arc<Mutex<AsyncFd<File>>>;

const DEFAULT_POINTER_WIDTH: i32 = 1920;
const DEFAULT_POINTER_HEIGHT: i32 = 1080;
const HID_WRITE_TIMEOUT: Duration = Duration::from_millis(100);

#[derive(Clone, Default)]
pub struct InputDevicePaths {
    pub keyboard: Option<PathBuf>,
    pub relative_mouse: Option<PathBuf>,
    pub absolute_mouse: Option<PathBuf>,
}

#[derive(Clone, Default)]
pub struct UsbGadgetInputSink {
    keyboard: Option<HidEndpoint>,
    relative_mouse: Option<HidEndpoint>,
    absolute_mouse: Option<HidEndpoint>,
    state: Arc<Mutex<InputState>>,
    trace_pointer_input: bool,
}

struct InputState {
    keyboard: KeyboardState,
    relative_buttons: u8,
    absolute_buttons: u8,
    pointer_x: u16,
    pointer_y: u16,
    pointer_width: i32,
    pointer_height: i32,
    pointer_mode: PointerMode,
}

#[derive(Default)]
struct KeyboardState {
    modifiers: u8,
    keys: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum PointerMode {
    #[default]
    Relative,
    Absolute,
}

impl UsbGadgetInputSink {
    pub async fn open(paths: InputDevicePaths, trace_pointer_input: bool) -> io::Result<Self> {
        Ok(Self {
            keyboard: open_optional_file(paths.keyboard).await?,
            relative_mouse: open_optional_file(paths.relative_mouse).await?,
            absolute_mouse: open_optional_file(paths.absolute_mouse).await?,
            state: Arc::default(),
            trace_pointer_input,
        })
    }

    async fn write_input(&self, kind: InputEventKind) -> io::Result<()> {
        match kind {
            InputEventKind::Keyboard(event) => self.write_keyboard(event).await,
            InputEventKind::Mouse(event) => self.write_mouse(event).await,
            InputEventKind::Scroll(event) => self.write_scroll(event).await,
            InputEventKind::Touch(event) => {
                println!("touch input is not mapped by the three-interface HID gadget: {event:?}");
                Ok(())
            }
            InputEventKind::Text(text) => {
                println!("text input is not mapped to HID reports yet: {text:?}");
                Ok(())
            }
            other => {
                println!("unhandled input event: {other:?}");
                Ok(())
            }
        }
    }

    pub async fn write_dom_key(&self, code: &str, pressed: bool) -> io::Result<()> {
        self.write_keyboard_usage(keyboard::map_dom_code(code), pressed)
            .await
    }

    pub async fn write_web_button(&self, button: &str, pressed: bool) -> io::Result<()> {
        mouse::write_web_button(self, button, pressed).await
    }

    pub async fn write_web_absolute(&self, x: i16, y: i16) -> io::Result<()> {
        mouse::write_web_absolute(self, x, y).await
    }

    pub async fn write_web_relative(&self, dx: i8, dy: i8) -> io::Result<()> {
        mouse::write_web_relative(self, dx, dy).await
    }

    pub async fn write_web_wheel(&self, dy: i8) -> io::Result<()> {
        scroll::write_wheel(self, dy).await
    }

    pub async fn release_all(&self) -> io::Result<()> {
        let mut state = self.state.lock().await;
        state.keyboard = KeyboardState::default();
        state.relative_buttons = 0;
        state.absolute_buttons = 0;
        let keyboard = crate::hid_report::encode_keyboard(&crate::hid_report::KeyboardReport {
            modifiers: 0,
            reserved: 0,
            keys: [0; 6],
        })?;
        let relative =
            crate::hid_report::encode_relative_mouse(&crate::hid_report::RelativeMouseReport {
                buttons: 0,
                x: 0,
                y: 0,
                wheel: 0,
            })?;
        let absolute = crate::hid_report::encode_absolute_pointer(
            &crate::hid_report::AbsolutePointerReport {
                buttons: 0,
                x: state.pointer_x,
                y: state.pointer_y,
                wheel: 0,
            },
        )?;
        drop(state);

        let mut result = Ok(());
        if let Some(file) = &self.keyboard {
            result = result.and(write_report(file, &keyboard).await);
        }
        if let Some(file) = &self.relative_mouse {
            result = result.and(write_report(file, &relative).await);
        }
        if let Some(file) = &self.absolute_mouse {
            result = result.and(write_report(file, &absolute).await);
        }
        result
    }

    pub(super) fn endpoint(&self, mode: PointerMode) -> Option<&HidEndpoint> {
        match mode {
            PointerMode::Relative => self.relative_mouse.as_ref(),
            PointerMode::Absolute => self.absolute_mouse.as_ref(),
        }
    }

    pub(super) fn keyboard(&self) -> Option<&HidEndpoint> {
        self.keyboard.as_ref()
    }

    pub(super) fn relative_mouse(&self) -> Option<&HidEndpoint> {
        self.relative_mouse.as_ref()
    }

    pub(super) fn absolute_mouse(&self) -> Option<&HidEndpoint> {
        self.absolute_mouse.as_ref()
    }

    pub(super) fn trace(&self) -> bool {
        self.trace_pointer_input
    }
}

impl Default for InputState {
    fn default() -> Self {
        Self {
            keyboard: KeyboardState::default(),
            relative_buttons: 0,
            absolute_buttons: 0,
            pointer_x: POINTER_AXIS_MAX / 2,
            pointer_y: POINTER_AXIS_MAX / 2,
            pointer_width: DEFAULT_POINTER_WIDTH,
            pointer_height: DEFAULT_POINTER_HEIGHT,
            pointer_mode: PointerMode::default(),
        }
    }
}

impl InputState {
    pub(super) fn set_pointer_from_absolute(&mut self, x: i16, y: i16, width: i16, height: i16) {
        self.pointer_width = i32::from(width).max(1);
        self.pointer_height = i32::from(height).max(1);
        self.pointer_x = scale_axis(i32::from(x), self.pointer_width);
        self.pointer_y = scale_axis(i32::from(y), self.pointer_height);
    }

    pub(super) fn set_pointer_from_web(&mut self, x: i16, y: i16) {
        self.pointer_x = web_axis_to_hid(x);
        self.pointer_y = web_axis_to_hid(y);
    }

    pub(super) fn buttons(&self, mode: PointerMode) -> u8 {
        match mode {
            PointerMode::Relative => self.relative_buttons,
            PointerMode::Absolute => self.absolute_buttons,
        }
    }

    pub(super) fn take_buttons(&mut self, mode: PointerMode) -> u8 {
        let buttons = self.buttons(mode);
        match mode {
            PointerMode::Relative => self.relative_buttons = 0,
            PointerMode::Absolute => self.absolute_buttons = 0,
        }
        buttons
    }

    pub(super) fn set_button(&mut self, mode: PointerMode, mask: u8, pressed: bool) {
        match mode {
            PointerMode::Relative => {
                if pressed {
                    self.relative_buttons |= mask;
                } else {
                    self.relative_buttons &= !mask;
                }
            }
            PointerMode::Absolute => {
                if pressed {
                    self.absolute_buttons |= mask;
                } else {
                    self.absolute_buttons &= !mask;
                }
            }
        }
    }
}

#[async_trait]
impl moonlink_core::InputSink for UsbGadgetInputSink {
    async fn on_input(&self, context: InputContext, event: InputEvent) {
        if let Err(err) = self.write_input(event.kind).await {
            eprintln!("input from {} dropped: {err}", context.client_id);
        }
    }
}

async fn open_optional_file(path: Option<PathBuf>) -> io::Result<Option<HidEndpoint>> {
    let Some(path) = path else {
        return Ok(None);
    };
    let file = OpenOptions::new().write(true).open(path)?;
    set_nonblocking(&file)?;
    Ok(Some(Arc::new(Mutex::new(AsyncFd::new(file)?))))
}

async fn write_report(file: &HidEndpoint, report: &[u8]) -> io::Result<()> {
    let file = file.lock().await;
    loop {
        // A target can stop polling the gadget for an arbitrarily long time.
        // Bound each attempt so one stalled HID endpoint cannot wedge later sessions.
        let mut ready = tokio::time::timeout(HID_WRITE_TIMEOUT, file.writable())
            .await
            .map_err(|_| {
                io::Error::new(io::ErrorKind::TimedOut, "HID endpoint write timed out")
            })??;
        match ready.try_io(|inner| write_report_now(inner.get_ref(), report)) {
            Ok(result) => return result,
            Err(_would_block) => continue,
        }
    }
}

fn set_nonblocking(file: &File) -> io::Result<()> {
    let fd = file.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn write_report_now(file: &File, report: &[u8]) -> io::Result<()> {
    let written = unsafe {
        libc::write(
            file.as_raw_fd(),
            report.as_ptr().cast::<libc::c_void>(),
            report.len(),
        )
    };
    if written < 0 {
        return Err(io::Error::last_os_error());
    }
    if written as usize != report.len() {
        return Err(io::Error::new(
            io::ErrorKind::WriteZero,
            "short HID report write",
        ));
    }
    Ok(())
}

fn clamp_i8(value: i16) -> i8 {
    value.clamp(i8::MIN as i16, i8::MAX as i16) as i8
}

fn clamp_axis(value: i32) -> u16 {
    value.clamp(0, i32::from(POINTER_AXIS_MAX)) as u16
}

fn scale_axis(value: i32, max: i32) -> u16 {
    clamp_axis(value.clamp(0, max) * i32::from(POINTER_AXIS_MAX) / max.max(1))
}

fn web_axis_to_hid(value: i16) -> u16 {
    let unsigned = i64::from(value) - i64::from(i16::MIN);
    ((unsigned * i64::from(POINTER_AXIS_MAX) + i64::from(u16::MAX) / 2) / i64::from(u16::MAX))
        as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_web_axis_to_hid_range() {
        assert_eq!(web_axis_to_hid(i16::MIN), 0);
        assert_eq!(web_axis_to_hid(0), 16_384);
        assert_eq!(web_axis_to_hid(i16::MAX), POINTER_AXIS_MAX);
    }
}
