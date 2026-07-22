use std::sync::Arc;

use moonlink_core::ServerRuntimeConfig;
use tokio::sync::{Mutex, broadcast, mpsc, watch};

use crate::shared::{StreamPeerTracker, clear_stream_peer};

#[derive(Debug)]
pub(crate) enum ControlCommand {
    DisconnectClients,
}

#[derive(Clone)]
pub struct ServerController {
    runtime_config: watch::Sender<ServerRuntimeConfig>,
    stream_stop: broadcast::Sender<()>,
    control_commands: mpsc::Sender<ControlCommand>,
    video_peer: Arc<Mutex<StreamPeerTracker>>,
    audio_peer: Arc<Mutex<StreamPeerTracker>>,
}

impl ServerController {
    pub(crate) fn new(
        runtime_config: watch::Sender<ServerRuntimeConfig>,
        stream_stop: broadcast::Sender<()>,
        control_commands: mpsc::Sender<ControlCommand>,
        video_peer: Arc<Mutex<StreamPeerTracker>>,
        audio_peer: Arc<Mutex<StreamPeerTracker>>,
    ) -> Self {
        Self {
            runtime_config,
            stream_stop,
            control_commands,
            video_peer,
            audio_peer,
        }
    }

    pub fn apply_runtime_config(&self, config: ServerRuntimeConfig) {
        self.runtime_config.send_replace(config);
    }

    pub async fn disconnect_clients(&self) {
        let _ = self.stream_stop.send(());
        clear_stream_peer(&self.video_peer).await;
        clear_stream_peer(&self.audio_peer).await;
        let _ = self
            .control_commands
            .send(ControlCommand::DisconnectClients)
            .await;
    }
}
