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

#[derive(Debug, Clone)]
pub(super) enum StreamPrefetchInspection {
    NeedMore,
    NonError,
    EmbeddedError(serde_json::Value),
    /// Body is complete but not valid JSON. Callers must decide whether to
    /// synthesize a 502 or pass through; it must never be treated as a
    /// confirmed non-error response.
    MalformedJson,
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
    if stripped.is_empty() {
        // BOM/whitespace-only body: nothing classifiable yet, keep prefetching.
        return StreamPrefetchInspection::NeedMore;
    }
    let content_type = headers
        .get("content-type")
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_default();
    let looks_json = content_type.contains("json") || content_type.ends_with("+json");
    if looks_json || stripped.starts_with(b"{") || stripped.starts_with(b"[") {
        match serde_json::from_slice::<serde_json::Value>(stripped) {
            Ok(json_body) => {
                return if has_nested_error(&json_body) {
                    StreamPrefetchInspection::EmbeddedError(json_body)
                } else {
                    StreamPrefetchInspection::NonError
                };
            }
            Err(err) => {
                // Incomplete JSON must keep prefetching: the error envelope may
                // span multiple data frames. Treating a partial JSON body as
                // NonError commits the candidate stream before the embedded
                // provider error is visible, which disables failover.
                if err.is_eof() {
                    return StreamPrefetchInspection::NeedMore;
                }
                // Complete but invalid JSON is not a confirmed success.
                return StreamPrefetchInspection::MalformedJson;
            }
        }
    }

    // Only non-JSON (SSE/text) bodies reach the line-oriented parser. The
    // previous "ends with } or ]" heuristic is removed because it guessed
    // completeness from a trailing character and could commit a partial JSON
    // line as a non-error.
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
        if let Ok(json_body) = serde_json::from_str::<serde_json::Value>(data_line) {
            return if has_nested_error(&json_body) {
                StreamPrefetchInspection::EmbeddedError(json_body)
            } else {
                StreamPrefetchInspection::NonError
            };
        }
    }

    if saw_meaningful_line {
        StreamPrefetchInspection::NonError
    } else {
        StreamPrefetchInspection::NeedMore
    }
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
                let chunk = if let Some(chunk_b64) = chunk_b64 {
                    base64::engine::general_purpose::STANDARD
                        .decode(chunk_b64)
                        .map_err(|err| GatewayError::Internal(err.to_string()))?
                } else {
                    text.unwrap_or_default().into_bytes()
                };
                body.extend_from_slice(&chunk);
                if body.len() >= MAX_ERROR_BODY_BYTES {
                    body.truncate(MAX_ERROR_BODY_BYTES);
                    break;
                }
            }
            StreamFramePayload::Telemetry { .. } => {}
            StreamFramePayload::Eof { .. } => break,
            StreamFramePayload::Error { error } => {
                warn!(error = %error.message, "execution runtime stream emitted error frame while collecting error body");
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
    use super::*;
    use std::collections::BTreeMap;

    fn json_headers() -> BTreeMap<String, String> {
        let mut headers = BTreeMap::new();
        headers.insert("content-type".to_string(), "application/json".to_string());
        headers
    }

    fn error_body() -> String {
        r#"{"error":{"code":"server_error","type":"service_unavailable_error","message":"Our servers are currently overloaded. Please try again later."}}"#
            .to_string()
    }

    #[test]
    fn empty_body_is_need_more() {
        assert!(matches!(
            inspect_prefetched_stream_body(&json_headers(), b""),
            StreamPrefetchInspection::NeedMore
        ));
    }

    #[test]
    fn whitespace_only_body_is_need_more() {
        assert!(matches!(
            inspect_prefetched_stream_body(&json_headers(), b"  \n\t "),
            StreamPrefetchInspection::NeedMore
        ));
    }

    #[test]
    fn complete_error_json_is_embedded_error() {
        let body = error_body();
        match inspect_prefetched_stream_body(&json_headers(), body.as_bytes()) {
            StreamPrefetchInspection::EmbeddedError(value) => {
                assert_eq!(
                    value["error"]["type"].as_str(),
                    Some("service_unavailable_error")
                );
            }
            other => panic!("expected EmbeddedError, got {other:?}"),
        }
    }

    #[test]
    fn partial_json_is_need_more_at_every_split() {
        let body = error_body();
        let bytes = body.as_bytes();
        // Split at every byte boundary that leaves an incomplete JSON value.
        for split in 1..bytes.len() {
            let inspection = inspect_prefetched_stream_body(&json_headers(), &bytes[..split]);
            assert!(
                matches!(inspection, StreamPrefetchInspection::NeedMore),
                "prefix of len {split} should be NeedMore, got {inspection:?}"
            );
        }
    }

    #[test]
    fn complete_success_json_is_non_error() {
        let body = r#"{"id":"resp_1","object":"response","status":"completed"}"#;
        assert!(matches!(
            inspect_prefetched_stream_body(&json_headers(), body.as_bytes()),
            StreamPrefetchInspection::NonError
        ));
    }

    #[test]
    fn malformed_complete_json_is_malformed() {
        // Syntactically invalid JSON with a trailing comma: serde reports a
        // syntax error (not EOF), so this must classify as MalformedJson
        // rather than NeedMore or NonError.
        let body = r#"{"a": 1,}"#;
        assert!(matches!(
            inspect_prefetched_stream_body(&json_headers(), body.as_bytes()),
            StreamPrefetchInspection::MalformedJson
        ));
    }

    #[test]
    fn json_detected_by_leading_brace_without_header() {
        // Missing Content-Type but body starts with '{': still classified as JSON.
        let headers = BTreeMap::new();
        let partial = error_body()[..20].as_bytes().to_vec();
        assert!(matches!(
            inspect_prefetched_stream_body(&headers, &partial),
            StreamPrefetchInspection::NeedMore
        ));
        let complete = error_body();
        assert!(matches!(
            inspect_prefetched_stream_body(&headers, complete.as_bytes()),
            StreamPrefetchInspection::EmbeddedError(_)
        ));
    }

    #[test]
    fn sse_data_line_json_is_classified() {
        let mut headers = BTreeMap::new();
        headers.insert("content-type".to_string(), "text/event-stream".to_string());
        let body = "data: {\"error\":{\"type\":\"api_error\",\"message\":\"boom\"}}\n\n";
        assert!(matches!(
            inspect_prefetched_stream_body(&headers, body.as_bytes()),
            StreamPrefetchInspection::EmbeddedError(_)
        ));
    }
}
