use std::io::Write;
use std::sync::Arc;

use moonlink_core::{
    InputSink, MediaProvider, PairingPin, PairingRequest, PairingStore, ServerConfig, ServerError,
    ServerRuntimeConfig,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

use crate::pairing::handle_pair;
use crate::shared::{
    ControlCrypto, ControlCryptoState, PairingSessions, RiKeyMaterial, StreamPorts,
};
use crate::xml::{read_query_value, write_app_list_xml, write_launch_xml, write_server_info_xml};

pub(crate) async fn serve_http<P, I, S>(
    listener: TcpListener,
    config: Arc<ServerConfig>,
    runtime_config: watch::Receiver<ServerRuntimeConfig>,
    pairing_store: Arc<S>,
    media_provider: Arc<P>,
    input_sink: Arc<I>,
    ports: StreamPorts,
    pairing_sessions: PairingSessions,
    control_crypto: ControlCrypto,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), ServerError>
where
    P: MediaProvider,
    I: InputSink,
    S: PairingStore,
{
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, peer) = accepted?;
                let config = Arc::clone(&config);
                let runtime_config = runtime_config.clone();
                let pairing_store = Arc::clone(&pairing_store);
                let media_provider = Arc::clone(&media_provider);
                let input_sink = Arc::clone(&input_sink);
                let pairing_sessions = Arc::clone(&pairing_sessions);
                let control_crypto = Arc::clone(&control_crypto);
                tokio::spawn(async move {
                    let _ = process_http_connection(stream, peer.ip().to_string(), config, runtime_config, pairing_store, media_provider, input_sink, ports, pairing_sessions, control_crypto, false, None).await;
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

async fn process_http_connection<P, I, S>(
    stream: TcpStream,
    peer: String,
    config: Arc<ServerConfig>,
    runtime_config: watch::Receiver<ServerRuntimeConfig>,
    pairing_store: Arc<S>,
    media_provider: Arc<P>,
    input_sink: Arc<I>,
    ports: StreamPorts,
    pairing_sessions: PairingSessions,
    control_crypto: ControlCrypto,
    secure: bool,
    client_certificate: Option<Vec<u8>>,
) -> Result<(), ServerError>
where
    P: MediaProvider,
    I: InputSink,
    S: PairingStore,
{
    process_http_stream(
        stream,
        peer,
        config,
        runtime_config,
        pairing_store,
        media_provider,
        input_sink,
        ports,
        pairing_sessions,
        control_crypto,
        secure,
        client_certificate,
    )
    .await
}

pub(crate) async fn process_http_stream<T, P, I, S>(
    mut stream: T,
    peer: String,
    config: Arc<ServerConfig>,
    runtime_config: watch::Receiver<ServerRuntimeConfig>,
    pairing_store: Arc<S>,
    _media_provider: Arc<P>,
    _input_sink: Arc<I>,
    ports: StreamPorts,
    pairing_sessions: PairingSessions,
    control_crypto: ControlCrypto,
    secure: bool,
    client_certificate: Option<Vec<u8>>,
) -> Result<(), ServerError>
where
    T: AsyncRead + AsyncWrite + Unpin,
    P: MediaProvider,
    I: InputSink,
    S: PairingStore,
{
    let request = read_http_request(&mut stream).await?;
    let runtime_config = runtime_config.borrow().clone();
    let Some(path) = request_path(&request) else {
        write_http_response(&mut stream, 400, "Bad Request", "text/plain", "bad request").await?;
        return Ok(());
    };

    let trusted_client = match client_certificate.as_deref() {
        Some(certificate) => pairing_store.is_client_trusted(certificate).await?,
        None => false,
    };
    eprintln!(
        "{} request from {peer}: path={} client_certificate={} trusted={trusted_client}",
        if secure { "HTTPS" } else { "HTTP" },
        path.split('?').next().unwrap_or(path),
        client_certificate.is_some(),
    );
    if protected_https_path(path) && (!secure || !trusted_client) {
        return write_http_response(
            &mut stream,
            401,
            "Unauthorized",
            "text/xml",
            "<?xml version=\"1.0\" encoding=\"utf-8\"?><root status_code=\"401\"/>",
        )
        .await;
    }

    let body = if path.starts_with("/serverinfo") {
        let paired = secure && trusted_client;
        let server_unique_id = pairing_store
            .server_identity()
            .await?
            .map(|identity| identity.id.simple().to_string())
            .unwrap_or_default();
        write_server_info_xml(&config, &runtime_config, ports, paired, &server_unique_id)
    } else if path.starts_with("/applist") {
        write_app_list_xml(&config)
    } else if path.starts_with("/launch") || path.starts_with("/resume") {
        store_control_crypto(path, &control_crypto).await?;
        write_launch_xml()
    } else if path.starts_with("/pair") {
        let pin = resolve_pairing_pin(path, &config.pairing_pin).await?;
        handle_pair(path, &peer, &pin, pairing_store.as_ref(), pairing_sessions).await?
    } else if path.starts_with("/cancel") || path.starts_with("/unpair") {
        "<?xml version=\"1.0\" encoding=\"utf-8\"?><root status_code=\"200\"/>".to_owned()
    } else {
        "<?xml version=\"1.0\" encoding=\"utf-8\"?><root status_code=\"404\"/>".to_owned()
    };
    write_http_response(&mut stream, 200, "OK", "text/xml", &body).await
}

fn protected_https_path(path: &str) -> bool {
    path.starts_with("/applist")
        || path.starts_with("/launch")
        || path.starts_with("/resume")
        || path.starts_with("/unpair")
}

async fn resolve_pairing_pin(path: &str, pairing_pin: &PairingPin) -> Result<String, ServerError> {
    match pairing_pin {
        PairingPin::Static(pin) => Ok(pin.clone()),
        PairingPin::InteractiveStdin
            if read_query_value(path, "phrase").as_deref() == Some("getservercert") =>
        {
            tokio::task::spawn_blocking(read_pairing_pin_from_stdin)
                .await
                .map_err(|err| ServerError::InvalidConfig(err.to_string()))?
        }
        PairingPin::InteractiveStdin => Ok(String::new()),
        PairingPin::Provider(provider)
            if read_query_value(path, "phrase").as_deref() == Some("getservercert") =>
        {
            provider
                .request_pin(PairingRequest {
                    client_id: pairing_request_id(path),
                    display_name: read_query_value(path, "devicename"),
                })
                .await
                .ok_or_else(|| {
                    ServerError::InvalidConfig(
                        "pairing PIN request expired or was rejected".to_owned(),
                    )
                })
        }
        PairingPin::Provider(_) => Ok(String::new()),
    }
}

fn pairing_request_id(path: &str) -> String {
    read_query_value(path, "clientcert")
        .and_then(|certificate| hex::decode(certificate).ok())
        .filter(|certificate| !certificate.is_empty())
        .map(|certificate| moonlink_core::client_id_from_certificate(&certificate))
        .or_else(|| read_query_value(path, "uniqueid"))
        .unwrap_or_default()
}

fn read_pairing_pin_from_stdin() -> Result<String, ServerError> {
    print!("Moonlight pairing PIN: ");
    std::io::stdout()
        .flush()
        .map_err(|err| ServerError::InvalidConfig(err.to_string()))?;
    let mut pin = String::new();
    std::io::stdin()
        .read_line(&mut pin)
        .map_err(|err| ServerError::InvalidConfig(err.to_string()))?;
    let pin = pin.trim().to_owned();
    if pin.is_empty() {
        return Err(ServerError::InvalidConfig(
            "interactive pairing PIN cannot be empty".to_owned(),
        ));
    }
    Ok(pin)
}

async fn store_control_crypto(
    path: &str,
    control_crypto: &ControlCrypto,
) -> Result<(), ServerError> {
    let Some(key_hex) = read_query_value(path, "rikey") else {
        return Ok(());
    };
    let key = hex::decode(key_hex).map_err(|err| {
        ServerError::InvalidConfig(format!("launch rikey is not valid hex: {err}"))
    })?;
    let key: [u8; 16] = key
        .try_into()
        .map_err(|_| ServerError::InvalidConfig("launch rikey must be 16 bytes".to_owned()))?;
    let ri_key_id = read_query_value(path, "rikeyid")
        .as_deref()
        .and_then(parse_ri_key_id)
        .unwrap_or(0);
    *control_crypto.lock().await =
        Some(ControlCryptoState::new(RiKeyMaterial::new(key, ri_key_id)));
    Ok(())
}

fn parse_ri_key_id(value: &str) -> Option<u32> {
    value
        .parse::<i32>()
        .map(|value| value as u32)
        .or_else(|_| value.parse::<u32>())
        .ok()
}

async fn read_http_request<T>(stream: &mut T) -> Result<String, ServerError>
where
    T: AsyncRead + Unpin,
{
    let mut buffer = Vec::with_capacity(4096);
    let mut chunk = [0_u8; 1024];
    loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if buffer.len() > 64 * 1024 {
            return Err(ServerError::InvalidConfig(
                "HTTP request is too large".to_owned(),
            ));
        }
    }
    Ok(String::from_utf8_lossy(&buffer).to_string())
}

fn request_path(request: &str) -> Option<&str> {
    let first_line = request.lines().next()?;
    let mut parts = first_line.split_whitespace();
    let method = parts.next()?;
    let path = parts.next()?;
    if method == "GET" || method == "POST" {
        Some(path)
    } else {
        None
    }
}

async fn write_http_response<T>(
    stream: &mut T,
    status: u16,
    reason: &str,
    content_type: &str,
    body: &str,
) -> Result<(), ServerError>
where
    T: AsyncWrite + Unpin,
{
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes()).await?;
    stream.write_all(body.as_bytes()).await?;
    stream.shutdown().await?;
    Ok(())
}
