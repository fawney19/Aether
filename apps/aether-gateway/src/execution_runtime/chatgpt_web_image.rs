use std::collections::{BTreeMap, BTreeSet};
use std::io::Error as IoError;
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use aether_admin::provider::quota::{
    parse_chatgpt_web_conversation_init_response, quota_refresh_success_invalid_state,
};
use aether_admin::provider::redaction::admin_provider_metadata_bucket_safe_json;
use aether_contracts::{
    ExecutionPlan, ExecutionResult, ExecutionStreamTerminalSummary, ExecutionTelemetry,
    ExecutionTimeouts, ProxySnapshot, RequestBody, ResolvedTransportProfile, ResponseBody,
    StreamFrame, StreamFramePayload, StreamFrameType, TRANSPORT_BACKEND_BROWSER_WREQ,
    TRANSPORT_HTTP_MODE_AUTO, TRANSPORT_POOL_SCOPE_KEY,
};
use aether_data_contracts::repository::provider_catalog::ProviderCatalogKeyRuntimeMetadataUpdate;
use aether_provider_pool::{
    build_chatgpt_web_pool_quota_request, normalize_chatgpt_web_image_quota_limit,
    ProviderPoolQuotaRequestSpec,
};
use axum::body::Bytes;
use base64::Engine as _;
use chrono::{FixedOffset, Utc};
use futures_util::stream::{self, BoxStream};
use futures_util::StreamExt;
use serde_json::{json, Map, Value};
use tracing::{debug, warn};
use uuid::Uuid;

use crate::ai_serving::api::StreamingStandardTerminalObserver;
use crate::clock::current_unix_secs;
use crate::execution_runtime::ndjson::encode_stream_frame_ndjson;
use crate::execution_runtime::transport::{
    decode_base64_body_with_limit, format_upstream_request_error, json_value_fits_serialized_limit,
    maximum_base64_len_for_decoded_limit, safe_transport_error_message,
    serialize_json_body_with_limit, with_non_stream_total_timeout, DirectSyncExecutionRuntime,
    ExecutionRuntimeTransportError,
};
use crate::handlers::shared::{
    sync_provider_key_oauth_status_snapshot, sync_provider_key_quota_status_snapshot,
};
use crate::AppState;

const CHATGPT_WEB_INTERNAL_HEADER: &str = "x-aether-chatgpt-web-image";
const CHATGPT_WEB_DEFAULT_BASE_URL: &str = "https://chatgpt.com";
const CHATGPT_WEB_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36 Edg/143.0.0.0";
const CHATGPT_WEB_CLIENT_VERSION: &str = "prod-be885abbfcfe7b1f511e88b3003d9ee44757fbad";
const CHATGPT_WEB_BUILD_NUMBER: &str = "5955942";
const CHATGPT_WEB_SEC_CH_UA: &str =
    r#""Microsoft Edge";v="143", "Chromium";v="143", "Not A(Brand";v="24""#;
const CHATGPT_WEB_BROWSER_PROFILE: &str = "chrome143";
const CHATGPT_WEB_QUOTA_REFRESH_TIMEOUT_MS: u64 = 30_000;
const CHATGPT_WEB_QUOTA_REFRESH_PROXY_TIMEOUT_MS: u64 = 60_000;
const RUNTIME_METADATA_CAS_MAX_ATTEMPTS: usize = 16;
const GPT_IMAGE2_TOKEN_MIN_PIXELS: u64 = 655_360;
const GPT_IMAGE2_TOKEN_MAX_PIXELS: u64 = 8_294_400;
const GPT_IMAGE2_TOKEN_MAX_EDGE: u64 = 3_840;
const GPT_IMAGE2_TOKEN_MAX_ASPECT_RATIO: u64 = 3;
const GPT_IMAGE2_PARTIAL_IMAGE_OUTPUT_TOKENS: u64 = 100;
const CHATGPT_WEB_IMAGE_DOWNLOAD_MAX_REDIRECTS: usize = 10;
// A generated SSE response contains the image's base64 text plus JSON/event
// framing.  Keep its decoded envelope bounded independently from the raw image
// limit, while retaining support for a raw image up to the default 64 MiB cap.
const CHATGPT_WEB_IMAGE_SSE_WRAPPER_OVERHEAD_BYTES: usize = 256 * 1024;
const CHATGPT_WEB_IMAGE_SSE_HARD_MAX_BYTES: usize = 128 * 1024 * 1024;
const CHATGPT_WEB_IMAGE_STREAM_CHUNK_BYTES: usize = 1024 * 1024;
const CHATGPT_WEB_IMAGE_PUBLIC_CONNECT_TIMEOUT_MS: u64 = 10_000;
const CHATGPT_WEB_IMAGE_PUBLIC_READ_TIMEOUT_MS: u64 = 30_000;
const CHATGPT_WEB_IMAGE_PUBLIC_TOTAL_TIMEOUT_MS: u64 = 300_000;
const CHATGPT_WEB_OPAQUE_ID_MAX_BYTES: usize = 256;
const CHATGPT_WEB_IMAGE_MAX_UPLOAD_URL_BYTES: usize = 64 * 1024;
const CHATGPT_WEB_IMAGE_UPLOAD_RESPONSE_LIMIT_BYTES: usize = 64 * 1024;
const CHATGPT_WEB_IMAGE_MAX_PROMPT_BYTES: usize = 32 * 1024;
const CHATGPT_WEB_IMAGE_MAX_MODEL_BYTES: usize = 256;
const CHATGPT_WEB_IMAGE_MAX_OPTION_BYTES: usize = 128;
const CHATGPT_WEB_IMAGE_MAX_EXTERNAL_URL_BYTES: usize = 64 * 1024;
const CHATGPT_WEB_IMAGE_MAX_INPUT_IMAGES: usize = 16;
const CHATGPT_WEB_IMAGE_MAX_DIMENSION: u32 = 16_384;
// Values in a provider SSE/poll response are merged across the initial
// response and up to 24 follow-up polls.  Keep the retained candidate set
// bounded independently of the per-response body limit so a peer cannot make
// the gateway grow memory over the lifetime of one request.
const CHATGPT_WEB_IMAGE_SUMMARY_MAX_ITEMS: usize = 256;
const CHATGPT_WEB_IMAGE_SUMMARY_MAX_DIRECT_URLS: usize = 16;
const CHATGPT_WEB_IMAGE_SUMMARY_MAX_ID_BYTES: usize = 1024 * 1024;
const CHATGPT_WEB_IMAGE_SUMMARY_MAX_TEXT_BYTES: usize = 64 * 1024;

pub(crate) struct ChatGptWebImageStream {
    pub(crate) frame_stream: BoxStream<'static, Result<Bytes, IoError>>,
    pub(crate) report_context: Option<Value>,
}

#[derive(Debug, Clone)]
struct WebFingerprint {
    user_agent: &'static str,
    device_id: String,
    session_id: String,
}

#[derive(Debug, Clone, Default)]
struct WebRequirement {
    token: String,
    proof_token: Option<String>,
    so_token: Option<String>,
}

#[derive(Debug, Clone)]
struct WebUploadMeta {
    file_id: String,
    library_file_id: Option<String>,
    file_name: String,
    file_size: usize,
    mime: String,
    width: Option<u32>,
    height: Option<u32>,
}

#[derive(Debug, Clone, Default)]
struct WebImageSseSummary {
    conversation_id: Option<String>,
    file_ids: Vec<String>,
    sediment_ids: Vec<String>,
    direct_urls: Vec<String>,
    failure: Option<Value>,
    last_text: Option<String>,
}

#[derive(Debug, Clone, Copy)]
enum WebImageSummaryCollection {
    FileId,
    SedimentId,
    DirectUrl,
}

impl WebImageSseSummary {
    fn retained_item_count(&self) -> usize {
        self.file_ids
            .len()
            .saturating_add(self.sediment_ids.len())
            .saturating_add(self.direct_urls.len())
    }

    fn retained_value_bytes(&self) -> usize {
        saturating_string_bytes(&self.file_ids)
            .saturating_add(saturating_string_bytes(&self.sediment_ids))
            .saturating_add(saturating_string_bytes(&self.direct_urls))
    }

    fn add_values<I>(&mut self, collection: WebImageSummaryCollection, incoming: I)
    where
        I: IntoIterator<Item = String>,
    {
        for value in incoming {
            self.add_value(collection, value);
        }
    }

    fn add_value(&mut self, collection: WebImageSummaryCollection, value: String) {
        if value.is_empty() {
            return;
        }
        let (max_items, collection_budget) = match collection {
            WebImageSummaryCollection::FileId | WebImageSummaryCollection::SedimentId => (
                CHATGPT_WEB_IMAGE_SUMMARY_MAX_ITEMS,
                CHATGPT_WEB_IMAGE_SUMMARY_MAX_ID_BYTES,
            ),
            WebImageSummaryCollection::DirectUrl if is_data_image_reference(&value) => {
                (4, chatgpt_web_image_sse_envelope_limit_bytes())
            }
            WebImageSummaryCollection::DirectUrl => (
                CHATGPT_WEB_IMAGE_SUMMARY_MAX_DIRECT_URLS,
                CHATGPT_WEB_IMAGE_MAX_EXTERNAL_URL_BYTES,
            ),
        };
        // Reject an over-sized value before it can become part of the retained
        // set.  The caller may have had to materialize it to parse a JSON
        // field, but this prevents repeated poll responses from accumulating
        // it and bounds the final synthetic SSE envelope.
        if value.len() > collection_budget
            || self.retained_item_count() >= CHATGPT_WEB_IMAGE_SUMMARY_MAX_ITEMS
        {
            return;
        }
        let values = match collection {
            WebImageSummaryCollection::FileId => &self.file_ids,
            WebImageSummaryCollection::SedimentId => &self.sediment_ids,
            WebImageSummaryCollection::DirectUrl => &self.direct_urls,
        };
        if values.len() >= max_items || values.iter().any(|existing| existing == &value) {
            return;
        }
        let collection_bytes = match collection {
            WebImageSummaryCollection::FileId => saturating_string_bytes(&self.file_ids),
            WebImageSummaryCollection::SedimentId => saturating_string_bytes(&self.sediment_ids),
            WebImageSummaryCollection::DirectUrl => saturating_string_bytes(&self.direct_urls),
        };
        let total_budget = chatgpt_web_image_sse_envelope_limit_bytes()
            .saturating_add(CHATGPT_WEB_IMAGE_SUMMARY_MAX_ID_BYTES);
        if value.len() > collection_budget.saturating_sub(collection_bytes)
            || value.len() > total_budget.saturating_sub(self.retained_value_bytes())
        {
            return;
        }
        match collection {
            WebImageSummaryCollection::FileId => self.file_ids.push(value),
            WebImageSummaryCollection::SedimentId => self.sediment_ids.push(value),
            WebImageSummaryCollection::DirectUrl => self.direct_urls.push(value),
        }
    }
}

fn saturating_string_bytes(values: &[String]) -> usize {
    values
        .iter()
        .fold(0usize, |total, value| total.saturating_add(value.len()))
}

fn is_data_image_reference(value: &str) -> bool {
    value
        .get(..11)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("data:image/"))
}

#[derive(Debug, Clone)]
struct DownloadedImage {
    b64_json: String,
    mime: String,
    width: Option<u32>,
    height: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WebImageDownloadTrust {
    UntrustedInput,
    ProviderOutput,
}

struct WebImageHttpPayload {
    data: Vec<u8>,
    content_type: Option<String>,
}

pub(crate) async fn maybe_execute_chatgpt_web_image_sync(
    state: &AppState,
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
) -> Result<Option<ExecutionResult>, ExecutionRuntimeTransportError> {
    if !is_chatgpt_web_image_plan(plan, report_context) {
        return Ok(None);
    }
    with_non_stream_total_timeout(plan, async move {
        let started_at = Instant::now();
        let result = match execute_chatgpt_web_image(state, plan, report_context, started_at).await
        {
            Ok(result) => result,
            Err(ExecutionRuntimeTransportError::UpstreamHttpStatus {
                status_code,
                message,
            }) => chatgpt_web_http_error_execution_result(
                plan,
                started_at,
                status_code,
                message.as_str(),
            ),
            Err(error) => return Err(error),
        };
        Ok(Some(result))
    })
    .await
}

pub(crate) async fn maybe_execute_chatgpt_web_image_stream(
    state: &AppState,
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
) -> Result<Option<ChatGptWebImageStream>, ExecutionRuntimeTransportError> {
    if !is_chatgpt_web_image_plan(plan, report_context) {
        return Ok(None);
    }
    let started_at = Instant::now();
    let result = match execute_chatgpt_web_image(state, plan, report_context, started_at).await {
        Ok(result) => result,
        Err(ExecutionRuntimeTransportError::UpstreamHttpStatus {
            status_code,
            message,
        }) => {
            chatgpt_web_http_error_execution_result(plan, started_at, status_code, message.as_str())
        }
        Err(error) => return Err(error),
    };
    Ok(Some(ChatGptWebImageStream {
        frame_stream: execution_result_frame_stream(plan, &result, report_context)?,
        report_context: report_context.cloned(),
    }))
}

fn is_chatgpt_web_image_plan(plan: &ExecutionPlan, report_context: Option<&Value>) -> bool {
    if !plan
        .provider_api_format
        .eq_ignore_ascii_case("openai:image")
    {
        return false;
    }
    let header_marker = plan.headers.iter().any(|(name, value)| {
        name.eq_ignore_ascii_case(CHATGPT_WEB_INTERNAL_HEADER) && value == "1"
    });
    let context_marker = report_context
        .and_then(|value| value.get("chatgpt_web_image"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    header_marker || context_marker
}

async fn execute_chatgpt_web_image(
    state: &AppState,
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
    started_at: Instant,
) -> Result<ExecutionResult, ExecutionRuntimeTransportError> {
    let body = plan.body.json_body.as_ref().ok_or_else(|| {
        ExecutionRuntimeTransportError::UpstreamRequest(
            "ChatGPT-Web image plan missing internal request body".to_string(),
        )
    })?;
    if let Some(error) = body.get("error") {
        return Ok(json_execution_result(
            plan,
            400,
            json!({ "error": error }),
            started_at,
        ));
    }

    let request = ChatGptWebImageRequest::from_body(body)?;
    let base_url = chatgpt_web_base_url_from_plan(plan);
    let token = bearer_token_from_headers(&plan.headers).unwrap_or_default();
    let fp = WebFingerprint::new();

    debug!(
        event_name = "chatgpt_web_image_start",
        log_type = "debug",
        request_id = %plan.request_id,
        candidate_id = ?plan.candidate_id,
        upstream_origin = %crate::handlers::shared::security_log_url_origin(&base_url),
        operation = %request.operation,
        image_count = request.images.len(),
        size = %request.size,
        ratio = %request.ratio,
        "gateway executing ChatGPT-Web image request"
    );

    web_bootstrap(plan, &base_url, &fp).await?;
    let requirements = web_requirements(plan, &base_url, &fp, token.as_str()).await?;
    let mut uploads = Vec::new();
    for (index, image) in request.images.iter().enumerate() {
        uploads.push(
            web_upload_image(
                state,
                plan,
                &base_url,
                &fp,
                token.as_str(),
                image,
                format!("image_{}.png", index + 1),
            )
            .await?,
        );
    }

    let conduit = web_prepare_conversation(
        plan,
        &base_url,
        &fp,
        token.as_str(),
        &requirements,
        request.web_model.as_str(),
    )
    .await?;
    let mut summary = web_start_conversation(
        plan,
        &base_url,
        &fp,
        token.as_str(),
        &requirements,
        conduit.as_str(),
        &request,
        &uploads,
    )
    .await?;
    apply_chatgpt_web_image_quota_request_delta_after_conversation_start(state, plan).await;
    spawn_chatgpt_web_image_quota_refresh_after_request(state, plan, &base_url, token.as_str());
    filter_uploaded_asset_ids(&mut summary, &uploads);

    let mut downloaded = resolve_and_download_images(
        state,
        plan,
        &base_url,
        &fp,
        token.as_str(),
        &mut summary,
        &uploads,
    )
    .await?;
    if downloaded.is_empty() && summary.failure.is_none() {
        for _ in 0..24 {
            if let Some(conversation_id) = summary.conversation_id.as_deref() {
                let mut poll = web_poll_conversation(
                    plan,
                    &base_url,
                    &fp,
                    token.as_str(),
                    conversation_id,
                    &uploads,
                )
                .await?;
                merge_web_summary(&mut summary, &mut poll);
                filter_uploaded_asset_ids(&mut summary, &uploads);
                downloaded = resolve_and_download_images(
                    state,
                    plan,
                    &base_url,
                    &fp,
                    token.as_str(),
                    &mut summary,
                    &uploads,
                )
                .await?;
                if !downloaded.is_empty() || summary.failure.is_some() {
                    break;
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    }

    let body = if let Some(failure) = summary.failure.as_ref().filter(|_| downloaded.is_empty()) {
        build_failed_sse(&request, failure)
    } else if let Some(image) = downloaded.into_iter().next() {
        build_success_sse(&request, &image, report_context)
    } else {
        build_failed_sse(
            &request,
            &json!({
                "type": "response.failed",
                "response": {
                    "status": "failed",
                    "error": {
                        "code": "chatgpt_web_no_image",
                        "message": summary.last_text.unwrap_or_else(|| "ChatGPT-Web image proxy returned no image".to_string())
                    }
                }
            }),
        )
    };

    bytes_execution_result(
        plan,
        200,
        BTreeMap::from([
            ("cache-control".to_string(), "no-cache".to_string()),
            ("content-type".to_string(), "text/event-stream".to_string()),
        ]),
        body.into_bytes(),
        started_at,
    )
}

#[derive(Debug, Clone)]
struct ChatGptWebImageRequest {
    operation: String,
    model: String,
    web_model: String,
    prompt: String,
    size: String,
    ratio: String,
    output_format: String,
    quality: Option<String>,
    partial_images: u64,
    images: Vec<String>,
}

impl ChatGptWebImageRequest {
    fn from_body(body: &Value) -> Result<Self, ExecutionRuntimeTransportError> {
        let model =
            bounded_chatgpt_web_text_field(body, "model", CHATGPT_WEB_IMAGE_MAX_MODEL_BYTES)?
                .unwrap_or_else(|| "gpt-image-2".to_string());
        let web_model =
            bounded_chatgpt_web_text_field(body, "web_model", CHATGPT_WEB_IMAGE_MAX_MODEL_BYTES)?
                .unwrap_or_else(|| "gpt-5-5-thinking".to_string());
        let prompt =
            bounded_chatgpt_web_text_field(body, "prompt", CHATGPT_WEB_IMAGE_MAX_PROMPT_BYTES)?
                .unwrap_or_else(|| "Generate a high quality image.".to_string());
        let size =
            bounded_chatgpt_web_text_field(body, "size", CHATGPT_WEB_IMAGE_MAX_OPTION_BYTES)?
                .unwrap_or_else(|| "1024x1024".to_string());
        let ratio =
            bounded_chatgpt_web_text_field(body, "ratio", CHATGPT_WEB_IMAGE_MAX_OPTION_BYTES)?
                .unwrap_or_else(|| "1:1".to_string());
        let output_format = bounded_chatgpt_web_text_field(
            body,
            "output_format",
            CHATGPT_WEB_IMAGE_MAX_OPTION_BYTES,
        )?
        .unwrap_or_else(|| "png".to_string());
        let quality =
            bounded_chatgpt_web_text_field(body, "quality", CHATGPT_WEB_IMAGE_MAX_OPTION_BYTES)?;

        let mut images = Vec::new();
        if let Some(values) = body.get("images").and_then(Value::as_array) {
            if values.len() > CHATGPT_WEB_IMAGE_MAX_INPUT_IMAGES {
                return Err(chatgpt_web_image_request_field_too_large("images"));
            }
            for value in values {
                let Some(value) = value.as_str() else {
                    continue;
                };
                let value = value.trim();
                if value.is_empty() {
                    continue;
                }
                let max_bytes = if value
                    .get(..5)
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case("data:"))
                {
                    maximum_base64_len_for_decoded_limit(
                        chatgpt_web_image_raw_payload_limit_bytes(),
                    )
                    .saturating_add(128)
                } else {
                    CHATGPT_WEB_IMAGE_MAX_EXTERNAL_URL_BYTES
                };
                if value.len() > max_bytes {
                    return Err(chatgpt_web_image_request_field_too_large("image reference"));
                }
                images.push(value.to_string());
            }
        }
        let partial_images = json_u64(body.get("partial_images")).unwrap_or(0);
        if partial_images > 3 {
            return Err(ExecutionRuntimeTransportError::UpstreamRequest(
                "ChatGPT-Web image partial_images must be between 0 and 3".to_string(),
            ));
        }
        Ok(Self {
            operation: chatgpt_web_image_operation(body.get("operation")),
            model,
            web_model,
            prompt,
            size,
            ratio,
            output_format,
            quality,
            partial_images,
            images,
        })
    }
}

fn bounded_chatgpt_web_text_field(
    body: &Value,
    key: &str,
    max_bytes: usize,
) -> Result<Option<String>, ExecutionRuntimeTransportError> {
    let Some(value) = body.get(key).and_then(Value::as_str) else {
        return Ok(None);
    };
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value.len() > max_bytes {
        return Err(chatgpt_web_image_request_field_too_large(key));
    }
    Ok(Some(value.to_string()))
}

fn chatgpt_web_image_request_field_too_large(field: &str) -> ExecutionRuntimeTransportError {
    ExecutionRuntimeTransportError::UpstreamRequest(format!(
        "ChatGPT-Web image request {field} exceeds the supported size"
    ))
}

impl WebFingerprint {
    fn new() -> Self {
        Self {
            user_agent: CHATGPT_WEB_USER_AGENT,
            device_id: Uuid::new_v4().to_string(),
            session_id: Uuid::new_v4().to_string(),
        }
    }
}

async fn web_bootstrap(
    plan: &ExecutionPlan,
    base_url: &str,
    fp: &WebFingerprint,
) -> Result<(), ExecutionRuntimeTransportError> {
    let headers = {
        let mut headers = web_base_headers(fp, "", "");
        headers.insert(
            "accept".to_string(),
            "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8"
                .to_string(),
        );
        headers
    };
    let result =
        execute_subrequest(plan, "GET", format!("{base_url}/"), headers, None, false).await?;
    ensure_success(&result, "ChatGPT-Web bootstrap")
}

async fn web_requirements(
    plan: &ExecutionPlan,
    base_url: &str,
    fp: &WebFingerprint,
    token: &str,
) -> Result<WebRequirement, ExecutionRuntimeTransportError> {
    let path = "/backend-api/sentinel/chat-requirements";
    let mut headers = web_base_headers(fp, token, path);
    headers.insert("content-type".to_string(), "application/json".to_string());
    let body = json!({ "p": build_legacy_requirements_token(fp.user_agent) });
    let result = execute_subrequest(
        plan,
        "POST",
        format!("{base_url}{path}"),
        headers,
        Some(RequestBody::from_json(body)),
        false,
    )
    .await?;
    ensure_success(&result, "ChatGPT-Web requirements")?;
    let payload = execution_result_json(&result)?;
    if payload
        .get("arkose")
        .and_then(|value| value.get("required"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Err(ExecutionRuntimeTransportError::UpstreamRequest(
            "ChatGPT-Web image proxy requires Arkose".to_string(),
        ));
    }
    let token = payload
        .get("token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ExecutionRuntimeTransportError::UpstreamRequest(
                "ChatGPT-Web requirements response missing token".to_string(),
            )
        })?;
    let proof_token = payload
        .get("proofofwork")
        .filter(|value| {
            value
                .get("required")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .and_then(|value| {
            let seed = value.get("seed").and_then(Value::as_str)?;
            let difficulty = value.get("difficulty").and_then(Value::as_str)?;
            Some(build_proof_token(seed, difficulty, fp.user_agent))
        });
    Ok(WebRequirement {
        token: token.to_string(),
        proof_token,
        so_token: payload
            .get("so_token")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
    })
}

async fn web_prepare_conversation(
    plan: &ExecutionPlan,
    base_url: &str,
    fp: &WebFingerprint,
    token: &str,
    requirements: &WebRequirement,
    model_slug: &str,
) -> Result<String, ExecutionRuntimeTransportError> {
    let path = "/backend-api/f/conversation/prepare";
    let headers = web_image_headers(fp, token, path, requirements, None, "*/*");
    let body = json!({
        "action": "next",
        "fork_from_shared_post": false,
        "parent_message_id": "client-created-root",
        "model": model_slug,
        "client_prepare_state": "none",
        "timezone_offset_min": -480,
        "timezone": "Asia/Shanghai",
        "conversation_mode": {"kind": "primary_assistant"},
        "system_hints": ["picture_v2"],
        "attachment_mime_types": ["image/png"],
        "supports_buffering": true,
        "supported_encodings": ["v1"],
        "client_contextual_info": {"app_name": "chatgpt.com"},
        "thinking_effort": "standard"
    });
    let result = execute_subrequest(
        plan,
        "POST",
        format!("{base_url}{path}"),
        headers,
        Some(RequestBody::from_json(body)),
        false,
    )
    .await?;
    ensure_success(&result, "ChatGPT-Web conversation prepare")?;
    execution_result_json(&result)?
        .get("conduit_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            ExecutionRuntimeTransportError::UpstreamRequest(
                "ChatGPT-Web prepare response missing conduit token".to_string(),
            )
        })
}

async fn web_start_conversation(
    plan: &ExecutionPlan,
    base_url: &str,
    fp: &WebFingerprint,
    token: &str,
    requirements: &WebRequirement,
    conduit: &str,
    request: &ChatGptWebImageRequest,
    uploads: &[WebUploadMeta],
) -> Result<WebImageSseSummary, ExecutionRuntimeTransportError> {
    let path = "/backend-api/f/conversation";
    let headers = web_image_headers(
        fp,
        token,
        path,
        requirements,
        Some(conduit),
        "text/event-stream",
    );
    let (content, metadata) = web_image_message_content(request.prompt.as_str(), uploads);
    let body = json!({
        "action": "next",
        "fork_from_shared_post": false,
        "parent_message_id": "client-created-root",
        "model": request.web_model,
        "client_prepare_state": "success",
        "timezone_offset_min": -480,
        "timezone": "Asia/Shanghai",
        "conversation_mode": {"kind": "primary_assistant"},
        "enable_message_followups": true,
        "system_hints": [],
        "supports_buffering": true,
        "supported_encodings": ["v1"],
        "client_contextual_info": {
            "is_dark_mode": false,
            "time_since_loaded": 51,
            "page_height": 1111,
            "page_width": 1731,
            "pixel_ratio": 1.5,
            "screen_height": 1440,
            "screen_width": 2560,
            "app_name": "chatgpt.com"
        },
        "paragen_cot_summary_display_override": "allow",
        "force_parallel_switch": "auto",
        "thinking_effort": "standard",
        "messages": [{
            "id": Uuid::new_v4().to_string(),
            "author": {"role": "user"},
            "create_time": current_unix_secs(),
            "content": content,
            "metadata": metadata
        }]
    });
    let result = execute_subrequest(
        plan,
        "POST",
        format!("{base_url}{path}"),
        headers,
        Some(RequestBody::from_json(body)),
        true,
    )
    .await?;
    ensure_success(&result, "ChatGPT-Web conversation")?;
    Ok(parse_web_image_sse(&execution_result_bytes(&result)?))
}

async fn web_poll_conversation(
    plan: &ExecutionPlan,
    base_url: &str,
    fp: &WebFingerprint,
    token: &str,
    conversation_id: &str,
    uploads: &[WebUploadMeta],
) -> Result<WebImageSseSummary, ExecutionRuntimeTransportError> {
    let conversation_id = validated_web_opaque_id(conversation_id, "conversation ID")?;
    let path = format!("/backend-api/conversation/{conversation_id}");
    let mut headers = web_base_headers(fp, token, path.as_str());
    headers.insert("accept".to_string(), "application/json".to_string());
    let result = execute_subrequest(
        plan,
        "GET",
        format!("{base_url}{path}"),
        headers,
        None,
        false,
    )
    .await?;
    ensure_success(&result, "ChatGPT-Web conversation poll")?;
    let mut summary = WebImageSseSummary::default();
    extract_web_image_values(&execution_result_json(&result)?, &mut summary);
    filter_uploaded_asset_ids(&mut summary, uploads);
    Ok(summary)
}

async fn resolve_and_download_images(
    state: &AppState,
    plan: &ExecutionPlan,
    base_url: &str,
    fp: &WebFingerprint,
    token: &str,
    summary: &mut WebImageSseSummary,
    uploads: &[WebUploadMeta],
) -> Result<Vec<DownloadedImage>, ExecutionRuntimeTransportError> {
    let mut urls = Vec::new();
    add_unique_values(&mut urls, summary.direct_urls.iter().cloned());
    let resolved = web_resolve_image_urls(plan, base_url, fp, token, summary, uploads).await?;
    add_unique_values(&mut urls, resolved);
    let mut downloaded = Vec::new();
    for url in urls {
        match web_download_image(
            state,
            plan,
            base_url,
            fp,
            token,
            url.as_str(),
            WebImageDownloadTrust::ProviderOutput,
        )
        .await
        {
            Ok(image) => {
                downloaded.push(image);
                break;
            }
            Err(err) => {
                debug!(
                    event_name = "chatgpt_web_image_download_failed",
                    log_type = "debug",
                    request_id = %plan.request_id,
                    candidate_id = ?plan.candidate_id,
                    error = %safe_transport_error_message(&err),
                    "gateway failed to download one ChatGPT-Web image URL"
                );
            }
        }
    }
    Ok(downloaded)
}

async fn web_resolve_image_urls(
    plan: &ExecutionPlan,
    base_url: &str,
    fp: &WebFingerprint,
    token: &str,
    summary: &WebImageSseSummary,
    uploads: &[WebUploadMeta],
) -> Result<Vec<String>, ExecutionRuntimeTransportError> {
    let mut urls = Vec::new();
    let uploaded_ids = uploaded_file_ids(uploads);
    let conversation_id = summary
        .conversation_id
        .as_deref()
        .map(|value| validated_web_opaque_id(value, "conversation ID"))
        .transpose()?;
    for raw_file_id in &summary.file_ids {
        let file_id = validated_web_file_id(raw_file_id)?;
        if uploaded_ids.contains(file_id) || file_id == "file_upload" {
            continue;
        }
        let mut path = format!("/backend-api/files/download/{file_id}");
        if let Some(conversation_id) = conversation_id {
            path.push_str("?conversation_id=");
            path.push_str(conversation_id);
            path.push_str("&inline=false");
        }
        if let Some(url) = web_download_url(plan, base_url, fp, token, path.as_str()).await? {
            add_unique_values(&mut urls, [url]);
        }
    }
    if let Some(conversation_id) = conversation_id {
        for raw_sediment_id in &summary.sediment_ids {
            let sediment_id = validated_web_opaque_id(raw_sediment_id, "sediment ID")?;
            if uploaded_ids.contains(sediment_id) {
                continue;
            }
            let path = format!(
                "/backend-api/conversation/{conversation_id}/attachment/{sediment_id}/download"
            );
            if let Some(url) = web_download_url(plan, base_url, fp, token, path.as_str()).await? {
                add_unique_values(&mut urls, [url]);
            }
        }
    }
    Ok(urls)
}

async fn web_download_url(
    plan: &ExecutionPlan,
    base_url: &str,
    fp: &WebFingerprint,
    token: &str,
    path: &str,
) -> Result<Option<String>, ExecutionRuntimeTransportError> {
    let mut headers = web_base_headers(fp, token, path);
    headers.insert("accept".to_string(), "application/json".to_string());
    let result = execute_subrequest(
        plan,
        "GET",
        format!("{base_url}{path}"),
        headers,
        None,
        false,
    )
    .await?;
    if !(200..300).contains(&result.status_code) {
        return Ok(None);
    }
    let body = execution_result_json(&result)?;
    Ok(body
        .get("download_url")
        .or_else(|| body.get("url"))
        .and_then(Value::as_str)
        .map(str::trim)
        .and_then(|value| {
            if value.is_empty() || value.len() > CHATGPT_WEB_IMAGE_MAX_EXTERNAL_URL_BYTES {
                return None;
            }
            let base = url::Url::parse(base_url).ok()?;
            let absolute = url::Url::parse(value).is_ok();
            let url = if absolute {
                url::Url::parse(value).ok()?
            } else {
                base.join(value).ok()?
            };
            validate_web_image_http_url(&url).ok()?;
            // Relative download paths are expected from the authenticated
            // ChatGPT API.  Do not let a provider response turn one into an
            // arbitrary cross-origin target through URL joining.
            if !absolute && !web_download_url_is_same_origin(&base, &url) {
                return None;
            }
            let serialized = url.to_string();
            (serialized.len() <= CHATGPT_WEB_IMAGE_MAX_EXTERNAL_URL_BYTES).then_some(serialized)
        }))
}

async fn web_download_image(
    _state: &AppState,
    plan: &ExecutionPlan,
    base_url: &str,
    fp: &WebFingerprint,
    token: &str,
    raw_url: &str,
    trust: WebImageDownloadTrust,
) -> Result<DownloadedImage, ExecutionRuntimeTransportError> {
    if let Some(data) = parse_data_url(raw_url) {
        return Ok(data);
    }
    let payload = match trust {
        WebImageDownloadTrust::UntrustedInput => {
            let url = parse_absolute_web_image_url(raw_url)?;
            download_public_web_image(url, plan.timeouts.as_ref(), false).await?
        }
        WebImageDownloadTrust::ProviderOutput => {
            download_provider_web_image(plan, base_url, fp, token, raw_url).await?
        }
    };
    let data = payload.data;
    let mime = validate_web_image_payload(&data, payload.content_type.as_deref())?.to_string();
    let (width, height) = image_dimensions(&data);
    Ok(DownloadedImage {
        b64_json: base64::engine::general_purpose::STANDARD.encode(data),
        mime,
        width,
        height,
    })
}

fn parse_absolute_web_image_url(raw_url: &str) -> Result<url::Url, ExecutionRuntimeTransportError> {
    let url = url::Url::parse(raw_url.trim()).map_err(|err| {
        ExecutionRuntimeTransportError::UpstreamRequest(format!(
            "ChatGPT-Web image URL is invalid: {err}"
        ))
    })?;
    validate_web_image_http_url(&url)?;
    Ok(url)
}

fn validate_web_image_http_url(url: &url::Url) -> Result<(), ExecutionRuntimeTransportError> {
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(ExecutionRuntimeTransportError::UpstreamRequest(
            "ChatGPT-Web image URL must be an absolute http or https URL".to_string(),
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(ExecutionRuntimeTransportError::UpstreamRequest(
            "ChatGPT-Web image URL must not contain credentials".to_string(),
        ));
    }
    Ok(())
}

/// Return the canonical MIME type for a supported, non-active image payload.
///
/// Content-Type is metadata supplied by an untrusted upstream and must not be
/// used as the sole type check: an HTML/SVG response can be labelled as
/// `image/png`.  Require a real PNG/JPEG/WebP signature and, when a concrete
/// content type is supplied, require it to agree with the signature.
fn validate_web_image_payload(
    data: &[u8],
    content_type: Option<&str>,
) -> Result<&'static str, ExecutionRuntimeTransportError> {
    if data.is_empty() {
        return Err(ExecutionRuntimeTransportError::UpstreamRequest(
            "ChatGPT-Web image download returned empty body".to_string(),
        ));
    }
    let detected = detected_web_image_mime(data).ok_or_else(|| {
        ExecutionRuntimeTransportError::UpstreamRequest(
            "ChatGPT-Web image download returned an unsupported image payload".to_string(),
        )
    })?;
    let declared = declared_web_image_mime(content_type)?;
    if let Some(declared) = declared {
        if declared != detected {
            return Err(ExecutionRuntimeTransportError::UpstreamRequest(
                "ChatGPT-Web image content type does not match its payload".to_string(),
            ));
        }
    }
    Ok(detected)
}

/// Parse a response Content-Type into one of the formats we can safely pass
/// through the OpenAI image response surface.  Generic octet-stream is
/// allowed only because the payload signature is checked independently.
fn declared_web_image_mime(
    content_type: Option<&str>,
) -> Result<Option<&'static str>, ExecutionRuntimeTransportError> {
    let Some(content_type) = content_type else {
        return Ok(None);
    };
    let token = content_type
        .split(';')
        .next()
        .map(str::trim)
        .unwrap_or_default();
    if token.is_empty()
        || token
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control() || byte == b',')
    {
        return Err(ExecutionRuntimeTransportError::UpstreamRequest(
            "ChatGPT-Web image response has an invalid content type".to_string(),
        ));
    }
    if token.eq_ignore_ascii_case("application/octet-stream")
        || token.eq_ignore_ascii_case("binary/octet-stream")
    {
        return Ok(None);
    }
    let mime =
        if token.eq_ignore_ascii_case("image/png") || token.eq_ignore_ascii_case("image/x-png") {
            Some("image/png")
        } else if token.eq_ignore_ascii_case("image/jpeg")
            || token.eq_ignore_ascii_case("image/jpg")
            || token.eq_ignore_ascii_case("image/pjpeg")
        {
            Some("image/jpeg")
        } else if token.eq_ignore_ascii_case("image/webp") {
            Some("image/webp")
        } else {
            // This intentionally rejects image/svg+xml, image/avif, generic
            // image/*, text/html, and all other active/unsupported types.
            None
        };
    mime.ok_or_else(|| {
        ExecutionRuntimeTransportError::UpstreamRequest(
            "ChatGPT-Web image response has an unsupported content type".to_string(),
        )
    })
    .map(Some)
}

fn detected_web_image_mime(data: &[u8]) -> Option<&'static str> {
    // Require the PNG signature and an IHDR chunk before treating the body as
    // PNG.  This also gives image_dimensions a safe minimum length.
    if data.len() >= 24 && data.starts_with(b"\x89PNG\r\n\x1a\n") && &data[12..16] == b"IHDR" {
        return Some("image/png");
    }
    // JPEG's SOI marker must be followed by a marker prefix.  This rejects a
    // bare/truncated `ff d8` body while leaving full structural validation to
    // the image decoder downstream.
    if data.len() >= 3 && data.starts_with(&[0xff, 0xd8, 0xff]) {
        return Some("image/jpeg");
    }
    // WebP is a RIFF container with a WEBP form type.
    if data.len() >= 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
}

async fn download_provider_web_image(
    plan: &ExecutionPlan,
    base_url: &str,
    fp: &WebFingerprint,
    token: &str,
    raw_url: &str,
) -> Result<WebImageHttpPayload, ExecutionRuntimeTransportError> {
    let base = parse_absolute_web_image_url(base_url)?;
    let mut current = base.join(raw_url.trim()).map_err(|err| {
        ExecutionRuntimeTransportError::UpstreamRequest(format!(
            "ChatGPT-Web image URL is invalid: {err}"
        ))
    })?;
    validate_web_image_http_url(&current)?;
    let mut redirects = 0usize;

    loop {
        if !web_download_url_is_same_origin(&base, &current) {
            // Provider-generated storage URLs may be resolved to RFC 2544
            // synthetic addresses by a local DNS interception tool.  The
            // public downloader still decides whether the exact storage
            // origin is eligible; this flag is never enabled for untrusted
            // request input.
            return download_public_web_image(current, plan.timeouts.as_ref(), true).await;
        }
        let path = match current.query() {
            Some(query) => format!("{}?{query}", current.path()),
            None => current.path().to_string(),
        };
        let mut headers = BTreeMap::new();
        if is_authenticated_web_download_url(&base, &current) {
            headers.extend(web_base_headers(fp, token, path.as_str()));
        }
        headers.insert(
            "accept".to_string(),
            "image/avif,image/webp,image/apng,image/svg+xml,image/*,*/*;q=0.8".to_string(),
        );
        let result = execute_subrequest(
            plan,
            "GET",
            current.as_str().to_string(),
            headers,
            None,
            false,
        )
        .await?;

        if (300..400).contains(&result.status_code) {
            if redirects >= CHATGPT_WEB_IMAGE_DOWNLOAD_MAX_REDIRECTS {
                return Err(ExecutionRuntimeTransportError::UpstreamRequest(
                    "ChatGPT-Web image download exceeded redirect limit".to_string(),
                ));
            }
            let location = result.headers.get("location").ok_or_else(|| {
                ExecutionRuntimeTransportError::UpstreamRequest(
                    "ChatGPT-Web image redirect is missing Location header".to_string(),
                )
            })?;
            current = current.join(location).map_err(|err| {
                ExecutionRuntimeTransportError::UpstreamRequest(format!(
                    "ChatGPT-Web image redirect URL is invalid: {err}"
                ))
            })?;
            validate_web_image_http_url(&current)?;
            redirects += 1;
            continue;
        }

        ensure_success(&result, "ChatGPT-Web image download")?;
        return Ok(WebImageHttpPayload {
            data: execution_result_bytes_with_limit(
                &result,
                chatgpt_web_image_raw_payload_limit_bytes(),
            )?,
            content_type: result.headers.get("content-type").cloned(),
        });
    }
}

async fn download_public_web_image(
    mut current: url::Url,
    timeouts: Option<&ExecutionTimeouts>,
    allow_benchmarking_fake_ip: bool,
) -> Result<WebImageHttpPayload, ExecutionRuntimeTransportError> {
    let mut redirects = 0usize;
    let total_timeout = bounded_chatgpt_web_image_timeout(
        timeouts.and_then(|value| value.total_ms),
        CHATGPT_WEB_IMAGE_PUBLIC_TOTAL_TIMEOUT_MS,
    );
    let deadline = Instant::now() + total_timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(ExecutionRuntimeTransportError::UpstreamRequest(
                "ChatGPT-Web public image download timed out".to_string(),
            ));
        }
        validate_web_image_http_url(&current)?;
        let connect_timeout = bounded_chatgpt_web_image_timeout(
            timeouts.and_then(|value| value.connect_ms),
            CHATGPT_WEB_IMAGE_PUBLIC_CONNECT_TIMEOUT_MS,
        );
        let (host, resolved) = resolve_public_web_image_addrs(
            &current,
            connect_timeout.min(remaining),
            allow_benchmarking_fake_ip,
        )
        .await?;
        let mut builder = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none());
        builder = builder
            .connect_timeout(
                bounded_chatgpt_web_image_timeout(
                    timeouts.and_then(|value| value.connect_ms),
                    CHATGPT_WEB_IMAGE_PUBLIC_CONNECT_TIMEOUT_MS,
                )
                .min(remaining),
            )
            .read_timeout(
                bounded_chatgpt_web_image_timeout(
                    timeouts.and_then(|value| value.read_ms),
                    CHATGPT_WEB_IMAGE_PUBLIC_READ_TIMEOUT_MS,
                )
                .min(remaining),
            )
            .timeout(remaining);
        if host.parse::<IpAddr>().is_err() {
            builder = builder.resolve_to_addrs(host.as_str(), &resolved);
        }
        let client = builder
            .build()
            .map_err(ExecutionRuntimeTransportError::ClientBuild)?;
        let response = client
            .get(current.clone())
            .header(
                reqwest::header::ACCEPT,
                "image/avif,image/webp,image/apng,image/svg+xml,image/*,*/*;q=0.8",
            )
            .send()
            .await
            .map_err(|err| {
                ExecutionRuntimeTransportError::UpstreamRequest(format!(
                    "ChatGPT-Web public image download failed: {}",
                    format_upstream_request_error(&err)
                ))
            })?;

        if response.status().is_redirection() {
            if redirects >= CHATGPT_WEB_IMAGE_DOWNLOAD_MAX_REDIRECTS {
                return Err(ExecutionRuntimeTransportError::UpstreamRequest(
                    "ChatGPT-Web image download exceeded redirect limit".to_string(),
                ));
            }
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| {
                    ExecutionRuntimeTransportError::UpstreamRequest(
                        "ChatGPT-Web image redirect is missing Location header".to_string(),
                    )
                })?;
            current = current.join(location).map_err(|err| {
                ExecutionRuntimeTransportError::UpstreamRequest(format!(
                    "ChatGPT-Web image redirect URL is invalid: {err}"
                ))
            })?;
            redirects += 1;
            continue;
        }
        if !response.status().is_success() {
            return Err(ExecutionRuntimeTransportError::UpstreamRequest(format!(
                "ChatGPT-Web image download returned {}",
                response.status().as_u16()
            )));
        }
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned);
        let data = aether_http::read_response_bytes_with_limit(
            response,
            chatgpt_web_image_raw_payload_limit_bytes(),
        )
        .await
        .map_err(|err| {
            ExecutionRuntimeTransportError::UpstreamRequest(format!(
                "ChatGPT-Web public image body read failed: {err}"
            ))
        })?;
        return Ok(WebImageHttpPayload { data, content_type });
    }
}

fn bounded_chatgpt_web_image_timeout(configured_ms: Option<u64>, default_ms: u64) -> Duration {
    Duration::from_millis(configured_ms.unwrap_or(default_ms).clamp(1, 1_200_000))
}

async fn resolve_public_web_image_addrs(
    url: &url::Url,
    lookup_timeout: Duration,
    allow_benchmarking_fake_ip: bool,
) -> Result<(String, Vec<SocketAddr>), ExecutionRuntimeTransportError> {
    let host = url.host_str().ok_or_else(|| {
        ExecutionRuntimeTransportError::UpstreamRequest(
            "ChatGPT-Web image URL is missing a host".to_string(),
        )
    })?;
    let port = url.port_or_known_default().ok_or_else(|| {
        ExecutionRuntimeTransportError::UpstreamRequest(
            "ChatGPT-Web image URL is missing a port".to_string(),
        )
    })?;
    let resolved = aether_http::lookup_host_with_limits(host, port, lookup_timeout)
        .await
        .map_err(|error| {
            let message = match error.kind() {
                std::io::ErrorKind::TimedOut => "ChatGPT-Web image URL DNS resolution timed out",
                std::io::ErrorKind::InvalidData => {
                    "ChatGPT-Web image URL DNS resolution returned too many addresses"
                }
                _ => "ChatGPT-Web image URL DNS resolution failed",
            };
            ExecutionRuntimeTransportError::UpstreamRequest(message.to_string())
        })?;
    if resolved.is_empty() {
        return Err(ExecutionRuntimeTransportError::UpstreamRequest(
            "ChatGPT-Web image URL DNS resolution returned no addresses".to_string(),
        ));
    }
    validate_public_web_image_addresses(url, &resolved, allow_benchmarking_fake_ip)?;
    Ok((host.to_string(), resolved))
}

fn validate_public_web_image_addresses(
    url: &url::Url,
    addresses: &[SocketAddr],
    allow_benchmarking_fake_ip: bool,
) -> Result<(), ExecutionRuntimeTransportError> {
    let allows_benchmarking_fake_ip =
        allow_benchmarking_fake_ip && web_image_storage_origin_allows_benchmarking_fake_ip(url);
    if addresses.iter().any(|address| {
        aether_http::is_private_or_reserved_ip(address.ip())
            && !(allows_benchmarking_fake_ip
                && aether_http::is_ipv4_benchmarking_fake_ip(address.ip()))
    }) {
        return Err(ExecutionRuntimeTransportError::UpstreamRequest(
            "ChatGPT-Web image URL resolves to a private or reserved address".to_string(),
        ));
    }
    Ok(())
}

/// Synthetic DNS is accepted only for the storage origins that ChatGPT uses
/// for generated assets and upload blobs.  In particular, a user-supplied URL
/// on an arbitrary host cannot opt into this exception merely by resolving to
/// the RFC 2544 benchmark range.
fn web_image_storage_origin_allows_benchmarking_fake_ip(url: &url::Url) -> bool {
    url.scheme().eq_ignore_ascii_case("https")
        && url.port_or_known_default() == Some(443)
        && url.username().is_empty()
        && url.password().is_none()
        && url
            .host_str()
            .is_some_and(chatgpt_web_upload_host_is_allowed)
}

/// Validate the destination returned by ChatGPT's upload-metadata endpoint.
///
/// The upload URL is provider-controlled data, not a trusted request target.
/// Keep this boundary narrower than the generic execution URL policy: uploads
/// must go to the storage origins used by ChatGPT, over HTTPS, without
/// credentials or fragments.  Azure SAS query parameters are intentionally
/// retained because they carry the upload authorization.
fn validate_chatgpt_web_upload_url(
    raw_url: &str,
) -> Result<url::Url, ExecutionRuntimeTransportError> {
    let raw_url = raw_url.trim();
    if raw_url.is_empty() || raw_url.len() > CHATGPT_WEB_IMAGE_MAX_UPLOAD_URL_BYTES {
        return Err(ExecutionRuntimeTransportError::UpstreamRequest(
            "ChatGPT-Web upload URL is invalid or too large".to_string(),
        ));
    }
    let url = url::Url::parse(raw_url).map_err(|_| {
        ExecutionRuntimeTransportError::UpstreamRequest(
            "ChatGPT-Web upload URL is invalid".to_string(),
        )
    })?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || url.port().is_some_and(|port| port != 443)
    {
        return Err(ExecutionRuntimeTransportError::UpstreamRequest(
            "ChatGPT-Web upload URL must use HTTPS on the default port".to_string(),
        ));
    }
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        return Err(ExecutionRuntimeTransportError::UpstreamRequest(
            "ChatGPT-Web upload URL must not contain credentials or a fragment".to_string(),
        ));
    }
    let host = url.host_str().unwrap_or_default();
    if host.parse::<IpAddr>().is_ok() || !chatgpt_web_upload_host_is_allowed(host) {
        return Err(ExecutionRuntimeTransportError::UpstreamRequest(
            "ChatGPT-Web upload URL host is not an allowed storage origin".to_string(),
        ));
    }
    if url.path().len() > CHATGPT_WEB_IMAGE_MAX_UPLOAD_URL_BYTES
        || url
            .query()
            .is_some_and(|query| query.len() > CHATGPT_WEB_IMAGE_MAX_UPLOAD_URL_BYTES)
    {
        return Err(ExecutionRuntimeTransportError::UpstreamRequest(
            "ChatGPT-Web upload URL is too large".to_string(),
        ));
    }
    Ok(url)
}

fn chatgpt_web_upload_host_is_allowed(host: &str) -> bool {
    if !web_dns_host_is_valid(host) {
        return false;
    }
    if web_host_is_domain_or_subdomain(host, "files.oaiusercontent.com") {
        return true;
    }
    if web_host_is_domain_or_subdomain(host, "oaidalleapiprodscus.blob.core.windows.net") {
        return true;
    }
    web_host_is_strict_subdomain(host, "blob.core.windows.net")
        && !web_host_is_domain_or_subdomain(host, "openaiassets.blob.core.windows.net")
}

/// PUT image bytes to a validated ChatGPT storage URL using a DNS-pinned,
/// proxy-free client.  The generic execution runtime intentionally supports
/// configured proxies and broad public HTTPS targets; that is inappropriate
/// for a provider-supplied upload destination.
async fn upload_chatgpt_web_blob(
    plan: &ExecutionPlan,
    upload_url: &url::Url,
    content_type: &str,
    user_agent: &str,
    base_url: &str,
    body: Vec<u8>,
) -> Result<(), ExecutionRuntimeTransportError> {
    let total_timeout = bounded_chatgpt_web_image_timeout(
        plan.timeouts
            .as_ref()
            .and_then(|timeouts| timeouts.total_ms),
        CHATGPT_WEB_IMAGE_PUBLIC_TOTAL_TIMEOUT_MS,
    );
    let connect_timeout = bounded_chatgpt_web_image_timeout(
        plan.timeouts
            .as_ref()
            .and_then(|timeouts| timeouts.connect_ms),
        CHATGPT_WEB_IMAGE_PUBLIC_CONNECT_TIMEOUT_MS,
    )
    .min(total_timeout);
    let read_timeout = bounded_chatgpt_web_image_timeout(
        plan.timeouts.as_ref().and_then(|timeouts| timeouts.read_ms),
        CHATGPT_WEB_IMAGE_PUBLIC_READ_TIMEOUT_MS,
    )
    .min(total_timeout);
    let (host, resolved) =
        resolve_public_web_image_addrs(upload_url, connect_timeout, true).await?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(connect_timeout)
        .read_timeout(read_timeout)
        .timeout(total_timeout)
        .resolve_to_addrs(host.as_str(), &resolved)
        .build()
        .map_err(|_| {
            ExecutionRuntimeTransportError::UpstreamRequest(
                "ChatGPT-Web upload client initialization failed".to_string(),
            )
        })?;

    let response = client
        .put(upload_url.clone())
        .header(reqwest::header::CONTENT_TYPE, content_type)
        .header("x-ms-blob-type", "BlockBlob")
        .header("x-ms-version", "2020-04-08")
        .header(reqwest::header::ORIGIN, base_url)
        .header(reqwest::header::REFERER, format!("{base_url}/"))
        .header(reqwest::header::USER_AGENT, user_agent)
        .body(body)
        .send()
        .await
        .map_err(|_| {
            ExecutionRuntimeTransportError::UpstreamRequest(
                "ChatGPT-Web upload request failed".to_string(),
            )
        })?;
    let status_code = response.status().as_u16();
    if !(200..300).contains(&status_code) {
        return Err(ExecutionRuntimeTransportError::UpstreamHttpStatus {
            status_code,
            message: chatgpt_web_stage_http_error_message("ChatGPT-Web upload blob", status_code),
        });
    }
    aether_http::read_response_bytes_with_limit(
        response,
        CHATGPT_WEB_IMAGE_UPLOAD_RESPONSE_LIMIT_BYTES,
    )
    .await
    .map_err(|_| {
        ExecutionRuntimeTransportError::UpstreamRequest(
            "ChatGPT-Web upload response body is invalid or too large".to_string(),
        )
    })?;
    Ok(())
}

async fn web_upload_image(
    state: &AppState,
    plan: &ExecutionPlan,
    base_url: &str,
    fp: &WebFingerprint,
    token: &str,
    ref_url: &str,
    file_name: String,
) -> Result<WebUploadMeta, ExecutionRuntimeTransportError> {
    let image = web_download_image(
        state,
        plan,
        base_url,
        fp,
        token,
        ref_url,
        WebImageDownloadTrust::UntrustedInput,
    )
    .await?;
    let bytes = decode_base64_body_with_limit(
        image.b64_json.as_str(),
        chatgpt_web_image_raw_payload_limit_bytes(),
    )?;
    let file_size = bytes.len();
    let path = "/backend-api/files";
    let mut headers = web_base_headers(fp, token, path);
    headers.insert("content-type".to_string(), "application/json".to_string());
    headers.insert("accept".to_string(), "application/json".to_string());
    let body = json!({
        "file_name": file_name,
        "file_size": bytes.len(),
        "use_case": "multimodal",
        "width": image.width.unwrap_or(1024),
        "height": image.height.unwrap_or(1024)
    });
    let result = execute_subrequest(
        plan,
        "POST",
        format!("{base_url}{path}"),
        headers,
        Some(RequestBody::from_json(body)),
        false,
    )
    .await?;
    ensure_success(&result, "ChatGPT-Web upload metadata")?;
    let upload_payload = execution_result_json(&result)?;
    let raw_file_id = upload_payload
        .get("file_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ExecutionRuntimeTransportError::UpstreamRequest(
                "ChatGPT-Web upload response missing file_id".to_string(),
            )
        })?;
    let file_id = validated_web_file_id(raw_file_id)?.to_string();
    let upload_url = upload_payload
        .get("upload_url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ExecutionRuntimeTransportError::UpstreamRequest(
                "ChatGPT-Web upload response missing upload_url".to_string(),
            )
        })?;
    let upload_url = validate_chatgpt_web_upload_url(upload_url)?;
    upload_chatgpt_web_blob(
        plan,
        &upload_url,
        image.mime.as_str(),
        fp.user_agent,
        base_url,
        bytes,
    )
    .await?;

    let uploaded_path = format!("/backend-api/files/{file_id}/uploaded");
    let mut uploaded_headers = web_base_headers(fp, token, uploaded_path.as_str());
    uploaded_headers.insert("content-type".to_string(), "application/json".to_string());
    let uploaded_result = execute_subrequest(
        plan,
        "POST",
        format!("{base_url}{uploaded_path}"),
        uploaded_headers,
        Some(RequestBody::from_json(json!({}))),
        false,
    )
    .await?;
    ensure_success(&uploaded_result, "ChatGPT-Web upload confirm")?;

    let library_file_id = web_process_upload_stream(
        plan,
        base_url,
        fp,
        token,
        file_id.as_str(),
        file_name.as_str(),
    )
    .await?;
    Ok(WebUploadMeta {
        file_id,
        library_file_id,
        file_name,
        file_size,
        mime: image.mime,
        width: image.width,
        height: image.height,
    })
}

async fn web_process_upload_stream(
    plan: &ExecutionPlan,
    base_url: &str,
    fp: &WebFingerprint,
    token: &str,
    file_id: &str,
    file_name: &str,
) -> Result<Option<String>, ExecutionRuntimeTransportError> {
    let path = "/backend-api/files/process_upload_stream";
    let mut headers = web_base_headers(fp, token, path);
    headers.insert("content-type".to_string(), "application/json".to_string());
    headers.insert("accept".to_string(), "text/event-stream".to_string());
    let body = json!({
        "file_id": file_id,
        "use_case": "multimodal",
        "index_for_retrieval": false,
        "file_name": file_name,
        "library_persistence_mode": "opportunistic",
        "metadata": {"store_in_library": true},
        "entry_surface": "chat_composer"
    });
    let result = execute_subrequest(
        plan,
        "POST",
        format!("{base_url}{path}"),
        headers,
        Some(RequestBody::from_json(body)),
        true,
    )
    .await?;
    ensure_success(&result, "ChatGPT-Web process upload")?;
    let text = String::from_utf8_lossy(&execution_result_bytes(&result)?).to_string();
    Ok(text.lines().find_map(|line| {
        serde_json::from_str::<Value>(line.trim())
            .ok()
            .and_then(|value| {
                value
                    .get("extra")
                    .and_then(|extra| extra.get("metadata_object_id"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| web_opaque_id_is_safe(value))
                    .map(ToOwned::to_owned)
            })
    }))
}

async fn execute_subrequest(
    plan: &ExecutionPlan,
    method: &str,
    url: String,
    headers: BTreeMap<String, String>,
    body: Option<RequestBody>,
    stream: bool,
) -> Result<ExecutionResult, ExecutionRuntimeTransportError> {
    let subplan = ExecutionPlan {
        request_id: plan.request_id.clone(),
        candidate_id: plan.candidate_id.clone(),
        provider_name: plan.provider_name.clone(),
        provider_id: plan.provider_id.clone(),
        endpoint_id: plan.endpoint_id.clone(),
        key_id: plan.key_id.clone(),
        method: method.to_string(),
        url,
        headers,
        content_type: None,
        content_encoding: None,
        body: body.unwrap_or(RequestBody {
            json_body: None,
            body_bytes_b64: None,
            body_ref: None,
        }),
        stream,
        client_api_format: plan.client_api_format.clone(),
        provider_api_format: plan.provider_api_format.clone(),
        model_name: plan.model_name.clone(),
        proxy: plan.proxy.clone(),
        transport_profile: chatgpt_web_image_transport_profile(plan),
        timeouts: plan.timeouts.clone(),
    };
    DirectSyncExecutionRuntime::new()
        .execute_sync(&subplan)
        .await
}

async fn apply_chatgpt_web_image_quota_request_delta_after_conversation_start(
    state: &AppState,
    plan: &ExecutionPlan,
) {
    if !state.has_provider_catalog_data_reader() || !state.has_provider_catalog_data_writer() {
        return;
    }
    if plan.key_id.trim().is_empty() || plan.provider_id.trim().is_empty() {
        return;
    }

    match apply_chatgpt_web_image_quota_request_delta(state, plan).await {
        Ok(true) => {
            debug!(
                event_name = "chatgpt_web_image_quota_request_delta_applied",
                log_type = "debug",
                request_id = %plan.request_id,
                candidate_id = ?plan.candidate_id,
                provider_id = %plan.provider_id,
                key_id = %plan.key_id,
                "gateway persisted ChatGPT-Web image quota request delta after conversation start"
            );
        }
        Ok(false) => {
            debug!(
                event_name = "chatgpt_web_image_quota_request_delta_skipped",
                log_type = "debug",
                request_id = %plan.request_id,
                candidate_id = ?plan.candidate_id,
                provider_id = %plan.provider_id,
                key_id = %plan.key_id,
                "gateway skipped ChatGPT-Web image quota request delta after conversation start"
            );
        }
        Err(err) => {
            warn!(
                event_name = "chatgpt_web_image_quota_request_delta_failed",
                log_type = "ops",
                request_id = %plan.request_id,
                candidate_id = ?plan.candidate_id,
                provider_id = %plan.provider_id,
                key_id = %plan.key_id,
                error = %err,
                "gateway failed to persist ChatGPT-Web image quota request delta after conversation start"
            );
        }
    }
}

fn spawn_chatgpt_web_image_quota_refresh_after_request(
    state: &AppState,
    plan: &ExecutionPlan,
    base_url: &str,
    token: &str,
) {
    if !state.has_provider_catalog_data_reader() || !state.has_provider_catalog_data_writer() {
        return;
    }
    let token = token.trim();
    if token.is_empty() || plan.key_id.trim().is_empty() || plan.provider_id.trim().is_empty() {
        return;
    }

    let state = state.clone();
    let plan = plan.clone();
    let base_url = base_url.to_string();
    let token = token.to_string();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(5)).await;
        if let Err(err) =
            refresh_chatgpt_web_image_quota_after_success(&state, &plan, &base_url, &token).await
        {
            warn!(
                event_name = "chatgpt_web_image_quota_refresh_after_success_failed",
                log_type = "ops",
                request_id = %plan.request_id,
                candidate_id = ?plan.candidate_id,
                provider_id = %plan.provider_id,
                key_id = %plan.key_id,
                error = %err,
                "gateway failed to refresh ChatGPT-Web image quota after a generation request"
            );
        }
    });
}

async fn apply_chatgpt_web_image_quota_request_delta(
    state: &AppState,
    plan: &ExecutionPlan,
) -> Result<bool, String> {
    let key_id = plan.key_id.trim();
    let provider_id = plan.provider_id.trim();
    if key_id.is_empty() || provider_id.is_empty() {
        return Ok(false);
    }
    let request_dedup_key = chatgpt_web_image_quota_request_delta_dedup_key(plan);
    for attempt in 0..RUNTIME_METADATA_CAS_MAX_ATTEMPTS {
        let Some(mut latest_key) = state
            .read_provider_catalog_keys_by_ids(&[key_id.to_string()])
            .await
            .map_err(|_| "ChatGPT-Web quota state read failed".to_string())?
            .into_iter()
            .find(|key| key.id == key_id && key.provider_id == provider_id)
        else {
            return Ok(false);
        };
        let expected_namespace_value = latest_key
            .upstream_metadata
            .as_ref()
            .and_then(Value::as_object)
            .and_then(|metadata| metadata.get("chatgpt_web"))
            .cloned();
        let mut metadata = expected_namespace_value
            .as_ref()
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let now_unix_secs = current_unix_secs();
        if !apply_chatgpt_web_image_quota_request_delta_to_metadata(
            &mut metadata,
            latest_key.status_snapshot.as_ref(),
            now_unix_secs,
            request_dedup_key.as_deref(),
        ) {
            return Ok(false);
        }

        let namespace_value =
            admin_provider_metadata_bucket_safe_json("chatgpt_web", Some(&Value::Object(metadata)));
        let updated_upstream_metadata = merge_provider_metadata_object(
            latest_key.upstream_metadata.as_ref(),
            "chatgpt_web",
            namespace_value.clone(),
        );
        latest_key.upstream_metadata = updated_upstream_metadata;
        latest_key.status_snapshot = sync_provider_key_quota_status_snapshot(
            latest_key.status_snapshot.as_ref(),
            "chatgpt_web",
            latest_key.upstream_metadata.as_ref(),
            "image_request_local",
        );
        latest_key.status_snapshot = sync_provider_key_oauth_status_snapshot(
            latest_key.status_snapshot.as_ref(),
            &latest_key,
        );
        latest_key.updated_at_unix_secs = Some(now_unix_secs);

        let persisted = state
            .update_provider_catalog_key_runtime_metadata(
                &ProviderCatalogKeyRuntimeMetadataUpdate {
                    key_id: latest_key.id.clone(),
                    namespace: "chatgpt_web".to_string(),
                    expected_upstream_metadata_value: expected_namespace_value,
                    upstream_metadata_value: namespace_value,
                    status_snapshot_patch: provider_operational_status_patch(
                        latest_key.status_snapshot.as_ref(),
                    ),
                    updated_at_unix_secs: latest_key.updated_at_unix_secs,
                },
            )
            .await
            .map_err(|_| "ChatGPT-Web quota state update failed".to_string())?;
        if persisted {
            return Ok(true);
        }
        if attempt + 1 < RUNTIME_METADATA_CAS_MAX_ATTEMPTS {
            let backoff_us = 50_u64.saturating_mul((attempt + 1) as u64).min(1_000);
            tokio::time::sleep(Duration::from_micros(backoff_us)).await;
        }
    }
    Ok(false)
}

fn apply_chatgpt_web_image_quota_request_delta_to_metadata(
    metadata: &mut Map<String, Value>,
    status_snapshot: Option<&Value>,
    now_unix_secs: u64,
    request_dedup_key: Option<&str>,
) -> bool {
    let request_dedup_key = request_dedup_key
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if let Some(request_dedup_key) = request_dedup_key {
        if metadata
            .get("image_quota_last_local_request_key")
            .and_then(Value::as_str)
            .is_some_and(|value| value == request_dedup_key)
        {
            return false;
        }
    }

    let snapshot_window = chatgpt_web_image_quota_snapshot_window(status_snapshot);
    let metadata_limit =
        chatgpt_web_image_quota_f64(metadata.get("image_quota_total")).filter(|value| *value > 0.0);
    let snapshot_limit = snapshot_window.and_then(|window| {
        chatgpt_web_image_quota_f64(window.get("limit_value")).filter(|value| *value > 0.0)
    });
    let candidate_limit = metadata_limit.or(snapshot_limit);
    let used = chatgpt_web_image_quota_f64(metadata.get("image_quota_used")).or_else(|| {
        snapshot_window.and_then(|window| chatgpt_web_image_quota_f64(window.get("used_value")))
    });
    let remaining = chatgpt_web_image_quota_f64(metadata.get("image_quota_remaining"))
        .or_else(|| {
            snapshot_window
                .and_then(|window| chatgpt_web_image_quota_f64(window.get("remaining_value")))
        })
        .or_else(|| {
            candidate_limit
                .zip(used)
                .map(|(limit, used)| (limit - used).max(0.0))
        });
    let limit = chatgpt_web_image_quota_request_limit_choice(
        metadata,
        status_snapshot,
        metadata_limit,
        snapshot_limit,
        remaining,
    );
    if limit.is_none()
        && chatgpt_web_image_quota_metadata_limit_is_legacy_free_default(
            metadata,
            status_snapshot,
            metadata_limit,
            remaining,
        )
    {
        metadata.remove("image_quota_total");
        metadata.remove("image_quota_limit_source");
    }
    let limit_value = limit
        .as_ref()
        .map(|limit| limit.value)
        .unwrap_or_else(|| remaining.unwrap_or(0.0).max(0.0));

    if limit_value > 0.0 {
        metadata.insert("image_quota_total".to_string(), json!(limit_value));
        if let Some(source) = limit
            .as_ref()
            .and_then(|limit| limit.source.as_deref())
            .filter(|value| !value.is_empty())
        {
            metadata.insert("image_quota_limit_source".to_string(), json!(source));
        }
    }
    match remaining {
        Some(remaining) => {
            let new_remaining = (remaining - 1.0).max(0.0);
            metadata.insert("image_quota_remaining".to_string(), json!(new_remaining));
            if limit_value > 0.0 {
                metadata.insert(
                    "image_quota_used".to_string(),
                    json!((limit_value - new_remaining).max(0.0)),
                );
            } else if let Some(used) = used {
                metadata.insert("image_quota_used".to_string(), json!(used + 1.0));
            } else {
                metadata.insert("image_quota_used".to_string(), json!(1.0));
            }
        }
        None => {
            let new_used = used.unwrap_or(0.0).max(0.0) + 1.0;
            metadata.insert("image_quota_used".to_string(), json!(new_used));
            if limit_value > 0.0 {
                metadata.insert(
                    "image_quota_remaining".to_string(),
                    json!((limit_value - new_used).max(0.0)),
                );
            }
        }
    }
    if !metadata.contains_key("image_quota_reset_at") {
        if let Some(reset_at) =
            snapshot_window.and_then(|window| chatgpt_web_image_quota_u64(window.get("reset_at")))
        {
            metadata.insert("image_quota_reset_at".to_string(), json!(reset_at));
        }
    }
    metadata.insert("updated_at".to_string(), json!(now_unix_secs));
    metadata.insert(
        "image_quota_last_local_request_at".to_string(),
        json!(now_unix_secs),
    );
    if let Some(request_dedup_key) = request_dedup_key {
        metadata.insert(
            "image_quota_last_local_request_key".to_string(),
            json!(request_dedup_key),
        );
    }
    let local_request_count =
        chatgpt_web_image_quota_u64(metadata.get("image_quota_local_request_count")).unwrap_or(0);
    metadata.insert(
        "image_quota_local_request_count".to_string(),
        json!(local_request_count.saturating_add(1)),
    );
    true
}

fn chatgpt_web_image_quota_request_delta_dedup_key(plan: &ExecutionPlan) -> Option<String> {
    let request_id = plan.request_id.trim();
    if request_id.is_empty() {
        return None;
    }
    let candidate_id = plan
        .candidate_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    Some(match candidate_id {
        Some(candidate_id) => format!("{request_id}:{candidate_id}"),
        None => request_id.to_string(),
    })
}

#[derive(Debug, Clone)]
struct ChatGptWebImageQuotaRequestLimit {
    value: f64,
    source: Option<String>,
}

fn chatgpt_web_image_quota_metadata_limit_is_legacy_free_default(
    metadata: &Map<String, Value>,
    status_snapshot: Option<&Value>,
    metadata_limit: Option<f64>,
    remaining: Option<f64>,
) -> bool {
    let Some(limit) = metadata_limit else {
        return false;
    };
    let plan_type = chatgpt_web_image_quota_metadata_str(metadata, "plan_type").or_else(|| {
        chatgpt_web_image_quota_snapshot(status_snapshot)
            .and_then(|quota| chatgpt_web_image_quota_metadata_str(quota, "plan_type"))
    });
    let metadata_limit_source =
        chatgpt_web_image_quota_metadata_str(metadata, "image_quota_limit_source");
    chatgpt_web_image_quota_limit_is_legacy_free_default(
        limit,
        metadata_limit_source,
        plan_type,
        remaining,
    )
}

fn chatgpt_web_image_quota_request_limit_choice(
    metadata: &Map<String, Value>,
    status_snapshot: Option<&Value>,
    metadata_limit: Option<f64>,
    snapshot_limit: Option<f64>,
    remaining: Option<f64>,
) -> Option<ChatGptWebImageQuotaRequestLimit> {
    let plan_type = chatgpt_web_image_quota_metadata_str(metadata, "plan_type").or_else(|| {
        chatgpt_web_image_quota_snapshot(status_snapshot)
            .and_then(|quota| chatgpt_web_image_quota_metadata_str(quota, "plan_type"))
    });
    let metadata_limit_source =
        chatgpt_web_image_quota_metadata_str(metadata, "image_quota_limit_source");

    if let Some(limit) = metadata_limit {
        if !chatgpt_web_image_quota_limit_is_legacy_free_default(
            limit,
            metadata_limit_source,
            plan_type,
            remaining,
        ) {
            let source = metadata_limit_source.map(ToOwned::to_owned).or_else(|| {
                let is_first_remaining = plan_type
                    .is_some_and(|value| value.eq_ignore_ascii_case("free"))
                    && remaining.is_some_and(|remaining| (limit - remaining).abs() <= f64::EPSILON);
                Some(
                    if is_first_remaining {
                        "first_remaining"
                    } else {
                        "stored"
                    }
                    .to_string(),
                )
            });
            return Some(ChatGptWebImageQuotaRequestLimit {
                value: limit,
                source,
            });
        }
    }

    if let Some(limit) = snapshot_limit {
        if !chatgpt_web_image_quota_limit_is_legacy_free_default(limit, None, plan_type, remaining)
        {
            return Some(ChatGptWebImageQuotaRequestLimit {
                value: limit,
                source: Some("status_snapshot".to_string()),
            });
        }
    }

    remaining
        .filter(|remaining| remaining.is_finite() && *remaining > 0.0)
        .map(|remaining| ChatGptWebImageQuotaRequestLimit {
            value: remaining,
            source: Some("first_remaining".to_string()),
        })
}

fn chatgpt_web_image_quota_metadata_str<'a>(
    metadata: &'a Map<String, Value>,
    key: &str,
) -> Option<&'a str> {
    metadata
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn chatgpt_web_image_quota_limit_is_legacy_free_default(
    limit: f64,
    source: Option<&str>,
    plan_type: Option<&str>,
    remaining: Option<f64>,
) -> bool {
    let plan_type_is_free = plan_type
        .map(str::trim)
        .is_some_and(|value| value.eq_ignore_ascii_case("free"));
    if !plan_type_is_free || source.is_some() {
        return false;
    }
    if (limit - 25.0).abs() > f64::EPSILON {
        return false;
    }
    remaining.is_none_or(|remaining| remaining.is_finite() && remaining < limit)
}

async fn refresh_chatgpt_web_image_quota_after_success(
    state: &AppState,
    plan: &ExecutionPlan,
    base_url: &str,
    token: &str,
) -> Result<bool, String> {
    let key_id = plan.key_id.trim();
    let provider_id = plan.provider_id.trim();
    let key_ids = [key_id.to_string()];
    let provider_ids = [provider_id.to_string()];
    let key_available = state
        .read_provider_catalog_keys_by_ids(&key_ids)
        .await
        .map_err(|_| "ChatGPT-Web quota key read failed".to_string())?
        .into_iter()
        .any(|key| key.id == key_id && key.provider_id == provider_id);
    if !key_available {
        return Ok(false);
    }
    let Some(provider) = state
        .read_provider_catalog_providers_by_ids(&provider_ids)
        .await
        .map_err(|_| "ChatGPT-Web quota provider read failed".to_string())?
        .into_iter()
        .find(|provider| provider.id == provider_id)
    else {
        return Ok(false);
    };
    if !provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case("chatgpt_web")
    {
        return Ok(false);
    }

    let authorization = (
        "authorization".to_string(),
        format!("Bearer {}", token.trim()),
    );
    let spec = build_chatgpt_web_pool_quota_request(key_id, base_url, authorization);
    let quota_plan = build_chatgpt_web_image_quota_refresh_plan(plan, spec);
    let result = DirectSyncExecutionRuntime::new()
        .execute_sync(&quota_plan)
        .await
        .map_err(|_| "ChatGPT-Web quota refresh request failed".to_string())?;
    if result.status_code != 200 {
        return Err(format!(
            "ChatGPT-Web quota refresh returned HTTP {}",
            result.status_code
        ));
    }

    let body_json = execution_result_json(&result)
        .map_err(|_| "ChatGPT-Web quota refresh response was invalid".to_string())?;
    let now_unix_secs = current_unix_secs();
    let Some(metadata) = parse_chatgpt_web_conversation_init_response(&body_json, now_unix_secs)
    else {
        return Ok(false);
    };
    let Some(latest_key) = state
        .read_provider_catalog_keys_by_ids(&key_ids)
        .await
        .map_err(|_| "ChatGPT-Web quota key read failed".to_string())?
        .into_iter()
        .find(|key| key.id == key_id && key.provider_id == provider_id)
    else {
        return Ok(false);
    };
    let expected_namespace_value = latest_key
        .upstream_metadata
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|metadata| metadata.get("chatgpt_web"))
        .cloned();
    let mut metadata = metadata.clone();
    normalize_chatgpt_web_image_quota_limit(&mut metadata, latest_key.upstream_metadata.as_ref());
    metadata = admin_provider_metadata_bucket_safe_json("chatgpt_web", Some(&metadata));

    let mut updated_key = latest_key;
    let namespace_value = metadata.clone();
    let updated_upstream_metadata = merge_provider_metadata_object(
        updated_key.upstream_metadata.as_ref(),
        "chatgpt_web",
        metadata,
    );
    updated_key.upstream_metadata = updated_upstream_metadata;
    let (oauth_invalid_at_unix_secs, oauth_invalid_reason) =
        quota_refresh_success_invalid_state(&updated_key);
    updated_key.oauth_invalid_at_unix_secs = oauth_invalid_at_unix_secs;
    updated_key.oauth_invalid_reason = oauth_invalid_reason;
    updated_key.status_snapshot = sync_provider_key_quota_status_snapshot(
        updated_key.status_snapshot.as_ref(),
        "chatgpt_web",
        updated_key.upstream_metadata.as_ref(),
        "image_success",
    );
    updated_key.status_snapshot =
        sync_provider_key_oauth_status_snapshot(updated_key.status_snapshot.as_ref(), &updated_key);
    updated_key.updated_at_unix_secs = Some(now_unix_secs);

    let persisted = state
        .update_provider_catalog_key_runtime_metadata(&ProviderCatalogKeyRuntimeMetadataUpdate {
            key_id: updated_key.id.clone(),
            namespace: "chatgpt_web".to_string(),
            expected_upstream_metadata_value: expected_namespace_value,
            upstream_metadata_value: namespace_value,
            status_snapshot_patch: provider_operational_status_patch(
                updated_key.status_snapshot.as_ref(),
            ),
            updated_at_unix_secs: updated_key.updated_at_unix_secs,
        })
        .await
        .map_err(|_| "ChatGPT-Web quota state update failed".to_string())?;
    if persisted {
        return state
            .update_provider_catalog_key_oauth_runtime_state(
                &updated_key.id,
                updated_key.oauth_invalid_at_unix_secs,
                updated_key.oauth_invalid_reason.as_deref(),
                updated_key.updated_at_unix_secs,
            )
            .await
            .map_err(|_| "ChatGPT-Web OAuth state update failed".to_string());
    }
    // The conversation/init response is an authoritative snapshot.  A
    // conflict means a newer local delta won; do not overwrite it with
    // this stale response.  The next refresh will observe the new value.
    Ok(false)
}

fn build_chatgpt_web_image_quota_refresh_plan(
    plan: &ExecutionPlan,
    spec: ProviderPoolQuotaRequestSpec,
) -> ExecutionPlan {
    let ProviderPoolQuotaRequestSpec {
        request_id,
        provider_name,
        quota_kind: _,
        method,
        url,
        headers,
        content_type,
        json_body,
        client_api_format,
        provider_api_format,
        model_name,
    } = spec;
    let body = json_body
        .map(RequestBody::from_json)
        .unwrap_or(RequestBody {
            json_body: None,
            body_bytes_b64: None,
            body_ref: None,
        });
    ExecutionPlan {
        request_id,
        candidate_id: plan.candidate_id.clone(),
        provider_name: Some(provider_name),
        provider_id: plan.provider_id.clone(),
        endpoint_id: plan.endpoint_id.clone(),
        key_id: plan.key_id.clone(),
        method,
        url,
        headers,
        content_type,
        content_encoding: None,
        body,
        stream: false,
        client_api_format,
        provider_api_format,
        model_name,
        proxy: plan.proxy.clone(),
        transport_profile: chatgpt_web_image_transport_profile(plan),
        timeouts: Some(chatgpt_web_image_quota_refresh_timeouts(
            plan.proxy.as_ref(),
        )),
    }
}

fn chatgpt_web_image_quota_refresh_timeouts(proxy: Option<&ProxySnapshot>) -> ExecutionTimeouts {
    let timeout_ms = if proxy.is_some() {
        CHATGPT_WEB_QUOTA_REFRESH_PROXY_TIMEOUT_MS
    } else {
        CHATGPT_WEB_QUOTA_REFRESH_TIMEOUT_MS
    };
    ExecutionTimeouts {
        connect_ms: Some(timeout_ms),
        read_ms: Some(timeout_ms),
        write_ms: Some(timeout_ms),
        pool_ms: Some(timeout_ms),
        total_ms: Some(timeout_ms),
        ..ExecutionTimeouts::default()
    }
}

fn merge_provider_metadata_object(
    current: Option<&Value>,
    section_key: &str,
    section_value: Value,
) -> Option<Value> {
    let mut merged = current
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    merged.insert(section_key.to_string(), section_value);
    Some(Value::Object(merged))
}

fn provider_operational_status_patch(status_snapshot: Option<&Value>) -> Value {
    let mut patch = Map::new();
    if let Some(snapshot) = status_snapshot.and_then(Value::as_object) {
        for field in ["quota", "oauth"] {
            if let Some(value) = snapshot.get(field) {
                patch.insert(field.to_string(), value.clone());
            }
        }
    }
    Value::Object(patch)
}

fn chatgpt_web_image_quota_snapshot_window(
    status_snapshot: Option<&Value>,
) -> Option<&Map<String, Value>> {
    let quota = chatgpt_web_image_quota_snapshot(status_snapshot)?;
    quota
        .get("windows")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(Value::as_object)
        .find(|window| {
            window
                .get("code")
                .and_then(Value::as_str)
                .is_some_and(|value| value.trim().eq_ignore_ascii_case("image_gen"))
        })
        .or_else(|| {
            quota
                .get("windows")
                .and_then(Value::as_array)?
                .iter()
                .filter_map(Value::as_object)
                .find(|window| {
                    window
                        .get("scope")
                        .and_then(Value::as_str)
                        .is_some_and(|value| value.trim().eq_ignore_ascii_case("account"))
                })
        })
}

fn chatgpt_web_image_quota_snapshot(
    status_snapshot: Option<&Value>,
) -> Option<&Map<String, Value>> {
    let quota = status_snapshot
        .and_then(Value::as_object)
        .and_then(|snapshot| snapshot.get("quota"))
        .and_then(Value::as_object)?;
    if quota
        .get("provider_type")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().eq_ignore_ascii_case("chatgpt_web"))
    {
        return None;
    }
    Some(quota)
}

fn chatgpt_web_image_quota_f64(value: Option<&Value>) -> Option<f64> {
    match value {
        Some(Value::Number(number)) => number.as_f64(),
        Some(Value::String(value)) => value.trim().parse::<f64>().ok(),
        _ => None,
    }
    .filter(|value| value.is_finite())
}

fn chatgpt_web_image_quota_u64(value: Option<&Value>) -> Option<u64> {
    let mut parsed = chatgpt_web_image_quota_f64(value)?;
    if parsed <= 0.0 {
        return None;
    }
    if parsed > 1_000_000_000_000.0 {
        parsed /= 1000.0;
    }
    Some(parsed.floor() as u64)
}

fn chatgpt_web_image_transport_profile(plan: &ExecutionPlan) -> Option<ResolvedTransportProfile> {
    match plan.transport_profile.as_ref() {
        Some(profile)
            if profile
                .backend
                .trim()
                .eq_ignore_ascii_case(TRANSPORT_BACKEND_BROWSER_WREQ) =>
        {
            Some(profile.clone())
        }
        _ => Some(default_chatgpt_web_image_transport_profile()),
    }
}

fn default_chatgpt_web_image_transport_profile() -> ResolvedTransportProfile {
    ResolvedTransportProfile {
        profile_id: CHATGPT_WEB_BROWSER_PROFILE.to_string(),
        backend: TRANSPORT_BACKEND_BROWSER_WREQ.to_string(),
        http_mode: TRANSPORT_HTTP_MODE_AUTO.to_string(),
        pool_scope: TRANSPORT_POOL_SCOPE_KEY.to_string(),
        header_fingerprint: None,
        extra: Some(json!({
            "browser_profile": CHATGPT_WEB_BROWSER_PROFILE,
            "source": "chatgpt_web_image_default",
        })),
    }
}

fn web_base_headers(fp: &WebFingerprint, token: &str, path: &str) -> BTreeMap<String, String> {
    let mut headers = BTreeMap::from([
        ("user-agent".to_string(), fp.user_agent.to_string()),
        (
            "origin".to_string(),
            CHATGPT_WEB_DEFAULT_BASE_URL.to_string(),
        ),
        (
            "referer".to_string(),
            format!("{CHATGPT_WEB_DEFAULT_BASE_URL}/"),
        ),
        (
            "accept-language".to_string(),
            "zh-CN,zh;q=0.9,en;q=0.8,en-US;q=0.7".to_string(),
        ),
        ("cache-control".to_string(), "no-cache".to_string()),
        ("pragma".to_string(), "no-cache".to_string()),
        ("priority".to_string(), "u=1, i".to_string()),
        ("sec-ch-ua".to_string(), CHATGPT_WEB_SEC_CH_UA.to_string()),
        ("sec-ch-ua-arch".to_string(), r#""x86""#.to_string()),
        ("sec-ch-ua-bitness".to_string(), r#""64""#.to_string()),
        ("sec-ch-ua-mobile".to_string(), "?0".to_string()),
        ("sec-ch-ua-model".to_string(), r#""""#.to_string()),
        ("sec-ch-ua-platform".to_string(), r#""Windows""#.to_string()),
        (
            "sec-ch-ua-platform-version".to_string(),
            r#""19.0.0""#.to_string(),
        ),
        ("sec-fetch-dest".to_string(), "empty".to_string()),
        ("sec-fetch-mode".to_string(), "cors".to_string()),
        ("sec-fetch-site".to_string(), "same-origin".to_string()),
        ("oai-device-id".to_string(), fp.device_id.clone()),
        ("oai-session-id".to_string(), fp.session_id.clone()),
        ("oai-language".to_string(), "zh-CN".to_string()),
        (
            "oai-client-version".to_string(),
            CHATGPT_WEB_CLIENT_VERSION.to_string(),
        ),
        (
            "oai-client-build-number".to_string(),
            CHATGPT_WEB_BUILD_NUMBER.to_string(),
        ),
    ]);
    if !path.is_empty() {
        headers.insert("x-openai-target-path".to_string(), path.to_string());
        headers.insert("x-openai-target-route".to_string(), path.to_string());
    }
    if !token.trim().is_empty() {
        headers.insert(
            "authorization".to_string(),
            format!("Bearer {}", token.trim()),
        );
    }
    headers
}

fn web_image_headers(
    fp: &WebFingerprint,
    token: &str,
    path: &str,
    requirements: &WebRequirement,
    conduit: Option<&str>,
    accept: &str,
) -> BTreeMap<String, String> {
    let mut headers = web_base_headers(fp, token, path);
    headers.insert("content-type".to_string(), "application/json".to_string());
    headers.insert("accept".to_string(), accept.to_string());
    headers.insert(
        "openai-sentinel-chat-requirements-token".to_string(),
        requirements.token.clone(),
    );
    if let Some(proof_token) = requirements.proof_token.as_ref() {
        headers.insert(
            "openai-sentinel-proof-token".to_string(),
            proof_token.clone(),
        );
    }
    if let Some(so_token) = requirements.so_token.as_ref() {
        headers.insert("openai-sentinel-so-token".to_string(), so_token.clone());
    }
    if let Some(conduit) = conduit.map(str::trim).filter(|value| !value.is_empty()) {
        headers.insert("x-conduit-token".to_string(), conduit.to_string());
    }
    if accept == "text/event-stream" {
        headers.insert(
            "x-oai-turn-trace-id".to_string(),
            Uuid::new_v4().to_string(),
        );
    }
    headers
}

fn web_image_message_content(prompt: &str, uploads: &[WebUploadMeta]) -> (Value, Value) {
    if uploads.is_empty() {
        return (
            json!({"content_type": "text", "parts": [prompt]}),
            json!({
                "developer_mode_connector_ids": [],
                "selected_github_repos": [],
                "selected_all_github_repos": false,
                "system_hints": ["picture_v2"],
                "serialization_metadata": {"custom_symbol_offsets": []}
            }),
        );
    }

    let mut parts = Vec::new();
    let mut attachments = Vec::new();
    for upload in uploads {
        parts.push(json!({
            "content_type": "image_asset_pointer",
            "asset_pointer": format!("sediment://file_{}", upload.file_id.trim_start_matches("file_")),
            "width": upload.width.unwrap_or(1024),
            "height": upload.height.unwrap_or(1024),
            "size_bytes": upload.file_size
        }));
        let mut attachment = json!({
            "id": upload.file_id,
            "mime_type": upload.mime,
            "name": upload.file_name,
            "size": upload.file_size,
            "width": upload.width.unwrap_or(1024),
            "height": upload.height.unwrap_or(1024),
            "source": "library",
            "is_big_paste": false
        });
        if let Some(library_file_id) = upload.library_file_id.as_ref() {
            attachment["library_file_id"] = Value::String(library_file_id.clone());
        }
        attachments.push(attachment);
    }
    parts.push(Value::String(prompt.to_string()));
    (
        json!({"content_type": "multimodal_text", "parts": parts}),
        json!({
            "developer_mode_connector_ids": [],
            "selected_github_repos": [],
            "selected_all_github_repos": false,
            "system_hints": ["picture_v2"],
            "serialization_metadata": {"custom_symbol_offsets": []},
            "attachments": attachments
        }),
    )
}

fn parse_web_image_sse(bytes: &[u8]) -> WebImageSseSummary {
    let text = String::from_utf8_lossy(bytes);
    let mut summary = WebImageSseSummary::default();
    let mut data_lines = Vec::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            flush_sse_data(&mut data_lines, &mut summary);
            continue;
        }
        if let Some(data) = line.strip_prefix("data:") {
            data_lines.push(data.trim().to_string());
        }
    }
    flush_sse_data(&mut data_lines, &mut summary);
    summary
}

fn flush_sse_data(data_lines: &mut Vec<String>, summary: &mut WebImageSseSummary) {
    if data_lines.is_empty() {
        return;
    }
    let data = data_lines.join("\n");
    data_lines.clear();
    if data.trim().is_empty() || data.trim() == "[DONE]" {
        return;
    }
    if let Ok(value) = serde_json::from_str::<Value>(&data) {
        if matches!(
            value.get("type").and_then(Value::as_str),
            Some("error" | "response.failed")
        ) {
            summary.failure = Some(bounded_web_failure_value(&value));
        }
        if let Some(text) = extract_assistant_text(&value) {
            summary.last_text = Some(text);
        }
        if let Some(item) = value.get("item").filter(|item| {
            item.get("type").and_then(Value::as_str) == Some("image_generation_call")
        }) {
            // Keep the provider's declared output format when constructing a
            // data URL.  The bytes are still verified by `parse_data_url`
            // before download, but labelling every output as PNG would create
            // an avoidable MIME/signature mismatch (and an extra failed
            // download attempt) for JPEG/WebP results.
            if let Some(result) = item
                .get("result")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                let mime = mime_for_web_output_format(
                    item.get("output_format")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                );
                if let Some(url) = bounded_web_image_data_url(mime, result) {
                    summary.add_values(WebImageSummaryCollection::DirectUrl, [url]);
                }
            }
        }
        summary.add_values(
            WebImageSummaryCollection::DirectUrl,
            extract_web_image_payload_urls(&value),
        );
        extract_web_image_values(&value, summary);
    }
}

fn extract_web_image_payload_urls(value: &Value) -> Vec<String> {
    let mut urls = Vec::new();
    match value.get("type").and_then(Value::as_str) {
        Some("response.output_item.done") => {
            if let Some(item) = value.get("item") {
                add_web_output_item_image_url(&mut urls, item);
            }
        }
        Some("response.completed") => {
            if let Some(output) = value
                .get("response")
                .and_then(|response| response.get("output"))
                .or_else(|| value.get("output"))
                .and_then(Value::as_array)
            {
                for item in output {
                    add_web_output_item_image_url(&mut urls, item);
                }
            }
        }
        Some("response.image_generation_call.partial_image") => {
            if let Some(partial_b64) = value
                .get("partial_image_b64")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                let mime = mime_for_web_output_format(
                    value
                        .get("output_format")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                );
                if let Some(url) = bounded_web_image_data_url(mime, partial_b64) {
                    add_unique_values(&mut urls, [url]);
                }
            }
        }
        _ => {
            if value.get("item").is_some() {
                if let Some(item) = value.get("item") {
                    add_web_output_item_image_url(&mut urls, item);
                }
            }
            if let Some(output) = value.get("output").and_then(Value::as_array) {
                for item in output {
                    add_web_output_item_image_url(&mut urls, item);
                }
            }
        }
    }
    urls
}

fn add_web_output_item_image_url(urls: &mut Vec<String>, item: &Value) {
    if item.get("type").and_then(Value::as_str) != Some("image_generation_call") {
        return;
    }
    if let Some(url) = web_output_item_url(item) {
        add_unique_values(urls, [url]);
    }
}

fn web_output_item_url(item: &Value) -> Option<String> {
    if let Some(url) = image_payload_url_from_object(item) {
        return Some(url);
    }
    item.get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find_map(image_payload_url_from_object)
}

fn image_payload_url_from_object(value: &Value) -> Option<String> {
    if let Some(url) = value
        .get("url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if url.len() > CHATGPT_WEB_IMAGE_MAX_EXTERNAL_URL_BYTES {
            return None;
        }
        return Some(url.to_string());
    }
    let b64 = value
        .get("result")
        .or_else(|| value.get("b64_json"))
        .or_else(|| value.get("image_b64"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let mime = mime_for_web_output_format(
        value
            .get("output_format")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    );
    bounded_web_image_data_url(mime, b64)
}

fn bounded_web_image_data_url(mime: &str, b64: &str) -> Option<String> {
    let b64 = b64.trim();
    if b64.is_empty()
        || b64.len()
            > maximum_base64_len_for_decoded_limit(chatgpt_web_image_raw_payload_limit_bytes())
    {
        return None;
    }
    let prefix_len = "data:;base64,".len().saturating_add(mime.len());
    if prefix_len.saturating_add(b64.len()) > chatgpt_web_image_sse_envelope_limit_bytes() {
        return None;
    }
    Some(format!("data:{mime};base64,{b64}"))
}

fn mime_for_web_output_format(format: &str) -> &'static str {
    let format = format.trim();
    if format.eq_ignore_ascii_case("jpeg") || format.eq_ignore_ascii_case("jpg") {
        "image/jpeg"
    } else if format.eq_ignore_ascii_case("webp") {
        "image/webp"
    } else {
        "image/png"
    }
}

fn extract_web_image_values(value: &Value, summary: &mut WebImageSseSummary) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if key == "conversation_id" {
                    if let Some(conversation_id) = value
                        .as_str()
                        .map(str::trim)
                        .filter(|value| web_opaque_id_is_safe(value))
                    {
                        summary
                            .conversation_id
                            .get_or_insert(conversation_id.to_string());
                    }
                }
                extract_web_image_values(value, summary);
            }
        }
        Value::Array(values) => {
            for value in values {
                extract_web_image_values(value, summary);
            }
        }
        Value::String(text) => {
            let text = text.trim();
            if let Some(sediment_id) = text.strip_prefix("sediment://") {
                if web_opaque_id_is_safe(sediment_id) {
                    summary.add_values(
                        WebImageSummaryCollection::SedimentId,
                        [sediment_id.to_string()],
                    );
                }
            } else if is_web_file_id(text) {
                summary.add_values(WebImageSummaryCollection::FileId, [text.to_string()]);
            } else if (text.len() <= CHATGPT_WEB_IMAGE_MAX_EXTERNAL_URL_BYTES
                && is_generated_web_asset_url(text))
                || (text.len() <= chatgpt_web_image_sse_envelope_limit_bytes()
                    && is_data_image_reference(text))
            {
                summary.add_values(WebImageSummaryCollection::DirectUrl, [text.to_string()]);
            }
        }
        _ => {}
    }
}

fn extract_assistant_text(value: &Value) -> Option<String> {
    value
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(|content| content.get("parts"))
        .and_then(Value::as_array)
        .and_then(|parts| parts.iter().filter_map(Value::as_str).next())
        .map(str::trim)
        .filter(|value| {
            !value.is_empty() && value.len() <= CHATGPT_WEB_IMAGE_SUMMARY_MAX_TEXT_BYTES
        })
        .map(ToOwned::to_owned)
}

fn bounded_web_failure_value(value: &Value) -> Value {
    if json_value_fits_serialized_limit(value, CHATGPT_WEB_IMAGE_SUMMARY_MAX_TEXT_BYTES) {
        return value.clone();
    }
    if value.get("type").and_then(Value::as_str) == Some("response.failed") {
        json!({
            "type": "response.failed",
            "response": {
                "status": "failed",
                "error": {
                    "code": "chatgpt_web_image_failed",
                    "message": "ChatGPT-Web image provider returned an oversized failure"
                }
            }
        })
    } else {
        json!({
            "type": "error",
            "error": {
                "code": "chatgpt_web_image_failed",
                "message": "ChatGPT-Web image provider returned an oversized failure"
            }
        })
    }
}

fn merge_web_summary(target: &mut WebImageSseSummary, source: &mut WebImageSseSummary) {
    if target.conversation_id.is_none() {
        target.conversation_id = source
            .conversation_id
            .take()
            .filter(|value| web_opaque_id_is_safe(value));
    }
    target.add_values(
        WebImageSummaryCollection::FileId,
        source
            .file_ids
            .drain(..)
            .filter(|value| is_web_file_id(value)),
    );
    target.add_values(
        WebImageSummaryCollection::SedimentId,
        source
            .sediment_ids
            .drain(..)
            .filter(|value| web_opaque_id_is_safe(value)),
    );
    target.add_values(
        WebImageSummaryCollection::DirectUrl,
        source.direct_urls.drain(..),
    );
    if target.failure.is_none() {
        target.failure = source.failure.take();
    }
    if target.last_text.is_none() {
        target.last_text = source.last_text.take();
    }
}

fn filter_uploaded_asset_ids(summary: &mut WebImageSseSummary, uploads: &[WebUploadMeta]) {
    let uploaded = uploaded_file_ids(uploads);
    summary.file_ids.retain(|id| !uploaded.contains(id));
    summary.sediment_ids.retain(|id| !uploaded.contains(id));
}

fn uploaded_file_ids(uploads: &[WebUploadMeta]) -> BTreeSet<String> {
    uploads
        .iter()
        .flat_map(|upload| {
            [Some(upload.file_id.clone()), upload.library_file_id.clone()]
                .into_iter()
                .flatten()
        })
        .collect()
}

fn add_unique_values(values: &mut Vec<String>, incoming: impl IntoIterator<Item = String>) {
    let budget = chatgpt_web_image_sse_envelope_limit_bytes();
    let mut retained_bytes = saturating_string_bytes(values);
    for value in incoming {
        if value.is_empty()
            || value.len() > budget
            || values.len() >= CHATGPT_WEB_IMAGE_SUMMARY_MAX_DIRECT_URLS
            || value.len() > budget.saturating_sub(retained_bytes)
            || values.iter().any(|existing| existing == &value)
        {
            continue;
        }
        retained_bytes = retained_bytes.saturating_add(value.len());
        values.push(value);
    }
}

fn build_success_sse(
    request: &ChatGptWebImageRequest,
    image: &DownloadedImage,
    report_context: Option<&Value>,
) -> String {
    let response_id = format!("resp_{}", Uuid::new_v4().simple());
    let item_id = format!("ig_{}", Uuid::new_v4().simple());
    let created_at = current_unix_secs() as i64;
    let output_format = output_format_from_mime(&image.mime, request.output_format.as_str());
    let usage = chatgpt_web_image_usage(request, image, report_context);
    let item = json!({
        "id": item_id,
        "type": "image_generation_call",
        "result": image.b64_json,
        "output_format": output_format,
        "width": image.width,
        "height": image.height,
        "revised_prompt": Value::Null
    });
    let created = json!({
        "type": "response.created",
        "response": {
            "id": response_id,
            "object": "response",
            "created_at": created_at,
            "model": request.model,
            "status": "in_progress"
        }
    });
    let done = json!({
        "type": "response.output_item.done",
        "output_index": 0,
        "item": item
    });
    let completed = json!({
        "type": "response.completed",
        "response": {
            "id": response_id,
            "object": "response",
            "created_at": created_at,
            "model": request.model,
            "status": "completed",
            "output": [{
                "type": "image_generation_call",
                "output_format": output_format,
                "width": image.width,
                "height": image.height,
                "revised_prompt": Value::Null
            }],
            "usage": usage.0,
            "tool_usage": usage.1
        }
    });
    format!(
        "event: response.created\ndata: {}\n\nevent: response.output_item.done\ndata: {}\n\nevent: response.completed\ndata: {}\n\ndata: [DONE]\n\n",
        created, done, completed
    )
}

fn chatgpt_web_image_usage(
    request: &ChatGptWebImageRequest,
    image: &DownloadedImage,
    report_context: Option<&Value>,
) -> (Value, Value) {
    let input_tokens = chatgpt_web_image_input_tokens(request, report_context);
    let estimated_output_tokens = chatgpt_web_image_output_tokens(request, image, report_context);
    let usage = json!({
        "input_tokens": input_tokens,
        "output_tokens": estimated_output_tokens,
        "total_tokens": input_tokens.saturating_add(estimated_output_tokens),
    });
    let tool_usage = json!({
        "image_gen": {
            "input_tokens": input_tokens,
            "input_tokens_details": {
                "image_tokens": 0,
                "text_tokens": input_tokens
            },
            "output_tokens": estimated_output_tokens,
            "output_tokens_details": {
                "image_tokens": estimated_output_tokens,
                "text_tokens": 0
            },
            "total_tokens": input_tokens.saturating_add(estimated_output_tokens),
        }
    });
    (usage, tool_usage)
}

fn chatgpt_web_image_input_tokens(
    request: &ChatGptWebImageRequest,
    report_context: Option<&Value>,
) -> u64 {
    let prompt = chatgpt_web_image_prompt_text(request, report_context);
    estimate_text_tokens(prompt.as_str())
}

fn chatgpt_web_image_output_tokens(
    request: &ChatGptWebImageRequest,
    image: &DownloadedImage,
    report_context: Option<&Value>,
) -> u64 {
    let quality = chatgpt_web_image_quality(request, report_context);
    let size = chatgpt_web_image_size(request, image, report_context);
    let partial_images = chatgpt_web_image_partial_images(request, report_context);
    let base_tokens = size
        .map(|(width, height)| gpt_image2_output_tokens(width, height, quality.as_str()))
        .unwrap_or_else(|| gpt_image2_output_tokens(1024, 1024, quality.as_str()));
    base_tokens
        .saturating_add(partial_images.saturating_mul(GPT_IMAGE2_PARTIAL_IMAGE_OUTPUT_TOKENS))
}

fn chatgpt_web_image_quality(
    request: &ChatGptWebImageRequest,
    report_context: Option<&Value>,
) -> String {
    let candidate = [
        chatgpt_web_report_context_image_request_text(report_context, "quality"),
        chatgpt_web_report_context_original_request_text(report_context, "quality"),
        request.quality.clone(),
    ]
    .into_iter()
    .flatten()
    .find(|value| !value.is_empty())
    .unwrap_or_else(|| "medium".to_string());
    normalize_gpt_image2_quality(candidate.as_str())
}

fn chatgpt_web_image_size(
    request: &ChatGptWebImageRequest,
    image: &DownloadedImage,
    report_context: Option<&Value>,
) -> Option<(u64, u64)> {
    if let Some(candidate) = downloaded_image_dimensions(image)
        .filter(|(width, height)| gpt_image2_dimensions_are_plausible(*width, *height))
    {
        return Some(candidate);
    }

    let candidates = [
        chatgpt_web_report_context_image_request_text(report_context, "size")
            .and_then(|value| parse_gpt_image2_size(value.as_str())),
        chatgpt_web_report_context_original_request_text(report_context, "size")
            .and_then(|value| parse_gpt_image2_size(value.as_str())),
        parse_gpt_image2_size(request.size.as_str()),
    ];
    for candidate in candidates.into_iter().flatten() {
        if gpt_image2_dimensions_are_valid(candidate.0, candidate.1) {
            return Some(candidate);
        }
    }

    let ratio = chatgpt_web_image_ratio(request, report_context);
    Some(chatgpt_web_fallback_size_for_ratio(ratio.as_str()))
}

fn chatgpt_web_image_partial_images(
    request: &ChatGptWebImageRequest,
    report_context: Option<&Value>,
) -> u64 {
    chatgpt_web_report_context_image_request_u64(report_context, "partial_images")
        .or_else(|| {
            chatgpt_web_report_context_original_request_u64(report_context, "partial_images")
        })
        .unwrap_or(request.partial_images)
}

fn chatgpt_web_image_ratio(
    request: &ChatGptWebImageRequest,
    report_context: Option<&Value>,
) -> String {
    chatgpt_web_report_context_image_request_text(report_context, "ratio")
        .or_else(|| chatgpt_web_report_context_original_request_text(report_context, "ratio"))
        .or_else(|| {
            chatgpt_web_report_context_original_request_text(report_context, "aspect_ratio")
        })
        .unwrap_or_else(|| request.ratio.clone())
}

fn chatgpt_web_report_context_image_request_text(
    report_context: Option<&Value>,
    key: &str,
) -> Option<String> {
    report_context
        .and_then(|value| value.get("image_request"))
        .and_then(|value| value.get(key))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn chatgpt_web_report_context_image_request_u64(
    report_context: Option<&Value>,
    key: &str,
) -> Option<u64> {
    report_context
        .and_then(|value| value.get("image_request"))
        .and_then(|value| value.get(key))
        .and_then(|value| json_u64(Some(value)))
}

fn chatgpt_web_report_context_original_request_text(
    report_context: Option<&Value>,
    key: &str,
) -> Option<String> {
    let original = report_context?.get("original_request_body")?;
    value_text(original.get(key)).or_else(|| {
        chatgpt_web_original_image_tool_value(original, key)
            .and_then(|value| value_text(Some(value)))
    })
}

fn chatgpt_web_report_context_original_request_u64(
    report_context: Option<&Value>,
    key: &str,
) -> Option<u64> {
    let original = report_context?.get("original_request_body")?;
    json_u64(original.get(key)).or_else(|| {
        chatgpt_web_original_image_tool_value(original, key).and_then(|value| json_u64(Some(value)))
    })
}

fn chatgpt_web_original_image_tool_value<'a>(original: &'a Value, key: &str) -> Option<&'a Value> {
    original
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|tool| {
            tool.get("type")
                .and_then(Value::as_str)
                .is_some_and(|value| value.trim().eq_ignore_ascii_case("image_generation"))
        })
        .find_map(|tool| tool.get(key))
}

fn chatgpt_web_image_prompt_text(
    request: &ChatGptWebImageRequest,
    report_context: Option<&Value>,
) -> String {
    chatgpt_web_report_context_original_request_text(report_context, "prompt")
        .or_else(|| chatgpt_web_report_context_image_request_text(report_context, "prompt"))
        .unwrap_or_else(|| request.prompt.clone())
}

fn value_text(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn downloaded_image_dimensions(image: &DownloadedImage) -> Option<(u64, u64)> {
    match (image.width, image.height) {
        (Some(width), Some(height)) if width > 0 && height > 0 => {
            Some((width as u64, height as u64))
        }
        _ => None,
    }
}

fn gpt_image2_dimensions_are_plausible(width: u64, height: u64) -> bool {
    let pixels = width.saturating_mul(height);
    if !(GPT_IMAGE2_TOKEN_MIN_PIXELS..=GPT_IMAGE2_TOKEN_MAX_PIXELS).contains(&pixels) {
        return false;
    }
    let max_edge = width.max(height);
    let min_edge = width.min(height);
    if max_edge > GPT_IMAGE2_TOKEN_MAX_EDGE {
        return false;
    }
    if max_edge > min_edge.saturating_mul(GPT_IMAGE2_TOKEN_MAX_ASPECT_RATIO) {
        return false;
    }
    true
}

fn gpt_image2_dimensions_are_valid(width: u64, height: u64) -> bool {
    width.is_multiple_of(16)
        && height.is_multiple_of(16)
        && gpt_image2_dimensions_are_plausible(width, height)
}

fn normalize_gpt_image2_quality(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "low" => "low".to_string(),
        "medium" | "standard" | "auto" => "medium".to_string(),
        "high" | "hd" => "high".to_string(),
        _ => "medium".to_string(),
    }
}

fn parse_gpt_image2_size(size: &str) -> Option<(u64, u64)> {
    let normalized = size.trim().to_ascii_lowercase().replace('×', "x");
    let (width, height) = normalized.split_once('x')?;
    let width = width
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)?;
    let height = height
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)?;
    Some((width, height))
}

fn chatgpt_web_fallback_size_for_ratio(ratio: &str) -> (u64, u64) {
    match ratio.trim() {
        "3:2" => (1216, 832),
        "2:3" => (832, 1216),
        "4:3" => (1152, 864),
        "3:4" => (864, 1152),
        "5:4" => (1120, 896),
        "4:5" => (896, 1120),
        "16:9" => (1344, 768),
        "9:16" => (768, 1344),
        "21:9" => (1536, 640),
        _ => (1024, 1024),
    }
}

// Estimate GPT Image 2 image-token output using the same dimensions and quality
// drivers as OpenAI's public cost calculator. This intentionally ignores the
// base64 response length, which is only a transport encoding.
fn gpt_image2_output_tokens(width: u64, height: u64, quality: &str) -> u64 {
    let quality_scale = match quality.trim().to_ascii_lowercase().as_str() {
        "low" => 16u64,
        "high" => 96u64,
        _ => 48u64,
    };
    let long = width.max(height);
    let short = width.min(height);
    let short_scale = round_div_u64(quality_scale.saturating_mul(short), long);
    let (long_scale, short_scale) = if width >= height {
        (quality_scale, short_scale)
    } else {
        (short_scale, quality_scale)
    };
    let latent_pixels = u128::from(long_scale).saturating_mul(u128::from(short_scale));
    let image_pixels = u128::from(width).saturating_mul(u128::from(height));
    let numerator =
        latent_pixels.saturating_mul(u128::from(2_000_000u64).saturating_add(image_pixels));
    let tokens = (numerator.saturating_add(4_000_000u128 - 1)) / 4_000_000u128;
    u64::try_from(tokens).unwrap_or(u64::MAX)
}

fn round_div_u64(numerator: u64, denominator: u64) -> u64 {
    if denominator == 0 {
        return 0;
    }
    numerator.saturating_add(denominator / 2) / denominator
}

fn estimate_text_tokens(text: &str) -> u64 {
    let chars = text.chars().count() as u64;
    if chars == 0 {
        0
    } else {
        chars.div_ceil(4).max(1)
    }
}

fn json_u64(value: Option<&Value>) -> Option<u64> {
    value.and_then(|value| {
        value
            .as_u64()
            .or_else(|| {
                value
                    .as_i64()
                    .and_then(|number| (number >= 0).then_some(number as u64))
            })
            .or_else(|| {
                value
                    .as_str()
                    .and_then(|number| number.trim().parse::<u64>().ok())
            })
    })
}

fn chatgpt_web_image_operation(value: Option<&Value>) -> String {
    let Some(value) = value.and_then(Value::as_str).map(str::trim) else {
        return "generate".to_string();
    };
    if value.eq_ignore_ascii_case("edit") {
        "edit".to_string()
    } else {
        "generate".to_string()
    }
}

fn build_failed_sse(request: &ChatGptWebImageRequest, failure: &Value) -> String {
    let failed = if failure.get("type").and_then(Value::as_str) == Some("response.failed") {
        failure.clone()
    } else {
        let operation = match request.operation.as_str() {
            "edit" => "edit",
            _ => "generation",
        };
        json!({
            "type": "response.failed",
            "response": {
                "status": "failed",
                "model": request.model,
                "error": failure.get("error").cloned().unwrap_or_else(|| json!({
                    "code": "chatgpt_web_image_failed",
                    "message": format!("ChatGPT-Web image {operation} failed")
                }))
            }
        })
    };
    format!("event: response.failed\ndata: {failed}\n\ndata: [DONE]\n\n")
}

fn output_format_from_mime(mime: &str, fallback: &str) -> String {
    match mime {
        "image/jpeg" | "image/jpg" => "jpeg",
        "image/webp" => "webp",
        "image/png" => "png",
        _ => fallback,
    }
    .to_string()
}

fn json_execution_result(
    plan: &ExecutionPlan,
    status_code: u16,
    body: Value,
    started_at: Instant,
) -> ExecutionResult {
    ExecutionResult {
        request_id: plan.request_id.clone(),
        candidate_id: plan.candidate_id.clone(),
        status_code,
        headers: BTreeMap::from([("content-type".to_string(), "application/json".to_string())]),
        response_observation: None,
        body: Some(ResponseBody {
            json_body: Some(body),
            body_bytes_b64: None,
        }),
        telemetry: Some(telemetry(started_at, 0)),
        error: None,
    }
}

fn chatgpt_web_http_error_execution_result(
    plan: &ExecutionPlan,
    started_at: Instant,
    status_code: u16,
    message: &str,
) -> ExecutionResult {
    json_execution_result(
        plan,
        status_code,
        json!({
            "error": {
                "type": "upstream_error",
                "code": "chatgpt_web_image_execution_unavailable",
                "message": message
            }
        }),
        started_at,
    )
}

fn bytes_execution_result(
    plan: &ExecutionPlan,
    status_code: u16,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
    started_at: Instant,
) -> Result<ExecutionResult, ExecutionRuntimeTransportError> {
    let envelope_limit = chatgpt_web_image_sse_envelope_limit_bytes();
    if body.len() > envelope_limit {
        return Err(ExecutionRuntimeTransportError::BodyTooLarge {
            limit_bytes: envelope_limit,
        });
    }
    let body_len = body.len() as u64;
    Ok(ExecutionResult {
        request_id: plan.request_id.clone(),
        candidate_id: plan.candidate_id.clone(),
        status_code,
        headers,
        response_observation: None,
        body: Some(ResponseBody {
            json_body: None,
            body_bytes_b64: Some(base64::engine::general_purpose::STANDARD.encode(body)),
        }),
        telemetry: Some(telemetry(started_at, body_len)),
        error: None,
    })
}

fn execution_result_frame_stream(
    plan: &ExecutionPlan,
    result: &ExecutionResult,
    report_context: Option<&Value>,
) -> Result<BoxStream<'static, Result<Bytes, IoError>>, ExecutionRuntimeTransportError> {
    // The synthetic ChatGPT-Web SSE body embeds an image as base64, so its
    // envelope is larger than the decoded image/body limit.  Use the bounded
    // envelope budget here instead of rejecting valid images near 64 MiB.
    let body =
        execution_result_bytes_with_limit(result, chatgpt_web_image_sse_envelope_limit_bytes())?;
    let terminal_summary = chatgpt_web_stream_terminal_summary(plan, result, report_context, &body);
    let mut frames = vec![
        StreamFrame {
            frame_type: StreamFrameType::Headers,
            payload: StreamFramePayload::Headers {
                status_code: result.status_code,
                headers: result.headers.clone(),
                response_observation: result.response_observation.clone(),
            },
        },
        StreamFrame {
            frame_type: StreamFrameType::Telemetry,
            payload: StreamFramePayload::Telemetry {
                telemetry: ExecutionTelemetry {
                    ttfb_ms: result.telemetry.as_ref().and_then(|value| value.ttfb_ms),
                    elapsed_ms: result.telemetry.as_ref().and_then(|value| value.elapsed_ms),
                    upstream_bytes: Some(0),
                },
            },
        },
    ];
    for chunk in body.chunks(CHATGPT_WEB_IMAGE_STREAM_CHUNK_BYTES) {
        frames.push(StreamFrame {
            frame_type: StreamFrameType::Data,
            payload: StreamFramePayload::Data {
                chunk_b64: Some(base64::engine::general_purpose::STANDARD.encode(chunk)),
                text: None,
            },
        });
    }
    frames.push(StreamFrame {
        frame_type: StreamFrameType::Telemetry,
        payload: StreamFramePayload::Telemetry {
            telemetry: result.telemetry.clone().unwrap_or(ExecutionTelemetry {
                ttfb_ms: None,
                elapsed_ms: None,
                upstream_bytes: None,
            }),
        },
    });
    frames.push(StreamFrame::eof_with_summary(terminal_summary));
    Ok(stream::iter(
        frames
            .into_iter()
            .map(|frame| encode_stream_frame_ndjson(&frame)),
    )
    .boxed())
}

fn chatgpt_web_stream_terminal_summary(
    plan: &ExecutionPlan,
    result: &ExecutionResult,
    report_context: Option<&Value>,
    body: &[u8],
) -> Option<ExecutionStreamTerminalSummary> {
    if !(200..300).contains(&result.status_code) || body.is_empty() {
        return None;
    }

    let observer_context = chatgpt_web_stream_observer_context(plan, report_context);
    let mut observer = StreamingStandardTerminalObserver::default();
    let mut line_start = 0usize;
    for (index, byte) in body.iter().enumerate() {
        if *byte != b'\n' {
            continue;
        }
        observer
            .push_line(&observer_context, body[line_start..=index].to_vec())
            .ok()?;
        line_start = index.saturating_add(1);
    }
    if line_start < body.len() {
        observer
            .push_line(&observer_context, body[line_start..].to_vec())
            .ok()?;
    }
    observer.finish(&observer_context).ok().flatten()
}

fn chatgpt_web_stream_observer_context(
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
) -> Value {
    let mut context = report_context
        .cloned()
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    let object = context
        .as_object_mut()
        .expect("observer context should be an object");
    object
        .entry("provider_api_format".to_string())
        .or_insert_with(|| Value::String(plan.provider_api_format.clone()));
    object
        .entry("client_api_format".to_string())
        .or_insert_with(|| Value::String(plan.client_api_format.clone()));
    object
        .entry("model".to_string())
        .or_insert_with(|| Value::String(plan.model_name.clone().unwrap_or_default()));
    if !object.contains_key("image_request") {
        if let Some(image_request) = chatgpt_web_image_request_context(plan) {
            object.insert("image_request".to_string(), image_request);
        }
    }
    context
}

fn chatgpt_web_image_request_context(plan: &ExecutionPlan) -> Option<Value> {
    let body = plan.body.json_body.as_ref()?.as_object()?;
    let mut image_request = Map::new();
    image_request.insert(
        "operation".to_string(),
        Value::String(chatgpt_web_image_operation(body.get("operation"))),
    );
    for key in [
        "model",
        "size",
        "quality",
        "ratio",
        "output_format",
        "partial_images",
    ] {
        if let Some(value) = body.get(key).and_then(Value::as_str).map(str::trim) {
            if !value.is_empty() {
                image_request.insert(key.to_string(), Value::String(value.to_string()));
            }
            continue;
        }
        if let Some(value) = body.get(key).and_then(Value::as_u64) {
            image_request.insert(key.to_string(), Value::Number(value.into()));
        }
    }
    Some(Value::Object(image_request))
}

fn telemetry(started_at: Instant, upstream_bytes: u64) -> ExecutionTelemetry {
    let elapsed_ms = started_at.elapsed().as_millis() as u64;
    ExecutionTelemetry {
        ttfb_ms: Some(elapsed_ms),
        elapsed_ms: Some(elapsed_ms),
        upstream_bytes: Some(upstream_bytes),
    }
}

fn execution_result_json(
    result: &ExecutionResult,
) -> Result<Value, ExecutionRuntimeTransportError> {
    if let Some(json_body) = result
        .body
        .as_ref()
        .and_then(|body| body.json_body.as_ref())
    {
        return Ok(json_body.clone());
    }
    let bytes = execution_result_bytes(result)?;
    serde_json::from_slice(&bytes).map_err(ExecutionRuntimeTransportError::InvalidJson)
}

fn execution_result_bytes(
    result: &ExecutionResult,
) -> Result<Vec<u8>, ExecutionRuntimeTransportError> {
    execution_result_bytes_with_limit(result, crate::headers::max_internal_buffered_body_bytes())
}

fn execution_result_bytes_with_limit(
    result: &ExecutionResult,
    body_limit: usize,
) -> Result<Vec<u8>, ExecutionRuntimeTransportError> {
    let Some(body) = result.body.as_ref() else {
        return Ok(Vec::new());
    };
    if let Some(json_body) = body.json_body.as_ref() {
        return serialize_json_body_with_limit(json_body, body_limit);
    }
    body.body_bytes_b64
        .as_deref()
        .map(|value| decode_base64_body_with_limit(value, body_limit))
        .unwrap_or_else(|| Ok(Vec::new()))
}

fn execution_result_body_bytes_lossy(result: &ExecutionResult) -> Vec<u8> {
    execution_result_bytes(result).unwrap_or_default()
}

pub(super) fn chatgpt_web_image_sse_envelope_limit_bytes() -> usize {
    let image_limit = crate::headers::max_internal_buffered_body_bytes();
    maximum_base64_len_for_decoded_limit(image_limit)
        .saturating_add(CHATGPT_WEB_IMAGE_SSE_WRAPPER_OVERHEAD_BYTES)
        .min(CHATGPT_WEB_IMAGE_SSE_HARD_MAX_BYTES)
}

fn chatgpt_web_image_raw_payload_limit_bytes() -> usize {
    let configured_limit = crate::headers::max_internal_buffered_body_bytes();
    let envelope_limit = chatgpt_web_image_sse_envelope_limit_bytes();
    let available_for_base64 =
        envelope_limit.saturating_sub(CHATGPT_WEB_IMAGE_SSE_WRAPPER_OVERHEAD_BYTES);
    // Standard base64 expands three bytes into four.  Use the floor of the
    // inverse expansion so a raw image can always be represented by the
    // synthetic SSE envelope without first allocating an over-sized body.
    let representable_raw_limit = available_for_base64
        .saturating_div(4)
        .saturating_mul(3)
        .max(1);
    configured_limit.min(representable_raw_limit)
}

fn ensure_success(
    result: &ExecutionResult,
    stage: &str,
) -> Result<(), ExecutionRuntimeTransportError> {
    if (200..300).contains(&result.status_code) {
        return Ok(());
    }
    Err(ExecutionRuntimeTransportError::UpstreamHttpStatus {
        status_code: result.status_code,
        message: chatgpt_web_stage_http_error_message(stage, result.status_code),
    })
}

fn chatgpt_web_stage_http_error_message(stage: &str, status_code: u16) -> String {
    format!("{stage} returned HTTP {status_code}")
}

fn chatgpt_web_base_url_from_plan(plan: &ExecutionPlan) -> String {
    let Ok(url) = url::Url::parse(&plan.url) else {
        return CHATGPT_WEB_DEFAULT_BASE_URL.to_string();
    };
    let Some(host) = url.host_str() else {
        return CHATGPT_WEB_DEFAULT_BASE_URL.to_string();
    };
    let port = url
        .port()
        .map(|port| format!(":{port}"))
        .unwrap_or_default();
    format!("{}://{}{}", url.scheme(), host, port)
}

fn bearer_token_from_headers(headers: &BTreeMap<String, String>) -> Option<String> {
    headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("authorization"))
        .and_then(|(_, value)| {
            value
                .trim()
                .strip_prefix("Bearer ")
                .or_else(|| value.trim().strip_prefix("bearer "))
                .map(str::trim)
        })
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn build_legacy_requirements_token(user_agent: &str) -> String {
    let seed = format!("0.{}", Uuid::new_v4().simple());
    let (answer, _) = pow_generate(seed.as_str(), "0fffff", pow_config(user_agent));
    format!("gAAAAAC{answer}")
}

fn build_proof_token(seed: &str, difficulty: &str, user_agent: &str) -> String {
    let (answer, solved) = pow_generate(seed.trim(), difficulty.trim(), pow_config(user_agent));
    if solved {
        format!("gAAAAAB{answer}")
    } else {
        format!(
            "gAAAAAB{}",
            base64::engine::general_purpose::STANDARD.encode(format!("\"{}\"", seed.trim()))
        )
    }
}

fn pow_config(user_agent: &str) -> Vec<Value> {
    let est = FixedOffset::west_opt(5 * 3600).expect("fixed EST offset should be valid");
    let now = Utc::now();
    let now_est = now.with_timezone(&est);
    let timestamp_ms = now.timestamp_millis() as f64;
    vec![
        json!(3000),
        json!(format!(
            "{} GMT-0500 (Eastern Standard Time)",
            now_est.format("%a %b %d %Y %H:%M:%S")
        )),
        json!(4_294_705_152_u64),
        json!(0),
        json!(user_agent),
        json!("https://chatgpt.com/backend-api/sentinel/sdk.js"),
        json!(""),
        json!("en-US"),
        json!("en-US,es-US,en,es"),
        json!(0),
        json!("webdriver≭false"),
        json!("location"),
        json!("window"),
        json!(timestamp_ms),
        json!(Uuid::new_v4().to_string()),
        json!(""),
        json!(16),
        json!(timestamp_ms),
    ]
}

fn pow_generate(seed: &str, difficulty: &str, config: Vec<Value>) -> (String, bool) {
    let Some(diff_bytes) = hex_to_bytes(difficulty) else {
        return (encode_pow_seed(seed), false);
    };
    // `sha3_512` yields exactly 64 bytes.  Difficulty comes from the
    // upstream sentinel response, so reject an overlong value before the
    // comparison below could slice the digest out of bounds.
    if diff_bytes.is_empty() || diff_bytes.len() > 64 {
        return (encode_pow_seed(seed), false);
    }

    let static1 = serde_json::to_string(&config[..3]).unwrap_or_else(|_| "[]".to_string());
    let static1 = format!("{},", static1.trim_end_matches(']'));
    let static2 = serde_json::to_string(&config[4..9]).unwrap_or_else(|_| "[]".to_string());
    let static2 = format!(
        ",{},",
        static2.trim_start_matches('[').trim_end_matches(']')
    );
    let static3 = serde_json::to_string(&config[10..]).unwrap_or_else(|_| "[]".to_string());
    let static3 = format!(",{}", static3.trim_start_matches('['));
    let seed_bytes = seed.as_bytes();

    for i in 0..500_000_u64 {
        let final_config = format!("{static1}{i}{static2}{}{static3}", i >> 1);
        let encoded = base64::engine::general_purpose::STANDARD.encode(final_config.as_bytes());
        let mut candidate = Vec::with_capacity(seed_bytes.len() + encoded.len());
        candidate.extend_from_slice(seed_bytes);
        candidate.extend_from_slice(encoded.as_bytes());
        let digest = sha3_512(candidate.as_slice());
        if digest[..diff_bytes.len()] <= diff_bytes[..] {
            return (encoded, true);
        }
    }

    (encode_pow_seed(seed), false)
}

fn encode_pow_seed(seed: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(format!("\"{}\"", seed.trim()))
}

fn hex_to_bytes(value: &str) -> Option<Vec<u8>> {
    // Avoid copying/allocating an unbounded upstream difficulty string.  The
    // proof comparison cannot consume more than the 64-byte SHA-3 digest.
    let trimmed = value.trim();
    if trimmed.len() > 128 {
        return None;
    }
    let mut hex = trimmed.to_string();
    if hex.len() % 2 == 1 {
        hex.insert(0, '0');
    }
    let mut out = Vec::with_capacity(hex.len() / 2);
    let bytes = hex.as_bytes();
    for chunk in bytes.chunks(2) {
        let high = hex_nibble(chunk[0])?;
        let low = hex_nibble(chunk[1])?;
        out.push((high << 4) | low);
    }
    Some(out)
}

fn hex_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn sha3_512(input: &[u8]) -> [u8; 64] {
    const RATE: usize = 72;
    let mut state = [0_u64; 25];
    let mut offset = 0;
    while offset + RATE <= input.len() {
        absorb_sha3_block(&mut state, &input[offset..offset + RATE]);
        keccak_f1600(&mut state);
        offset += RATE;
    }

    let mut block = [0_u8; RATE];
    let remaining = &input[offset..];
    block[..remaining.len()].copy_from_slice(remaining);
    block[remaining.len()] ^= 0x06;
    block[RATE - 1] ^= 0x80;
    absorb_sha3_block(&mut state, &block);
    keccak_f1600(&mut state);

    let mut out = [0_u8; 64];
    for (lane, chunk) in state.iter().zip(out.chunks_mut(8)) {
        chunk.copy_from_slice(&lane.to_le_bytes());
    }
    out
}

fn absorb_sha3_block(state: &mut [u64; 25], block: &[u8]) {
    for (index, chunk) in block.chunks_exact(8).enumerate() {
        state[index] ^= u64::from_le_bytes([
            chunk[0], chunk[1], chunk[2], chunk[3], chunk[4], chunk[5], chunk[6], chunk[7],
        ]);
    }
}

fn keccak_f1600(state: &mut [u64; 25]) {
    const ROUND_CONSTANTS: [u64; 24] = [
        0x0000_0000_0000_0001,
        0x0000_0000_0000_8082,
        0x8000_0000_0000_808a,
        0x8000_0000_8000_8000,
        0x0000_0000_0000_808b,
        0x0000_0000_8000_0001,
        0x8000_0000_8000_8081,
        0x8000_0000_0000_8009,
        0x0000_0000_0000_008a,
        0x0000_0000_0000_0088,
        0x0000_0000_8000_8009,
        0x0000_0000_8000_000a,
        0x0000_0000_8000_808b,
        0x8000_0000_0000_008b,
        0x8000_0000_0000_8089,
        0x8000_0000_0000_8003,
        0x8000_0000_0000_8002,
        0x8000_0000_0000_0080,
        0x0000_0000_0000_800a,
        0x8000_0000_8000_000a,
        0x8000_0000_8000_8081,
        0x8000_0000_0000_8080,
        0x0000_0000_8000_0001,
        0x8000_0000_8000_8008,
    ];
    const RHO: [u32; 25] = [
        0, 1, 62, 28, 27, 36, 44, 6, 55, 20, 3, 10, 43, 25, 39, 41, 45, 15, 21, 8, 18, 2, 61, 56,
        14,
    ];

    for round_constant in ROUND_CONSTANTS {
        let mut c = [0_u64; 5];
        for x in 0..5 {
            c[x] = state[x] ^ state[x + 5] ^ state[x + 10] ^ state[x + 15] ^ state[x + 20];
        }
        for x in 0..5 {
            let d = c[(x + 4) % 5] ^ c[(x + 1) % 5].rotate_left(1);
            for y in 0..5 {
                state[x + 5 * y] ^= d;
            }
        }

        let mut b = [0_u64; 25];
        for x in 0..5 {
            for y in 0..5 {
                b[y + 5 * ((2 * x + 3 * y) % 5)] = state[x + 5 * y].rotate_left(RHO[x + 5 * y]);
            }
        }

        for y in 0..5 {
            for x in 0..5 {
                state[x + 5 * y] =
                    b[x + 5 * y] ^ ((!b[(x + 1) % 5 + 5 * y]) & b[(x + 2) % 5 + 5 * y]);
            }
        }

        state[0] ^= round_constant;
    }
}

fn parse_data_url(value: &str) -> Option<DownloadedImage> {
    parse_data_url_with_limit(value, chatgpt_web_image_raw_payload_limit_bytes())
}

fn parse_data_url_with_limit(value: &str, decoded_limit: usize) -> Option<DownloadedImage> {
    let (header, data) = value.trim().split_once(',')?;
    if header.is_empty()
        || data.is_empty()
        || header
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
        || data
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
    {
        return None;
    }

    // Only pass image formats that the OpenAI image surface can represent safely.
    // In particular, accepting arbitrary `image/*` values would allow SVG/XML
    // payloads to cross a JSON image boundary and be interpreted as active markup.
    let (scheme, metadata) = header.split_once(':')?;
    if !scheme.eq_ignore_ascii_case("data") {
        return None;
    }
    let (mime, encoding) = metadata.rsplit_once(';')?;
    if !encoding.eq_ignore_ascii_case("base64") || mime.contains(';') {
        return None;
    }
    let mime = if mime.eq_ignore_ascii_case("image/png") {
        "image/png"
    } else if mime.eq_ignore_ascii_case("image/jpeg") || mime.eq_ignore_ascii_case("image/jpg") {
        "image/jpeg"
    } else if mime.eq_ignore_ascii_case("image/webp") {
        "image/webp"
    } else {
        return None;
    };

    // Check the encoded length before invoking the decoder.  The base64 engine
    // allocates from the input length, so a decoded-size check performed after
    // decoding would still leave an allocation DoS.  Keep the operator-configured
    // 64 MiB default so valid large image responses remain supported.
    let bytes = decode_base64_body_with_limit(data, decoded_limit).ok()?;
    let detected_mime = validate_web_image_payload(&bytes, Some(mime)).ok()?;
    let (width, height) = image_dimensions(&bytes);
    Some(DownloadedImage {
        // `decode_base64_body_with_limit` has already validated the canonical
        // alphabet and padding.  Preserve the source text to avoid a second
        // 64 MiB-scale allocation when handling large images.
        b64_json: data.to_string(),
        mime: detected_mime.to_string(),
        width,
        height,
    })
}

fn image_dimensions(bytes: &[u8]) -> (Option<u32>, Option<u32>) {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") && bytes.len() >= 24 {
        let width = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
        let height = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
        if width == 0
            || height == 0
            || width > CHATGPT_WEB_IMAGE_MAX_DIMENSION
            || height > CHATGPT_WEB_IMAGE_MAX_DIMENSION
        {
            return (None, None);
        }
        return (Some(width), Some(height));
    }
    if bytes.starts_with(&[0xff, 0xd8]) {
        let mut cursor = 2usize;
        while cursor + 9 < bytes.len() {
            if bytes[cursor] != 0xff {
                cursor += 1;
                continue;
            }
            let marker = bytes[cursor + 1];
            let segment_len = u16::from_be_bytes([bytes[cursor + 2], bytes[cursor + 3]]) as usize;
            if matches!(
                marker,
                0xc0 | 0xc1
                    | 0xc2
                    | 0xc3
                    | 0xc5
                    | 0xc6
                    | 0xc7
                    | 0xc9
                    | 0xca
                    | 0xcb
                    | 0xcd
                    | 0xce
                    | 0xcf
            ) && cursor + 8 < bytes.len()
            {
                let height = u16::from_be_bytes([bytes[cursor + 5], bytes[cursor + 6]]) as u32;
                let width = u16::from_be_bytes([bytes[cursor + 7], bytes[cursor + 8]]) as u32;
                if width == 0
                    || height == 0
                    || width > CHATGPT_WEB_IMAGE_MAX_DIMENSION
                    || height > CHATGPT_WEB_IMAGE_MAX_DIMENSION
                {
                    return (None, None);
                }
                return (Some(width), Some(height));
            }
            if segment_len < 2 {
                break;
            }
            cursor = cursor.saturating_add(2 + segment_len);
        }
    }
    (None, None)
}

fn is_web_file_id(value: &str) -> bool {
    let value = value.trim();
    (value.starts_with("file-") || value.starts_with("file_"))
        && value.len() >= 10
        && web_opaque_id_is_safe(value)
}

fn web_opaque_id_is_safe(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= CHATGPT_WEB_OPAQUE_ID_MAX_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn validated_web_opaque_id<'a>(
    value: &'a str,
    field: &str,
) -> Result<&'a str, ExecutionRuntimeTransportError> {
    let value = value.trim();
    if !web_opaque_id_is_safe(value) {
        return Err(ExecutionRuntimeTransportError::UpstreamRequest(format!(
            "ChatGPT-Web response contains an invalid {field}"
        )));
    }
    Ok(value)
}

fn validated_web_file_id(value: &str) -> Result<&str, ExecutionRuntimeTransportError> {
    let value = value.trim();
    if !is_web_file_id(value) {
        return Err(ExecutionRuntimeTransportError::UpstreamRequest(
            "ChatGPT-Web response contains an invalid file ID".to_string(),
        ));
    }
    Ok(value)
}

fn web_dns_host_is_valid(host: &str) -> bool {
    let host = host.strip_suffix('.').unwrap_or(host);
    !host.is_empty()
        && !host.ends_with('.')
        && host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                && label
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_alphanumeric)
                && label
                    .as_bytes()
                    .last()
                    .is_some_and(u8::is_ascii_alphanumeric)
        })
}

fn web_host_is_domain_or_subdomain(host: &str, domain: &str) -> bool {
    let host = host.strip_suffix('.').unwrap_or(host);
    if !web_dns_host_is_valid(host) {
        return false;
    }
    if host.eq_ignore_ascii_case(domain) {
        return true;
    }
    host.len() > domain.len()
        && host.as_bytes()[host.len() - domain.len() - 1] == b'.'
        && host[host.len() - domain.len()..].eq_ignore_ascii_case(domain)
}

fn web_host_is_strict_subdomain(host: &str, domain: &str) -> bool {
    web_host_is_domain_or_subdomain(host, domain)
        && !host.trim_end_matches('.').eq_ignore_ascii_case(domain)
}

fn is_generated_web_asset_url(raw_url: &str) -> bool {
    let Ok(url) = url::Url::parse(raw_url.trim()) else {
        return false;
    };
    if validate_web_image_http_url(&url).is_err() {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    let path = url.path().to_ascii_lowercase();
    if web_host_is_domain_or_subdomain(host, "openaiassets.blob.core.windows.net") {
        return false;
    }
    if path.contains("/$web/chatgpt/") {
        return false;
    }
    web_host_is_domain_or_subdomain(host, "files.oaiusercontent.com")
        || web_host_is_domain_or_subdomain(host, "oaidalleapiprodscus.blob.core.windows.net")
        || (web_host_is_strict_subdomain(host, "blob.core.windows.net") && !path.contains("/$web/"))
}

fn is_authenticated_web_download_url(base: &url::Url, target: &url::Url) -> bool {
    target.path().starts_with("/backend-api/")
        && web_download_url_is_same_origin(base, target)
        && target.username().is_empty()
        && target.password().is_none()
}

fn web_download_url_is_same_origin(base: &url::Url, target: &url::Url) -> bool {
    target.scheme().eq_ignore_ascii_case(base.scheme())
        && target
            .host_str()
            .zip(base.host_str())
            .is_some_and(|(target, base)| target.eq_ignore_ascii_case(base))
        && target.port_or_known_default() == base.port_or_known_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
    use aether_data_contracts::repository::provider_catalog::{
        ProviderCatalogReadRepository, StoredProviderCatalogEndpoint, StoredProviderCatalogKey,
        StoredProviderCatalogProvider,
    };
    use axum::body::Body;
    use axum::extract::Request;
    use axum::routing::any;
    use axum::Router;
    use futures_util::StreamExt as _;
    use http::{Method, StatusCode};

    use crate::data::GatewayDataState;

    fn sample_plan(base_url: &str, body: Value, stream: bool) -> ExecutionPlan {
        ExecutionPlan {
            request_id: "req-chatgpt-web-image-test".to_string(),
            candidate_id: Some("cand-chatgpt-web-image-test".to_string()),
            provider_name: Some("ChatGPT Web".to_string()),
            provider_id: "provider-chatgpt-web-image-test".to_string(),
            endpoint_id: "endpoint-chatgpt-web-image-test".to_string(),
            key_id: "key-chatgpt-web-image-test".to_string(),
            method: "POST".to_string(),
            url: format!("{base_url}/__aether/chatgpt-web-image"),
            headers: BTreeMap::from([
                (CHATGPT_WEB_INTERNAL_HEADER.to_string(), "1".to_string()),
                (
                    "authorization".to_string(),
                    "Bearer test-access-token".to_string(),
                ),
            ]),
            content_type: Some("application/json".to_string()),
            content_encoding: None,
            body: RequestBody::from_json(body),
            stream,
            client_api_format: "openai:image".to_string(),
            provider_api_format: "openai:image".to_string(),
            model_name: Some("gpt-image-2".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        }
    }

    fn sample_provider_catalog_provider() -> StoredProviderCatalogProvider {
        StoredProviderCatalogProvider::new(
            "provider-chatgpt-web-image-test".to_string(),
            "ChatGPT Web".to_string(),
            Some(CHATGPT_WEB_DEFAULT_BASE_URL.to_string()),
            "chatgpt_web".to_string(),
        )
        .expect("provider should build")
    }

    fn sample_provider_catalog_endpoint(base_url: &str) -> StoredProviderCatalogEndpoint {
        StoredProviderCatalogEndpoint::new(
            "endpoint-chatgpt-web-image-test".to_string(),
            "provider-chatgpt-web-image-test".to_string(),
            "openai:image".to_string(),
            Some("openai".to_string()),
            Some("image".to_string()),
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            base_url.to_string(),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("endpoint transport fields should build")
    }

    fn sample_provider_catalog_key(upstream_metadata: Value) -> StoredProviderCatalogKey {
        let mut key = StoredProviderCatalogKey::new(
            "key-chatgpt-web-image-test".to_string(),
            "provider-chatgpt-web-image-test".to_string(),
            "ChatGPT Web test key".to_string(),
            "oauth".to_string(),
            None,
            true,
        )
        .expect("key should build")
        .with_transport_fields(
            Some(json!(["openai:image"])),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("key transport fields should build");
        key.upstream_metadata = Some(upstream_metadata);
        key
    }

    fn state_with_chatgpt_web_key(
        base_url: &str,
        upstream_metadata: Value,
    ) -> (AppState, Arc<InMemoryProviderCatalogReadRepository>) {
        let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider_catalog_provider()],
            vec![sample_provider_catalog_endpoint(base_url)],
            vec![sample_provider_catalog_key(upstream_metadata)],
        ));
        let state = crate::AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(Arc::clone(
                    &repository,
                )),
            );
        (state, repository)
    }

    async fn reloaded_chatgpt_web_metadata(
        repository: &InMemoryProviderCatalogReadRepository,
    ) -> Map<String, Value> {
        repository
            .list_keys_by_ids(&["key-chatgpt-web-image-test".to_string()])
            .await
            .expect("key reload should succeed")
            .into_iter()
            .next()
            .expect("key should exist")
            .upstream_metadata
            .and_then(|value| value.get("chatgpt_web").cloned())
            .and_then(|value| value.as_object().cloned())
            .expect("chatgpt_web metadata should exist")
    }

    fn completed_response_from_sse(sse: &str) -> Value {
        sse.lines()
            .find_map(|line| {
                let payload = line.strip_prefix("data: ")?;
                let event = serde_json::from_str::<Value>(payload).ok()?;
                (event.get("type").and_then(Value::as_str) == Some("response.completed"))
                    .then_some(event)
            })
            .and_then(|event| event.get("response").cloned())
            .expect("completed response should be present")
    }

    #[test]
    fn gpt_image2_output_token_estimator_matches_pricing_calculator_examples() {
        assert_eq!(gpt_image2_output_tokens(1024, 1024, "low"), 196);
        assert_eq!(gpt_image2_output_tokens(1024, 1024, "medium"), 1756);
        assert_eq!(gpt_image2_output_tokens(1536, 1024, "medium"), 1372);
        assert_eq!(gpt_image2_output_tokens(1024, 1536, "medium"), 1372);
        assert_eq!(gpt_image2_output_tokens(1024, 1024, "high"), 7024);
    }

    #[test]
    fn chatgpt_web_http_status_error_omits_upstream_body() {
        let result = ExecutionResult {
            request_id: "req-chatgpt-web-image-test".to_string(),
            candidate_id: None,
            status_code: 502,
            headers: BTreeMap::new(),
            response_observation: None,
            body: Some(ResponseBody {
                json_body: Some(json!({"error": "Bearer secret-chatgpt-web-body"})),
                body_bytes_b64: None,
            }),
            telemetry: None,
            error: None,
        };

        let error = ensure_success(&result, "ChatGPT-Web bootstrap")
            .expect_err("non-success result should be rejected");
        let message = error.to_string();

        assert!(matches!(
            error,
            ExecutionRuntimeTransportError::UpstreamHttpStatus {
                status_code: 502,
                ..
            }
        ));
        assert_eq!(message, "ChatGPT-Web bootstrap returned HTTP 502");
        assert!(!message.contains("secret-chatgpt-web-body"));
    }

    #[test]
    fn chatgpt_web_success_sse_includes_estimated_image_usage() {
        let request = ChatGptWebImageRequest {
            operation: "generate".to_string(),
            model: "gpt-image-2".to_string(),
            web_model: "gpt-5-5-thinking".to_string(),
            prompt: "draw a test image".to_string(),
            size: "1024x1024".to_string(),
            ratio: "1:1".to_string(),
            output_format: "png".to_string(),
            quality: Some("low".to_string()),
            partial_images: 0,
            images: Vec::new(),
        };
        let image = DownloadedImage {
            b64_json: "aGVsbG8=".repeat(128),
            mime: "image/png".to_string(),
            width: Some(1024),
            height: Some(1024),
        };
        let body = build_success_sse(
            &request,
            &image,
            Some(&json!({
                "image_request": {
                    "size": "1024x1024",
                    "quality": "low"
                }
            })),
        );
        let completed = completed_response_from_sse(body.as_str());
        let input_tokens = estimate_text_tokens("draw a test image");
        let output_tokens = 196;

        assert_eq!(completed["usage"]["input_tokens"], json!(input_tokens));
        assert_eq!(completed["usage"]["output_tokens"], json!(output_tokens));
        assert_eq!(
            completed["tool_usage"]["image_gen"]["output_tokens"],
            json!(output_tokens)
        );
        assert_eq!(
            completed["tool_usage"]["image_gen"]["input_tokens_details"]["text_tokens"],
            json!(input_tokens)
        );
        assert_eq!(
            completed["tool_usage"]["image_gen"]["output_tokens_details"]["image_tokens"],
            json!(output_tokens)
        );
        assert_eq!(
            completed["usage"]["total_tokens"],
            json!(input_tokens.saturating_add(output_tokens))
        );
    }

    #[test]
    fn chatgpt_web_success_sse_uses_image_dimensions_not_output_text() {
        let request = ChatGptWebImageRequest {
            operation: "generate".to_string(),
            model: "gpt-image-2".to_string(),
            web_model: "gpt-5-5-thinking".to_string(),
            prompt: "draw a test image".to_string(),
            size: "1024x1024".to_string(),
            ratio: "1:1".to_string(),
            output_format: "png".to_string(),
            quality: Some("low".to_string()),
            partial_images: 0,
            images: Vec::new(),
        };
        let image = DownloadedImage {
            b64_json: "iVBORw0KGgoAAAANSUhEUgAA".repeat(64),
            mime: "image/png".to_string(),
            width: Some(1402),
            height: Some(1122),
        };
        let body = build_success_sse(
            &request,
            &image,
            Some(&json!({
                "image_request": {
                    "size": "1024x1024",
                    "quality": "low"
                }
            })),
        );
        let completed = completed_response_from_sse(body.as_str());

        assert_eq!(
            completed["usage"]["output_tokens"],
            json!(gpt_image2_output_tokens(1402, 1122, "low"))
        );
    }

    #[test]
    fn chatgpt_web_image_subrequests_default_to_browser_wreq_transport() {
        let plan = sample_plan(
            CHATGPT_WEB_DEFAULT_BASE_URL,
            json!({"prompt": "draw a small test image"}),
            false,
        );

        let profile = chatgpt_web_image_transport_profile(&plan).expect("transport profile");

        assert_eq!(profile.backend, TRANSPORT_BACKEND_BROWSER_WREQ);
        assert_eq!(profile.profile_id, CHATGPT_WEB_BROWSER_PROFILE);
        assert_eq!(profile.http_mode, TRANSPORT_HTTP_MODE_AUTO);
        assert_eq!(profile.pool_scope, TRANSPORT_POOL_SCOPE_KEY);
        assert_eq!(
            profile
                .extra
                .as_ref()
                .and_then(|value| value.get("source"))
                .and_then(Value::as_str),
            Some("chatgpt_web_image_default")
        );
    }

    #[test]
    fn chatgpt_web_image_request_context_preserves_edit_operation() {
        let plan = sample_plan(
            CHATGPT_WEB_DEFAULT_BASE_URL,
            json!({
                "operation": "edit",
                "model": "gpt-image-2",
                "web_model": "gpt-5-5-thinking",
                "prompt": "adjust this image",
                "size": "512x512",
                "ratio": "1:1",
                "images": ["data:image/png;base64,aW1hZ2U="],
                "count": 1,
                "output_format": "png"
            }),
            true,
        );

        let context = chatgpt_web_stream_observer_context(&plan, None);

        assert_eq!(context["image_request"]["operation"], json!("edit"));
        assert_eq!(context["image_request"]["model"], json!("gpt-image-2"));
        assert_eq!(context["image_request"]["size"], json!("512x512"));
        assert_eq!(context["provider_api_format"], json!("openai:image"));
    }

    #[test]
    fn chatgpt_web_image_quota_refresh_plan_uses_conversation_init() {
        let plan = sample_plan(
            CHATGPT_WEB_DEFAULT_BASE_URL,
            json!({"prompt": "draw a small test image"}),
            false,
        );
        let spec = build_chatgpt_web_pool_quota_request(
            &plan.key_id,
            CHATGPT_WEB_DEFAULT_BASE_URL,
            (
                "authorization".to_string(),
                "Bearer test-access-token".to_string(),
            ),
        );

        let quota_plan = build_chatgpt_web_image_quota_refresh_plan(&plan, spec);

        assert_eq!(quota_plan.method, "POST");
        assert_eq!(
            quota_plan.url,
            "https://chatgpt.com/backend-api/conversation/init"
        );
        assert_eq!(
            quota_plan.provider_api_format,
            "chatgpt_web:conversation_init"
        );
        assert_eq!(
            quota_plan.headers.get("authorization").map(String::as_str),
            Some("Bearer test-access-token")
        );
        assert_eq!(
            quota_plan
                .transport_profile
                .as_ref()
                .map(|profile| profile.backend.as_str()),
            Some(TRANSPORT_BACKEND_BROWSER_WREQ)
        );
        assert_eq!(
            quota_plan
                .timeouts
                .as_ref()
                .and_then(|timeouts| timeouts.total_ms),
            Some(CHATGPT_WEB_QUOTA_REFRESH_TIMEOUT_MS)
        );
    }

    #[test]
    fn chatgpt_web_image_quota_request_delta_decrements_remaining_count() {
        let mut metadata = Map::from_iter([
            ("image_quota_remaining".to_string(), json!(25.0)),
            ("image_quota_total".to_string(), json!(25.0)),
            ("image_quota_used".to_string(), json!(0.0)),
            ("image_quota_reset_at".to_string(), json!(2_000u64)),
        ]);

        assert!(apply_chatgpt_web_image_quota_request_delta_to_metadata(
            &mut metadata,
            None,
            1_000,
            None,
        ));

        assert_eq!(metadata["image_quota_remaining"], json!(24.0));
        assert_eq!(metadata["image_quota_total"], json!(25.0));
        assert_eq!(metadata["image_quota_used"], json!(1.0));
        assert_eq!(metadata["image_quota_local_request_count"], json!(1u64));
    }

    #[test]
    fn chatgpt_web_image_quota_request_delta_can_use_status_snapshot() {
        let mut metadata = Map::new();
        let status_snapshot = json!({
            "quota": {
                "provider_type": "chatgpt_web",
                "windows": [{
                    "code": "image_gen",
                    "scope": "account",
                    "remaining_value": 19.0,
                    "limit_value": 25.0,
                    "used_value": 6.0,
                    "reset_at": 2_000u64
                }]
            }
        });

        assert!(apply_chatgpt_web_image_quota_request_delta_to_metadata(
            &mut metadata,
            Some(&status_snapshot),
            1_000,
            None,
        ));

        assert_eq!(metadata["image_quota_remaining"], json!(18.0));
        assert_eq!(metadata["image_quota_total"], json!(25.0));
        assert_eq!(metadata["image_quota_used"], json!(7.0));
        assert_eq!(metadata["image_quota_reset_at"], json!(2_000u64));
    }

    #[test]
    fn chatgpt_web_image_quota_request_delta_records_unknown_quota_use() {
        let mut metadata = Map::new();

        assert!(apply_chatgpt_web_image_quota_request_delta_to_metadata(
            &mut metadata,
            None,
            1_000,
            None,
        ));

        assert_eq!(metadata.get("image_quota_remaining"), None);
        assert_eq!(metadata.get("image_quota_total"), None);
        assert_eq!(metadata["image_quota_used"], json!(1.0));
        assert_eq!(metadata["image_quota_local_request_count"], json!(1u64));
        assert_eq!(metadata["updated_at"], json!(1_000u64));
    }

    #[test]
    fn chatgpt_web_image_quota_request_delta_derives_remaining_from_limit_only() {
        let mut metadata = Map::from_iter([("image_quota_total".to_string(), json!(10.0))]);

        assert!(apply_chatgpt_web_image_quota_request_delta_to_metadata(
            &mut metadata,
            None,
            1_000,
            None,
        ));

        assert_eq!(metadata["image_quota_remaining"], json!(9.0));
        assert_eq!(metadata["image_quota_total"], json!(10.0));
        assert_eq!(metadata["image_quota_used"], json!(1.0));
    }

    #[test]
    fn chatgpt_web_image_quota_request_delta_ignores_legacy_free_25_limit() {
        let mut metadata = Map::from_iter([
            ("plan_type".to_string(), json!("free")),
            ("image_quota_remaining".to_string(), json!(19.0)),
            ("image_quota_total".to_string(), json!(25.0)),
            ("image_quota_used".to_string(), json!(6.0)),
        ]);

        assert!(apply_chatgpt_web_image_quota_request_delta_to_metadata(
            &mut metadata,
            None,
            1_000,
            None,
        ));

        assert_eq!(metadata["image_quota_remaining"], json!(18.0));
        assert_eq!(metadata["image_quota_total"], json!(19.0));
        assert_eq!(metadata["image_quota_used"], json!(1.0));
        assert_eq!(
            metadata["image_quota_limit_source"],
            json!("first_remaining")
        );
    }

    #[test]
    fn chatgpt_web_image_quota_request_delta_ignores_legacy_free_25_without_remaining() {
        let mut metadata = Map::from_iter([
            ("plan_type".to_string(), json!("free")),
            ("image_quota_total".to_string(), json!(25.0)),
        ]);

        assert!(apply_chatgpt_web_image_quota_request_delta_to_metadata(
            &mut metadata,
            None,
            1_000,
            None,
        ));

        assert_eq!(metadata.get("image_quota_remaining"), None);
        assert_eq!(metadata.get("image_quota_total"), None);
        assert_eq!(metadata["image_quota_used"], json!(1.0));
        assert_eq!(
            metadata.get("image_quota_limit_source"),
            None,
            "legacy free default should not become a first observed limit without remaining"
        );
    }

    #[test]
    fn chatgpt_web_image_quota_request_delta_dedupes_same_candidate_start() {
        let mut metadata = Map::from_iter([
            ("plan_type".to_string(), json!("free")),
            ("image_quota_remaining".to_string(), json!(25.0)),
            ("image_quota_total".to_string(), json!(25.0)),
            ("image_quota_used".to_string(), json!(0.0)),
        ]);

        assert!(apply_chatgpt_web_image_quota_request_delta_to_metadata(
            &mut metadata,
            None,
            1_000,
            Some("request-1:candidate-1"),
        ));
        assert!(!apply_chatgpt_web_image_quota_request_delta_to_metadata(
            &mut metadata,
            None,
            1_001,
            Some("request-1:candidate-1"),
        ));
        assert!(apply_chatgpt_web_image_quota_request_delta_to_metadata(
            &mut metadata,
            None,
            1_002,
            Some("request-1:candidate-2"),
        ));

        assert_eq!(metadata["image_quota_remaining"], json!(23.0));
        assert_eq!(metadata["image_quota_total"], json!(25.0));
        assert_eq!(metadata["image_quota_used"], json!(2.0));
        assert_eq!(metadata["image_quota_local_request_count"], json!(2u64));
        assert_eq!(
            metadata["image_quota_last_local_request_key"],
            json!("request-1:candidate-2")
        );
    }

    async fn start_mock_chatgpt_web() -> (String, tokio::task::JoinHandle<()>) {
        let app = Router::new().fallback(any(|request: Request| async move {
            let path = request.uri().path().to_string();
            let method = request.method().clone();
            match (method, path.as_str()) {
                (Method::GET, "/") => response(StatusCode::OK, "text/html", "ok"),
                (Method::POST, "/backend-api/sentinel/chat-requirements") => json_response(json!({
                    "token": "requirements-token",
                    "proofofwork": {"required": false},
                    "arkose": {"required": false}
                })),
                (Method::POST, "/backend-api/f/conversation/prepare") => {
                    json_response(json!({"conduit_token": "conduit-token"}))
                }
                (Method::POST, "/backend-api/f/conversation") => response(
                    StatusCode::OK,
                    "text/event-stream",
                    concat!(
                        "data: {\"conversation_id\":\"conv-test-1\"}\n\n",
                        "data: {\"message\":{\"content\":{\"parts\":[\"working\"]}},\"asset\":\"file-generated-123456\"}\n\n",
                        "data: [DONE]\n\n"
                    ),
                ),
                (Method::GET, "/backend-api/files/download/file-generated-123456") => {
                    json_response(json!({"download_url": "/generated.png"}))
                }
                (Method::GET, "/generated.png") => response(
                    StatusCode::OK,
                    "image/png",
                    png_header_bytes(2, 3),
                ),
                _ => response(StatusCode::NOT_FOUND, "text/plain", "not found"),
            }
        }));
        let listener = crate::test_support::bind_loopback_listener()
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("local addr should resolve");
        let handle = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("mock server should run");
        });
        (format!("http://{addr}"), handle)
    }

    async fn start_bootstrap_failing_chatgpt_web() -> (String, tokio::task::JoinHandle<()>) {
        let app = Router::new().fallback(any(|_request: Request| async move {
            response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "text/plain",
                "bootstrap failed",
            )
        }));
        let listener = crate::test_support::bind_loopback_listener()
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("local addr should resolve");
        let handle = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("mock server should run");
        });
        (format!("http://{addr}"), handle)
    }

    fn response(
        status: StatusCode,
        content_type: &'static str,
        body: impl Into<Body>,
    ) -> http::Response<Body> {
        http::Response::builder()
            .status(status)
            .header(http::header::CONTENT_TYPE, content_type)
            .body(body.into())
            .expect("response should build")
    }

    fn json_response(body: Value) -> http::Response<Body> {
        response(
            StatusCode::OK,
            "application/json",
            serde_json::to_vec(&body).expect("json should encode"),
        )
    }

    fn png_header_bytes(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::from(&b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR"[..]);
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    #[test]
    fn parse_web_image_sse_extracts_completed_output_result() {
        let summary = parse_web_image_sse(
            br#"data: {"type":"response.completed","response":{"output":[{"type":"image_generation_call","result":"ZmFrZS1pbWFnZQ==","output_format":"webp"}]}}

data: [DONE]

"#,
        );

        assert_eq!(
            summary.direct_urls,
            vec!["data:image/webp;base64,ZmFrZS1pbWFnZQ=="]
        );
    }

    #[test]
    fn parse_web_image_sse_extracts_partial_image_result() {
        let summary = parse_web_image_sse(
            br#"data: {"type":"response.image_generation_call.partial_image","partial_image_b64":"cGFydGlhbA==","output_format":"jpeg"}

data: [DONE]

"#,
        );

        assert_eq!(
            summary.direct_urls,
            vec!["data:image/jpeg;base64,cGFydGlhbA=="]
        );
    }

    #[test]
    fn parse_web_image_sse_preserves_inline_output_format() {
        let jpeg_payload =
            base64::engine::general_purpose::STANDARD.encode([0xff, 0xd8, 0xff, 0xd9]);
        let event = format!(
            "data: {{\"type\":\"response.output_item.done\",\"item\":{{\"type\":\"image_generation_call\",\"result\":\"{jpeg_payload}\",\"output_format\":\"jpeg\"}}}}\n\n"
        );

        let summary = parse_web_image_sse(event.as_bytes());

        assert_eq!(
            summary.direct_urls,
            vec![format!("data:image/jpeg;base64,{jpeg_payload}")]
        );
    }

    #[test]
    fn parse_web_image_sse_preserves_response_failed_event() {
        let summary = parse_web_image_sse(
            br#"data: {"type":"response.failed","response":{"status":"failed","error":{"code":"rate_limit_exceeded","message":"limited"}}}

data: [DONE]

"#,
        );

        assert_eq!(
            summary
                .failure
                .as_ref()
                .and_then(|value| value.get("type"))
                .and_then(Value::as_str),
            Some("response.failed")
        );
    }

    #[test]
    fn generated_asset_filter_does_not_drop_icon_or_logo_outputs() {
        for accepted in [
            "https://files.oaiusercontent.com/generated/icon-logo-output.png",
            "https://cdn.files.oaiusercontent.com/generated/image.png",
            "https://oaidalleapiprodscus.blob.core.windows.net/generated/image.png",
            "https://tenant.blob.core.windows.net/generated/image.png",
            "https://files.oaiusercontent.com./generated/image.png",
        ] {
            assert!(
                is_generated_web_asset_url(accepted),
                "generated image host should be accepted: {accepted}"
            );
        }
        assert!(!is_generated_web_asset_url(
            "https://openaiassets.blob.core.windows.net/$web/chatgpt/filled-plus-icon.svg"
        ));

        for rejected in [
            "https://notfiles.oaiusercontent.com/generated/image.png",
            "https://files.oaiusercontent.com.attacker.invalid/generated/image.png",
            "https://not-oaidalleapiprodscus.blob.core.windows.net.attacker.invalid/image.png",
            "https://blob.core.windows.net/generated/image.png",
            "https://openaiassets.blob.core.windows.net/generated/image.png",
            "https://sub.openaiassets.blob.core.windows.net/generated/image.png",
            "ftp://files.oaiusercontent.com/generated/image.png",
            "https://user@files.oaiusercontent.com/generated/image.png",
        ] {
            assert!(
                !is_generated_web_asset_url(rejected),
                "lookalike or static asset host should be rejected: {rejected}"
            );
        }
    }

    #[test]
    fn web_opaque_ids_reject_path_and_query_injection() {
        for accepted in ["conv-test_123", "file-generated-123456", "sediment_123"] {
            assert!(web_opaque_id_is_safe(accepted));
        }
        assert!(is_web_file_id("file-generated-123456"));

        for rejected in [
            "../admin",
            "conv/other",
            "conv?inline=true",
            "conv#fragment",
            "conv%2fadmin",
            "conv&inline=true",
            "conv=value",
            "conv value",
            "\r\nX-Injected: true",
        ] {
            assert!(
                !web_opaque_id_is_safe(rejected),
                "unsafe opaque ID should be rejected: {rejected:?}"
            );
        }
        assert!(!web_opaque_id_is_safe(
            "a".repeat(CHATGPT_WEB_OPAQUE_ID_MAX_BYTES + 1).as_str()
        ));
        assert!(!is_web_file_id("file-generated-123456/../../admin"));
        assert!(validated_web_file_id("file-generated-123456?download=1").is_err());
    }

    #[test]
    fn web_image_value_extraction_keeps_only_safe_opaque_ids() {
        let mut summary = WebImageSseSummary::default();
        extract_web_image_values(
            &json!({
                "conversation_id": "conv-test_123",
                "file": "file-generated-123456",
                "sediment": "sediment://sediment_123"
            }),
            &mut summary,
        );
        assert_eq!(summary.conversation_id.as_deref(), Some("conv-test_123"));
        assert_eq!(summary.file_ids, vec!["file-generated-123456"]);
        assert_eq!(summary.sediment_ids, vec!["sediment_123"]);

        let mut malicious = WebImageSseSummary::default();
        extract_web_image_values(
            &json!({
                "conversation_id": "conv-test?inline=true",
                "file": "file-generated-123456/../../admin",
                "sediment": "sediment://sediment_123?download=1"
            }),
            &mut malicious,
        );
        assert!(malicious.conversation_id.is_none());
        assert!(malicious.file_ids.is_empty());
        assert!(malicious.sediment_ids.is_empty());
    }

    #[test]
    fn chatgpt_web_image_url_validation_requires_absolute_http_without_credentials() {
        assert!(parse_absolute_web_image_url("https://cdn.example/image.png").is_ok());

        for rejected in [
            "/relative.png",
            "file:///etc/passwd",
            "ftp://cdn.example/image.png",
            "https://user:password@cdn.example/image.png",
        ] {
            assert!(
                parse_absolute_web_image_url(rejected).is_err(),
                "URL should be rejected: {rejected}"
            );
        }
    }

    #[test]
    fn chatgpt_web_upload_url_is_restricted_to_signed_storage_origins() {
        for accepted in [
            "https://files.oaiusercontent.com/upload/blob?sig=abc&se=123",
            "https://cdn.files.oaiusercontent.com/upload/blob?sig=abc",
            "https://oaidalleapiprodscus.blob.core.windows.net/container/blob?sig=abc",
            "https://tenant.blob.core.windows.net/container/blob?sig=abc",
            "https://tenant.blob.core.windows.net:443/container/blob?sig=abc",
        ] {
            assert!(
                validate_chatgpt_web_upload_url(accepted).is_ok(),
                "valid storage URL should be accepted: {accepted}"
            );
        }

        for rejected in [
            "http://files.oaiusercontent.com/upload/blob?sig=abc",
            "https://127.0.0.1/upload/blob?sig=abc",
            "https://user:pass@files.oaiusercontent.com/upload/blob?sig=abc",
            "https://files.oaiusercontent.com.attacker.invalid/upload/blob?sig=abc",
            "https://attacker.invalid/upload/blob?sig=abc",
            "https://blob.core.windows.net/upload/blob?sig=abc",
            "https://openaiassets.blob.core.windows.net/upload/blob?sig=abc",
            "https://tenant.blob.core.windows.net:8443/upload/blob?sig=abc",
            "https://tenant.blob.core.windows.net/upload/blob?sig=abc#fragment",
        ] {
            assert!(
                validate_chatgpt_web_upload_url(rejected).is_err(),
                "unsafe storage URL should be rejected: {rejected}"
            );
        }

        let oversized = format!(
            "https://files.oaiusercontent.com/upload/blob?sig={}",
            "a".repeat(CHATGPT_WEB_IMAGE_MAX_UPLOAD_URL_BYTES)
        );
        assert!(validate_chatgpt_web_upload_url(&oversized).is_err());
    }

    #[test]
    fn chatgpt_web_image_request_fields_are_bounded() {
        let oversized_prompt = json!({
            "prompt": "x".repeat(CHATGPT_WEB_IMAGE_MAX_PROMPT_BYTES + 1)
        });
        assert!(ChatGptWebImageRequest::from_body(&oversized_prompt).is_err());

        let oversized_images = json!({
            "images": vec!["data:image/png;base64,AA=="; CHATGPT_WEB_IMAGE_MAX_INPUT_IMAGES + 1]
        });
        assert!(ChatGptWebImageRequest::from_body(&oversized_images).is_err());

        let too_many_partial_images = json!({"partial_images": 4});
        assert!(ChatGptWebImageRequest::from_body(&too_many_partial_images).is_err());
    }

    #[test]
    fn chatgpt_web_image_summary_bounds_assets_across_merges() {
        let mut summary = WebImageSseSummary::default();
        for round in 0..32 {
            let mut poll = WebImageSseSummary::default();
            poll.add_values(
                WebImageSummaryCollection::FileId,
                (0..16).map(|index| format!("file-{round}-{index}")),
            );
            poll.add_values(
                WebImageSummaryCollection::SedimentId,
                (0..16).map(|index| format!("sediment-{round}-{index}")),
            );
            poll.add_values(
                WebImageSummaryCollection::DirectUrl,
                (0..16).map(|index| format!("https://files.oaiusercontent.com/{round}/{index}")),
            );
            merge_web_summary(&mut summary, &mut poll);
        }
        assert!(summary.retained_item_count() <= CHATGPT_WEB_IMAGE_SUMMARY_MAX_ITEMS);
        assert!(summary.retained_value_bytes() <= chatgpt_web_image_sse_envelope_limit_bytes());
    }

    #[test]
    fn chatgpt_web_image_data_url_is_bounded_before_formatting() {
        let oversized = "A".repeat(
            maximum_base64_len_for_decoded_limit(chatgpt_web_image_raw_payload_limit_bytes())
                .saturating_add(1),
        );
        assert!(bounded_web_image_data_url("image/png", &oversized).is_none());
        assert!(bounded_web_image_data_url("image/png", "AAAA").is_some());
    }

    #[test]
    fn chatgpt_web_image_same_origin_requires_scheme_host_and_effective_port() {
        let base = url::Url::parse("https://chatgpt.example").expect("base URL should parse");

        for same_origin in [
            "https://chatgpt.example/backend-api/files/download/file-1",
            "https://CHATGPT.example:443/backend-api/files/download/file-1",
        ] {
            let target = url::Url::parse(same_origin).expect("target URL should parse");
            assert!(web_download_url_is_same_origin(&base, &target));
        }

        for cross_origin in [
            "http://chatgpt.example/backend-api/files/download/file-1",
            "https://chatgpt.example:444/backend-api/files/download/file-1",
            "https://cdn.chatgpt.example/backend-api/files/download/file-1",
        ] {
            let target = url::Url::parse(cross_origin).expect("target URL should parse");
            assert!(!web_download_url_is_same_origin(&base, &target));
        }
    }

    #[test]
    fn chatgpt_web_image_authentication_is_limited_to_same_origin_backend_api_paths() {
        let base = url::Url::parse("https://chatgpt.example").expect("base URL should parse");
        let authenticated =
            url::Url::parse("https://chatgpt.example/backend-api/files/download/file-1")
                .expect("authenticated URL should parse");
        assert!(is_authenticated_web_download_url(&base, &authenticated));

        for unauthenticated in [
            "https://chatgpt.example/generated.png",
            "https://chatgpt.example/backend-api-impersonator/image.png",
            "https://cdn.example/backend-api/files/download/file-1",
            "http://chatgpt.example/backend-api/files/download/file-1",
            "https://chatgpt.example:444/backend-api/files/download/file-1",
            "https://user@chatgpt.example/backend-api/files/download/file-1",
        ] {
            let target = url::Url::parse(unauthenticated).expect("target URL should parse");
            assert!(
                !is_authenticated_web_download_url(&base, &target),
                "provider credentials must not be sent to {unauthenticated}"
            );
        }
    }

    #[test]
    fn chatgpt_web_data_url_parser_accepts_only_bounded_supported_image_types() {
        let payload = base64::engine::general_purpose::STANDARD.encode(png_header_bytes(2, 3));
        let png =
            parse_data_url_with_limit(format!("data:image/png;base64,{payload}").as_str(), 64)
                .expect("png data URL should parse");
        assert_eq!(png.mime, "image/png");
        assert_eq!(png.b64_json, payload);

        let jpeg_payload =
            base64::engine::general_purpose::STANDARD.encode([0xff, 0xd8, 0xff, 0xd9]);
        let jpeg = parse_data_url_with_limit(
            format!("DATA:IMAGE/JPEG;BASE64,{jpeg_payload}").as_str(),
            64,
        )
        .expect("jpeg data URL should parse");
        assert_eq!(jpeg.mime, "image/jpeg");

        for rejected in [
            "data:text/html;base64,PGh0bWw+",
            "data:image/svg+xml;base64,PHN2Zz4=",
            "data:image/gif;base64,R0lGODlh",
            "data:image/png;base64,",
            "data:image/png;base64,!!!!",
            "data:image/png;charset=utf-8;base64,aW1hZ2U=",
            "data:image/png;base64,aW1h\nZ2U=",
            "data:image/png;base64,PHN2Zz4=",
        ] {
            assert!(
                parse_data_url_with_limit(rejected, 64).is_none(),
                "unsafe data URL should be rejected: {rejected}"
            );
        }
    }

    #[test]
    fn chatgpt_web_data_url_parser_enforces_decoded_limit_before_allocation() {
        let exact_bytes = png_header_bytes(2, 3);
        let exact_payload = base64::engine::general_purpose::STANDARD.encode(&exact_bytes);
        let exact = parse_data_url_with_limit(
            format!("data:image/png;base64,{exact_payload}").as_str(),
            exact_bytes.len(),
        )
        .expect("payload at the decoded limit should parse");
        assert_eq!(exact.b64_json, exact_payload);

        let exact_len = exact_bytes.len();
        let mut over_bytes = exact_bytes.clone();
        over_bytes.push(0);
        let over_payload = base64::engine::general_purpose::STANDARD.encode(over_bytes);
        assert!(
            parse_data_url_with_limit(
                format!("data:image/png;base64,{over_payload}").as_str(),
                exact_len,
            )
            .is_none(),
            "payload over the decoded limit must be rejected"
        );
    }

    #[test]
    fn chatgpt_web_image_payload_requires_supported_magic_and_matching_mime() {
        let png = png_header_bytes(2, 3);
        assert_eq!(
            validate_web_image_payload(&png, Some("image/png; charset=binary"))
                .expect("valid png should pass"),
            "image/png"
        );
        assert_eq!(
            validate_web_image_payload(&png, Some("application/octet-stream"))
                .expect("octet-stream with a valid signature should pass"),
            "image/png"
        );
        assert!(validate_web_image_payload(&png, Some("image/jpeg")).is_err());
        assert!(validate_web_image_payload(&png, Some("image/svg+xml")).is_err());
        assert!(validate_web_image_payload(b"<svg><script>x</script></svg>", None).is_err());
        assert!(
            validate_web_image_payload(b"<html>not an image</html>", Some("image/png")).is_err()
        );
        assert_eq!(
            validate_web_image_payload(&[0xff, 0xd8, 0xff, 0xd9], Some("image/jpg"))
                .expect("jpeg signature should pass"),
            "image/jpeg"
        );
        assert_eq!(
            validate_web_image_payload(b"RIFF\x04\0\0\0WEBP", None)
                .expect("webp signature should pass"),
            "image/webp"
        );
        assert!(validate_web_image_payload(&[0xff, 0xd8], Some("image/jpeg")).is_err());
    }

    #[test]
    fn chatgpt_web_image_sse_envelope_budget_covers_base64_expansion() {
        let raw_limit = crate::headers::max_internal_buffered_body_bytes();
        let expected_minimum = maximum_base64_len_for_decoded_limit(raw_limit)
            .saturating_add(CHATGPT_WEB_IMAGE_SSE_WRAPPER_OVERHEAD_BYTES)
            .min(CHATGPT_WEB_IMAGE_SSE_HARD_MAX_BYTES);
        assert!(chatgpt_web_image_sse_envelope_limit_bytes() >= expected_minimum);
        assert!(chatgpt_web_image_sse_envelope_limit_bytes() >= raw_limit.min(64 * 1024 * 1024));
    }

    #[test]
    fn chatgpt_web_execution_result_body_decode_is_bounded() {
        let result = ExecutionResult {
            request_id: "req-chatgpt-web-image-test".to_string(),
            candidate_id: None,
            status_code: 200,
            headers: BTreeMap::new(),
            response_observation: None,
            body: Some(ResponseBody {
                json_body: None,
                body_bytes_b64: Some("!!!!".to_string()),
            }),
            telemetry: None,
            error: None,
        };
        assert!(execution_result_bytes(&result).is_err());
        assert!(execution_result_body_bytes_lossy(&result).is_empty());
    }

    #[tokio::test]
    async fn chatgpt_web_public_image_resolution_rejects_private_ip_literals() {
        for private_url in [
            "http://127.0.0.1/image.png",
            "http://169.254.169.254/latest/meta-data",
        ] {
            let url = url::Url::parse(private_url).expect("private URL should parse");
            let error = resolve_public_web_image_addrs(&url, Duration::from_secs(5), false)
                .await
                .expect_err("private address must be rejected");
            assert!(
                error.to_string().contains("private or reserved"),
                "unexpected error for {private_url}: {error}"
            );
        }

        let public_url =
            url::Url::parse("https://8.8.8.8/image.png").expect("public URL should parse");
        let (host, addresses) =
            resolve_public_web_image_addrs(&public_url, Duration::from_secs(5), false)
                .await
                .expect("public IP literal should be accepted");
        assert_eq!(host, "8.8.8.8");
        assert_eq!(addresses, vec!["8.8.8.8:443".parse().unwrap()]);
    }

    #[test]
    fn chatgpt_web_image_fake_ip_exception_is_limited_to_storage_origins() {
        let storage =
            url::Url::parse("https://files.oaiusercontent.com/generated/image.png?sig=test")
                .expect("storage URL should parse");
        let fake = vec!["198.18.75.234:443".parse().unwrap()];
        assert!(validate_public_web_image_addresses(&storage, &fake, true).is_ok());
        assert!(validate_public_web_image_addresses(&storage, &fake, false).is_err());

        let arbitrary = url::Url::parse("https://cdn.example/generated/image.png")
            .expect("arbitrary URL should parse");
        assert!(validate_public_web_image_addresses(&arbitrary, &fake, true).is_err());

        let mixed = vec![
            "198.18.75.234:443".parse().unwrap(),
            "10.0.0.1:443".parse().unwrap(),
        ];
        assert!(validate_public_web_image_addresses(&storage, &mixed, true).is_err());
    }

    #[test]
    fn sha3_512_matches_standard_empty_input_vector() {
        assert_eq!(
            hex(&sha3_512(b"")),
            concat!(
                "a69f73cca23a9ac5c8b567dc185a756e97c982164fe25859e0d1dcc1475c80a",
                "615b2123af1f5f94c11e3e9402c3ac558f500199d95b6d3e301758586281dcd26"
            )
        );
    }

    #[test]
    fn pow_generate_solves_easy_target() {
        let (answer, solved) = pow_generate("seed", "ff", pow_config(CHATGPT_WEB_USER_AGENT));
        assert!(solved);
        assert!(!answer.is_empty());
    }

    #[test]
    fn pow_generate_rejects_difficulty_larger_than_digest() {
        let seed = "seed";
        let (answer, solved) = pow_generate(
            seed,
            "f".repeat(130).as_str(),
            pow_config(CHATGPT_WEB_USER_AGENT),
        );
        assert!(!solved);
        assert_eq!(answer, encode_pow_seed(seed));
    }

    #[tokio::test]
    async fn chatgpt_web_image_executor_downloads_file_id_result_as_openai_image_sse() {
        let (base_url, handle) = start_mock_chatgpt_web().await;
        let state = crate::AppState::new().expect("state should build");
        let plan = sample_plan(
            base_url.as_str(),
            json!({
                "operation": "generate",
                "model": "gpt-image-2",
                "web_model": "gpt-5-5-thinking",
                "prompt": "draw a precise test image",
                "size": "512x512",
                "ratio": "1:1",
                "size_best_effort": true,
                "images": [],
                "count": 1,
                "output_format": "png"
            }),
            false,
        );

        let result = maybe_execute_chatgpt_web_image_sync(
            &state,
            &plan,
            Some(&json!({"chatgpt_web_image": true})),
        )
        .await
        .expect("executor should run")
        .expect("plan should be intercepted");

        assert_eq!(result.status_code, 200);
        assert_eq!(
            result.headers.get("content-type").map(String::as_str),
            Some("text/event-stream")
        );
        let body = String::from_utf8(execution_result_body_bytes_lossy(&result))
            .expect("sse body should be utf8");
        assert!(body.contains("response.output_item.done"));
        assert!(body.contains("\"type\":\"image_generation_call\""));
        assert!(body.contains("\"width\":2"));
        assert!(body.contains("\"height\":3"));
        let expected_output_text =
            base64::engine::general_purpose::STANDARD.encode(png_header_bytes(2, 3));
        assert!(body.contains(&expected_output_text));
        let completed = completed_response_from_sse(body.as_str());
        assert_eq!(completed["usage"]["output_tokens"], json!(1756));
        assert_eq!(
            completed["tool_usage"]["image_gen"]["output_tokens"],
            json!(1756)
        );

        handle.abort();
    }

    #[tokio::test]
    async fn chatgpt_web_image_executor_decrements_quota_after_conversation_start_once() {
        let (base_url, handle) = start_mock_chatgpt_web().await;
        let (state, repository) = state_with_chatgpt_web_key(
            base_url.as_str(),
            json!({
                "chatgpt_web": {
                    "plan_type": "free",
                    "image_quota_remaining": 25.0,
                    "image_quota_total": 25.0,
                    "image_quota_used": 0.0
                }
            }),
        );
        let plan = sample_plan(
            base_url.as_str(),
            json!({
                "operation": "generate",
                "model": "gpt-image-2",
                "web_model": "gpt-5-5-thinking",
                "prompt": "draw a precise test image",
                "size": "512x512",
                "ratio": "1:1",
                "images": [],
                "count": 1,
                "output_format": "png"
            }),
            false,
        );

        let result = maybe_execute_chatgpt_web_image_sync(
            &state,
            &plan,
            Some(&json!({"chatgpt_web_image": true})),
        )
        .await
        .expect("executor should run")
        .expect("plan should be intercepted");

        assert_eq!(result.status_code, 200);
        let metadata = reloaded_chatgpt_web_metadata(repository.as_ref()).await;
        assert_eq!(metadata["image_quota_remaining"], json!(24.0));
        assert_eq!(metadata["image_quota_used"], json!(1.0));
        assert_eq!(metadata["image_quota_local_request_count"], json!(1u64));
        assert_eq!(
            metadata["image_quota_last_local_request_key"],
            json!("req-chatgpt-web-image-test:cand-chatgpt-web-image-test")
        );

        handle.abort();
    }

    #[tokio::test]
    async fn chatgpt_web_image_executor_does_not_decrement_quota_before_conversation_start() {
        let (base_url, handle) = start_bootstrap_failing_chatgpt_web().await;
        let (state, repository) = state_with_chatgpt_web_key(
            base_url.as_str(),
            json!({
                "chatgpt_web": {
                    "plan_type": "free",
                    "image_quota_remaining": 25.0,
                    "image_quota_total": 25.0,
                    "image_quota_used": 0.0
                }
            }),
        );
        let plan = sample_plan(
            base_url.as_str(),
            json!({
                "operation": "generate",
                "model": "gpt-image-2",
                "web_model": "gpt-5-5-thinking",
                "prompt": "draw a precise test image",
                "size": "512x512",
                "ratio": "1:1",
                "images": [],
                "count": 1,
                "output_format": "png"
            }),
            false,
        );

        let result = maybe_execute_chatgpt_web_image_sync(
            &state,
            &plan,
            Some(&json!({"chatgpt_web_image": true})),
        )
        .await
        .expect("executor should preserve the upstream HTTP response")
        .expect("plan should be intercepted");

        assert_eq!(result.status_code, 500);
        assert_eq!(
            execution_result_json(&result).expect("error response should be json")["error"]["code"],
            json!("chatgpt_web_image_execution_unavailable")
        );
        let metadata = reloaded_chatgpt_web_metadata(repository.as_ref()).await;
        assert_eq!(metadata["image_quota_remaining"], json!(25.0));
        assert_eq!(metadata["image_quota_used"], json!(0.0));
        assert_eq!(metadata.get("image_quota_local_request_count"), None);
        assert_eq!(metadata.get("image_quota_last_local_request_key"), None);

        handle.abort();
    }

    #[tokio::test]
    async fn chatgpt_web_image_sync_propagates_network_failure_without_synthetic_503() {
        let listener = crate::test_support::bind_loopback_listener()
            .await
            .expect("listener should bind");
        let base_url = format!(
            "http://{}",
            listener.local_addr().expect("local addr should resolve")
        );
        drop(listener);
        let state = crate::AppState::new().expect("state should build");
        let plan = sample_plan(
            base_url.as_str(),
            json!({"prompt": "draw a small test image"}),
            false,
        );

        let error = maybe_execute_chatgpt_web_image_sync(
            &state,
            &plan,
            Some(&json!({"chatgpt_web_image": true})),
        )
        .await
        .expect_err("connection failure should propagate to the candidate loop");

        assert!(matches!(
            error,
            ExecutionRuntimeTransportError::UpstreamRequest(_)
        ));
    }

    #[tokio::test]
    async fn chatgpt_web_image_stream_propagates_network_failure_without_synthetic_503() {
        let listener = crate::test_support::bind_loopback_listener()
            .await
            .expect("listener should bind");
        let base_url = format!(
            "http://{}",
            listener.local_addr().expect("local addr should resolve")
        );
        drop(listener);
        let state = crate::AppState::new().expect("state should build");
        let plan = sample_plan(
            base_url.as_str(),
            json!({"prompt": "draw a small test image"}),
            true,
        );

        let error = match maybe_execute_chatgpt_web_image_stream(
            &state,
            &plan,
            Some(&json!({"chatgpt_web_image": true})),
        )
        .await
        {
            Err(error) => error,
            Ok(_) => panic!("connection failure should propagate to the candidate loop"),
        };

        assert!(matches!(
            error,
            ExecutionRuntimeTransportError::UpstreamRequest(_)
        ));
    }

    #[tokio::test]
    async fn chatgpt_web_image_stream_path_wraps_success_sse_as_ndjson_frames() {
        let (base_url, handle) = start_mock_chatgpt_web().await;
        let state = crate::AppState::new().expect("state should build");
        let plan = sample_plan(
            base_url.as_str(),
            json!({
                "operation": "generate",
                "model": "gpt-image-2",
                "web_model": "gpt-5-5-thinking",
                "prompt": "draw a streamed test image",
                "size": "1024x1024",
                "ratio": "1:1",
                "images": [],
                "count": 1,
                "output_format": "png"
            }),
            true,
        );

        let stream = maybe_execute_chatgpt_web_image_stream(
            &state,
            &plan,
            Some(&json!({"chatgpt_web_image": true})),
        )
        .await
        .expect("executor should run")
        .expect("plan should be intercepted");
        let chunks = stream
            .frame_stream
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|chunk| chunk.expect("frame should encode"))
            .collect::<Vec<_>>();
        let text = String::from_utf8(
            chunks
                .iter()
                .flat_map(|chunk| chunk.iter().copied())
                .collect::<Vec<_>>(),
        )
        .expect("ndjson should be utf8");
        let decoded_data = text
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter_map(|frame| {
                frame
                    .get("payload")
                    .and_then(|payload| payload.get("chunk_b64"))
                    .and_then(Value::as_str)
                    .and_then(|chunk| base64::engine::general_purpose::STANDARD.decode(chunk).ok())
            })
            .flat_map(|bytes| String::from_utf8(bytes).ok())
            .collect::<String>();

        assert!(text.contains("\"status_code\":200"));
        assert!(decoded_data.contains("response.output_item.done"));
        assert!(decoded_data.contains("\"width\":2"));
        assert!(decoded_data.contains("\"height\":3"));
        assert!(text.contains("\"type\":\"eof\""));
        let eof_frame = text
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .find(|frame| frame.get("type").and_then(Value::as_str) == Some("eof"))
            .expect("eof frame should exist");
        assert_eq!(
            eof_frame
                .get("payload")
                .and_then(|payload| payload.get("summary"))
                .and_then(|summary| summary.get("standardized_usage"))
                .and_then(|usage| usage.get("output_tokens"))
                .and_then(Value::as_i64),
            Some(1756)
        );
        assert_eq!(
            eof_frame
                .get("payload")
                .and_then(|payload| payload.get("summary"))
                .and_then(|summary| summary.get("standardized_usage"))
                .and_then(|usage| usage.get("dimensions"))
                .and_then(|dimensions| dimensions.get("image_count"))
                .and_then(Value::as_u64),
            Some(1)
        );
        assert_eq!(
            eof_frame
                .get("payload")
                .and_then(|payload| payload.get("summary"))
                .and_then(|summary| summary.get("standardized_usage"))
                .and_then(|usage| usage.get("dimensions"))
                .and_then(|dimensions| dimensions.get("image_size"))
                .and_then(Value::as_str),
            Some("1024x1024")
        );

        handle.abort();
    }

    #[tokio::test]
    async fn chatgpt_web_image_executor_returns_embedded_resolution_error_as_400() {
        let state = crate::AppState::new().expect("state should build");
        let plan = sample_plan(
            CHATGPT_WEB_DEFAULT_BASE_URL,
            json!({
                "error": {
                    "message": "ChatGPT-Web 不支持该分辨率",
                    "type": "invalid_request_error",
                    "code": "chatgpt_web_image_unsupported"
                }
            }),
            false,
        );

        let result = maybe_execute_chatgpt_web_image_sync(
            &state,
            &plan,
            Some(&json!({"chatgpt_web_image": true})),
        )
        .await
        .expect("executor should run")
        .expect("plan should be intercepted");

        assert_eq!(result.status_code, 400);
        let body = execution_result_json(&result).expect("error should be json");
        assert_eq!(body["error"]["type"], "invalid_request_error");
        assert_eq!(body["error"]["code"], "chatgpt_web_image_unsupported");
    }

    #[tokio::test]
    async fn chatgpt_web_image_executor_accepts_marked_responses_client_plan() {
        let state = crate::AppState::new().expect("state should build");
        let mut plan = sample_plan(
            CHATGPT_WEB_DEFAULT_BASE_URL,
            json!({
                "error": {
                    "message": "ChatGPT-Web 不支持该分辨率",
                    "type": "invalid_request_error",
                    "code": "chatgpt_web_image_unsupported"
                }
            }),
            false,
        );
        plan.client_api_format = "openai:responses".to_string();

        let result = maybe_execute_chatgpt_web_image_sync(&state, &plan, None)
            .await
            .expect("executor should run")
            .expect("marked image provider plan should be intercepted");

        assert_eq!(result.status_code, 400);
        let body = execution_result_json(&result).expect("error should be json");
        assert_eq!(body["error"]["code"], "chatgpt_web_image_unsupported");
    }

    #[tokio::test]
    async fn chatgpt_web_image_stream_path_wraps_executor_result_as_ndjson_frames() {
        let state = crate::AppState::new().expect("state should build");
        let plan = sample_plan(
            CHATGPT_WEB_DEFAULT_BASE_URL,
            json!({
                "error": {
                    "message": "ChatGPT-Web 不支持该分辨率",
                    "type": "invalid_request_error",
                    "code": "chatgpt_web_image_unsupported"
                }
            }),
            true,
        );

        let stream = maybe_execute_chatgpt_web_image_stream(
            &state,
            &plan,
            Some(&json!({"chatgpt_web_image": true})),
        )
        .await
        .expect("executor should run")
        .expect("plan should be intercepted");
        let chunks = stream
            .frame_stream
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|chunk| chunk.expect("frame should encode"))
            .collect::<Vec<_>>();
        let text = String::from_utf8(
            chunks
                .iter()
                .flat_map(|chunk| chunk.iter().copied())
                .collect::<Vec<_>>(),
        )
        .expect("ndjson should be utf8");

        assert!(text.contains("\"status_code\":400"));
        let decoded_data = text
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter_map(|frame| {
                frame
                    .get("payload")
                    .and_then(|payload| payload.get("chunk_b64"))
                    .and_then(Value::as_str)
                    .and_then(|chunk| base64::engine::general_purpose::STANDARD.decode(chunk).ok())
            })
            .flat_map(|bytes| String::from_utf8(bytes).ok())
            .collect::<String>();
        assert!(decoded_data.contains("chatgpt_web_image_unsupported"));
        assert!(text.contains("\"type\":\"eof\""));
    }
}
