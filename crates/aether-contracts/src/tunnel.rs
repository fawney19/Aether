use std::io::Read;

use bytes::{Buf, BufMut, Bytes, BytesMut};
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;

pub const HEADER_SIZE: usize = 10;
pub const TUNNEL_RELAY_FORWARDED_BY_HEADER: &str = "x-aether-tunnel-forwarded-by";
pub const TUNNEL_RELAY_OWNER_INSTANCE_HEADER: &str = "x-aether-tunnel-owner-instance-id";

pub mod flags {
    pub const END_STREAM: u8 = 0x01;
    pub const GZIP_COMPRESSED: u8 = 0x02;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MsgType {
    RequestHeaders = 0x01,
    RequestBody = 0x02,
    ResponseHeaders = 0x03,
    ResponseBody = 0x04,
    StreamEnd = 0x05,
    StreamError = 0x06,
    Ping = 0x10,
    Pong = 0x11,
    GoAway = 0x12,
    HeartbeatData = 0x13,
    HeartbeatAck = 0x14,
}

impl MsgType {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            REQUEST_HEADERS => Some(Self::RequestHeaders),
            REQUEST_BODY => Some(Self::RequestBody),
            RESPONSE_HEADERS => Some(Self::ResponseHeaders),
            RESPONSE_BODY => Some(Self::ResponseBody),
            STREAM_END => Some(Self::StreamEnd),
            STREAM_ERROR => Some(Self::StreamError),
            PING => Some(Self::Ping),
            PONG => Some(Self::Pong),
            GOAWAY => Some(Self::GoAway),
            HEARTBEAT_DATA => Some(Self::HeartbeatData),
            HEARTBEAT_ACK => Some(Self::HeartbeatAck),
            _ => None,
        }
    }
}

pub const REQUEST_HEADERS: u8 = MsgType::RequestHeaders as u8;
pub const REQUEST_BODY: u8 = MsgType::RequestBody as u8;
pub const RESPONSE_HEADERS: u8 = MsgType::ResponseHeaders as u8;
pub const RESPONSE_BODY: u8 = MsgType::ResponseBody as u8;
pub const STREAM_END: u8 = MsgType::StreamEnd as u8;
pub const STREAM_ERROR: u8 = MsgType::StreamError as u8;
pub const PING: u8 = MsgType::Ping as u8;
pub const PONG: u8 = MsgType::Pong as u8;
pub const GOAWAY: u8 = MsgType::GoAway as u8;
pub const HEARTBEAT_DATA: u8 = MsgType::HeartbeatData as u8;
pub const HEARTBEAT_ACK: u8 = MsgType::HeartbeatAck as u8;
pub const FLAG_END_STREAM: u8 = flags::END_STREAM;
pub const FLAG_GZIP_COMPRESSED: u8 = flags::GZIP_COMPRESSED;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameHeader {
    pub stream_id: u32,
    pub msg_type: u8,
    pub flags: u8,
    pub payload_len: u32,
}

impl FrameHeader {
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

#[derive(Debug, Clone)]
pub struct Frame {
    pub stream_id: u32,
    pub msg_type: MsgType,
    pub flags: u8,
    pub payload: Bytes,
}

impl Frame {
    pub fn new(stream_id: u32, msg_type: MsgType, flags: u8, payload: impl Into<Bytes>) -> Self {
        Self {
            stream_id,
            msg_type,
            flags,
            payload: payload.into(),
        }
    }

    pub fn control(msg_type: MsgType, payload: impl Into<Bytes>) -> Self {
        Self::new(0, msg_type, 0, payload)
    }

    pub fn is_end_stream(&self) -> bool {
        self.flags & flags::END_STREAM != 0
    }

    pub fn is_gzip(&self) -> bool {
        self.flags & flags::GZIP_COMPRESSED != 0
    }

    pub fn encode(&self) -> Bytes {
        let mut buf = BytesMut::with_capacity(HEADER_SIZE + self.payload.len());
        buf.put_u32(self.stream_id);
        buf.put_u8(self.msg_type as u8);
        buf.put_u8(self.flags);
        buf.put_u32(self.payload.len() as u32);
        buf.put(self.payload.clone());
        buf.freeze()
    }

    pub fn decode(mut data: Bytes) -> Result<Self, ProtocolError> {
        if data.len() < HEADER_SIZE {
            return Err(ProtocolError::TooShort {
                expected: HEADER_SIZE,
                actual: data.len(),
            });
        }

        let stream_id = data.get_u32();
        let msg_type_raw = data.get_u8();
        let frame_flags = data.get_u8();
        let payload_len = data.get_u32() as usize;

        if data.remaining() < payload_len {
            return Err(ProtocolError::Incomplete {
                expected: HEADER_SIZE + payload_len,
                actual: HEADER_SIZE + data.remaining(),
            });
        }

        let msg_type =
            MsgType::from_u8(msg_type_raw).ok_or(ProtocolError::UnknownMsgType(msg_type_raw))?;
        let payload = data.split_to(payload_len);

        Ok(Self {
            stream_id,
            msg_type,
            flags: frame_flags,
            payload,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("frame too short: expected {expected} bytes, got {actual}")]
    TooShort { expected: usize, actual: usize },
    #[error("frame incomplete: expected {expected} bytes, got {actual}")]
    Incomplete { expected: usize, actual: usize },
    #[error("unknown message type: 0x{0:02x}")]
    UnknownMsgType(u8),
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RequestMeta {
    pub method: String,
    pub url: String,
    pub headers: std::collections::HashMap<String, String>,
    #[serde(default = "default_timeout", deserialize_with = "deserialize_timeout")]
    pub timeout: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follow_redirects: Option<bool>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub http1_only: bool,
}

fn default_timeout() -> u64 {
    60
}

fn is_false(value: &bool) -> bool {
    !*value
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

pub fn encode_stream_error(stream_id: u32, msg: &str) -> Vec<u8> {
    encode_frame(stream_id, STREAM_ERROR, 0, msg.as_bytes())
}

pub fn encode_ping() -> Vec<u8> {
    encode_frame(0, PING, 0, &[])
}

pub fn encode_pong(payload: &[u8]) -> Vec<u8> {
    encode_frame(0, PONG, 0, payload)
}

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
            .map_err(|err| format!("failed to decompress payload: {err}"))?;
        Ok(decoded)
    } else {
        Ok(payload.to_vec())
    }
}

pub fn decompress_if_gzip(frame: &Frame) -> Result<Bytes, std::io::Error> {
    if frame.is_gzip() {
        decompress_gzip(&frame.payload)
    } else {
        Ok(frame.payload.clone())
    }
}

pub fn compress_payload(data: Bytes) -> (Bytes, u8) {
    if data.len() >= COMPRESS_MIN_SIZE {
        if let Ok(compressed) = compress_gzip(&data) {
            if compressed.len() < data.len() {
                return (compressed, flags::GZIP_COMPRESSED);
            }
        }
    }
    (data, 0)
}

const COMPRESS_MIN_SIZE: usize = 512;

fn decompress_gzip(data: &[u8]) -> Result<Bytes, std::io::Error> {
    let mut decoder = GzDecoder::new(data);
    let mut buf = Vec::new();
    decoder.read_to_end(&mut buf)?;
    Ok(Bytes::from(buf))
}

fn compress_gzip(data: &[u8]) -> Result<Bytes, std::io::Error> {
    use std::io::Write;

    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(data)?;
    let compressed = encoder.finish()?;
    Ok(Bytes::from(compressed))
}

#[cfg(test)]
mod tests {
    use super::{encode_ping, Frame, FrameHeader, MsgType, RequestMeta, FLAG_GZIP_COMPRESSED};
    use bytes::Bytes;

    #[test]
    fn request_meta_accepts_integer_timeout() {
        let raw = br#"{"method":"GET","url":"https://example.com","headers":{},"timeout":15}"#;
        let meta: RequestMeta = serde_json::from_slice(raw).expect("parse request meta");
        assert_eq!(meta.timeout, 15);
    }

    #[test]
    fn request_meta_accepts_integer_like_float_timeout() {
        let raw = br#"{"method":"GET","url":"https://example.com","headers":{},"timeout":15.0}"#;
        let meta: RequestMeta = serde_json::from_slice(raw).expect("parse request meta");
        assert_eq!(meta.timeout, 15);
    }

    #[test]
    fn frame_round_trip_decodes_back_to_original_message() {
        let frame = Frame::new(7, MsgType::ResponseBody, 0, Bytes::from_static(b"hello"));
        let encoded = frame.encode();
        let decoded = Frame::decode(encoded).expect("frame should decode");
        assert_eq!(decoded.stream_id, 7);
        assert_eq!(decoded.msg_type, MsgType::ResponseBody);
        assert_eq!(decoded.payload, Bytes::from_static(b"hello"));
    }

    #[test]
    fn frame_header_parses_raw_ping_frame() {
        let encoded = encode_ping();
        let header = FrameHeader::parse(&encoded).expect("ping header should parse");
        assert_eq!(header.msg_type, MsgType::Ping as u8);
        assert_eq!(header.flags & FLAG_GZIP_COMPRESSED, 0);
    }
}
