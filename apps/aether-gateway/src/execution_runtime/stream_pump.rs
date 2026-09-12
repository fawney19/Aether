use std::collections::{BTreeMap, VecDeque};
use std::future::Future;
use std::io::Error as IoError;
use std::time::{Duration, Instant};

use aether_contracts::{
    ExecutionError, ExecutionErrorKind, ExecutionPhase, ExecutionResponseObservation,
    ExecutionStreamTerminalSummary, ExecutionTelemetry, StreamFrame, StreamFramePayload,
    StreamFrameType,
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
use crate::execution_runtime::stream::ClientVisibleStreamCompletionTracker;
use crate::execution_runtime::stream_read_timeout::{
    await_stream_idle_read, stream_idle_timeout_message,
};
use crate::execution_runtime::transport::{
    append_upstream_response_body_chunk, decode_response_body_bytes,
    direct_upstream_response_byte_stream, stream_first_byte_timeout_message,
    DirectUpstreamResponse,
};
use crate::execution_runtime::DirectUpstreamStreamExecution;
use crate::GatewayError;

const STREAM_USAGE_OBSERVER_MAX_LINE_BYTES: usize = 1024 * 1024;
const UPSTREAM_STREAM_READ_ERROR_MESSAGE: &str = "Upstream response stream failed";

fn upstream_stream_error_category(response: &DirectUpstreamResponse) -> &'static str {
    match response {
        DirectUpstreamResponse::Reqwest(_) => "reqwest_body_read_failed",
        DirectUpstreamResponse::HyperH2c(_) => "hyper_body_read_failed",
        DirectUpstreamResponse::BrowserWreq(_) => "browser_body_read_failed",
        DirectUpstreamResponse::LocalTunnel(_) => "tunnel_body_read_failed",
    }
}

pub(crate) fn build_direct_execution_frame_stream(
    execution: DirectUpstreamStreamExecution,
) -> impl Stream<Item = Result<Bytes, IoError>> + Send + 'static {
    stream! {
        let DirectUpstreamStreamExecution {
            request_id: _,
            candidate_id: _,
            status_code,
            headers,
            upstream_content_length,
            provider_api_format,
            stream_summary_report_context,
            prefetched_body,
            stream_precommit_committed: _,
            response,
            started_at,
            response_observation,
            stream_first_byte_timeout,
            stream_idle_timeout,
            upstream_target_permit,
        } = execution;
        let _upstream_target_permit = upstream_target_permit;
        let upstream_error_category = upstream_stream_error_category(&response);

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
        let mut stream_completion = ClientVisibleStreamCompletionTracker::default();
        let mut observer_buffered = Vec::new();

        if should_buffer_non_stream_response(
            &headers,
            upstream_content_length,
            &observer_context,
        ) {
            let original_headers = headers.clone();
            match buffer_non_sse_upstream_body(
                prefetched_body,
                response,
                started_at,
                stream_first_byte_timeout,
                stream_idle_timeout,
            )
            .await
            {
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
                        Err(_err) => {
                            yield Err(IoError::other(
                                "Execution runtime stream conversion failed",
                            ));
                            return;
                        }
                    }

                    match encode_headers_frame(
                        status_code,
                        response_headers,
                        &response_observation,
                    ) {
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
                    first_byte_timeout,
                    idle_timeout,
                }) => {
                    match encode_headers_frame(
                        status_code,
                        original_headers,
                        &response_observation,
                    ) {
                        Ok(frame) => yield Ok(frame),
                        Err(err) => {
                            yield Err(err);
                            return;
                        }
                    }
                    let error_frame = if let Some(timeout) = first_byte_timeout {
                        encode_first_byte_timeout_frame(timeout)
                    } else if let Some(timeout) = idle_timeout {
                        encode_idle_timeout_frame(timeout)
                    } else {
                        encode_error_frame(message)
                    };
                    match error_frame {
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

        match encode_headers_frame(
            status_code,
            headers,
            &response_observation,
        ) {
            Ok(frame) => yield Ok(frame),
            Err(err) => {
                yield Err(err);
                return;
            }
        }

        let mut upstream_bytes = 0u64;
        let mut ttfb_ms = None;
        let mut first_chunk_telemetry_emitted = false;
        let mut prefetched_body_failed = false;
        for item in prefetched_body {
            match item {
                Ok(chunk) if chunk.is_empty() => continue,
                Ok(chunk) => {
                    stream_completion.observe_chunk(&chunk);
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
                Err(_message) => {
                    warn!(
                        event_name = "stream_pump_body_read_error",
                        log_type = "ops",
                        status_code,
                        upstream_bytes,
                        error_category = "prefetched_body_read_failed",
                        "upstream body stream read error"
                    );
                    match encode_error_frame(UPSTREAM_STREAM_READ_ERROR_MESSAGE.to_string()) {
                        Ok(frame) => yield Ok(frame),
                        Err(encode_err) => {
                            yield Err(encode_err);
                            return;
                        }
                    }
                    prefetched_body_failed = true;
                    break;
                }
            }
        }
        if !prefetched_body_failed {
        let mut bytes_stream = direct_upstream_response_byte_stream(VecDeque::new(), response);
        loop {
            let item = if ttfb_ms.is_none() {
                match await_stream_first_byte(
                    bytes_stream.next(), started_at, stream_first_byte_timeout,
                ).await {
                    Ok(item) => item,
                    Err(timeout) => {
                        match encode_first_byte_timeout_frame(timeout) {
                            Ok(frame) => yield Ok(frame),
                            Err(err) => yield Err(err),
                        }
                        break;
                    }
                }
            } else {
                match await_stream_idle_read(bytes_stream.next(), stream_idle_timeout).await {
                    Ok(item) => item,
                    Err(timeout) => {
                        drop(bytes_stream);
                        if stream_completion.successful_completion()
                            || (!stream_completion.observed_terminal()
                                && stream_terminal_observer.latest_summary().is_some_and(|summary| {
                                summary.observed_finish && summary.parser_error.is_none()
                                    && summary.finish_reason.as_deref() != Some("error")
                            }))
                        {
                            break;
                        }
                        if stream_terminal_observer.latest_summary().is_some_and(|summary| {
                            summary.observed_finish && summary.parser_error.is_some()
                        }) {
                            // The terminal summary carries the original provider failure.
                            break;
                        }
                        match encode_idle_timeout_frame(timeout) {
                            Ok(frame) => yield Ok(frame),
                            Err(err) => yield Err(err),
                        }
                        break;
                    }
                }
            };
            let Some(item) = item else { break };
            match item {
                Ok(chunk) => {
                    stream_completion.observe_chunk(&chunk);
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
                Err(_) => {
                    warn!(
                        event_name = "stream_pump_body_read_error",
                        log_type = "ops",
                        status_code,
                        upstream_bytes,
                        error_category = upstream_error_category,
                        "upstream body stream read error"
                    );
                    match encode_error_frame(UPSTREAM_STREAM_READ_ERROR_MESSAGE.to_string()) {
                        Ok(frame) => yield Ok(frame),
                        Err(err) => yield Err(err),
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
    response_observation: &ExecutionResponseObservation,
) -> Result<Bytes, IoError> {
    encode_stream_frame_ndjson(&StreamFrame {
        frame_type: StreamFrameType::Headers,
        payload: StreamFramePayload::Headers {
            status_code,
            headers,
            response_observation: Some(response_observation.clone()),
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

fn encode_error_frame(_message: String) -> Result<Bytes, IoError> {
    encode_stream_frame_ndjson(&StreamFrame {
        frame_type: StreamFrameType::Error,
        payload: StreamFramePayload::Error {
            error: ExecutionError {
                kind: ExecutionErrorKind::ProtocolError,
                phase: ExecutionPhase::StreamRead,
                message: UPSTREAM_STREAM_READ_ERROR_MESSAGE.to_string(),
                upstream_status: None,
                retryable: true,
                failover_recommended: true,
            },
        },
    })
}

fn encode_first_byte_timeout_frame(timeout: Duration) -> Result<Bytes, IoError> {
    encode_stream_frame_ndjson(&StreamFrame {
        frame_type: StreamFrameType::Error,
        payload: StreamFramePayload::Error {
            error: ExecutionError {
                kind: ExecutionErrorKind::FirstByteTimeout,
                phase: ExecutionPhase::FirstByte,
                message: stream_first_byte_timeout_message(timeout),
                upstream_status: None,
                retryable: true,
                failover_recommended: true,
            },
        },
    })
}

fn encode_idle_timeout_frame(timeout: Duration) -> Result<Bytes, IoError> {
    encode_stream_frame_ndjson(&StreamFrame {
        frame_type: StreamFrameType::Error,
        payload: StreamFramePayload::Error {
            error: ExecutionError {
                kind: ExecutionErrorKind::ReadTimeout,
                phase: ExecutionPhase::StreamRead,
                message: stream_idle_timeout_message(timeout),
                upstream_status: None,
                retryable: true,
                failover_recommended: true,
            },
        },
    })
}

async fn await_stream_first_byte<T, F>(
    future: F,
    started_at: Instant,
    timeout: Option<Duration>,
) -> Result<T, Duration>
where
    F: Future<Output = T>,
{
    let Some(timeout) = timeout else {
        return Ok(future.await);
    };
    let Some(remaining) = timeout.checked_sub(started_at.elapsed()) else {
        return Err(timeout);
    };
    if remaining.is_zero() {
        return Err(timeout);
    }
    tokio::time::timeout(remaining, future)
        .await
        .map_err(|_| timeout)
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
    first_byte_timeout: Option<Duration>,
    idle_timeout: Option<Duration>,
}

fn append_buffered_upstream_body_chunk(
    body_bytes: &mut Vec<u8>,
    chunk: &[u8],
    ttfb_ms: Option<u64>,
    upstream_bytes: &mut u64,
) -> Result<(), BufferedUpstreamBodyError> {
    *upstream_bytes = upstream_bytes.saturating_add(chunk.len() as u64);
    append_upstream_response_body_chunk(body_bytes, chunk).map_err(|error| {
        BufferedUpstreamBodyError {
            message: error.to_string(),
            ttfb_ms,
            upstream_bytes: *upstream_bytes,
            first_byte_timeout: None,
            idle_timeout: None,
        }
    })
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
    upstream_content_length: Option<u64>,
    report_context: &Value,
) -> bool {
    if should_treat_upstream_response_as_stream(headers, report_context) {
        return false;
    }

    if report_context
        .get("upstream_is_stream")
        .and_then(Value::as_bool)
        == Some(false)
    {
        return true;
    }

    // `content-length` is intentionally removed from the response header map
    // before it reaches the execution stream.  Retain its parsed value as
    // internal metadata so only a declared fixed-length JSON response is
    // converted to the client's SSE contract.
    if report_context
        .get("upstream_is_stream")
        .and_then(Value::as_bool)
        == Some(true)
        && upstream_content_length.is_some()
        && headers
            .get("content-type")
            .is_some_and(|value| value.to_ascii_lowercase().contains("json"))
    {
        return true;
    }

    headers
        .get("content-length")
        .and_then(|value| value.trim().parse::<u64>().ok())
        .is_some()
}

async fn buffer_non_sse_upstream_body(
    prefetched_body: VecDeque<Result<Bytes, String>>,
    response: DirectUpstreamResponse,
    started_at: Instant,
    stream_first_byte_timeout: Option<Duration>,
    stream_idle_timeout: Option<Duration>,
) -> Result<BufferedUpstreamBody, BufferedUpstreamBodyError> {
    let mut body_bytes = Vec::new();
    let mut upstream_bytes = 0u64;
    let mut ttfb_ms = None;
    let upstream_error_category = upstream_stream_error_category(&response);
    let mut bytes_stream = direct_upstream_response_byte_stream(prefetched_body, response);
    loop {
        let item = if ttfb_ms.is_none() {
            match await_stream_first_byte(
                bytes_stream.next(),
                started_at,
                stream_first_byte_timeout,
            )
            .await
            {
                Ok(item) => item,
                Err(timeout) => {
                    return Err(BufferedUpstreamBodyError {
                        message: stream_first_byte_timeout_message(timeout),
                        ttfb_ms,
                        upstream_bytes,
                        first_byte_timeout: Some(timeout),
                        idle_timeout: None,
                    })
                }
            }
        } else {
            match await_stream_idle_read(bytes_stream.next(), stream_idle_timeout).await {
                Ok(item) => item,
                Err(timeout) => {
                    return Err(BufferedUpstreamBodyError {
                        message: stream_idle_timeout_message(timeout),
                        ttfb_ms,
                        upstream_bytes,
                        first_byte_timeout: None,
                        idle_timeout: Some(timeout),
                    })
                }
            }
        };
        let Some(item) = item else { break };
        match item {
            Ok(chunk) => {
                if ttfb_ms.is_none() {
                    ttfb_ms = Some(started_at.elapsed().as_millis() as u64);
                }
                append_buffered_upstream_body_chunk(
                    &mut body_bytes,
                    &chunk,
                    ttfb_ms,
                    &mut upstream_bytes,
                )?;
            }
            Err(_) => {
                warn!(
                    event_name = "stream_pump_body_read_error",
                    log_type = "ops",
                    upstream_bytes,
                    error_category = upstream_error_category,
                    "upstream body stream read error"
                );
                return Err(BufferedUpstreamBodyError {
                    message: UPSTREAM_STREAM_READ_ERROR_MESSAGE.to_string(),
                    ttfb_ms,
                    upstream_bytes,
                    first_byte_timeout: None,
                    idle_timeout: None,
                });
            }
        }
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

    let decoded_body_bytes = decode_response_body_bytes(headers, body_bytes).map_err(|_error| {
        GatewayError::Internal("execution runtime response decode failed".to_string())
    })?;
    if !response_body_is_json(headers, decoded_body_bytes.as_ref()) {
        return Ok(None);
    }

    let body_json: Value = serde_json::from_slice(decoded_body_bytes.as_ref()).map_err(|_err| {
        GatewayError::Internal("execution runtime response JSON decode failed".to_string())
    })?;
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
            Err(_err) => {
                observer.disable_with_error("provider stream normalization failed");
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
            Err(_err) => observer.disable_with_error("provider stream normalization failed"),
        }
    }

    if !observer_buffered.is_empty() {
        let line = std::mem::take(observer_buffered);
        if let Err(_err) = observer.push_line(report_context, line) {
            observer.disable_with_error("stream usage parsing failed");
        }
    }

    match observer.finish(report_context) {
        Ok(summary) => summary,
        Err(_err) => {
            observer.disable_with_error("stream usage parsing failed");
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
    if normalized.is_empty()
        || observer
            .latest_summary()
            .and_then(|summary| summary.parser_error.as_deref())
            .is_some()
    {
        return;
    }

    let mut remaining = normalized;
    while !remaining.is_empty() {
        let line_part_len = remaining
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(remaining.len(), |index| index + 1);
        if observer_buffered.len().saturating_add(line_part_len)
            > STREAM_USAGE_OBSERVER_MAX_LINE_BYTES
        {
            observer.disable_with_error(format!(
                "stream usage event exceeded {STREAM_USAGE_OBSERVER_MAX_LINE_BYTES} bytes"
            ));
            observer_buffered.clear();
            return;
        }
        observer_buffered.extend_from_slice(&remaining[..line_part_len]);
        remaining = &remaining[line_part_len..];
        if observer_buffered.last() == Some(&b'\n') {
            let line = std::mem::take(observer_buffered);
            if let Err(_err) = observer.push_line(report_context, line) {
                observer.disable_with_error("stream usage parsing failed");
                observer_buffered.clear();
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::convert::Infallible;
    use std::sync::Arc;
    use std::time::Duration;

    use aether_contracts::tunnel_security::TUNNEL_SECURITY_NON_TLS_REQUIRED;
    use aether_contracts::{ExecutionPlan, ExecutionTimeouts, RequestBody};
    use aether_crypto::DEVELOPMENT_ENCRYPTION_KEY;
    use aether_data::repository::proxy_nodes::{InMemoryProxyNodeRepository, StoredProxyNode};
    use async_stream::stream;
    use axum::body::{Body, Bytes};
    use axum::extract::ws::Message;
    use axum::routing::post;
    use axum::{http::header, http::HeaderValue, Router};
    use base64::Engine as _;
    use futures_util::StreamExt;
    use serde_json::Value;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::watch;

    use super::{
        build_direct_execution_frame_stream, encode_error_frame, observe_normalized_bytes,
        should_buffer_non_stream_response, should_treat_upstream_response_as_stream,
        STREAM_USAGE_OBSERVER_MAX_LINE_BYTES, UPSTREAM_STREAM_READ_ERROR_MESSAGE,
    };
    use crate::ai_serving::api::StreamingStandardTerminalObserver;
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

    const LOCAL_TUNNEL_TEST_PSK: &str = "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=";
    const LOCAL_TUNNEL_TEST_GENERATION: &str = "stream-pump-test-generation-1";

    fn authenticated_local_tunnel_test_state() -> AppState {
        let node = StoredProxyNode::new(
            "node-1".to_string(),
            "Node 1".to_string(),
            "127.0.0.1".to_string(),
            0,
            false,
            "online".to_string(),
            30,
            1,
            0,
            0,
            0,
            0,
            true,
            true,
            1,
        )
        .expect("tunnel node should build")
        .with_runtime_fields(
            None,
            None,
            None,
            None,
            Some(serde_json::json!({
                "tunnel_security": {
                    "mode": TUNNEL_SECURITY_NON_TLS_REQUIRED,
                    "encryption_key": LOCAL_TUNNEL_TEST_PSK,
                }
            })),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .with_tunnel_generation(LOCAL_TUNNEL_TEST_GENERATION.to_string());
        let data = crate::data::GatewayDataState::with_proxy_node_repository_for_tests(Arc::new(
            InMemoryProxyNodeRepository::seed([node]),
        ))
        .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY);
        AppState::new()
            .expect("app state should build")
            .with_data_state_for_tests(data)
    }

    async fn recv_tunnel_test_frame(
        proxy_rx: &mut aether_runtime::BoundedQueueReceiver<Message>,
        description: &str,
    ) -> Message {
        tokio::time::timeout(Duration::from_secs(5), proxy_rx.recv())
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for {description}"))
            .unwrap_or_else(|| panic!("proxy channel closed before {description}"))
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
    fn buffers_declared_non_stream_responses_without_relying_on_content_length() {
        let streaming_context = serde_json::json!({
            "provider_api_format": "openai:chat",
            "client_api_format": "openai:chat",
            "upstream_is_stream": true,
        });
        let non_stream_context = serde_json::json!({
            "provider_api_format": "openai:image",
            "client_api_format": "openai:responses",
            "upstream_is_stream": false,
        });

        assert!(!should_buffer_non_stream_response(
            &BTreeMap::from([("content-type".into(), "application/json".into())]),
            None,
            &streaming_context
        ));
        assert!(should_buffer_non_stream_response(
            &BTreeMap::from([("content-type".into(), "application/json".into())]),
            None,
            &non_stream_context
        ));
        assert!(should_buffer_non_stream_response(
            &BTreeMap::from([
                ("content-type".into(), "application/json".into()),
                ("content-length".into(), "128".into()),
            ]),
            Some(128),
            &streaming_context
        ));
        assert!(!should_buffer_non_stream_response(
            &BTreeMap::from([("content-type".into(), "text/event-stream".into())]),
            None,
            &non_stream_context
        ));
    }

    #[test]
    fn error_frames_do_not_include_transport_details() {
        let secret = "Bearer stream-secret https://user:password@example.test/private";
        let frame = encode_error_frame(secret.to_string()).expect("error frame should encode");
        let frame = String::from_utf8(frame.to_vec()).expect("error frame should be utf8");

        assert!(frame.contains(UPSTREAM_STREAM_READ_ERROR_MESSAGE));
        assert!(!frame.contains(secret));
        assert!(!frame.contains("stream-secret"));
    }

    #[test]
    fn oversized_usage_line_disables_observation_without_retaining_the_line() {
        let mut observer = StreamingStandardTerminalObserver::default();
        let report_context = serde_json::json!({
            "provider_api_format": "claude:messages",
            "client_api_format": "claude:messages",
        });
        let mut buffered = Vec::new();
        let oversized = vec![b'x'; STREAM_USAGE_OBSERVER_MAX_LINE_BYTES + 1];

        observe_normalized_bytes(&mut observer, &report_context, &mut buffered, &oversized);

        assert!(buffered.is_empty());
        assert!(observer
            .latest_summary()
            .and_then(|summary| summary.parser_error.as_deref())
            .is_some_and(|error| error.contains("stream usage event exceeded")));
        observe_normalized_bytes(&mut observer, &report_context, &mut buffered, b"ignored");
        assert!(buffered.is_empty());
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
                transport_profile: None,
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
    async fn direct_execution_frame_stream_applies_first_byte_timeout_after_headers() {
        let listener = crate::test_support::bind_loopback_listener()
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("local addr should resolve");
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("client should connect");
            let mut request = [0_u8; 1024];
            let _ = socket
                .read(&mut request)
                .await
                .expect("request should read");
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n",
                )
                .await
                .expect("headers should write");
            socket.flush().await.expect("headers should flush");
            tokio::time::sleep(Duration::from_millis(200)).await;
            let _ = socket.write_all(b"d\r\ndata: hello\n\n\r\n0\r\n\r\n").await;
        });

        let execution = DirectSyncExecutionRuntime::new()
            .execute_stream(&ExecutionPlan {
                request_id: "req-stream-first-byte-timeout".into(),
                candidate_id: Some("cand-stream-first-byte-timeout".into()),
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
                transport_profile: None,
                timeouts: Some(ExecutionTimeouts {
                    first_byte_ms: Some(50),
                    total_ms: Some(5_000),
                    ..ExecutionTimeouts::default()
                }),
            })
            .await
            .expect("stream execution should receive response headers");

        let frames = build_direct_execution_frame_stream(execution)
            .map(|item| item.expect("frame should encode"))
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|bytes| String::from_utf8(bytes.to_vec()).expect("frame should be utf8"))
            .collect::<Vec<_>>();

        server.abort();

        let error_frame = frames
            .iter()
            .map(|line| serde_json::from_str::<Value>(line).expect("frame should parse"))
            .find(|frame| frame.get("type").and_then(Value::as_str) == Some("error"))
            .expect("timeout should emit an error frame");

        assert_eq!(
            error_frame
                .get("payload")
                .and_then(|payload| payload.get("error"))
                .and_then(|error| error.get("kind"))
                .and_then(Value::as_str),
            Some("first_byte_timeout")
        );
        assert!(error_frame
            .get("payload")
            .and_then(|payload| payload.get("error"))
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
            .is_some_and(
                |message| message.contains("provider stream first byte timeout after 50 ms")
            ));
        let error = error_frame
            .get("payload")
            .and_then(|payload| payload.get("error"))
            .expect("timeout error should exist");
        assert_eq!(error.get("upstream_status"), None);
        assert_eq!(error.get("retryable"), Some(&Value::Bool(true)));
        assert_eq!(error.get("failover_recommended"), Some(&Value::Bool(true)));
    }

    #[tokio::test]
    async fn direct_execution_frame_stream_enforces_idle_timeout_after_first_byte() {
        for (content_type, first_chunk, expect_timeout, provider_format) in [
            ("text/event-stream", "data: hello\n\n", true, "openai:chat"),
            ("application/json", "{\"message\":", true, "openai:chat"),
            ("text/event-stream", "data: [DONE]\n\n", false, "openai:chat"),
            ("text/event-stream", "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n", false, "openai:responses"),
            ("text/event-stream", "event: response.incomplete\ndata: {\"type\":\"response.incomplete\",\"response\":{}}\n\n", true, "openai:responses"),
        ] {
            let listener = crate::test_support::bind_loopback_listener().await.unwrap();
            let addr = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0_u8; 4096];
                assert!(socket.read(&mut request).await.unwrap() > 0);
                let response = if content_type == "application/json" {
                    format!("HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: 1024\r\n\r\n{first_chunk}")
                } else {
                    format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ntransfer-encoding: chunked\r\n\r\n{:x}\r\n{first_chunk}\r\n",
                        first_chunk.len(),
                    )
                };
                socket.write_all(response.as_bytes()).await.unwrap();
                socket.flush().await.unwrap();
                tokio::time::sleep(Duration::from_secs(5)).await;
            });
            let execution = DirectSyncExecutionRuntime::new()
                .execute_stream(&ExecutionPlan {
                    request_id: "req-stream-idle-timeout".into(),
                    candidate_id: Some("cand-stream-idle-timeout".into()),
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
                    provider_api_format: provider_format.into(),
                    model_name: Some("gpt-5".into()),
                    proxy: None,
                    transport_profile: None,
                    timeouts: Some(ExecutionTimeouts {
                        first_byte_ms: Some(1_000),
                        read_ms: Some(10),
                        ..ExecutionTimeouts::default()
                    }),
                })
                .await
                .expect("stream response headers");
            let frames = tokio::time::timeout(
                Duration::from_secs(1),
                build_direct_execution_frame_stream(execution).collect::<Vec<_>>(),
            )
            .await;
            server.abort();
            let frames = frames
                .expect("idle timeout must terminate both SSE and buffered JSON")
                .into_iter()
                .map(|line| serde_json::from_slice::<Value>(&line.unwrap()).unwrap())
                .collect::<Vec<_>>();
            let errors = frames
                .iter()
                .filter(|frame| frame["type"] == "error")
                .collect::<Vec<_>>();
            assert_eq!(errors.len(), usize::from(expect_timeout));
            if expect_timeout {
                assert_eq!(errors[0]["payload"]["error"]["kind"], "read_timeout");
                assert_eq!(errors[0]["payload"]["error"]["phase"], "stream_read");
            }
            assert!(frames.iter().any(|frame| frame["type"] == "eof"));
            assert_eq!(
                frames.iter().any(|frame| frame["type"] == "data"),
                content_type == "text/event-stream"
            );
        }
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
                transport_profile: None,
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
            let (mut socket, _) = listener.accept().await.expect("client should connect");
            let mut request = [0_u8; 4096];
            let _ = socket
                .read(&mut request)
                .await
                .expect("request should read");
            let body = serde_json::to_vec(&serde_json::json!({
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
            }))
            .expect("json should encode");
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .expect("headers should write");
            socket.flush().await.expect("headers should flush");
            tokio::time::sleep(Duration::from_millis(75)).await;
            socket.write_all(&body).await.expect("body should write");
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
                transport_profile: None,
                timeouts: Some(ExecutionTimeouts {
                    connect_ms: Some(5_000),
                    total_ms: Some(5_000),
                    ..ExecutionTimeouts::default()
                }),
            })
            .await
            .expect("stream execution should succeed");
        let expected_observation = execution.response_observation.clone();
        assert!(
            expected_observation.response_headers_observed_at_unix_ms
                >= expected_observation.request_started_at_unix_ms
        );
        assert!(!expected_observation.request_order_id.is_empty());

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
        let encoded_observation: aether_contracts::ExecutionResponseObservation =
            serde_json::from_value(header_frame["payload"]["response_observation"].clone())
                .expect("headers frame should retain the response observation");
        assert_eq!(encoded_observation, expected_observation);
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
        let generated_image = "a".repeat(32 * 1024);
        let expected_image = generated_image.clone();
        let server = tokio::spawn(async move {
            let app = Router::new().route(
                "/images/generations",
                post(move || {
                    let generated_image = generated_image.clone();
                    async move {
                        let body = serde_json::json!({
                            "created": 1776971267_u64,
                            "data": [{
                                "b64_json": generated_image
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
                        let encoded = serde_json::to_vec(&body).expect("json should encode");
                        let chunks = encoded
                            .chunks(4096)
                            .map(Bytes::copy_from_slice)
                            .collect::<Vec<_>>();
                        let chunked_body = stream! {
                            for chunk in chunks {
                                yield Ok::<Bytes, Infallible>(chunk);
                                tokio::task::yield_now().await;
                            }
                        };
                        let mut response =
                            axum::http::Response::new(Body::from_stream(chunked_body));
                        response.headers_mut().insert(
                            header::CONTENT_TYPE,
                            HeaderValue::from_static("application/json"),
                        );
                        response
                    }
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
                url: format!("http://{addr}/images/generations"),
                headers: BTreeMap::new(),
                content_type: None,
                content_encoding: None,
                body: RequestBody::from_json(serde_json::json!({
                    "model": "gpt-image-1",
                    "prompt": "poster"
                })),
                stream: false,
                client_api_format: "openai:image".to_string(),
                provider_api_format: "openai:image".to_string(),
                model_name: Some("gpt-image-1".into()),
                proxy: None,
                transport_profile: None,
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
        assert!(bridged_text.contains(&format!("\"b64_json\":\"{expected_image}\"")));
        assert!(bridged_text.len() > 32 * 1024);
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
    async fn direct_execution_frame_stream_sanitizes_local_tunnel_stream_error_message() {
        let state = authenticated_local_tunnel_test_state();
        let tunnel_app = state.tunnel.app_state();
        let (proxy_tx, mut proxy_rx) = aether_runtime::bounded_queue(8);
        let (proxy_close_tx, _) = watch::channel(false);
        tunnel_app.hub.register_proxy(Arc::new(
            TunnelProxyConn::new(
                801,
                "node-1".to_string(),
                "Node 1".to_string(),
                proxy_tx,
                proxy_close_tx,
                16,
                2,
            )
            .with_tunnel_generation(LOCAL_TUNNEL_TEST_GENERATION.to_string())
            .with_authenticated_key(LOCAL_TUNNEL_TEST_PSK.to_string()),
        ));

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
            transport_profile: None,
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

        let request_headers = match recv_tunnel_test_frame(&mut proxy_rx, "headers frame").await {
            Message::Binary(data) => data,
            other => panic!("unexpected message: {other:?}"),
        };
        let request_header = tunnel_protocol::FrameHeader::parse(&request_headers)
            .expect("request header frame should parse");
        assert_eq!(request_header.msg_type, tunnel_protocol::REQUEST_HEADERS);

        let request_body = match recv_tunnel_test_frame(&mut proxy_rx, "body frame").await {
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

        assert_eq!(error_message, UPSTREAM_STREAM_READ_ERROR_MESSAGE);
        assert!(!error_message.contains(original_error));
    }

    #[tokio::test]
    async fn second_local_tunnel_request_works_after_first_completes() {
        let state = authenticated_local_tunnel_test_state();
        let tunnel_app = state.tunnel.app_state();
        let (proxy_tx, mut proxy_rx) = aether_runtime::bounded_queue(8);
        let (proxy_close_tx, _) = watch::channel(false);
        tunnel_app.hub.register_proxy(Arc::new(
            TunnelProxyConn::new(
                900,
                "node-1".to_string(),
                "Node 1".to_string(),
                proxy_tx,
                proxy_close_tx,
                16,
                2,
            )
            .with_tunnel_generation(LOCAL_TUNNEL_TEST_GENERATION.to_string())
            .with_authenticated_key(LOCAL_TUNNEL_TEST_PSK.to_string()),
        ));

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
            transport_profile: None,
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
        let req1_headers = match recv_tunnel_test_frame(&mut proxy_rx, "req1 headers").await {
            Message::Binary(data) => data,
            other => panic!("unexpected: {other:?}"),
        };
        let req1_header =
            tunnel_protocol::FrameHeader::parse(&req1_headers).expect("req1 header parse");
        let _req1_body = recv_tunnel_test_frame(&mut proxy_rx, "req1 body").await;

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
        let req2_headers = recv_tunnel_test_frame(&mut proxy_rx, "req2 headers").await;
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
