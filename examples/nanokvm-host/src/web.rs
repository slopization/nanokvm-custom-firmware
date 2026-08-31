use std::convert::Infallible;
use std::net::SocketAddr;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::header::{CONTENT_SECURITY_POLICY, CONTENT_TYPE, COOKIE, SET_COOKIE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio_stream::wrappers::ReceiverStream;

use crate::auth::AuthState;
use crate::cli::{AudioSettings, AudioSource, StreamCodec};
use crate::pairing_manager::{PairingManager, PendingPairing};
use crate::session_arbiter::{SessionArbiter, SessionOwner};
use crate::settings::{MoonlinkSettings, SettingsStore};
use crate::v4l2_provider::{V4l2Session, V4l2Settings, output_fps};
use moonlink_core::{MediaSession, MediaSessionConfig, PairingStore, VideoControlMessage};
use moonlink_protocol::VideoCodec;
use moonlink_tokio::{FilePairingStore, ServerController};

const SESSION_COOKIE: &str = "moonlink_session";

#[derive(Clone)]
struct WebState {
    auth: AuthState,
    settings: SettingsStore,
    ip_address: String,
    pairing_manager: PairingManager,
    pairing_store: FilePairingStore,
    server: ServerController,
    arbiter: SessionArbiter,
    web_v4l2: Option<V4l2Settings>,
}

#[derive(Serialize)]
struct Status {
    authenticated: bool,
    password_configured: bool,
    ip_address: String,
    capture_owner: Option<SessionOwner>,
}

#[derive(Serialize)]
struct Message {
    message: String,
}

#[derive(Deserialize)]
struct PasswordLogin {
    password: String,
}

#[derive(Deserialize)]
struct PasswordRecovery {
    otp: String,
    new_password: String,
}

#[derive(Deserialize)]
struct PasswordChange {
    current_password: String,
    new_password: String,
}

#[derive(Deserialize)]
struct PairingPinRequest {
    pin: String,
}

#[derive(Serialize)]
struct PairingDevices {
    pending: Vec<PendingPairing>,
    paired: Vec<PairedDevice>,
}

#[derive(Serialize)]
struct PairedDevice {
    client_id: String,
    display_name: Option<String>,
}

pub async fn serve_web(
    bind: SocketAddr,
    auth: AuthState,
    settings: SettingsStore,
    ip_address: String,
    pairing_manager: PairingManager,
    pairing_store: FilePairingStore,
    server: ServerController,
    arbiter: SessionArbiter,
    web_v4l2: Option<V4l2Settings>,
) -> std::io::Result<()> {
    let state = WebState {
        auth,
        settings,
        ip_address,
        pairing_manager,
        pairing_store,
        server,
        arbiter,
        web_v4l2,
    };
    let protected = Router::new()
        .route("/api/logout", post(logout))
        .route("/api/password/change", post(change_password))
        .route("/api/settings", get(get_settings).put(put_settings))
        .route("/api/pairing", get(get_pairing_devices))
        .route(
            "/api/pairing/{client_id}",
            post(submit_pairing_pin).delete(remove_pairing_device),
        )
        .route("/api/system/reboot", post(reboot))
        .route("/api/kvm/stream", get(kvm_stream))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_authentication,
        ));
    let router = Router::new()
        .route("/", get(index))
        .route("/style.css", get(stylesheet))
        .route("/app.js", get(script))
        .route("/kvm.js", get(kvm_script))
        .route("/libde265-worker.js", get(libde265_worker))
        .route("/vendor/libde265/libde265.mjs", get(libde265_script))
        .route("/vendor/libde265/libde265.wasm", get(libde265_wasm))
        .route("/api/status", get(status))
        .route("/api/login", post(login))
        .route("/api/password/recover", post(recover_password))
        .merge(protected)
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(bind).await?;
    println!("nanokvm web UI listening: http://{bind}");
    axum::serve(listener, router)
        .await
        .map_err(std::io::Error::other)
}

async fn index() -> Response {
    let mut response = Html(INDEX_HTML).into_response();
    response.headers_mut().insert(
        CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; object-src 'none'; frame-ancestors 'none'; worker-src 'self'"),
    );
    response
}

async fn stylesheet() -> Response {
    ([(CONTENT_TYPE, "text/css; charset=utf-8")], STYLE_CSS).into_response()
}

async fn script() -> Response {
    ([(CONTENT_TYPE, "text/javascript; charset=utf-8")], APP_JS).into_response()
}

async fn kvm_script() -> Response {
    ([(CONTENT_TYPE, "text/javascript; charset=utf-8")], KVM_JS).into_response()
}

async fn libde265_worker() -> Response {
    (
        [(CONTENT_TYPE, "text/javascript; charset=utf-8")],
        LIBDE265_WORKER,
    )
        .into_response()
}

async fn libde265_script() -> Response {
    (
        [(CONTENT_TYPE, "text/javascript; charset=utf-8")],
        LIBDE265_SCRIPT,
    )
        .into_response()
}

async fn libde265_wasm() -> Response {
    ([(CONTENT_TYPE, "application/wasm")], LIBDE265_WASM).into_response()
}

async fn status(State(state): State<WebState>, headers: HeaderMap) -> Json<Status> {
    let authenticated = authenticate(&state.auth, &headers).await;
    Json(Status {
        authenticated,
        password_configured: state.auth.has_password().await,
        ip_address: state.ip_address,
        capture_owner: state.arbiter.owner(),
    })
}

async fn login(State(state): State<WebState>, Json(request): Json<PasswordLogin>) -> Response {
    match state.auth.authenticate_password(&request.password).await {
        Some(token) => session_response(StatusCode::OK, "signed in", &token),
        None => message(StatusCode::UNAUTHORIZED, "invalid password"),
    }
}

async fn logout(State(state): State<WebState>, headers: HeaderMap) -> Response {
    if let Some(token) = session_token(&headers) {
        state.auth.remove_session(token).await;
    }
    let mut response = message(StatusCode::OK, "signed out");
    response.headers_mut().insert(
        SET_COOKIE,
        HeaderValue::from_static("moonlink_session=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0"),
    );
    response
}

async fn recover_password(
    State(state): State<WebState>,
    Json(request): Json<PasswordRecovery>,
) -> Response {
    match state
        .auth
        .recover_password(&request.otp, &request.new_password)
        .await
    {
        Ok(Some(token)) => session_response(StatusCode::OK, "password updated", &token),
        Ok(None) => message(StatusCode::UNAUTHORIZED, "invalid or expired recovery code"),
        Err(err) => message(StatusCode::BAD_REQUEST, &err.to_string()),
    }
}

async fn change_password(
    State(state): State<WebState>,
    Json(request): Json<PasswordChange>,
) -> Response {
    match state
        .auth
        .change_password(&request.current_password, &request.new_password)
        .await
    {
        Ok(true) => message(StatusCode::OK, "password updated; sign in again"),
        Ok(false) => message(StatusCode::UNAUTHORIZED, "current password is incorrect"),
        Err(err) => message(StatusCode::BAD_REQUEST, &err.to_string()),
    }
}

async fn get_settings(State(state): State<WebState>) -> Response {
    Json(state.settings.get().await).into_response()
}

async fn put_settings(
    State(state): State<WebState>,
    Json(settings): Json<MoonlinkSettings>,
) -> Response {
    let runtime_config = match settings.runtime_config() {
        Ok(config) => config,
        Err(err) => return message(StatusCode::BAD_REQUEST, err),
    };
    let previous = state.settings.get().await;
    let disconnect = settings.changes_stream(&previous);
    match state.settings.save(settings).await {
        Ok(()) => {
            state.server.apply_runtime_config(runtime_config);
            if disconnect {
                state.server.disconnect_clients().await;
            }
            message(StatusCode::OK, "settings applied")
        }
        Err(err) => message(StatusCode::BAD_REQUEST, &err.to_string()),
    }
}

async fn get_pairing_devices(State(state): State<WebState>) -> Response {
    let paired = match state.pairing_store.trusted_clients().await {
        Ok(clients) => clients
            .into_iter()
            .map(|client| PairedDevice {
                client_id: client.identity.id,
                display_name: client.display_name,
            })
            .collect(),
        Err(err) => return message(StatusCode::INTERNAL_SERVER_ERROR, &err.to_string()),
    };
    Json(PairingDevices {
        pending: state.pairing_manager.pending().await,
        paired,
    })
    .into_response()
}

async fn submit_pairing_pin(
    State(state): State<WebState>,
    Path(client_id): Path<String>,
    Json(request): Json<PairingPinRequest>,
) -> Response {
    match state
        .pairing_manager
        .submit_pin(&client_id, &request.pin)
        .await
    {
        Ok(()) => message(StatusCode::OK, "pairing PIN submitted"),
        Err(err) => message(StatusCode::BAD_REQUEST, err),
    }
}

async fn remove_pairing_device(
    State(state): State<WebState>,
    Path(client_id): Path<String>,
) -> Response {
    state.pairing_manager.reject(&client_id).await;
    match state.pairing_store.remove_client(&client_id).await {
        Ok(()) => message(StatusCode::OK, "device removed"),
        Err(err) => message(StatusCode::INTERNAL_SERVER_ERROR, &err.to_string()),
    }
}

async fn reboot() -> Response {
    tokio::spawn(async {
        tokio::time::sleep(Duration::from_millis(750)).await;
        let _ = tokio::process::Command::new("/sbin/reboot").status().await;
    });
    message(StatusCode::ACCEPTED, "rebooting")
}

async fn kvm_stream(State(state): State<WebState>) -> Response {
    let Some(mut settings) = state.web_v4l2.clone() else {
        return message(
            StatusCode::NOT_IMPLEMENTED,
            "browser capture requires the V4L2 source",
        );
    };
    let lease = match state.arbiter.acquire(SessionOwner::Web) {
        Ok(lease) => lease,
        Err(owner) => {
            return message(
                StatusCode::CONFLICT,
                &format!("capture is owned by {owner:?}"),
            );
        }
    };
    let current = state.settings.get().await;
    current.apply_v4l2(&mut settings);
    settings.codec = StreamCodec::Hevc;
    let fps = match output_fps(&settings) {
        Ok(fps) => fps,
        Err(err) => return message(StatusCode::BAD_REQUEST, &err.to_string()),
    };
    let config = MediaSessionConfig {
        width: settings.capture_width,
        height: settings.capture_height,
        fps,
        video_codec: VideoCodec::Hevc,
        audio_codec: None,
        ..MediaSessionConfig::default()
    };
    let audio = AudioSettings {
        source: AudioSource::None,
        pcm_device: None,
        opus_bitrate_kbps: 128,
        opus_complexity: 0,
    };
    let mut session = match V4l2Session::start(settings.clone(), audio, config).await {
        Ok(session) => session,
        Err(err) => return message(StatusCode::SERVICE_UNAVAILABLE, &err.to_string()),
    };
    let (sender, receiver) = tokio::sync::mpsc::channel::<Result<bytes::Bytes, Infallible>>(4);
    let preamble = stream_preamble(settings.capture_width, settings.capture_height, fps);
    sender
        .try_send(Ok(bytes::Bytes::copy_from_slice(&preamble)))
        .expect("new stream channel");
    tokio::spawn(async move {
        let _lease = lease;
        let mut waiting_for_keyframe = false;
        loop {
            let frame = match session.next_encoded().await {
                Ok(Some(frame)) => frame,
                Ok(None) => continue,
                Err(err) => {
                    eprintln!("browser capture stopped: {err}");
                    break;
                }
            };
            if waiting_for_keyframe && !frame.keyframe {
                continue;
            }
            let packet = match stream_packet(frame.bytes, frame.timestamp_90khz, frame.keyframe) {
                Ok(packet) => packet,
                Err(err) => {
                    eprintln!("browser frame dropped: {err}");
                    continue;
                }
            };
            match sender.try_send(Ok(packet)) {
                Ok(()) => waiting_for_keyframe = false,
                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                    if !waiting_for_keyframe {
                        waiting_for_keyframe = true;
                        let _ = session
                            .handle_video_control(VideoControlMessage::RequestIdrFrame)
                            .await;
                    }
                }
                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => break,
            }
        }
        let _ = session.stop().await;
    });

    let mut response = Response::new(Body::from_stream(ReceiverStream::new(receiver)));
    *response.status_mut() = StatusCode::OK;
    let headers = response.headers_mut();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/vnd.nanokvm.hevc"),
    );
    headers.insert(
        "cache-control",
        HeaderValue::from_static("no-store, no-transform"),
    );
    headers.insert("x-accel-buffering", HeaderValue::from_static("no"));
    response
}

fn stream_preamble(width: u32, height: u32, fps: u32) -> [u8; 24] {
    let mut bytes = [0; 24];
    bytes[..8].copy_from_slice(b"NKHEVC01");
    bytes[8..12].copy_from_slice(&width.to_be_bytes());
    bytes[12..16].copy_from_slice(&height.to_be_bytes());
    bytes[16..20].copy_from_slice(&fps.to_be_bytes());
    bytes[20..24].copy_from_slice(&90_000_u32.to_be_bytes());
    bytes
}

fn stream_packet(
    payload: Vec<u8>,
    timestamp: u32,
    keyframe: bool,
) -> Result<bytes::Bytes, &'static str> {
    let length =
        u32::try_from(payload.len()).map_err(|_| "encoded frame exceeds protocol limit")?;
    let mut packet = Vec::with_capacity(12 + payload.len());
    packet.extend_from_slice(&length.to_be_bytes());
    packet.extend_from_slice(&timestamp.to_be_bytes());
    packet.push(u8::from(keyframe));
    packet.extend_from_slice(&[0; 3]);
    packet.extend_from_slice(&payload);
    Ok(packet.into())
}

async fn authenticate(auth: &AuthState, headers: &HeaderMap) -> bool {
    match session_token(headers) {
        Some(token) => auth.validate_session(token).await,
        None => false,
    }
}

async fn require_authentication(
    State(state): State<WebState>,
    request: Request,
    next: Next,
) -> Response {
    if authenticate(&state.auth, request.headers()).await {
        next.run(request).await
    } else {
        unauthorized()
    }
}

fn session_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .map(str::trim)
        .find_map(|cookie| cookie.strip_prefix("moonlink_session="))
}

fn session_response(status: StatusCode, body: &str, token: &str) -> Response {
    let mut response = message(status, body);
    let cookie =
        format!("{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age=43200");
    response.headers_mut().insert(
        SET_COOKIE,
        HeaderValue::from_str(&cookie).expect("session cookie"),
    );
    response
}

fn unauthorized() -> Response {
    message(StatusCode::UNAUTHORIZED, "authentication required")
}

fn message(status: StatusCode, body: &str) -> Response {
    let mut response = (
        status,
        Json(Message {
            message: body.to_owned(),
        }),
    )
        .into_response();
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    response
}

const INDEX_HTML: &str = include_str!("../web/index.html");
const STYLE_CSS: &str = include_str!("../web/style.css");
const APP_JS: &str = include_str!("../web/app.js");
const KVM_JS: &str = include_str!("../web/kvm.js");
const LIBDE265_WORKER: &str = include_str!("../web/libde265-worker.js");
const LIBDE265_SCRIPT: &str = include_str!("../web/vendor/libde265/libde265.mjs");
const LIBDE265_WASM: &[u8] = include_bytes!("../web/vendor/libde265/libde265.wasm");

#[cfg(test)]
mod stream_tests {
    use super::*;

    #[test]
    fn stream_wire_format_is_network_order_and_self_delimiting() {
        let preamble = stream_preamble(1920, 1080, 60);
        assert_eq!(&preamble[..8], b"NKHEVC01");
        assert_eq!(
            u32::from_be_bytes(preamble[8..12].try_into().unwrap()),
            1920
        );
        assert_eq!(
            u32::from_be_bytes(preamble[20..24].try_into().unwrap()),
            90_000
        );

        let packet = stream_packet(vec![1, 2, 3], 1500, true).unwrap();
        assert_eq!(u32::from_be_bytes(packet[..4].try_into().unwrap()), 3);
        assert_eq!(u32::from_be_bytes(packet[4..8].try_into().unwrap()), 1500);
        assert_eq!(&packet[8..12], &[1, 0, 0, 0]);
        assert_eq!(&packet[12..], &[1, 2, 3]);
    }
}
