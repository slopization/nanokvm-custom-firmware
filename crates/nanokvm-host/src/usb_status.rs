use std::fs;
use std::io;
use std::net::{Ipv4Addr, UdpSocket};
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::sync::watch;

use crate::auth::AuthState;
use crate::display::DisplayStatus;

pub fn primary_ipv4() -> io::Result<Ipv4Addr> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.connect("1.1.1.1:80")?;
    match socket.local_addr()?.ip() {
        std::net::IpAddr::V4(address) => Ok(address),
        std::net::IpAddr::V6(_) => Err(io::Error::other("default route has no IPv4 address")),
    }
}

pub fn udc_state_path(udc: Option<&str>) -> io::Result<PathBuf> {
    if let Some(udc) = udc {
        return Ok(Path::new("/sys/class/udc").join(udc).join("state"));
    }
    let entry = fs::read_dir("/sys/class/udc")?
        .next()
        .transpose()?
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no USB device controller"))?;
    Ok(entry.path().join("state"))
}

pub async fn monitor_usb_attachment(
    state_path: PathBuf,
    ip_address: String,
    auth: AuthState,
    display: watch::Sender<Option<DisplayStatus>>,
) -> io::Result<()> {
    let mut configured = false;
    let mut clear_at = None;
    let mut interval = tokio::time::interval(Duration::from_millis(500));
    loop {
        interval.tick().await;
        let state = tokio::fs::read_to_string(&state_path).await?;
        let now_configured = state.trim() == "configured";
        if now_configured && !configured {
            let otp = auth.issue_otp().await;
            println!("USB attached; recovery code available on OLED for five minutes");
            display.send_replace(Some(DisplayStatus {
                otp,
                ip_address: ip_address.clone(),
            }));
            clear_at = Some(tokio::time::Instant::now() + Duration::from_secs(5 * 60));
        } else if !now_configured && configured {
            display.send_replace(None);
            clear_at = None;
        }
        configured = now_configured;
        if clear_at.is_some_and(|deadline| deadline <= tokio::time::Instant::now()) {
            auth.clear_expired_otp().await;
            display.send_replace(None);
            clear_at = None;
        }
    }
}
