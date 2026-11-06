mod alsa_pcm;
mod atx;
mod audio;
mod auth;
mod browser_stream;
mod cli;
mod display;
mod external;
mod gadget;
mod hevc;
mod hid;
mod hid_report;
mod media;
mod msd;
mod pairing_manager;
mod pikvm;
mod session_arbiter;
mod settings;
mod usb_status;
mod v4l2_provider;
mod web;

use clap::Parser;
use moonlink_core::{PairingPin, ServerConfig};
use moonlink_tokio::{FilePairingStore, ServerBuilder};

use crate::cli::{Args, SourceKind};
use crate::gadget::{GadgetConfig, setup_usb_gadget};
use crate::hid::{InputDevicePaths, UsbGadgetInputSink};
use crate::media::HostMediaProvider;
use crate::pairing_manager::PairingManager;
use crate::session_arbiter::SessionArbiter;
use crate::settings::{Settings, SettingsStore};
use crate::usb_status::{monitor_usb_attachment, primary_ipv4, udc_state_path};
use crate::web::serve_web;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = Args::parse();
    let settings_store =
        SettingsStore::open(args.settings_store.clone(), Settings::from_args(&args))?;
    let running_settings = settings_store.get().await;
    running_settings.apply_device_args(&mut args);
    running_settings.moonlight.apply_args(&mut args);
    let auth = auth::AuthState::open();
    let pairing_store = FilePairingStore::open(args.pairing_store.clone()).await?;
    let pairing_manager = PairingManager::default();
    let ip_address = primary_ipv4()?.to_string();

    let usb_gadget = if args.setup_usb_gadget {
        Some(setup_usb_gadget(&GadgetConfig::from_args(&args))?)
    } else {
        None
    };
    let input_paths = usb_gadget
        .as_ref()
        .map(|gadget| InputDevicePaths {
            keyboard: Some(gadget.hid.keyboard.clone()),
            relative_mouse: Some(gadget.hid.relative_mouse.clone()),
            absolute_mouse: Some(gadget.hid.absolute_mouse.clone()),
        })
        .or_else(|| args.input_device_paths())
        .unwrap_or_default();
    let input_sink = UsbGadgetInputSink::open(input_paths, args.trace_pointer_input).await?;
    let atx = match atx::AtxController::open() {
        Ok(controller) => Some(controller),
        Err(error) => {
            eprintln!("ATX GPIO unavailable: {error}");
            None
        }
    };

    let source_kind = args.source_kind()?;
    let mut web_args = args.clone();
    running_settings.web.apply_args(&mut web_args);
    let web_source_kind = web_args.source_kind()?;
    let audio = args.audio_settings();
    let web_audio = web_args.audio_settings();
    let runtime_config = running_settings.moonlight_runtime_config()?;
    let arbiter = SessionArbiter::default();

    let config = ServerConfig::new(
        args.name.clone(),
        args.bind_http,
        args.bind_http,
        args.bind_rtsp,
        runtime_config.capabilities,
    )
    .with_https_bind(args.bind_https)
    .with_stream_ports(args.bind_video, args.bind_audio, args.bind_control)
    .with_video_stream(runtime_config.video_stream)
    .with_encryption_mode(runtime_config.encryption_mode);

    let mut config = config;
    config.pairing_pin = args
        .pin
        .clone()
        .map(PairingPin::static_pin)
        .unwrap_or_else(|| PairingPin::provider(pairing_manager.clone()));

    let web_v4l2 = match web_source_kind {
        SourceKind::V4l2(settings) => Some(settings),
        SourceKind::Command(_) => None,
    };
    let provider = match source_kind {
        SourceKind::Command(command) => {
            HostMediaProvider::command(command, audio, settings_store.clone(), arbiter.clone())
        }
        SourceKind::V4l2(settings) => {
            HostMediaProvider::v4l2(settings, audio, settings_store.clone(), arbiter.clone())
        }
    };

    let (display_tx, display_rx) = tokio::sync::watch::channel(None);
    let display_device = args.display_device.clone();
    let display_transform = args.display_transform;
    let display_task = tokio::spawn(async move {
        if let Err(err) = display::run_display(&display_device, display_transform, display_rx).await
        {
            eprintln!("OLED display stopped: {err}");
        }
    });
    let usb_task = if args.setup_usb_gadget {
        let state_path = udc_state_path(args.udc.as_deref())?;
        let usb_ip_address = ip_address.clone();
        let usb_auth = auth.clone();
        Some(tokio::spawn(async move {
            if let Err(err) =
                monitor_usb_attachment(state_path, usb_ip_address, usb_auth, display_tx).await
            {
                eprintln!("USB attachment monitor stopped: {err}");
            }
        }))
    } else {
        None
    };

    let handle = ServerBuilder::new(config)
        .media_provider(provider)
        .input_sink(input_sink.clone())
        .pairing_store(pairing_store.clone())
        .serve()
        .await?;
    let web_task = tokio::spawn(serve_web(
        args.bind_web,
        auth.clone(),
        settings_store,
        pairing_manager,
        pairing_store.clone(),
        handle.controller(),
        arbiter,
        web_v4l2,
        web_audio,
        input_sink,
        atx,
        usb_gadget
            .as_ref()
            .map(|gadget| gadget.mass_storage.clone()),
    ));

    println!(
        "nanokvm host listening: http={}, https={}, discovery={}, rtsp={}",
        handle.http_addr(),
        handle.https_addr(),
        handle.discovery_addr(),
        handle.rtsp_addr()
    );
    tokio::select! {
        result = tokio::signal::ctrl_c() => result?,
        result = web_task => {
            result.map_err(std::io::Error::other)??;
        }
    }
    handle.shutdown().await?;
    display_task.abort();
    if let Some(usb_task) = usb_task {
        usb_task.abort();
    }
    Ok(())
}
