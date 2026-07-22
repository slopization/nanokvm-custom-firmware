use std::sync::Arc;

use moonlink_core::{InputSink, PairingStore, ServerConfig, ServerError, ServerRuntimeConfig};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio_rustls::TlsAcceptor;

use crate::http_loop::process_http_stream;
use crate::shared::{ControlCrypto, PairingSessions, StreamPorts};

pub(crate) async fn serve_https<I, S>(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    config: Arc<ServerConfig>,
    runtime_config: watch::Receiver<ServerRuntimeConfig>,
    pairing_store: Arc<S>,
    input_sink: Arc<I>,
    ports: StreamPorts,
    pairing_sessions: PairingSessions,
    control_crypto: ControlCrypto,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), ServerError>
where
    I: InputSink,
    S: PairingStore,
{
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, peer) = accepted?;
                let acceptor = acceptor.clone();
                let config = Arc::clone(&config);
                let runtime_config = runtime_config.clone();
                let pairing_store = Arc::clone(&pairing_store);
                let input_sink = Arc::clone(&input_sink);
                let pairing_sessions = Arc::clone(&pairing_sessions);
                let control_crypto = Arc::clone(&control_crypto);
                tokio::spawn(async move {
                    let stream = match acceptor.accept(stream).await {
                        Ok(stream) => stream,
                        Err(err) => {
                            eprintln!("HTTPS handshake from {peer} failed: {err}");
                            return;
                        }
                    };
                    let client_certificate = stream
                        .get_ref()
                        .1
                        .peer_certificates()
                        .and_then(|certificates| certificates.first())
                        .map(|certificate| certificate.as_ref().to_vec());
                    if let Err(err) = process_http_stream(
                        stream,
                        peer.ip().to_string(),
                        config,
                        runtime_config,
                        pairing_store,
                        Arc::new(EmptyMediaProvider),
                        input_sink,
                        ports,
                        pairing_sessions,
                        control_crypto,
                        true,
                        client_certificate,
                    ).await {
                        eprintln!("HTTPS request from {peer} failed: {err}");
                    }
                });
            }
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    return Ok(());
                }
            }
        }
    }
}

struct EmptyMediaProvider;

#[async_trait::async_trait]
impl moonlink_core::MediaProvider for EmptyMediaProvider {
    type Session = EmptyMediaSession;

    async fn start_session(
        &self,
        _config: moonlink_core::MediaSessionConfig,
    ) -> Result<Self::Session, moonlink_core::MediaSessionError> {
        Ok(EmptyMediaSession)
    }
}

struct EmptyMediaSession;

#[async_trait::async_trait]
impl moonlink_core::MediaSession for EmptyMediaSession {
    async fn next_video(
        &mut self,
    ) -> Result<Option<moonlink_core::VideoFrame>, moonlink_core::MediaSessionError> {
        Ok(None)
    }
}
