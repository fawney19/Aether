use std::collections::{BTreeMap, BTreeSet};
use std::io::Error as IoError;
use std::time::{Duration, Instant};

use aether_admin::provider::quota::{
    parse_chatgpt_web_conversation_init_response, quota_refresh_success_invalid_state,
};
use aether_contracts::{
    ExecutionPlan, ExecutionResult, ExecutionStreamTerminalSummary, ExecutionTelemetry,
    ExecutionTimeouts, ProxySnapshot, RequestBody, ResolvedTransportProfile, ResponseBody,
    StreamFrame, StreamFramePayload, StreamFrameType,
    EXECUTION_REQUEST_ACCEPT_INVALID_CERTS_HEADER, EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER,
    TRANSPORT_BACKEND_BROWSER_WREQ, TRANSPORT_HTTP_MODE_AUTO, TRANSPORT_POOL_SCOPE_KEY,
};
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
    DirectSyncExecutionRuntime, ExecutionRuntimeTransportError,
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
const GPT_IMAGE2_TOKEN_MIN_PIXELS: u64 = 655_360;
const GPT_IMAGE2_TOKEN_MAX_PIXELS: u64 = 8_294_400;
const GPT_IMAGE2_TOKEN_MAX_EDGE: u64 = 3_840;
const GPT_IMAGE2_TOKEN_MAX_ASPECT_RATIO: u64 = 3;
const GPT_IMAGE2_PARTIAL_IMAGE_OUTPUT_TOKENS: u64 = 100;

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

#[derive(Debug, Clone)]
struct DownloadedImage {
    b64_json: String,
    mime: String,
    width: Option<u32>,
    height: Option<u32>,
}

pub(crate) async fn maybe_execute_chatgpt_web_image_sync(
    state: &AppState,
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
) -> Result<Option<ExecutionResult>, ExecutionRuntimeTransportError> {
    if !is_chatgpt_web_image_plan(plan, report_context) {
        return Ok(None);
    }
    let started_at = Instant::now();
    let result = match execute_chatgpt_web_image(state, plan, report_context, started_at).await {
        Ok(result) => result,
        Err(err) => chatgpt_web_transport_error_execution_result(plan, started_at, &err),
    };
    Ok(Some(result))
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
        Err(err) => chatgpt_web_transport_error_execution_result(plan, started_at, &err),
    };
    Ok(Some(ChatGptWebImageStream {
        frame_stream: execution_result_frame_stream(plan, &result, report_context),
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
        base_url = %base_url,
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

    Ok(bytes_execution_result(
        plan,
        200,
        BTreeMap::from([
            ("cache-control".to_string(), "no-cache".to_string()),
            ("content-type".to_string(), "text/event-stream".to_string()),
        ]),
        body.into_bytes(),
        started_at,
    ))
}

#[derive(Debug, Clone)]
struct ChatGptWebImageRequest {
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
        let text = |key: &str| {
            body.get(key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        };
        let images = body
            .get("images")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        Ok(Self {
            model: text("model").unwrap_or_else(|| "gpt-image-2".to_string()),
            web_model: text("web_model").unwrap_or_else(|| "gpt-5-5-thinking".to_string()),
            prompt: text("prompt").unwrap_or_else(|| "Generate a high quality image.".to_string()),
            size: text("size").unwrap_or_else(|| "1024x1024".to_string()),
            ratio: text("ratio").unwrap_or_else(|| "1:1".to_string()),
            output_format: text("output_format").unwrap_or_else(|| "png".to_string()),
            quality: text("quality"),
            partial_images: json_u64(body.get("partial_images")).unwrap_or(0),
            images,
        })
    }
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
        match web_download_image(state, plan, base_url, fp, token, url.as_str()).await {
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
                    error = %err,
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
    for file_id in &summary.file_ids {
        if uploaded_ids.contains(file_id) || file_id == "file_upload" {
            continue;
        }
        let mut path = format!("/backend-api/files/download/{file_id}");
        if let Some(conversation_id) = summary.conversation_id.as_deref() {
            path.push_str("?conversation_id=");
            path.push_str(conversation_id);
            path.push_str("&inline=false");
        }
        if let Some(url) = web_download_url(plan, base_url, fp, token, path.as_str()).await? {
            add_unique_values(&mut urls, [url]);
        }
    }
    if let Some(conversation_id) = summary.conversation_id.as_deref() {
        for sediment_id in &summary.sediment_ids {
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
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned))
}

async fn web_download_image(
    _state: &AppState,
    plan: &ExecutionPlan,
    base_url: &str,
    fp: &WebFingerprint,
    token: &str,
    raw_url: &str,
) -> Result<DownloadedImage, ExecutionRuntimeTransportError> {
    if let Some(data) = parse_data_url(raw_url) {
        return Ok(data);
    }
    let download_url = if raw_url.starts_with('/') {
        format!("{base_url}{raw_url}")
    } else {
        raw_url.to_string()
    };
    let mut headers = BTreeMap::from([(
        EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER.to_string(),
        "true".to_string(),
    )]);
    if should_use_web_download_headers(base_url, download_url.as_str()) {
        let path = url::Url::parse(download_url.as_str())
            .ok()
            .map(|url| url.path().to_string())
            .filter(|path| !path.is_empty())
            .unwrap_or_else(|| "/".to_string());
        headers.extend(web_base_headers(fp, token, path.as_str()));
        headers.insert(
            "accept".to_string(),
            "image/avif,image/webp,image/apng,image/svg+xml,image/*,*/*;q=0.8".to_string(),
        );
    }
    let result = execute_subrequest(plan, "GET", download_url, headers, None, false).await?;
    ensure_success(&result, "ChatGPT-Web image download")?;
    let data = execution_result_bytes(&result)?;
    if data.is_empty() {
        return Err(ExecutionRuntimeTransportError::UpstreamRequest(
            "ChatGPT-Web image download returned empty body".to_string(),
        ));
    }
    let mime = result
        .headers
        .get("content-type")
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .filter(|value| value.starts_with("image/"))
        .unwrap_or("image/png")
        .to_string();
    let (width, height) = image_dimensions(&data);
    Ok(DownloadedImage {
        b64_json: base64::engine::general_purpose::STANDARD.encode(data),
        mime,
        width,
        height,
    })
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
    let image = web_download_image(state, plan, base_url, fp, token, ref_url).await?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(image.b64_json.as_bytes())
        .map_err(ExecutionRuntimeTransportError::BodyDecode)?;
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
    let file_id = upload_payload
        .get("file_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ExecutionRuntimeTransportError::UpstreamRequest(
                "ChatGPT-Web upload response missing file_id".to_string(),
            )
        })?
        .to_string();
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

    let put_headers = BTreeMap::from([
        ("content-type".to_string(), image.mime.clone()),
        ("x-ms-blob-type".to_string(), "BlockBlob".to_string()),
        ("x-ms-version".to_string(), "2020-04-08".to_string()),
        ("origin".to_string(), base_url.to_string()),
        ("referer".to_string(), format!("{base_url}/")),
        ("user-agent".to_string(), fp.user_agent.to_string()),
    ]);
    let put_result = execute_subrequest(
        plan,
        "PUT",
        upload_url.to_string(),
        put_headers,
        Some(RequestBody {
            json_body: None,
            body_bytes_b64: Some(base64::engine::general_purpose::STANDARD.encode(&bytes)),
            body_ref: None,
        }),
        false,
    )
    .await?;
    ensure_success(&put_result, "ChatGPT-Web upload blob")?;

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
        file_size: bytes.len(),
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
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned)
            })
    }))
}

async fn execute_subrequest(
    plan: &ExecutionPlan,
    method: &str,
    url: String,
    mut headers: BTreeMap<String, String>,
    body: Option<RequestBody>,
    stream: bool,
) -> Result<ExecutionResult, ExecutionRuntimeTransportError> {
    headers.insert(
        EXECUTION_REQUEST_ACCEPT_INVALID_CERTS_HEADER.to_string(),
        "true".to_string(),
    );
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
    let Some(mut latest_key) = state
        .read_provider_catalog_keys_by_ids(&[key_id.to_string()])
        .await
        .map_err(|err| err.into_message())?
        .into_iter()
        .find(|key| key.id == key_id && key.provider_id == provider_id)
    else {
        return Ok(false);
    };

    let mut metadata = latest_key
        .upstream_metadata
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|metadata| metadata.get("chatgpt_web"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    let now_unix_secs = current_unix_secs();
    let request_dedup_key = chatgpt_web_image_quota_request_delta_dedup_key(plan);
    if !apply_chatgpt_web_image_quota_request_delta_to_metadata(
        &mut metadata,
        latest_key.status_snapshot.as_ref(),
        now_unix_secs,
        request_dedup_key.as_deref(),
    ) {
        return Ok(false);
    }

    let updated_upstream_metadata = merge_provider_metadata_object(
        latest_key.upstream_metadata.as_ref(),
        "chatgpt_web",
        Value::Object(metadata),
    );
    latest_key.upstream_metadata = updated_upstream_metadata;
    latest_key.status_snapshot = sync_provider_key_quota_status_snapshot(
        latest_key.status_snapshot.as_ref(),
        "chatgpt_web",
        latest_key.upstream_metadata.as_ref(),
        "image_request_local",
    );
    latest_key.status_snapshot =
        sync_provider_key_oauth_status_snapshot(latest_key.status_snapshot.as_ref(), &latest_key);
    latest_key.updated_at_unix_secs = Some(now_unix_secs);

    Ok(state
        .update_provider_catalog_key_runtime_state(&latest_key)
        .await
        .map_err(|err| err.into_message())?
        .is_some())
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
        .map_err(|err| err.into_message())?
        .into_iter()
        .any(|key| key.id == key_id && key.provider_id == provider_id);
    if !key_available {
        return Ok(false);
    }
    let Some(provider) = state
        .read_provider_catalog_providers_by_ids(&provider_ids)
        .await
        .map_err(|err| err.into_message())?
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
        .map_err(|err| err.to_string())?;
    if result.status_code != 200 {
        let body_excerpt = String::from_utf8_lossy(&execution_result_body_bytes_lossy(&result))
            .chars()
            .take(320)
            .collect::<String>();
        return Err(format!(
            "conversation/init returned {}: {}",
            result.status_code, body_excerpt
        ));
    }

    let body_json = execution_result_json(&result).map_err(|err| err.to_string())?;
    let now_unix_secs = current_unix_secs();
    let Some(mut metadata) =
        parse_chatgpt_web_conversation_init_response(&body_json, now_unix_secs)
    else {
        return Ok(false);
    };

    let Some(latest_key) = state
        .read_provider_catalog_keys_by_ids(&key_ids)
        .await
        .map_err(|err| err.into_message())?
        .into_iter()
        .find(|key| key.id == key_id && key.provider_id == provider_id)
    else {
        return Ok(false);
    };
    normalize_chatgpt_web_image_quota_limit(&mut metadata, latest_key.upstream_metadata.as_ref());

    let mut updated_key = latest_key;
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

    Ok(state
        .update_provider_catalog_key_runtime_state(&updated_key)
        .await
        .map_err(|err| err.into_message())?
        .is_some())
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
        mut headers,
        content_type,
        json_body,
        client_api_format,
        provider_api_format,
        model_name,
        accept_invalid_certs,
    } = spec;
    if accept_invalid_certs {
        headers.insert(
            EXECUTION_REQUEST_ACCEPT_INVALID_CERTS_HEADER.to_string(),
            "true".to_string(),
        );
    }
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
            summary.failure = Some(value.clone());
        }
        if let Some(text) = extract_assistant_text(&value) {
            summary.last_text = Some(text);
        }
        if let Some(result) = value
            .get("item")
            .filter(|item| {
                item.get("type").and_then(Value::as_str) == Some("image_generation_call")
            })
            .and_then(|item| item.get("result"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            add_unique_values(
                &mut summary.direct_urls,
                [format!("data:image/png;base64,{result}")],
            );
        }
        add_unique_values(
            &mut summary.direct_urls,
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
                add_unique_values(&mut urls, [format!("data:{mime};base64,{partial_b64}")]);
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
    Some(format!("data:{mime};base64,{b64}"))
}

fn mime_for_web_output_format(format: &str) -> &'static str {
    match format.trim().to_ascii_lowercase().as_str() {
        "jpeg" | "jpg" => "image/jpeg",
        "webp" => "image/webp",
        _ => "image/png",
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
                        .filter(|value| !value.is_empty())
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
            if text.starts_with("sediment://") {
                add_unique_values(
                    &mut summary.sediment_ids,
                    [text.trim_start_matches("sediment://").to_string()],
                );
            } else if is_web_file_id(text) {
                add_unique_values(&mut summary.file_ids, [text.to_string()]);
            } else if is_generated_web_asset_url(text) || text.starts_with("data:image/") {
                add_unique_values(&mut summary.direct_urls, [text.to_string()]);
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
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn merge_web_summary(target: &mut WebImageSseSummary, source: &mut WebImageSseSummary) {
    if target.conversation_id.is_none() {
        target.conversation_id = source.conversation_id.take();
    }
    add_unique_values(&mut target.file_ids, source.file_ids.drain(..));
    add_unique_values(&mut target.sediment_ids, source.sediment_ids.drain(..));
    add_unique_values(&mut target.direct_urls, source.direct_urls.drain(..));
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
    for value in incoming {
        if !value.is_empty() && !values.iter().any(|existing| existing == &value) {
            values.push(value);
        }
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

fn build_failed_sse(request: &ChatGptWebImageRequest, failure: &Value) -> String {
    let failed = if failure.get("type").and_then(Value::as_str) == Some("response.failed") {
        failure.clone()
    } else {
        json!({
            "type": "response.failed",
            "response": {
                "status": "failed",
                "model": request.model,
                "error": failure.get("error").cloned().unwrap_or_else(|| json!({
                    "code": "chatgpt_web_image_failed",
                    "message": "ChatGPT-Web image generation failed"
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
        body: Some(ResponseBody {
            json_body: Some(body),
            body_bytes_b64: None,
        }),
        telemetry: Some(telemetry(started_at, 0)),
        error: None,
    }
}

fn chatgpt_web_transport_error_execution_result(
    plan: &ExecutionPlan,
    started_at: Instant,
    error: &ExecutionRuntimeTransportError,
) -> ExecutionResult {
    json_execution_result(
        plan,
        503,
        json!({
            "error": {
                "type": "upstream_error",
                "code": "chatgpt_web_image_execution_unavailable",
                "message": error.to_string()
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
) -> ExecutionResult {
    let body_len = body.len() as u64;
    ExecutionResult {
        request_id: plan.request_id.clone(),
        candidate_id: plan.candidate_id.clone(),
        status_code,
        headers,
        body: Some(ResponseBody {
            json_body: None,
            body_bytes_b64: Some(base64::engine::general_purpose::STANDARD.encode(body)),
        }),
        telemetry: Some(telemetry(started_at, body_len)),
        error: None,
    }
}

fn execution_result_frame_stream(
    plan: &ExecutionPlan,
    result: &ExecutionResult,
    report_context: Option<&Value>,
) -> BoxStream<'static, Result<Bytes, IoError>> {
    let body = execution_result_body_bytes_lossy(result);
    let terminal_summary = chatgpt_web_stream_terminal_summary(plan, result, report_context, &body);
    let mut frames = vec![
        StreamFrame {
            frame_type: StreamFrameType::Headers,
            payload: StreamFramePayload::Headers {
                status_code: result.status_code,
                headers: result.headers.clone(),
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
    if !body.is_empty() {
        frames.push(StreamFrame {
            frame_type: StreamFrameType::Data,
            payload: StreamFramePayload::Data {
                chunk_b64: Some(base64::engine::general_purpose::STANDARD.encode(body.as_slice())),
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
    stream::iter(
        frames
            .into_iter()
            .map(|frame| encode_stream_frame_ndjson(&frame)),
    )
    .boxed()
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
        Value::String("generate".to_string()),
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
    Ok(execution_result_body_bytes_lossy(result))
}

fn execution_result_body_bytes_lossy(result: &ExecutionResult) -> Vec<u8> {
    let Some(body) = result.body.as_ref() else {
        return Vec::new();
    };
    if let Some(json_body) = body.json_body.as_ref() {
        return serde_json::to_vec(json_body).unwrap_or_default();
    }
    body.body_bytes_b64
        .as_deref()
        .and_then(|value| base64::engine::general_purpose::STANDARD.decode(value).ok())
        .unwrap_or_default()
}

fn ensure_success(
    result: &ExecutionResult,
    stage: &str,
) -> Result<(), ExecutionRuntimeTransportError> {
    if (200..300).contains(&result.status_code) {
        return Ok(());
    }
    let body = String::from_utf8_lossy(&execution_result_body_bytes_lossy(result)).to_string();
    Err(ExecutionRuntimeTransportError::UpstreamRequest(format!(
        "{stage} returned {}: {}",
        result.status_code,
        body.chars().take(320).collect::<String>()
    )))
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
    if diff_bytes.is_empty() {
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
    let mut hex = value.trim().to_string();
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
    let (header, data) = value.trim().split_once(',')?;
    let mime = header
        .strip_prefix("data:")
        .and_then(|value| value.split(';').next())
        .filter(|value| value.starts_with("image/"))
        .unwrap_or("image/png")
        .to_string();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .ok()?;
    let (width, height) = image_dimensions(&bytes);
    Some(DownloadedImage {
        b64_json: base64::engine::general_purpose::STANDARD.encode(bytes),
        mime,
        width,
        height,
    })
}

fn image_dimensions(bytes: &[u8]) -> (Option<u32>, Option<u32>) {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") && bytes.len() >= 24 {
        let width = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
        let height = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
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
    (value.starts_with("file-") || value.starts_with("file_")) && value.len() >= 10
}

fn is_generated_web_asset_url(raw_url: &str) -> bool {
    let Ok(url) = url::Url::parse(raw_url.trim()) else {
        return false;
    };
    let Some(host) = url.host_str().map(str::to_ascii_lowercase) else {
        return false;
    };
    let path = url.path().to_ascii_lowercase();
    if host.contains("openaiassets.blob.core.windows.net") {
        return false;
    }
    if path.contains("/$web/chatgpt/") {
        return false;
    }
    host.contains("files.oaiusercontent.com")
        || host.contains("oaidalleapiprodscus.blob.core.windows.net")
        || (host.ends_with(".blob.core.windows.net") && !path.contains("/$web/"))
}

fn should_use_web_download_headers(base_url: &str, raw_url: &str) -> bool {
    let Ok(url) = url::Url::parse(raw_url) else {
        return raw_url.starts_with("/backend-api/");
    };
    if url.path().starts_with("/backend-api/") {
        return true;
    }
    let Ok(base) = url::Url::parse(base_url) else {
        return false;
    };
    url.domain() == base.domain()
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
            Some("test-access-token".to_string()),
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
    fn chatgpt_web_success_sse_includes_estimated_image_usage() {
        let request = ChatGptWebImageRequest {
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
                .headers
                .get(EXECUTION_REQUEST_ACCEPT_INVALID_CERTS_HEADER)
                .map(String::as_str),
            Some("true")
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
        assert!(is_generated_web_asset_url(
            "https://files.oaiusercontent.com/generated/icon-logo-output.png"
        ));
        assert!(!is_generated_web_asset_url(
            "https://openaiassets.blob.core.windows.net/$web/chatgpt/filled-plus-icon.svg"
        ));
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
        .expect("executor should run")
        .expect("plan should be intercepted");

        assert_ne!(result.status_code, 200);
        let metadata = reloaded_chatgpt_web_metadata(repository.as_ref()).await;
        assert_eq!(metadata["image_quota_remaining"], json!(25.0));
        assert_eq!(metadata["image_quota_used"], json!(0.0));
        assert_eq!(metadata.get("image_quota_local_request_count"), None);
        assert_eq!(metadata.get("image_quota_last_local_request_key"), None);

        handle.abort();
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
