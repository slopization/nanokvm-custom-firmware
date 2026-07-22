use std::net::SocketAddr;

use moonlink_core::ServerError;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::ServerController;
use crate::error::TokioServerError;

pub struct ServerHandle {
    pub(crate) http_addr: SocketAddr,
    pub(crate) https_addr: SocketAddr,
    pub(crate) discovery_addr: SocketAddr,
    pub(crate) rtsp_addr: SocketAddr,
    pub(crate) shutdown_tx: watch::Sender<bool>,
    pub(crate) tasks: Vec<JoinHandle<Result<(), ServerError>>>,
    pub(crate) controller: ServerController,
}

impl ServerHandle {
    pub fn http_addr(&self) -> SocketAddr {
        self.http_addr
    }

    pub fn discovery_addr(&self) -> SocketAddr {
        self.discovery_addr
    }

    pub fn https_addr(&self) -> SocketAddr {
        self.https_addr
    }

    pub fn rtsp_addr(&self) -> SocketAddr {
        self.rtsp_addr
    }

    pub fn controller(&self) -> ServerController {
        self.controller.clone()
    }

    pub async fn shutdown(mut self) -> Result<(), TokioServerError> {
        let _ = self.shutdown_tx.send(true);
        for task in self.tasks.drain(..) {
            task.await??;
        }
        Ok(())
    }
}
