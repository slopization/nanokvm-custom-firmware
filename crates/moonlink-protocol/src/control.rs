pub const ENCRYPTED_CONTROL_PACKET_TYPE: u16 = 0x0001;
pub const INPUT_DATA_GEN5_PACKET_TYPE: u16 = 0x0207;
pub const INPUT_DATA_GEN7_PACKET_TYPE: u16 = 0x0206;
pub const START_ACK_PACKET_TYPE: u16 = 0x0013;

pub const FEATURE_FLAG_AUDIO_ENCRYPTION: u32 = 0x20;
pub const STREAM_ENCRYPTION_CONTROL_V2: u32 = 0x01;
pub const STREAM_ENCRYPTION_VIDEO: u32 = 0x02;
pub const STREAM_ENCRYPTION_AUDIO: u32 = 0x04;
pub const STREAM_ENCRYPTION_MEDIA: u32 = STREAM_ENCRYPTION_VIDEO | STREAM_ENCRYPTION_AUDIO;
pub const STREAM_ENCRYPTION_ALL: u32 = STREAM_ENCRYPTION_CONTROL_V2 | STREAM_ENCRYPTION_MEDIA;

pub const REQUEST_IDR_PACKET_TYPES: [u16; 3] = [0x1407, 0x0606, 0x0302];
pub const INVALIDATE_REFERENCE_PACKET_TYPES: [u16; 3] = [0x1404, 0x0604, 0x0301];

pub fn is_input_data_packet_type(packet_type: u16) -> bool {
    matches!(
        packet_type,
        INPUT_DATA_GEN5_PACKET_TYPE | INPUT_DATA_GEN7_PACKET_TYPE
    )
}

pub fn is_request_idr_packet_type(packet_type: u16) -> bool {
    REQUEST_IDR_PACKET_TYPES.contains(&packet_type)
}

pub fn is_invalidate_reference_packet_type(packet_type: u16) -> bool {
    INVALIDATE_REFERENCE_PACKET_TYPES.contains(&packet_type)
}
