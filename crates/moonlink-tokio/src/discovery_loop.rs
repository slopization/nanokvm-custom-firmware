use moonlink_core::{ServerError, ServerRuntimeConfig};
use tokio::net::UdpSocket;
use tokio::sync::watch;

pub(crate) async fn serve_discovery(
    socket: UdpSocket,
    config: watch::Receiver<ServerRuntimeConfig>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), ServerError> {
    let mut buffer = [0_u8; 2048];
    loop {
        tokio::select! {
            result = socket.recv_from(&mut buffer) => {
                let (_, peer) = result?;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\n{}",
                    config.borrow().name
                );
                socket.send_to(response.as_bytes(), peer).await?;
            }
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    return Ok(());
                }
            }
        }
    }
}
