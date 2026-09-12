use crate::ai_serving::{
    build_core_error_body_for_client_format, normalize_openai_image_quality, LocalCoreSyncErrorKind,
};
use crate::async_task::CancelVideoTaskError;
use crate::control::GatewayControlDecision;
use crate::control::GatewayPublicRequestContext;
use crate::handlers::shared::{
    find_multipart_boundary, find_multipart_boundary_after_crlf, parse_multipart_boundary,
    query_param_value, unix_ms_to_rfc3339, unix_secs_to_rfc3339, MAX_MULTIPART_PARTS,
    MAX_MULTIPART_PART_HEADER_BYTES,
};
use crate::image_capabilities::openai_image_gateway_max_generation_count;
use crate::{AppState, GatewayError};
use aether_data_contracts::repository::gemini_file_mappings::StoredGeminiFileMapping;
use aether_data_contracts::repository::video_tasks::{
    StoredVideoTask, VideoTaskQueryFilter, VideoTaskStatus,
};
use axum::body::{Body, Bytes};
use axum::http::{self, Response};
use axum::response::IntoResponse;
use axum::Json;
use serde_json::{json, Value};

const GEMINI_VIDEO_TASK_NOT_FOUND_DETAIL: &str = "Video task not found";
const GEMINI_FILE_NOT_FOUND_DETAIL: &str = "File not found";
const GEMINI_FILES_DATA_UNAVAILABLE_DETAIL: &str = "Gemini Files data is unavailable";
const AI_PUBLIC_METHOD_NOT_ALLOWED_DETAIL: &str = "Method not allowed";
const AI_PUBLIC_UNAUTHORIZED_DETAIL: &str = "Unauthorized";
const AI_PUBLIC_INTERNAL_ERROR_DETAIL: &str = "Service temporarily unavailable";
const AI_PUBLIC_UPSTREAM_ERROR_DETAIL: &str = "Upstream request failed";
const OPENAI_IMAGE_PROMPT_DETAIL: &str = "图片生成/编辑请求缺少 prompt";
const OPENAI_IMAGE_EDIT_INPUT_DETAIL: &str = "图片编辑请求至少需要 1 张输入图片";
const OPENAI_IMAGE_PARTIAL_IMAGES_DETAIL: &str =
    "partial_images 仅支持 0-3，且必须配合 stream=true";
const OPENAI_IMAGE_STYLE_DETAIL: &str = "当前 Codex 图片反代暂不支持 style 参数";
const OPENAI_IMAGE_RESPONSE_FORMAT_DETAIL: &str = "response_format 仅支持 url 或 b64_json";
const OPENAI_IMAGE_OUTPUT_FORMAT_DETAIL: &str = "output_format 仅支持 png、jpeg 或 webp";
const OPENAI_IMAGE_QUALITY_DETAIL: &str = "quality 仅支持 auto、low、medium、high、standard 或 hd";
const OPENAI_IMAGE_BACKGROUND_DETAIL: &str = "background 仅支持 auto、opaque 或 transparent";
const OPENAI_IMAGE_MODERATION_DETAIL: &str = "moderation 仅支持 auto 或 low";
const OPENAI_IMAGE_INPUT_FIDELITY_DETAIL: &str = "input_fidelity 仅支持 low 或 high";
const OPENAI_IMAGE_OUTPUT_COMPRESSION_DETAIL: &str = "output_compression 必须是 0-100 的整数";
const OPENAI_IMAGE_INVALID_JSON_DETAIL: &str = "图片接口 JSON 请求体无效";
const OPENAI_IMAGE_INVALID_MULTIPART_DETAIL: &str = "图片接口 multipart/form-data 请求体无效";
const OPENAI_EMBEDDING_CONTENT_TYPE_DETAIL: &str =
    "Embedding request content-type must be application/json";
const OPENAI_EMBEDDING_INVALID_JSON_DETAIL: &str = "Embedding request JSON body is invalid";
const OPENAI_EMBEDDING_MODEL_REQUIRED_DETAIL: &str = "Embedding request model is required";
const OPENAI_EMBEDDING_INPUT_REQUIRED_DETAIL: &str = "Embedding request input is required";
const OPENAI_EMBEDDING_CHAT_PAYLOAD_DETAIL: &str =
    "Embedding request must use input, not chat messages";
const OPENAI_EMBEDDING_STREAM_UNSUPPORTED_DETAIL: &str =
    "Embedding requests do not support streaming";
const OPENAI_RERANK_CONTENT_TYPE_DETAIL: &str =
    "Rerank request content-type must be application/json";
const OPENAI_RERANK_INVALID_JSON_DETAIL: &str = "Rerank request JSON body is invalid";
const OPENAI_RERANK_MODEL_REQUIRED_DETAIL: &str = "Rerank request model is required";
const OPENAI_RERANK_QUERY_REQUIRED_DETAIL: &str = "Rerank request query is required";
const OPENAI_RERANK_DOCUMENTS_REQUIRED_DETAIL: &str = "Rerank request documents are required";
const OPENAI_RERANK_TOP_N_DETAIL: &str = "Rerank request top_n must be a positive integer";
const OPENAI_RERANK_CHAT_PAYLOAD_DETAIL: &str =
    "Rerank request must use query/documents, not chat messages";
const OPENAI_RERANK_STREAM_UNSUPPORTED_DETAIL: &str = "Rerank requests do not support streaming";
const CLAUDE_COUNT_TOKENS_BODY_REQUIRED_DETAIL: &str = "Request body is required";
const CLAUDE_COUNT_TOKENS_INVALID_JSON_DETAIL: &str = "Invalid JSON body";
const CLAUDE_COUNT_TOKENS_MODEL_REQUIRED_DETAIL: &str = "model: Field required";
const CLAUDE_COUNT_TOKENS_MESSAGES_REQUIRED_DETAIL: &str = "messages: Field required";
const ANTIGRAVITY_USER_SETTINGS_MISSING_BODY_DETAIL: &str =
    "Antigravity setUserSettings request body is required";
const ANTIGRAVITY_USER_SETTINGS_INVALID_JSON_DETAIL: &str =
    "Antigravity setUserSettings request JSON body is invalid";
const ANTIGRAVITY_USER_SETTINGS_INVALID_DETAIL: &str =
    "Antigravity setUserSettings request must include object userSettings";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OpenAiImageOperation {
    Generate,
    Edit,
}

impl OpenAiImageOperation {
    fn from_path(path: &str) -> Option<Self> {
        match path {
            "/v1/images/generations" => Some(Self::Generate),
            "/v1/images/edits" => Some(Self::Edit),
            _ => None,
        }
    }
}

#[derive(Debug, Default)]
struct OpenAiImageValidationInput {
    model: Option<String>,
    prompt: Option<String>,
    image_count: usize,
    n: Option<u64>,
    stream: bool,
    partial_images: Option<u64>,
    response_format: Option<String>,
    output_format: Option<String>,
    quality: Option<String>,
    background: Option<String>,
    moderation: Option<String>,
    input_fidelity: Option<String>,
    output_compression: Option<u64>,
    style_present: bool,
}

pub(crate) fn ai_public_local_requires_buffered_body(
    request_context: &GatewayPublicRequestContext,
) -> bool {
    request_context
        .control_decision
        .as_ref()
        .is_some_and(|decision| {
            decision.route_class.as_deref() == Some("ai_public")
                && request_context.request_method == http::Method::POST
                && ((decision.route_family.as_deref() == Some("claude")
                    && decision.route_kind.as_deref() == Some("count_tokens"))
                    || (decision.route_family.as_deref() == Some("openai")
                        && decision.route_kind.as_deref() == Some("embedding")
                        && request_context.request_path == "/v1/embeddings")
                    || (decision.route_family.as_deref() == Some("openai")
                        && decision.route_kind.as_deref() == Some("rerank")
                        && request_context.request_path == "/v1/rerank")
                    || (decision.route_family.as_deref() == Some("antigravity")
                        && decision.route_kind.as_deref() != Some("stream_generate_content")))
        })
}

pub(crate) async fn maybe_build_local_ai_public_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    request_body: Option<&Bytes>,
) -> Option<Response<Body>> {
    if let Some(response) = maybe_build_local_ai_public_route_guard_response(request_context) {
        return Some(response);
    }

    let decision = request_context.control_decision.as_ref()?;
    if decision.route_class.as_deref() != Some("ai_public") {
        return None;
    }

    if let Some(response) =
        maybe_build_local_openai_request_validation_response(request_context, request_body)
    {
        return Some(response);
    }

    if let Some(response) =
        maybe_build_local_claude_count_tokens_validation_response(request_context, request_body)
    {
        return Some(response);
    }

    if let Some(response) =
        maybe_build_local_antigravity_v1internal_response(request_context, request_body)
    {
        return Some(response);
    }

    if let Some(response) =
        maybe_build_local_gemini_files_response(state, request_context, decision).await
    {
        return Some(response);
    }

    maybe_build_local_gemini_video_operations_response(state, request_context, decision).await
}

async fn maybe_build_local_gemini_files_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    decision: &GatewayControlDecision,
) -> Option<Response<Body>> {
    if decision.route_family.as_deref() != Some("gemini")
        || decision.route_kind.as_deref() != Some("files")
        || !request_context.request_path.starts_with("/v1beta/files")
    {
        return None;
    }

    let Some(user_id) = allowed_ai_public_user_id(decision) else {
        return Some(build_ai_public_error_response(
            http::StatusCode::NOT_FOUND,
            GEMINI_FILE_NOT_FOUND_DETAIL,
        ));
    };

    if request_context.request_path == "/v1beta/files" {
        return Some(match request_context.request_method {
            http::Method::GET => {
                build_local_gemini_files_list_response(state, request_context, user_id).await
            }
            _ => build_ai_public_error_response(
                http::StatusCode::METHOD_NOT_ALLOWED,
                AI_PUBLIC_METHOD_NOT_ALLOWED_DETAIL,
            ),
        });
    }

    if !matches!(
        request_context.request_method,
        http::Method::GET | http::Method::DELETE
    ) {
        return Some(build_ai_public_error_response(
            http::StatusCode::METHOD_NOT_ALLOWED,
            AI_PUBLIC_METHOD_NOT_ALLOWED_DETAIL,
        ));
    }

    let file_name = normalize_gemini_file_request_path(request_context.request_path.as_str());
    if let Some(short_id) = file_name
        .as_deref()
        .and_then(|value| value.strip_prefix("files/"))
        .and_then(|file_id| file_id.strip_prefix("aev_"))
        .filter(|value| !value.is_empty())
    {
        return Some(
            build_local_gemini_video_file_response(
                state,
                request_context.request_method.clone(),
                user_id,
                short_id,
            )
            .await,
        );
    }

    if !state.has_gemini_file_mapping_data_reader() {
        return Some(build_ai_public_error_response(
            http::StatusCode::SERVICE_UNAVAILABLE,
            GEMINI_FILES_DATA_UNAVAILABLE_DETAIL,
        ));
    }
    let Some(file_name) = file_name else {
        return Some(build_ai_public_error_response(
            http::StatusCode::NOT_FOUND,
            GEMINI_FILE_NOT_FOUND_DETAIL,
        ));
    };
    let mapping = match state
        .find_active_gemini_file_mapping_for_user(
            file_name.as_str(),
            user_id,
            crate::clock::current_unix_secs(),
        )
        .await
    {
        Ok(Some(mapping)) => mapping,
        Ok(None) => {
            return Some(build_ai_public_error_response(
                http::StatusCode::NOT_FOUND,
                GEMINI_FILE_NOT_FOUND_DETAIL,
            ));
        }
        Err(_) => {
            return Some(build_ai_public_error_response(
                http::StatusCode::SERVICE_UNAVAILABLE,
                GEMINI_FILES_DATA_UNAVAILABLE_DETAIL,
            ));
        }
    };
    if mapping.user_id.as_deref().map(str::trim) != Some(user_id) {
        return Some(build_ai_public_error_response(
            http::StatusCode::NOT_FOUND,
            GEMINI_FILE_NOT_FOUND_DETAIL,
        ));
    }

    None
}

fn allowed_ai_public_user_id(decision: &GatewayControlDecision) -> Option<&str> {
    decision
        .auth_context
        .as_ref()
        .filter(|auth_context| auth_context.access_allowed)
        .map(|auth_context| auth_context.user_id.trim())
        .filter(|value| !value.is_empty())
}

async fn build_local_gemini_video_file_response(
    state: &AppState,
    method: http::Method,
    user_id: &str,
    short_id: &str,
) -> Response<Body> {
    if method != http::Method::GET {
        return build_ai_public_error_response(
            http::StatusCode::METHOD_NOT_ALLOWED,
            AI_PUBLIC_METHOD_NOT_ALLOWED_DETAIL,
        );
    }

    let task = match state
        .find_video_task_by_short_id_for_user(short_id, user_id)
        .await
    {
        Ok(Some(task)) if is_gemini_video_task(&task) => task,
        Ok(_) => {
            return build_ai_public_error_response(
                http::StatusCode::NOT_FOUND,
                GEMINI_FILE_NOT_FOUND_DETAIL,
            );
        }
        Err(_) => {
            return build_ai_public_internal_error_response(
                "gemini_video_file_lookup",
                "data_store_unavailable",
            );
        }
    };

    let source = match crate::async_task::video_task_video_source_from_task(state, &task).await {
        Ok(Some(source)) => source,
        Ok(None) => {
            return build_ai_public_error_response(
                http::StatusCode::NOT_FOUND,
                GEMINI_FILE_NOT_FOUND_DETAIL,
            );
        }
        Err(_) => {
            return build_ai_public_internal_error_response(
                "gemini_video_file_source",
                "video_source_unavailable",
            );
        }
    };

    match crate::async_task::build_video_task_video_response(state, &task.id, source).await {
        Ok(response) => response,
        Err(_) => build_ai_public_internal_error_response(
            "gemini_video_file_delivery",
            "video_delivery_failed",
        ),
    }
}

async fn build_local_gemini_files_list_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    user_id: &str,
) -> Response<Body> {
    if !state.has_gemini_file_mapping_data_reader() {
        return build_ai_public_error_response(
            http::StatusCode::SERVICE_UNAVAILABLE,
            GEMINI_FILES_DATA_UNAVAILABLE_DETAIL,
        );
    }
    let page_size = query_param_value(request_context.request_query_string.as_deref(), "pageSize")
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(10)
        .min(100);
    let offset = query_param_value(request_context.request_query_string.as_deref(), "pageToken")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let mappings = match state
        .list_gemini_file_mappings(
            &aether_data::repository::gemini_file_mappings::GeminiFileMappingListQuery {
                user_id: Some(user_id.to_string()),
                include_expired: false,
                search: None,
                offset,
                limit: page_size,
                now_unix_secs: crate::clock::current_unix_secs(),
            },
        )
        .await
    {
        Ok(mappings) => mappings,
        Err(_) => {
            return build_ai_public_error_response(
                http::StatusCode::SERVICE_UNAVAILABLE,
                GEMINI_FILES_DATA_UNAVAILABLE_DETAIL,
            );
        }
    };
    let files = mappings
        .items
        .iter()
        .map(build_gemini_file_mapping_payload)
        .collect::<Vec<_>>();
    let next_offset = offset.saturating_add(files.len());
    let mut payload = json!({ "files": files });
    if next_offset < mappings.total {
        payload["nextPageToken"] = Value::String(next_offset.to_string());
    }
    Json(payload).into_response()
}

fn build_gemini_file_mapping_payload(mapping: &StoredGeminiFileMapping) -> Value {
    let mut payload = serde_json::Map::new();
    payload.insert("name".to_string(), Value::String(mapping.file_name.clone()));
    if let Some(display_name) = mapping.display_name.as_ref() {
        payload.insert(
            "displayName".to_string(),
            Value::String(display_name.clone()),
        );
    }
    if let Some(mime_type) = mapping.mime_type.as_ref() {
        payload.insert("mimeType".to_string(), Value::String(mime_type.clone()));
    }
    if let Some(created_at) = unix_ms_to_rfc3339(mapping.created_at_unix_ms) {
        payload.insert("createTime".to_string(), Value::String(created_at));
    }
    if let Some(expires_at) = unix_secs_to_rfc3339(mapping.expires_at_unix_secs) {
        payload.insert("expirationTime".to_string(), Value::String(expires_at));
    }
    payload.insert("state".to_string(), Value::String("ACTIVE".to_string()));
    Value::Object(payload)
}

fn normalize_gemini_file_request_path(path: &str) -> Option<String> {
    let suffix = path.strip_prefix("/v1beta/files/")?.trim_matches('/');
    let suffix = suffix.strip_suffix(":download").unwrap_or(suffix).trim();
    let suffix = suffix.strip_prefix("files/").unwrap_or(suffix).trim();
    if suffix.is_empty() || suffix.contains('/') {
        return None;
    }
    Some(format!("files/{suffix}"))
}

fn maybe_build_local_openai_request_validation_response(
    request_context: &GatewayPublicRequestContext,
    request_body: Option<&Bytes>,
) -> Option<Response<Body>> {
    let decision = request_context.control_decision.as_ref()?;
    if decision.route_family.as_deref() != Some("openai")
        || request_context.request_method != http::Method::POST
    {
        return None;
    }

    if decision.route_kind.as_deref() == Some("chat")
        && request_context.request_path == "/v1/chat/completions"
    {
        return None;
    }

    if decision.route_kind.as_deref() == Some("embedding")
        && request_context.request_path == "/v1/embeddings"
    {
        let Some(request_body) = request_body else {
            return Some(build_ai_public_error_response(
                http::StatusCode::BAD_REQUEST,
                OPENAI_EMBEDDING_INVALID_JSON_DETAIL,
            ));
        };
        if let Err(detail) = validate_openai_embedding_request(
            request_context.request_content_type.as_deref(),
            request_body,
        ) {
            return Some(build_ai_public_error_response(
                http::StatusCode::BAD_REQUEST,
                detail,
            ));
        }
        return None;
    }

    if decision.route_kind.as_deref() == Some("rerank")
        && request_context.request_path == "/v1/rerank"
    {
        let Some(request_body) = request_body else {
            return Some(build_ai_public_error_response(
                http::StatusCode::BAD_REQUEST,
                OPENAI_RERANK_INVALID_JSON_DETAIL,
            ));
        };
        if let Err(detail) = validate_openai_rerank_request(
            request_context.request_content_type.as_deref(),
            request_body,
        ) {
            return Some(build_ai_public_error_response(
                http::StatusCode::BAD_REQUEST,
                detail,
            ));
        }
        return None;
    }

    let request_body = request_body?;

    if decision.route_kind.as_deref() != Some("image")
        || !matches!(
            request_context.request_path.as_str(),
            "/v1/images/generations" | "/v1/images/edits"
        )
    {
        return None;
    }

    let Some(operation) = OpenAiImageOperation::from_path(&request_context.request_path) else {
        return None;
    };
    let validation = match parse_openai_image_validation_input(
        operation,
        request_context.request_content_type.as_deref(),
        request_body,
    ) {
        Ok(validation) => validation,
        Err(detail) => {
            return Some(build_ai_public_error_response(
                http::StatusCode::BAD_REQUEST,
                detail,
            ));
        }
    };

    match operation {
        OpenAiImageOperation::Generate | OpenAiImageOperation::Edit
            if validation.prompt.is_none() =>
        {
            return Some(build_ai_public_error_response(
                http::StatusCode::BAD_REQUEST,
                OPENAI_IMAGE_PROMPT_DETAIL,
            ));
        }
        OpenAiImageOperation::Edit if validation.image_count == 0 => {
            return Some(build_ai_public_error_response(
                http::StatusCode::BAD_REQUEST,
                OPENAI_IMAGE_EDIT_INPUT_DETAIL,
            ));
        }
        _ => {}
    }

    if let Some(detail) = validate_openai_image_n(&validation) {
        return Some(build_ai_public_error_response(
            http::StatusCode::BAD_REQUEST,
            detail,
        ));
    }

    if validation.partial_images.is_some_and(|value| value > 3)
        || (validation.partial_images.is_some() && !validation.stream)
    {
        return Some(build_ai_public_error_response(
            http::StatusCode::BAD_REQUEST,
            OPENAI_IMAGE_PARTIAL_IMAGES_DETAIL,
        ));
    }

    if validation.style_present {
        return Some(build_ai_public_error_response(
            http::StatusCode::BAD_REQUEST,
            OPENAI_IMAGE_STYLE_DETAIL,
        ));
    }

    if validation
        .response_format
        .as_deref()
        .is_some_and(|value| !matches!(value, "url" | "b64_json"))
    {
        return Some(build_ai_public_error_response(
            http::StatusCode::BAD_REQUEST,
            OPENAI_IMAGE_RESPONSE_FORMAT_DETAIL,
        ));
    }

    if validation
        .output_format
        .as_deref()
        .is_some_and(|value| !matches!(value, "png" | "jpeg" | "jpg" | "webp"))
    {
        return Some(build_ai_public_error_response(
            http::StatusCode::BAD_REQUEST,
            OPENAI_IMAGE_OUTPUT_FORMAT_DETAIL,
        ));
    }

    if validation
        .quality
        .as_deref()
        .is_some_and(|value| normalize_openai_image_quality(value).is_none())
    {
        return Some(build_ai_public_error_response(
            http::StatusCode::BAD_REQUEST,
            OPENAI_IMAGE_QUALITY_DETAIL,
        ));
    }

    if validation
        .background
        .as_deref()
        .is_some_and(|value| !matches!(value, "auto" | "opaque" | "transparent"))
    {
        return Some(build_ai_public_error_response(
            http::StatusCode::BAD_REQUEST,
            OPENAI_IMAGE_BACKGROUND_DETAIL,
        ));
    }

    if validation
        .moderation
        .as_deref()
        .is_some_and(|value| !matches!(value, "auto" | "low"))
    {
        return Some(build_ai_public_error_response(
            http::StatusCode::BAD_REQUEST,
            OPENAI_IMAGE_MODERATION_DETAIL,
        ));
    }

    if validation
        .input_fidelity
        .as_deref()
        .is_some_and(|value| !matches!(value, "low" | "high"))
    {
        return Some(build_ai_public_error_response(
            http::StatusCode::BAD_REQUEST,
            OPENAI_IMAGE_INPUT_FIDELITY_DETAIL,
        ));
    }

    if validation
        .output_compression
        .is_some_and(|value| value > 100)
    {
        return Some(build_ai_public_error_response(
            http::StatusCode::BAD_REQUEST,
            OPENAI_IMAGE_OUTPUT_COMPRESSION_DETAIL,
        ));
    }

    None
}

fn openai_image_n_detail(max_generation_count: u64) -> String {
    if max_generation_count >= openai_image_gateway_max_generation_count() {
        format!("当前图片反代仅支持 n=1..{max_generation_count}")
    } else {
        format!("当前图片模型仅支持 n=1..{max_generation_count}")
    }
}

fn validate_openai_image_n(validation: &OpenAiImageValidationInput) -> Option<String> {
    let max_generation_count = openai_image_gateway_max_generation_count();
    validation
        .n
        .is_some_and(|value| value == 0 || value > max_generation_count)
        .then(|| openai_image_n_detail(max_generation_count))
}

fn validate_openai_embedding_request(
    content_type: Option<&str>,
    request_body: &Bytes,
) -> Result<(), &'static str> {
    if !content_type
        .unwrap_or_default()
        .to_ascii_lowercase()
        .contains("application/json")
    {
        return Err(OPENAI_EMBEDDING_CONTENT_TYPE_DETAIL);
    }
    if request_body.is_empty() {
        return Err(OPENAI_EMBEDDING_INVALID_JSON_DETAIL);
    }
    let payload = serde_json::from_slice::<Value>(request_body)
        .map_err(|_| OPENAI_EMBEDDING_INVALID_JSON_DETAIL)?;
    let object = payload
        .as_object()
        .ok_or(OPENAI_EMBEDDING_INVALID_JSON_DETAIL)?;
    if object.contains_key("messages") {
        return Err(OPENAI_EMBEDDING_CHAT_PAYLOAD_DETAIL);
    }
    if object
        .get("stream")
        .and_then(value_as_bool)
        .unwrap_or(false)
    {
        return Err(OPENAI_EMBEDDING_STREAM_UNSUPPORTED_DETAIL);
    }
    if object
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_none()
    {
        return Err(OPENAI_EMBEDDING_MODEL_REQUIRED_DETAIL);
    }
    let Some(input) = object.get("input") else {
        return Err(OPENAI_EMBEDDING_INPUT_REQUIRED_DETAIL);
    };
    if !embedding_input_is_non_empty(input) {
        return Err(OPENAI_EMBEDDING_INPUT_REQUIRED_DETAIL);
    }
    Ok(())
}

fn validate_openai_rerank_request(
    content_type: Option<&str>,
    request_body: &Bytes,
) -> Result<(), &'static str> {
    if !content_type
        .unwrap_or_default()
        .to_ascii_lowercase()
        .contains("application/json")
    {
        return Err(OPENAI_RERANK_CONTENT_TYPE_DETAIL);
    }
    if request_body.is_empty() {
        return Err(OPENAI_RERANK_INVALID_JSON_DETAIL);
    }
    let payload = serde_json::from_slice::<Value>(request_body)
        .map_err(|_| OPENAI_RERANK_INVALID_JSON_DETAIL)?;
    let object = payload
        .as_object()
        .ok_or(OPENAI_RERANK_INVALID_JSON_DETAIL)?;
    if object.contains_key("messages") {
        return Err(OPENAI_RERANK_CHAT_PAYLOAD_DETAIL);
    }
    if object
        .get("stream")
        .and_then(value_as_bool)
        .unwrap_or(false)
    {
        return Err(OPENAI_RERANK_STREAM_UNSUPPORTED_DETAIL);
    }
    if object
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_none()
    {
        return Err(OPENAI_RERANK_MODEL_REQUIRED_DETAIL);
    }
    if object
        .get("query")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_none()
    {
        return Err(OPENAI_RERANK_QUERY_REQUIRED_DETAIL);
    }
    let Some(documents) = object.get("documents").and_then(Value::as_array) else {
        return Err(OPENAI_RERANK_DOCUMENTS_REQUIRED_DETAIL);
    };
    if documents.is_empty() || documents.iter().any(rerank_document_is_empty) {
        return Err(OPENAI_RERANK_DOCUMENTS_REQUIRED_DETAIL);
    }
    if object
        .get("top_n")
        .or_else(|| object.get("topN"))
        .is_some_and(|value| !positive_json_integer(value))
    {
        return Err(OPENAI_RERANK_TOP_N_DETAIL);
    }
    Ok(())
}

fn rerank_document_is_empty(value: &Value) -> bool {
    match value {
        Value::String(text) => text.trim().is_empty(),
        Value::Object(object) => object
            .get("text")
            .and_then(Value::as_str)
            .is_some_and(|text| text.trim().is_empty()),
        Value::Null => true,
        _ => false,
    }
}

fn positive_json_integer(value: &Value) -> bool {
    value.as_u64().is_some_and(|number| number > 0)
        || value.as_i64().is_some_and(|number| number > 0)
        || value
            .as_str()
            .and_then(|text| text.trim().parse::<u64>().ok())
            .is_some_and(|number| number > 0)
}

fn embedding_input_is_non_empty(value: &Value) -> bool {
    match value {
        Value::String(text) => !text.trim().is_empty(),
        Value::Array(items) if !items.is_empty() => embedding_array_input_is_non_empty(items),
        _ => false,
    }
}

fn embedding_array_input_is_non_empty(items: &[Value]) -> bool {
    items
        .iter()
        .all(|item| item.as_str().is_some_and(|text| !text.trim().is_empty()))
        || embedding_token_array_is_non_empty(items)
        || items.iter().all(|item| {
            item.as_array()
                .is_some_and(|items| embedding_token_array_is_non_empty(items))
        })
        || items.iter().all(embedding_multimodal_content_is_non_empty)
}

fn embedding_token_array_is_non_empty(items: &[Value]) -> bool {
    !items.is_empty() && items.iter().all(|item| item.as_u64().is_some())
}

fn embedding_multimodal_content_is_non_empty(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    let valid_text = object
        .get("text")
        .map(|value| value.as_str().is_some_and(|text| !text.trim().is_empty()));
    let valid_image = object
        .get("image")
        .map(|value| value.as_str().is_some_and(|image| !image.trim().is_empty()));
    let valid_video = object
        .get("video")
        .map(|value| value.as_str().is_some_and(|video| !video.trim().is_empty()));
    let valid_multi_images = object.get("multi_images").map(|value| {
        value.as_array().is_some_and(|items| {
            !items.is_empty()
                && items
                    .iter()
                    .all(|item| item.as_str().is_some_and(|image| !image.trim().is_empty()))
        })
    });

    [valid_text, valid_image, valid_video, valid_multi_images]
        .into_iter()
        .flatten()
        .all(|valid| valid)
        && [valid_text, valid_image, valid_video, valid_multi_images]
            .into_iter()
            .flatten()
            .any(|valid| valid)
}

fn image_request_count(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|number| u64::try_from(number).ok()))
        .or_else(|| {
            value
                .as_str()
                .and_then(|text| text.trim().parse::<u64>().ok())
        })
}

fn parse_openai_image_validation_input(
    operation: OpenAiImageOperation,
    content_type: Option<&str>,
    request_body: &Bytes,
) -> Result<OpenAiImageValidationInput, &'static str> {
    if request_body.is_empty() {
        return Err(match operation {
            OpenAiImageOperation::Generate | OpenAiImageOperation::Edit => {
                OPENAI_IMAGE_PROMPT_DETAIL
            }
        });
    }

    let content_type = content_type.unwrap_or_default();
    if content_type
        .to_ascii_lowercase()
        .contains("multipart/form-data")
    {
        parse_openai_image_validation_input_from_multipart(request_body, content_type)
    } else {
        parse_openai_image_validation_input_from_json(request_body)
    }
}

fn parse_openai_image_validation_input_from_json(
    request_body: &Bytes,
) -> Result<OpenAiImageValidationInput, &'static str> {
    let payload = serde_json::from_slice::<Value>(request_body)
        .map_err(|_| OPENAI_IMAGE_INVALID_JSON_DETAIL)?;
    let object = payload
        .as_object()
        .ok_or(OPENAI_IMAGE_INVALID_JSON_DETAIL)?;

    Ok(OpenAiImageValidationInput {
        model: normalize_openai_image_model_for_operation(
            object.get("model").and_then(Value::as_str),
        ),
        prompt: object
            .get("prompt")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        image_count: count_json_images(object),
        n: object.get("n").and_then(image_request_count),
        stream: object
            .get("stream")
            .and_then(value_as_bool)
            .unwrap_or(false),
        partial_images: object.get("partial_images").and_then(image_request_count),
        response_format: object
            .get("response_format")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_ascii_lowercase()),
        output_format: object
            .get("output_format")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_ascii_lowercase()),
        quality: object
            .get("quality")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_ascii_lowercase()),
        background: object
            .get("background")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_ascii_lowercase()),
        moderation: object
            .get("moderation")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_ascii_lowercase()),
        input_fidelity: object
            .get("input_fidelity")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_ascii_lowercase()),
        output_compression: object
            .get("output_compression")
            .and_then(image_request_count),
        style_present: object
            .get("style")
            .and_then(Value::as_str)
            .map(str::trim)
            .is_some_and(|value| !value.is_empty()),
    })
}

fn parse_openai_image_validation_input_from_multipart(
    request_body: &Bytes,
    content_type: &str,
) -> Result<OpenAiImageValidationInput, &'static str> {
    let boundary =
        parse_multipart_boundary(content_type).ok_or(OPENAI_IMAGE_INVALID_MULTIPART_DETAIL)?;
    let fields = parse_multipart_fields(request_body, &boundary);
    if fields.is_empty() {
        return Err(OPENAI_IMAGE_INVALID_MULTIPART_DETAIL);
    }

    let model = fields
        .iter()
        .find(|field| field.name.trim() == "model")
        .map(|field| String::from_utf8_lossy(&field.data).trim().to_string());

    Ok(OpenAiImageValidationInput {
        model: normalize_openai_image_model_for_operation(model.as_deref()),
        prompt: multipart_text_field(&fields, "prompt"),
        image_count: fields
            .iter()
            .filter(|field| {
                matches!(
                    field.name.trim(),
                    "image" | "image[]" | "images" | "images[]"
                )
            })
            .count(),
        n: multipart_text_field(&fields, "n").and_then(|value| value.trim().parse::<u64>().ok()),
        stream: multipart_text_field(&fields, "stream")
            .and_then(|value| parse_bool_string(&value))
            .unwrap_or(false),
        partial_images: multipart_text_field(&fields, "partial_images")
            .and_then(|value| value.trim().parse::<u64>().ok()),
        response_format: multipart_text_field(&fields, "response_format")
            .map(|value| value.to_ascii_lowercase()),
        output_format: multipart_text_field(&fields, "output_format")
            .map(|value| value.to_ascii_lowercase()),
        quality: multipart_text_field(&fields, "quality").map(|value| value.to_ascii_lowercase()),
        background: multipart_text_field(&fields, "background")
            .map(|value| value.to_ascii_lowercase()),
        moderation: multipart_text_field(&fields, "moderation")
            .map(|value| value.to_ascii_lowercase()),
        input_fidelity: multipart_text_field(&fields, "input_fidelity")
            .map(|value| value.to_ascii_lowercase()),
        output_compression: multipart_text_field(&fields, "output_compression")
            .and_then(|value| value.trim().parse::<u64>().ok()),
        style_present: multipart_text_field(&fields, "style").is_some(),
    })
}

fn normalize_openai_image_model_for_operation(model: Option<&str>) -> Option<String> {
    model
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn count_json_images(object: &serde_json::Map<String, Value>) -> usize {
    let mut count = 0usize;
    if let Some(value) = object.get("image") {
        count += json_image_count(value);
    }
    if let Some(values) = object.get("images").and_then(Value::as_array) {
        count += values.iter().map(json_image_count).sum::<usize>();
    }
    count
}

fn json_image_count(value: &Value) -> usize {
    match value {
        Value::Array(values) => values.iter().map(json_image_count).sum(),
        Value::String(text) => (!text.trim().is_empty()) as usize,
        Value::Object(_) => 1,
        _ => 0,
    }
}

fn value_as_bool(value: &Value) -> Option<bool> {
    value
        .as_bool()
        .or_else(|| value.as_str().and_then(parse_bool_string))
}

fn parse_bool_string(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" => Some(true),
        "false" | "0" | "no" => Some(false),
        _ => None,
    }
}

#[derive(Debug)]
struct MultipartField {
    name: String,
    data: Vec<u8>,
}

fn multipart_text_field(fields: &[MultipartField], name: &str) -> Option<String> {
    fields
        .iter()
        .find(|field| field.name.trim() == name)
        .map(|field| String::from_utf8_lossy(&field.data).trim().to_string())
        .filter(|value| !value.is_empty())
}

fn parse_multipart_fields(body: &[u8], boundary: &str) -> Vec<MultipartField> {
    let delimiter = format!("--{boundary}").into_bytes();
    let mut parts = Vec::new();
    let mut cursor = 0usize;
    let mut part_count = 0usize;

    while let Some(index) = find_multipart_boundary(&body[cursor..], &delimiter) {
        let start = cursor + index + delimiter.len();
        if body.get(start..start + 2) == Some(b"--") {
            let closing_suffix = body.get(start + 2..).unwrap_or_default();
            if !(closing_suffix.is_empty() || closing_suffix.starts_with(b"\r\n")) {
                return Vec::new();
            }
            break;
        }
        part_count = part_count.saturating_add(1);
        if part_count > MAX_MULTIPART_PARTS {
            return Vec::new();
        }
        let mut part = &body[start..];
        if part.starts_with(b"\r\n") {
            part = &part[2..];
        }
        // A multipart body is only valid once a real (CRLF-delimited) next
        // boundary has been found.  Returning the fields parsed so far here
        // would let a truncated request pass validation.
        let Some(next) = find_multipart_boundary_after_crlf(part, &delimiter) else {
            return Vec::new();
        };
        let raw = &part[..next];
        let raw = raw.strip_suffix(b"\r\n").unwrap_or(raw);
        if find_subslice(raw, b"\r\n\r\n")
            .is_some_and(|header_end| header_end > MAX_MULTIPART_PART_HEADER_BYTES)
        {
            return Vec::new();
        }
        let Some(field) = parse_multipart_field(raw) else {
            return Vec::new();
        };
        parts.push(field);
        cursor = start + next;
    }

    parts
}

fn parse_multipart_field(raw: &[u8]) -> Option<MultipartField> {
    let header_end = find_subslice(raw, b"\r\n\r\n")?;
    let headers = &raw[..header_end];
    let data = raw.get(header_end + 4..)?.to_vec();
    let header_text = std::str::from_utf8(headers).ok()?;

    let mut name = None;
    let mut disposition_seen = false;
    for line in header_text.split("\r\n") {
        let (header_name, header_value) = line.split_once(':')?;
        let header_name = header_name.trim();
        if header_name.eq_ignore_ascii_case("content-disposition") {
            if disposition_seen {
                return None;
            }
            disposition_seen = true;
            name = parse_multipart_content_disposition_name(header_value.trim());
        }
    }

    Some(MultipartField { name: name?, data })
}

fn parse_multipart_content_disposition_name(value: &str) -> Option<String> {
    let segments = split_multipart_header_parameters(value)?;
    let disposition = segments.first()?.trim();
    if !disposition.eq_ignore_ascii_case("form-data") {
        return None;
    }

    let mut seen_keys = Vec::new();
    let mut name = None;
    for segment in segments.into_iter().skip(1) {
        let segment = segment.trim();
        if segment.is_empty() {
            return None;
        }
        let (raw_key, raw_value) = segment.split_once('=')?;
        let key = raw_key.trim();
        if key.is_empty() || !key.as_bytes().iter().copied().all(is_multipart_token_byte) {
            return None;
        }
        if seen_keys
            .iter()
            .any(|seen: &String| seen.eq_ignore_ascii_case(key))
        {
            return None;
        }
        seen_keys.push(key.to_ascii_lowercase());

        let parsed_value = parse_multipart_header_parameter_value(raw_value.trim())?;
        if key.eq_ignore_ascii_case("name") {
            if parsed_value.is_empty() {
                return None;
            }
            name = Some(parsed_value);
        }
    }

    name
}

fn split_multipart_header_parameters(value: &str) -> Option<Vec<&str>> {
    let mut segments = Vec::new();
    let mut start = 0usize;
    let mut in_quotes = false;
    let mut escaped = false;

    for (index, byte) in value.as_bytes().iter().copied().enumerate() {
        if in_quotes {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_quotes = false;
            }
        } else if byte == b'"' {
            in_quotes = true;
        } else if byte == b';' {
            segments.push(&value[start..index]);
            start = index + 1;
        }
    }

    if in_quotes || escaped {
        return None;
    }
    segments.push(&value[start..]);
    Some(segments)
}

fn parse_multipart_header_parameter_value(value: &str) -> Option<String> {
    if value.is_empty() {
        return None;
    }
    if value.starts_with('"') {
        if value.len() < 2 || !value.ends_with('"') {
            return None;
        }
        let inner = &value[1..value.len() - 1];
        let mut parsed = String::with_capacity(inner.len());
        let mut escaped = false;
        for character in inner.chars() {
            if escaped {
                if character.is_control() {
                    return None;
                }
                parsed.push(character);
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else {
                if character == '"' || character.is_control() {
                    return None;
                }
                parsed.push(character);
            }
        }
        if escaped {
            return None;
        }
        return Some(parsed);
    }

    value
        .as_bytes()
        .iter()
        .copied()
        .all(is_multipart_token_byte)
        .then(|| value.to_string())
}

fn is_multipart_token_byte(byte: u8) -> bool {
    matches!(
        byte,
        b'0'..=b'9'
            | b'A'..=b'Z'
            | b'a'..=b'z'
            | b'!'
            | b'#'
            | b'$'
            | b'%'
            | b'&'
            | b'\''
            | b'*'
            | b'+'
            | b'-'
            | b'.'
            | b'^'
            | b'_'
            | b'`'
            | b'|'
            | b'~'
    )
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn maybe_build_local_ai_public_route_guard_response(
    request_context: &GatewayPublicRequestContext,
) -> Option<Response<Body>> {
    if request_context.request_path == "/upload/v1beta/files"
        && request_context.request_method != http::Method::POST
    {
        return Some(build_ai_public_error_response(
            http::StatusCode::METHOD_NOT_ALLOWED,
            AI_PUBLIC_METHOD_NOT_ALLOWED_DETAIL,
        ));
    }

    None
}

fn maybe_build_local_claude_count_tokens_validation_response(
    request_context: &GatewayPublicRequestContext,
    request_body: Option<&Bytes>,
) -> Option<Response<Body>> {
    let decision = request_context.control_decision.as_ref()?;
    if decision.route_family.as_deref() != Some("claude")
        || decision.route_kind.as_deref() != Some("count_tokens")
        || request_context.request_method != http::Method::POST
        || request_context.request_path != "/v1/messages/count_tokens"
    {
        return None;
    }

    let validation = validate_claude_count_tokens_request(request_body);
    validation.err().map(build_claude_invalid_request_response)
}

fn validate_claude_count_tokens_request(request_body: Option<&Bytes>) -> Result<(), &'static str> {
    let request_body = request_body
        .filter(|body| !body.is_empty())
        .ok_or(CLAUDE_COUNT_TOKENS_BODY_REQUIRED_DETAIL)?;
    let payload = serde_json::from_slice::<Value>(request_body)
        .map_err(|_| CLAUDE_COUNT_TOKENS_INVALID_JSON_DETAIL)?;
    let object = payload
        .as_object()
        .ok_or(CLAUDE_COUNT_TOKENS_INVALID_JSON_DETAIL)?;

    if object
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .is_none()
    {
        return Err(CLAUDE_COUNT_TOKENS_MODEL_REQUIRED_DETAIL);
    }
    if object.get("messages").and_then(Value::as_array).is_none() {
        return Err(CLAUDE_COUNT_TOKENS_MESSAGES_REQUIRED_DETAIL);
    }

    Ok(())
}

fn build_claude_invalid_request_response(detail: &'static str) -> Response<Body> {
    let body = build_core_error_body_for_client_format(
        "claude:messages",
        detail,
        None,
        LocalCoreSyncErrorKind::InvalidRequest,
    )
    .expect("Claude core error format should be available");
    (http::StatusCode::BAD_REQUEST, Json(body)).into_response()
}

fn maybe_build_local_antigravity_v1internal_response(
    request_context: &GatewayPublicRequestContext,
    request_body: Option<&Bytes>,
) -> Option<Response<Body>> {
    let decision = request_context.control_decision.as_ref()?;
    if decision.route_family.as_deref() != Some("antigravity")
        || request_context.request_method != http::Method::POST
    {
        return None;
    }

    match decision.route_kind.as_deref()? {
        "load_code_assist" => {
            Some(Json(build_antigravity_load_code_assist_payload()).into_response())
        }
        "fetch_available_models" => {
            Some(Json(build_antigravity_fetch_available_models_payload()).into_response())
        }
        "retrieve_user_quota_summary" => {
            Some(Json(build_antigravity_retrieve_user_quota_summary_payload()).into_response())
        }
        "fetch_user_info" => {
            Some(Json(build_antigravity_fetch_user_info_payload()).into_response())
        }
        "fetch_admin_controls" => Some(Json(json!({})).into_response()),
        "list_experiments" => Some(
            Json(json!({
                "experimentIds": [],
                "flags": []
            }))
            .into_response(),
        ),
        "record_code_assist_metrics" => Some(Json(json!({})).into_response()),
        "write_trajectory_acls" => Some(Json(json!({})).into_response()),
        "set_user_settings" => Some(build_antigravity_set_user_settings_response(request_body)),
        "stream_generate_content" => None,
        _ => None,
    }
}

fn build_antigravity_set_user_settings_response(request_body: Option<&Bytes>) -> Response<Body> {
    let Some(request_body) = request_body else {
        return build_ai_public_error_response(
            http::StatusCode::BAD_REQUEST,
            ANTIGRAVITY_USER_SETTINGS_MISSING_BODY_DETAIL,
        );
    };
    let payload = match serde_json::from_slice::<Value>(request_body) {
        Ok(payload) => payload,
        Err(_) => {
            return build_ai_public_error_response(
                http::StatusCode::BAD_REQUEST,
                ANTIGRAVITY_USER_SETTINGS_INVALID_JSON_DETAIL,
            );
        }
    };
    let Some(user_settings) = payload
        .get("userSettings")
        .filter(|value| value.is_object())
        .cloned()
    else {
        return build_ai_public_error_response(
            http::StatusCode::BAD_REQUEST,
            ANTIGRAVITY_USER_SETTINGS_INVALID_DETAIL,
        );
    };

    Json(json!({ "userSettings": user_settings })).into_response()
}

fn build_antigravity_load_code_assist_payload() -> Value {
    json!({
        "allowedTiers": [
            antigravity_free_tier_payload(true),
            antigravity_standard_tier_payload()
        ],
        "cloudaicompanionProject": "aether-antigravity-local",
        "currentTier": antigravity_free_tier_payload(false),
        "gcpManaged": false,
        "paidTier": antigravity_paid_tier_payload(),
        "upgradeSubscriptionUri": "https://codeassist.google.com/upgrade"
    })
}

fn antigravity_free_tier_payload(include_default_marker: bool) -> Value {
    if include_default_marker {
        json!({
            "id": "free-tier",
            "name": "Antigravity",
            "description": "Gemini-powered code suggestions and chat in multiple IDEs",
            "privacyNotice": {
                "showNotice": false
            },
            "isDefault": true
        })
    } else {
        json!({
            "id": "free-tier",
            "name": "Antigravity",
            "description": "Gemini-powered code suggestions and chat in multiple IDEs",
            "privacyNotice": {
                "showNotice": false
            },
            "upgradeSubscriptionUri": "https://codeassist.google.com/upgrade",
            "upgradeSubscriptionText": "Upgrade for higher Antigravity request limits",
            "upgradeSubscriptionType": "GDP_HELIUM"
        })
    }
}

fn antigravity_standard_tier_payload() -> Value {
    json!({
        "id": "standard-tier",
        "name": "Antigravity",
        "description": "Unlimited coding assistant with the most powerful Gemini models",
        "userDefinedCloudaicompanionProject": true,
        "privacyNotice": {},
        "usesGcpTos": true
    })
}

fn antigravity_paid_tier_payload() -> Value {
    json!({
        "id": "g1-pro-tier",
        "name": "Google AI Pro",
        "description": "Google AI Pro",
        "upgradeSubscriptionUri": "https://antigravity.google/g1-upgrade",
        "upgradeSubscriptionText": "Upgrade for the highest Antigravity request limits"
    })
}

fn build_antigravity_fetch_user_info_payload() -> Value {
    json!({
        "regionCode": "US",
        "userSettings": build_antigravity_default_user_settings_payload()
    })
}

fn build_antigravity_retrieve_user_quota_summary_payload() -> Value {
    json!({
        "description": "",
        "groups": []
    })
}

fn build_antigravity_default_user_settings_payload() -> Value {
    json!({
        "preferredModelId": "gemini-3.5-flash-low"
    })
}

fn build_antigravity_fetch_available_models_payload() -> Value {
    json!({
        "models": {
            "gemini-pro-agent": antigravity_model_payload("gemini-pro-agent", "Gemini 3.1 Pro (High)"),
            "gemini-3.1-pro-low": antigravity_model_payload("gemini-3.1-pro-low", "Gemini 3.1 Pro (Low)"),
            "gemini-3-flash-agent": antigravity_model_payload("gemini-3-flash-agent", "Gemini 3.5 Flash (High)"),
            "gemini-3.5-flash-low": antigravity_model_payload("gemini-3.5-flash-low", "Gemini 3.5 Flash (Medium)"),
            "gemini-3.5-flash-extra-low": antigravity_model_payload("gemini-3.5-flash-extra-low", "Gemini 3.5 Flash (Low)"),
            "claude-opus-4-6-thinking": antigravity_model_payload("claude-opus-4-6-thinking", "Claude Opus 4.6 (Thinking)"),
            "claude-sonnet-4-6": antigravity_model_payload("claude-sonnet-4-6", "Claude Sonnet 4.6 (Thinking)"),
            "gpt-oss-120b-medium": antigravity_model_payload("gpt-oss-120b-medium", "GPT-OSS 120B (Medium)"),
            "gemini-3.1-flash-lite": antigravity_model_payload("gemini-3.1-flash-lite", "Gemini 3.1 Flash Lite"),
            "gemini-3-flash": antigravity_model_payload("gemini-3-flash", "Gemini 3 Flash"),
            "gemini-2.5-flash": antigravity_model_payload("gemini-2.5-flash", "Gemini 3.1 Flash Lite"),
            "gemini-2.5-flash-lite": antigravity_model_payload("gemini-2.5-flash-lite", "Gemini 3.1 Flash Lite"),
            "gemini-2.5-flash-thinking": antigravity_model_payload("gemini-2.5-flash-thinking", "Gemini 3.1 Flash Lite"),
            "gemini-2.5-pro": antigravity_model_payload("gemini-2.5-pro", "Gemini 2.5 Pro"),
            "gemini-3.1-flash-image": antigravity_model_payload("gemini-3.1-flash-image", "Gemini 3.1 Flash Image"),
            "gemini-3.1-pro-high": antigravity_model_payload("gemini-3.1-pro-high", "Gemini 3.1 Pro (High)"),
            "chat_20706": antigravity_model_payload("chat_20706", ""),
            "chat_23310": antigravity_model_payload("chat_23310", ""),
            "tab_flash_lite_preview": antigravity_model_payload("tab_flash_lite_preview", ""),
            "tab_jump_flash_lite_preview": antigravity_model_payload("tab_jump_flash_lite_preview", ""),
            "models/proactive-observer": antigravity_model_payload("models/proactive-observer", "Proactive Observer")
        },
        "agentModelSorts": [
            {
                "displayName": "Recommended",
                "groups": [
                    {
                        "modelIds": [
                            "gemini-3.5-flash-low",
                            "gemini-3-flash-agent",
                            "gemini-3.5-flash-extra-low",
                            "gemini-3.1-pro-low",
                            "gemini-pro-agent",
                            "claude-sonnet-4-6",
                            "claude-opus-4-6-thinking",
                            "gpt-oss-120b-medium"
                        ]
                    }
                ]
            }
        ],
        "audioTranscriptionModelIds": ["models/proactive-observer"],
        "commandModelIds": ["gemini-3-flash"],
        "commitMessageModelIds": ["gemini-3.1-flash-lite"],
        "defaultAgentModelId": "gemini-3.5-flash-low",
        "deprecatedModelIds": {
            "gemini-3.1-pro-high": {
                "newModelEnum": "MODEL_PLACEHOLDER_M16",
                "newModelId": "gemini-pro-agent",
                "oldModelEnum": "MODEL_PLACEHOLDER_M37"
            }
        },
        "experimentIds": [],
        "imageGenerationModelIds": ["gemini-3.1-flash-image"],
        "mqueryModelIds": ["gemini-3.1-flash-lite"],
        "tabModelIds": ["chat_20706", "chat_23310"],
        "tieredModelIds": {
            "flash": ["gemini-3-flash-agent"],
            "flashLite": ["gemini-3.1-flash-lite"],
            "pro": ["gemini-3.1-pro-low"]
        },
        "webSearchModelIds": ["gemini-3.1-flash-lite"]
    })
}

fn antigravity_model_payload(id: &str, display_name: &str) -> Value {
    let model = match id {
        "chat_20706" => "MODEL_CHAT_20706",
        "chat_23310" => "MODEL_CHAT_23310",
        "claude-opus-4-6-thinking" => "MODEL_PLACEHOLDER_M26",
        "claude-sonnet-4-6" => "MODEL_PLACEHOLDER_M35",
        "gpt-oss-120b-medium" => "MODEL_OPENAI_GPT_OSS_120B_MEDIUM",
        "gemini-2.5-flash" => "MODEL_GOOGLE_GEMINI_2_5_FLASH",
        "gemini-2.5-flash-lite" => "MODEL_GOOGLE_GEMINI_2_5_FLASH_LITE",
        "gemini-2.5-flash-thinking" => "MODEL_GOOGLE_GEMINI_2_5_FLASH_THINKING",
        "gemini-2.5-pro" => "MODEL_GOOGLE_GEMINI_2_5_PRO",
        "gemini-3-flash" => "MODEL_PLACEHOLDER_M18",
        "gemini-3-flash-agent" => "MODEL_PLACEHOLDER_M132",
        "gemini-3.1-flash-image" => "MODEL_PLACEHOLDER_M21",
        "gemini-3.1-flash-lite" => "MODEL_PLACEHOLDER_M50",
        "gemini-pro-agent" => "MODEL_PLACEHOLDER_M16",
        "gemini-3.1-pro-high" => "MODEL_PLACEHOLDER_M37",
        "gemini-3.1-pro-low" => "MODEL_PLACEHOLDER_M36",
        "gemini-3.5-flash-low" => "MODEL_PLACEHOLDER_M20",
        "gemini-3.5-flash-extra-low" => "MODEL_PLACEHOLDER_M187",
        "models/proactive-observer" => "MODEL_PLACEHOLDER_M70",
        "tab_flash_lite_preview" => "MODEL_PLACEHOLDER_M19",
        "tab_jump_flash_lite_preview" => "MODEL_PLACEHOLDER_M28",
        _ => "MODEL_PLACEHOLDER_M20",
    };
    let (api_provider, model_provider) = match id {
        "claude-opus-4-6-thinking" | "claude-sonnet-4-6" => {
            ("API_PROVIDER_ANTHROPIC_VERTEX", "MODEL_PROVIDER_ANTHROPIC")
        }
        "gpt-oss-120b-medium" => ("API_PROVIDER_OPENAI_VERTEX", "MODEL_PROVIDER_OPENAI"),
        "chat_20706" | "chat_23310" => ("API_PROVIDER_INTERNAL", "MODEL_PROVIDER_GOOGLE"),
        _ => ("API_PROVIDER_GOOGLE_GEMINI", "MODEL_PROVIDER_GOOGLE"),
    };
    let mut payload = json!({
        "apiProvider": api_provider,
        "displayName": display_name,
        "maxOutputTokens": 65536,
        "maxTokens": 1048576,
        "minThinkingBudget": 32,
        "model": model,
        "modelProvider": model_provider,
        "recommended": matches!(
            id,
            "gemini-3.5-flash-low"
                | "gemini-3-flash-agent"
                | "gemini-3.5-flash-extra-low"
                | "gemini-3.1-pro-low"
                | "gemini-pro-agent"
                | "claude-sonnet-4-6"
                | "claude-opus-4-6-thinking"
                | "gpt-oss-120b-medium"
        ),
        "supportedMimeTypes": {
            "application/json": true,
            "application/pdf": true,
            "image/jpeg": true,
            "image/png": true,
            "text/markdown": true,
            "text/plain": true
        },
        "supportsImages": true,
        "supportsThinking": true,
        "supportsVideo": true,
        "thinkingBudget": 4000,
        "tokenizerType": "LLAMA_WITH_SPECIAL"
    });
    if let Some(object) = payload.as_object_mut() {
        match id {
            "gemini-3-flash-agent" => {
                object.insert("thinkingBudget".to_string(), json!(10000));
            }
            "gemini-pro-agent" => {
                object.insert("maxOutputTokens".to_string(), json!(65535));
                object.insert("thinkingBudget".to_string(), json!(10001));
            }
            "gemini-3.1-pro-high" => {
                object.insert("maxOutputTokens".to_string(), json!(65535));
                object.insert("minThinkingBudget".to_string(), json!(128));
                object.insert("thinkingBudget".to_string(), json!(10001));
            }
            "gemini-3.5-flash-extra-low" => {
                object.insert("thinkingBudget".to_string(), json!(1000));
                object.insert("maxOutputTokens".to_string(), json!(65536));
            }
            "gemini-3.1-pro-low" => {
                object.insert("maxOutputTokens".to_string(), json!(65535));
                object.insert("minThinkingBudget".to_string(), json!(128));
                object.insert("thinkingBudget".to_string(), json!(1001));
            }
            "claude-sonnet-4-6" | "claude-opus-4-6-thinking" => {
                object.insert("maxTokens".to_string(), json!(250000));
                object.insert("maxOutputTokens".to_string(), json!(64000));
                object.insert("thinkingBudget".to_string(), json!(1024));
                object.remove("minThinkingBudget");
                object.remove("supportsVideo");
            }
            "gpt-oss-120b-medium" => {
                object.insert("maxTokens".to_string(), json!(131072));
                object.insert("maxOutputTokens".to_string(), json!(32768));
                object.insert("thinkingBudget".to_string(), json!(8192));
                object.remove("minThinkingBudget");
                object.remove("supportsImages");
                object.remove("supportsVideo");
            }
            "chat_20706" => {
                object.insert("maxTokens".to_string(), json!(16384));
                object.remove("displayName");
                object.remove("maxOutputTokens");
                object.remove("minThinkingBudget");
                object.remove("recommended");
                object.remove("supportsImages");
                object.remove("supportsThinking");
                object.remove("supportsVideo");
                object.remove("thinkingBudget");
                object.remove("tokenizerType");
                object.remove("supportedMimeTypes");
            }
            "chat_23310" => {
                object.insert("maxTokens".to_string(), json!(32768));
                object.remove("displayName");
                object.remove("maxOutputTokens");
                object.remove("minThinkingBudget");
                object.remove("recommended");
                object.remove("supportsImages");
                object.remove("supportsThinking");
                object.remove("supportsVideo");
                object.remove("thinkingBudget");
                object.remove("tokenizerType");
                object.remove("supportedMimeTypes");
            }
            "tab_flash_lite_preview" | "tab_jump_flash_lite_preview" => {
                object.insert("maxTokens".to_string(), json!(16384));
                object.insert("maxOutputTokens".to_string(), json!(4096));
                object.remove("displayName");
                object.remove("minThinkingBudget");
                object.remove("recommended");
                object.remove("supportsImages");
                object.remove("supportsThinking");
                object.remove("supportsVideo");
                object.remove("thinkingBudget");
                object.remove("tokenizerType");
                object.remove("supportedMimeTypes");
            }
            _ => {}
        }
    }
    payload
}

async fn maybe_build_local_gemini_video_operations_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    decision: &GatewayControlDecision,
) -> Option<Response<Body>> {
    if decision.route_family.as_deref() != Some("gemini")
        || decision.route_kind.as_deref() != Some("video")
    {
        return None;
    }

    if request_context.request_path == "/v1beta/operations" {
        return Some(match request_context.request_method {
            http::Method::GET => {
                build_local_gemini_video_operations_list_response(state, decision).await
            }
            _ => build_ai_public_error_response(
                http::StatusCode::METHOD_NOT_ALLOWED,
                AI_PUBLIC_METHOD_NOT_ALLOWED_DETAIL,
            ),
        });
    }

    let Some(operation_path) = request_context
        .request_path
        .strip_prefix("/v1beta/operations/")
    else {
        return None;
    };

    Some(match request_context.request_method {
        http::Method::GET => {
            build_local_gemini_video_operation_detail_response(state, decision, operation_path)
                .await
        }
        http::Method::POST if operation_path.ends_with(":cancel") => {
            build_local_gemini_video_operation_cancel_response(state, decision, operation_path)
                .await
        }
        _ => build_ai_public_error_response(
            http::StatusCode::METHOD_NOT_ALLOWED,
            AI_PUBLIC_METHOD_NOT_ALLOWED_DETAIL,
        ),
    })
}

async fn build_local_gemini_video_operations_list_response(
    state: &AppState,
    decision: &GatewayControlDecision,
) -> Response<Body> {
    let Some(user_id) = allowed_ai_public_user_id(decision) else {
        return build_ai_public_error_response(
            http::StatusCode::UNAUTHORIZED,
            AI_PUBLIC_UNAUTHORIZED_DETAIL,
        );
    };

    let filter = VideoTaskQueryFilter {
        user_id: Some(user_id.to_string()),
        status: None,
        model_substring: None,
        client_api_format: Some("gemini:video".to_string()),
    };
    let tasks = match state.list_video_task_page(&filter, 0, 100).await {
        Ok(tasks) => tasks,
        Err(_) => {
            return build_ai_public_internal_error_response(
                "gemini_video_operations_list",
                "data_store_unavailable",
            );
        }
    };
    let operations = tasks
        .into_iter()
        .filter(is_gemini_video_task)
        .map(|task| build_gemini_video_operation_payload(&task))
        .collect::<Vec<_>>();

    Json(json!({ "operations": operations })).into_response()
}

async fn build_local_gemini_video_operation_detail_response(
    state: &AppState,
    decision: &GatewayControlDecision,
    operation_path: &str,
) -> Response<Body> {
    let task =
        match find_user_gemini_video_task_for_operation(state, decision, operation_path).await {
            Ok(Some(task)) => task,
            Ok(None) => {
                return build_ai_public_error_response(
                    http::StatusCode::NOT_FOUND,
                    GEMINI_VIDEO_TASK_NOT_FOUND_DETAIL,
                );
            }
            Err(_) => {
                return build_ai_public_internal_error_response(
                    "gemini_video_operation_detail",
                    "data_store_unavailable",
                );
            }
        };

    Json(build_gemini_video_operation_payload(&task)).into_response()
}

async fn build_local_gemini_video_operation_cancel_response(
    state: &AppState,
    decision: &GatewayControlDecision,
    operation_path: &str,
) -> Response<Body> {
    let Some(user_id) = allowed_ai_public_user_id(decision) else {
        return build_ai_public_error_response(
            http::StatusCode::UNAUTHORIZED,
            AI_PUBLIC_UNAUTHORIZED_DETAIL,
        );
    };
    let task =
        match find_user_gemini_video_task_for_operation(state, decision, operation_path).await {
            Ok(Some(task)) => task,
            Ok(None) => {
                return build_ai_public_error_response(
                    http::StatusCode::NOT_FOUND,
                    GEMINI_VIDEO_TASK_NOT_FOUND_DETAIL,
                );
            }
            Err(_) => {
                return build_ai_public_internal_error_response(
                    "gemini_video_operation_cancel_lookup",
                    "data_store_unavailable",
                );
            }
        };

    match crate::async_task::cancel_video_task_record_for_user(state, &task.id, user_id).await {
        Ok(_) => Json(json!({})).into_response(),
        Err(CancelVideoTaskError::NotFound) => build_ai_public_error_response(
            http::StatusCode::NOT_FOUND,
            GEMINI_VIDEO_TASK_NOT_FOUND_DETAIL,
        ),
        Err(CancelVideoTaskError::InvalidStatus(status)) => build_ai_public_error_response(
            http::StatusCode::BAD_REQUEST,
            format!(
                "Cannot cancel task with status: {}",
                video_task_status_name(status)
            ),
        ),
        Err(CancelVideoTaskError::Response(response)) => {
            build_ai_public_upstream_error_response(response, "gemini_video_operation_cancel")
        }
        Err(CancelVideoTaskError::Gateway(_)) => build_ai_public_internal_error_response(
            "gemini_video_operation_cancel",
            "cancel_execution_failed",
        ),
    }
}

async fn find_user_gemini_video_task_for_operation(
    state: &AppState,
    decision: &GatewayControlDecision,
    operation_path: &str,
) -> Result<Option<StoredVideoTask>, GatewayError> {
    let Some(user_id) = allowed_ai_public_user_id(decision) else {
        return Ok(None);
    };
    let Some(short_id) = extract_short_id_from_gemini_operation_path(operation_path) else {
        return Ok(None);
    };
    let Some(task) = state
        .find_video_task_by_short_id_for_user(short_id, user_id)
        .await?
    else {
        return Ok(None);
    };
    if !is_gemini_video_task(&task) {
        return Ok(None);
    }
    Ok(Some(task))
}

fn extract_short_id_from_gemini_operation_path(operation_path: &str) -> Option<&str> {
    let trimmed = operation_path.trim_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let short_id = trimmed
        .strip_suffix(":cancel")
        .unwrap_or(trimmed)
        .rsplit('/')
        .next()?;
    (!short_id.is_empty()).then_some(short_id)
}

fn is_gemini_video_task(task: &StoredVideoTask) -> bool {
    task.effective_api_format() == Some("gemini:video")
}

fn build_gemini_video_operation_payload(task: &StoredVideoTask) -> serde_json::Value {
    match task.status {
        VideoTaskStatus::Completed => json!({
            "name": gemini_video_operation_name(task),
            "done": true,
            "response": {
                "generateVideoResponse": {
                    "generatedSamples": [
                        {
                            "video": {
                                "uri": format!(
                                    "/v1beta/files/aev_{}:download?alt=media",
                                    gemini_operation_short_id(task)
                                ),
                                "mimeType": "video/mp4",
                            }
                        }
                    ]
                }
            }
        }),
        VideoTaskStatus::Failed | VideoTaskStatus::Expired => json!({
            "name": gemini_video_operation_name(task),
            "done": true,
            "error": gemini_video_task_error_projection(task),
        }),
        _ => json!({
            "name": gemini_video_operation_name(task),
            "done": false,
            "metadata": gemini_video_operation_metadata(task),
        }),
    }
}

fn gemini_video_task_error_projection(task: &StoredVideoTask) -> serde_json::Value {
    let code = match task.error_code.as_deref().map(str::trim) {
        Some("authentication_error") => "authentication_error",
        Some("content_policy_violation") => "content_policy_violation",
        Some("expired") => "expired",
        Some("invalid_request") => "invalid_request",
        Some("not_found") => "not_found",
        Some("permission_denied") => "permission_denied",
        Some("poll_permanent_error") => "poll_permanent_error",
        Some("poll_timeout") => "poll_timeout",
        Some("provider_error") => "provider_error",
        Some("rate_limit_exceeded") => "rate_limit_exceeded",
        Some("server_error") => "server_error",
        Some("unknown") => "unknown",
        _ => "provider_error",
    };
    json!({
        "code": code,
        "message": "Video generation failed",
    })
}

fn gemini_video_operation_name(task: &StoredVideoTask) -> String {
    format!(
        "models/{}/operations/{}",
        gemini_operation_model(task),
        gemini_operation_short_id(task)
    )
}

fn gemini_operation_model(task: &StoredVideoTask) -> String {
    task.model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            task.external_task_id.as_deref().and_then(|external_id| {
                let parts = external_id.split('/').collect::<Vec<_>>();
                if parts.len() >= 2 && parts[0] == "models" && !parts[1].trim().is_empty() {
                    Some(parts[1].trim().to_string())
                } else {
                    None
                }
            })
        })
        .unwrap_or_else(|| "unknown".to_string())
}

fn gemini_operation_short_id(task: &StoredVideoTask) -> String {
    task.short_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(task.id.as_str())
        .to_string()
}

fn gemini_video_operation_metadata(task: &StoredVideoTask) -> serde_json::Value {
    task.request_metadata
        .as_ref()
        .and_then(|metadata| metadata.get("rust_local_snapshot"))
        .and_then(|snapshot| snapshot.get("Gemini"))
        .and_then(|gemini| gemini.get("metadata"))
        .cloned()
        .unwrap_or_else(|| json!({}))
}

fn video_task_status_name(status: VideoTaskStatus) -> &'static str {
    match status {
        VideoTaskStatus::Pending => "pending",
        VideoTaskStatus::Submitted => "submitted",
        VideoTaskStatus::Queued => "queued",
        VideoTaskStatus::Processing => "processing",
        VideoTaskStatus::Completed => "completed",
        VideoTaskStatus::Failed => "failed",
        VideoTaskStatus::Cancelled => "cancelled",
        VideoTaskStatus::Expired => "expired",
        VideoTaskStatus::Deleted => "deleted",
    }
}

fn build_ai_public_error_response(
    status: http::StatusCode,
    detail: impl Into<String>,
) -> Response<Body> {
    let detail = detail.into();
    let public_detail = if status.is_server_error() {
        tracing::error!(
            event_name = "ai_public_internal_error",
            %status,
            "internal AI public API error hidden from client"
        );
        AI_PUBLIC_INTERNAL_ERROR_DETAIL.to_string()
    } else {
        detail
    };
    build_ai_public_error_payload(status, public_detail)
}

fn build_ai_public_internal_error_response(
    operation: &'static str,
    error_category: &'static str,
) -> Response<Body> {
    tracing::error!(
        event_name = "ai_public_internal_error",
        operation,
        error_category,
        "internal AI public operation failed"
    );
    build_ai_public_error_payload(
        http::StatusCode::INTERNAL_SERVER_ERROR,
        AI_PUBLIC_INTERNAL_ERROR_DETAIL,
    )
}

fn build_ai_public_upstream_error_response(
    response: Response<Body>,
    operation: &'static str,
) -> Response<Body> {
    let upstream_status = response.status();
    tracing::warn!(
        event_name = "ai_public_upstream_error",
        operation,
        error_category = "upstream_response_projected",
        "upstream error response body discarded"
    );

    // Preserve a provider 4xx status for caller retry/validation semantics, but never
    // forward its body. Non-4xx responses are represented as a gateway failure.
    let status = if upstream_status.is_client_error() {
        upstream_status
    } else {
        http::StatusCode::BAD_GATEWAY
    };
    let detail = if status.is_server_error() {
        AI_PUBLIC_INTERNAL_ERROR_DETAIL
    } else {
        AI_PUBLIC_UPSTREAM_ERROR_DETAIL
    };
    build_ai_public_error_payload(status, detail)
}

fn build_ai_public_error_payload(
    status: http::StatusCode,
    detail: impl Into<String>,
) -> Response<Body> {
    (status, Json(json!({ "detail": detail.into() }))).into_response()
}

#[cfg(test)]
mod tests {
    use super::{
        build_ai_public_error_response, build_ai_public_upstream_error_response,
        build_gemini_file_mapping_payload, gemini_video_task_error_projection,
        parse_multipart_fields, parse_openai_image_validation_input,
        validate_claude_count_tokens_request, validate_openai_image_n, OpenAiImageOperation,
        StoredGeminiFileMapping, AI_PUBLIC_UPSTREAM_ERROR_DETAIL,
        CLAUDE_COUNT_TOKENS_BODY_REQUIRED_DETAIL, CLAUDE_COUNT_TOKENS_INVALID_JSON_DETAIL,
        CLAUDE_COUNT_TOKENS_MESSAGES_REQUIRED_DETAIL, CLAUDE_COUNT_TOKENS_MODEL_REQUIRED_DETAIL,
        MAX_MULTIPART_PARTS, MAX_MULTIPART_PART_HEADER_BYTES,
        OPENAI_IMAGE_INVALID_MULTIPART_DETAIL,
    };
    use aether_data_contracts::repository::video_tasks::{StoredVideoTask, VideoTaskStatus};
    use axum::body::{to_bytes, Body, Bytes};
    use axum::http::StatusCode;
    use serde_json::json;

    #[tokio::test]
    async fn ai_public_server_errors_do_not_expose_internal_details() {
        let response = build_ai_public_error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "database connection failed: password=internal-secret",
        );

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("error response body should read");
        let payload: serde_json::Value =
            serde_json::from_slice(&body).expect("error response should be JSON");
        assert_eq!(payload["detail"], "Service temporarily unavailable");
        assert!(!String::from_utf8_lossy(&body).contains("internal-secret"));
    }

    #[tokio::test]
    async fn ai_public_upstream_errors_discard_the_upstream_response_body() {
        let upstream_response = axum::http::Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .body(Body::from(
                r#"{"error":{"message":"Bearer upstream-secret at https://internal.test"}}"#,
            ))
            .expect("upstream response should build");

        let response = build_ai_public_upstream_error_response(upstream_response, "test_operation");

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("projected response body should read");
        let payload: serde_json::Value =
            serde_json::from_slice(&body).expect("projected response should be JSON");
        assert_eq!(payload["detail"], AI_PUBLIC_UPSTREAM_ERROR_DETAIL);
        let body = String::from_utf8_lossy(&body);
        for secret in ["upstream-secret", "Bearer", "internal.test"] {
            assert!(!body.contains(secret));
        }
    }

    #[test]
    fn gemini_video_error_projection_discards_historical_sensitive_diagnostics() {
        let mut task = StoredVideoTask::new(
            "task-1".to_string(),
            Some("short-1".to_string()),
            "request-1".to_string(),
            Some("user-1".to_string()),
            None,
            None,
            None,
            Some("operations/upstream-1".to_string()),
            Some("provider-1".to_string()),
            Some("endpoint-1".to_string()),
            Some("key-1".to_string()),
            Some("gemini:video".to_string()),
            Some("gemini:video".to_string()),
            false,
            Some("veo-3".to_string()),
            None,
            None,
            None,
            None,
            None,
            None,
            VideoTaskStatus::Failed,
            100,
            None,
            0,
            10,
            None,
            1,
            360,
            1,
            Some(1),
            Some(2),
            2,
            Some("provider_error".to_string()),
            None,
            None,
            None,
        )
        .expect("task should be valid");
        task.error_code = Some(
            "Authorization: Bearer code-secret at https://internal.test/?key=secret".to_string(),
        );
        task.error_message = Some(
            "Authorization: Bearer message-secret at https://internal.test/?token=secret"
                .to_string(),
        );

        let payload = gemini_video_task_error_projection(&task);

        assert_eq!(payload["code"], "provider_error");
        assert_eq!(payload["message"], "Video generation failed");
        let encoded = payload.to_string();
        for sensitive in ["Bearer", "code-secret", "message-secret", "internal.test"] {
            assert!(!encoded.contains(sensitive));
        }
    }

    #[test]
    fn gemini_file_payload_formats_millisecond_timestamps_as_milliseconds() {
        let mapping = StoredGeminiFileMapping::new(
            "mapping-1".to_string(),
            "files/file-1".to_string(),
            "key-1".to_string(),
            1_710_000_123_456,
            1_710_003_723,
        )
        .expect("mapping should be valid");

        let payload = build_gemini_file_mapping_payload(&mapping);

        assert_eq!(payload["createTime"], "2024-03-09T16:02:03.456Z");
    }

    #[test]
    fn count_tokens_validation_rejects_only_structurally_invalid_requests() {
        assert_eq!(
            validate_claude_count_tokens_request(None),
            Err(CLAUDE_COUNT_TOKENS_BODY_REQUIRED_DETAIL)
        );
        assert_eq!(
            validate_claude_count_tokens_request(Some(&Bytes::from_static(b"{"))),
            Err(CLAUDE_COUNT_TOKENS_INVALID_JSON_DETAIL)
        );
        assert_eq!(
            validate_claude_count_tokens_request(Some(&Bytes::from_static(br#"{"messages":[]}"#,))),
            Err(CLAUDE_COUNT_TOKENS_MODEL_REQUIRED_DETAIL)
        );
        assert_eq!(
            validate_claude_count_tokens_request(Some(&Bytes::from_static(
                br#"{"model":"claude-sonnet-4-5"}"#,
            ))),
            Err(CLAUDE_COUNT_TOKENS_MESSAGES_REQUIRED_DETAIL)
        );
        assert_eq!(
            validate_claude_count_tokens_request(Some(&Bytes::from_static(
                br#"{"model":"claude-sonnet-4-5","messages":[],"tools":[{"name":"x"}]}"#,
            ))),
            Ok(())
        );
    }

    #[test]
    fn image_validation_accepts_custom_model_name() {
        let body =
            Bytes::from_static(br#"{"model":" Custom/Image-Model:V1 ","prompt":"draw an image"}"#);

        let validation = parse_openai_image_validation_input(
            OpenAiImageOperation::Generate,
            Some("application/json"),
            &body,
        )
        .expect("custom image model should validate");

        assert_eq!(validation.model.as_deref(), Some("Custom/Image-Model:V1"));
    }

    #[test]
    fn image_validation_accepts_multipart_with_mixed_case_boundary() {
        let boundary = "------------------------OYNWsMZCt0ILTwn8naP4Gb";
        let body = Bytes::from(format!(
            concat!(
                "--{boundary}\r\n",
                "Content-Disposition: form-data; name=\"model\"\r\n\r\n",
                "gpt-image-2\r\n",
                "--{boundary}\r\n",
                "Content-Disposition: form-data; name=\"prompt\"\r\n\r\n",
                "edit this image\r\n",
                "--{boundary}\r\n",
                "Content-Disposition: form-data; name=\"image\"; filename=\"image.jpg\"\r\n",
                "Content-Type: image/jpeg\r\n\r\n",
                "image-bytes\r\n",
                "--{boundary}--\r\n"
            ),
            boundary = boundary,
        ));

        let validation = parse_openai_image_validation_input(
            OpenAiImageOperation::Edit,
            Some(&format!("multipart/form-data; boundary={boundary}")),
            &body,
        )
        .expect("multipart image edit should validate");

        assert_eq!(validation.model.as_deref(), Some("gpt-image-2"));
        assert_eq!(validation.prompt.as_deref(), Some("edit this image"));
        assert_eq!(validation.image_count, 1);
    }

    #[test]
    fn image_validation_rejects_malformed_or_oversized_multipart_boundaries() {
        let body = Bytes::from_static(b"not-a-multipart-body");
        for content_type in [
            "multipart/form-data; boundary=bad boundary",
            "multipart/form-data; boundary=bad\"quote",
            "multipart/form-data; boundary=\"unterminated",
            "multipart/form-data; boundary=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ] {
            assert!(matches!(
                parse_openai_image_validation_input(
                    OpenAiImageOperation::Generate,
                    Some(content_type),
                    &body,
                ),
                Err(OPENAI_IMAGE_INVALID_MULTIPART_DETAIL)
            ));
        }
    }

    #[test]
    fn multipart_parser_caps_part_count_and_header_size() {
        let boundary = "bounded-parts";
        let mut accepted_body = Vec::new();
        for index in 0..MAX_MULTIPART_PARTS {
            accepted_body.extend_from_slice(
                format!(
                    "--{boundary}\r\nContent-Disposition: form-data; name=\"field-{index}\"\r\n\r\nvalue\r\n"
                )
                .as_bytes(),
            );
        }
        accepted_body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        assert_eq!(
            parse_multipart_fields(&accepted_body, boundary).len(),
            MAX_MULTIPART_PARTS
        );

        let mut body = Vec::new();
        for index in 0..(MAX_MULTIPART_PARTS + 1) {
            body.extend_from_slice(
                format!(
                    "--{boundary}\r\nContent-Disposition: form-data; name=\"field-{index}\"\r\n\r\nvalue\r\n"
                )
                .as_bytes(),
            );
        }
        body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        assert!(parse_multipart_fields(&body, boundary).is_empty());

        let mut oversized_header =
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"field\"; x=\"")
                .into_bytes();
        oversized_header.extend(std::iter::repeat_n(b'x', MAX_MULTIPART_PART_HEADER_BYTES));
        oversized_header
            .extend_from_slice(format!("\"\r\n\r\nvalue\r\n--{boundary}--\r\n").as_bytes());
        assert!(parse_multipart_fields(&oversized_header, boundary).is_empty());
    }

    #[test]
    fn multipart_parser_preserves_boundary_like_payload_and_fails_closed() {
        let boundary = "payload-boundary";
        let body = format!(
            concat!(
                "--{boundary}\r\n",
                "Content-Disposition: form-data; name=\"prompt\"\r\n\r\n",
                "prefix\r\n--{boundary}X\r\nsuffix--{boundary}\r\n",
                "--{boundary}--\r\n"
            ),
            boundary = boundary,
        );
        let fields = parse_multipart_fields(body.as_bytes(), boundary);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].name, "prompt");
        assert_eq!(
            fields[0].data,
            format!("prefix\r\n--{boundary}X\r\nsuffix--{boundary}").into_bytes()
        );

        // A valid first part must not make a truncated second part appear
        // valid.  The only marker after the second part has an invalid
        // suffix and there is no closing boundary.
        let malformed = format!(
            concat!(
                "--{boundary}\r\n",
                "Content-Disposition: form-data; name=\"first\"\r\n\r\n",
                "ok\r\n",
                "--{boundary}\r\n",
                "Content-Disposition: form-data; name=\"second\"\r\n\r\n",
                "truncated\r\n--{boundary}X\r\n"
            ),
            boundary = boundary,
        );
        assert!(parse_multipart_fields(malformed.as_bytes(), boundary).is_empty());
    }

    #[test]
    fn multipart_parser_does_not_extract_name_from_filename_and_rejects_duplicates() {
        let boundary = "header-parameters";
        let filename_only = format!(
            concat!(
                "--{boundary}\r\n",
                "Content-Disposition: form-data; filename=\"name=\\\"prompt\\\"\"\r\n\r\n",
                "attacker-value\r\n",
                "--{boundary}--\r\n"
            ),
            boundary = boundary,
        );
        assert!(parse_multipart_fields(filename_only.as_bytes(), boundary).is_empty());

        let duplicate_name = format!(
            concat!(
                "--{boundary}\r\n",
                "Content-Disposition: form-data; name=\"prompt\"; name=\"image\"\r\n\r\n",
                "ambiguous-value\r\n",
                "--{boundary}--\r\n"
            ),
            boundary = boundary,
        );
        assert!(parse_multipart_fields(duplicate_name.as_bytes(), boundary).is_empty());
    }

    #[test]
    fn multipart_parser_rejects_garbage_after_closing_boundary() {
        let boundary = "closing-suffix";
        let body = format!(
            concat!(
                "--{boundary}\r\n",
                "Content-Disposition: form-data; name=\"prompt\"\r\n\r\n",
                "value\r\n",
                "--{boundary}--junk"
            ),
            boundary = boundary,
        );
        assert!(parse_multipart_fields(body.as_bytes(), boundary).is_empty());
    }

    #[test]
    fn image_validation_applies_the_global_count_limit_before_model_mapping() {
        let openai_body = Bytes::from_static(br#"{"model":"gpt-image-2","prompt":"draw","n":2}"#);
        let openai_validation = parse_openai_image_validation_input(
            OpenAiImageOperation::Generate,
            Some("application/json"),
            &openai_body,
        )
        .expect("valid image payload should parse");

        assert!(validate_openai_image_n(&openai_validation).is_none());

        let grok_body =
            Bytes::from_static(br#"{"model":"grok-imagine-image-lite","prompt":"draw","n":4}"#);
        let grok_validation = parse_openai_image_validation_input(
            OpenAiImageOperation::Generate,
            Some("application/json"),
            &grok_body,
        )
        .expect("valid grok image payload should parse");

        assert!(validate_openai_image_n(&grok_validation).is_none());

        let alias_body =
            Bytes::from_static(br#"{"model":"production-image-alias","prompt":"draw","n":10}"#);
        let alias_validation = parse_openai_image_validation_input(
            OpenAiImageOperation::Generate,
            Some("application/json"),
            &alias_body,
        )
        .expect("valid image alias payload should parse");
        assert!(validate_openai_image_n(&alias_validation).is_none());

        let excessive_body =
            Bytes::from_static(br#"{"model":"production-image-alias","prompt":"draw","n":11}"#);
        let excessive_validation = parse_openai_image_validation_input(
            OpenAiImageOperation::Generate,
            Some("application/json"),
            &excessive_body,
        )
        .expect("image payload should parse before count validation");
        assert_eq!(
            validate_openai_image_n(&excessive_validation).as_deref(),
            Some("当前图片反代仅支持 n=1..10")
        );
    }
}
