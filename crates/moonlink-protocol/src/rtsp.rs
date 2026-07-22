use std::collections::BTreeMap;

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtspRequest {
    pub method: String,
    pub target: String,
    pub version: String,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

impl RtspRequest {
    pub fn cseq(&self) -> Option<&str> {
        self.headers.get("cseq").map(String::as_str)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtspResponse {
    pub status: u16,
    pub reason: String,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

impl RtspResponse {
    pub fn ok(cseq: Option<&str>) -> Self {
        let mut headers = BTreeMap::new();
        if let Some(cseq) = cseq {
            headers.insert("CSeq".to_owned(), cseq.to_owned());
        }
        Self {
            status: 200,
            reason: "OK".to_owned(),
            headers,
            body: Vec::new(),
        }
    }

    pub fn serialize(&self) -> Vec<u8> {
        let mut out = format!("RTSP/1.0 {} {}\r\n", self.status, self.reason);
        for (name, value) in &self.headers {
            out.push_str(name);
            out.push_str(": ");
            out.push_str(value);
            out.push_str("\r\n");
        }
        if !self.body.is_empty() {
            out.push_str("Content-Length: ");
            out.push_str(&self.body.len().to_string());
            out.push_str("\r\n");
        }
        out.push_str("\r\n");
        let mut bytes = out.into_bytes();
        bytes.extend_from_slice(&self.body);
        bytes
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum RtspParseError {
    #[error("RTSP message is incomplete")]
    Incomplete,
    #[error("RTSP request line is malformed")]
    MalformedRequestLine,
    #[error("RTSP header is malformed")]
    MalformedHeader,
    #[error("content-length is invalid")]
    InvalidContentLength,
}

pub fn parse_rtsp_request(buffer: &[u8]) -> Result<Option<(RtspRequest, usize)>, RtspParseError> {
    let Some(header_end) = find_header_end(buffer) else {
        return Ok(None);
    };

    let header =
        std::str::from_utf8(&buffer[..header_end]).map_err(|_| RtspParseError::MalformedHeader)?;
    let mut lines = header.split("\r\n");
    let request_line = lines.next().ok_or(RtspParseError::MalformedRequestLine)?;
    let mut parts = request_line.split_whitespace();
    let method = parts
        .next()
        .ok_or(RtspParseError::MalformedRequestLine)?
        .to_owned();
    let target = parts
        .next()
        .ok_or(RtspParseError::MalformedRequestLine)?
        .to_owned();
    let version = parts
        .next()
        .ok_or(RtspParseError::MalformedRequestLine)?
        .to_owned();
    if parts.next().is_some() || version != "RTSP/1.0" {
        return Err(RtspParseError::MalformedRequestLine);
    }

    let mut headers = BTreeMap::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let (name, value) = line
            .split_once(':')
            .ok_or(RtspParseError::MalformedHeader)?;
        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
    }

    let body_len = match headers.get("content-length") {
        Some(value) => value
            .parse::<usize>()
            .map_err(|_| RtspParseError::InvalidContentLength)?,
        None => 0,
    };
    let body_start = header_end + 4;
    let total = body_start + body_len;
    if buffer.len() < total {
        return Ok(None);
    }

    Ok(Some((
        RtspRequest {
            method,
            target,
            version,
            headers,
            body: buffer[body_start..total].to_vec(),
        },
        total,
    )))
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rtsp_request_with_body() {
        let raw = b"ANNOUNCE streamid=video RTSP/1.0\r\nCSeq: 5\r\nContent-Length: 4\r\n\r\ntest";
        let (req, used) = parse_rtsp_request(raw).unwrap().unwrap();
        assert_eq!(used, raw.len());
        assert_eq!(req.method, "ANNOUNCE");
        assert_eq!(req.cseq(), Some("5"));
        assert_eq!(req.body, b"test");
    }
}
