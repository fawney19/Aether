use crate::async_task::CancelVideoTaskError;
use crate::control::GatewayControlDecision;
use crate::control::GatewayPublicRequestContext;
use crate::{AppState, GatewayError};
use aether_data_contracts::repository::video_tasks::{
    StoredVideoTask, VideoTaskQueryFilter, VideoTaskStatus,
};
use axum::body::{Body, Bytes};
use axum::http::{self, Response};
use axum::response::IntoResponse;
use axum::Json;
use serde_json::{json, Value};

const CLAUDE_COUNT_TOKENS_INVALID_PAYLOAD_DETAIL: &str = "Invalid token count payload";
const CLAUDE_COUNT_TOKENS_MISSING_BODY_DETAIL: &str = "请求体不能为空";
const GEMINI_VIDEO_TASK_NOT_FOUND_DETAIL: &str = "Video task not found";
const AI_PUBLIC_METHOD_NOT_ALLOWED_DETAIL: &str = "Method not allowed";
const AI_PUBLIC_UNAUTHORIZED_DETAIL: &str = "Unauthorized";
const OPENAI_IMAGE_PROMPT_DETAIL: &str = "图片生成/编辑请求缺少 prompt";
const OPENAI_IMAGE_EDIT_INPUT_DETAIL: &str = "图片编辑请求至少需要 1 张输入图片";
const OPENAI_IMAGE_VARIATION_INPUT_DETAIL: &str = "图片变体请求需要 image 文件";
const OPENAI_IMAGE_N_DETAIL: &str = "当前 Codex 图片反代仅支持 n=1";
const OPENAI_IMAGE_STREAM_VARIATION_DETAIL: &str = "图片变体接口当前仅支持同步响应";
const OPENAI_IMAGE_PARTIAL_IMAGES_DETAIL: &str =
    "partial_images 仅支持 0-3，且必须配合 stream=true";
const OPENAI_IMAGE_STYLE_DETAIL: &str = "当前 Codex 图片反代暂不支持 style 参数";
const OPENAI_IMAGE_RESPONSE_FORMAT_DETAIL: &str = "response_format 仅支持 url 或 b64_json";
const OPENAI_IMAGE_OUTPUT_FORMAT_DETAIL: &str = "output_format 仅支持 png、jpeg 或 webp";
const OPENAI_IMAGE_QUALITY_DETAIL: &str = "quality 仅支持 low、medium、high、standard 或 hd";
const OPENAI_IMAGE_BACKGROUND_DETAIL: &str = "background 仅支持 auto、opaque 或 transparent";
const OPENAI_IMAGE_MODERATION_DETAIL: &str = "moderation 仅支持 auto 或 low";
const OPENAI_IMAGE_INPUT_FIDELITY_DETAIL: &str = "input_fidelity 仅支持 low 或 high";
const OPENAI_IMAGE_OUTPUT_COMPRESSION_DETAIL: &str = "output_compression 必须是 0-100 的整数";
const OPENAI_IMAGE_INVALID_JSON_DETAIL: &str = "图片接口 JSON 请求体无效";
const OPENAI_IMAGE_INVALID_MULTIPART_DETAIL: &str = "图片接口 multipart/form-data 请求体无效";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OpenAiImageOperation {
    Generate,
    Edit,
    Variation,
}

impl OpenAiImageOperation {
    fn from_path(path: &str) -> Option<Self> {
        match path {
            "/v1/images/generations" => Some(Self::Generate),
            "/v1/images/edits" => Some(Self::Edit),
            "/v1/images/variations" => Some(Self::Variation),
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
                && decision.route_family.as_deref() == Some("claude")
                && decision.route_kind.as_deref() == Some("count_tokens")
                && request_context.request_method == http::Method::POST
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
        maybe_build_local_claude_count_tokens_response(request_context, request_body)
    {
        return Some(response);
    }

    maybe_build_local_gemini_video_operations_response(state, request_context, decision).await
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

    let request_body = request_body?;

    if decision.route_kind.as_deref() == Some("chat")
        && request_context.request_path == "/v1/chat/completions"
    {
        return None;
    }

    if decision.route_kind.as_deref() != Some("image")
        || !matches!(
            request_context.request_path.as_str(),
            "/v1/images/generations" | "/v1/images/edits" | "/v1/images/variations"
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
        OpenAiImageOperation::Variation if validation.image_count == 0 => {
            return Some(build_ai_public_error_response(
                http::StatusCode::BAD_REQUEST,
                OPENAI_IMAGE_VARIATION_INPUT_DETAIL,
            ));
        }
        _ => {}
    }

    if validation.n.is_some_and(|value| value != 1) {
        return Some(build_ai_public_error_response(
            http::StatusCode::BAD_REQUEST,
            OPENAI_IMAGE_N_DETAIL,
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

    if validation.stream {
        if operation == OpenAiImageOperation::Variation {
            return Some(build_ai_public_error_response(
                http::StatusCode::BAD_REQUEST,
                OPENAI_IMAGE_STREAM_VARIATION_DETAIL,
            ));
        }
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
        .is_some_and(|value| !matches!(value, "low" | "medium" | "high" | "standard" | "hd"))
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
            OpenAiImageOperation::Variation => OPENAI_IMAGE_VARIATION_INPUT_DETAIL,
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
    let boundary = multipart_boundary(content_type).ok_or(OPENAI_IMAGE_INVALID_MULTIPART_DETAIL)?;
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

    while let Some(index) = find_subslice(&body[cursor..], &delimiter) {
        let start = cursor + index + delimiter.len();
        if body.get(start..start + 2) == Some(b"--") {
            break;
        }
        let mut part = &body[start..];
        if part.starts_with(b"\r\n") {
            part = &part[2..];
        }
        let Some(next) = find_subslice(part, &delimiter) else {
            break;
        };
        let raw = &part[..next];
        let raw = raw.strip_suffix(b"\r\n").unwrap_or(raw);
        if let Some(field) = parse_multipart_field(raw) {
            parts.push(field);
        }
        cursor = start + next;
    }

    parts
}

fn multipart_boundary(content_type: &str) -> Option<String> {
    content_type.split(';').find_map(|segment| {
        let (key, value) = segment.trim().split_once('=')?;
        if !key.trim().eq_ignore_ascii_case("boundary") {
            return None;
        }
        let boundary = value.trim().trim_matches('"').trim();
        (!boundary.is_empty()).then(|| boundary.to_string())
    })
}

fn parse_multipart_field(raw: &[u8]) -> Option<MultipartField> {
    let header_end = find_subslice(raw, b"\r\n\r\n")?;
    let headers = &raw[..header_end];
    let data = raw.get(header_end + 4..)?.to_vec();
    let header_text = String::from_utf8_lossy(headers);

    let mut name = None;
    for line in header_text.lines() {
        let trimmed = line.trim();
        if trimmed
            .to_ascii_lowercase()
            .starts_with("content-disposition:")
        {
            name = extract_quoted_header_value(trimmed, "name");
        }
    }

    Some(MultipartField { name: name?, data })
}

fn extract_quoted_header_value(header: &str, key: &str) -> Option<String> {
    let pattern = format!("{key}=\"");
    let start = header.find(&pattern)? + pattern.len();
    let rest = &header[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
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

fn maybe_build_local_claude_count_tokens_response(
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

    let Some(request_body) = request_body else {
        return Some(build_ai_public_error_response(
            http::StatusCode::BAD_REQUEST,
            CLAUDE_COUNT_TOKENS_MISSING_BODY_DETAIL,
        ));
    };

    let payload = match serde_json::from_slice::<serde_json::Value>(request_body) {
        Ok(payload) => payload,
        Err(_) => {
            return Some(build_ai_public_error_response(
                http::StatusCode::BAD_REQUEST,
                CLAUDE_COUNT_TOKENS_INVALID_PAYLOAD_DETAIL,
            ));
        }
    };

    let input_tokens = match estimate_claude_count_tokens(&payload) {
        Ok(tokens) => tokens,
        Err(_) => {
            return Some(build_ai_public_error_response(
                http::StatusCode::BAD_REQUEST,
                CLAUDE_COUNT_TOKENS_INVALID_PAYLOAD_DETAIL,
            ));
        }
    };

    Some(Json(json!({ "input_tokens": input_tokens })).into_response())
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
    let Some(user_id) = decision
        .auth_context
        .as_ref()
        .map(|auth_context| auth_context.user_id.trim())
        .filter(|value| !value.is_empty())
    else {
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
        Err(err) => {
            return build_ai_public_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("{err:?}"),
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
            Err(err) => {
                return build_ai_public_error_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("{err:?}"),
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
    let task =
        match find_user_gemini_video_task_for_operation(state, decision, operation_path).await {
            Ok(Some(task)) => task,
            Ok(None) => {
                return build_ai_public_error_response(
                    http::StatusCode::NOT_FOUND,
                    GEMINI_VIDEO_TASK_NOT_FOUND_DETAIL,
                );
            }
            Err(err) => {
                return build_ai_public_error_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("{err:?}"),
                );
            }
        };

    match crate::async_task::cancel_video_task_record(state, &task.id).await {
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
        Err(CancelVideoTaskError::Response(response)) => response,
        Err(CancelVideoTaskError::Gateway(err)) => build_ai_public_error_response(
            http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("{err:?}"),
        ),
    }
}

async fn find_user_gemini_video_task_for_operation(
    state: &AppState,
    decision: &GatewayControlDecision,
    operation_path: &str,
) -> Result<Option<StoredVideoTask>, GatewayError> {
    let Some(user_id) = decision
        .auth_context
        .as_ref()
        .map(|auth_context| auth_context.user_id.trim())
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    let Some(short_id) = extract_short_id_from_gemini_operation_path(operation_path) else {
        return Ok(None);
    };
    let Some(task) = state.find_video_task_by_short_id(short_id).await? else {
        return Ok(None);
    };
    if task.user_id.as_deref().map(str::trim) != Some(user_id) || !is_gemini_video_task(&task) {
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
    matches!(
        task.provider_api_format
            .as_deref()
            .or(task.client_api_format.as_deref())
            .map(str::trim),
        Some("gemini:video")
    )
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
            "error": {
                "code": task.error_code.clone().unwrap_or_else(|| "UNKNOWN".to_string()),
                "message": task
                    .error_message
                    .clone()
                    .unwrap_or_else(|| "Video generation failed".to_string()),
            }
        }),
        _ => json!({
            "name": gemini_video_operation_name(task),
            "done": false,
            "metadata": gemini_video_operation_metadata(task),
        }),
    }
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
    (status, Json(json!({ "detail": detail.into() }))).into_response()
}

fn estimate_claude_count_tokens(payload: &serde_json::Value) -> Result<u64, ()> {
    let object = payload.as_object().ok_or(())?;
    let model = object
        .get("model")
        .and_then(serde_json::Value::as_str)
        .ok_or(())?;
    if model.trim().is_empty() {
        return Err(());
    }

    let messages = object
        .get("messages")
        .and_then(serde_json::Value::as_array)
        .ok_or(())?;

    let system_tokens = estimate_claude_system_tokens(object.get("system"))?;
    let message_tokens = estimate_claude_message_tokens(messages)?;
    Ok(system_tokens.saturating_add(message_tokens))
}

fn estimate_claude_system_tokens(system: Option<&serde_json::Value>) -> Result<u64, ()> {
    let Some(system) = system else {
        return Ok(0);
    };

    match system {
        serde_json::Value::Null => Ok(0),
        serde_json::Value::String(text) => Ok(estimate_text_tokens(text)),
        serde_json::Value::Array(blocks) => {
            let mut total = 0_u64;
            for block in blocks {
                let block = block.as_object().ok_or(())?;
                if let Some(text) = block.get("text").and_then(serde_json::Value::as_str) {
                    total = total.saturating_add(estimate_text_tokens(text));
                }
            }
            Ok(total)
        }
        serde_json::Value::Object(_) => Ok(0),
        _ => Err(()),
    }
}

fn estimate_claude_message_tokens(messages: &[serde_json::Value]) -> Result<u64, ()> {
    let mut total = 0_u64;

    for message in messages {
        let message = message.as_object().ok_or(())?;
        let role = message
            .get("role")
            .and_then(serde_json::Value::as_str)
            .ok_or(())?;
        if !matches!(role, "user" | "assistant") {
            return Err(());
        }

        total = total.saturating_add(4);
        let content = message.get("content").ok_or(())?;
        match content {
            serde_json::Value::String(text) => {
                total = total.saturating_add(estimate_text_tokens(text));
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    let item = item.as_object().ok_or(())?;
                    if let Some(text) = item.get("text").and_then(serde_json::Value::as_str) {
                        total = total.saturating_add(estimate_text_tokens(text));
                    }
                }
            }
            _ => return Err(()),
        }
    }

    Ok(total)
}

fn estimate_text_tokens(text: &str) -> u64 {
    if text.is_empty() {
        return 0;
    }

    let char_count = text.chars().count() as u64;
    std::cmp::max(1, char_count / 4)
}

#[cfg(test)]
mod tests {
    use super::{
        estimate_claude_count_tokens, parse_openai_image_validation_input, OpenAiImageOperation,
    };
    use axum::body::Bytes;
    use serde_json::json;

    #[test]
    fn estimates_claude_count_tokens_from_system_and_messages() {
        let payload = json!({
            "model": "claude-sonnet-4-5",
            "system": [{"type": "text", "text": "abcdefghijklmnop"}],
            "messages": [
                {
                    "role": "user",
                    "content": "abcdefghijkl"
                },
                {
                    "role": "assistant",
                    "content": [
                        {"type": "text", "text": "abcdefgh"},
                        {"type": "tool_use", "name": "ignored", "input": {"city": "SF"}}
                    ]
                }
            ]
        });

        assert_eq!(estimate_claude_count_tokens(&payload), Ok(17));
    }

    #[test]
    fn rejects_invalid_claude_count_tokens_payload() {
        let payload = json!({
            "model": "claude-sonnet-4-5",
            "messages": [{"role": "system", "content": "bad"}]
        });

        assert_eq!(estimate_claude_count_tokens(&payload), Err(()));
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
}
