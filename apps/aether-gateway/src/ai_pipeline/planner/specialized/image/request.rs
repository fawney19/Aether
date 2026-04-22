use std::collections::BTreeMap;
use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Map, Value};

use crate::ai_pipeline::planner::candidate_preparation::{
    prepare_header_authenticated_candidate, OauthPreparationContext,
};
use crate::ai_pipeline::planner::spec_metadata::local_openai_image_spec_metadata;
use crate::ai_pipeline::transport::auth::{
    build_passthrough_headers_with_auth, resolve_local_openai_bearer_auth,
};
use crate::ai_pipeline::transport::url::build_openai_cli_url;
use crate::ai_pipeline::transport::{
    apply_local_header_rules, local_standard_transport_unsupported_reason_with_network,
};
use crate::ai_pipeline::{
    apply_codex_openai_cli_special_body_edits, apply_codex_openai_cli_special_headers,
    GatewayProviderTransportSnapshot, PlannerAppState,
};
use crate::AppState;

use super::support::{
    mark_skipped_local_openai_image_candidate, LocalOpenAiImageCandidateAttempt,
    LocalOpenAiImageDecisionInput, OPENAI_IMAGE_DEFAULT_MODEL,
};
use super::LocalOpenAiImageSpec;

const OPENAI_IMAGE_INTERNAL_MODEL: &str = "gpt-5.4";

pub(super) struct LocalOpenAiImageCandidatePayloadParts {
    pub(super) transport: Arc<GatewayProviderTransportSnapshot>,
    pub(super) auth_header: String,
    pub(super) auth_value: String,
    pub(super) requested_model: String,
    pub(super) mapped_model: String,
    pub(super) provider_request_headers: BTreeMap<String, String>,
    pub(super) provider_request_body: Value,
    pub(super) upstream_url: String,
    pub(super) input_summary: Value,
}

pub(super) async fn resolve_local_openai_image_candidate_payload_parts(
    state: &AppState,
    parts: &http::request::Parts,
    body_json: &Value,
    body_base64: Option<&str>,
    trace_id: &str,
    input: &LocalOpenAiImageDecisionInput,
    attempt: &LocalOpenAiImageCandidateAttempt,
    spec: LocalOpenAiImageSpec,
) -> Option<LocalOpenAiImageCandidatePayloadParts> {
    let spec_metadata = local_openai_image_spec_metadata(spec);
    let candidate = &attempt.eligible.candidate;
    let transport = &attempt.eligible.transport;

    if let Some(skip_reason) = local_standard_transport_unsupported_reason_with_network(
        transport,
        spec_metadata.api_format,
    ) {
        mark_skipped_local_openai_image_candidate(
            state,
            input,
            trace_id,
            candidate,
            attempt.candidate_index,
            &attempt.candidate_id,
            skip_reason,
        )
        .await;
        return None;
    }

    let prepared_candidate = match prepare_header_authenticated_candidate(
        PlannerAppState::new(state),
        transport,
        candidate,
        resolve_local_openai_bearer_auth(transport),
        OauthPreparationContext {
            trace_id,
            api_format: spec_metadata.api_format,
            operation: "openai_image_candidate_request",
        },
    )
    .await
    {
        Ok(prepared) => prepared,
        Err(skip_reason) => {
            mark_skipped_local_openai_image_candidate(
                state,
                input,
                trace_id,
                candidate,
                attempt.candidate_index,
                &attempt.candidate_id,
                skip_reason,
            )
            .await;
            return None;
        }
    };
    let auth_header = prepared_candidate.auth_header;
    let auth_value = prepared_candidate.auth_value;

    let Some(normalized_request) =
        normalize_openai_image_request(parts, body_json, body_base64).await
    else {
        mark_skipped_local_openai_image_candidate(
            state,
            input,
            trace_id,
            candidate,
            attempt.candidate_index,
            &attempt.candidate_id,
            "provider_request_body_missing",
        )
        .await;
        return None;
    };

    let upstream_url = build_openai_cli_url(&transport.endpoint.base_url, parts.uri.query(), false);
    let mut provider_request_body = build_provider_request_body(&normalized_request);
    apply_codex_openai_cli_special_body_edits(
        &mut provider_request_body,
        transport.provider.provider_type.as_str(),
        spec_metadata.api_format,
        transport.endpoint.body_rules.as_ref(),
        Some(candidate.key_id.as_str()),
    );

    let mut provider_request_headers = build_passthrough_headers_with_auth(
        &parts.headers,
        &auth_header,
        &auth_value,
        &BTreeMap::new(),
    );
    provider_request_headers.insert("content-type".to_string(), "application/json".to_string());
    provider_request_headers.insert("accept".to_string(), "text/event-stream".to_string());
    if !apply_local_header_rules(
        &mut provider_request_headers,
        transport.endpoint.header_rules.as_ref(),
        &[&auth_header, "content-type", "accept"],
        &provider_request_body,
        Some(body_json),
    ) {
        mark_skipped_local_openai_image_candidate(
            state,
            input,
            trace_id,
            candidate,
            attempt.candidate_index,
            &attempt.candidate_id,
            "transport_header_rules_apply_failed",
        )
        .await;
        return None;
    }
    apply_codex_openai_cli_special_headers(
        &mut provider_request_headers,
        &provider_request_body,
        &parts.headers,
        transport.provider.provider_type.as_str(),
        spec_metadata.api_format,
        Some(trace_id),
        transport.key.decrypted_auth_config.as_deref(),
    );

    Some(LocalOpenAiImageCandidatePayloadParts {
        transport: Arc::clone(transport),
        auth_header,
        auth_value,
        requested_model: normalized_request.requested_model,
        mapped_model: OPENAI_IMAGE_INTERNAL_MODEL.to_string(),
        provider_request_headers,
        provider_request_body,
        upstream_url,
        input_summary: normalized_request.summary_json,
    })
}

#[derive(Clone, Debug)]
struct NormalizedOpenAiImageRequest {
    requested_model: String,
    prompt: String,
    images: Vec<Value>,
    mask: Option<Value>,
    tool: Map<String, Value>,
    response_format: String,
    user: Option<String>,
    summary_json: Value,
}

fn build_provider_request_body(request: &NormalizedOpenAiImageRequest) -> Value {
    let generation_only = request.images.is_empty() && request.mask.is_none();
    let input = if generation_only {
        json!([{
            "role": "user",
            "content": request.prompt,
        }])
    } else {
        let mut content = Vec::new();
        content.push(json!({
            "type": "input_text",
            "text": request.prompt,
        }));
        content.extend(request.images.iter().cloned());
        if let Some(mask) = request.mask.as_ref() {
            content.push(mask.clone());
        }
        json!([{
            "role": "user",
            "content": content,
        }])
    };

    let mut body = Map::new();
    body.insert(
        "model".to_string(),
        Value::String(OPENAI_IMAGE_INTERNAL_MODEL.to_string()),
    );
    body.insert("input".to_string(), input);
    body.insert(
        "tools".to_string(),
        Value::Array(vec![Value::Object(request.tool.clone())]),
    );
    body.insert("tool_choice".to_string(), Value::String("auto".to_string()));
    body.insert(
        "instructions".to_string(),
        Value::String("you are a helpful assistant".to_string()),
    );
    body.insert("stream".to_string(), Value::Bool(true));
    body.insert("store".to_string(), Value::Bool(false));
    if let Some(user) = request.user.as_ref() {
        body.insert("user".to_string(), Value::String(user.clone()));
    }
    Value::Object(body)
}

async fn normalize_openai_image_request(
    parts: &http::request::Parts,
    body_json: &Value,
    body_base64: Option<&str>,
) -> Option<NormalizedOpenAiImageRequest> {
    if body_base64.is_some() {
        normalize_openai_image_multipart_request(parts, body_base64).await
    } else {
        normalize_openai_image_json_request(body_json)
    }
}

fn normalize_openai_image_json_request(body_json: &Value) -> Option<NormalizedOpenAiImageRequest> {
    let object = body_json.as_object()?;
    let requested_model = normalize_requested_image_model(object.get("model"))?;
    let prompt = object
        .get("prompt")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default()
        .to_string();
    let response_format = normalize_image_response_format(object.get("response_format"))?;
    let user = object
        .get("user")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);

    let mut images = Vec::new();
    if let Some(image) = object.get("image") {
        images.extend(normalize_image_value(image));
    }
    if let Some(value) = object.get("images").and_then(Value::as_array) {
        for image in value {
            images.extend(normalize_image_value(image));
        }
    }
    let mask = object.get("mask").and_then(normalize_mask_value);

    let mut tool = build_tool_options(object);
    if !images.is_empty() || mask.is_some() {
        tool.insert("action".to_string(), Value::String("edit".to_string()));
    }
    if let Some(mask) = mask.as_ref() {
        tool.insert("mask".to_string(), mask_payload(mask));
    }

    Some(NormalizedOpenAiImageRequest {
        requested_model,
        prompt: if prompt.is_empty() {
            "Generate an image.".to_string()
        } else {
            prompt
        },
        images,
        mask,
        tool,
        response_format: response_format.clone(),
        user,
        summary_json: json!({
            "operation": if object.contains_key("image") || object.contains_key("images") || object.contains_key("mask") { "edit" } else { "generate" },
            "response_format": response_format,
        }),
    })
}

async fn normalize_openai_image_multipart_request(
    parts: &http::request::Parts,
    body_base64: Option<&str>,
) -> Option<NormalizedOpenAiImageRequest> {
    let body_base64 = body_base64?.trim();
    if body_base64.is_empty() {
        return None;
    }
    let content_type = parts
        .headers
        .get(http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())?;
    let boundary = content_type
        .split(';')
        .find_map(|segment| segment.trim().strip_prefix("boundary="))?
        .trim_matches('"')
        .to_string();
    let body_bytes = base64::engine::general_purpose::STANDARD
        .decode(body_base64)
        .ok()?;
    let mut requested_model = OPENAI_IMAGE_DEFAULT_MODEL.to_string();
    let mut prompt = String::new();
    let mut response_format = "b64_json".to_string();
    let mut user = None;
    let mut tool_fields = Map::new();
    let mut images = Vec::new();
    let mut mask = None;

    for field in parse_multipart_fields(&body_bytes, boundary.as_str()) {
        let name = field.name.trim().to_string();
        if name.is_empty() {
            continue;
        }
        if matches!(name.as_str(), "image" | "images[]") {
            let content_type = field
                .content_type
                .clone()
                .unwrap_or_else(|| "application/octet-stream".to_string());
            images.push(json!({
                "type": "input_image",
                "image_url": format!(
                    "data:{};base64,{}",
                    content_type,
                    base64::engine::general_purpose::STANDARD.encode(&field.data),
                ),
            }));
            continue;
        }
        if name == "mask" {
            let content_type = field
                .content_type
                .clone()
                .unwrap_or_else(|| "application/octet-stream".to_string());
            let value = json!({
                "type": "input_image",
                "image_url": format!(
                    "data:{};base64,{}",
                    content_type,
                    base64::engine::general_purpose::STANDARD.encode(&field.data),
                ),
            });
            mask = Some(value);
            continue;
        }

        let value = String::from_utf8_lossy(&field.data).trim().to_string();
        match name.as_str() {
            "model" => {
                requested_model = normalize_requested_image_model(Some(&Value::String(value)))?
            }
            "prompt" => prompt = value,
            "response_format" => {
                response_format =
                    normalize_image_response_format(Some(&Value::String(value.clone())))?
            }
            "user" => {
                user = (!value.is_empty()).then_some(value);
            }
            "size" | "quality" | "background" | "output_format" | "output_compression"
            | "moderation" => {
                tool_fields.insert(
                    name,
                    if let Ok(number) = value.parse::<u64>() {
                        Value::Number(number.into())
                    } else {
                        Value::String(value)
                    },
                );
            }
            _ => {}
        }
    }

    let mut tool = build_tool_options_from_map(tool_fields);
    tool.insert("action".to_string(), Value::String("edit".to_string()));
    if let Some(mask) = mask.as_ref() {
        tool.insert("mask".to_string(), mask_payload(mask));
    }

    Some(NormalizedOpenAiImageRequest {
        requested_model,
        prompt: if prompt.is_empty() {
            "Edit the provided image.".to_string()
        } else {
            prompt
        },
        images,
        mask,
        tool,
        response_format: response_format.clone(),
        user,
        summary_json: json!({
            "operation": "edit",
            "response_format": response_format,
        }),
    })
}

fn normalize_requested_image_model(value: Option<&Value>) -> Option<String> {
    let model = value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(OPENAI_IMAGE_DEFAULT_MODEL);
    (model.eq_ignore_ascii_case(OPENAI_IMAGE_DEFAULT_MODEL))
        .then(|| OPENAI_IMAGE_DEFAULT_MODEL.to_string())
}

fn normalize_image_response_format(value: Option<&Value>) -> Option<String> {
    let response_format = value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("b64_json");
    (response_format.eq_ignore_ascii_case("b64_json")).then(|| "b64_json".to_string())
}

fn build_tool_options(object: &Map<String, Value>) -> Map<String, Value> {
    let mut tool = Map::new();
    tool.insert(
        "type".to_string(),
        Value::String("image_generation".to_string()),
    );
    for key in [
        "size",
        "quality",
        "background",
        "output_format",
        "output_compression",
        "moderation",
    ] {
        if let Some(value) = object.get(key) {
            tool.insert(key.to_string(), value.clone());
        }
    }
    tool
}

fn build_tool_options_from_map(mut tool: Map<String, Value>) -> Map<String, Value> {
    tool.insert(
        "type".to_string(),
        Value::String("image_generation".to_string()),
    );
    tool
}

fn normalize_image_value(value: &Value) -> Vec<Value> {
    match value {
        Value::Array(values) => values.iter().flat_map(normalize_image_value).collect(),
        Value::String(url) => {
            let url = url.trim();
            if url.is_empty() {
                Vec::new()
            } else {
                vec![json!({
                    "type": "input_image",
                    "image_url": url,
                })]
            }
        }
        Value::Object(object) => {
            if let Some(file_id) = object.get("file_id").and_then(Value::as_str) {
                return vec![json!({
                    "type": "input_image",
                    "file_id": file_id,
                })];
            }
            if let Some(image_url) = object
                .get("image_url")
                .and_then(Value::as_str)
                .or_else(|| object.get("url").and_then(Value::as_str))
            {
                return vec![json!({
                    "type": "input_image",
                    "image_url": image_url,
                })];
            }
            if let Some(b64_json) = object.get("b64_json").and_then(Value::as_str) {
                let mime_type = object
                    .get("mime_type")
                    .and_then(Value::as_str)
                    .unwrap_or("image/png");
                return vec![json!({
                    "type": "input_image",
                    "image_url": format!("data:{};base64,{}", mime_type, b64_json),
                })];
            }
            Vec::new()
        }
        _ => Vec::new(),
    }
}

fn normalize_mask_value(value: &Value) -> Option<Value> {
    normalize_image_value(value).into_iter().next()
}

fn mask_payload(mask: &Value) -> Value {
    mask.as_object()
        .and_then(|object| {
            object
                .get("file_id")
                .cloned()
                .map(|file_id| json!({ "file_id": file_id }))
                .or_else(|| {
                    object
                        .get("image_url")
                        .cloned()
                        .map(|image_url| json!({ "image_url": image_url }))
                })
        })
        .unwrap_or_else(|| mask.clone())
}

#[derive(Debug)]
struct MultipartField {
    name: String,
    #[allow(dead_code)]
    filename: Option<String>,
    content_type: Option<String>,
    data: Vec<u8>,
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

fn parse_multipart_field(raw: &[u8]) -> Option<MultipartField> {
    let header_end = find_subslice(raw, b"\r\n\r\n")?;
    let headers = &raw[..header_end];
    let data = raw.get(header_end + 4..)?.to_vec();
    let header_text = String::from_utf8_lossy(headers);

    let mut name = None;
    let mut filename = None;
    let mut content_type = None;
    for line in header_text.lines() {
        let trimmed = line.trim();
        let lower = trimmed.to_ascii_lowercase();
        if lower.starts_with("content-disposition:") {
            name = extract_quoted_header_value(trimmed, "name");
            filename = extract_quoted_header_value(trimmed, "filename");
        } else if lower.starts_with("content-type:") {
            content_type = trimmed
                .split_once(':')
                .map(|(_, value)| value.trim().to_string())
                .filter(|value| !value.is_empty());
        }
    }

    Some(MultipartField {
        name: name?,
        filename,
        content_type,
        data,
    })
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
