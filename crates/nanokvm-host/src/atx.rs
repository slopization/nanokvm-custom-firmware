use std::io;
use std::sync::Arc;
use std::time::Duration;

use gpiocdev::Request;
use gpiocdev::line::Value;
use tokio::sync::Mutex;

const GPIO_CHIP: &str = "/dev/gpiochip0";
const POWER_LINE: u32 = 23;
const POWER_LED_LINE: u32 = 24;
const RESET_LINE: u32 = 25;

#[derive(Clone)]
pub struct AtxController {
    inner: Arc<AtxInner>,
}

struct AtxInner {
    lines: Request,
    operation: Mutex<()>,
}

#[derive(Clone, Copy)]
pub enum AtxButton {
    Power,
    PowerLong,
    Reset,
}

impl AtxController {
    pub fn open() -> io::Result<Self> {
        let lines = Request::builder()
            .on_chip(GPIO_CHIP)
            .with_line(POWER_LED_LINE)
            .as_input()
            .with_lines(&[POWER_LINE, RESET_LINE])
            .as_output(Value::Inactive)
            .with_consumer("nanokvm-atx")
            .request()
            .map_err(gpio_error)?;
        Ok(Self {
            inner: Arc::new(AtxInner {
                lines,
                operation: Mutex::new(()),
            }),
        })
    }

    pub fn power_led(&self) -> io::Result<bool> {
        self.inner
            .lines
            .value(POWER_LED_LINE)
            .map(|value| value == Value::Inactive)
            .map_err(gpio_error)
    }

    pub fn busy(&self) -> bool {
        self.inner.operation.try_lock().is_err()
    }

    pub async fn click(&self, button: AtxButton) -> io::Result<()> {
        let _operation = self.inner.operation.try_lock().map_err(|_| {
            io::Error::new(io::ErrorKind::WouldBlock, "another ATX operation is active")
        })?;
        let (line, duration) = match button {
            AtxButton::Power => (POWER_LINE, Duration::from_millis(800)),
            AtxButton::PowerLong => (POWER_LINE, Duration::from_secs(5)),
            AtxButton::Reset => (RESET_LINE, Duration::from_millis(800)),
        };
        self.inner
            .lines
            .set_value(line, Value::Active)
            .map_err(gpio_error)?;
        let release = LineRelease {
            request: &self.inner.lines,
            line,
        };
        tokio::time::sleep(duration).await;
        release.release()
    }
}

struct LineRelease<'a> {
    request: &'a Request,
    line: u32,
}

impl LineRelease<'_> {
    fn release(self) -> io::Result<()> {
        self.request
            .set_value(self.line, Value::Inactive)
            .map_err(gpio_error)
    }
}

impl Drop for LineRelease<'_> {
    fn drop(&mut self) {
        let _ = self.request.set_value(self.line, Value::Inactive);
    }
}

fn gpio_error(error: gpiocdev::Error) -> io::Error {
    io::Error::other(error)
}
