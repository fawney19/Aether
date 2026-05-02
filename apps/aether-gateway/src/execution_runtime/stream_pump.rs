use std::collections::BTreeMap;
use std::io::Error as IoError;
use std::time::Instant;

use aether_contracts::{
    ExecutionError, ExecutionErrorKind, ExecutionPhase, ExecutionStreamTerminalSummary,
    ExecutionTelemetry, StreamFrame, StreamFramePayload, StreamFrameType,
};
use async_stream::stream;
use axum::body::Bytes;
use base64::Engine as _;
use futures_util::{Stream, StreamExt};
use serde_json::Value;
use tracing::warn;

use crate::ai_serving::api::{
    maybe_bridge_standard_sync_json_to_stream, maybe_build_provider_private_stream_normalizer,
    normalize_provider_private_report_context, StreamingStandardTerminalObserver,
};
use crate::execution_runtime::ndjson::encode_stream_frame_ndjson;
use crate::execution_runtime::transport::DirectUpstreamResponse;
use crate::execution_runtime::DirectUpstreamStreamExecution;
use crate::GatewayError;

pub(crate) fn build_direct_execution_frame_stream(
    execution: DirectUpstreamStreamExecution,
) -> impl Stream<Item = Result<Bytes, IoError>> + Send + 'static {
    stream! {
        let DirectUpstreamStreamExecution {
            request_id: _,
            candidate_id: _,
            status_code,
            headers,
            provider_api_format,
            stream_summary_report_context,
            response,
            started_at,
        } = execution;

        let mut observer_context = stream_summary_report_context;
        if observer_context
            .get("provider_api_format")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default()
            .is_empty()
        {
            if let Some(object) = observer_context.as_object_mut() {
                object.insert(
                    "provider_api_format".to_string(),
                    Value::String(provider_api_format.clone()),
                );
            }
        }
        let normalized_observer_context =
            normalize_provider_private_report_context(Some(&observer_context))
                .unwrap_or_else(|| observer_context.clone());
        let mut private_stream_normalizer =
            maybe_build_provider_private_stream_normalizer(Some(&observer_context));
        let mut stream_terminal_observer = StreamingStandardTerminalObserver::default();
        let mut observer_buffered = Vec::new();

        if should_buffer_non_stream_response(&headers, &observer_context) {
            let original_headers = headers.clone();
            match buffer_non_sse_upstream_body(response, started_at).await {
                Ok(buffered) => {
                    let mut response_headers = original_headers;
                    let mut response_body = Bytes::from(buffered.body_bytes);
                    let mut summary = None;
                    match maybe_bridge_non_sse_sync_json_to_stream(
                        status_code,
                        &response_headers,
                        response_body.as_ref(),
                        provider_api_format.as_str(),
                        &observer_context,
                    ) {
                        Ok(Some(outcome)) => {
                            response_headers = rewrite_headers_for_bridged_sse_response(
                                &response_headers,
                                outcome.sse_body.len(),
                            );
                            response_body = Bytes::from(outcome.sse_body);
                            summary = outcome.terminal_summary;
                        }
                        Ok(None) => {}
                        Err(err) => {
                            yield Err(IoError::other(format!("{err:?}")));
                            return;
                        }
                    }

                    match encode_headers_frame(status_code, response_headers) {
                        Ok(frame) => yield Ok(frame),
                        Err(err) => {
                            yield Err(err);
                            return;
                        }
                    }
                    if !response_body.is_empty() {
                        match encode_telemetry_frame(buffered.ttfb_ms, buffered.ttfb_ms, 0) {
                            Ok(frame) => yield Ok(frame),
                            Err(err) => {
                                yield Err(err);
                                return;
                            }
                        }
                        match encode_data_frame(&response_body) {
                            Ok(frame) => yield Ok(frame),
                            Err(err) => {
                                yield Err(err);
                                return;
                            }
                        }
                    }
                    match encode_telemetry_frame(
                        buffered.ttfb_ms,
                        Some(started_at.elapsed().as_millis() as u64),
                        buffered.upstream_bytes,
                    ) {
                        Ok(frame) => yield Ok(frame),
                        Err(err) => {
                            yield Err(err);
                            return;
                        }
                    }
                    match encode_stream_frame_ndjson(&StreamFrame::eof_with_summary(summary)) {
                        Ok(frame) => yield Ok(frame),
                        Err(err) => yield Err(err),
                    }
                }
                Err(BufferedUpstreamBodyError {
                    message,
                    ttfb_ms,
                    upstream_bytes,
                }) => {
                    match encode_headers_frame(status_code, original_headers) {
                        Ok(frame) => yield Ok(frame),
                        Err(err) => {
                            yield Err(err);
                            return;
                        }
                    }
                    match encode_error_frame(status_code, message) {
                        Ok(frame) => yield Ok(frame),
                        Err(err) => {
                            yield Err(err);
                            return;
                        }
                    }
                    match encode_telemetry_frame(
                        ttfb_ms,
                        Some(started_at.elapsed().as_millis() as u64),
                        upstream_bytes,
                    ) {
                        Ok(frame) => yield Ok(frame),
                        Err(err) => {
                            yield Err(err);
                            return;
                        }
                    }
                    match encode_stream_frame_ndjson(&StreamFrame::eof_with_summary(None)) {
                        Ok(frame) => yield Ok(frame),
                        Err(err) => yield Err(err),
                    }
                }
            }
            return;
        }

        match encode_headers_frame(status_code, headers) {
            Ok(frame) => yield Ok(frame),
            Err(err) => {
                yield Err(err);
                return;
            }
        }

        let mut upstream_bytes = 0u64;
        let mut ttfb_ms = None;
        let mut first_chunk_telemetry_emitted = false;
        match response {
            DirectUpstreamResponse::Reqwest(response) => {
                let mut bytes_stream = response.bytes_stream();
                while let Some(item) = bytes_stream.next().await {
                    match item {
                        Ok(chunk) => {
                            if ttfb_ms.is_none() {
                                ttfb_ms = Some(started_at.elapsed().as_millis() as u64);
                            }
                            if !first_chunk_telemetry_emitted {
                                match encode_telemetry_frame(ttfb_ms, ttfb_ms, upstream_bytes) {
                                    Ok(frame) => yield Ok(frame),
                                    Err(err) => {
                                        yield Err(err);
                                        return;
                                    }
                                }
                                first_chunk_telemetry_emitted = true;
                            }
                            upstream_bytes += chunk.len() as u64;
                            observe_stream_chunk(
                                &mut stream_terminal_observer,
                                &normalized_observer_context,
                                private_stream_normalizer.as_mut(),
                                &mut observer_buffered,
                                chunk.as_ref(),
                            );
                            match encode_data_frame(&chunk) {
                                Ok(frame) => yield Ok(frame),
                                Err(err) => {
                                    yield Err(err);
                                    return;
                                }
                            }
                        }
                        Err(err) => {
                            let message = format_error_chain(&err);
                            warn!(
                                event_name = "stream_pump_body_read_error",
                                log_type = "ops",
                                status_code,
                                upstream_bytes,
                                error = %message,
                                "upstream body stream read error"
                            );
                            match encode_error_frame(status_code, message) {
                                Ok(frame) => yield Ok(frame),
                                Err(encode_err) => {
                                    yield Err(encode_err);
                                    return;
                                }
                            }
                            break;
                        }
                    }
                }
            }
            DirectUpstreamResponse::LocalTunnel(mut response) => loop {
                match response.next_chunk().await {
                    Ok(Some(chunk)) => {
                        if ttfb_ms.is_none() {
                            ttfb_ms = Some(started_at.elapsed().as_millis() as u64);
                        }
                        if !first_chunk_telemetry_emitted {
                            match encode_telemetry_frame(ttfb_ms, ttfb_ms, upstream_bytes) {
                                Ok(frame) => yield Ok(frame),
                                Err(err) => {
                                    yield Err(err);
                                    return;
                                }
                            }
                            first_chunk_telemetry_emitted = true;
                        }
                        upstream_bytes += chunk.len() as u64;
                        observe_stream_chunk(
                            &mut stream_terminal_observer,
                            &normalized_observer_context,
                            private_stream_normalizer.as_mut(),
                            &mut observer_buffered,
                            chunk.as_ref(),
                        );
                        match encode_data_frame(&chunk) {
                            Ok(frame) => yield Ok(frame),
                            Err(err) => {
                                yield Err(err);
                                return;
                            }
                        }
                    }
                    Ok(None) => break,
                    Err(message) => {
                        warn!(
                            event_name = "stream_pump_body_read_error",
                            log_type = "ops",
                            status_code,
                            upstream_bytes,
                            error = %message,
                            "upstream body stream read error"
                        );
                        match encode_error_frame(status_code, message) {
                            Ok(frame) => yield Ok(frame),
                            Err(encode_err) => {
                                yield Err(encode_err);
                                return;
                            }
                        }
                        break;
                    }
                }
            }
        }
        let summary = finalize_stream_terminal_summary(
            &mut stream_terminal_observer,
            &normalized_observer_context,
            private_stream_normalizer.as_mut(),
            &mut observer_buffered,
        );

        match encode_telemetry_frame(
            ttfb_ms,
            Some(started_at.elapsed().as_millis() as u64),
            upstream_bytes,
        ) {
            Ok(frame) => yield Ok(frame),
            Err(err) => {
                yield Err(err);
                return;
            }
        }
        match encode_stream_frame_ndjson(&StreamFrame::eof_with_summary(summary)) {
            Ok(frame) => yield Ok(frame),
            Err(err) => yield Err(err),
        }
    }
}

fn encode_headers_frame(
    status_code: u16,
    headers: BTreeMap<String, String>,
) -> Result<Bytes, IoError> {
    encode_stream_frame_ndjson(&StreamFrame {
        frame_type: StreamFrameType::Headers,
        payload: StreamFramePayload::Headers {
            status_code,
            headers,
        },
    })
}

fn encode_telemetry_frame(
    ttfb_ms: Option<u64>,
    elapsed_ms: Option<u64>,
    upstream_bytes: u64,
) -> Result<Bytes, IoError> {
    encode_stream_frame_ndjson(&StreamFrame {
        frame_type: StreamFrameType::Telemetry,
        payload: StreamFramePayload::Telemetry {
            telemetry: ExecutionTelemetry {
                ttfb_ms,
                elapsed_ms,
                upstream_bytes: Some(upstream_bytes),
            },
        },
    })
}

fn encode_data_frame(chunk: &Bytes) -> Result<Bytes, IoError> {
    encode_stream_frame_ndjson(&StreamFrame {
        frame_type: StreamFrameType::Data,
        payload: StreamFramePayload::Data {
            chunk_b64: Some(base64::engine::general_purpose::STANDARD.encode(chunk)),
            text: None,
        },
    })
}

fn encode_error_frame(status_code: u16, message: String) -> Result<Bytes, IoError> {
    encode_stream_frame_ndjson(&StreamFrame {
        frame_type: StreamFrameType::Error,
        payload: StreamFramePayload::Error {
            error: ExecutionError {
                kind: ExecutionErrorKind::Internal,
                phase: ExecutionPhase::StreamRead,
                message,
                upstream_status: Some(status_code),
                retryable: false,
                failover_recommended: false,
            },
        },
    })
}

struct BufferedUpstreamBody {
    body_bytes: Vec<u8>,
    ttfb_ms: Option<u64>,
    upstream_bytes: u64,
}

struct BufferedUpstreamBodyError {
    message: String,
    ttfb_ms: Option<u64>,
    upstream_bytes: u64,
}

fn response_headers_indicate_sse(headers: &BTreeMap<String, String>) -> bool {
    headers
        .get("content-type")
        .is_some_and(|value| value.to_ascii_lowercase().contains("text/event-stream"))
}

fn should_treat_upstream_response_as_stream(
    headers: &BTreeMap<String, String>,
    report_context: &Value,
) -> bool {
    if response_headers_indicate_sse(headers) {
        return true;
    }

    report_context
        .get("envelope_name")
        .and_then(Value::as_str)
        .is_some_and(|value| value.eq_ignore_ascii_case(crate::ai_serving::KIRO_ENVELOPE_NAME))
}

fn should_buffer_non_stream_response(
    headers: &BTreeMap<String, String>,
    report_context: &Value,
) -> bool {
    if should_treat_upstream_response_as_stream(headers, report_context) {
        return false;
    }

    headers
        .get("content-length")
        .and_then(|value| value.trim().parse::<u64>().ok())
        .is_some()
}

async fn buffer_non_sse_upstream_body(
    response: DirectUpstreamResponse,
    started_at: Instant,
) -> Result<BufferedUpstreamBody, BufferedUpstreamBodyError> {
    let mut body_bytes = Vec::new();
    let mut upstream_bytes = 0u64;
    let mut ttfb_ms = None;

    match response {
        DirectUpstreamResponse::Reqwest(response) => {
            let mut bytes_stream = response.bytes_stream();
            while let Some(item) = bytes_stream.next().await {
                match item {
                    Ok(chunk) => {
                        if ttfb_ms.is_none() {
                            ttfb_ms = Some(started_at.elapsed().as_millis() as u64);
                        }
                        upstream_bytes += chunk.len() as u64;
                        body_bytes.extend_from_slice(&chunk);
                    }
                    Err(err) => {
                        let message = format_error_chain(&err);
                        warn!(
                            event_name = "stream_pump_body_read_error",
                            log_type = "ops",
                            upstream_bytes,
                            error = %message,
                            "upstream body stream read error"
                        );
                        return Err(BufferedUpstreamBodyError {
                            message,
                            ttfb_ms,
                            upstream_bytes,
                        });
                    }
                }
            }
        }
        DirectUpstreamResponse::LocalTunnel(mut response) => loop {
            match response.next_chunk().await {
                Ok(Some(chunk)) => {
                    if ttfb_ms.is_none() {
                        ttfb_ms = Some(started_at.elapsed().as_millis() as u64);
                    }
                    upstream_bytes += chunk.len() as u64;
                    body_bytes.extend_from_slice(&chunk);
                }
                Ok(None) => break,
                Err(message) => {
                    warn!(
                        event_name = "stream_pump_body_read_error",
                        log_type = "ops",
                        upstream_bytes,
                        error = %message,
                        "upstream body stream read error"
                    );
                    return Err(BufferedUpstreamBodyError {
                        message,
                        ttfb_ms,
                        upstream_bytes,
                    });
                }
            }
        },
    }

    Ok(BufferedUpstreamBody {
        body_bytes,
        ttfb_ms,
        upstream_bytes,
    })
}

fn maybe_bridge_non_sse_sync_json_to_stream(
    status_code: u16,
    headers: &BTreeMap<String, String>,
    body_bytes: &[u8],
    provider_api_format: &str,
    report_context: &Value,
) -> Result<Option<crate::ai_serving::SyncToStreamBridgeOutcome>, GatewayError> {
    if !(200..300).contains(&status_code) || body_bytes.is_empty() {
        return Ok(None);
    }

    let decoded_body_bytes = decode_non_sse_response_body_bytes(headers, body_bytes)
        .unwrap_or_else(|| body_bytes.to_vec());
    if !response_body_is_json(headers, &decoded_body_bytes) {
        return Ok(None);
    }

    let body_json: Value = serde_json::from_slice(&decoded_body_bytes)
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    let client_api_format = report_context
        .get("client_api_format")
        .and_then(Value::as_str)
        .unwrap_or(provider_api_format);
    maybe_bridge_standard_sync_json_to_stream(
        &body_json,
        provider_api_format,
        client_api_format,
        Some(report_context),
    )
}

fn rewrite_headers_for_bridged_sse_response(
    headers: &BTreeMap<String, String>,
    body_len: usize,
) -> BTreeMap<String, String> {
    let mut rewritten = headers.clone();
    rewritten.remove("content-encoding");
    rewritten.insert("content-type".to_string(), "text/event-stream".to_string());
    rewritten.insert("content-length".to_string(), body_len.to_string());
    rewritten
}

fn decode_non_sse_response_body_bytes(
    headers: &BTreeMap<String, String>,
    body_bytes: &[u8],
) -> Option<Vec<u8>> {
    let encoding = headers
        .get("content-encoding")
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase());
    match encoding.as_deref() {
        Some("gzip") => {
            let mut decoder = flate2::read::GzDecoder::new(body_bytes);
            let mut out = Vec::new();
            std::io::Read::read_to_end(&mut decoder, &mut out).ok()?;
            Some(out)
        }
        Some("deflate") => {
            let mut decoder = flate2::read::DeflateDecoder::new(body_bytes);
            let mut out = Vec::new();
            std::io::Read::read_to_end(&mut decoder, &mut out).ok()?;
            Some(out)
        }
        _ => None,
    }
}

fn response_body_is_json(headers: &BTreeMap<String, String>, body_bytes: &[u8]) -> bool {
    if headers
        .get("content-type")
        .map(|value| value.to_ascii_lowercase())
        .is_some_and(|value| value.contains("json"))
    {
        return true;
    }

    serde_json::from_slice::<Value>(body_bytes).is_ok()
}

fn format_error_chain(err: &(dyn std::error::Error + 'static)) -> String {
    let mut message = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message
}

fn observe_stream_chunk(
    observer: &mut StreamingStandardTerminalObserver,
    report_context: &Value,
    private_stream_normalizer: Option<&mut crate::ai_serving::ProviderPrivateStreamNormalizer<'_>>,
    observer_buffered: &mut Vec<u8>,
    chunk: &[u8],
) {
    let normalized = if let Some(normalizer) = private_stream_normalizer {
        match normalizer.push_chunk(chunk) {
            Ok(normalized) => normalized,
            Err(err) => {
                observer.disable_with_error(format!(
                    "failed to normalize provider private stream chunk: {err:?}"
                ));
                return;
            }
        }
    } else {
        chunk.to_vec()
    };

    observe_normalized_bytes(observer, report_context, observer_buffered, &normalized);
}

fn finalize_stream_terminal_summary(
    observer: &mut StreamingStandardTerminalObserver,
    report_context: &Value,
    private_stream_normalizer: Option<&mut crate::ai_serving::ProviderPrivateStreamNormalizer<'_>>,
    observer_buffered: &mut Vec<u8>,
) -> Option<ExecutionStreamTerminalSummary> {
    if let Some(normalizer) = private_stream_normalizer {
        match normalizer.finish() {
            Ok(flushed) => {
                observe_normalized_bytes(observer, report_context, observer_buffered, &flushed)
            }
            Err(err) => observer.disable_with_error(format!(
                "failed to flush provider private stream normalization: {err:?}"
            )),
        }
    }

    if !observer_buffered.is_empty() {
        let line = std::mem::take(observer_buffered);
        if let Err(err) = observer.push_line(report_context, line) {
            observer.disable_with_error(err.to_string());
        }
    }

    match observer.finish(report_context) {
        Ok(summary) => summary,
        Err(err) => {
            observer.disable_with_error(err.to_string());
            observer.latest_summary().cloned()
        }
    }
}

fn observe_normalized_bytes(
    observer: &mut StreamingStandardTerminalObserver,
    report_context: &Value,
    observer_buffered: &mut Vec<u8>,
    normalized: &[u8],
) {
    if normalized.is_empty() {
        return;
    }
    observer_buffered.extend_from_slice(normalized);
    while let Some(line_end) = observer_buffered.iter().position(|byte| *byte == b'\n') {
        let line = observer_buffered.drain(..=line_end).collect::<Vec<_>>();
        if let Err(err) = observer.push_line(report_context, line) {
            observer.disable_with_error(err.to_string());
            observer_buffered.clear();
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::convert::Infallible;
    use std::sync::Arc;
    use std::time::Duration;

    use aether_contracts::{ExecutionPlan, ExecutionTimeouts, RequestBody};
    use async_stream::stream;
    use axum::body::{Body, Bytes};
    use axum::extract::ws::Message;
    use axum::routing::post;
    use axum::{http::header, http::HeaderValue, Router};
    use base64::Engine as _;
    use futures_util::StreamExt;
    use serde_json::Value;
    use tokio::sync::watch;

    use super::{
        build_direct_execution_frame_stream, should_buffer_non_stream_response,
        should_treat_upstream_response_as_stream,
    };
    use crate::execution_runtime::transport::{
        execute_stream_plan_via_local_tunnel, DirectSyncExecutionRuntime, DirectUpstreamResponse,
    };
    use crate::tunnel::{tunnel_protocol, TunnelProxyConn};
    use crate::AppState;

    fn tunnel_proxy_snapshot(base_url: String) -> aether_contracts::ProxySnapshot {
        aether_contracts::ProxySnapshot {
            enabled: Some(true),
            mode: Some("tunnel".into()),
            node_id: Some("node-1".into()),
            label: Some("relay-node".into()),
            url: None,
            extra: Some(serde_json::json!({"tunnel_base_url": base_url})),
        }
    }

    #[test]
    fn treats_kiro_eventstream_envelope_as_stream_even_when_content_type_is_json() {
        let headers = BTreeMap::from([("content-type".into(), "application/json".into())]);
        let report_context = serde_json::json!({
            "envelope_name": "kiro:generateAssistantResponse",
        });

        assert!(should_treat_upstream_response_as_stream(
            &headers,
            &report_context
        ));
    }

    #[test]
    fn buffers_non_sse_response_only_when_content_length_is_known() {
        let report_context = serde_json::json!({
            "provider_api_format": "openai:chat",
            "client_api_format": "openai:chat",
        });

        assert!(!should_buffer_non_stream_response(
            &BTreeMap::from([("content-type".into(), "application/json".into())]),
            &report_context
        ));
        assert!(should_buffer_non_stream_response(
            &BTreeMap::from([
                ("content-type".into(), "application/json".into()),
                ("content-length".into(), "128".into()),
            ]),
            &report_context
        ));
    }

    #[tokio::test]
    async fn direct_execution_frame_stream_reports_ttfb_after_first_upstream_chunk() {
        let listener = crate::test_support::bind_loopback_listener()
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("local addr should resolve");
        let app = Router::new().route(
            "/chat",
            post(|| async {
                let stream = stream! {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                    yield Ok::<Bytes, Infallible>(Bytes::from_static(b"data: hello\n\n"));
                    yield Ok::<Bytes, Infallible>(Bytes::from_static(b"data: [DONE]\n\n"));
                };
                let mut response = axum::http::Response::new(Body::from_stream(stream));
                response.headers_mut().insert(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("text/event-stream"),
                );
                response
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("test server should run");
        });

        let execution = DirectSyncExecutionRuntime::new()
            .execute_stream(&ExecutionPlan {
                request_id: "req-stream-ttfb-1".into(),
                candidate_id: Some("cand-stream-ttfb-1".into()),
                provider_name: Some("openai".into()),
                provider_id: "prov-1".into(),
                endpoint_id: "ep-1".into(),
                key_id: "key-1".into(),
                method: "POST".into(),
                url: format!("http://{addr}/chat"),
                headers: BTreeMap::from([("content-type".into(), "application/json".into())]),
                content_type: Some("application/json".into()),
                content_encoding: None,
                body: RequestBody::from_json(serde_json::json!({"stream": true})),
                stream: true,
                client_api_format: "openai:chat".into(),
                provider_api_format: "openai:chat".into(),
                model_name: Some("gpt-5".into()),
                proxy: None,
                tls_profile: None,
                timeouts: Some(ExecutionTimeouts {
                    connect_ms: Some(5_000),
                    total_ms: Some(5_000),
                    ..ExecutionTimeouts::default()
                }),
            })
            .await
            .expect("stream execution should succeed");

        let frame_output = build_direct_execution_frame_stream(execution)
            .map(|item| item.expect("frame should encode"))
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|bytes| String::from_utf8(bytes.to_vec()).expect("frame should be utf8"))
            .collect::<String>();

        server.abort();

        let telemetry_ttfb_ms = frame_output
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .find_map(|frame| {
                (frame.get("type").and_then(Value::as_str) == Some("telemetry")).then(|| {
                    frame
                        .get("payload")
                        .and_then(|payload| payload.get("telemetry"))
                        .and_then(|telemetry| telemetry.get("ttfb_ms"))
                        .and_then(Value::as_u64)
                })?
            });

        assert!(
            telemetry_ttfb_ms.is_some_and(|value| value > 0),
            "telemetry frame should include a measured ttfb"
        );
    }

    #[tokio::test]
    async fn direct_execution_frame_stream_emits_telemetry_before_first_data_frame() {
        let listener = crate::test_support::bind_loopback_listener()
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("local addr should resolve");
        let server = tokio::spawn(async move {
            let app = Router::new().route(
                "/stream",
                post(|| async {
                    let body_stream = stream! {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                        yield Ok::<Bytes, Infallible>(Bytes::from_static(b"data: hello\n\n"));
                    };
                    (
                        [(
                            header::CONTENT_TYPE,
                            HeaderValue::from_static("text/event-stream"),
                        )],
                        Body::from_stream(body_stream),
                    )
                }),
            );
            axum::serve(listener, app)
                .await
                .expect("server should start");
        });

        let runtime = DirectSyncExecutionRuntime::new();
        let execution = runtime
            .execute_stream(&ExecutionPlan {
                request_id: "req-telemetry-order".to_string(),
                candidate_id: Some("cand-telemetry-order".to_string()),
                provider_name: Some("OpenAI".to_string()),
                provider_id: "provider-1".to_string(),
                endpoint_id: "endpoint-1".to_string(),
                key_id: "key-1".to_string(),
                method: "POST".to_string(),
                url: format!("http://{addr}/stream"),
                headers: BTreeMap::new(),
                content_type: None,
                content_encoding: None,
                body: RequestBody {
                    json_body: None,
                    body_bytes_b64: None,
                    body_ref: None,
                },
                stream: true,
                client_api_format: "openai:chat".to_string(),
                provider_api_format: "openai:chat".to_string(),
                model_name: Some("gpt-5".into()),
                proxy: None,
                tls_profile: None,
                timeouts: Some(ExecutionTimeouts {
                    connect_ms: Some(5_000),
                    total_ms: Some(5_000),
                    ..ExecutionTimeouts::default()
                }),
            })
            .await
            .expect("stream execution should succeed");

        let frames = build_direct_execution_frame_stream(execution)
            .map(|item| item.expect("frame should encode"))
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|bytes| String::from_utf8(bytes.to_vec()).expect("frame should be utf8"))
            .collect::<Vec<_>>();

        server.abort();

        let frame_types = frames
            .iter()
            .map(|line| {
                serde_json::from_str::<Value>(line)
                    .expect("frame should parse")
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect::<Vec<_>>();

        let first_data_idx = frame_types
            .iter()
            .position(|kind| kind == "data")
            .expect("data frame should exist");
        let first_telemetry_idx = frame_types
            .iter()
            .position(|kind| kind == "telemetry")
            .expect("telemetry frame should exist");

        assert!(
            first_telemetry_idx < first_data_idx,
            "first telemetry frame should be emitted before the first data frame"
        );
    }

    #[tokio::test]
    async fn direct_execution_frame_stream_bridges_sync_json_body_to_sse_for_standard_stream_request(
    ) {
        let listener = crate::test_support::bind_loopback_listener()
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("local addr should resolve");
        let server = tokio::spawn(async move {
            let app = Router::new().route(
                "/responses",
                post(|| async {
                    let body = serde_json::json!({
                        "id": "resp_sync_bridge_123",
                        "object": "response",
                        "model": "gpt-5.4",
                        "status": "completed",
                        "output": [{
                            "type": "message",
                            "id": "msg_sync_bridge_123",
                            "role": "assistant",
                            "content": [{
                                "type": "output_text",
                                "text": "Hello from buffered JSON stream",
                                "annotations": []
                            }]
                        }],
                        "usage": {
                            "input_tokens": 1,
                            "output_tokens": 2,
                            "total_tokens": 3
                        }
                    });
                    let mut response = axum::http::Response::new(Body::from(
                        serde_json::to_vec(&body).expect("json should encode"),
                    ));
                    response.headers_mut().insert(
                        header::CONTENT_TYPE,
                        HeaderValue::from_static("application/json"),
                    );
                    response
                }),
            );
            axum::serve(listener, app)
                .await
                .expect("server should start");
        });

        let runtime = DirectSyncExecutionRuntime::new();
        let execution = runtime
            .execute_stream(&ExecutionPlan {
                request_id: "req-sync-bridge".to_string(),
                candidate_id: Some("cand-sync-bridge".to_string()),
                provider_name: Some("OpenAI".to_string()),
                provider_id: "provider-1".to_string(),
                endpoint_id: "endpoint-1".to_string(),
                key_id: "key-1".to_string(),
                method: "POST".to_string(),
                url: format!("http://{addr}/responses"),
                headers: BTreeMap::new(),
                content_type: None,
                content_encoding: None,
                body: RequestBody::from_json(serde_json::json!({
                    "model": "gpt-5.4",
                    "input": "hello",
                    "stream": true
                })),
                stream: true,
                client_api_format: "openai:responses".to_string(),
                provider_api_format: "openai:responses".to_string(),
                model_name: Some("gpt-5.4".into()),
                proxy: None,
                tls_profile: None,
                timeouts: Some(ExecutionTimeouts {
                    connect_ms: Some(5_000),
                    total_ms: Some(5_000),
                    ..ExecutionTimeouts::default()
                }),
            })
            .await
            .expect("stream execution should succeed");

        let frames = build_direct_execution_frame_stream(execution)
            .map(|item| item.expect("frame should encode"))
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|bytes| String::from_utf8(bytes.to_vec()).expect("frame should be utf8"))
            .collect::<Vec<_>>();

        server.abort();

        let header_frame: Value =
            serde_json::from_str(&frames[0]).expect("headers frame should parse");
        assert_eq!(
            header_frame
                .get("payload")
                .and_then(|payload| payload.get("headers"))
                .and_then(|headers| headers.get("content-type"))
                .and_then(Value::as_str),
            Some("text/event-stream")
        );

        let data_frame = frames
            .iter()
            .map(|line| serde_json::from_str::<Value>(line).expect("frame should parse"))
            .find(|frame| frame.get("type").and_then(Value::as_str) == Some("data"))
            .expect("data frame should exist");
        let bridged_body = base64::engine::general_purpose::STANDARD
            .decode(
                data_frame
                    .get("payload")
                    .and_then(|payload| payload.get("chunk_b64"))
                    .and_then(Value::as_str)
                    .expect("chunk_b64 should exist"),
            )
            .expect("data frame should decode");
        let bridged_text = String::from_utf8(bridged_body).expect("bridged body should be utf8");
        assert!(bridged_text.contains("event: response.output_text.delta"));
        assert!(bridged_text.contains("\"delta\":\"Hello from buffered JSON stream\""));
        assert!(bridged_text.contains("event: response.completed"));

        let eof_frame = frames
            .iter()
            .map(|line| serde_json::from_str::<Value>(line).expect("frame should parse"))
            .find(|frame| frame.get("type").and_then(Value::as_str) == Some("eof"))
            .expect("eof frame should exist");
        assert_eq!(
            eof_frame
                .get("payload")
                .and_then(|payload| payload.get("summary"))
                .and_then(|summary| summary.get("response_id"))
                .and_then(Value::as_str),
            Some("resp_sync_bridge_123")
        );
    }

    #[tokio::test]
    async fn direct_execution_frame_stream_bridges_openai_image_sync_json_to_image_sse() {
        let listener = crate::test_support::bind_loopback_listener()
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("local addr should resolve");
        let server = tokio::spawn(async move {
            let app = Router::new().route(
                "/responses",
                post(|| async {
                    let body = serde_json::json!({
                        "created": 1776971267_u64,
                        "data": [{
                            "b64_json": "aGVsbG8="
                        }],
                        "usage": {
                            "total_tokens": 100,
                            "input_tokens": 50,
                            "output_tokens": 50,
                            "input_tokens_details": {
                                "text_tokens": 10,
                                "image_tokens": 40
                            }
                        }
                    });
                    let mut response = axum::http::Response::new(Body::from(
                        serde_json::to_vec(&body).expect("json should encode"),
                    ));
                    response.headers_mut().insert(
                        header::CONTENT_TYPE,
                        HeaderValue::from_static("application/json"),
                    );
                    response
                }),
            );
            axum::serve(listener, app)
                .await
                .expect("server should start");
        });

        let runtime = DirectSyncExecutionRuntime::new();
        let execution = runtime
            .execute_stream(&ExecutionPlan {
                request_id: "req-image-sync-bridge".to_string(),
                candidate_id: Some("cand-image-sync-bridge".to_string()),
                provider_name: Some("OpenAI".to_string()),
                provider_id: "provider-1".to_string(),
                endpoint_id: "endpoint-1".to_string(),
                key_id: "key-1".to_string(),
                method: "POST".to_string(),
                url: format!("http://{addr}/responses"),
                headers: BTreeMap::new(),
                content_type: None,
                content_encoding: None,
                body: RequestBody::from_json(serde_json::json!({
                    "model": "gpt-image-1",
                    "prompt": "poster",
                    "stream": true
                })),
                stream: true,
                client_api_format: "openai:image".to_string(),
                provider_api_format: "openai:image".to_string(),
                model_name: Some("gpt-image-1".into()),
                proxy: None,
                tls_profile: None,
                timeouts: Some(ExecutionTimeouts {
                    connect_ms: Some(5_000),
                    total_ms: Some(5_000),
                    ..ExecutionTimeouts::default()
                }),
            })
            .await
            .expect("stream execution should succeed");

        let frames = build_direct_execution_frame_stream(execution)
            .map(|item| item.expect("frame should encode"))
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|bytes| String::from_utf8(bytes.to_vec()).expect("frame should be utf8"))
            .collect::<Vec<_>>();

        server.abort();

        let header_frame: Value =
            serde_json::from_str(&frames[0]).expect("headers frame should parse");
        assert_eq!(
            header_frame
                .get("payload")
                .and_then(|payload| payload.get("headers"))
                .and_then(|headers| headers.get("content-type"))
                .and_then(Value::as_str),
            Some("text/event-stream")
        );

        let data_frame = frames
            .iter()
            .map(|line| serde_json::from_str::<Value>(line).expect("frame should parse"))
            .find(|frame| frame.get("type").and_then(Value::as_str) == Some("data"))
            .expect("data frame should exist");
        let bridged_body = base64::engine::general_purpose::STANDARD
            .decode(
                data_frame
                    .get("payload")
                    .and_then(|payload| payload.get("chunk_b64"))
                    .and_then(Value::as_str)
                    .expect("chunk_b64 should exist"),
            )
            .expect("data frame should decode");
        let bridged_text = String::from_utf8(bridged_body).expect("bridged body should be utf8");
        assert!(bridged_text.contains("event: image_generation.completed"));
        assert!(bridged_text.contains("\"type\":\"image_generation.completed\""));
        assert!(bridged_text.contains("\"b64_json\":\"aGVsbG8=\""));
        assert!(bridged_text.contains("\"total_tokens\":100"));

        let eof_frame = frames
            .iter()
            .map(|line| serde_json::from_str::<Value>(line).expect("frame should parse"))
            .find(|frame| frame.get("type").and_then(Value::as_str) == Some("eof"))
            .expect("eof frame should exist");
        assert_eq!(
            eof_frame
                .get("payload")
                .and_then(|payload| payload.get("summary"))
                .and_then(|summary| summary.get("model"))
                .and_then(Value::as_str),
            Some("gpt-image-1")
        );
        assert_eq!(
            eof_frame
                .get("payload")
                .and_then(|payload| payload.get("summary"))
                .and_then(|summary| summary.get("standardized_usage"))
                .and_then(|usage| usage.get("dimensions"))
                .and_then(|dimensions| dimensions.get("total_tokens"))
                .and_then(Value::as_i64),
            Some(100)
        );
    }

    #[tokio::test]
    async fn direct_execution_frame_stream_preserves_local_tunnel_stream_error_message() {
        let state = AppState::new().expect("app state should build");
        let tunnel_app = state.tunnel.app_state();
        let (proxy_tx, mut proxy_rx) = aether_runtime::bounded_queue(8);
        let (proxy_close_tx, _) = watch::channel(false);
        tunnel_app.hub.register_proxy(Arc::new(TunnelProxyConn::new(
            801,
            "node-1".to_string(),
            "Node 1".to_string(),
            proxy_tx,
            proxy_close_tx,
            16,
        )));

        let plan = ExecutionPlan {
            request_id: "req-local-stream-error-1".into(),
            candidate_id: Some("cand-local-stream-error-1".into()),
            provider_name: Some("openai".into()),
            provider_id: "prov-1".into(),
            endpoint_id: "ep-1".into(),
            key_id: "key-1".into(),
            method: "POST".into(),
            url: "https://example.com/chat".into(),
            headers: BTreeMap::from([("content-type".into(), "application/json".into())]),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(serde_json::json!({"stream": true})),
            stream: true,
            client_api_format: "openai:chat".into(),
            provider_api_format: "openai:chat".into(),
            model_name: Some("gpt-5".into()),
            proxy: Some(tunnel_proxy_snapshot("http://127.0.0.1:1".to_string())),
            tls_profile: None,
            timeouts: Some(ExecutionTimeouts {
                connect_ms: Some(5_000),
                total_ms: Some(5_000),
                ..ExecutionTimeouts::default()
            }),
        };

        let state_for_task = state.clone();
        let plan_for_task = plan.clone();
        let execution_task = tokio::spawn(async move {
            execute_stream_plan_via_local_tunnel(&state_for_task, &plan_for_task).await
        });

        let request_headers = match proxy_rx.recv().await.expect("headers frame should arrive") {
            Message::Binary(data) => data,
            other => panic!("unexpected message: {other:?}"),
        };
        let request_header = tunnel_protocol::FrameHeader::parse(&request_headers)
            .expect("request header frame should parse");
        assert_eq!(request_header.msg_type, tunnel_protocol::REQUEST_HEADERS);

        let request_body = match proxy_rx.recv().await.expect("body frame should arrive") {
            Message::Binary(data) => data,
            other => panic!("unexpected message: {other:?}"),
        };
        let request_body_header = tunnel_protocol::FrameHeader::parse(&request_body)
            .expect("request body frame should parse");
        assert_eq!(request_body_header.msg_type, tunnel_protocol::REQUEST_BODY);

        let response_meta = tunnel_protocol::ResponseMeta {
            status: 200,
            headers: vec![("content-type".to_string(), "text/event-stream".to_string())],
        };
        let response_payload =
            serde_json::to_vec(&response_meta).expect("response meta should serialize");
        let mut response_headers_frame = tunnel_protocol::encode_frame(
            request_header.stream_id,
            tunnel_protocol::RESPONSE_HEADERS,
            0,
            &response_payload,
        );
        tunnel_app
            .hub
            .handle_proxy_frame(801, &mut response_headers_frame)
            .await;

        let execution = execution_task
            .await
            .expect("execution task should complete")
            .expect("local tunnel execution should resolve")
            .expect("local tunnel execution should be available");

        let frame_task = tokio::spawn(async move {
            build_direct_execution_frame_stream(execution)
                .map(|item| item.expect("frame should encode"))
                .collect::<Vec<_>>()
                .await
                .into_iter()
                .map(|bytes| String::from_utf8(bytes.to_vec()).expect("frame should be utf8"))
                .collect::<Vec<_>>()
        });

        let mut response_body_frame = tunnel_protocol::encode_frame(
            request_header.stream_id,
            tunnel_protocol::RESPONSE_BODY,
            0,
            b"data: hello\n\n",
        );
        tunnel_app
            .hub
            .handle_proxy_frame(801, &mut response_body_frame)
            .await;

        let original_error = "proxy disconnected while forwarding upstream body";
        let mut response_error_frame =
            tunnel_protocol::encode_stream_error(request_header.stream_id, original_error);
        tunnel_app
            .hub
            .handle_proxy_frame(801, &mut response_error_frame)
            .await;

        let frames = frame_task.await.expect("frame task should complete");
        let parsed_frames = frames
            .iter()
            .map(|line| serde_json::from_str::<Value>(line).expect("frame should parse"))
            .collect::<Vec<_>>();

        assert!(
            parsed_frames
                .iter()
                .any(|frame| { frame.get("type").and_then(Value::as_str) == Some("data") }),
            "stream should contain at least one data frame before the error"
        );

        let error_message = parsed_frames
            .iter()
            .find(|frame| frame.get("type").and_then(Value::as_str) == Some("error"))
            .and_then(|frame| frame.get("payload"))
            .and_then(|payload| payload.get("error"))
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
            .expect("error frame should include a message");

        assert_eq!(error_message, original_error);
        assert!(
            !error_message.contains("unexpected EOF during chunk size line"),
            "local tunnel path should preserve the original proxy error text"
        );
    }

    #[tokio::test]
    async fn second_local_tunnel_request_works_after_first_completes() {
        let state = AppState::new().expect("app state should build");
        let tunnel_app = state.tunnel.app_state();
        let (proxy_tx, mut proxy_rx) = aether_runtime::bounded_queue(8);
        let (proxy_close_tx, _) = watch::channel(false);
        tunnel_app.hub.register_proxy(Arc::new(TunnelProxyConn::new(
            900,
            "node-1".to_string(),
            "Node 1".to_string(),
            proxy_tx,
            proxy_close_tx,
            16,
        )));

        let plan = ExecutionPlan {
            request_id: "req-reuse-1".into(),
            candidate_id: Some("cand-reuse-1".into()),
            provider_name: Some("openai".into()),
            provider_id: "prov-1".into(),
            endpoint_id: "ep-1".into(),
            key_id: "key-1".into(),
            method: "POST".into(),
            url: "https://example.com/chat".into(),
            headers: BTreeMap::from([("content-type".into(), "application/json".into())]),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(serde_json::json!({"stream": true})),
            stream: true,
            client_api_format: "openai:chat".into(),
            provider_api_format: "openai:chat".into(),
            model_name: Some("gpt-5".into()),
            proxy: Some(tunnel_proxy_snapshot("http://127.0.0.1:1".to_string())),
            tls_profile: None,
            timeouts: Some(ExecutionTimeouts {
                connect_ms: Some(5_000),
                total_ms: Some(5_000),
                ..ExecutionTimeouts::default()
            }),
        };

        // --- First request ---
        let state1 = state.clone();
        let plan1 = plan.clone();
        let exec1 =
            tokio::spawn(
                async move { execute_stream_plan_via_local_tunnel(&state1, &plan1).await },
            );

        // Read request frames from proxy side
        let req1_headers = match proxy_rx.recv().await.expect("req1 headers") {
            Message::Binary(data) => data,
            other => panic!("unexpected: {other:?}"),
        };
        let req1_header =
            tunnel_protocol::FrameHeader::parse(&req1_headers).expect("req1 header parse");
        let _req1_body = proxy_rx.recv().await.expect("req1 body");

        // Simulate proxy response
        let resp_meta = serde_json::to_vec(&tunnel_protocol::ResponseMeta {
            status: 200,
            headers: vec![("content-type".to_string(), "text/event-stream".to_string())],
        })
        .unwrap();
        let mut resp_headers = tunnel_protocol::encode_frame(
            req1_header.stream_id,
            tunnel_protocol::RESPONSE_HEADERS,
            0,
            &resp_meta,
        );
        tunnel_app
            .hub
            .handle_proxy_frame(900, &mut resp_headers)
            .await;

        let execution1 = exec1
            .await
            .expect("task")
            .expect("transport")
            .expect("execution");

        // Consume the body stream fully
        let mut resp1 = match execution1.response {
            DirectUpstreamResponse::LocalTunnel(r) => r,
            _ => panic!("expected local tunnel response"),
        };

        // Send body + STREAM_END
        let mut body_frame = tunnel_protocol::encode_frame(
            req1_header.stream_id,
            tunnel_protocol::RESPONSE_BODY,
            0,
            b"data: hello\n\n",
        );
        tunnel_app
            .hub
            .handle_proxy_frame(900, &mut body_frame)
            .await;
        let mut end_frame = tunnel_protocol::encode_frame(
            req1_header.stream_id,
            tunnel_protocol::STREAM_END,
            0,
            &[],
        );
        tunnel_app.hub.handle_proxy_frame(900, &mut end_frame).await;

        // Drain the body
        while let Ok(Some(_)) = resp1.next_chunk().await {}
        drop(resp1);

        // --- Second request ---
        let state2 = state.clone();
        let plan2 = ExecutionPlan {
            request_id: "req-reuse-2".into(),
            candidate_id: Some("cand-reuse-2".into()),
            ..plan.clone()
        };
        let exec2 =
            tokio::spawn(
                async move { execute_stream_plan_via_local_tunnel(&state2, &plan2).await },
            );

        // Read second request's frames
        let req2_headers = tokio::time::timeout(Duration::from_secs(2), proxy_rx.recv())
            .await
            .expect("second request should arrive within 2s")
            .expect("req2 headers");
        let req2_data = match req2_headers {
            Message::Binary(data) => data,
            other => panic!("unexpected: {other:?}"),
        };
        let req2_header =
            tunnel_protocol::FrameHeader::parse(&req2_data).expect("req2 header parse");
        assert_eq!(req2_header.msg_type, tunnel_protocol::REQUEST_HEADERS);

        // Simulate proxy response for second request
        let mut resp2_headers = tunnel_protocol::encode_frame(
            req2_header.stream_id,
            tunnel_protocol::RESPONSE_HEADERS,
            0,
            &resp_meta,
        );
        tunnel_app
            .hub
            .handle_proxy_frame(900, &mut resp2_headers)
            .await;

        let execution2 = exec2
            .await
            .expect("task")
            .expect("transport")
            .expect("second execution should succeed");
        assert_eq!(execution2.status_code, 200);

        // Clean up
        let mut end2 = tunnel_protocol::encode_frame(
            req2_header.stream_id,
            tunnel_protocol::STREAM_END,
            0,
            &[],
        );
        tunnel_app.hub.handle_proxy_frame(900, &mut end2).await;
    }
}
