use std::io::Read;

use bytes::{Buf, BufMut, Bytes, BytesMut};
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;

pub const HEADER_SIZE: usize = 10;
pub const TUNNEL_RELAY_FORWARDED_BY_HEADER: &str = "x-aether-tunnel-forwarded-by";
pub const TUNNEL_RELAY_OWNER_INSTANCE_HEADER: &str = "x-aether-tunnel-owner-instance-id";
pub const TUNNEL_PROTOCOL_VERSION_HEADER: &str = "x-aether-tunnel-protocol-version";
pub const TUNNEL_NODE_NAME_B64_HEADER: &str = "x-aether-tunnel-node-name-b64";
pub const CURRENT_TUNNEL_PROTOCOL_VERSION: u8 = 3;
pub const CURRENT_TUNNEL_PROTOCOL_VERSION_STR: &str = "3";
pub const MAX_TUNNEL_RELAY_META_LEN: usize = 256 * 1024;

pub mod flags {
    pub const END_STREAM: u8 = 0x01;
    pub const GZIP_COMPRESSED: u8 = 0x02;
    pub const ENCRYPTED: u8 = crate::tunnel_security::FLAG_ENCRYPTED;
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
    Hello = 0x15,
    Settings = 0x16,
    WindowUpdate = 0x17,
    ResetStream = 0x18,
    ConnectionClose = 0x19,
    LoadReport = 0x1a,
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
            HELLO => Some(Self::Hello),
            SETTINGS => Some(Self::Settings),
            WINDOW_UPDATE => Some(Self::WindowUpdate),
            RESET_STREAM => Some(Self::ResetStream),
            CONNECTION_CLOSE => Some(Self::ConnectionClose),
            LOAD_REPORT => Some(Self::LoadReport),
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
pub const HELLO: u8 = MsgType::Hello as u8;
pub const SETTINGS: u8 = MsgType::Settings as u8;
pub const WINDOW_UPDATE: u8 = MsgType::WindowUpdate as u8;
pub const RESET_STREAM: u8 = MsgType::ResetStream as u8;
pub const CONNECTION_CLOSE: u8 = MsgType::ConnectionClose as u8;
pub const LOAD_REPORT: u8 = MsgType::LoadReport as u8;
pub const FLAG_END_STREAM: u8 = flags::END_STREAM;
pub const FLAG_GZIP_COMPRESSED: u8 = flags::GZIP_COMPRESSED;
pub const FLAG_ENCRYPTED: u8 = flags::ENCRYPTED;

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_id: Option<String>,
    pub method: String,
    pub url: String,
    pub headers: std::collections::HashMap<String, String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub stream: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_first_byte_timeout_ms: Option<u64>,
    #[serde(default = "default_timeout", deserialize_with = "deserialize_timeout")]
    pub timeout: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follow_redirects: Option<bool>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub http1_only: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport_profile: Option<crate::ResolvedTransportProfile>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedTunnelRequestTimeouts {
    pub first_byte_ms: u64,
    pub response_body_ms: Option<u64>,
}

pub fn resolve_tunnel_request_timeouts(meta: &RequestMeta) -> ResolvedTunnelRequestTimeouts {
    let legacy_timeout_ms = meta.timeout.saturating_mul(1_000);
    let first_byte_ms = if meta.stream {
        meta.stream_first_byte_timeout_ms
            .unwrap_or(legacy_timeout_ms)
    } else {
        meta.request_timeout_ms
            .or(meta.stream_first_byte_timeout_ms)
            .unwrap_or(legacy_timeout_ms)
    };
    let response_body_ms = (!meta.stream).then_some(first_byte_ms);

    ResolvedTunnelRequestTimeouts {
        first_byte_ms: if meta.stream {
            clamp_stream_first_byte_timeout_ms(first_byte_ms)
        } else {
            clamp_upstream_request_timeout_ms(first_byte_ms)
        },
        response_body_ms: response_body_ms.map(clamp_upstream_request_timeout_ms),
    }
}

pub fn try_decode_tunnel_relay_request_meta(
    buffer: &[u8],
) -> Result<Option<(RequestMeta, usize)>, String> {
    if buffer.len() < 4 {
        return Ok(None);
    }
    let meta_len = u32::from_be_bytes([buffer[0], buffer[1], buffer[2], buffer[3]]) as usize;
    if meta_len > MAX_TUNNEL_RELAY_META_LEN {
        return Err("relay metadata too large".to_string());
    }
    let meta_end = 4usize
        .checked_add(meta_len)
        .ok_or_else(|| "relay envelope length overflow".to_string())?;
    if buffer.len() < meta_end {
        return Ok(None);
    }
    let meta = serde_json::from_slice::<RequestMeta>(&buffer[4..meta_end])
        .map_err(|error| format!("invalid relay metadata: {error}"))?;
    Ok(Some((meta, meta_end)))
}

fn clamp_upstream_request_timeout_ms(timeout_ms: u64) -> u64 {
    timeout_ms.clamp(1, crate::MAX_EXECUTION_REQUEST_TIMEOUT_MS)
}

fn clamp_stream_first_byte_timeout_ms(timeout_ms: u64) -> u64 {
    timeout_ms.clamp(1, crate::MAX_EXECUTION_STREAM_FIRST_BYTE_TIMEOUT_MS)
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

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HelloPayload {
    pub protocol_version: u8,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replica_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SettingsPayload {
    pub initial_stream_window_bytes: u32,
    pub min_window_update_bytes: u32,
    pub drain_deadline_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WindowUpdatePayload {
    pub delta_bytes: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ResetStreamPayload {
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GoAwayPayload {
    pub last_accepted_stream_id: u32,
    pub drain_deadline_ms: u64,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ConnectionClosePayload {
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LoadReportPayload {
    pub active_streams: u32,
    pub queue_depth: u32,
    pub queue_capacity: u32,
    pub health_score: u8,
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

pub fn encode_reset_stream(stream_id: u32, reason: &str) -> Vec<u8> {
    let payload = serde_json::to_vec(&ResetStreamPayload {
        reason: reason.to_string(),
    })
    .expect("reset stream payload should serialize");
    encode_frame(stream_id, RESET_STREAM, 0, &payload)
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

pub fn encode_goaway_v3(
    last_accepted_stream_id: u32,
    drain_deadline_ms: u64,
    reason: &str,
) -> Vec<u8> {
    let payload = serde_json::to_vec(&GoAwayPayload {
        last_accepted_stream_id,
        drain_deadline_ms,
        reason: reason.to_string(),
    })
    .expect("goaway payload should serialize");
    encode_frame(0, GOAWAY, 0, &payload)
}

pub fn encode_hello(payload: &HelloPayload) -> Vec<u8> {
    encode_json_control(HELLO, payload)
}

pub fn encode_settings(payload: &SettingsPayload) -> Vec<u8> {
    encode_json_control(SETTINGS, payload)
}

pub fn encode_window_update(stream_id: u32, delta_bytes: u32) -> Vec<u8> {
    let payload = serde_json::to_vec(&WindowUpdatePayload { delta_bytes })
        .expect("window update payload should serialize");
    encode_frame(stream_id, WINDOW_UPDATE, 0, &payload)
}

pub fn encode_connection_close(reason: &str) -> Vec<u8> {
    let payload = serde_json::to_vec(&ConnectionClosePayload {
        reason: reason.to_string(),
    })
    .expect("connection close payload should serialize");
    encode_frame(0, CONNECTION_CLOSE, 0, &payload)
}

pub fn encode_load_report(payload: &LoadReportPayload) -> Vec<u8> {
    encode_json_control(LOAD_REPORT, payload)
}

fn encode_json_control<T: serde::Serialize>(msg_type: u8, payload: &T) -> Vec<u8> {
    let payload = serde_json::to_vec(payload).expect("tunnel control payload should serialize");
    encode_frame(0, msg_type, 0, &payload)
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

pub fn raw_payload(data: Bytes) -> (Bytes, u8) {
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
    use super::{
        compress_payload, decode_payload, encode_frame, encode_goaway_v3, encode_ping,
        encode_reset_stream, encode_window_update, raw_payload, resolve_tunnel_request_timeouts,
        try_decode_tunnel_relay_request_meta, Frame, FrameHeader, GoAwayPayload, MsgType,
        RequestMeta, ResetStreamPayload, WindowUpdatePayload, CURRENT_TUNNEL_PROTOCOL_VERSION,
        CURRENT_TUNNEL_PROTOCOL_VERSION_STR, FLAG_GZIP_COMPRESSED, MAX_TUNNEL_RELAY_META_LEN,
        REQUEST_HEADERS, TUNNEL_PROTOCOL_VERSION_HEADER,
    };
    use bytes::Bytes;

    fn request_meta(stream: bool) -> RequestMeta {
        RequestMeta {
            provider_id: None,
            endpoint_id: None,
            key_id: None,
            method: "POST".to_string(),
            url: "https://example.com/responses".to_string(),
            headers: std::collections::HashMap::new(),
            stream,
            request_timeout_ms: None,
            stream_first_byte_timeout_ms: None,
            timeout: 60,
            follow_redirects: None,
            http1_only: false,
            transport_profile: None,
        }
    }

    #[test]
    fn tunnel_request_timeouts_preserve_non_stream_total_timeout() {
        let mut meta = request_meta(false);
        meta.request_timeout_ms = Some(900_000);
        meta.stream_first_byte_timeout_ms = Some(12_000);

        let resolved = resolve_tunnel_request_timeouts(&meta);

        assert_eq!(resolved.first_byte_ms, 900_000);
        assert_eq!(resolved.response_body_ms, Some(900_000));
    }

    #[test]
    fn tunnel_request_timeouts_keep_stream_body_unbounded() {
        let mut meta = request_meta(true);
        meta.request_timeout_ms = Some(900_000);
        meta.stream_first_byte_timeout_ms = Some(12_000);

        let resolved = resolve_tunnel_request_timeouts(&meta);

        assert_eq!(resolved.first_byte_ms, 12_000);
        assert_eq!(resolved.response_body_ms, None);
    }

    #[test]
    fn tunnel_request_timeouts_keep_stream_first_byte_protocol_limit() {
        let mut meta = request_meta(true);
        meta.stream_first_byte_timeout_ms =
            Some(crate::MAX_EXECUTION_STREAM_FIRST_BYTE_TIMEOUT_MS + 1);

        let resolved = resolve_tunnel_request_timeouts(&meta);

        assert_eq!(
            resolved.first_byte_ms,
            crate::MAX_EXECUTION_STREAM_FIRST_BYTE_TIMEOUT_MS
        );
        assert_eq!(resolved.response_body_ms, None);
    }

    #[test]
    fn tunnel_request_timeouts_clamp_only_out_of_range_protocol_values() {
        let mut meta = request_meta(false);
        meta.request_timeout_ms = Some(crate::MAX_EXECUTION_REQUEST_TIMEOUT_MS + 1);

        let resolved = resolve_tunnel_request_timeouts(&meta);

        assert_eq!(
            resolved.first_byte_ms,
            crate::MAX_EXECUTION_REQUEST_TIMEOUT_MS
        );
        assert_eq!(
            resolved.response_body_ms,
            Some(crate::MAX_EXECUTION_REQUEST_TIMEOUT_MS)
        );
    }

    #[test]
    fn tunnel_relay_request_meta_decodes_from_a_partial_prefix() {
        let meta = request_meta(false);
        let encoded_meta = serde_json::to_vec(&meta).expect("meta should encode");
        let mut envelope = Vec::new();
        envelope.extend_from_slice(&(encoded_meta.len() as u32).to_be_bytes());
        envelope.extend_from_slice(&encoded_meta);
        envelope.extend_from_slice(b"request-body");

        assert!(try_decode_tunnel_relay_request_meta(&envelope[..3])
            .expect("partial prefix should be valid")
            .is_none());
        let (decoded, body_offset) = try_decode_tunnel_relay_request_meta(&envelope)
            .expect("envelope should be valid")
            .expect("metadata should be complete");

        assert_eq!(decoded.request_timeout_ms, meta.request_timeout_ms);
        assert_eq!(&envelope[body_offset..], b"request-body");
    }

    #[test]
    fn tunnel_relay_request_meta_rejects_oversized_prefix() {
        let oversized = (MAX_TUNNEL_RELAY_META_LEN as u32 + 1).to_be_bytes();

        assert!(try_decode_tunnel_relay_request_meta(&oversized).is_err());
    }

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

    #[test]
    fn raw_payload_never_compresses_body_frames() {
        let body = Bytes::from(vec![b'a'; 4 * 1024]);
        let (payload, flags) = raw_payload(body.clone());

        assert_eq!(payload, body);
        assert_eq!(flags & FLAG_GZIP_COMPRESSED, 0);
    }

    #[test]
    fn compress_payload_remains_available_for_control_payloads() {
        let control_payload = Bytes::from(vec![b'a'; 4 * 1024]);
        let (payload, flags) = compress_payload(control_payload.clone());
        assert_ne!(flags & FLAG_GZIP_COMPRESSED, 0);

        let encoded = encode_frame(1, REQUEST_HEADERS, flags, &payload);
        let header = FrameHeader::parse(&encoded).expect("frame should parse");
        let decoded = decode_payload(&encoded, &header).expect("payload should decode");
        assert_eq!(decoded, control_payload.to_vec());
    }

    #[test]
    fn tunnel_protocol_version_header_defaults_to_v2() {
        assert_eq!(
            TUNNEL_PROTOCOL_VERSION_HEADER,
            "x-aether-tunnel-protocol-version"
        );
        assert_eq!(CURRENT_TUNNEL_PROTOCOL_VERSION, 3);
        assert_eq!(CURRENT_TUNNEL_PROTOCOL_VERSION_STR, "3");
    }

    #[test]
    fn v3_control_frames_round_trip_json_payloads() {
        let reset = encode_reset_stream(9, "request body window exhausted");
        let reset_header = FrameHeader::parse(&reset).expect("reset header");
        assert_eq!(reset_header.msg_type, super::RESET_STREAM);
        let reset_payload = decode_payload(&reset, &reset_header).expect("reset payload");
        let reset_payload: ResetStreamPayload =
            serde_json::from_slice(&reset_payload).expect("reset json");
        assert_eq!(reset_payload.reason, "request body window exhausted");

        let window = encode_window_update(9, 1024 * 1024);
        let window_header = FrameHeader::parse(&window).expect("window header");
        assert_eq!(window_header.msg_type, super::WINDOW_UPDATE);
        let window_payload = decode_payload(&window, &window_header).expect("window payload");
        let window_payload: WindowUpdatePayload =
            serde_json::from_slice(&window_payload).expect("window json");
        assert_eq!(window_payload.delta_bytes, 1024 * 1024);

        let goaway = encode_goaway_v3(42, 30_000, "rolling restart");
        let goaway_header = FrameHeader::parse(&goaway).expect("goaway header");
        assert_eq!(goaway_header.msg_type, super::GOAWAY);
        let goaway_payload = decode_payload(&goaway, &goaway_header).expect("goaway payload");
        let goaway_payload: GoAwayPayload =
            serde_json::from_slice(&goaway_payload).expect("goaway json");
        assert_eq!(goaway_payload.last_accepted_stream_id, 42);
        assert_eq!(goaway_payload.drain_deadline_ms, 30_000);
        assert_eq!(goaway_payload.reason, "rolling restart");
    }
}
