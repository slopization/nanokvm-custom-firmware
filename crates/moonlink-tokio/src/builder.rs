use std::collections::HashMap;
use std::sync::Arc;

use moonlink_core::{
    InputSink, MediaProvider, PairingStore, ServerConfig, ServerError, ServerRuntimeConfig,
};
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::{Mutex, broadcast, mpsc, watch};
use tokio_enet::{Host as EnetHost, HostConfig as EnetHostConfig};

use crate::control_loop::serve_control;
use crate::discovery_loop::serve_discovery;
use crate::error::TokioServerError;
use crate::handle::ServerHandle;
use crate::http_loop::serve_http;
use crate::https_loop::serve_https;
use crate::pairing::ensure_server_identity;
use crate::peer_loop::learn_stream_peer;
use crate::rtsp_loop::serve_rtsp;
use crate::server_controller::ServerController;
use crate::shared::{ControlCrypto, StreamPeerPolicy, StreamPeerTracker, StreamPorts};
use crate::tls::create_tls_acceptor;

pub struct ServerBuilder<P, I, S> {
    config: ServerConfig,
    media_provider: Option<P>,
    input_sink: Option<I>,
    pairing_store: Option<S>,
}

impl ServerBuilder<(), (), ()> {
    pub fn new(config: ServerConfig) -> Self {
        Self {
            config,
            media_provider: None,
            input_sink: None,
            pairing_store: None,
        }
    }
}

impl<P, I, S> ServerBuilder<P, I, S> {
    pub fn media_provider<NP>(self, provider: NP) -> ServerBuilder<NP, I, S> {
        ServerBuilder {
            config: self.config,
            media_provider: Some(provider),
            input_sink: self.input_sink,
            pairing_store: self.pairing_store,
        }
    }

    pub fn input_sink<NI>(self, sink: NI) -> ServerBuilder<P, NI, S> {
        ServerBuilder {
            config: self.config,
            media_provider: self.media_provider,
            input_sink: Some(sink),
            pairing_store: self.pairing_store,
        }
    }

    pub fn pairing_store<NS>(self, store: NS) -> ServerBuilder<P, I, NS> {
        ServerBuilder {
            config: self.config,
            media_provider: self.media_provider,
            input_sink: self.input_sink,
            pairing_store: Some(store),
        }
    }
}

impl<P, I, S> ServerBuilder<P, I, S>
where
    P: MediaProvider,
    I: InputSink,
    S: PairingStore,
{
    pub async fn serve(self) -> Result<ServerHandle, TokioServerError> {
        let media_provider = Arc::new(
            self.media_provider
                .ok_or(TokioServerError::MissingMediaProvider)?,
        );
        let input_sink = Arc::new(self.input_sink.ok_or(TokioServerError::MissingInputSink)?);
        let pairing_store = Arc::new(
            self.pairing_store
                .ok_or(TokioServerError::MissingPairingStore)?,
        );

        let identity = ensure_server_identity(pairing_store.as_ref()).await?;
        let tls_acceptor = create_tls_acceptor(&identity).map_err(ServerError::InvalidConfig)?;

        let http_listener = TcpListener::bind(self.config.bind_http).await?;
        let https_listener = TcpListener::bind(self.config.bind_https).await?;
        let rtsp_listener = TcpListener::bind(self.config.bind_rtsp).await?;
        let discovery_socket = UdpSocket::bind(self.config.bind_discovery).await?;
        let video_socket = Arc::new(UdpSocket::bind(self.config.bind_video).await?);
        let audio_socket = Arc::new(UdpSocket::bind(self.config.bind_audio).await?);
        let control_host = EnetHost::new(EnetHostConfig {
            address: Some(self.config.bind_control),
            peer_count: 32,
            channel_limit: 0x30,
            ..Default::default()
        })
        .map_err(|err| ServerError::InvalidConfig(err.to_string()))?;
        let control_addr = control_host
            .local_addr()
            .map_err(|err| ServerError::InvalidConfig(err.to_string()))?;

        let http_addr = http_listener.local_addr()?;
        let https_addr = https_listener.local_addr()?;
        let discovery_addr = discovery_socket.local_addr()?;
        let rtsp_addr = rtsp_listener.local_addr()?;
        let video_addr = video_socket.local_addr()?;
        let audio_addr = audio_socket.local_addr()?;
        let ports = StreamPorts {
            https: https_addr.port(),
            rtsp: rtsp_addr.port(),
            video: video_addr.port(),
            audio: audio_addr.port(),
            control: control_addr.port(),
        };

        let config = Arc::new(self.config);
        let pairing_sessions = Arc::new(Mutex::new(HashMap::new()));
        let control_crypto: ControlCrypto = Arc::new(Mutex::new(None));
        let video_peer = Arc::new(Mutex::new(StreamPeerTracker::default()));
        let audio_peer = Arc::new(Mutex::new(StreamPeerTracker::default()));
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let (runtime_config_tx, runtime_config_rx) =
            watch::channel(ServerRuntimeConfig::from(config.as_ref()));
        let (video_control_tx, _) = broadcast::channel(32);
        let (stream_stop_tx, _) = broadcast::channel(8);
        let (control_command_tx, control_command_rx) = mpsc::channel(8);
        let controller = ServerController::new(
            runtime_config_tx,
            stream_stop_tx.clone(),
            control_command_tx,
            Arc::clone(&video_peer),
            Arc::clone(&audio_peer),
        );

        let tasks = vec![
            tokio::spawn(serve_http(
                http_listener,
                Arc::clone(&config),
                runtime_config_rx.clone(),
                Arc::clone(&pairing_store),
                Arc::clone(&media_provider),
                Arc::clone(&input_sink),
                ports,
                Arc::clone(&pairing_sessions),
                Arc::clone(&control_crypto),
                shutdown_rx.clone(),
            )),
            tokio::spawn(serve_https(
                https_listener,
                tls_acceptor,
                Arc::clone(&config),
                runtime_config_rx.clone(),
                Arc::clone(&pairing_store),
                Arc::clone(&input_sink),
                ports,
                Arc::clone(&pairing_sessions),
                Arc::clone(&control_crypto),
                shutdown_tx.subscribe(),
            )),
            tokio::spawn(serve_discovery(
                discovery_socket,
                runtime_config_rx.clone(),
                shutdown_tx.subscribe(),
            )),
            tokio::spawn(serve_rtsp(
                rtsp_listener,
                runtime_config_rx,
                Arc::clone(&media_provider),
                Arc::clone(&video_socket),
                Arc::clone(&audio_socket),
                ports,
                Arc::clone(&video_peer),
                Arc::clone(&audio_peer),
                video_control_tx.clone(),
                stream_stop_tx.clone(),
                Arc::clone(&control_crypto),
                shutdown_tx.subscribe(),
            )),
            tokio::spawn(learn_stream_peer(
                Arc::clone(&video_socket),
                Arc::clone(&video_peer),
                "video",
                StreamPeerPolicy::moonlight_nat(),
                shutdown_tx.subscribe(),
            )),
            tokio::spawn(learn_stream_peer(
                Arc::clone(&audio_socket),
                Arc::clone(&audio_peer),
                "audio",
                StreamPeerPolicy::moonlight_nat(),
                shutdown_tx.subscribe(),
            )),
            tokio::spawn(serve_control(
                control_host,
                Arc::clone(&input_sink),
                control_crypto,
                video_control_tx,
                stream_stop_tx,
                control_command_rx,
                shutdown_tx.subscribe(),
            )),
        ];

        Ok(ServerHandle {
            http_addr,
            https_addr,
            discovery_addr,
            rtsp_addr,
            shutdown_tx,
            tasks,
            controller,
        })
    }
}
