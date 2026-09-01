use std::borrow::Cow;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::Form;
use axum::body::Body;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path as AxumPath, Query, State};
use axum::http::header::{
    CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_TYPE, COOKIE, LOCATION, SET_COOKIE,
};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rust_embed::RustEmbed;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_stream::StreamExt;

use crate::atx::{AtxButton, AtxController};
use crate::auth::AuthState;
use crate::hid::UsbGadgetInputSink;
use crate::msd::MassStorageController;
use crate::settings::SettingsStore;
use moonlink_tokio::ServerController;

const SESSION_COOKIE: &str = "moonlink_session";

#[derive(RustEmbed)]
#[folder = "../../third_party/pikvm-kvmd/web/"]
struct Assets;

#[derive(Clone)]
struct PikvmState {
    auth: AuthState,
    hid: UsbGadgetInputSink,
    atx: Option<AtxController>,
    mass_storage: Option<MassStorageController>,
    jiggler: Jiggler,
    settings: SettingsStore,
    server: ServerController,
}

#[derive(Clone, Default)]
struct Jiggler(Arc<AtomicBool>);

#[derive(Deserialize)]
struct LoginForm {
    user: String,
    passwd: String,
    otp: Option<String>,
    #[serde(rename = "expire")]
    _expire: Option<u64>,
}

#[derive(Deserialize)]
struct AtxClickForm {
    button: String,
}

#[derive(Deserialize)]
struct MsdParams {
    image: Option<String>,
    cdrom: Option<String>,
    rw: Option<String>,
}

#[derive(Deserialize)]
struct MsdConnected {
    connected: String,
}

#[derive(Deserialize)]
struct MsdImage {
    image: String,
}

#[derive(Deserialize)]
struct MsdWrite {
    #[allow(dead_code)]
    prefix: Option<String>,
    image: String,
    #[allow(dead_code)]
    remove_incomplete: Option<String>,
}

#[derive(Deserialize)]
struct MsdRemoteWrite {
    #[allow(dead_code)]
    prefix: Option<String>,
    url: String,
    #[allow(dead_code)]
    remove_incomplete: Option<String>,
}

#[derive(Deserialize)]
struct HidParams {
    jiggler: Option<String>,
}

#[derive(Deserialize)]
struct HidPrintParams {
    #[allow(dead_code)]
    limit: Option<usize>,
    keymap: Option<String>,
    delay: Option<f64>,
}

#[derive(Deserialize)]
struct StreamerParams {
    desired_fps: Option<u32>,
    resolution: Option<String>,
    quality: Option<u32>,
    h264_bitrate: Option<u32>,
    h264_gop: Option<u32>,
}

pub fn routes(
    auth: AuthState,
    hid: UsbGadgetInputSink,
    atx: Option<AtxController>,
    mass_storage: Option<MassStorageController>,
    settings: SettingsStore,
    server: ServerController,
) -> Router {
    let state = PikvmState {
        auth,
        hid,
        atx,
        mass_storage,
        jiggler: Jiggler::default(),
        settings,
        server,
    };
    Router::new()
        .route("/kvm", get(redirect_kvm))
        .route("/kvm/", get(kvm_index))
        .route("/kvm/{*path}", get(kvm_asset))
        .route("/login", get(redirect_login))
        .route("/login/", get(login_index))
        .route("/login/{*path}", get(login_asset))
        .route("/share/{*path}", get(shared_asset))
        .route("/favicon.ico", get(favicon))
        .route("/api/auth/check", get(auth_check))
        .route("/api/auth/login", post(auth_login))
        .route("/api/auth/logout", post(auth_logout))
        .route("/api/ws", get(websocket))
        .route("/api/atx/click", post(atx_click))
        .route("/api/hid/reset", post(hid_reset))
        .route("/api/hid/print", post(hid_print))
        .route("/api/hid/set_params", post(hid_set_params))
        .route("/api/hid/set_connected", post(not_implemented))
        .route("/api/streamer/reset", post(streamer_reset))
        .route("/api/streamer/set_params", post(streamer_set_params))
        .route("/api/msd/set_params", post(msd_set_params))
        .route("/api/msd/set_connected", post(msd_set_connected))
        .route("/api/msd/reset", post(msd_reset))
        .route("/api/msd/remove", post(msd_remove))
        .route("/api/msd/read", get(msd_read))
        .route("/api/msd/write", post(msd_write))
        .route("/api/msd/write_remote", post(msd_write_remote))
        .with_state(state)
}

async fn redirect_kvm() -> Response {
    redirect("/kvm/")
}

async fn redirect_login() -> Response {
    redirect("/login/")
}

fn redirect(location: &'static str) -> Response {
    (StatusCode::TEMPORARY_REDIRECT, [(LOCATION, location)]).into_response()
}

async fn kvm_index(State(state): State<PikvmState>, headers: HeaderMap) -> Response {
    if authenticated(&state.auth, &headers).await {
        asset("kvm/index.html")
    } else {
        redirect("/login/")
    }
}

async fn kvm_asset(
    State(state): State<PikvmState>,
    headers: HeaderMap,
    AxumPath(path): AxumPath<String>,
) -> Response {
    if path == "index.html" && !authenticated(&state.auth, &headers).await {
        return redirect("/login/");
    }
    asset(&format!("kvm/{path}"))
}

async fn login_index() -> Response {
    asset("login/index.html")
}

async fn login_asset(AxumPath(path): AxumPath<String>) -> Response {
    asset(&format!("login/{path}"))
}

async fn shared_asset(AxumPath(path): AxumPath<String>) -> Response {
    asset(&format!("share/{path}"))
}

async fn favicon() -> Response {
    asset("favicon.ico")
}

fn asset(path: &str) -> Response {
    let Some(file) = Assets::get(path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let content_type = mime_guess::from_path(path).first_or_octet_stream();
    let body = match file.data {
        Cow::Borrowed(bytes) => Body::from(bytes),
        Cow::Owned(bytes) => Body::from(bytes),
    };
    let mut response = Response::new(body);
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_str(content_type.as_ref()).expect("valid embedded asset MIME type"),
    );
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn auth_check(State(state): State<PikvmState>, headers: HeaderMap) -> StatusCode {
    if authenticated(&state.auth, &headers).await {
        StatusCode::OK
    } else {
        StatusCode::UNAUTHORIZED
    }
}

async fn auth_login(State(state): State<PikvmState>, Form(form): Form<LoginForm>) -> Response {
    let token = match authenticate_login(&state.auth, &form).await {
        Ok(token) => token,
        Err(response) => return response,
    };
    let mut response = StatusCode::OK.into_response();
    let cookie =
        format!("{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age=43200");
    response.headers_mut().insert(
        SET_COOKIE,
        HeaderValue::from_str(&cookie).expect("valid session cookie"),
    );
    response
}

async fn authenticate_login(auth: &AuthState, form: &LoginForm) -> Result<String, Response> {
    if form.user != "root" {
        return Err(StatusCode::FORBIDDEN.into_response());
    }
    if let Some(otp) = form.otp.as_deref().filter(|otp| !otp.is_empty()) {
        match auth.recover_password(otp, &form.passwd).await {
            Ok(Some(token)) => Ok(token),
            Ok(None) => return Err(StatusCode::FORBIDDEN.into_response()),
            Err(error) => {
                return Err((StatusCode::BAD_REQUEST, error.to_string()).into_response());
            }
        }
    } else {
        let Some(token) = auth.authenticate_password(&form.passwd).await else {
            return Err(StatusCode::FORBIDDEN.into_response());
        };
        Ok(token)
    }
}

async fn auth_logout(State(state): State<PikvmState>, headers: HeaderMap) -> Response {
    if let Some(token) = session_token(&headers) {
        state.auth.remove_session(token).await;
    }
    let mut response = StatusCode::OK.into_response();
    response.headers_mut().insert(
        SET_COOKIE,
        HeaderValue::from_static("moonlink_session=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0"),
    );
    response
}

async fn websocket(
    State(state): State<PikvmState>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    if !authenticated(&state.auth, &headers).await {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    upgrade.on_upgrade(move |socket| websocket_session(socket, state))
}

async fn websocket_session(mut socket: WebSocket, state: PikvmState) {
    for event in initial_events(&state).await {
        if send_event(&mut socket, event).await.is_err() {
            return;
        }
    }
    let mut status_tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            _ = status_tick.tick() => {
                if send_event(&mut socket, atx_state(&state)).await.is_err() {
                    break;
                }
                if send_event(&mut socket, hid_state(&state)).await.is_err() {
                    break;
                }
                if send_event(&mut socket, streamer_state(&state).await).await.is_err() {
                    break;
                }
                if let Some(mass_storage) = &state.mass_storage
                    && send_event(&mut socket, mass_storage.snapshot().await).await.is_err()
                {
                    break;
                }
            }
            message = socket.recv() => {
                let Some(Ok(message)) = message else { break; };
                match message {
                    Message::Binary(data) if data.first() == Some(&0) => {
                        if socket.send(Message::Binary(vec![255].into())).await.is_err() { break; }
                    }
                    Message::Binary(data) => {
                        if let Err(error) = handle_hid(&state.hid, &data).await {
                            eprintln!("PiKVM HID event failed: {error}");
                        }
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }
        }
    }
    let _ = state.hid.release_all().await;
}

async fn initial_events(state: &PikvmState) -> Vec<Value> {
    let mut events = vec![
        hid_state(state),
        json!({"event_type":"hid_keymaps","event":{
            "keymaps":{"default":"en-us","available":["en-us"]}
        }}),
        atx_state(state),
        streamer_state(state).await,
    ];
    events.push(match &state.mass_storage {
        Some(mass_storage) => mass_storage.snapshot().await,
        None => json!({"event_type":"msd","event":{"enabled":false,"online":false}}),
    });
    events
}

fn atx_state(state: &PikvmState) -> Value {
    let (enabled, power, busy) = match &state.atx {
        Some(atx) => (true, atx.power_led().unwrap_or(false), atx.busy()),
        None => (false, false, false),
    };
    json!({"event_type":"atx","event":{
        "enabled":enabled,"busy":busy,"leds":{"power":power,"hdd":false}
    }})
}

fn hid_state(state: &PikvmState) -> Value {
    json!({"event_type":"hid","event":{
        "enabled":true,"online":true,"busy":false,"connected":null,
        "jiggler":{"enabled":true,"active":state.jiggler.0.load(Ordering::Acquire)},
        "keyboard":{"online":true,"outputs":{"available":[],"active":"usb"},"leds":{"caps":false,"scroll":false,"num":false}},
        "mouse":{"online":true,"outputs":{"available":[],"active":"usb"},"absolute":true}
    }})
}

async fn streamer_state(state: &PikvmState) -> Value {
    let settings = state.settings.get().await;
    let output_fps = settings.advertised_fps / settings.frame_divider.max(1);
    let resolution = format!("{}x{}", settings.capture_width, settings.capture_height);
    json!({"event_type":"streamer","event":{
        "features":{"hevc":true,"h264":false,"quality":false,"resolution":false},
        "limits":{
            "desired_fps":{"min":1,"max":settings.advertised_fps},
            "available_resolutions":[resolution],
        },
        "applied":{
            "desired_fps":output_fps,
            "resolution":resolution,
            "h264_bitrate":settings.bitrate_kbps,
            "h264_gop":settings.gop,
        },
        "streamer":{"source":{"online":true,"resolution":{
            "width":settings.capture_width,"height":settings.capture_height
        }}}
    }})
}

async fn send_event(socket: &mut WebSocket, event: Value) -> Result<(), axum::Error> {
    socket.send(Message::Text(event.to_string().into())).await
}

async fn handle_hid(hid: &UsbGadgetInputSink, data: &[u8]) -> std::io::Result<()> {
    match data.first().copied() {
        Some(1) if data.len() >= 3 => {
            let code = std::str::from_utf8(&data[2..]).unwrap_or_default();
            hid.write_dom_key(code, data[1] & 1 != 0).await?;
            if data[1] & 2 != 0 {
                hid.write_dom_key(code, false).await?;
            }
        }
        Some(2) if data.len() >= 3 => {
            let button = std::str::from_utf8(&data[2..]).unwrap_or_default();
            hid.write_web_button(button, data[1] != 0).await?;
        }
        Some(3) if data.len() == 5 => {
            hid.write_web_absolute(
                i16::from_be_bytes([data[1], data[2]]),
                i16::from_be_bytes([data[3], data[4]]),
            )
            .await?;
        }
        Some(4) if data.len() >= 4 => {
            for delta in data[2..].chunks_exact(2) {
                hid.write_web_relative(delta[0] as i8, delta[1] as i8)
                    .await?;
            }
        }
        Some(5) if data.len() >= 4 => {
            for delta in data[2..].chunks_exact(2) {
                hid.write_web_wheel(delta[1] as i8).await?;
            }
        }
        _ => {}
    }
    Ok(())
}

async fn atx_click(
    State(state): State<PikvmState>,
    headers: HeaderMap,
    Query(form): Query<AtxClickForm>,
) -> Response {
    if !authenticated(&state.auth, &headers).await {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some(atx) = &state.atx else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let button = match form.button.as_str() {
        "power" => AtxButton::Power,
        "power_long" => AtxButton::PowerLong,
        "reset" => AtxButton::Reset,
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };
    match atx.click(button).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
            StatusCode::CONFLICT.into_response()
        }
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response(),
    }
}

async fn msd_set_params(
    State(state): State<PikvmState>,
    headers: HeaderMap,
    Query(form): Query<MsdParams>,
) -> Response {
    let Some(msd) = authorized_msd(&state, &headers).await else {
        return msd_access_error(&state, &headers).await;
    };
    let result = if let Some(image) = form.image.as_deref() {
        msd.set_image(Some(image)).await
    } else if let Some(cdrom) = form.cdrom.as_deref() {
        match parse_flag(cdrom) {
            Ok(enabled) => msd.set_cdrom(enabled).await,
            Err(error) => Err(error),
        }
    } else if let Some(read_write) = form.rw.as_deref() {
        match parse_flag(read_write) {
            Ok(enabled) => msd.set_read_write(enabled).await,
            Err(error) => Err(error),
        }
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "no supported mass-storage parameter supplied",
        ))
    };
    operation_response(result)
}

async fn msd_set_connected(
    State(state): State<PikvmState>,
    headers: HeaderMap,
    Query(form): Query<MsdConnected>,
) -> Response {
    let Some(msd) = authorized_msd(&state, &headers).await else {
        return msd_access_error(&state, &headers).await;
    };
    let result = match parse_flag(&form.connected) {
        Ok(connected) => msd.set_connected(connected).await,
        Err(error) => Err(error),
    };
    operation_response(result)
}

async fn msd_reset(State(state): State<PikvmState>, headers: HeaderMap) -> Response {
    let Some(msd) = authorized_msd(&state, &headers).await else {
        return msd_access_error(&state, &headers).await;
    };
    operation_response(msd.reset().await)
}

async fn msd_remove(
    State(state): State<PikvmState>,
    headers: HeaderMap,
    Query(form): Query<MsdImage>,
) -> Response {
    let Some(msd) = authorized_msd(&state, &headers).await else {
        return msd_access_error(&state, &headers).await;
    };
    operation_response(msd.remove_image(&form.image).await)
}

async fn msd_read(
    State(state): State<PikvmState>,
    headers: HeaderMap,
    Query(query): Query<MsdImage>,
) -> Response {
    let Some(msd) = authorized_msd(&state, &headers).await else {
        return msd_access_error(&state, &headers).await;
    };
    let path = match msd.image_path(&query.image) {
        Ok(path) => path,
        Err(error) => return operation_response(Err(error)),
    };
    let file = match tokio::fs::File::open(path).await {
        Ok(file) => file,
        Err(error) => return operation_response(Err(error)),
    };
    let size = file.metadata().await.map(|meta| meta.len()).unwrap_or(0);
    let (sender, receiver) = tokio::sync::mpsc::channel::<Result<bytes::Bytes, io::Error>>(4);
    tokio::spawn(async move {
        let mut file = file;
        loop {
            let mut buffer = vec![0; 64 * 1024];
            match file.read(&mut buffer).await {
                Ok(0) => break,
                Ok(read) => {
                    buffer.truncate(read);
                    if sender.send(Ok(buffer.into())).await.is_err() {
                        break;
                    }
                }
                Err(error) => {
                    let _ = sender.send(Err(error)).await;
                    break;
                }
            }
        }
    });
    let mut response = Response::new(Body::from_stream(
        tokio_stream::wrappers::ReceiverStream::new(receiver),
    ));
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    response
        .headers_mut()
        .insert(CONTENT_DISPOSITION, HeaderValue::from_static("attachment"));
    if let Ok(length) = HeaderValue::from_str(&size.to_string()) {
        response.headers_mut().insert(CONTENT_LENGTH, length);
    }
    response
}

async fn msd_write(
    State(state): State<PikvmState>,
    headers: HeaderMap,
    Query(query): Query<MsdWrite>,
    body: Body,
) -> Response {
    let Some(msd) = authorized_msd(&state, &headers).await else {
        return msd_access_error(&state, &headers).await;
    };
    if query
        .prefix
        .as_deref()
        .is_some_and(|prefix| !prefix.is_empty())
    {
        return (
            StatusCode::BAD_REQUEST,
            "external storage prefixes are unsupported",
        )
            .into_response();
    }
    let expected_size = headers
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let path = match msd.image_path(&query.image) {
        Ok(path) => path,
        Err(error) => return operation_response(Err(error)),
    };
    if path.exists() {
        return (
            StatusCode::CONFLICT,
            "an image with this name already exists",
        )
            .into_response();
    }
    let temporary = match msd.temporary_image_path(&query.image) {
        Ok(path) => path,
        Err(error) => return operation_response(Err(error)),
    };
    if let Err(error) = msd.begin_upload(&query.image, expected_size).await {
        return operation_response(Err(error));
    }
    let result = async {
        let mut file = tokio::fs::File::create(&temporary).await?;
        let mut stream = body.into_data_stream();
        let mut written = 0_u64;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(io::Error::other)?;
            file.write_all(&chunk).await?;
            written = written.saturating_add(chunk.len() as u64);
            msd.upload_progress(written).await;
        }
        file.sync_all().await?;
        drop(file);
        tokio::fs::rename(&temporary, &path).await
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&temporary).await;
    }
    let finish = msd.finish_upload(&query.image, result.is_ok()).await;
    operation_response(result.and(finish))
}

async fn msd_write_remote(
    State(state): State<PikvmState>,
    headers: HeaderMap,
    Query(query): Query<MsdRemoteWrite>,
) -> Response {
    let Some(msd) = authorized_msd(&state, &headers).await else {
        return msd_access_error(&state, &headers).await;
    };
    if query
        .prefix
        .as_deref()
        .is_some_and(|prefix| !prefix.is_empty())
    {
        return (
            StatusCode::BAD_REQUEST,
            "external storage prefixes are unsupported",
        )
            .into_response();
    }
    let url = match reqwest::Url::parse(&query.url) {
        Ok(url) if matches!(url.scheme(), "http" | "https") => url,
        Ok(_) => {
            return (
                StatusCode::BAD_REQUEST,
                "only HTTP and HTTPS URLs are accepted",
            )
                .into_response();
        }
        Err(error) => return (StatusCode::BAD_REQUEST, error.to_string()).into_response(),
    };
    let name = match url
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .filter(|name| !name.is_empty())
    {
        Some(name) => name.to_owned(),
        None => return (StatusCode::BAD_REQUEST, "URL has no image filename").into_response(),
    };
    let path = match msd.image_path(&name) {
        Ok(path) => path,
        Err(error) => return operation_response(Err(error)),
    };
    if path.exists() {
        return (
            StatusCode::CONFLICT,
            "an image with this name already exists",
        )
            .into_response();
    }
    let temporary = match msd.temporary_image_path(&name) {
        Ok(path) => path,
        Err(error) => return operation_response(Err(error)),
    };
    let mut download = match reqwest::Client::new().get(url).send().await {
        Ok(response) if response.status().is_success() => response,
        Ok(response) => {
            return (
                StatusCode::BAD_GATEWAY,
                format!("remote server returned {}", response.status()),
            )
                .into_response();
        }
        Err(error) => return (StatusCode::BAD_GATEWAY, error.to_string()).into_response(),
    };
    let expected_size = download.content_length().unwrap_or(0);
    if let Err(error) = msd.begin_upload(&name, expected_size).await {
        return operation_response(Err(error));
    }
    let result = async {
        let mut file = tokio::fs::File::create(&temporary).await?;
        let mut written = 0_u64;
        while let Some(chunk) = download.chunk().await.map_err(io::Error::other)? {
            file.write_all(&chunk).await?;
            written = written.saturating_add(chunk.len() as u64);
            msd.upload_progress(written).await;
        }
        file.sync_all().await?;
        drop(file);
        tokio::fs::rename(&temporary, &path).await
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&temporary).await;
    }
    let finish = msd.finish_upload(&name, result.is_ok()).await;
    match result.and(finish) {
        Ok(()) => (StatusCode::OK, Json(json!({"ok": true, "image": name}))).into_response(),
        Err(error) => operation_response(Err(error)),
    }
}

async fn not_implemented(State(state): State<PikvmState>, headers: HeaderMap) -> Response {
    if authenticated(&state.auth, &headers).await {
        StatusCode::NOT_IMPLEMENTED.into_response()
    } else {
        StatusCode::UNAUTHORIZED.into_response()
    }
}

async fn authorized_msd<'a>(
    state: &'a PikvmState,
    headers: &HeaderMap,
) -> Option<&'a MassStorageController> {
    if authenticated(&state.auth, headers).await {
        state.mass_storage.as_ref()
    } else {
        None
    }
}

async fn msd_access_error(state: &PikvmState, headers: &HeaderMap) -> Response {
    if authenticated(&state.auth, headers).await {
        StatusCode::NOT_IMPLEMENTED.into_response()
    } else {
        StatusCode::UNAUTHORIZED.into_response()
    }
}

fn parse_flag(value: &str) -> io::Result<bool> {
    match value {
        "1" | "true" | "on" => Ok(true),
        "0" | "false" | "off" => Ok(false),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid boolean value",
        )),
    }
}

fn operation_response(result: io::Result<()>) -> Response {
    match result {
        Ok(()) => StatusCode::OK.into_response(),
        Err(error) => {
            let status = match error.kind() {
                io::ErrorKind::InvalidInput => StatusCode::BAD_REQUEST,
                io::ErrorKind::NotFound => StatusCode::NOT_FOUND,
                io::ErrorKind::AlreadyExists | io::ErrorKind::WouldBlock => StatusCode::CONFLICT,
                io::ErrorKind::PermissionDenied => StatusCode::FORBIDDEN,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            };
            (status, error.to_string()).into_response()
        }
    }
}

async fn hid_reset(State(state): State<PikvmState>, headers: HeaderMap) -> Response {
    if !authenticated(&state.auth, &headers).await {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    match state.hid.release_all().await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response(),
    }
}

async fn hid_print(
    State(state): State<PikvmState>,
    headers: HeaderMap,
    Query(query): Query<HidPrintParams>,
    body: String,
) -> Response {
    if !authenticated(&state.auth, &headers).await {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if query.keymap.as_deref().unwrap_or("en-us") != "en-us" {
        return (StatusCode::BAD_REQUEST, "unsupported keymap").into_response();
    }
    if body.len() > 64 * 1024 {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    }
    let delay = query.delay.unwrap_or(0.02);
    if !delay.is_finite() || !(0.0..=1.0).contains(&delay) {
        return (
            StatusCode::BAD_REQUEST,
            "delay must be between 0 and 1 second",
        )
            .into_response();
    }
    operation_response(
        state
            .hid
            .write_text(&body, Duration::from_secs_f64(delay))
            .await,
    )
}

async fn hid_set_params(
    State(state): State<PikvmState>,
    headers: HeaderMap,
    Query(form): Query<HidParams>,
) -> Response {
    if !authenticated(&state.auth, &headers).await {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some(value) = form.jiggler else {
        return (StatusCode::NOT_IMPLEMENTED, "unsupported HID parameter").into_response();
    };
    let enabled = match parse_flag(&value) {
        Ok(enabled) => enabled,
        Err(error) => return operation_response(Err(error)),
    };
    let was_enabled = state.jiggler.0.swap(enabled, Ordering::AcqRel);
    if enabled && !was_enabled {
        let active = state.jiggler.0.clone();
        let hid = state.hid.clone();
        tokio::spawn(async move {
            while active.load(Ordering::Acquire) {
                tokio::time::sleep(Duration::from_secs(15)).await;
                if !active.load(Ordering::Acquire) {
                    break;
                }
                if let Err(error) = hid.write_web_relative(1, 0).await {
                    eprintln!("HID jiggler stopped: {error}");
                    active.store(false, Ordering::Release);
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
                let _ = hid.write_web_relative(-1, 0).await;
            }
        });
    }
    StatusCode::OK.into_response()
}

async fn streamer_reset(State(state): State<PikvmState>, headers: HeaderMap) -> Response {
    if !authenticated(&state.auth, &headers).await {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    // The frontend closes the persistent HTTP response before this request.
    // Its replacement request creates a fresh capture/encoder session.
    StatusCode::OK.into_response()
}

async fn streamer_set_params(
    State(state): State<PikvmState>,
    headers: HeaderMap,
    Query(params): Query<StreamerParams>,
) -> Response {
    if !authenticated(&state.auth, &headers).await {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let mut settings = state.settings.get().await;
    if let Some(fps) = params.desired_fps {
        let fps = if fps == 0 {
            settings.advertised_fps
        } else {
            fps
        };
        if fps > settings.advertised_fps || settings.advertised_fps % fps != 0 {
            return (
                StatusCode::BAD_REQUEST,
                "FPS must be a non-zero divisor of the HDMI capture rate",
            )
                .into_response();
        }
        settings.frame_divider = settings.advertised_fps / fps;
    } else if let Some(resolution) = params.resolution {
        let current = format!("{}x{}", settings.capture_width, settings.capture_height);
        if resolution != current {
            return (
                StatusCode::NOT_IMPLEMENTED,
                "runtime scaling is unavailable",
            )
                .into_response();
        }
    } else if params.quality.is_some() {
        return (
            StatusCode::NOT_IMPLEMENTED,
            "quality-based rate control is unavailable",
        )
            .into_response();
    } else if let Some(bitrate) = params.h264_bitrate {
        if bitrate == 0 {
            return (StatusCode::BAD_REQUEST, "bitrate must be non-zero").into_response();
        }
        settings.bitrate_kbps = Some(bitrate);
    } else if let Some(gop) = params.h264_gop {
        if gop == 0 {
            return (StatusCode::BAD_REQUEST, "GOP must be non-zero").into_response();
        }
        settings.gop = gop;
    } else {
        return (
            StatusCode::BAD_REQUEST,
            "no supported streamer parameter supplied",
        )
            .into_response();
    }

    let runtime = match settings.runtime_config() {
        Ok(runtime) => runtime,
        Err(error) => return (StatusCode::BAD_REQUEST, error).into_response(),
    };
    match state.settings.save(settings).await {
        Ok(()) => {
            state.server.apply_runtime_config(runtime);
            state.server.disconnect_clients().await;
            StatusCode::OK.into_response()
        }
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response(),
    }
}

async fn authenticated(auth: &AuthState, headers: &HeaderMap) -> bool {
    match session_token(headers) {
        Some(token) => auth.validate_session(token).await,
        None => false,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn login(password: &str, otp: Option<String>) -> LoginForm {
        LoginForm {
            user: "root".to_owned(),
            passwd: password.to_owned(),
            otp,
            _expire: Some(0),
        }
    }

    #[tokio::test]
    async fn login_supports_password_and_oled_recovery() {
        let auth = AuthState::with_password("old-password");
        assert!(
            authenticate_login(&auth, &login("old-password", None))
                .await
                .is_ok()
        );

        let otp = auth.issue_otp().await;
        assert!(
            authenticate_login(&auth, &login("new-password", Some(otp)))
                .await
                .is_ok()
        );
        assert!(
            authenticate_login(&auth, &login("old-password", None))
                .await
                .is_err()
        );
        assert!(
            authenticate_login(&auth, &login("new-password", None))
                .await
                .is_ok()
        );
    }
}
