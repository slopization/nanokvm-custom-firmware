use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use moonlink_core::InputContext;
use moonlink_protocol::{InputEvent, InputEventKind};
use tokio::io::unix::AsyncFd;
use tokio::sync::Mutex;

use crate::hid_report::POINTER_AXIS_MAX;

mod keyboard;
mod mouse;
mod scroll;
mod touch;

const DEFAULT_POINTER_WIDTH: i32 = 1920;
const DEFAULT_POINTER_HEIGHT: i32 = 1080;

#[derive(Clone, Default)]
pub struct UsbGadgetInputSink {
    hid: Option<Arc<Mutex<AsyncFd<File>>>>,
    state: Arc<Mutex<InputState>>,
    trace_pointer_input: bool,
}

struct InputState {
    keyboard: KeyboardState,
    mouse_buttons: u8,
    pointer_x: u16,
    pointer_y: u16,
    pointer_width: i32,
    pointer_height: i32,
    pointer_mode: PointerMode,
    touch_contact_id: u8,
}

#[derive(Default)]
struct KeyboardState {
    modifiers: u8,
    keys: Vec<u8>,
}

#[derive(Clone, Copy, Default)]
enum PointerMode {
    #[default]
    Relative,
    Absolute,
}

impl UsbGadgetInputSink {
    pub async fn open(hid: Option<PathBuf>, trace_pointer_input: bool) -> std::io::Result<Self> {
        Ok(Self {
            hid: open_optional_file(hid).await?,
            state: Arc::default(),
            trace_pointer_input,
        })
    }

    async fn write_input(&self, kind: InputEventKind) -> std::io::Result<()> {
        match kind {
            InputEventKind::Keyboard(event) => self.write_keyboard(event).await,
            InputEventKind::Mouse(event) => self.write_mouse(event).await,
            InputEventKind::Scroll(event) => self.write_scroll(event).await,
            InputEventKind::Touch(event) => self.write_touch(event).await,
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

    pub async fn write_dom_key(&self, code: &str, pressed: bool) -> std::io::Result<()> {
        self.write_keyboard_usage(keyboard::map_dom_code(code), pressed)
            .await
    }

    pub async fn write_web_button(&self, button: &str, pressed: bool) -> std::io::Result<()> {
        mouse::write_web_button(self, button, pressed).await
    }

    pub async fn write_web_absolute(&self, x: u16, y: u16) -> std::io::Result<()> {
        mouse::write_web_absolute(self, x, y).await
    }

    pub async fn write_web_relative(&self, dx: i8, dy: i8) -> std::io::Result<()> {
        mouse::write_web_relative(self, dx, dy).await
    }

    pub async fn write_web_wheel(&self, dy: i8) -> std::io::Result<()> {
        scroll::write_web_wheel(self, dy).await
    }

    pub async fn release_all(&self) -> std::io::Result<()> {
        let Some(file) = &self.hid else {
            return Ok(());
        };
        let mut state = self.state.lock().await;
        state.keyboard = KeyboardState::default();
        state.mouse_buttons = 0;
        let keyboard = crate::hid_report::encode_keyboard(&crate::hid_report::KeyboardReport {
            modifiers: 0,
            reserved: 0,
            keys: [0; 6],
        })?;
        let mouse =
            crate::hid_report::encode_relative_mouse(&crate::hid_report::RelativeMouseReport {
                buttons: 0,
                x: 0,
                y: 0,
                wheel: 0,
            })?;
        drop(state);
        write_report(file, &keyboard).await?;
        write_report(file, &mouse).await
    }
}

impl Default for InputState {
    fn default() -> Self {
        Self {
            keyboard: KeyboardState::default(),
            mouse_buttons: 0,
            pointer_x: POINTER_AXIS_MAX / 2,
            pointer_y: POINTER_AXIS_MAX / 2,
            pointer_width: DEFAULT_POINTER_WIDTH,
            pointer_height: DEFAULT_POINTER_HEIGHT,
            pointer_mode: PointerMode::default(),
            touch_contact_id: 0,
        }
    }
}

impl InputState {
    fn set_pointer_from_absolute(&mut self, x: i16, y: i16, width: i16, height: i16) {
        self.pointer_width = i32::from(width).max(1);
        self.pointer_height = i32::from(height).max(1);
        self.pointer_x = scale_axis(i32::from(x), self.pointer_width);
        self.pointer_y = scale_axis(i32::from(y), self.pointer_height);
    }

    fn set_pointer_from_unit_float(&mut self, x: f32, y: f32) {
        self.pointer_x = (x.clamp(0.0, 1.0) * f32::from(POINTER_AXIS_MAX)).round() as u16;
        self.pointer_y = (y.clamp(0.0, 1.0) * f32::from(POINTER_AXIS_MAX)).round() as u16;
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

async fn open_optional_file(
    path: Option<PathBuf>,
) -> std::io::Result<Option<Arc<Mutex<AsyncFd<File>>>>> {
    let Some(path) = path else {
        return Ok(None);
    };
    let file = OpenOptions::new().write(true).open(path)?;
    set_nonblocking(&file)?;
    Ok(Some(Arc::new(Mutex::new(AsyncFd::new(file)?))))
}

async fn write_report(file: &Arc<Mutex<AsyncFd<File>>>, report: &[u8]) -> std::io::Result<()> {
    let file = file.lock().await;
    loop {
        let mut ready = file.writable().await?;
        match ready.try_io(|inner| write_report_now(inner.get_ref(), report)) {
            Ok(result) => return result,
            Err(_would_block) => continue,
        }
    }
}

fn set_nonblocking(file: &File) -> std::io::Result<()> {
    let fd = file.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

fn write_report_now(file: &File, report: &[u8]) -> std::io::Result<()> {
    let written = unsafe {
        libc::write(
            file.as_raw_fd(),
            report.as_ptr().cast::<libc::c_void>(),
            report.len(),
        )
    };
    if written < 0 {
        return Err(std::io::Error::last_os_error());
    }
    if written as usize != report.len() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::WriteZero,
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
