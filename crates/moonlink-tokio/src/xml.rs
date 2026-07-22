use moonlink_core::{ServerConfig, ServerRuntimeConfig};
use moonlink_protocol::VideoCodec;

use crate::shared::StreamPorts;

pub(crate) fn write_server_info_xml(
    config: &ServerConfig,
    runtime_config: &ServerRuntimeConfig,
    ports: StreamPorts,
    paired: bool,
    server_unique_id: &str,
) -> String {
    let codec_modes = calculate_codec_mode_flags(runtime_config);
    let max_luma_pixels_hevc = calculate_max_luma_pixels_hevc(runtime_config);
    let pair_status = u8::from(paired);
    format!(
        concat!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>",
            "<root status_code=\"200\">",
            "<hostname>{}</hostname>",
            "<appversion>{}</appversion>",
            "<uniqueid>{}</uniqueid>",
            "<HttpsPort>{}</HttpsPort>",
            "<ExternalPort>{}</ExternalPort>",
            "<RTSPPort>{}</RTSPPort>",
            "<VideoPort>{}</VideoPort>",
            "<AudioPort>{}</AudioPort>",
            "<ControlPort>{}</ControlPort>",
            "<ServerCodecModeSupport>{}</ServerCodecModeSupport>",
            "<MaxLumaPixelsHEVC>{}</MaxLumaPixelsHEVC>",
            "<PairStatus>{}</PairStatus>",
            "<currentgame>0</currentgame>",
            "<state>Moonlight Compatible</state>",
            "</root>"
        ),
        escape_xml(&runtime_config.name),
        escape_xml(&config.app_version),
        escape_xml(server_unique_id),
        ports.https,
        ports.https,
        ports.rtsp,
        ports.video,
        ports.audio,
        ports.control,
        codec_modes,
        max_luma_pixels_hevc,
        pair_status
    )
}

pub(crate) fn write_app_list_xml(config: &ServerConfig) -> String {
    let apps = config
        .apps
        .iter()
        .map(|app| {
            format!(
                "<App><ID>{}</ID><AppTitle>{}</AppTitle><IsHdrSupported>0</IsHdrSupported></App>",
                app.id,
                escape_xml(&app.name)
            )
        })
        .collect::<String>();
    format!("<?xml version=\"1.0\" encoding=\"utf-8\"?><root status_code=\"200\">{apps}</root>")
}

pub(crate) fn write_launch_xml() -> String {
    "<?xml version=\"1.0\" encoding=\"utf-8\"?><root status_code=\"200\"><gamesession>DEADBEEFCAFE</gamesession></root>".to_owned()
}

pub(crate) fn read_query_value(path: &str, key: &str) -> Option<String> {
    let query = path.split_once('?')?.1;
    for pair in query.split('&') {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        if name == key {
            return Some(value.to_owned());
        }
    }
    None
}

pub(crate) fn escape_xml(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn calculate_codec_mode_flags(config: &ServerRuntimeConfig) -> u32 {
    let mut flags = 0;
    if config.capabilities.video.contains(&VideoCodec::H264) {
        flags |= 0x1;
    }
    if config.capabilities.video.contains(&VideoCodec::Hevc) {
        flags |= 0x100;
    }
    flags
}

fn calculate_max_luma_pixels_hevc(config: &ServerRuntimeConfig) -> u32 {
    if config.capabilities.video.contains(&VideoCodec::Hevc) {
        config.video_stream.max_luma_pixels()
    } else {
        0
    }
}
