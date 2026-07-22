use bytes::{Bytes, BytesMut};
use oxideav_bitstream::hevc::{
    NAL_TYPE_AUD, NAL_TYPE_CRA, NAL_TYPE_IDR_N_LP, NAL_TYPE_IDR_W_RADL, NAL_TYPE_PPS, NAL_TYPE_SPS,
    NAL_TYPE_VPS, is_irap, nal_header, split_annex_b,
};

#[derive(Debug, Default, Clone)]
pub struct ParameterSets {
    vps: Option<Bytes>,
    sps: Option<Bytes>,
    pps: Option<Bytes>,
}

impl ParameterSets {
    pub fn update(&mut self, bytes: &[u8]) {
        for_each_annex_b_nal(bytes, |nal_type, nal| match nal_type {
            NAL_TYPE_VPS => self.vps = Some(Bytes::copy_from_slice(nal)),
            NAL_TYPE_SPS => self.sps = Some(Bytes::copy_from_slice(nal)),
            NAL_TYPE_PPS => self.pps = Some(Bytes::copy_from_slice(nal)),
            _ => {}
        });
    }

    pub fn prepend_if_missing(&self, bytes: Bytes) -> Bytes {
        if has_parameter_sets(&bytes) || !self.has_all() {
            return bytes;
        }

        let mut out = BytesMut::with_capacity(
            self.vps.as_ref().map_or(0, Bytes::len)
                + self.sps.as_ref().map_or(0, Bytes::len)
                + self.pps.as_ref().map_or(0, Bytes::len)
                + bytes.len(),
        );
        out.extend_from_slice(self.vps.as_ref().expect("checked by has_all"));
        out.extend_from_slice(self.sps.as_ref().expect("checked by has_all"));
        out.extend_from_slice(self.pps.as_ref().expect("checked by has_all"));
        out.extend_from_slice(&bytes);
        out.freeze()
    }

    fn has_all(&self) -> bool {
        self.vps.is_some() && self.sps.is_some() && self.pps.is_some()
    }
}

pub fn take_access_unit(buffer: &mut BytesMut) -> Option<(Bytes, bool)> {
    let starts = find_start_codes(buffer);
    if starts.len() < 3 {
        return None;
    }

    let mut seen_payload = false;
    for window in starts.windows(2).skip(1) {
        let start_code = window[0].0;
        let nal_start = window[0].1;
        let nal_end = window[1].0;
        let Some(nal_type) = nal_type(&buffer[nal_start..nal_end]) else {
            continue;
        };
        if nal_type == NAL_TYPE_AUD && seen_payload {
            let bytes = filter_access_unit(buffer.split_to(start_code).freeze());
            let marker = has_keyframe(&bytes);
            return Some((bytes, marker));
        }
        seen_payload |= nal_type <= 31;
    }

    None
}

pub fn finish_access_unit(buffer: &mut BytesMut) -> Option<(Bytes, bool)> {
    if buffer.is_empty() {
        return None;
    }
    let bytes = buffer.split().freeze();
    let marker = has_keyframe(&bytes);
    Some((filter_access_unit(bytes), marker))
}

pub fn has_keyframe(bytes: &[u8]) -> bool {
    split_annex_b(bytes)
        .into_iter()
        .filter_map(nal_type)
        .any(is_irap)
}

pub fn describe_access_unit(bytes: &[u8]) -> String {
    let mut out = Vec::new();
    for nal_type in split_annex_b(bytes)
        .into_iter()
        .filter_map(nal_type)
        .take(16)
    {
        out.push(nal_type_name(nal_type).to_owned());
        if out.len() >= 16 {
            out.push("...".to_owned());
        }
    }
    out.join(",")
}

fn has_parameter_sets(bytes: &[u8]) -> bool {
    let mut has_vps = false;
    let mut has_sps = false;
    let mut has_pps = false;
    for nal_type in split_annex_b(bytes).into_iter().filter_map(nal_type) {
        match nal_type {
            NAL_TYPE_VPS => has_vps = true,
            NAL_TYPE_SPS => has_sps = true,
            NAL_TYPE_PPS => has_pps = true,
            _ => {}
        }
    }
    has_vps && has_sps && has_pps
}

pub fn filter_access_unit(bytes: Bytes) -> Bytes {
    let starts = find_start_codes(&bytes);
    if starts.is_empty() {
        return bytes;
    }

    let mut out = BytesMut::with_capacity(bytes.len());
    for (index, (start_code, nal_start)) in starts.iter().copied().enumerate() {
        let nal_end = starts
            .get(index + 1)
            .map(|(next_start, _)| *next_start)
            .unwrap_or(bytes.len());
        let nal_type = nal_type(&bytes[nal_start..nal_end]).unwrap_or(64);
        if matches!(nal_type, NAL_TYPE_AUD | 38 | 39 | 40) {
            continue;
        }
        out.extend_from_slice(&bytes[start_code..nal_end]);
    }
    out.freeze()
}

fn nal_type(nal: &[u8]) -> Option<u8> {
    if nal.len() < 2 {
        return None;
    }
    let (forbidden, nal_type, _, temporal_id_plus1) = nal_header(nal[0], nal[1]);
    (forbidden == 0 && temporal_id_plus1 != 0).then_some(nal_type)
}

fn nal_type_name(nal_type: u8) -> &'static str {
    match nal_type {
        NAL_TYPE_IDR_W_RADL => "IDR_W_RADL(19)",
        NAL_TYPE_IDR_N_LP => "IDR_N_LP(20)",
        NAL_TYPE_CRA => "CRA(21)",
        NAL_TYPE_VPS => "VPS(32)",
        NAL_TYPE_SPS => "SPS(33)",
        NAL_TYPE_PPS => "PPS(34)",
        NAL_TYPE_AUD => "AUD(35)",
        39 => "PREFIX_SEI(39)",
        40 => "SUFFIX_SEI(40)",
        0..=31 => "VCL",
        _ => "NON_VCL",
    }
}

fn find_start_codes(bytes: &[u8]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 4 <= bytes.len() {
        if bytes[i..].starts_with(&[0, 0, 1]) {
            out.push((i, i + 3));
            i += 3;
        } else if bytes[i..].starts_with(&[0, 0, 0, 1]) {
            out.push((i, i + 4));
            i += 4;
        } else {
            i += 1;
        }
    }
    out
}

fn for_each_annex_b_nal(bytes: &[u8], mut f: impl FnMut(u8, &[u8])) {
    let starts = find_start_codes(bytes);
    for (index, (start_code, nal_start)) in starts.iter().copied().enumerate() {
        let nal_end = starts
            .get(index + 1)
            .map(|(next_start, _)| *next_start)
            .unwrap_or(bytes.len());
        let nal = &bytes[start_code..nal_end];
        let Some(nal_type) = nal_type(&bytes[nal_start..nal_end]) else {
            continue;
        };
        f(nal_type, nal);
        if index >= 15 {
            break;
        }
    }
}
