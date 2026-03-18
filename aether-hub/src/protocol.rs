/// Tunnel binary frame protocol
///
/// Frame format (10-byte header + payload):
/// | stream_id (4B) | msg_type (1B) | flags (1B) | payload_len (4B) | payload (NB) |
use std::io::Read;

use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;

pub const HEADER_SIZE: usize = 10;

// Message types
pub const REQUEST_HEADERS: u8 = 0x01;
pub const REQUEST_BODY: u8 = 0x02;
pub const RESPONSE_HEADERS: u8 = 0x03;
pub const RESPONSE_BODY: u8 = 0x04;
pub const STREAM_END: u8 = 0x05;
pub const STREAM_ERROR: u8 = 0x06;
pub const PING: u8 = 0x10;
pub const PONG: u8 = 0x11;
pub const GOAWAY: u8 = 0x12;
pub const HEARTBEAT_DATA: u8 = 0x13;
pub const HEARTBEAT_ACK: u8 = 0x14;
// Flags
pub const FLAG_END_STREAM: u8 = 0x01;
pub const FLAG_GZIP_COMPRESSED: u8 = 0x02;

#[derive(Debug, Clone, Copy)]
pub struct FrameHeader {
    pub stream_id: u32,
    pub msg_type: u8,
    pub flags: u8,
    pub payload_len: u32,
}

impl FrameHeader {
    /// Parse frame header from raw bytes (must be >= HEADER_SIZE)
    #[inline]
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < HEADER_SIZE {
            return None;
        }
        Some(Self {
            stream_id: u32::from_be_bytes([data[0], data[1], data[2], data[3]]),
            msg_type: data[4],
            flags: data[5],
            payload_len: u32::from_be_bytes([data[6], data[7], data[8], data[9]]),
        })
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RequestMeta {
    pub method: String,
    pub url: String,
    pub headers: std::collections::HashMap<String, String>,
    #[serde(default = "default_timeout", deserialize_with = "deserialize_timeout")]
    pub timeout: u64,
}

fn default_timeout() -> u64 {
    60
}

fn deserialize_timeout<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum TimeoutValue {
        Int(u64),
        Float(f64),
    }

    match <TimeoutValue as serde::Deserialize>::deserialize(deserializer)? {
        TimeoutValue::Int(v) => Ok(v),
        TimeoutValue::Float(v) => {
            if !v.is_finite() || v < 0.0 {
                return Err(serde::de::Error::custom(
                    "timeout must be a non-negative finite number",
                ));
            }
            if v.fract() != 0.0 {
                return Err(serde::de::Error::custom("timeout must be integer seconds"));
            }
            if v > (u64::MAX as f64) {
                return Err(serde::de::Error::custom("timeout is too large"));
            }
            Ok(v as u64)
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ResponseMeta {
    pub status: u16,
    pub headers: Vec<(String, String)>,
}

pub fn encode_frame(stream_id: u32, msg_type: u8, flags: u8, payload: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(HEADER_SIZE + payload.len());
    buf.extend_from_slice(&stream_id.to_be_bytes());
    buf.push(msg_type);
    buf.push(flags);
    buf.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    buf.extend_from_slice(payload);
    buf
}

/// Encode a STREAM_ERROR frame for a given stream_id with an error message
pub fn encode_stream_error(stream_id: u32, msg: &str) -> Vec<u8> {
    encode_frame(stream_id, STREAM_ERROR, 0, msg.as_bytes())
}

/// Encode a PING frame (stream_id=0)
pub fn encode_ping() -> Vec<u8> {
    encode_frame(0, PING, 0, &[])
}

/// Encode a PONG frame (stream_id=0, echo payload)
pub fn encode_pong(payload: &[u8]) -> Vec<u8> {
    encode_frame(0, PONG, 0, payload)
}

/// Encode a GOAWAY frame (stream_id=0)
pub fn encode_goaway() -> Vec<u8> {
    encode_frame(0, GOAWAY, 0, &[])
}

#[inline]
pub fn frame_payload_by_header<'a>(data: &'a [u8], header: &FrameHeader) -> Option<&'a [u8]> {
    let payload_len = header.payload_len as usize;
    let end = HEADER_SIZE.checked_add(payload_len)?;
    if data.len() < end {
        return None;
    }
    Some(&data[HEADER_SIZE..end])
}

pub fn decode_payload(data: &[u8], header: &FrameHeader) -> Result<Vec<u8>, String> {
    let payload = frame_payload_by_header(data, header)
        .ok_or_else(|| "incomplete frame payload".to_string())?;
    if header.flags & FLAG_GZIP_COMPRESSED != 0 {
        let mut decoder = GzDecoder::new(payload);
        let mut decoded = Vec::new();
        decoder
            .read_to_end(&mut decoded)
            .map_err(|e| format!("failed to decompress payload: {e}"))?;
        Ok(decoded)
    } else {
        Ok(payload.to_vec())
    }
}

pub fn compress_payload(payload: &[u8]) -> Result<(Vec<u8>, u8), std::io::Error> {
    maybe_recompress_payload(payload, true)
}

fn maybe_recompress_payload(
    payload: &[u8],
    prefer_gzip: bool,
) -> Result<(Vec<u8>, u8), std::io::Error> {
    if !prefer_gzip {
        return Ok((payload.to_vec(), 0));
    }
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    std::io::Write::write_all(&mut encoder, payload)?;
    let compressed = encoder.finish()?;
    if compressed.len() < payload.len() {
        Ok((compressed, FLAG_GZIP_COMPRESSED))
    } else {
        Ok((payload.to_vec(), 0))
    }
}
