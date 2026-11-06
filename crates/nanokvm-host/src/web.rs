use std::net::SocketAddr;
use std::time::Duration;

use axum::extract::{Path, Request, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE, COOKIE, LOCATION};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::atx::AtxController;
use crate::auth::AuthState;
use crate::browser_stream::BrowserCapture;
use crate::cli::AudioSettings;
use crate::hid::UsbGadgetInputSink;
use crate::pairing_manager::{PairingManager, PendingPairing};
use crate::session_arbiter::SessionArbiter;
use crate::settings::{FlatSettings, Settings, SettingsStore};
use crate::v4l2_provider::V4l2Settings;
use moonlink_core::PairingStore;
use moonlink_tokio::{FilePairingStore, ServerController};

#[derive(Clone)]
struct WebState {
    auth: AuthState,
    settings: SettingsStore,
    pairing_manager: PairingManager,
    pairing_store: FilePairingStore,
    server: ServerController,
    browser_capture: BrowserCapture,
}

#[derive(Serialize)]
struct Message {
    message: String,
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
    pairing_manager: PairingManager,
    pairing_store: FilePairingStore,
    server: ServerController,
    arbiter: SessionArbiter,
    web_v4l2: Option<V4l2Settings>,
    web_audio: AudioSettings,
    input_sink: UsbGadgetInputSink,
    atx: Option<AtxController>,
    mass_storage: Option<crate::msd::MassStorageController>,
) -> std::io::Result<()> {
    let browser_capture =
        BrowserCapture::new(arbiter.clone(), settings.clone(), web_v4l2, web_audio);
    let pikvm_settings = settings.clone();
    let state = WebState {
        auth: auth.clone(),
        settings,
        pairing_manager,
        pairing_store,
        server,
        browser_capture,
    };
    let protected = Router::new()
        .route("/api/password/change", post(change_password))
        .route("/api/settings", get(get_settings).put(put_settings))
        .route(
            "/api/settings/sections",
            get(get_sections).put(put_sections),
        )
        .route("/api/pairing", get(get_pairing_devices))
        .route(
            "/api/pairing/{client_id}",
            post(submit_pairing_pin).delete(remove_pairing_device),
        )
        .route("/api/system/reboot", post(reboot))
        .route("/api/kvm/stream", get(kvm_stream))
        .route("/api/kvm/audio", get(kvm_audio))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_authentication,
        ));
    let router = Router::new()
        .route("/", get(index))
        .route("/binary.js", get(binary_script))
        .route("/hevc.js", get(hevc_script))
        .route("/isobmff.js", get(isobmff_script))
        .route("/http-stream.js", get(http_stream_script))
        .route("/mse-player.js", get(mse_player_script))
        .route("/software-player.js", get(software_player_script))
        .route("/libde265-worker.js", get(libde265_worker))
        .route("/vendor/libde265/libde265.mjs", get(libde265_script))
        .route("/vendor/libde265/libde265.wasm", get(libde265_wasm))
        .route(
            "/vendor/libde265-threaded/libde265.mjs",
            get(libde265_threaded_script),
        )
        .route(
            "/vendor/libde265-threaded/libde265.wasm",
            get(libde265_threaded_wasm),
        )
        .merge(protected)
        .with_state(state)
        .merge(crate::pikvm::routes(
            auth.clone(),
            input_sink,
            atx,
            mass_storage,
            pikvm_settings,
        ))
        .layer(middleware::from_fn(cross_origin_isolation));
    let listener = tokio::net::TcpListener::bind(bind).await?;
    println!("nanokvm web UI listening: http://{bind}");
    axum::serve(listener, router)
        .await
        .map_err(std::io::Error::other)
}

async fn cross_origin_isolation(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        "cross-origin-opener-policy",
        HeaderValue::from_static("same-origin"),
    );
    response.headers_mut().insert(
        "cross-origin-embedder-policy",
        HeaderValue::from_static("require-corp"),
    );
    response
}

async fn index() -> Response {
    (
        StatusCode::TEMPORARY_REDIRECT,
        [(LOCATION, HeaderValue::from_static("/kvm/"))],
    )
        .into_response()
}

async fn binary_script() -> Response {
    javascript(BINARY_JS)
}

async fn hevc_script() -> Response {
    javascript(HEVC_JS)
}

async fn isobmff_script() -> Response {
    javascript(ISOBMFF_JS)
}

async fn http_stream_script() -> Response {
    javascript(HTTP_STREAM_JS)
}

async fn mse_player_script() -> Response {
    javascript(MSE_PLAYER_JS)
}

async fn software_player_script() -> Response {
    javascript(SOFTWARE_PLAYER_JS)
}

async fn libde265_worker() -> Response {
    javascript(LIBDE265_WORKER)
}

async fn libde265_script() -> Response {
    javascript(LIBDE265_SCRIPT)
}

async fn libde265_wasm() -> Response {
    ([(CONTENT_TYPE, "application/wasm")], LIBDE265_WASM).into_response()
}

async fn libde265_threaded_script() -> Response {
    javascript(LIBDE265_THREADED_SCRIPT)
}

async fn libde265_threaded_wasm() -> Response {
    ([(CONTENT_TYPE, "application/wasm")], LIBDE265_THREADED_WASM).into_response()
}

fn javascript(source: &'static str) -> Response {
    (
        [
            (CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (CACHE_CONTROL, "no-store, no-transform"),
        ],
        source,
    )
        .into_response()
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
    let settings = state.settings.get().await;
    Json(FlatSettings::from_settings(&settings)).into_response()
}

async fn get_sections(State(state): State<WebState>) -> Response {
    Json(state.settings.get().await).into_response()
}

async fn put_settings(State(state): State<WebState>, Json(flat): Json<FlatSettings>) -> Response {
    let previous = state.settings.get().await;
    let mut updated = previous.clone();
    flat.apply_to(&mut updated);
    apply_settings(state, previous, updated, false).await
}

async fn put_sections(State(state): State<WebState>, Json(settings): Json<Settings>) -> Response {
    let previous = state.settings.get().await;
    apply_settings(state, previous, settings, true).await
}

async fn apply_settings(
    state: WebState,
    previous: Settings,
    updated: Settings,
    web_touched: bool,
) -> Response {
    if let Err(err) = updated.validate() {
        return message(StatusCode::BAD_REQUEST, err);
    }
    let runtime_config = match updated.moonlight_runtime_config() {
        Ok(config) => config,
        Err(err) => return message(StatusCode::BAD_REQUEST, err),
    };
    let web_changed = web_touched && updated.web_stream_changed(&previous);
    let moonlight_changed = updated.moonlight_stream_changed(&previous);
    let restart = updated.requires_restart(&previous);

    match state.settings.save(updated).await {
        Ok(()) => {
            if restart {
                restart_service();
                return message(
                    StatusCode::ACCEPTED,
                    "settings saved; USB reconnecting; sign in again",
                );
            }
            state.server.apply_runtime_config(runtime_config);
            if moonlight_changed {
                state.server.disconnect_clients().await;
            }
            if web_changed {
                state.browser_capture.stop_active().await;
            }
            message(StatusCode::OK, "settings applied")
        }
        Err(err) => message(StatusCode::BAD_REQUEST, &err.to_string()),
    }
}

fn restart_service() {
    tokio::spawn(async {
        tokio::time::sleep(Duration::from_millis(750)).await;
        let _ = tokio::process::Command::new("systemctl")
            .args(["--no-block", "restart", "nanokvm-host.service"])
            .status()
            .await;
    });
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
    state.browser_capture.stream().await
}

async fn kvm_audio(State(state): State<WebState>) -> Response {
    state.browser_capture.audio_stream().await
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

const BINARY_JS: &str = include_str!("../web/binary.js");
const HEVC_JS: &str = include_str!("../web/hevc.js");
const ISOBMFF_JS: &str = include_str!("../web/isobmff.js");
const HTTP_STREAM_JS: &str = include_str!("../web/http-stream.js");
const MSE_PLAYER_JS: &str = include_str!("../web/mse-player.js");
const SOFTWARE_PLAYER_JS: &str = include_str!("../web/software-player.js");
const LIBDE265_WORKER: &str = include_str!("../web/libde265-worker.js");
const LIBDE265_SCRIPT: &str = include_str!("../web/vendor/libde265/libde265.mjs");
const LIBDE265_WASM: &[u8] = include_bytes!("../web/vendor/libde265/libde265.wasm");
const LIBDE265_THREADED_SCRIPT: &str = include_str!("../web/vendor/libde265-threaded/libde265.mjs");
const LIBDE265_THREADED_WASM: &[u8] =
    include_bytes!("../web/vendor/libde265-threaded/libde265.wasm");
