use std::collections::BTreeMap;

use aether_contracts::{StreamFrame, StreamFramePayload};
use axum::http::StatusCode;
use base64::Engine as _;
use futures_util::StreamExt;
use serde_json::{json, Map, Value};
use tokio_util::codec::{FramedRead, LinesCodec};
use tracing::warn;

use crate::execution_runtime::ndjson::decode_stream_frame_ndjson;
use crate::execution_runtime::submission::{has_nested_error, strip_utf8_bom_and_ws};
use crate::GatewayError;
use crate::{MAX_ERROR_BODY_BYTES, MAX_STREAM_PREFETCH_FRAMES};

#[derive(Debug)]
pub(super) enum StreamPrefetchInspection {
    NeedMore,
    NonError,
    EmbeddedError(serde_json::Value),
}

pub(super) fn decode_stream_error_body(
    headers: &BTreeMap<String, String>,
    error_body: &[u8],
) -> (Option<serde_json::Value>, Option<String>) {
    if error_body.is_empty() {
        return (None, None);
    }

    let content_type = headers
        .get("content-type")
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_default();
    let looks_json = content_type.contains("json") || content_type.ends_with("+json");
    if looks_json {
        if let Ok(json_body) = serde_json::from_slice::<serde_json::Value>(error_body) {
            return (Some(json_body), None);
        }
    }

    (
        None,
        Some(base64::engine::general_purpose::STANDARD.encode(error_body)),
    )
}

fn header_value_case_insensitive<'a>(
    headers: &'a BTreeMap<String, String>,
    name: &str,
) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn remove_header_case_insensitive(headers: &mut BTreeMap<String, String>, name: &str) {
    let keys = headers
        .keys()
        .filter(|key| key.eq_ignore_ascii_case(name))
        .cloned()
        .collect::<Vec<_>>();
    for key in keys {
        headers.remove(&key);
    }
}

pub(super) fn should_synthesize_non_success_stream_error_body(
    status_code: u16,
    error_body: &[u8],
) -> bool {
    !(200..300).contains(&status_code)
        && ((300..400).contains(&status_code) || error_body.is_empty())
}

pub(super) fn build_synthetic_non_success_stream_error_body(
    status_code: u16,
    headers: &BTreeMap<String, String>,
) -> Value {
    let mut error = Map::from_iter([
        (
            "type".to_string(),
            Value::String("execution_runtime_non_success_status".to_string()),
        ),
        (
            "message".to_string(),
            Value::String(format!(
                "execution runtime stream returned non-success status {status_code}"
            )),
        ),
        ("code".to_string(), Value::from(status_code)),
        ("upstream_status".to_string(), Value::from(status_code)),
    ]);
    if let Some(location) = header_value_case_insensitive(headers, "location") {
        error.insert("location".to_string(), Value::String(location.to_string()));
    }

    Value::Object(Map::from_iter([(
        "error".to_string(),
        Value::Object(error),
    )]))
}

pub(super) fn synthetic_error_response_headers(
    mut headers: BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    remove_header_case_insensitive(&mut headers, "content-encoding");
    remove_header_case_insensitive(&mut headers, "content-length");
    remove_header_case_insensitive(&mut headers, "content-type");
    remove_header_case_insensitive(&mut headers, "location");
    headers.insert("content-type".to_string(), "application/json".to_string());
    headers
}

fn client_error_status_code_for_upstream_status(status_code: u16) -> u16 {
    if (300..400).contains(&status_code) || status_code < 200 {
        StatusCode::BAD_GATEWAY.as_u16()
    } else {
        status_code
    }
}

pub(super) fn stream_client_error_status_code_for_upstream_status(status_code: u16) -> u16 {
    client_error_status_code_for_upstream_status(status_code)
}

pub(super) fn inspect_prefetched_stream_body(
    headers: &BTreeMap<String, String>,
    body: &[u8],
) -> StreamPrefetchInspection {
    if body.is_empty() {
        return StreamPrefetchInspection::NeedMore;
    }

    let stripped = strip_utf8_bom_and_ws(body);
    let content_type = headers
        .get("content-type")
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_default();
    let looks_json = content_type.contains("json") || content_type.ends_with("+json");
    if looks_json || stripped.starts_with(b"{") || stripped.starts_with(b"[") {
        if let Ok(json_body) = serde_json::from_slice::<serde_json::Value>(stripped) {
            return if has_nested_error(&json_body) {
                StreamPrefetchInspection::EmbeddedError(json_body)
            } else {
                StreamPrefetchInspection::NonError
            };
        }
    }

    let text = String::from_utf8_lossy(body);
    let mut saw_meaningful_line = false;
    for line in text.lines().take(MAX_STREAM_PREFETCH_FRAMES) {
        let line = line.trim_matches('\r').trim();
        if line.is_empty() || line.starts_with(':') || line.starts_with("event:") {
            continue;
        }

        let data_line = line.strip_prefix("data: ").unwrap_or(line).trim();
        if data_line.is_empty() {
            continue;
        }
        if data_line == "[DONE]" {
            return StreamPrefetchInspection::NonError;
        }

        saw_meaningful_line = true;
        match serde_json::from_str::<serde_json::Value>(data_line) {
            Ok(json_body) => {
                return if has_nested_error(&json_body) {
                    StreamPrefetchInspection::EmbeddedError(json_body)
                } else {
                    StreamPrefetchInspection::NonError
                };
            }
            Err(_) => {
                if data_line.ends_with('}') || data_line.ends_with(']') {
                    return StreamPrefetchInspection::NonError;
                }
            }
        }
    }

    if saw_meaningful_line {
        StreamPrefetchInspection::NonError
    } else {
        StreamPrefetchInspection::NeedMore
    }
}

fn append_error_frame_payload(
    body: &mut Vec<u8>,
    chunk_b64: Option<&str>,
    text: Option<&str>,
) -> Result<bool, GatewayError> {
    let remaining = MAX_ERROR_BODY_BYTES.saturating_sub(body.len());
    if remaining == 0 {
        return Ok(false);
    }

    if let Some(chunk_b64) = chunk_b64 {
        // Do not decode an attacker-controlled megabyte-scale base64 value
        // merely to retain the first 16 KiB of an error body. A standard
        // base64 value representing at most `remaining` bytes cannot exceed
        // this length.
        let max_encoded_len = remaining
            .saturating_add(2)
            .checked_div(3)
            .unwrap_or(usize::MAX)
            .saturating_mul(4);
        if chunk_b64.len() > max_encoded_len {
            warn!(
                encoded_bytes = chunk_b64.len(),
                max_encoded_bytes = max_encoded_len,
                "execution runtime error frame body exceeded capture limit"
            );
            return Ok(false);
        }
        let chunk = base64::engine::general_purpose::STANDARD
            .decode(chunk_b64)
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        body.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
    } else if let Some(text) = text {
        let text_bytes = text.as_bytes();
        body.extend_from_slice(&text_bytes[..text_bytes.len().min(remaining)]);
    }

    Ok(body.len() < MAX_ERROR_BODY_BYTES)
}

pub(super) async fn collect_error_body<R>(
    lines: &mut FramedRead<R, LinesCodec>,
) -> Result<Vec<u8>, GatewayError>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut body = Vec::new();
    while let Some(frame) = read_next_frame(lines).await? {
        match frame.payload {
            StreamFramePayload::Data { chunk_b64, text } => {
                if !append_error_frame_payload(&mut body, chunk_b64.as_deref(), text.as_deref())? {
                    break;
                }
            }
            StreamFramePayload::Telemetry { .. } => {}
            StreamFramePayload::Eof { .. } => break,
            StreamFramePayload::Error { error } => {
                warn!(
                    error_kind = ?error.kind,
                    error_phase = ?error.phase,
                    upstream_status = ?error.upstream_status,
                    "execution runtime stream emitted error frame while collecting error body"
                );
                break;
            }
            StreamFramePayload::Headers { .. } => {}
        }
    }
    Ok(body)
}

pub(super) async fn read_next_frame<R>(
    lines: &mut FramedRead<R, LinesCodec>,
) -> Result<Option<StreamFrame>, GatewayError>
where
    R: tokio::io::AsyncRead + Unpin,
{
    while let Some(line) = lines.next().await {
        let line = line.map_err(|err| GatewayError::Internal(err.to_string()))?;
        if line.trim().is_empty() {
            continue;
        }
        let frame = decode_stream_frame_ndjson(line.as_bytes())?;
        return Ok(Some(frame));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::{append_error_frame_payload, MAX_ERROR_BODY_BYTES};

    #[test]
    fn oversized_base64_error_frame_is_rejected_before_decode() {
        let mut body = Vec::new();
        let encoded = "x".repeat((MAX_ERROR_BODY_BYTES + 2) / 3 * 4 + 1);

        let keep_reading = append_error_frame_payload(&mut body, Some(&encoded), None)
            .expect("oversized frame should be handled without a decode error");

        assert!(!keep_reading);
        assert!(body.is_empty());
    }

    #[test]
    fn error_frame_payload_is_capped_to_remaining_capture_budget() {
        let mut body = vec![b'a'; MAX_ERROR_BODY_BYTES - 2];

        let keep_reading = append_error_frame_payload(&mut body, None, Some("hello"))
            .expect("text payload should append");

        assert!(!keep_reading);
        assert_eq!(body.len(), MAX_ERROR_BODY_BYTES);
        assert_eq!(&body[MAX_ERROR_BODY_BYTES - 2..], b"he");
    }
}
