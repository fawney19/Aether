use std::collections::BTreeMap;
use std::future::Future;
use std::io::Error as IoError;
use std::net::IpAddr;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use aether_contracts::{
    ExecutionPlan, ExecutionResult, ExecutionStreamTerminalSummary, ExecutionTelemetry,
    RequestBody, ResponseBody, StandardizedUsage, StreamFrame, StreamFramePayload, StreamFrameType,
    EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER,
};
use axum::body::Bytes;
use base64::Engine as _;
use futures_util::stream::{self, BoxStream};
use futures_util::StreamExt;
use http::{HeaderMap, HeaderName, HeaderValue};
use http_body_util::BodyExt;
use regex::{Captures, Regex};
use serde_json::{json, Map, Value};
use uuid::Uuid;
use wreq::ws::message::Message as WreqWsMessage;

use crate::ai_serving::api::{
    convert_standard_chat_response, maybe_bridge_standard_sync_json_to_stream,
    CanonicalContentPart, CanonicalStreamEvent, CanonicalStreamFrame, ClaudeClientEmitter,
    OpenAIChatClientEmitter, OpenAIResponsesClientEmitter, StreamingCanonicalUsage,
};
use crate::clock::current_unix_secs;
use crate::execution_runtime::ndjson::encode_stream_frame_ndjson;
use crate::execution_runtime::transport::{
    build_browser_wreq_client, build_request_body, build_request_headers,
    decode_response_body_bytes, format_hyper_error_chain, format_upstream_request_error,
    format_wreq_upstream_request_error, resolve_stream_first_byte_timeout, send_request,
    stream_first_byte_timeout_message, with_non_stream_total_timeout, DirectHttpResponse,
    ExecutionRuntimeTransportError, ExecutionTransportControls,
};

const GROK_INTERNAL_HEADER: &str = "x-aether-grok-runtime";
const GROK_ASSET_BASE: &str = "https://assets.grok.com/";
const GROK_UPLOAD_PATH: &str = "/rest/app-chat/upload-file";
const GROK_MEDIA_POST_PATH: &str = "/rest/media/post/create";
const GROK_IMAGINE_WS_URL: &str = "wss://grok.com/ws/imagine/listen";
const GROK_STANDARD_PROVIDER_API_FORMAT: &str = "openai:responses";
const GROK_PROMPT_OVERHEAD_TOKENS: u64 = 4;
const GROK_MAX_ATTACHMENT_BYTES: usize = 25 * 1024 * 1024;
const GROK_MAX_ATTACHMENT_REDIRECTS: usize = 5;
const GROK_IMAGINE_STREAM_TIMEOUT_MS: u64 = 10_000;
const GROK_IMAGINE_ROUND_TIMEOUT_MS: u64 = 120_000;

static GROK_RENDER_RE: OnceLock<Regex> = OnceLock::new();

fn grok_render_regex() -> &'static Regex {
    GROK_RENDER_RE.get_or_init(|| {
        Regex::new(
        r#"(?s)<grok:render\s+card_id="([^"]+)"\s+card_type="([^"]+)"\s+type="([^"]+)"[^>]*>.*?</grok:render>"#,
    )
        .expect("Grok render regex should compile")
    })
}

pub(crate) struct GrokRuntimeStream {
    pub(crate) frame_stream: BoxStream<'static, Result<Bytes, IoError>>,
    pub(crate) report_context: Option<Value>,
}

#[derive(Debug)]
struct GrokCollected {
    status_code: u16,
    headers: BTreeMap<String, String>,
    text: String,
    thinking: String,
    images: Vec<String>,
    telemetry: ExecutionTelemetry,
}

#[derive(Debug, Clone)]
struct GrokImagineImage {
    image_id: String,
    order: usize,
    url: Option<String>,
    blob_b64: Option<String>,
    done: bool,
    moderated: bool,
}

impl Default for GrokCollected {
    fn default() -> Self {
        Self {
            status_code: 0,
            headers: BTreeMap::new(),
            text: String::new(),
            thinking: String::new(),
            images: Vec::new(),
            telemetry: ExecutionTelemetry {
                ttfb_ms: None,
                elapsed_ms: None,
                upstream_bytes: None,
            },
        }
    }
}

#[derive(Debug, Default)]
struct GrokStreamAdapter {
    buffered: String,
    text: String,
    thinking: String,
    images: Vec<String>,
    cards: BTreeMap<String, GrokCard>,
    citation_order: Vec<String>,
    last_citation_index: Option<usize>,
}

#[derive(Debug, Clone)]
struct GrokCard {
    url: Option<String>,
    title: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct GrokUsageEstimate {
    input_tokens: u64,
    output_tokens: u64,
    reasoning_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GrokAttachmentInput {
    source: String,
    filename: Option<String>,
    mime_type: Option<String>,
}

#[derive(Debug)]
struct GrokAttachmentPayload {
    filename: String,
    mime_type: String,
    content_b64: String,
}

#[derive(Debug, Clone)]
struct GrokUploadedAttachment {
    file_id: String,
    file_uri: Option<String>,
}

pub(crate) async fn maybe_execute_grok_sync(
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
) -> Result<Option<ExecutionResult>, ExecutionRuntimeTransportError> {
    if !is_grok_plan(plan, report_context) {
        return Ok(None);
    }
    with_non_stream_total_timeout(plan, async move {
        let mut collected = execute_grok_app_chat(plan, report_context).await?;
        materialize_grok_image_assets(plan, &mut collected).await;
        Ok(Some(grok_execution_result(plan, collected, report_context)))
    })
    .await
}

pub(crate) async fn maybe_execute_grok_stream(
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
) -> Result<Option<GrokRuntimeStream>, ExecutionRuntimeTransportError> {
    if !is_grok_plan(plan, report_context) {
        return Ok(None);
    }
    if grok_should_collect_image_stream(plan, report_context)? {
        let collected = execute_grok_app_chat(plan, report_context).await?;
        return Ok(Some(GrokRuntimeStream {
            frame_stream: grok_collected_frame_stream(plan.clone(), collected, report_context),
            report_context: report_context.cloned(),
        }));
    }
    Ok(Some(
        execute_grok_app_chat_stream(plan, report_context).await?,
    ))
}

fn is_grok_plan(plan: &ExecutionPlan, report_context: Option<&Value>) -> bool {
    let header_marker = plan
        .headers
        .iter()
        .any(|(name, value)| name.eq_ignore_ascii_case(GROK_INTERNAL_HEADER) && value == "1");
    let context_marker = report_context
        .and_then(|value| value.get("provider_type"))
        .and_then(Value::as_str)
        .map(|value| value.eq_ignore_ascii_case("grok"))
        .unwrap_or(false);
    header_marker || context_marker
}

async fn execute_grok_app_chat(
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
) -> Result<GrokCollected, ExecutionRuntimeTransportError> {
    if grok_should_use_imagine_websocket(plan, report_context)? {
        return execute_grok_imagine_websocket(plan, report_context).await;
    }
    let upstream_plan = grok_upstream_plan(plan, report_context).await?;
    let request_body = build_request_body(&upstream_plan)?;
    let started_at = Instant::now();
    let response = send_request(&upstream_plan, request_body).await?;
    let ttfb_ms = started_at.elapsed().as_millis() as u64;
    let status_code = response.status_code();
    let headers = response.headers();
    let mut upstream_bytes = 0u64;
    let mut raw_body = Vec::new();
    let mut adapter = GrokStreamAdapter::default();
    collect_grok_response_stream(
        response,
        status_code,
        &mut upstream_bytes,
        &mut raw_body,
        &mut adapter,
    )
    .await?;
    if (200..300).contains(&status_code) {
        adapter.finish();
    }

    let elapsed_ms = started_at.elapsed().as_millis() as u64;
    if !(200..300).contains(&status_code) {
        let decoded = decode_response_body_bytes(&headers, &raw_body).unwrap_or(raw_body);
        let text = String::from_utf8_lossy(&decoded).to_string();
        return Ok(GrokCollected {
            status_code,
            headers,
            text,
            telemetry: ExecutionTelemetry {
                ttfb_ms: Some(ttfb_ms),
                elapsed_ms: Some(elapsed_ms),
                upstream_bytes: Some(upstream_bytes),
            },
            ..GrokCollected::default()
        });
    }

    Ok(GrokCollected {
        status_code,
        headers,
        text: adapter.text,
        thinking: adapter.thinking,
        images: adapter.images,
        telemetry: ExecutionTelemetry {
            ttfb_ms: Some(ttfb_ms),
            elapsed_ms: Some(elapsed_ms),
            upstream_bytes: Some(upstream_bytes),
        },
    })
}

async fn execute_grok_app_chat_stream(
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
) -> Result<GrokRuntimeStream, ExecutionRuntimeTransportError> {
    let upstream_plan = grok_upstream_plan(plan, report_context).await?;
    let request_body = build_request_body(&upstream_plan)?;
    let started_at = Instant::now();
    let response = send_request(&upstream_plan, request_body).await?;
    let status_code = response.status_code();
    let headers = response.headers();
    if !(200..300).contains(&status_code) {
        let mut upstream_bytes = 0u64;
        let mut raw_body = Vec::new();
        let mut adapter = GrokStreamAdapter::default();
        collect_grok_response_stream(
            response,
            status_code,
            &mut upstream_bytes,
            &mut raw_body,
            &mut adapter,
        )
        .await?;
        let decoded = decode_response_body_bytes(&headers, &raw_body).unwrap_or(raw_body);
        let text = String::from_utf8_lossy(&decoded).to_string();
        let elapsed_ms = started_at.elapsed().as_millis() as u64;
        let collected = GrokCollected {
            status_code,
            headers,
            text,
            telemetry: ExecutionTelemetry {
                ttfb_ms: Some(elapsed_ms),
                elapsed_ms: Some(elapsed_ms),
                upstream_bytes: Some(upstream_bytes),
            },
            ..GrokCollected::default()
        };
        return Ok(GrokRuntimeStream {
            frame_stream: grok_collected_frame_stream(plan.clone(), collected, report_context),
            report_context: report_context.cloned(),
        });
    }

    Ok(GrokRuntimeStream {
        frame_stream: grok_success_frame_stream(
            plan.clone(),
            status_code,
            headers,
            started_at,
            grok_response_body_stream(response),
        ),
        report_context: report_context.cloned(),
    })
}

fn grok_should_use_imagine_websocket(
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
) -> Result<bool, ExecutionRuntimeTransportError> {
    let client_format = normalized_client_api_format(plan);
    if !matches!(
        client_format.as_str(),
        "openai:image" | "openai:responses" | "openai:responses:compact" | "openai:chat"
    ) {
        return Ok(false);
    }
    let mapped_model = grok_upstream_model_name(report_context)?;
    let model = mapped_model.to_ascii_lowercase();
    Ok(model.contains("grok-imagine-image") && !model.contains("lite") && !model.contains("edit"))
}

fn grok_should_collect_image_stream(
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
) -> Result<bool, ExecutionRuntimeTransportError> {
    if normalized_client_api_format(plan) == "openai:image" {
        return Ok(true);
    }
    if grok_plan_uses_structured_image_generation(plan, report_context) {
        return Ok(true);
    }
    grok_should_use_imagine_websocket(plan, report_context)
}

async fn execute_grok_imagine_websocket(
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
) -> Result<GrokCollected, ExecutionRuntimeTransportError> {
    let body = plan.body.json_body.as_ref().ok_or_else(|| {
        ExecutionRuntimeTransportError::UpstreamRequest(
            "Grok Imagine requires JSON request body".to_string(),
        )
    })?;
    let prompt = grok_image_prompt_from_provider_body(body).ok_or_else(|| {
        ExecutionRuntimeTransportError::UpstreamRequest(
            "Grok Imagine requires a non-empty prompt".to_string(),
        )
    })?;
    let requested = grok_image_count_from_provider_body(body).clamp(1, 4);
    let enable_pro = grok_upstream_model_name(report_context)?
        .to_ascii_lowercase()
        .contains("pro");
    let aspect_ratio = grok_aspect_ratio_from_provider_body(body);
    let started_at = Instant::now();
    let mut images =
        grok_imagine_websocket_images(plan, &prompt, requested, &aspect_ratio, enable_pro).await?;
    images.sort_by_key(|image| image.order);
    Ok(GrokCollected {
        status_code: 200,
        headers: BTreeMap::from([("content-type".to_string(), "application/json".to_string())]),
        images: images
            .into_iter()
            .filter_map(|image| {
                image
                    .url
                    .or_else(|| image.blob_b64.map(grok_data_image_url))
            })
            .collect(),
        telemetry: ExecutionTelemetry {
            ttfb_ms: None,
            elapsed_ms: Some(started_at.elapsed().as_millis() as u64),
            upstream_bytes: None,
        },
        ..GrokCollected::default()
    })
}

async fn grok_imagine_websocket_images(
    plan: &ExecutionPlan,
    prompt: &str,
    requested: usize,
    aspect_ratio: &str,
    enable_pro: bool,
) -> Result<Vec<GrokImagineImage>, ExecutionRuntimeTransportError> {
    let headers = build_request_headers(&plan.headers, None, false)?;
    let profile = plan.transport_profile.as_ref().ok_or_else(|| {
        ExecutionRuntimeTransportError::UnsupportedTransportProfile("browser_wreq".to_string())
    })?;
    let client = build_browser_wreq_client(
        plan.timeouts.as_ref(),
        plan.proxy.as_ref(),
        profile,
        ExecutionTransportControls::default(),
        true,
    )?;
    let response = client
        .websocket(GROK_IMAGINE_WS_URL)
        .headers(headers)
        .max_frame_size(64 << 20)
        .max_message_size(64 << 20)
        .send()
        .await
        .map_err(|err| {
            ExecutionRuntimeTransportError::UpstreamRequest(format_wreq_upstream_request_error(
                &err,
            ))
        })?;
    let status = response.status();
    if !status.is_success() && status.as_u16() != 101 {
        return Err(ExecutionRuntimeTransportError::UpstreamRequest(format!(
            "Grok Imagine websocket returned {}",
            status.as_u16()
        )));
    }
    let mut websocket = response.into_websocket().await.map_err(|err| {
        ExecutionRuntimeTransportError::UpstreamRequest(format_wreq_upstream_request_error(&err))
    })?;
    let reset = grok_imagine_reset_message();
    websocket
        .send(WreqWsMessage::text(reset.to_string()))
        .await
        .map_err(|err| {
            ExecutionRuntimeTransportError::UpstreamRequest(format_wreq_upstream_request_error(
                &err,
            ))
        })?;
    let request = grok_imagine_request_message(prompt, aspect_ratio, enable_pro);
    websocket
        .send(WreqWsMessage::text(request.to_string()))
        .await
        .map_err(|err| {
            ExecutionRuntimeTransportError::UpstreamRequest(format_wreq_upstream_request_error(
                &err,
            ))
        })?;

    let deadline = Instant::now() + Duration::from_millis(GROK_IMAGINE_ROUND_TIMEOUT_MS);
    let mut slots: BTreeMap<String, GrokImagineImage> = BTreeMap::new();
    while Instant::now() < deadline {
        if grok_imagine_completed_count(&slots) >= requested {
            break;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        let timeout = remaining.min(Duration::from_millis(GROK_IMAGINE_STREAM_TIMEOUT_MS));
        let Some(message) = tokio::time::timeout(timeout, websocket.recv())
            .await
            .map_err(|_| {
                ExecutionRuntimeTransportError::UpstreamRequest(
                    "Grok Imagine websocket timed out waiting for image frames".to_string(),
                )
            })?
        else {
            break;
        };
        let message = message.map_err(|err| {
            ExecutionRuntimeTransportError::UpstreamRequest(format_wreq_upstream_request_error(
                &err,
            ))
        })?;
        let WreqWsMessage::Text(text) = message else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(text.as_str()) else {
            continue;
        };
        grok_handle_imagine_ws_message(&value, &mut slots)?;
    }

    let mut images = slots
        .into_values()
        .filter(|image| {
            image.done && !image.moderated && (image.url.is_some() || image.blob_b64.is_some())
        })
        .collect::<Vec<_>>();
    images.sort_by_key(|image| image.order);
    images.truncate(requested);
    if images.is_empty() {
        return Err(ExecutionRuntimeTransportError::UpstreamRequest(
            "Grok Imagine returned no images".to_string(),
        ));
    }
    Ok(images)
}

async fn collect_grok_response_stream(
    response: DirectHttpResponse,
    status_code: u16,
    upstream_bytes: &mut u64,
    raw_body: &mut Vec<u8>,
    adapter: &mut GrokStreamAdapter,
) -> Result<(), ExecutionRuntimeTransportError> {
    match response {
        DirectHttpResponse::Reqwest(response) => {
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|err| {
                    ExecutionRuntimeTransportError::UpstreamRequest(format_upstream_request_error(
                        &err,
                    ))
                })?;
                collect_grok_response_chunk(status_code, upstream_bytes, raw_body, adapter, &chunk);
            }
        }
        DirectHttpResponse::HyperH2c(response) => {
            let mut stream = response.into_body().into_data_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|err| {
                    ExecutionRuntimeTransportError::UpstreamRequest(format_hyper_error_chain(&err))
                })?;
                collect_grok_response_chunk(status_code, upstream_bytes, raw_body, adapter, &chunk);
            }
        }
        DirectHttpResponse::BrowserWreq(response) => {
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|err| {
                    ExecutionRuntimeTransportError::UpstreamRequest(
                        format_wreq_upstream_request_error(&err),
                    )
                })?;
                collect_grok_response_chunk(status_code, upstream_bytes, raw_body, adapter, &chunk);
            }
        }
    }
    Ok(())
}

fn collect_grok_response_chunk(
    status_code: u16,
    upstream_bytes: &mut u64,
    raw_body: &mut Vec<u8>,
    adapter: &mut GrokStreamAdapter,
    chunk: &[u8],
) {
    *upstream_bytes += chunk.len() as u64;
    raw_body.extend_from_slice(chunk);
    if (200..300).contains(&status_code) {
        adapter.push_chunk(chunk);
    }
}

type GrokUpstreamBodyStream = BoxStream<'static, Result<Bytes, String>>;

fn grok_response_body_stream(response: DirectHttpResponse) -> GrokUpstreamBodyStream {
    match response {
        DirectHttpResponse::Reqwest(response) => response
            .bytes_stream()
            .map(|chunk| {
                chunk.map_err(|err| {
                    ExecutionRuntimeTransportError::UpstreamRequest(format_upstream_request_error(
                        &err,
                    ))
                    .to_string()
                })
            })
            .boxed(),
        DirectHttpResponse::HyperH2c(response) => response
            .into_body()
            .into_data_stream()
            .map(|chunk| {
                chunk.map_err(|err| {
                    ExecutionRuntimeTransportError::UpstreamRequest(format_hyper_error_chain(&err))
                        .to_string()
                })
            })
            .boxed(),
        DirectHttpResponse::BrowserWreq(response) => response
            .bytes_stream()
            .map(|chunk| {
                chunk.map_err(|err| {
                    ExecutionRuntimeTransportError::UpstreamRequest(
                        format_wreq_upstream_request_error(&err),
                    )
                    .to_string()
                })
            })
            .boxed(),
    }
}

fn grok_success_frame_stream(
    plan: ExecutionPlan,
    status_code: u16,
    headers: BTreeMap<String, String>,
    started_at: Instant,
    mut body_stream: GrokUpstreamBodyStream,
) -> BoxStream<'static, Result<Bytes, IoError>> {
    let stream_first_byte_timeout = resolve_stream_first_byte_timeout(&plan);
    async_stream::stream! {
        match encode_grok_headers_frame(
            status_code,
            BTreeMap::from([("content-type".to_string(), "text/event-stream".to_string())]),
        ) {
            Ok(frame) => yield Ok(frame),
            Err(err) => {
                yield Err(err);
                return;
            }
        }

        let mut adapter = GrokStreamAdapter::default();
        let mut client_emitter = GrokClientStreamEmitter::new(&plan);
        let mut upstream_bytes = 0u64;
        let mut ttfb_ms = None;
        let mut first_chunk_telemetry_emitted = false;
        let mut text_len = 0usize;
        let mut thinking_len = 0usize;
        let mut image_len = 0usize;
        let mut terminal_error_emitted = false;

        loop {
            let item = if ttfb_ms.is_none() {
                match await_grok_stream_first_byte(
                    body_stream.next(),
                    started_at,
                    stream_first_byte_timeout,
                )
                .await
                {
                    Ok(item) => item,
                    Err(timeout) => {
                        match encode_grok_first_byte_timeout_frame(timeout) {
                            Ok(frame) => yield Ok(frame),
                            Err(err) => {
                                yield Err(err);
                                return;
                            }
                        }
                        terminal_error_emitted = true;
                        break;
                    }
                }
            } else {
                body_stream.next().await
            };
            let Some(item) = item else {
                break;
            };
            let chunk = match item {
                Ok(chunk) => chunk,
                Err(message) => {
                    match encode_grok_error_frame(status_code, message) {
                        Ok(frame) => yield Ok(frame),
                        Err(err) => {
                            yield Err(err);
                            return;
                        }
                    }
                    terminal_error_emitted = true;
                    break;
                }
            };
            if ttfb_ms.is_none() {
                ttfb_ms = Some(started_at.elapsed().as_millis() as u64);
            }
            if !first_chunk_telemetry_emitted {
                match encode_grok_telemetry_frame(ttfb_ms, ttfb_ms, upstream_bytes) {
                    Ok(frame) => yield Ok(frame),
                    Err(err) => {
                        yield Err(err);
                        return;
                    }
                }
                first_chunk_telemetry_emitted = true;
            }
            upstream_bytes += chunk.len() as u64;
            adapter.push_chunk(&chunk);
            match emit_grok_adapter_deltas(
                &mut client_emitter,
                &adapter,
                &mut text_len,
                &mut thinking_len,
                &mut image_len,
            ) {
                Ok(frames) => {
                    for frame in frames {
                        yield Ok(frame);
                    }
                }
                Err(err) => {
                    yield Err(err);
                    return;
                }
            }
        }

        if terminal_error_emitted {
            match encode_grok_telemetry_frame(
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
            return;
        }

        adapter.finish();
        match emit_grok_adapter_deltas(
            &mut client_emitter,
            &adapter,
            &mut text_len,
            &mut thinking_len,
            &mut image_len,
        ) {
            Ok(frames) => {
                for frame in frames {
                    yield Ok(frame);
                }
            }
            Err(err) => {
                yield Err(err);
                return;
            }
        }

        let elapsed_ms = Some(started_at.elapsed().as_millis() as u64);
        let collected = GrokCollected {
            status_code,
            headers,
            text: adapter.text,
            thinking: adapter.thinking,
            images: adapter.images,
            telemetry: ExecutionTelemetry {
                ttfb_ms,
                elapsed_ms,
                upstream_bytes: Some(upstream_bytes),
            },
        };
        let usage = grok_usage_estimate(&plan, &collected);
        match emit_grok_client_bytes(client_emitter.finish(usage)) {
            Ok(frames) => {
                for frame in frames {
                    yield Ok(frame);
                }
            }
            Err(err) => {
                yield Err(err);
                return;
            }
        }
        match encode_grok_telemetry_frame(ttfb_ms, elapsed_ms, upstream_bytes) {
            Ok(frame) => yield Ok(frame),
            Err(err) => {
                yield Err(err);
                return;
            }
        }
        match encode_stream_frame_ndjson(&StreamFrame::eof_with_summary(Some(
            grok_stream_terminal_summary(&plan, usage),
        ))) {
            Ok(frame) => yield Ok(frame),
            Err(err) => yield Err(err),
        }
    }
    .boxed()
}

async fn await_grok_stream_first_byte<T, F>(
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

fn emit_grok_adapter_deltas(
    client_emitter: &mut GrokClientStreamEmitter,
    adapter: &GrokStreamAdapter,
    text_len: &mut usize,
    thinking_len: &mut usize,
    image_len: &mut usize,
) -> Result<Vec<Bytes>, IoError> {
    let mut out = Vec::new();
    if let Some(delta) = adapter.thinking.get(*thinking_len..) {
        if !delta.is_empty() {
            out.extend(emit_grok_client_bytes(
                client_emitter.emit_reasoning_delta(delta.to_string()),
            )?);
        }
    }
    *thinking_len = adapter.thinking.len();
    if let Some(delta) = adapter.text.get(*text_len..) {
        if !delta.is_empty() {
            out.extend(emit_grok_client_bytes(
                client_emitter.emit_text_delta(delta.to_string()),
            )?);
        }
    }
    *text_len = adapter.text.len();
    for image in adapter.images.iter().skip(*image_len) {
        out.extend(emit_grok_client_bytes(
            client_emitter.emit_image_url(image.clone()),
        )?);
    }
    *image_len = adapter.images.len();
    Ok(out)
}

fn emit_grok_client_bytes(
    body: Result<Vec<u8>, ExecutionRuntimeTransportError>,
) -> Result<Vec<Bytes>, IoError> {
    let body = body.map_err(|err| IoError::other(err.to_string()))?;
    if body.is_empty() {
        return Ok(Vec::new());
    }
    encode_grok_data_frame(&Bytes::from(body)).map(|frame| vec![frame])
}

fn encode_grok_headers_frame(
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

fn encode_grok_data_frame(chunk: &Bytes) -> Result<Bytes, IoError> {
    encode_stream_frame_ndjson(&StreamFrame {
        frame_type: StreamFrameType::Data,
        payload: StreamFramePayload::Data {
            chunk_b64: Some(base64::engine::general_purpose::STANDARD.encode(chunk)),
            text: None,
        },
    })
}

fn encode_grok_telemetry_frame(
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

fn encode_grok_error_frame(status_code: u16, message: String) -> Result<Bytes, IoError> {
    encode_stream_frame_ndjson(&StreamFrame {
        frame_type: StreamFrameType::Error,
        payload: StreamFramePayload::Error {
            error: aether_contracts::ExecutionError {
                kind: aether_contracts::ExecutionErrorKind::Internal,
                phase: aether_contracts::ExecutionPhase::StreamRead,
                message,
                upstream_status: Some(status_code),
                retryable: false,
                failover_recommended: false,
            },
        },
    })
}

fn encode_grok_first_byte_timeout_frame(timeout: Duration) -> Result<Bytes, IoError> {
    encode_stream_frame_ndjson(&StreamFrame {
        frame_type: StreamFrameType::Error,
        payload: StreamFramePayload::Error {
            error: aether_contracts::ExecutionError {
                kind: aether_contracts::ExecutionErrorKind::FirstByteTimeout,
                phase: aether_contracts::ExecutionPhase::FirstByte,
                message: stream_first_byte_timeout_message(timeout),
                upstream_status: Some(504),
                retryable: true,
                failover_recommended: true,
            },
        },
    })
}

enum GrokClientStreamEmitter {
    OpenAiChat {
        id: String,
        model: String,
        emitter: OpenAIChatClientEmitter,
    },
    OpenAiResponses {
        id: String,
        model: String,
        emitter: OpenAIResponsesClientEmitter,
    },
    ClaudeMessages {
        id: String,
        model: String,
        emitter: ClaudeClientEmitter,
    },
}

impl GrokClientStreamEmitter {
    fn new(plan: &ExecutionPlan) -> Self {
        let model = plan
            .model_name
            .clone()
            .unwrap_or_else(|| "grok".to_string());
        match normalized_client_api_format(plan).as_str() {
            "openai:responses" | "openai:responses:compact" => Self::OpenAiResponses {
                id: format!("resp_{}", Uuid::new_v4()),
                model,
                emitter: OpenAIResponsesClientEmitter::default(),
            },
            "claude:messages" => Self::ClaudeMessages {
                id: format!("msg_{}", Uuid::new_v4()),
                model,
                emitter: ClaudeClientEmitter::default(),
            },
            _ => Self::OpenAiChat {
                id: format!("chatcmpl-{}", Uuid::new_v4()),
                model,
                emitter: OpenAIChatClientEmitter::default(),
            },
        }
    }

    fn emit_text_delta(&mut self, text: String) -> Result<Vec<u8>, ExecutionRuntimeTransportError> {
        self.emit(CanonicalStreamEvent::TextDelta(text))
    }

    fn emit_reasoning_delta(
        &mut self,
        text: String,
    ) -> Result<Vec<u8>, ExecutionRuntimeTransportError> {
        self.emit(CanonicalStreamEvent::ReasoningDelta(text))
    }

    fn emit_image_url(&mut self, url: String) -> Result<Vec<u8>, ExecutionRuntimeTransportError> {
        self.emit(CanonicalStreamEvent::ContentPart(
            CanonicalContentPart::ImageUrl(url),
        ))
    }

    fn finish(
        &mut self,
        usage: GrokUsageEstimate,
    ) -> Result<Vec<u8>, ExecutionRuntimeTransportError> {
        let mut out = self.emit(CanonicalStreamEvent::Finish {
            finish_reason: Some("stop".to_string()),
            usage: Some(grok_canonical_usage(usage)),
        })?;
        out.extend(self.finish_emitter()?);
        Ok(out)
    }

    fn emit(
        &mut self,
        event: CanonicalStreamEvent,
    ) -> Result<Vec<u8>, ExecutionRuntimeTransportError> {
        let frame = self.frame(event);
        match self {
            Self::OpenAiChat { emitter, .. } => emitter.emit(frame),
            Self::OpenAiResponses { emitter, .. } => emitter.emit(frame),
            Self::ClaudeMessages { emitter, .. } => emitter.emit(frame),
        }
        .map_err(|err| ExecutionRuntimeTransportError::UpstreamRequest(err.to_string()))
    }

    fn finish_emitter(&mut self) -> Result<Vec<u8>, ExecutionRuntimeTransportError> {
        match self {
            Self::OpenAiChat { emitter, .. } => emitter.finish(),
            Self::OpenAiResponses { emitter, .. } => emitter.finish(),
            Self::ClaudeMessages { emitter, .. } => emitter.finish(),
        }
        .map_err(|err| ExecutionRuntimeTransportError::UpstreamRequest(err.to_string()))
    }

    fn frame(&self, event: CanonicalStreamEvent) -> CanonicalStreamFrame {
        match self {
            Self::OpenAiChat { id, model, .. }
            | Self::OpenAiResponses { id, model, .. }
            | Self::ClaudeMessages { id, model, .. } => CanonicalStreamFrame {
                id: id.clone(),
                model: model.clone(),
                event,
            },
        }
    }
}

fn grok_canonical_usage(usage: GrokUsageEstimate) -> StreamingCanonicalUsage {
    StreamingCanonicalUsage {
        input_tokens: usage.input_tokens,
        input_tokens_include_cache: false,
        output_tokens: usage.output_tokens,
        total_tokens: usage.input_tokens.saturating_add(usage.output_tokens),
        cache_creation_tokens: 0,
        cache_creation_ephemeral_5m_tokens: 0,
        cache_creation_ephemeral_1h_tokens: 0,
        cache_read_tokens: 0,
        reasoning_tokens: usage.reasoning_tokens,
    }
}

fn grok_standardized_usage(usage: GrokUsageEstimate) -> StandardizedUsage {
    let mut standardized = StandardizedUsage::new();
    standardized.input_tokens = i64::try_from(usage.input_tokens).unwrap_or(i64::MAX);
    standardized.output_tokens = i64::try_from(usage.output_tokens).unwrap_or(i64::MAX);
    standardized.reasoning_tokens = i64::try_from(usage.reasoning_tokens).unwrap_or(i64::MAX);
    standardized
}

fn grok_stream_terminal_summary(
    plan: &ExecutionPlan,
    usage: GrokUsageEstimate,
) -> ExecutionStreamTerminalSummary {
    ExecutionStreamTerminalSummary {
        standardized_usage: Some(grok_standardized_usage(usage)),
        finish_reason: Some("stop".to_string()),
        response_id: None,
        model: plan.model_name.clone(),
        provider_actual_service_tier: None,
        observed_finish: true,
        unknown_event_count: 0,
        parser_error: None,
    }
}

async fn grok_upstream_plan(
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
) -> Result<ExecutionPlan, ExecutionRuntimeTransportError> {
    let body = plan.body.json_body.as_ref().ok_or_else(|| {
        ExecutionRuntimeTransportError::UpstreamRequest(
            "Grok runtime requires JSON request body".to_string(),
        )
    })?;
    let mapped_model = grok_upstream_model_name(report_context)?;
    let mut upstream_body = crate::ai_serving::transport::build_grok_app_chat_body(
        plan.client_api_format.as_str(),
        Some(mapped_model.as_str()),
        body,
    );
    if grok_is_image_edit_plan(plan, &upstream_body) {
        attach_grok_image_edit_references(plan, body, &mut upstream_body).await?;
    } else {
        attach_grok_uploaded_files(plan, body, &mut upstream_body).await?;
    }
    let mut upstream_plan = plan.clone();
    upstream_plan.body = RequestBody::from_json(upstream_body);
    upstream_plan.stream = true;
    upstream_plan.model_name = Some(mapped_model);
    Ok(upstream_plan)
}

fn grok_is_image_edit_plan(plan: &ExecutionPlan, upstream_body: &Value) -> bool {
    normalized_client_api_format(plan) == "openai:image"
        && upstream_body
            .get("modelName")
            .and_then(Value::as_str)
            .is_some_and(|value| value == "imagine-image-edit")
}

fn grok_upstream_model_name(
    report_context: Option<&Value>,
) -> Result<String, ExecutionRuntimeTransportError> {
    let mapped_model = report_context
        .and_then(|value| value.get("mapped_model"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ExecutionRuntimeTransportError::UpstreamRequest(
                "Grok runtime requires mapped_model in report context".to_string(),
            )
        })?;
    Ok(mapped_model.to_string())
}

async fn attach_grok_uploaded_files(
    plan: &ExecutionPlan,
    original_body: &Value,
    upstream_body: &mut Value,
) -> Result<(), ExecutionRuntimeTransportError> {
    let inputs = extract_grok_attachment_inputs(plan.client_api_format.as_str(), original_body);
    if inputs.is_empty() {
        return Ok(());
    }

    let mut attachment_ids = Vec::with_capacity(inputs.len());
    for (index, input) in inputs.into_iter().enumerate() {
        let payload = resolve_grok_attachment_payload(&input, index).await?;
        let uploaded = upload_grok_attachment(plan, payload).await?;
        if !uploaded.file_id.trim().is_empty() {
            attachment_ids.push(Value::String(uploaded.file_id));
        }
    }
    if attachment_ids.is_empty() {
        return Ok(());
    }
    let Some(object) = upstream_body.as_object_mut() else {
        return Err(ExecutionRuntimeTransportError::UpstreamRequest(
            "Grok runtime generated non-object app-chat body".to_string(),
        ));
    };
    object.insert("fileAttachments".to_string(), Value::Array(attachment_ids));
    Ok(())
}

async fn attach_grok_image_edit_references(
    plan: &ExecutionPlan,
    original_body: &Value,
    upstream_body: &mut Value,
) -> Result<(), ExecutionRuntimeTransportError> {
    let inputs = extract_grok_attachment_inputs(plan.client_api_format.as_str(), original_body);
    if inputs.is_empty() {
        return Err(ExecutionRuntimeTransportError::UpstreamRequest(
            "Grok image edit requires at least one reference image".to_string(),
        ));
    }

    let mut image_references = Vec::with_capacity(inputs.len());
    for (index, input) in inputs.into_iter().enumerate() {
        let payload = resolve_grok_attachment_payload(&input, index).await?;
        let uploaded = upload_grok_attachment(plan, payload).await?;
        let reference = resolve_grok_uploaded_asset_reference(plan, &uploaded)?;
        image_references.push(Value::String(reference));
    }
    let parent_post_id =
        create_grok_media_post(plan, grok_image_edit_prompt(upstream_body)).await?;
    set_grok_image_edit_config(upstream_body, image_references, parent_post_id)
}

fn extract_grok_attachment_inputs(
    client_api_format: &str,
    body: &Value,
) -> Vec<GrokAttachmentInput> {
    match client_api_format.trim().to_ascii_lowercase().as_str() {
        "openai:responses" | "openai:responses:compact" => {
            extract_responses_attachment_inputs(body)
        }
        "claude:messages" => extract_claude_attachment_inputs(body),
        _ => extract_openai_chat_attachment_inputs(body),
    }
}

fn extract_openai_chat_attachment_inputs(body: &Value) -> Vec<GrokAttachmentInput> {
    let mut out = Vec::new();
    if let Some(messages) = body.get("messages").and_then(Value::as_array) {
        for message in messages {
            collect_content_attachment_inputs(message.get("content"), &mut out);
        }
    }
    out
}

fn extract_responses_attachment_inputs(body: &Value) -> Vec<GrokAttachmentInput> {
    let mut out = Vec::new();
    collect_responses_input_attachment_inputs(body.get("input"), &mut out);
    out
}

fn collect_responses_input_attachment_inputs(
    value: Option<&Value>,
    out: &mut Vec<GrokAttachmentInput>,
) {
    let Some(value) = value else {
        return;
    };
    match value {
        Value::Array(items) => {
            for item in items {
                if item.get("type").and_then(Value::as_str) == Some("message") {
                    collect_content_attachment_inputs(item.get("content"), out);
                } else {
                    collect_attachment_input_from_object(item, out);
                }
            }
        }
        Value::Object(_) => collect_attachment_input_from_object(value, out),
        _ => {}
    }
}

fn extract_claude_attachment_inputs(body: &Value) -> Vec<GrokAttachmentInput> {
    let mut out = Vec::new();
    if let Some(messages) = body.get("messages").and_then(Value::as_array) {
        for message in messages {
            collect_content_attachment_inputs(message.get("content"), &mut out);
        }
    }
    out
}

fn collect_content_attachment_inputs(value: Option<&Value>, out: &mut Vec<GrokAttachmentInput>) {
    let Some(value) = value else {
        return;
    };
    match value {
        Value::Array(items) => {
            for item in items {
                collect_attachment_input_from_object(item, out);
            }
        }
        Value::Object(_) => collect_attachment_input_from_object(value, out),
        _ => {}
    }
}

fn collect_attachment_input_from_object(value: &Value, out: &mut Vec<GrokAttachmentInput>) {
    let Some(object) = value.as_object() else {
        return;
    };
    if let Some(input) = claude_source_attachment(object) {
        out.push(input);
        return;
    }
    if let Some(source) = image_url_source(object) {
        out.push(GrokAttachmentInput {
            source,
            filename: None,
            mime_type: None,
        });
        return;
    }
    if let Some(input) = file_source(object) {
        out.push(input);
    }
}

fn image_url_source(object: &Map<String, Value>) -> Option<String> {
    if let Some(source) = object.get("image_url").and_then(string_or_url_value) {
        return Some(source);
    }
    if object
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|value| value.eq_ignore_ascii_case("image_url"))
    {
        return object
            .get("url")
            .and_then(Value::as_str)
            .map(trimmed_string);
    }
    if object
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|value| value.eq_ignore_ascii_case("input_image"))
    {
        return object
            .get("image_url")
            .or_else(|| object.get("source"))
            .and_then(string_or_url_value);
    }
    None
}

fn file_source(object: &Map<String, Value>) -> Option<GrokAttachmentInput> {
    let file_object = object.get("file").and_then(Value::as_object);
    let source = file_object
        .and_then(|file| {
            file.get("file_data")
                .or_else(|| file.get("data"))
                .or_else(|| file.get("url"))
                .or_else(|| file.get("file_url"))
                .and_then(Value::as_str)
        })
        .or_else(|| object.get("file_data").and_then(Value::as_str))
        .or_else(|| object.get("file_url").and_then(Value::as_str))
        .or_else(|| object.get("data").and_then(Value::as_str))
        .map(trimmed_string)
        .filter(|value| !value.is_empty())?;
    let filename = file_object
        .and_then(|file| file.get("filename").or_else(|| file.get("name")))
        .or_else(|| object.get("filename"))
        .or_else(|| object.get("name"))
        .and_then(Value::as_str)
        .map(trimmed_string)
        .filter(|value| !value.is_empty());
    let mime_type = file_object
        .and_then(|file| file.get("mime_type").or_else(|| file.get("mimeType")))
        .or_else(|| object.get("mime_type"))
        .or_else(|| object.get("mimeType"))
        .and_then(Value::as_str)
        .map(trimmed_string)
        .filter(|value| !value.is_empty());
    Some(GrokAttachmentInput {
        source,
        filename,
        mime_type,
    })
}

fn claude_source_attachment(object: &Map<String, Value>) -> Option<GrokAttachmentInput> {
    let block_type = object
        .get("type")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if !matches!(block_type, "image" | "document") {
        return None;
    }
    let source = object.get("source").and_then(Value::as_object)?;
    let source_type = source
        .get("type")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    let mime_type = source
        .get("media_type")
        .or_else(|| source.get("mediaType"))
        .and_then(Value::as_str)
        .map(trimmed_string)
        .filter(|value| !value.is_empty());
    let filename = object
        .get("filename")
        .or_else(|| object.get("name"))
        .and_then(Value::as_str)
        .map(trimmed_string)
        .filter(|value| !value.is_empty());

    match source_type {
        "base64" => {
            let data = source
                .get("data")
                .and_then(Value::as_str)
                .map(trimmed_string)
                .filter(|value| !value.is_empty())?;
            let mime = mime_type
                .clone()
                .unwrap_or_else(|| "application/octet-stream".to_string());
            Some(GrokAttachmentInput {
                source: format!("data:{mime};base64,{data}"),
                filename,
                mime_type,
            })
        }
        "url" => {
            let url = source
                .get("url")
                .and_then(Value::as_str)
                .map(trimmed_string)
                .filter(|value| !value.is_empty())?;
            Some(GrokAttachmentInput {
                source: url,
                filename,
                mime_type,
            })
        }
        _ => None,
    }
}

fn string_or_url_value(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(trimmed_string)
        .or_else(|| value.get("url").and_then(Value::as_str).map(trimmed_string))
        .filter(|value| !value.is_empty())
}

fn trimmed_string(value: &str) -> String {
    value.trim().to_string()
}

async fn resolve_grok_attachment_payload(
    input: &GrokAttachmentInput,
    index: usize,
) -> Result<GrokAttachmentPayload, ExecutionRuntimeTransportError> {
    if input.source.starts_with("data:") {
        return grok_attachment_payload_from_data_uri(input, index);
    }
    grok_attachment_payload_from_url(input, index).await
}

fn grok_attachment_payload_from_data_uri(
    input: &GrokAttachmentInput,
    index: usize,
) -> Result<GrokAttachmentPayload, ExecutionRuntimeTransportError> {
    let (header, content_b64) = input.source.split_once(',').ok_or_else(|| {
        ExecutionRuntimeTransportError::UpstreamRequest(
            "Grok attachment data URI is missing comma separator".to_string(),
        )
    })?;
    if !header.contains(";base64") {
        return Err(ExecutionRuntimeTransportError::UpstreamRequest(
            "Grok attachment data URI must be base64 encoded".to_string(),
        ));
    }
    let mime_type = input
        .mime_type
        .clone()
        .or_else(|| {
            header
                .strip_prefix("data:")
                .and_then(|value| value.split(';').next())
                .map(trimmed_string)
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_else(|| "application/octet-stream".to_string());
    let normalized_b64 = content_b64.split_whitespace().collect::<String>();
    let decoded_len = base64::engine::general_purpose::STANDARD
        .decode(&normalized_b64)
        .map_err(|err| {
            ExecutionRuntimeTransportError::UpstreamRequest(format!(
                "Grok attachment data URI base64 is invalid: {err}"
            ))
        })?
        .len();
    if decoded_len > GROK_MAX_ATTACHMENT_BYTES {
        return Err(ExecutionRuntimeTransportError::UpstreamRequest(format!(
            "Grok attachment exceeds {} byte limit",
            GROK_MAX_ATTACHMENT_BYTES
        )));
    }
    Ok(GrokAttachmentPayload {
        filename: input
            .filename
            .clone()
            .unwrap_or_else(|| default_attachment_filename(index, &mime_type)),
        mime_type,
        content_b64: normalized_b64,
    })
}

async fn grok_attachment_payload_from_url(
    input: &GrokAttachmentInput,
    index: usize,
) -> Result<GrokAttachmentPayload, ExecutionRuntimeTransportError> {
    let url = reqwest::Url::parse(input.source.as_str()).map_err(|err| {
        ExecutionRuntimeTransportError::UpstreamRequest(format!(
            "Grok attachment URL is invalid: {err}"
        ))
    })?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(ExecutionRuntimeTransportError::UpstreamRequest(
            "Grok attachment URL must use http or https".to_string(),
        ));
    }
    let response = fetch_grok_attachment_url(url.clone(), 0).await?;
    let final_url = response.url().clone();
    let mime_type = input
        .mime_type
        .clone()
        .or_else(|| {
            response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.split(';').next())
                .map(trimmed_string)
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_else(|| "application/octet-stream".to_string());
    let bytes = collect_grok_attachment_url_bytes(response).await?;
    Ok(GrokAttachmentPayload {
        filename: input
            .filename
            .clone()
            .or_else(|| filename_from_url_path(final_url.path()))
            .unwrap_or_else(|| default_attachment_filename(index, &mime_type)),
        mime_type,
        content_b64: base64::engine::general_purpose::STANDARD.encode(&bytes),
    })
}

async fn fetch_grok_attachment_url(
    mut url: reqwest::Url,
    mut redirects: usize,
) -> Result<reqwest::Response, ExecutionRuntimeTransportError> {
    loop {
        validate_grok_attachment_public_url(&url).await?;
        let response = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .resolve_to_addrs(
                url.host_str().unwrap_or_default(),
                &[public_socket_addr_for_url(&url).await?],
            )
            .build()
            .map_err(ExecutionRuntimeTransportError::ClientBuild)?
            .get(url.clone())
            .send()
            .await
            .map_err(|err| {
                ExecutionRuntimeTransportError::UpstreamRequest(format_upstream_request_error(&err))
            })?;
        if response.status().is_redirection() {
            redirects += 1;
            if redirects > GROK_MAX_ATTACHMENT_REDIRECTS {
                return Err(ExecutionRuntimeTransportError::UpstreamRequest(
                    "Grok attachment URL fetch exceeded redirect limit".to_string(),
                ));
            }
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| {
                    ExecutionRuntimeTransportError::UpstreamRequest(
                        "Grok attachment redirect is missing Location header".to_string(),
                    )
                })?;
            url = url.join(location).map_err(|err| {
                ExecutionRuntimeTransportError::UpstreamRequest(format!(
                    "Grok attachment redirect URL is invalid: {err}"
                ))
            })?;
            continue;
        }
        if !response.status().is_success() {
            return Err(ExecutionRuntimeTransportError::UpstreamRequest(format!(
                "Grok attachment URL fetch returned {}",
                response.status().as_u16()
            )));
        }
        return Ok(response);
    }
}

async fn validate_grok_attachment_public_url(
    url: &reqwest::Url,
) -> Result<(), ExecutionRuntimeTransportError> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err(ExecutionRuntimeTransportError::UpstreamRequest(format!(
            "Grok attachment URL scheme is unsupported: {}",
            url.scheme()
        )));
    }
    public_socket_addr_for_url(url).await.map(|_| ())
}

async fn public_socket_addr_for_url(
    url: &reqwest::Url,
) -> Result<std::net::SocketAddr, ExecutionRuntimeTransportError> {
    let host = url.host_str().ok_or_else(|| {
        ExecutionRuntimeTransportError::UpstreamRequest(
            "Grok attachment URL is missing a host".to_string(),
        )
    })?;
    let port = url.port_or_known_default().ok_or_else(|| {
        ExecutionRuntimeTransportError::UpstreamRequest(
            "Grok attachment URL is missing a port".to_string(),
        )
    })?;
    if let Ok(ip) = host.parse::<IpAddr>() {
        if !grok_attachment_ip_is_public(ip) {
            return Err(ExecutionRuntimeTransportError::UpstreamRequest(
                "Grok attachment URL resolves to a non-public address".to_string(),
            ));
        }
        return Ok(std::net::SocketAddr::new(ip, port));
    }
    let mut public_addr = None;
    let mut resolved_any = false;
    for addr in tokio::net::lookup_host((host, port)).await.map_err(|err| {
        ExecutionRuntimeTransportError::UpstreamRequest(format!(
            "Grok attachment URL DNS resolution failed: {err}"
        ))
    })? {
        resolved_any = true;
        if !grok_attachment_ip_is_public(addr.ip()) {
            return Err(ExecutionRuntimeTransportError::UpstreamRequest(
                "Grok attachment URL resolves to a non-public address".to_string(),
            ));
        }
        public_addr.get_or_insert(addr);
    }
    if !resolved_any {
        return Err(ExecutionRuntimeTransportError::UpstreamRequest(
            "Grok attachment URL DNS resolution returned no addresses".to_string(),
        ));
    }
    public_addr.ok_or_else(|| {
        ExecutionRuntimeTransportError::UpstreamRequest(
            "Grok attachment URL has no public address".to_string(),
        )
    })
}

fn grok_attachment_ip_is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_documentation()
                || ip.is_unspecified())
        }
        IpAddr::V6(ip) => {
            !(ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
                || is_ipv6_documentation_addr(ip))
        }
    }
}

fn is_ipv6_documentation_addr(ip: std::net::Ipv6Addr) -> bool {
    let segments = ip.segments();
    segments[0] == 0x2001 && segments[1] == 0x0db8
}

async fn collect_grok_attachment_url_bytes(
    response: reqwest::Response,
) -> Result<Vec<u8>, ExecutionRuntimeTransportError> {
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|err| {
            ExecutionRuntimeTransportError::UpstreamRequest(format_upstream_request_error(&err))
        })?;
        if bytes.len().saturating_add(chunk.len()) > GROK_MAX_ATTACHMENT_BYTES {
            return Err(ExecutionRuntimeTransportError::UpstreamRequest(format!(
                "Grok attachment exceeds {} byte limit",
                GROK_MAX_ATTACHMENT_BYTES
            )));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

async fn upload_grok_attachment(
    plan: &ExecutionPlan,
    payload: GrokAttachmentPayload,
) -> Result<GrokUploadedAttachment, ExecutionRuntimeTransportError> {
    let body = json!({
        "fileName": payload.filename,
        "fileMimeType": payload.mime_type,
        "content": payload.content_b64,
    });
    let mut upload_plan = plan.clone();
    upload_plan.url = grok_upload_url(plan.url.as_str());
    upload_plan.method = "POST".to_string();
    upload_plan.stream = false;
    upload_plan.content_type = Some("application/json".to_string());
    upload_plan.body = RequestBody::from_json(body);
    upload_plan.headers = grok_upload_headers(&plan.headers)?;
    let request_body = build_request_body(&upload_plan)?;
    let response = send_request(&upload_plan, request_body).await?;
    let status_code = response.status_code();
    let bytes = response.bytes().await?;
    if !(200..300).contains(&status_code) {
        let text = String::from_utf8_lossy(&bytes).to_string();
        return Err(ExecutionRuntimeTransportError::UpstreamRequest(format!(
            "Grok attachment upload returned {status_code}: {text}"
        )));
    }
    let value = serde_json::from_slice::<Value>(&bytes)
        .map_err(ExecutionRuntimeTransportError::InvalidJson)?;
    let file_id = value
        .get("fileMetadataId")
        .or_else(|| value.get("fileId"))
        .and_then(Value::as_str)
        .map(trimmed_string)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ExecutionRuntimeTransportError::UpstreamRequest(
                "Grok attachment upload response is missing fileMetadataId".to_string(),
            )
        })?;
    let file_uri = value
        .get("fileUri")
        .and_then(Value::as_str)
        .map(trimmed_string)
        .filter(|value| !value.is_empty());
    Ok(GrokUploadedAttachment { file_id, file_uri })
}

async fn create_grok_media_post(
    plan: &ExecutionPlan,
    prompt: String,
) -> Result<String, ExecutionRuntimeTransportError> {
    let body = json!({
        "mediaType": "MEDIA_POST_TYPE_IMAGE",
        "prompt": prompt,
    });
    let mut media_plan = plan.clone();
    media_plan.url = grok_media_post_url(plan.url.as_str());
    media_plan.method = "POST".to_string();
    media_plan.stream = false;
    media_plan.content_type = Some("application/json".to_string());
    media_plan.body = RequestBody::from_json(body);
    media_plan.headers = grok_upload_headers(&plan.headers)?;
    media_plan.headers.insert(
        "referer".to_string(),
        grok_imagine_referer(plan.url.as_str()),
    );
    let request_body = build_request_body(&media_plan)?;
    let response = send_request(&media_plan, request_body).await?;
    let status_code = response.status_code();
    let bytes = response.bytes().await?;
    if !(200..300).contains(&status_code) {
        let text = String::from_utf8_lossy(&bytes).to_string();
        return Err(ExecutionRuntimeTransportError::UpstreamRequest(format!(
            "Grok media post create returned {status_code}: {text}"
        )));
    }
    let value = serde_json::from_slice::<Value>(&bytes)
        .map_err(ExecutionRuntimeTransportError::InvalidJson)?;
    value
        .get("post")
        .and_then(|post| post.get("id"))
        .or_else(|| value.get("id"))
        .and_then(Value::as_str)
        .map(trimmed_string)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ExecutionRuntimeTransportError::UpstreamRequest(
                "Grok media post create response is missing post id".to_string(),
            )
        })
}

fn grok_upload_headers(
    headers: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, ExecutionRuntimeTransportError> {
    let mut out = headers.clone();
    out.insert("accept".to_string(), "application/json".to_string());
    out.insert("content-type".to_string(), "application/json".to_string());
    out.insert("sec-fetch-dest".to_string(), "empty".to_string());
    out.insert("sec-fetch-mode".to_string(), "cors".to_string());
    out.insert("sec-fetch-site".to_string(), "same-origin".to_string());
    out.insert("x-xai-request-id".to_string(), Uuid::new_v4().to_string());
    Ok(out)
}

fn grok_upload_url(chat_url: &str) -> String {
    let Ok(mut url) = reqwest::Url::parse(chat_url) else {
        return format!("https://grok.com{GROK_UPLOAD_PATH}");
    };
    url.set_path(GROK_UPLOAD_PATH);
    url.set_query(None);
    url.set_fragment(None);
    url.to_string()
}

fn grok_media_post_url(chat_url: &str) -> String {
    let Ok(mut url) = reqwest::Url::parse(chat_url) else {
        return format!("https://grok.com{GROK_MEDIA_POST_PATH}");
    };
    url.set_path(GROK_MEDIA_POST_PATH);
    url.set_query(None);
    url.set_fragment(None);
    url.to_string()
}

fn grok_imagine_referer(chat_url: &str) -> String {
    let Ok(mut url) = reqwest::Url::parse(chat_url) else {
        return "https://grok.com/imagine".to_string();
    };
    url.set_path("/imagine");
    url.set_query(None);
    url.set_fragment(None);
    url.to_string()
}

fn resolve_grok_uploaded_asset_reference(
    plan: &ExecutionPlan,
    uploaded: &GrokUploadedAttachment,
) -> Result<String, ExecutionRuntimeTransportError> {
    if let Some(file_uri) = uploaded.file_uri.as_deref() {
        if !file_uri.trim().is_empty() {
            return Ok(grok_asset_url(file_uri));
        }
    }
    let user_id = grok_user_id_from_cookie_header(&plan.headers).ok_or_else(|| {
        ExecutionRuntimeTransportError::UpstreamRequest(
            "Grok image edit upload response is missing fileUri and cookie x-userid is unavailable"
                .to_string(),
        )
    })?;
    Ok(format!(
        "{GROK_ASSET_BASE}users/{}/{}/content",
        user_id, uploaded.file_id
    ))
}

fn grok_asset_url(value: &str) -> String {
    let value = value.trim();
    if value.starts_with("http://") || value.starts_with("https://") {
        value.to_string()
    } else {
        format!("{GROK_ASSET_BASE}{}", value.trim_start_matches('/'))
    }
}

fn grok_user_id_from_cookie_header(headers: &BTreeMap<String, String>) -> Option<String> {
    let cookie = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("cookie"))
        .map(|(_, value)| value.as_str())?;
    cookie.split(';').find_map(|part| {
        let (name, value) = part.trim().split_once('=')?;
        (name.trim() == "x-userid")
            .then(|| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

fn grok_image_edit_prompt(upstream_body: &Value) -> String {
    upstream_body
        .get("message")
        .and_then(Value::as_str)
        .map(trimmed_string)
        .unwrap_or_default()
}

fn grok_image_prompt_from_provider_body(body: &Value) -> Option<String> {
    body.get("prompt")
        .and_then(Value::as_str)
        .map(trimmed_string)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            body.get("input")
                .and_then(grok_value_text)
                .map(|value| trimmed_string(&value))
        })
        .or_else(|| grok_last_user_message_text(body))
        .filter(|value| !value.is_empty())
}

fn grok_image_count_from_provider_body(body: &Value) -> usize {
    body.get("n")
        .and_then(Value::as_u64)
        .or_else(|| body.get("imageGenerationCount").and_then(Value::as_u64))
        .or_else(|| {
            body.get("image_config")
                .and_then(|config| {
                    config
                        .get("n")
                        .or_else(|| config.get("imageGenerationCount"))
                })
                .and_then(Value::as_u64)
        })
        .or_else(|| {
            body.get("tools")
                .and_then(Value::as_array)
                .and_then(|tools| tools.first())
                .and_then(|tool| tool.get("n").or_else(|| tool.get("imageGenerationCount")))
                .and_then(Value::as_u64)
        })
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(1)
}

fn grok_aspect_ratio_from_provider_body(body: &Value) -> String {
    let size_or_ratio = grok_image_option_from_provider_body(body, "aspect_ratio")
        .or_else(|| grok_image_option_from_provider_body(body, "aspectRatio"))
        .or_else(|| grok_image_option_from_provider_body(body, "ratio"))
        .or_else(|| grok_image_option_from_provider_body(body, "size"))
        .unwrap_or("1024x1024");
    match size_or_ratio {
        "1280x720" | "16:9" => "16:9",
        "720x1280" | "9:16" => "9:16",
        "1792x1024" | "3:2" => "3:2",
        "1024x1792" | "2:3" => "2:3",
        "1024x1024" | "1:1" => "1:1",
        _ => "2:3",
    }
    .to_string()
}

fn grok_image_option_from_provider_body<'a>(body: &'a Value, key: &str) -> Option<&'a str> {
    body.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            body.get("image_config")
                .and_then(|config| config.get(key))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
        .or_else(|| {
            body.get("tools")
                .and_then(Value::as_array)
                .and_then(|tools| tools.first())
                .and_then(|tool| tool.get(key))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
}

fn grok_last_user_message_text(body: &Value) -> Option<String> {
    body.get("messages")
        .and_then(Value::as_array)?
        .iter()
        .rev()
        .find_map(|message| {
            let role = message.get("role").and_then(Value::as_str)?;
            if !role.eq_ignore_ascii_case("user") {
                return None;
            }
            message
                .get("content")
                .and_then(grok_value_text)
                .map(|value| trimmed_string(&value))
                .filter(|value| !value.is_empty())
        })
}

fn grok_value_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Array(items) => {
            let parts = items
                .iter()
                .filter_map(grok_value_text)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>();
            (!parts.is_empty()).then(|| parts.join("\n"))
        }
        Value::Object(object) => object
            .get("text")
            .or_else(|| object.get("input_text"))
            .or_else(|| object.get("content"))
            .and_then(grok_value_text),
        _ => None,
    }
}

fn grok_imagine_reset_message() -> Value {
    json!({
        "type": "conversation.item.create",
        "timestamp": current_unix_secs().saturating_mul(1000),
        "item": {
            "type": "message",
            "content": [{"type": "reset"}],
        },
    })
}

fn grok_imagine_request_message(prompt: &str, aspect_ratio: &str, enable_pro: bool) -> Value {
    json!({
        "type": "conversation.item.create",
        "timestamp": current_unix_secs().saturating_mul(1000),
        "item": {
            "type": "message",
            "content": [{
                "requestId": Uuid::new_v4().to_string(),
                "text": prompt,
                "type": "input_text",
                "properties": {
                    "section_count": 0,
                    "is_kids_mode": false,
                    "enable_nsfw": true,
                    "skip_upsampler": false,
                    "enable_side_by_side": true,
                    "is_initial": false,
                    "aspect_ratio": aspect_ratio,
                    "enable_pro": enable_pro,
                },
            }],
        },
    })
}

fn grok_handle_imagine_ws_message(
    value: &Value,
    slots: &mut BTreeMap<String, GrokImagineImage>,
) -> Result<(), ExecutionRuntimeTransportError> {
    match value.get("type").and_then(Value::as_str) {
        Some("json") => grok_handle_imagine_json_frame(value, slots),
        Some("image") => {
            grok_handle_imagine_image_frame(value, slots);
            Ok(())
        }
        Some("error") => Err(ExecutionRuntimeTransportError::UpstreamRequest(format!(
            "Grok Imagine websocket error: {}",
            value
                .get("err_msg")
                .or_else(|| value.get("error"))
                .and_then(Value::as_str)
                .unwrap_or("unknown")
        ))),
        _ => Ok(()),
    }
}

fn grok_handle_imagine_json_frame(
    value: &Value,
    slots: &mut BTreeMap<String, GrokImagineImage>,
) -> Result<(), ExecutionRuntimeTransportError> {
    let status = value.get("current_status").and_then(Value::as_str);
    let Some(image_id) = value
        .get("image_id")
        .or_else(|| value.get("job_id"))
        .and_then(Value::as_str)
        .map(trimmed_string)
        .filter(|value| !value.is_empty())
    else {
        return Ok(());
    };
    let order = value
        .get("order")
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or_default();
    match status {
        Some("start_stage") => {
            slots.entry(image_id.clone()).or_insert(GrokImagineImage {
                image_id,
                order,
                url: None,
                blob_b64: None,
                done: false,
                moderated: false,
            });
        }
        Some("completed") => {
            let slot = slots.entry(image_id.clone()).or_insert(GrokImagineImage {
                image_id,
                order,
                url: None,
                blob_b64: None,
                done: false,
                moderated: false,
            });
            slot.order = order;
            slot.done = true;
            slot.moderated = value
                .get("moderated")
                .and_then(Value::as_bool)
                .unwrap_or(false);
        }
        _ => {}
    }
    Ok(())
}

fn grok_handle_imagine_image_frame(value: &Value, slots: &mut BTreeMap<String, GrokImagineImage>) {
    let Some(url) = value.get("url").and_then(Value::as_str).map(grok_asset_url) else {
        return;
    };
    let image_id =
        grok_imagine_image_id_from_url(&url).unwrap_or_else(|| Uuid::new_v4().to_string());
    let fallback_order = slots.len();
    let slot = slots.entry(image_id.clone()).or_insert(GrokImagineImage {
        image_id,
        order: fallback_order,
        url: None,
        blob_b64: None,
        done: false,
        moderated: false,
    });
    slot.url = Some(url);
    slot.blob_b64 = value
        .get("blob")
        .and_then(Value::as_str)
        .map(trimmed_string)
        .filter(|value| !value.is_empty());
}

fn grok_imagine_image_id_from_url(url: &str) -> Option<String> {
    let path = reqwest::Url::parse(url)
        .ok()
        .map(|url| url.path().to_string())
        .unwrap_or_else(|| url.to_string());
    let file_name = path.rsplit('/').next()?;
    let (stem, _) = file_name.rsplit_once('.')?;
    (!stem.trim().is_empty()).then(|| stem.to_string())
}

fn grok_imagine_completed_count(slots: &BTreeMap<String, GrokImagineImage>) -> usize {
    slots
        .values()
        .filter(|image| {
            image.done && !image.moderated && (image.url.is_some() || image.blob_b64.is_some())
        })
        .count()
}

fn grok_data_image_url(blob_b64: String) -> String {
    format!("data:image/png;base64,{blob_b64}")
}

fn set_grok_image_edit_config(
    upstream_body: &mut Value,
    image_references: Vec<Value>,
    parent_post_id: String,
) -> Result<(), ExecutionRuntimeTransportError> {
    let Some(config) = upstream_body
        .get_mut("responseMetadata")
        .and_then(|value| value.get_mut("modelConfigOverride"))
        .and_then(|value| value.get_mut("modelMap"))
        .and_then(|value| value.get_mut("imageEditModelConfig"))
        .and_then(Value::as_object_mut)
    else {
        return Err(ExecutionRuntimeTransportError::UpstreamRequest(
            "Grok image edit payload is missing imageEditModelConfig".to_string(),
        ));
    };
    config.insert(
        "imageReferences".to_string(),
        Value::Array(image_references),
    );
    config.insert("parentPostId".to_string(), Value::String(parent_post_id));
    Ok(())
}

fn filename_from_url_path(path: &str) -> Option<String> {
    path.rsplit('/')
        .next()
        .map(trimmed_string)
        .filter(|value| !value.is_empty())
}

fn default_attachment_filename(index: usize, mime_type: &str) -> String {
    let ext = mime_type
        .rsplit('/')
        .next()
        .map(|value| value.split('+').next().unwrap_or(value))
        .map(trimmed_string)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "bin".to_string());
    format!("file-{}.{}", index + 1, ext)
}

fn grok_execution_result(
    plan: &ExecutionPlan,
    collected: GrokCollected,
    report_context: Option<&Value>,
) -> ExecutionResult {
    let status_code = collected.status_code;
    let body_json = if (200..300).contains(&status_code) {
        grok_client_json_body(plan, &collected, report_context)
    } else {
        json!({
            "error": {
                "message": collected.text,
                "type": "grok_upstream_error",
                "code": status_code,
            }
        })
    };
    ExecutionResult {
        request_id: plan.request_id.clone(),
        candidate_id: plan.candidate_id.clone(),
        status_code,
        headers: BTreeMap::from([("content-type".to_string(), "application/json".to_string())]),
        body: Some(ResponseBody {
            json_body: Some(body_json),
            body_bytes_b64: None,
        }),
        telemetry: Some(collected.telemetry),
        error: None,
    }
}

fn grok_client_json_body(
    plan: &ExecutionPlan,
    collected: &GrokCollected,
    report_context: Option<&Value>,
) -> Value {
    let model = plan.model_name.as_deref().unwrap_or("grok");
    let usage = grok_usage_estimate(plan, collected);
    let client_format = normalized_client_api_format(plan);
    if client_format == "openai:image" {
        return openai_image_body(collected);
    }

    let provider_body = openai_responses_body(
        model,
        collected,
        usage,
        grok_plan_uses_structured_image_generation(plan, report_context),
    );
    if client_format == "openai:responses" {
        return provider_body;
    }

    convert_standard_chat_response(
        &provider_body,
        GROK_STANDARD_PROVIDER_API_FORMAT,
        client_format.as_str(),
        &grok_conversion_report_context(plan, model),
    )
    .unwrap_or_else(|| {
        grok_legacy_client_json_body(client_format.as_str(), model, collected, usage)
    })
}

fn grok_collected_frame_stream(
    plan: ExecutionPlan,
    collected: GrokCollected,
    report_context: Option<&Value>,
) -> BoxStream<'static, Result<Bytes, IoError>> {
    let body = grok_client_stream_body(&plan, &collected, report_context);
    let telemetry = collected.telemetry.clone();
    let status_code = collected.status_code;
    let frames = vec![
        StreamFrame {
            frame_type: StreamFrameType::Headers,
            payload: StreamFramePayload::Headers {
                status_code,
                headers: BTreeMap::from([(
                    "content-type".to_string(),
                    if (200..300).contains(&status_code) {
                        "text/event-stream".to_string()
                    } else {
                        "application/json".to_string()
                    },
                )]),
            },
        },
        StreamFrame {
            frame_type: StreamFrameType::Telemetry,
            payload: StreamFramePayload::Telemetry {
                telemetry: ExecutionTelemetry {
                    ttfb_ms: telemetry.ttfb_ms,
                    elapsed_ms: telemetry.ttfb_ms,
                    upstream_bytes: Some(0),
                },
            },
        },
        StreamFrame {
            frame_type: StreamFrameType::Data,
            payload: StreamFramePayload::Data {
                chunk_b64: Some(base64::engine::general_purpose::STANDARD.encode(body.as_bytes())),
                text: None,
            },
        },
        StreamFrame {
            frame_type: StreamFrameType::Telemetry,
            payload: StreamFramePayload::Telemetry { telemetry },
        },
        StreamFrame::eof(),
    ];
    stream::iter(
        frames
            .into_iter()
            .map(|frame| encode_stream_frame_ndjson(&frame)),
    )
    .boxed()
}

fn grok_client_stream_body(
    plan: &ExecutionPlan,
    collected: &GrokCollected,
    report_context: Option<&Value>,
) -> String {
    if !(200..300).contains(&collected.status_code) {
        return serde_json::to_string(&json!({
            "error": {
                "message": collected.text,
                "type": "grok_upstream_error",
                "code": collected.status_code,
            }
        }))
        .unwrap_or_else(|_| "{}".to_string());
    }
    let model = plan.model_name.as_deref().unwrap_or("grok");
    let usage = grok_usage_estimate(plan, collected);
    let client_format = normalized_client_api_format(plan);
    if client_format == "openai:image" {
        return openai_image_sse(collected);
    }

    let provider_body = openai_responses_body(
        model,
        collected,
        usage,
        grok_plan_uses_structured_image_generation(plan, report_context),
    );
    let report_context = grok_conversion_report_context(plan, model);
    match maybe_bridge_standard_sync_json_to_stream(
        &provider_body,
        GROK_STANDARD_PROVIDER_API_FORMAT,
        client_format.as_str(),
        Some(&report_context),
    ) {
        Ok(Some(outcome)) => String::from_utf8(outcome.sse_body)
            .unwrap_or_else(|err| String::from_utf8_lossy(&err.into_bytes()).into_owned()),
        Ok(None) | Err(_) => {
            grok_legacy_client_stream_body(client_format.as_str(), model, collected, usage)
        }
    }
}

fn normalized_client_api_format(plan: &ExecutionPlan) -> String {
    let value = plan.client_api_format.trim();
    if value.is_empty() {
        "openai:chat".to_string()
    } else {
        value.to_ascii_lowercase()
    }
}

fn grok_plan_uses_structured_image_generation(
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
) -> bool {
    let client_format = normalized_client_api_format(plan);
    if client_format == "openai:image" {
        return true;
    }
    if !matches!(
        client_format.as_str(),
        "openai:chat" | "openai:responses" | "openai:responses:compact"
    ) {
        return false;
    }
    let model_is_image_generation = plan
        .model_name
        .as_deref()
        .is_some_and(grok_model_name_is_image_generation)
        || grok_report_context_mapped_model(report_context)
            .is_some_and(grok_model_name_is_image_generation);
    let body_has_image_generation_tool = plan
        .body
        .json_body
        .as_ref()
        .is_some_and(grok_body_has_image_generation_tool);
    model_is_image_generation || body_has_image_generation_tool
}

fn grok_report_context_mapped_model(report_context: Option<&Value>) -> Option<&str> {
    report_context
        .and_then(|value| value.get("mapped_model"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn grok_model_name_is_image_generation(model: &str) -> bool {
    let model = model.trim().to_ascii_lowercase();
    model.contains("grok-imagine-image") && !model.contains("edit")
}

fn grok_body_has_image_generation_tool(body: &Value) -> bool {
    body.get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|tool| {
            tool.get("type")
                .and_then(Value::as_str)
                .is_some_and(|value| value.eq_ignore_ascii_case("image_generation"))
                && !tool
                    .get("action")
                    .and_then(Value::as_str)
                    .is_some_and(|value| value.eq_ignore_ascii_case("edit"))
        })
}

fn grok_conversion_report_context(plan: &ExecutionPlan, model: &str) -> Value {
    json!({
        "provider_type": "grok",
        "provider_api_format": GROK_STANDARD_PROVIDER_API_FORMAT,
        "client_api_format": normalized_client_api_format(plan),
        "mapped_model": model,
        "model": model,
    })
}

fn grok_legacy_client_json_body(
    client_format: &str,
    model: &str,
    collected: &GrokCollected,
    usage: GrokUsageEstimate,
) -> Value {
    match client_format {
        "openai:responses" | "openai:responses:compact" => {
            openai_responses_body(model, collected, usage, false)
        }
        "claude:messages" => claude_messages_body(model, collected, usage),
        _ => openai_chat_body(model, collected, usage),
    }
}

fn grok_legacy_client_stream_body(
    client_format: &str,
    model: &str,
    collected: &GrokCollected,
    usage: GrokUsageEstimate,
) -> String {
    match client_format {
        "openai:responses" | "openai:responses:compact" => {
            openai_responses_sse(model, collected, usage)
        }
        "claude:messages" => claude_messages_sse(model, collected, usage),
        _ => openai_chat_sse(model, collected, usage),
    }
}

impl GrokStreamAdapter {
    fn push_chunk(&mut self, chunk: &[u8]) {
        self.buffered.push_str(&String::from_utf8_lossy(chunk));
        while let Some(index) = self.buffered.find('\n') {
            let line = self.buffered.drain(..=index).collect::<String>();
            self.handle_line(line.trim());
        }
    }

    fn finish(&mut self) {
        if !self.buffered.trim().is_empty() {
            let line = std::mem::take(&mut self.buffered);
            self.handle_line(line.trim());
        }
    }

    fn handle_line(&mut self, line: &str) {
        let line = line.trim();
        if line.is_empty() || line.starts_with("event:") {
            return;
        }
        let data = line.strip_prefix("data:").map(str::trim).unwrap_or(line);
        if data.is_empty() || data == "[DONE]" || !data.starts_with('{') {
            return;
        }
        let Ok(value) = serde_json::from_str::<Value>(data) else {
            return;
        };
        self.handle_event(&value);
    }

    fn handle_event(&mut self, value: &Value) {
        let Some(response) = value
            .get("result")
            .and_then(|result| result.get("response"))
        else {
            return;
        };
        self.handle_streaming_image_generation_response(response);
        self.handle_model_response_images(response);
        if let Some(card) = response.get("cardAttachment") {
            self.handle_card(card);
        }
        if let Some(token) = response.get("token").and_then(Value::as_str) {
            if response
                .get("isThinking")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                self.thinking.push_str(token);
            } else if response.get("messageTag").and_then(Value::as_str) == Some("final") {
                let cleaned = self.clean_token(token);
                self.text.push_str(&cleaned);
            }
        }
    }

    fn handle_streaming_image_generation_response(&mut self, response: &Value) {
        let Some(stream) = response.get("streamingImageGenerationResponse") else {
            return;
        };
        if stream
            .get("moderated")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return;
        }
        if stream
            .get("progress")
            .and_then(Value::as_u64)
            .unwrap_or_default()
            < 100
        {
            return;
        }
        let url = stream
            .get("assetId")
            .and_then(Value::as_str)
            .map(|asset_id| format!("{GROK_ASSET_BASE}{asset_id}/content"))
            .or_else(|| {
                stream
                    .get("imageUrl")
                    .and_then(Value::as_str)
                    .map(grok_asset_url)
            });
        if let Some(url) = url {
            self.push_image_url(url);
        }
    }

    fn handle_model_response_images(&mut self, response: &Value) {
        let Some(model_response) = response.get("modelResponse") else {
            return;
        };
        if let Some(urls) = model_response
            .get("generatedImageUrls")
            .and_then(Value::as_array)
        {
            for url in urls.iter().filter_map(Value::as_str) {
                self.push_image_url(grok_asset_url(url));
            }
        }
        if let Some(attachments) = model_response
            .get("fileAttachments")
            .and_then(Value::as_array)
        {
            for asset_id in attachments.iter().filter_map(Value::as_str) {
                self.push_image_url(format!("{GROK_ASSET_BASE}{asset_id}/content"));
            }
        }
    }

    fn handle_card(&mut self, card: &Value) {
        let Some(json_data) = card.get("jsonData").and_then(Value::as_str) else {
            return;
        };
        let Ok(value) = serde_json::from_str::<Value>(json_data) else {
            return;
        };
        if let Some(card_id) = value.get("id").and_then(Value::as_str) {
            self.cards.insert(
                card_id.to_string(),
                GrokCard {
                    url: value
                        .get("url")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                    title: value
                        .get("title")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                },
            );
        }
        let Some(chunk) = value.get("image_chunk") else {
            return;
        };
        if chunk.get("progress").and_then(Value::as_u64) != Some(100)
            || chunk
                .get("moderated")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        {
            return;
        }
        let Some(path) = chunk.get("imageUrl").and_then(Value::as_str) else {
            return;
        };
        let url = if path.starts_with("http://") || path.starts_with("https://") {
            path.to_string()
        } else {
            format!("{GROK_ASSET_BASE}{path}")
        };
        self.push_image_url(url);
    }

    fn push_image_url(&mut self, url: String) {
        if !url.trim().is_empty() && !self.images.iter().any(|item| item == &url) {
            self.images.push(url);
        }
    }

    fn clean_token(&mut self, token: &str) -> String {
        if !token.contains("<grok:render") {
            return token.to_string();
        }
        let replaced = grok_render_regex()
            .replace_all(token, |captures: &Captures<'_>| {
                self.render_replacement(
                    captures
                        .get(1)
                        .map(|value| value.as_str())
                        .unwrap_or_default(),
                    captures
                        .get(3)
                        .map(|value| value.as_str())
                        .unwrap_or_default(),
                )
            })
            .to_string();
        if replaced.starts_with('\n') && replaced.contains("[[") {
            replaced.trim_start_matches('\n').to_string()
        } else {
            replaced
        }
    }

    fn render_replacement(&mut self, card_id: &str, render_type: &str) -> String {
        let Some(card) = self.cards.get(card_id) else {
            return String::new();
        };
        match render_type {
            "render_inline_citation" => {
                let Some(url) = card.url.clone().filter(|value| !value.trim().is_empty()) else {
                    return String::new();
                };
                let index = self
                    .citation_order
                    .iter()
                    .position(|existing| existing == &url)
                    .map(|position| position + 1)
                    .unwrap_or_else(|| {
                        self.citation_order.push(url.clone());
                        self.citation_order.len()
                    });
                if self.last_citation_index == Some(index) {
                    return String::new();
                }
                self.last_citation_index = Some(index);
                let title = card.title.clone().unwrap_or_else(|| url.clone());
                format!(" [[{index}]]({url} \"{}\")", title.replace('"', "'"))
            }
            "render_searched_image" => {
                let Some(url) = card.url.clone().filter(|value| !value.trim().is_empty()) else {
                    return String::new();
                };
                let title = card.title.clone().unwrap_or_else(|| "image".to_string());
                format!("![{}]({url})", title.replace(['[', ']'], ""))
            }
            "render_generated_image" => String::new(),
            _ => String::new(),
        }
    }
}

fn openai_chat_body(model: &str, collected: &GrokCollected, usage: GrokUsageEstimate) -> Value {
    json!({
        "id": format!("chatcmpl-{}", Uuid::new_v4()),
        "object": "chat.completion",
        "created": current_unix_secs(),
        "model": model,
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": chat_text_with_images(collected),
            },
            "finish_reason": "stop",
        }],
        "usage": openai_chat_usage(usage),
    })
}

fn openai_chat_sse(model: &str, collected: &GrokCollected, usage: GrokUsageEstimate) -> String {
    let id = format!("chatcmpl-{}", Uuid::new_v4());
    let mut body = String::new();
    if !collected.thinking.is_empty() {
        push_sse_data(
            &mut body,
            &json!({
                "id": id,
                "object": "chat.completion.chunk",
                "created": current_unix_secs(),
                "model": model,
                "choices": [{
                    "index": 0,
                    "delta": {"role": "assistant", "reasoning_content": collected.thinking},
                }],
            }),
        );
    }
    push_sse_data(
        &mut body,
        &json!({
            "id": id,
            "object": "chat.completion.chunk",
            "created": current_unix_secs(),
            "model": model,
            "choices": [{
                "index": 0,
                "delta": {"role": "assistant", "content": chat_text_with_images(collected)},
            }],
        }),
    );
    push_sse_data(
        &mut body,
        &json!({
            "id": id,
            "object": "chat.completion.chunk",
            "created": current_unix_secs(),
            "model": model,
            "choices": [{
                "index": 0,
                "delta": {},
                "finish_reason": "stop",
            }],
            "usage": openai_chat_usage(usage),
        }),
    );
    body.push_str("data: [DONE]\n\n");
    body
}

fn openai_responses_body(
    model: &str,
    collected: &GrokCollected,
    usage: GrokUsageEstimate,
    images_as_generation_calls: bool,
) -> Value {
    let response_id = format!("resp_{}", Uuid::new_v4());
    let mut output = Vec::new();
    if !collected.thinking.trim().is_empty() {
        output.push(json!({
            "id": format!("{response_id}_rs_0"),
            "type": "reasoning",
            "status": "completed",
            "summary": [{
                "type": "summary_text",
                "text": collected.thinking.trim(),
            }],
        }));
    }
    let message_text = if images_as_generation_calls {
        collected.text.clone()
    } else {
        chat_text_with_images(collected)
    };
    if !message_text.trim().is_empty() {
        output.push(json!({
            "id": format!("{response_id}_msg"),
            "type": "message",
            "role": "assistant",
            "content": [{"type": "output_text", "text": message_text, "annotations": []}],
            "status": "completed",
        }));
    }
    if images_as_generation_calls {
        for (index, image) in collected.images.iter().enumerate() {
            output.push(grok_openai_responses_image_generation_item(
                response_id.as_str(),
                index,
                image.as_str(),
            ));
        }
    }
    json!({
        "id": response_id,
        "object": "response",
        "created_at": current_unix_secs(),
        "status": "completed",
        "model": model,
        "output": output,
        "usage": openai_responses_usage(usage),
    })
}

fn grok_openai_responses_image_generation_item(
    response_id: &str,
    index: usize,
    image: &str,
) -> Value {
    let mut item = Map::new();
    item.insert(
        "id".to_string(),
        Value::String(format!("{response_id}_ig_{index}")),
    );
    item.insert(
        "type".to_string(),
        Value::String("image_generation_call".to_string()),
    );
    item.insert("status".to_string(), Value::String("completed".to_string()));
    item.insert("action".to_string(), Value::String("generate".to_string()));
    if let Some((mime_type, b64_json)) = grok_data_image_parts(image) {
        item.insert("result".to_string(), Value::String(b64_json));
        item.insert(
            "output_format".to_string(),
            Value::String(grok_output_format_from_mime_type(mime_type.as_str())),
        );
        item.insert("mime_type".to_string(), Value::String(mime_type));
    } else {
        item.insert("url".to_string(), Value::String(image.to_string()));
        item.insert(
            "output_format".to_string(),
            Value::String("png".to_string()),
        );
    }
    Value::Object(item)
}

fn grok_output_format_from_mime_type(mime_type: &str) -> String {
    match mime_type.trim().to_ascii_lowercase().as_str() {
        "image/jpeg" | "image/jpg" => "jpeg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        _ => "png",
    }
    .to_string()
}

fn openai_responses_sse(
    model: &str,
    collected: &GrokCollected,
    usage: GrokUsageEstimate,
) -> String {
    let response_id = format!("resp_{}", Uuid::new_v4());
    let message_id = format!("msg_{}", Uuid::new_v4());
    let text = chat_text_with_images(collected);
    let response = openai_responses_body(model, collected, usage, false);
    let mut body = String::new();
    push_sse_event(
        &mut body,
        "response.created",
        &json!({"type": "response.created", "response": {
            "id": response_id,
            "object": "response",
            "created_at": current_unix_secs(),
            "status": "in_progress",
            "model": model,
            "output": [],
        }}),
    );
    push_sse_event(
        &mut body,
        "response.output_item.added",
        &json!({"type":"response.output_item.added","output_index":0,"item":{
            "id": message_id, "type":"message", "role":"assistant", "content":[], "status":"in_progress"
        }}),
    );
    push_sse_event(
        &mut body,
        "response.output_text.delta",
        &json!({"type":"response.output_text.delta","item_id":message_id,"output_index":0,"content_index":0,"delta":text}),
    );
    push_sse_event(
        &mut body,
        "response.completed",
        &json!({"type":"response.completed","response": response}),
    );
    body.push_str("data: [DONE]\n\n");
    body
}

fn claude_messages_body(model: &str, collected: &GrokCollected, usage: GrokUsageEstimate) -> Value {
    json!({
        "id": format!("msg_{}", Uuid::new_v4()),
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": [{"type": "text", "text": chat_text_with_images(collected)}],
        "stop_reason": "end_turn",
        "stop_sequence": Value::Null,
        "usage": {
            "input_tokens": usage.input_tokens,
            "output_tokens": usage.output_tokens,
        },
    })
}

fn claude_messages_sse(model: &str, collected: &GrokCollected, usage: GrokUsageEstimate) -> String {
    let message_id = format!("msg_{}", Uuid::new_v4());
    let mut body = String::new();
    let mut next_block_index = 0usize;

    push_sse_event(
        &mut body,
        "message_start",
        &json!({
            "type": "message_start",
            "message": {
                "id": message_id,
                "type": "message",
                "role": "assistant",
                "model": model,
                "content": [],
                "stop_reason": Value::Null,
                "stop_sequence": Value::Null,
                "usage": {
                    "input_tokens": 0,
                    "output_tokens": 0,
                },
            },
        }),
    );

    if !collected.thinking.is_empty() {
        let block_index = next_block_index;
        next_block_index += 1;
        push_sse_event(
            &mut body,
            "content_block_start",
            &json!({
                "type": "content_block_start",
                "index": block_index,
                "content_block": {
                    "type": "thinking",
                    "thinking": "",
                },
            }),
        );
        push_sse_event(
            &mut body,
            "content_block_delta",
            &json!({
                "type": "content_block_delta",
                "index": block_index,
                "delta": {
                    "type": "thinking_delta",
                    "thinking": collected.thinking,
                },
            }),
        );
        push_sse_event(
            &mut body,
            "content_block_stop",
            &json!({
                "type": "content_block_stop",
                "index": block_index,
            }),
        );
    }

    let text = chat_text_with_images(collected);
    if !text.is_empty() {
        let block_index = next_block_index;
        push_sse_event(
            &mut body,
            "content_block_start",
            &json!({
                "type": "content_block_start",
                "index": block_index,
                "content_block": {
                    "type": "text",
                    "text": "",
                },
            }),
        );
        push_sse_event(
            &mut body,
            "content_block_delta",
            &json!({
                "type": "content_block_delta",
                "index": block_index,
                "delta": {
                    "type": "text_delta",
                    "text": text,
                },
            }),
        );
        push_sse_event(
            &mut body,
            "content_block_stop",
            &json!({
                "type": "content_block_stop",
                "index": block_index,
            }),
        );
    }

    push_sse_event(
        &mut body,
        "message_delta",
        &json!({
            "type": "message_delta",
            "delta": {
                "stop_reason": "end_turn",
                "stop_sequence": Value::Null,
            },
            "usage": {
                "input_tokens": usage.input_tokens,
                "output_tokens": usage.output_tokens,
            },
        }),
    );
    push_sse_event(
        &mut body,
        "message_stop",
        &json!({
            "type": "message_stop",
        }),
    );
    body
}

fn openai_image_body(collected: &GrokCollected) -> Value {
    json!({
        "created": current_unix_secs(),
        "data": collected
            .images
            .iter()
            .map(|url| grok_openai_image_item(url.as_str()))
            .collect::<Vec<_>>(),
    })
}

fn grok_openai_image_item(url: &str) -> Value {
    if let Some((mime_type, b64_json)) = grok_data_image_parts(url) {
        return json!({
            "b64_json": b64_json,
            "mime_type": mime_type,
        });
    }
    json!({ "url": url })
}

async fn materialize_grok_image_assets(plan: &ExecutionPlan, collected: &mut GrokCollected) {
    if normalized_client_api_format(plan) != "openai:image" || collected.images.is_empty() {
        return;
    }

    let mut resolved_images = Vec::with_capacity(collected.images.len());
    for image_url in &collected.images {
        match grok_download_image_asset(plan, image_url.as_str()).await {
            Ok(Some(data_url)) => resolved_images.push(data_url),
            Ok(None) | Err(_) => resolved_images.push(image_url.clone()),
        }
    }
    collected.images = resolved_images;
}

async fn grok_download_image_asset(
    plan: &ExecutionPlan,
    raw_url: &str,
) -> Result<Option<String>, ExecutionRuntimeTransportError> {
    if !grok_image_asset_url_is_supported(raw_url) {
        return Ok(None);
    }
    let url = reqwest::Url::parse(raw_url).map_err(|err| {
        ExecutionRuntimeTransportError::UpstreamRequest(format!(
            "Grok image asset URL is invalid: {err}"
        ))
    })?;
    let mut download_plan = plan.clone();
    download_plan.method = "GET".to_string();
    download_plan.url = url.to_string();
    download_plan.headers.remove("content-type");
    download_plan
        .headers
        .insert("accept".to_string(), "image/*,*/*;q=0.8".to_string());
    download_plan.headers.insert(
        EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER.to_string(),
        "true".to_string(),
    );
    download_plan.body = RequestBody {
        json_body: None,
        body_bytes_b64: None,
        body_ref: None,
    };
    download_plan.stream = false;

    let response = send_request(&download_plan, Vec::new()).await?;
    if !(200..300).contains(&response.status_code()) {
        return Ok(None);
    }
    let headers = response.headers();
    let content_type = headers
        .get("content-type")
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| value.starts_with("image/"))
        .unwrap_or("image/png")
        .to_string();
    let bytes = response.bytes().await.map_err(|err| {
        ExecutionRuntimeTransportError::UpstreamRequest(format!(
            "Grok image asset download failed: {err}"
        ))
    })?;
    if bytes.is_empty() {
        return Ok(None);
    }
    if bytes.len() > GROK_MAX_ATTACHMENT_BYTES {
        return Err(ExecutionRuntimeTransportError::UpstreamRequest(format!(
            "Grok image asset exceeds {} byte limit",
            GROK_MAX_ATTACHMENT_BYTES
        )));
    }
    Ok(Some(format!(
        "data:{content_type};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )))
}

fn grok_image_asset_url_is_supported(raw_url: &str) -> bool {
    if raw_url.starts_with("data:image/") {
        return true;
    }
    let Ok(url) = reqwest::Url::parse(raw_url) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    if matches!(host, "assets.grok.com" | "assets.grokusercontent.com") {
        return true;
    }
    cfg!(test) && matches!(host, "localhost" | "127.0.0.1" | "::1")
}

fn grok_data_image_parts(raw_url: &str) -> Option<(String, String)> {
    let Some((header, data)) = raw_url.trim().split_once(',') else {
        return None;
    };
    if !header.starts_with("data:image/") || !header.contains(";base64") {
        return None;
    }
    let mime = header
        .strip_prefix("data:")
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .filter(|value| value.starts_with("image/"))?
        .to_string();
    let normalized = data.split_whitespace().collect::<String>();
    if normalized.is_empty() {
        return None;
    }
    Some((mime, normalized))
}

fn openai_image_sse(collected: &GrokCollected) -> String {
    let mut body = String::new();
    for (index, url) in collected.images.iter().enumerate() {
        push_sse_event(
            &mut body,
            "image_generation.completed",
            &json!({
                "type": "image_generation.completed",
                "url": url,
                "partial_image_index": index,
            }),
        );
    }
    body.push_str("data: [DONE]\n\n");
    body
}

fn chat_text_with_images(collected: &GrokCollected) -> String {
    let mut text = collected.text.clone();
    for image in &collected.images {
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(image);
    }
    text
}

fn grok_usage_estimate(plan: &ExecutionPlan, collected: &GrokCollected) -> GrokUsageEstimate {
    let input_tokens = plan
        .body
        .json_body
        .as_ref()
        .map(estimated_prompt_tokens)
        .unwrap_or_default();
    let reasoning_tokens = estimated_text_tokens(&collected.thinking);
    let output_tokens =
        estimated_text_tokens(&chat_text_with_images(collected)).saturating_add(reasoning_tokens);
    GrokUsageEstimate {
        input_tokens,
        output_tokens,
        reasoning_tokens,
    }
}

fn estimated_text_tokens(text: &str) -> u64 {
    if text.trim().is_empty() {
        return 0;
    }
    let chars = text.chars().count() as u64;
    ((chars + 3) / 4).max(1)
}

fn estimated_prompt_tokens(value: &Value) -> u64 {
    let Ok(text) = serde_json::to_string(value) else {
        return 0;
    };
    let tokens = estimated_text_tokens(&text);
    if tokens == 0 {
        0
    } else {
        tokens.saturating_add(GROK_PROMPT_OVERHEAD_TOKENS)
    }
}

fn openai_chat_usage(usage: GrokUsageEstimate) -> Value {
    json!({
        "prompt_tokens": usage.input_tokens,
        "completion_tokens": usage.output_tokens,
        "total_tokens": usage.input_tokens.saturating_add(usage.output_tokens),
        "prompt_tokens_details": {
            "cached_tokens": 0,
            "text_tokens": usage.input_tokens,
            "audio_tokens": 0,
            "image_tokens": 0,
        },
        "completion_tokens_details": {
            "text_tokens": usage.output_tokens.saturating_sub(usage.reasoning_tokens),
            "audio_tokens": 0,
            "reasoning_tokens": usage.reasoning_tokens,
        },
    })
}

fn openai_responses_usage(usage: GrokUsageEstimate) -> Value {
    json!({
        "input_tokens": usage.input_tokens,
        "output_tokens": usage.output_tokens,
        "total_tokens": usage.input_tokens.saturating_add(usage.output_tokens),
        "output_tokens_details": {
            "reasoning_tokens": usage.reasoning_tokens,
        },
    })
}

fn push_sse_data(body: &mut String, data: &Value) {
    body.push_str("data: ");
    body.push_str(&serde_json::to_string(data).unwrap_or_else(|_| "{}".to_string()));
    body.push_str("\n\n");
}

fn push_sse_event(body: &mut String, event: &str, data: &Value) {
    body.push_str("event: ");
    body.push_str(event);
    body.push('\n');
    push_sse_data(body, data);
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use aether_contracts::{ExecutionPlan, RequestBody, StreamFrame, StreamFramePayload};
    use axum::body::{Body, Bytes};
    use axum::extract::Request;
    use axum::routing::any;
    use axum::Router;
    use base64::Engine as _;
    use futures_util::{stream, StreamExt};
    use http::{Method, StatusCode};

    use super::{
        extract_grok_attachment_inputs, grok_aspect_ratio_from_provider_body, grok_asset_url,
        grok_attachment_ip_is_public, grok_client_json_body, grok_client_stream_body,
        grok_handle_imagine_ws_message, grok_image_count_from_provider_body,
        grok_image_prompt_from_provider_body, grok_imagine_request_message,
        grok_imagine_reset_message, grok_media_post_url,
        grok_plan_uses_structured_image_generation, grok_should_collect_image_stream,
        grok_should_use_imagine_websocket, grok_success_frame_stream, grok_upload_url,
        grok_upstream_model_name, grok_usage_estimate, grok_user_id_from_cookie_header,
        materialize_grok_image_assets, openai_chat_body, openai_image_body, openai_responses_body,
        set_grok_image_edit_config, GrokCollected, GrokImagineImage, GrokStreamAdapter,
    };

    fn sample_plan(body: serde_json::Value, client_api_format: &str) -> ExecutionPlan {
        ExecutionPlan {
            request_id: "req-1".to_string(),
            candidate_id: Some("cand-1".to_string()),
            provider_name: Some("Grok".to_string()),
            provider_id: "provider-1".to_string(),
            endpoint_id: "endpoint-1".to_string(),
            key_id: "key-1".to_string(),
            method: "POST".to_string(),
            url: "https://grok.com/rest/app-chat/conversations/new".to_string(),
            headers: BTreeMap::new(),
            content_type: Some("application/json".to_string()),
            content_encoding: None,
            body: RequestBody::from_json(body),
            stream: true,
            client_api_format: client_api_format.to_string(),
            provider_api_format: "openai:chat".to_string(),
            model_name: Some("grok-test".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        }
    }

    fn report_context_with_mapped_model(mapped_model: &str) -> serde_json::Value {
        serde_json::json!({
            "mapped_model": mapped_model,
            "provider_type": "grok",
        })
    }

    fn grok_token_chunk(token: &str) -> Bytes {
        Bytes::from(format!(
            "data: {}\n\n",
            serde_json::json!({
                "result": {
                    "response": {
                        "token": token,
                        "messageTag": "final"
                    }
                }
            })
        ))
    }

    async fn collect_decoded_data_frames(
        mut frame_stream: futures_util::stream::BoxStream<'static, Result<Bytes, std::io::Error>>,
    ) -> Vec<String> {
        let mut out = Vec::new();
        while let Some(item) = frame_stream.next().await {
            let bytes = item.expect("frame should encode");
            let line = String::from_utf8(bytes.to_vec()).expect("frame should be utf8");
            let frame: StreamFrame =
                serde_json::from_str(line.trim()).expect("frame should deserialize");
            if let StreamFramePayload::Data { chunk_b64, text } = frame.payload {
                let chunk = if let Some(chunk_b64) = chunk_b64 {
                    base64::engine::general_purpose::STANDARD
                        .decode(chunk_b64)
                        .expect("chunk should decode")
                } else {
                    text.unwrap_or_default().into_bytes()
                };
                out.push(String::from_utf8(chunk).expect("chunk should be utf8"));
            }
        }
        out
    }

    #[tokio::test]
    async fn grok_success_stream_forwards_token_chunks_incrementally() {
        let plan = sample_plan(
            serde_json::json!({
                "messages": [{"role": "user", "content": "hello"}],
                "stream": true,
            }),
            "openai:chat",
        );
        let upstream = stream::iter(vec![
            Ok(grok_token_chunk("hel")),
            Ok(grok_token_chunk("lo")),
        ])
        .boxed();

        let chunks = collect_decoded_data_frames(grok_success_frame_stream(
            plan,
            200,
            BTreeMap::new(),
            std::time::Instant::now(),
            upstream,
        ))
        .await;

        assert!(
            chunks
                .iter()
                .any(|chunk| chunk.contains("\"content\":\"hel\"")),
            "first upstream token should be emitted as its own client chunk: {chunks:?}"
        );
        assert!(
            chunks
                .iter()
                .any(|chunk| chunk.contains("\"content\":\"lo\"")),
            "second upstream token should be emitted as its own client chunk: {chunks:?}"
        );
        let joined = chunks.join("");
        let first = joined.find("\"content\":\"hel\"").expect("hel chunk");
        let second = joined.find("\"content\":\"lo\"").expect("lo chunk");
        let done = joined.find("data: [DONE]").expect("done chunk");
        assert!(first < second && second < done);
    }

    #[test]
    fn grok_upstream_model_name_prefers_report_context_mapping() {
        let mut plan = sample_plan(
            serde_json::json!({"messages": [{"role": "user", "content": "hello"}]}),
            "openai:chat",
        );
        plan.model_name = Some("grok-4.20-0309-reasoning".to_string());

        let mapped =
            grok_upstream_model_name(Some(&report_context_with_mapped_model("grok-4.20-fast")))
                .expect("mapped model should resolve");

        assert_eq!(mapped, "grok-4.20-fast");
    }

    #[test]
    fn grok_upstream_model_name_requires_report_context_mapping() {
        let err = grok_upstream_model_name(None).expect_err("missing mapped model should fail");

        assert!(err
            .to_string()
            .contains("Grok runtime requires mapped_model"));
    }

    #[test]
    fn adapter_extracts_text_and_image_url() {
        let image_json = serde_json::json!({
            "image_chunk": {
                "progress": 100,
                "imageUrl": "generated/example.png"
            }
        })
        .to_string();
        let line = format!(
            "data: {}\n",
            serde_json::json!({
                "result": {
                    "response": {
                        "token": "hello",
                        "messageTag": "final",
                        "cardAttachment": {"jsonData": image_json}
                    }
                }
            })
        );
        let mut adapter = GrokStreamAdapter::default();
        adapter.push_chunk(line.as_bytes());

        assert_eq!(adapter.text, "hello");
        assert_eq!(
            adapter.images,
            vec!["https://assets.grok.com/generated/example.png"]
        );
    }

    #[test]
    fn adapter_extracts_grok_image_edit_streaming_response() {
        let line = format!(
            "data: {}\n",
            serde_json::json!({
                "result": {
                    "response": {
                        "streamingImageGenerationResponse": {
                            "progress": 100,
                            "imageUrl": "generated/edit.png"
                        }
                    }
                }
            })
        );
        let mut adapter = GrokStreamAdapter::default();
        adapter.push_chunk(line.as_bytes());

        assert_eq!(
            adapter.images,
            vec!["https://assets.grok.com/generated/edit.png"]
        );
    }

    #[test]
    fn adapter_extracts_grok_image_edit_model_response_fallbacks() {
        let line = format!(
            "data: {}\n",
            serde_json::json!({
                "result": {
                    "response": {
                        "modelResponse": {
                            "generatedImageUrls": ["/generated/a.png"],
                            "fileAttachments": ["asset-123"]
                        }
                    }
                }
            })
        );
        let mut adapter = GrokStreamAdapter::default();
        adapter.push_chunk(line.as_bytes());

        assert_eq!(
            adapter.images,
            vec![
                "https://assets.grok.com/generated/a.png",
                "https://assets.grok.com/asset-123/content"
            ]
        );
    }

    #[test]
    fn grok_image_edit_config_sets_references_and_parent_post() {
        let mut body = serde_json::json!({
            "responseMetadata": {
                "modelConfigOverride": {
                    "modelMap": {
                        "imageEditModelConfig": {
                            "imageReferences": [],
                            "parentPostId": ""
                        }
                    }
                }
            }
        });

        set_grok_image_edit_config(
            &mut body,
            vec![serde_json::json!("https://assets.grok.com/ref.png")],
            "post-1".to_string(),
        )
        .expect("config should update");

        assert_eq!(
            body["responseMetadata"]["modelConfigOverride"]["modelMap"]["imageEditModelConfig"]
                ["imageReferences"][0],
            serde_json::json!("https://assets.grok.com/ref.png")
        );
        assert_eq!(
            body["responseMetadata"]["modelConfigOverride"]["modelMap"]["imageEditModelConfig"]
                ["parentPostId"],
            serde_json::json!("post-1")
        );
    }

    #[test]
    fn grok_image_edit_helpers_resolve_urls_and_cookie_user_id() {
        assert_eq!(
            grok_upload_url("https://grok.com/rest/app-chat/conversations/new"),
            "https://grok.com/rest/app-chat/upload-file"
        );
        assert_eq!(
            grok_media_post_url("https://grok.com/rest/app-chat/conversations/new"),
            "https://grok.com/rest/media/post/create"
        );
        assert_eq!(
            grok_asset_url("/users/u/file/content"),
            "https://assets.grok.com/users/u/file/content"
        );

        let headers = BTreeMap::from([(
            "cookie".to_string(),
            "sso=abc; x-userid=user-1; cf_clearance=ok".to_string(),
        )]);
        assert_eq!(
            grok_user_id_from_cookie_header(&headers),
            Some("user-1".to_string())
        );
    }

    #[test]
    fn grok_imagine_request_message_matches_websocket_protocol() {
        let reset = grok_imagine_reset_message();
        assert_eq!(reset["type"], serde_json::json!("conversation.item.create"));
        assert_eq!(
            reset["item"]["content"][0]["type"],
            serde_json::json!("reset")
        );

        let request = grok_imagine_request_message("a red chair", "16:9", true);
        assert_eq!(
            request["item"]["content"][0]["requestId"]
                .as_str()
                .is_some(),
            true
        );
        assert_eq!(
            request["item"]["content"][0]["text"],
            serde_json::json!("a red chair")
        );
        assert_eq!(
            request["item"]["content"][0]["properties"]["aspect_ratio"],
            serde_json::json!("16:9")
        );
        assert_eq!(
            request["item"]["content"][0]["properties"]["enable_pro"],
            serde_json::json!(true)
        );
    }

    #[test]
    fn grok_imagine_ws_parser_collects_completed_image() {
        let mut slots = BTreeMap::<String, GrokImagineImage>::new();
        grok_handle_imagine_ws_message(
            &serde_json::json!({
                "type": "json",
                "current_status": "start_stage",
                "image_id": "abc",
                "order": 1
            }),
            &mut slots,
        )
        .expect("start stage should parse");
        grok_handle_imagine_ws_message(
            &serde_json::json!({
                "type": "image",
                "url": "/images/abc.png",
                "blob": "aW1hZ2U="
            }),
            &mut slots,
        )
        .expect("image frame should parse");
        grok_handle_imagine_ws_message(
            &serde_json::json!({
                "type": "json",
                "current_status": "completed",
                "image_id": "abc",
                "order": 1,
                "moderated": false
            }),
            &mut slots,
        )
        .expect("completed frame should parse");

        let image = slots.get("abc").expect("slot should exist");
        assert_eq!(
            image.url.as_deref(),
            Some("https://assets.grok.com/images/abc.png")
        );
        assert_eq!(image.blob_b64.as_deref(), Some("aW1hZ2U="));
        assert!(image.done);
        assert!(!image.moderated);
    }

    #[test]
    fn grok_imagine_helpers_extract_prompt_count_and_route() {
        let body = serde_json::json!({
            "input": [{
                "role": "user",
                "content": [{"type": "input_text", "text": "a chair"}]
            }],
            "tools": [{"size": "1280x720"}],
            "n": 3
        });
        assert_eq!(
            grok_image_prompt_from_provider_body(&body),
            Some("a chair".to_string())
        );
        assert_eq!(grok_image_count_from_provider_body(&body), 3);
        assert_eq!(grok_aspect_ratio_from_provider_body(&body), "16:9");

        let plan = sample_plan(body, "openai:image");
        assert!(grok_should_use_imagine_websocket(
            &plan,
            Some(&report_context_with_mapped_model("grok-imagine-image-pro"))
        )
        .expect("route should resolve"));
        assert!(!grok_should_use_imagine_websocket(
            &plan,
            Some(&report_context_with_mapped_model("grok-imagine-image-lite"))
        )
        .expect("route should resolve"));
    }

    #[test]
    fn grok_attachment_public_ip_guard_rejects_private_ranges() {
        for ip in [
            "127.0.0.1",
            "10.0.0.1",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "::1",
            "fc00::1",
            "fe80::1",
        ] {
            assert!(
                !grok_attachment_ip_is_public(ip.parse().expect("ip should parse")),
                "{ip} should be rejected"
            );
        }
        assert!(grok_attachment_ip_is_public(
            "8.8.8.8".parse().expect("ip should parse")
        ));
        assert!(grok_attachment_ip_is_public(
            "2606:4700:4700::1111".parse().expect("ip should parse")
        ));
    }

    #[test]
    fn adapter_cleans_inline_citation_render_tags() {
        let card_json = serde_json::json!({
            "id": "803514",
            "url": "https://example.com/source",
            "title": "Example Source"
        })
        .to_string();
        let line = format!(
            "data: {}\n",
            serde_json::json!({
                "result": {
                    "response": {
                        "messageTag": "final",
                        "cardAttachment": {"jsonData": card_json},
                        "token": "answer<grok:render card_id=\"803514\" card_type=\"citation_card\" type=\"render_inline_citation\"><argument name=\"citation_id\">5</argument></grok:render>"
                    }
                }
            })
        );
        let mut adapter = GrokStreamAdapter::default();
        adapter.push_chunk(line.as_bytes());

        assert!(!adapter.text.contains("<grok:render"));
        assert!(adapter.text.contains("[[1]](https://example.com/source"));
    }

    #[test]
    fn openai_chat_body_includes_estimated_usage() {
        let plan = sample_plan(
            serde_json::json!({
                "messages": [{"role": "user", "content": "hello"}]
            }),
            "openai:chat",
        );
        let collected = GrokCollected {
            text: "hello back".to_string(),
            thinking: "thinking".to_string(),
            ..GrokCollected::default()
        };
        let usage = grok_usage_estimate(&plan, &collected);
        let body = openai_chat_body("grok-test", &collected, usage);

        assert!(body["usage"]["prompt_tokens"].as_u64().unwrap_or_default() > 0);
        assert!(
            body["usage"]["completion_tokens"]
                .as_u64()
                .unwrap_or_default()
                > 0
        );
        assert_eq!(
            body["usage"]["completion_tokens_details"]["reasoning_tokens"],
            serde_json::json!(usage.reasoning_tokens)
        );
    }

    #[test]
    fn openai_responses_body_includes_estimated_usage() {
        let plan = sample_plan(serde_json::json!({"input": "hello"}), "openai:responses");
        let collected = GrokCollected {
            text: "hello back".to_string(),
            thinking: "short reasoning".to_string(),
            ..GrokCollected::default()
        };
        let usage = grok_usage_estimate(&plan, &collected);
        let body = openai_responses_body("grok-test", &collected, usage, false);

        assert_eq!(
            body["usage"]["input_tokens"],
            serde_json::json!(usage.input_tokens)
        );
        assert_eq!(
            body["usage"]["output_tokens"],
            serde_json::json!(usage.output_tokens)
        );
        assert_eq!(
            body["usage"]["output_tokens_details"]["reasoning_tokens"],
            serde_json::json!(usage.reasoning_tokens)
        );
        assert_eq!(body["output"][0]["type"], serde_json::json!("reasoning"));
        assert_eq!(body["output"][1]["type"], serde_json::json!("message"));
    }

    #[test]
    fn openai_responses_body_emits_structured_image_generation_calls() {
        let plan = sample_plan(serde_json::json!({"input": "draw"}), "openai:responses");
        let collected = GrokCollected {
            text: "done".to_string(),
            images: vec!["data:image/png;base64,aW1hZ2U=".to_string()],
            ..GrokCollected::default()
        };
        let usage = grok_usage_estimate(&plan, &collected);
        let body = openai_responses_body("grok-imagine-image-lite", &collected, usage, true);

        assert_eq!(body["output"][0]["type"], serde_json::json!("message"));
        assert_eq!(
            body["output"][1]["type"],
            serde_json::json!("image_generation_call")
        );
        assert_eq!(body["output"][1]["result"], serde_json::json!("aW1hZ2U="));
        assert_eq!(body["output"][1]["output_format"], serde_json::json!("png"));
    }

    #[test]
    fn openai_responses_body_preserves_url_images_without_result_field() {
        let plan = sample_plan(serde_json::json!({"input": "draw"}), "openai:responses");
        let collected = GrokCollected {
            images: vec!["https://assets.grok.com/generated/example.png".to_string()],
            ..GrokCollected::default()
        };
        let usage = grok_usage_estimate(&plan, &collected);
        let body = openai_responses_body("grok-imagine-image-lite", &collected, usage, true);

        assert_eq!(
            body["output"][0]["type"],
            serde_json::json!("image_generation_call")
        );
        assert_eq!(
            body["output"][0]["url"],
            serde_json::json!("https://assets.grok.com/generated/example.png")
        );
        assert!(body["output"][0].get("result").is_none());
    }

    #[test]
    fn openai_responses_body_preserves_text_whitespace() {
        let plan = sample_plan(serde_json::json!({"input": "hello"}), "openai:responses");
        let collected = GrokCollected {
            text: "\n  hello back  \n".to_string(),
            ..GrokCollected::default()
        };
        let usage = grok_usage_estimate(&plan, &collected);
        let body = openai_responses_body("grok-test", &collected, usage, false);

        assert_eq!(
            body["output"][0]["content"][0]["text"],
            serde_json::json!("\n  hello back  \n")
        );
    }

    #[test]
    fn openai_responses_body_keeps_non_image_intent_images_as_text() {
        let plan = sample_plan(
            serde_json::json!({"input": "show source"}),
            "openai:responses",
        );
        let collected = GrokCollected {
            text: "source".to_string(),
            images: vec!["https://assets.grok.com/generated/example.png".to_string()],
            ..GrokCollected::default()
        };
        let usage = grok_usage_estimate(&plan, &collected);
        let body = openai_responses_body("grok-test", &collected, usage, false);

        assert_eq!(body["output"][0]["type"], serde_json::json!("message"));
        assert_eq!(
            body["output"][0]["content"][0]["text"],
            serde_json::json!("source\n\nhttps://assets.grok.com/generated/example.png")
        );
        assert_eq!(body["output"].as_array().map(Vec::len), Some(1));
    }

    #[test]
    fn openai_chat_json_body_uses_standard_responses_conversion() {
        let plan = sample_plan(
            serde_json::json!({
                "messages": [{"role": "user", "content": "hello"}]
            }),
            "openai:chat",
        );
        let collected = GrokCollected {
            status_code: 200,
            text: "hello back".to_string(),
            thinking: "Thinking about your request".to_string(),
            ..GrokCollected::default()
        };
        let body = grok_client_json_body(&plan, &collected, None);

        assert_eq!(body["object"], serde_json::json!("chat.completion"));
        assert_eq!(
            body["choices"][0]["message"]["content"],
            serde_json::json!("hello back")
        );
        assert_eq!(
            body["usage"]["completion_tokens_details"]["reasoning_tokens"],
            serde_json::json!(grok_usage_estimate(&plan, &collected).reasoning_tokens)
        );
    }

    #[test]
    fn openai_chat_json_body_converts_grok_images_through_standard_matrix() {
        let plan = sample_plan(
            serde_json::json!({
                "messages": [{"role": "user", "content": "draw"}]
            }),
            "openai:chat",
        );
        let mut plan = plan;
        plan.model_name = Some("grok-imagine-image-lite".to_string());
        let collected = GrokCollected {
            status_code: 200,
            images: vec!["data:image/png;base64,aW1hZ2U=".to_string()],
            ..GrokCollected::default()
        };
        let body = grok_client_json_body(&plan, &collected, None);

        assert_eq!(body["object"], serde_json::json!("chat.completion"));
        assert_eq!(
            body["choices"][0]["message"]["content"][0]["type"],
            serde_json::json!("image_url")
        );
        assert_eq!(
            body["choices"][0]["message"]["content"][0]["image_url"]["url"],
            serde_json::json!("data:image/png;base64,aW1hZ2U=")
        );
    }

    #[test]
    fn openai_chat_json_body_uses_mapped_image_model_for_alias() {
        let mut plan = sample_plan(
            serde_json::json!({
                "messages": [{"role": "user", "content": "draw"}]
            }),
            "openai:chat",
        );
        plan.model_name = Some("custom-image-alias".to_string());
        let report_context = report_context_with_mapped_model("grok-imagine-image-lite");
        let collected = GrokCollected {
            status_code: 200,
            images: vec!["data:image/png;base64,aW1hZ2U=".to_string()],
            ..GrokCollected::default()
        };
        let body = grok_client_json_body(&plan, &collected, Some(&report_context));

        assert_eq!(body["object"], serde_json::json!("chat.completion"));
        assert_eq!(
            body["choices"][0]["message"]["content"][0]["type"],
            serde_json::json!("image_url")
        );
    }

    #[test]
    fn openai_responses_stream_uses_aether_standard_emitter() {
        let plan = sample_plan(
            serde_json::json!({"input": "hello", "stream": true}),
            "openai:responses",
        );
        let collected = GrokCollected {
            status_code: 200,
            text: "hello back".to_string(),
            thinking: "Thinking about your request".to_string(),
            ..GrokCollected::default()
        };
        let body = grok_client_stream_body(&plan, &collected, None);

        assert!(body.contains("event: response.created"));
        assert!(body.contains("event: response.in_progress"));
        assert!(body.contains("event: response.reasoning_summary_part.added"));
        assert!(body.contains("event: response.content_part.added"));
        assert!(body.contains("event: response.output_text.done"));
        assert!(body.contains("event: response.completed"));
        assert!(body.contains("\"sequence_number\""));
        assert!(!body.contains("chat.completion.chunk"));
    }

    #[test]
    fn openai_responses_stream_preserves_image_generation_calls() {
        let plan = sample_plan(
            serde_json::json!({
                "input": "draw",
                "stream": true,
                "tools": [{"type": "image_generation"}]
            }),
            "openai:responses",
        );
        let collected = GrokCollected {
            status_code: 200,
            images: vec!["data:image/png;base64,aGVsbG8=".to_string()],
            ..GrokCollected::default()
        };
        let body = grok_client_stream_body(&plan, &collected, None);

        assert!(body.contains("event: response.output_item.done"));
        assert!(body.contains("\"type\":\"image_generation_call\""));
        assert!(body.contains("\"result\":\"aGVsbG8=\""));
        assert!(body.contains("event: response.completed"));
    }

    #[test]
    fn openai_responses_stream_uses_mapped_image_model_for_alias() {
        let mut plan = sample_plan(
            serde_json::json!({
                "input": "draw",
                "stream": true
            }),
            "openai:responses",
        );
        plan.model_name = Some("custom-image-alias".to_string());
        let report_context = report_context_with_mapped_model("grok-imagine-image-lite");
        let collected = GrokCollected {
            status_code: 200,
            images: vec!["data:image/png;base64,aGVsbG8=".to_string()],
            ..GrokCollected::default()
        };
        let body = grok_client_stream_body(&plan, &collected, Some(&report_context));

        assert!(grok_plan_uses_structured_image_generation(
            &plan,
            Some(&report_context)
        ));
        assert!(body.contains("event: response.output_item.done"));
        assert!(body.contains("\"type\":\"image_generation_call\""));
        assert!(body.contains("\"result\":\"aGVsbG8=\""));
    }

    #[test]
    fn grok_lite_image_responses_stream_collects_before_client_bridge() {
        let mut plan = sample_plan(
            serde_json::json!({
                "input": "draw",
                "stream": true,
                "tools": [{"type": "image_generation"}]
            }),
            "openai:responses",
        );
        plan.model_name = Some("grok-imagine-image-lite".to_string());

        assert!(grok_plan_uses_structured_image_generation(&plan, None));
        assert!(grok_should_collect_image_stream(
            &plan,
            Some(&report_context_with_mapped_model("grok-imagine-image-lite"))
        )
        .expect("collect decision should succeed"));
    }

    #[test]
    fn grok_lite_alias_responses_stream_collects_from_mapped_model() {
        let mut plan = sample_plan(
            serde_json::json!({
                "input": "draw",
                "stream": true
            }),
            "openai:responses",
        );
        plan.model_name = Some("custom-image-alias".to_string());

        assert!(!grok_plan_uses_structured_image_generation(&plan, None));
        assert!(grok_should_collect_image_stream(
            &plan,
            Some(&report_context_with_mapped_model("grok-imagine-image-lite"))
        )
        .expect("collect decision should succeed"));
    }

    #[test]
    fn claude_messages_stream_uses_claude_event_shape() {
        let plan = sample_plan(
            serde_json::json!({"messages": [{"role": "user", "content": "hello"}]}),
            "claude:messages",
        );
        let collected = GrokCollected {
            status_code: 200,
            text: "hello back".to_string(),
            thinking: "Thinking about your request".to_string(),
            ..GrokCollected::default()
        };
        let body = grok_client_stream_body(&plan, &collected, None);

        assert!(body.contains("event: message_start"));
        assert!(body.contains("\"type\":\"message_start\""));
        assert!(body.contains("\"type\":\"thinking_delta\""));
        assert!(body.contains("\"type\":\"text_delta\""));
        assert!(body.contains("event: message_delta"));
        assert!(body.contains("event: message_stop"));
        assert!(!body.contains("chat.completion.chunk"));
    }

    #[test]
    fn extracts_openai_chat_image_and_file_attachment_inputs() {
        let inputs = extract_grok_attachment_inputs(
            "openai:chat",
            &serde_json::json!({
                "messages": [{
                    "role": "user",
                    "content": [
                        {"type": "text", "text": "describe these"},
                        {"type": "image_url", "image_url": {"url": "data:image/png;base64,aGVsbG8="}},
                        {"type": "file", "file": {"filename": "notes.txt", "file_data": "data:text/plain;base64,bm90ZXM="}}
                    ]
                }]
            }),
        );

        assert_eq!(inputs.len(), 2);
        assert_eq!(inputs[0].source.as_str(), "data:image/png;base64,aGVsbG8=");
        assert_eq!(inputs[1].filename.as_deref(), Some("notes.txt"));
        assert_eq!(inputs[1].source.as_str(), "data:text/plain;base64,bm90ZXM=");
    }

    #[test]
    fn extracts_responses_and_claude_attachment_inputs() {
        let responses = extract_grok_attachment_inputs(
            "openai:responses",
            &serde_json::json!({
                "input": [{
                    "type": "message",
                    "role": "user",
                    "content": [
                        {"type": "input_text", "text": "read it"},
                        {"type": "input_image", "image_url": "https://example.com/a.png"},
                        {"type": "input_file", "filename": "doc.pdf", "file_data": "data:application/pdf;base64,JVBERi0="}
                    ]
                }]
            }),
        );
        let claude = extract_grok_attachment_inputs(
            "claude:messages",
            &serde_json::json!({
                "messages": [{
                    "role": "user",
                    "content": [
                        {"type": "text", "text": "read"},
                        {"type": "image", "source": {"type": "url", "url": "https://example.com/b.png"}},
                        {"type": "document", "filename": "memo.pdf", "source": {"type": "base64", "media_type": "application/pdf", "data": "JVBERi0="}}
                    ]
                }]
            }),
        );

        assert_eq!(responses.len(), 2);
        assert_eq!(responses[0].source.as_str(), "https://example.com/a.png");
        assert_eq!(responses[1].filename.as_deref(), Some("doc.pdf"));
        assert_eq!(claude.len(), 2);
        assert_eq!(claude[0].source.as_str(), "https://example.com/b.png");
        assert_eq!(claude[1].filename.as_deref(), Some("memo.pdf"));
        assert_eq!(
            claude[1].source.as_str(),
            "data:application/pdf;base64,JVBERi0="
        );
    }

    fn response(
        status: StatusCode,
        content_type: &'static str,
        body: impl Into<Body>,
    ) -> http::Response<Body> {
        http::Response::builder()
            .status(status)
            .header("content-type", content_type)
            .body(body.into())
            .expect("response should build")
    }

    #[tokio::test]
    async fn grok_openai_image_sync_materializes_public_asset_urls_for_preview() {
        let listener = crate::test_support::bind_loopback_listener()
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("local addr should resolve");
        let app = Router::new().fallback(any(|request: Request| async move {
            let path = request.uri().path().to_string();
            let method = request.method().clone();
            match (method, path.as_str()) {
                (Method::GET, "/generated.png") => response(
                    StatusCode::OK,
                    "image/png",
                    Body::from(vec![0x89, b'P', b'N', b'G']),
                ),
                _ => response(StatusCode::NOT_FOUND, "text/plain", "not found"),
            }
        }));
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("mock server should run");
        });

        let plan = sample_plan(
            serde_json::json!({"model": "grok-imagine-image-lite"}),
            "openai:image",
        );
        let mut collected = GrokCollected {
            status_code: 200,
            images: vec![format!("http://{addr}/generated.png")],
            ..GrokCollected::default()
        };

        materialize_grok_image_assets(&plan, &mut collected).await;

        server.abort();

        assert!(collected.images[0].starts_with("data:image/png;base64,"));
        let body = openai_image_body(&collected);
        assert_eq!(body["data"][0]["b64_json"].as_str().is_some(), true);
        assert!(body["data"][0].get("url").is_none());
    }

    #[test]
    fn grok_openai_image_body_preserves_plain_urls_when_asset_is_not_materialized() {
        let body = openai_image_body(&GrokCollected {
            status_code: 200,
            images: vec!["https://assets.grok.com/generated/example.png".to_string()],
            ..GrokCollected::default()
        });

        assert_eq!(
            body["data"][0]["url"].as_str(),
            Some("https://assets.grok.com/generated/example.png")
        );
        assert!(body["data"][0].get("b64_json").is_none());
    }
}
