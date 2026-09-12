use std::collections::BTreeMap;

use base64::Engine as _;
use serde_json::{json, Map, Number, Value};

use crate::formats::openai::responses::codex::CODEX_OPENAI_IMAGE_DEFAULT_MODEL;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenAiImageOperation {
    Generate,
    Edit,
}

impl OpenAiImageOperation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Generate => "generate",
            Self::Edit => "edit",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenAiImageResponseFormat {
    B64Json,
    Url,
}

impl OpenAiImageResponseFormat {
    fn as_str(self) -> &'static str {
        match self {
            Self::B64Json => "b64_json",
            Self::Url => "url",
        }
    }
}

#[derive(Clone, Debug)]
pub struct NormalizedOpenAiImageRequest {
    pub operation: OpenAiImageOperation,
    pub requested_model: Option<String>,
    pub summary_json: Value,
    prompt: Option<String>,
    images: Vec<Value>,
    tool: Map<String, Value>,
    image_count: Option<u64>,
    max_generation_count: u64,
    user: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpenAiImageNormalizeOptions {
    max_generation_count: u64,
}

impl Default for OpenAiImageNormalizeOptions {
    fn default() -> Self {
        Self {
            max_generation_count: OPENAI_IMAGE_MAX_GENERATION_COUNT,
        }
    }
}

impl OpenAiImageNormalizeOptions {
    pub fn with_max_generation_count(max_generation_count: u64) -> Self {
        Self {
            max_generation_count: max_generation_count.max(1),
        }
    }
}

pub const CHATGPT_WEB_IMAGE_MAX_AREA: u64 = 1_500_000;
pub const OPENAI_IMAGE_MAX_GENERATION_COUNT: u64 = 10;
const MAX_MULTIPART_PARTS: usize = 128;
const MAX_MULTIPART_PART_HEADER_BYTES: usize = 64 * 1024;
const MAX_MULTIPART_BODY_BYTES: usize = 256 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatGptWebImageRequestError {
    pub status_code: u16,
    pub error_type: &'static str,
    pub code: &'static str,
    pub message: String,
}

impl ChatGptWebImageRequestError {
    fn invalid_request(message: impl Into<String>) -> Self {
        Self {
            status_code: 400,
            error_type: "invalid_request_error",
            code: "chatgpt_web_image_unsupported",
            message: message.into(),
        }
    }

    pub fn to_error_json(&self) -> Value {
        json!({
            "error": {
                "message": self.message,
                "type": self.error_type,
                "code": self.code,
                "param": Value::Null,
            }
        })
    }
}

#[derive(Debug, Clone, Default)]
struct ChatGptWebRawImageFields {
    size: Option<String>,
    resolution: Option<String>,
    size_tier: Option<String>,
    ratio: Option<String>,
    aspect_ratio: Option<String>,
    web_model: Option<String>,
}

#[derive(Debug, Clone)]
struct ChatGptWebResolvedSize {
    size: String,
    ratio: String,
    best_effort: bool,
}

pub fn build_chatgpt_web_image_request_body(
    parts: &http::request::Parts,
    body_json: &Value,
    body_base64: Option<&str>,
) -> Result<Value, ChatGptWebImageRequestError> {
    let request = normalize_openai_image_request(parts, body_json, body_base64).ok_or_else(|| {
        ChatGptWebImageRequestError::invalid_request(
            "ChatGPT-Web image proxy only supports OpenAI image requests with prompt, n=1, supported image inputs, and supported output options",
        )
    })?;
    if request.tool.contains_key("input_image_mask") {
        return Err(ChatGptWebImageRequestError::invalid_request(
            "ChatGPT-Web image proxy does not support mask inputs",
        ));
    }

    let raw_fields = resolve_chatgpt_web_raw_image_fields(parts, body_json, body_base64)?;
    let resolved_size = resolve_chatgpt_web_size(&raw_fields)?;
    let prompt = request
        .prompt
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(match request.operation {
            OpenAiImageOperation::Generate | OpenAiImageOperation::Edit => {
                "Generate a high quality image."
            }
        });
    let prompt = chatgpt_web_prompt_with_ratio(prompt, resolved_size.ratio.as_str());
    let mut image_urls = Vec::new();
    for image in &request.images {
        let Some(object) = image.as_object() else {
            continue;
        };
        if object
            .get("file_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .is_some_and(|value| !value.is_empty())
        {
            return Err(ChatGptWebImageRequestError::invalid_request(
                "ChatGPT-Web image proxy does not support file_id image inputs; use URL, data URL, or multipart file inputs",
            ));
        }
        if let Some(image_url) = object
            .get("image_url")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            image_urls.push(Value::String(image_url.to_string()));
        }
    }

    let requested_model = request
        .requested_model
        .clone()
        .unwrap_or_else(|| default_model_for_openai_image_operation(request.operation).to_string());
    let mut body = Map::new();
    body.insert(
        "operation".to_string(),
        Value::String(request.operation.as_str().to_string()),
    );
    body.insert("model".to_string(), Value::String(requested_model));
    body.insert(
        "web_model".to_string(),
        Value::String(
            raw_fields
                .web_model
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("gpt-5-5-thinking")
                .to_string(),
        ),
    );
    body.insert("prompt".to_string(), Value::String(prompt));
    body.insert("size".to_string(), Value::String(resolved_size.size));
    body.insert("ratio".to_string(), Value::String(resolved_size.ratio));
    body.insert(
        "size_best_effort".to_string(),
        Value::Bool(resolved_size.best_effort),
    );
    body.insert("images".to_string(), Value::Array(image_urls));
    body.insert("count".to_string(), Value::Number(Number::from(1)));
    if let Some(user) = request.user.as_ref() {
        body.insert("user".to_string(), Value::String(user.clone()));
    }
    if let Some(quality) = request
        .tool
        .get("quality")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        body.insert("quality".to_string(), Value::String(quality.to_string()));
    }
    if let Some(partial_images) = request.tool.get("partial_images").and_then(Value::as_u64) {
        body.insert(
            "partial_images".to_string(),
            Value::Number(Number::from(partial_images)),
        );
    }
    if let Some(output_format) = request
        .summary_json
        .get("output_format")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        body.insert(
            "output_format".to_string(),
            Value::String(output_format.to_string()),
        );
    }
    if let Some(response_format) = request
        .summary_json
        .get("response_format")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        body.insert(
            "response_format".to_string(),
            Value::String(response_format.to_string()),
        );
    }
    Ok(Value::Object(body))
}

pub fn is_openai_image_stream_request(
    parts: &http::request::Parts,
    body_json: &Value,
    body_base64: Option<&str>,
) -> bool {
    if !matches!(
        openai_image_operation_from_path(parts.uri.path()),
        Some(OpenAiImageOperation::Generate | OpenAiImageOperation::Edit)
    ) {
        return false;
    }

    if let Some(body_base64) = body_base64 {
        return parse_multipart_fields_from_base64(parts, body_base64)
            .and_then(|fields| find_multipart_text_field(&fields, "stream"))
            .and_then(|value| parse_bool_string(&value))
            .unwrap_or(false);
    }

    body_json
        .get("stream")
        .and_then(value_as_bool)
        .unwrap_or(false)
}

pub fn openai_image_operation_from_path(path: &str) -> Option<OpenAiImageOperation> {
    match path {
        "/v1/images/generations" => Some(OpenAiImageOperation::Generate),
        "/v1/images/edits" => Some(OpenAiImageOperation::Edit),
        _ => None,
    }
}

fn resolve_chatgpt_web_raw_image_fields(
    parts: &http::request::Parts,
    body_json: &Value,
    body_base64: Option<&str>,
) -> Result<ChatGptWebRawImageFields, ChatGptWebImageRequestError> {
    if let Some(body_base64) = body_base64 {
        let fields = parse_multipart_fields_from_base64(parts, body_base64).ok_or_else(|| {
            ChatGptWebImageRequestError::invalid_request(
                "ChatGPT-Web image proxy could not parse multipart image request",
            )
        })?;
        return Ok(ChatGptWebRawImageFields {
            size: find_multipart_text_field(&fields, "size"),
            resolution: find_multipart_text_field(&fields, "resolution"),
            size_tier: find_multipart_text_field(&fields, "size_tier"),
            ratio: find_multipart_text_field(&fields, "ratio"),
            aspect_ratio: find_multipart_text_field(&fields, "aspect_ratio"),
            web_model: find_multipart_text_field(&fields, "web_model"),
        });
    }

    let Some(object) = body_json.as_object() else {
        return Ok(ChatGptWebRawImageFields::default());
    };
    Ok(ChatGptWebRawImageFields {
        size: json_text_field(object, "size"),
        resolution: json_text_field(object, "resolution"),
        size_tier: json_text_field(object, "size_tier"),
        ratio: json_text_field(object, "ratio"),
        aspect_ratio: json_text_field(object, "aspect_ratio"),
        web_model: json_text_field(object, "web_model"),
    })
}

fn json_text_field(object: &Map<String, Value>, key: &str) -> Option<String> {
    object
        .get(key)
        .and_then(|value| {
            value
                .as_str()
                .map(ToOwned::to_owned)
                .or_else(|| value.as_u64().map(|number| number.to_string()))
                .or_else(|| value.as_i64().map(|number| number.to_string()))
        })
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn resolve_chatgpt_web_size(
    fields: &ChatGptWebRawImageFields,
) -> Result<ChatGptWebResolvedSize, ChatGptWebImageRequestError> {
    let tier = fields
        .resolution
        .as_deref()
        .or(fields.size_tier.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if let Some(tier) = tier {
        let normalized = tier.to_ascii_uppercase();
        if normalized != "1K" && normalized != "1" {
            return Err(unsupported_chatgpt_web_resolution_error());
        }
    }

    let fallback_ratio = fields
        .ratio
        .as_deref()
        .or(fields.aspect_ratio.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("1:1");
    let explicit_size = fields
        .size
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let size = explicit_size
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| chatgpt_web_1k_size_for_ratio(fallback_ratio).to_string());
    let (width, height) = parse_image_size_or_default(size.as_str());
    if width.saturating_mul(height) > CHATGPT_WEB_IMAGE_MAX_AREA {
        return Err(unsupported_chatgpt_web_resolution_error());
    }
    let ratio = chatgpt_web_ratio_from_size(size.as_str(), fallback_ratio).to_string();
    let best_effort = explicit_size.is_some_and(|_| !chatgpt_web_is_exact_1k_size(size.as_str()));

    Ok(ChatGptWebResolvedSize {
        size,
        ratio,
        best_effort,
    })
}

fn unsupported_chatgpt_web_resolution_error() -> ChatGptWebImageRequestError {
    ChatGptWebImageRequestError::invalid_request(
        "ChatGPT-Web image proxy does not support the requested resolution; use resolution=1K, size_tier=1K, or a size area <= 1,500,000 pixels",
    )
}

fn chatgpt_web_1k_size_for_ratio(ratio: &str) -> &'static str {
    match ratio.trim() {
        "3:2" => "1216x832",
        "2:3" => "832x1216",
        "4:3" => "1152x864",
        "3:4" => "864x1152",
        "5:4" => "1120x896",
        "4:5" => "896x1120",
        "16:9" => "1344x768",
        "9:16" => "768x1344",
        "21:9" => "1536x640",
        _ => "1024x1024",
    }
}

fn chatgpt_web_ratio_from_size<'a>(size: &str, fallback: &'a str) -> &'a str {
    match size.trim() {
        "1024x1024" | "1248x1248" | "2480x2480" | "512x512" => "1:1",
        "1216x832" | "1536x1024" | "3056x2032" => "3:2",
        "832x1216" | "1024x1536" | "2032x3056" => "2:3",
        "1152x864" | "1440x1088" | "2880x2160" => "4:3",
        "864x1152" | "1088x1440" | "2160x2880" => "3:4",
        "1120x896" | "1392x1120" | "2784x2224" => "5:4",
        "896x1120" | "1120x1392" | "2224x2784" => "4:5",
        "1344x768" | "1664x928" | "3312x1872" => "16:9",
        "768x1344" | "928x1664" | "1872x3312" => "9:16",
        "1536x640" | "1904x816" | "3808x1632" => "21:9",
        _ => fallback.trim(),
    }
}

fn chatgpt_web_is_exact_1k_size(size: &str) -> bool {
    matches!(
        size.trim(),
        "1024x1024"
            | "1216x832"
            | "832x1216"
            | "1152x864"
            | "864x1152"
            | "1120x896"
            | "896x1120"
            | "1344x768"
            | "768x1344"
            | "1536x640"
    )
}

fn parse_image_size_or_default(size: &str) -> (u64, u64) {
    let Some((width, height)) = size.trim().split_once('x') else {
        return (1024, 1024);
    };
    let width = width.trim().parse::<u64>().ok().filter(|value| *value > 0);
    let height = height.trim().parse::<u64>().ok().filter(|value| *value > 0);
    (width.unwrap_or(1024), height.unwrap_or(1024))
}

fn chatgpt_web_prompt_with_ratio(prompt: &str, ratio: &str) -> String {
    let prompt = prompt.trim();
    let ratio = ratio.trim();
    if ratio.is_empty() || ratio == "1:1" {
        return prompt.to_string();
    }
    format!("{prompt}\n\nSet the image aspect ratio to {ratio}.")
}

pub fn resolve_requested_openai_image_model_for_request(
    parts: &http::request::Parts,
    body_json: &Value,
    body_base64: Option<&str>,
) -> Option<String> {
    let operation = openai_image_operation_from_path(parts.uri.path())?;
    if let Some(body_base64) = body_base64 {
        let multipart_fields = parse_multipart_fields_from_base64(parts, body_base64)?;
        let model = multipart_fields
            .iter()
            .find(|field| field.name.trim() == "model")
            .map(|field| String::from_utf8_lossy(&field.data).trim().to_string());
        normalize_requested_image_model(model.as_deref())
            .or_else(|| Some(default_model_for_openai_image_operation(operation).to_string()))
    } else {
        normalize_requested_image_model(body_json.get("model").and_then(Value::as_str))
            .or_else(|| Some(default_model_for_openai_image_operation(operation).to_string()))
    }
}

pub fn default_model_for_openai_image_operation(operation: OpenAiImageOperation) -> &'static str {
    match operation {
        OpenAiImageOperation::Generate | OpenAiImageOperation::Edit => {
            CODEX_OPENAI_IMAGE_DEFAULT_MODEL
        }
    }
}

pub fn normalize_openai_image_request(
    parts: &http::request::Parts,
    body_json: &Value,
    body_base64: Option<&str>,
) -> Option<NormalizedOpenAiImageRequest> {
    normalize_openai_image_request_with_options(
        parts,
        body_json,
        body_base64,
        OpenAiImageNormalizeOptions::default(),
    )
}

pub fn normalize_openai_image_request_with_options(
    parts: &http::request::Parts,
    body_json: &Value,
    body_base64: Option<&str>,
    options: OpenAiImageNormalizeOptions,
) -> Option<NormalizedOpenAiImageRequest> {
    let operation = openai_image_operation_from_path(parts.uri.path())?;
    if let Some(body_base64) = body_base64 {
        normalize_openai_image_multipart_request(parts, body_base64, operation, options)
    } else {
        normalize_openai_image_json_request(body_json, operation, options)
    }
}

pub fn build_openai_image_provider_request_body(request: &NormalizedOpenAiImageRequest) -> Value {
    let input = if request.operation == OpenAiImageOperation::Generate && request.images.is_empty()
    {
        json!([{
            "role": "user",
            "content": request.prompt.clone().unwrap_or_default(),
        }])
    } else {
        let mut content = Vec::new();
        if let Some(prompt) = request.prompt.as_ref() {
            content.push(json!({
                "type": "input_text",
                "text": prompt,
            }));
        }
        content.extend(request.images.iter().cloned());
        json!([{
            "role": "user",
            "content": content,
        }])
    };

    let mut body = Map::new();
    body.insert("input".to_string(), input);
    body.insert(
        "tools".to_string(),
        Value::Array(vec![Value::Object(request.tool.clone())]),
    );
    if let Some(user) = request.user.as_ref() {
        body.insert("user".to_string(), Value::String(user.clone()));
    }
    if let Some(image_count) = request.image_count.filter(|value| *value > 1) {
        body.insert("n".to_string(), Value::Number(Number::from(image_count)));
    }
    Value::Object(body)
}

pub fn build_openai_image_api_provider_request_body(
    request: &NormalizedOpenAiImageRequest,
    mapped_model: Option<&str>,
    upstream_is_stream: bool,
) -> Option<Value> {
    let model = resolve_openai_image_api_provider_model(request, mapped_model);
    let body = build_unprojected_openai_image_api_provider_request_body(
        request,
        model,
        upstream_is_stream,
    );
    project_openai_image_api_request_body(
        &body,
        model,
        request.operation,
        request.max_generation_count,
    )
}

pub fn build_codex_openai_image_api_provider_request_body(
    request: &NormalizedOpenAiImageRequest,
    mapped_model: Option<&str>,
    upstream_is_stream: bool,
) -> Option<Value> {
    let model = resolve_openai_image_api_provider_model(request, mapped_model);
    let body = build_unprojected_openai_image_api_provider_request_body(
        request,
        model,
        upstream_is_stream,
    );
    project_codex_openai_image_api_request_body_with_max_generation_count(
        &body,
        request.operation,
        request.max_generation_count,
    )
}

fn resolve_openai_image_api_provider_model<'a>(
    request: &'a NormalizedOpenAiImageRequest,
    mapped_model: Option<&'a str>,
) -> &'a str {
    mapped_model
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or(request.requested_model.as_deref())
        .unwrap_or_else(|| default_model_for_openai_image_operation(request.operation))
}

fn build_unprojected_openai_image_api_provider_request_body(
    request: &NormalizedOpenAiImageRequest,
    model: &str,
    upstream_is_stream: bool,
) -> Value {
    let mut body = Map::new();
    body.insert("model".to_string(), Value::String(model.to_string()));
    if let Some(prompt) = request.prompt.as_ref() {
        body.insert("prompt".to_string(), Value::String(prompt.clone()));
    }
    if let Some(image_count) = request.image_count {
        body.insert("n".to_string(), Value::Number(Number::from(image_count)));
    }
    if let Some(user) = request.user.as_ref() {
        body.insert("user".to_string(), Value::String(user.clone()));
    }
    if upstream_is_stream {
        body.insert("stream".to_string(), Value::Bool(true));
    }
    for (key, value) in &request.tool {
        match key.as_str() {
            "type" | "action" => {}
            "input_image_mask" => {
                body.insert("mask".to_string(), value.clone());
            }
            _ => {
                body.insert(key.clone(), value.clone());
            }
        }
    }
    if let Some(response_format) = request.summary_json.get("response_format") {
        body.entry("response_format".to_string())
            .or_insert_with(|| response_format.clone());
    }
    insert_standard_openai_image_inputs(&mut body, request.images.clone());
    Value::Object(body)
}

pub fn project_openai_image_api_request_body(
    body: &Value,
    provider_model: &str,
    operation: OpenAiImageOperation,
    max_generation_count: u64,
) -> Option<Value> {
    let mut projected = body.as_object()?.clone();
    let model = provider_model.trim();
    if model.is_empty() {
        return None;
    }
    let model_family = openai_image_model_family(model);
    if operation == OpenAiImageOperation::Edit && model_family == OpenAiImageModelFamily::DallE3 {
        return None;
    }

    let prompt = non_empty_image_string(projected.get("prompt"))?;
    let max_prompt_chars = match model_family {
        OpenAiImageModelFamily::DallE2 => Some(1_000),
        OpenAiImageModelFamily::DallE3 => Some(4_000),
        OpenAiImageModelFamily::GptImage
        | OpenAiImageModelFamily::GptImageMini
        | OpenAiImageModelFamily::GptImage2 => Some(32_000),
        OpenAiImageModelFamily::Other => None,
    };
    if max_prompt_chars.is_some_and(|limit| prompt.chars().count() > limit) {
        return None;
    }

    let image_inputs = openai_image_api_inputs(&projected)?
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    projected.remove("image");
    projected.remove("images");
    let image_input_count = image_inputs.len();
    if operation == OpenAiImageOperation::Edit {
        let images = image_inputs
            .into_iter()
            .map(|image| {
                let image = image.as_object()?;
                image
                    .get("image_url")
                    .cloned()
                    .map(|image_url| json!({ "image_url": image_url }))
                    .or_else(|| {
                        image
                            .get("file_id")
                            .cloned()
                            .map(|file_id| json!({ "file_id": file_id }))
                    })
            })
            .collect::<Option<Vec<_>>>()?;
        projected.insert("images".to_string(), Value::Array(images));
    } else {
        insert_standard_openai_image_inputs(&mut projected, image_inputs);
    }
    match operation {
        OpenAiImageOperation::Generate
            if image_input_count > 0 || projected.contains_key("mask") =>
        {
            return None;
        }
        OpenAiImageOperation::Edit if image_input_count == 0 => return None,
        OpenAiImageOperation::Edit => {
            let max_images = match model_family {
                OpenAiImageModelFamily::DallE2 => 1,
                OpenAiImageModelFamily::GptImage
                | OpenAiImageModelFamily::GptImageMini
                | OpenAiImageModelFamily::GptImage2 => 16,
                OpenAiImageModelFamily::DallE3 => 0,
                OpenAiImageModelFamily::Other => usize::MAX,
            };
            if image_input_count > max_images {
                return None;
            }
        }
        OpenAiImageOperation::Generate => {}
    }

    if let Some(n) = projected.get("n") {
        let model_limit = if model_family == OpenAiImageModelFamily::DallE3 {
            1
        } else {
            OPENAI_IMAGE_MAX_GENERATION_COUNT
        };
        let max_generation_count = max_generation_count.max(1).min(model_limit);
        let n = n
            .as_u64()
            .filter(|value| (1..=max_generation_count).contains(value))?;
        projected.insert("n".to_string(), Value::Number(Number::from(n)));
    }
    if let Some(quality) = projected.get("quality") {
        let quality = non_empty_image_string(Some(quality))?;
        let canonical = normalize_openai_image_quality(quality)?;
        let wire = match model_family {
            OpenAiImageModelFamily::DallE3 => match canonical {
                "medium" => "standard",
                "high" => "hd",
                _ => return None,
            },
            OpenAiImageModelFamily::DallE2 => match canonical {
                "medium" => "standard",
                _ => return None,
            },
            OpenAiImageModelFamily::GptImage
            | OpenAiImageModelFamily::GptImageMini
            | OpenAiImageModelFamily::GptImage2
            | OpenAiImageModelFamily::Other => canonical,
        };
        projected.insert("quality".to_string(), Value::String(wire.to_string()));
    }

    let output_format = if let Some(value) = projected.get("output_format") {
        let value = non_empty_image_string(Some(value))?.to_ascii_lowercase();
        if !matches!(value.as_str(), "png" | "jpeg" | "webp")
            || !matches!(
                model_family,
                OpenAiImageModelFamily::GptImage
                    | OpenAiImageModelFamily::GptImageMini
                    | OpenAiImageModelFamily::GptImage2
                    | OpenAiImageModelFamily::Other
            )
        {
            return None;
        }
        projected.insert("output_format".to_string(), Value::String(value.clone()));
        Some(value)
    } else {
        None
    };

    if let Some(value) = projected.get("response_format") {
        let value = non_empty_image_string(Some(value))?.to_ascii_lowercase();
        if !matches!(value.as_str(), "url" | "b64_json") {
            return None;
        }
        match model_family {
            OpenAiImageModelFamily::GptImage
            | OpenAiImageModelFamily::GptImageMini
            | OpenAiImageModelFamily::GptImage2 => {
                if value != "b64_json" {
                    return None;
                }
                projected.remove("response_format");
            }
            OpenAiImageModelFamily::DallE2
            | OpenAiImageModelFamily::DallE3
            | OpenAiImageModelFamily::Other => {
                projected.insert("response_format".to_string(), Value::String(value));
            }
        }
    }

    if let Some(value) = projected.get("background") {
        let value = non_empty_image_string(Some(value))?.to_ascii_lowercase();
        if !matches!(value.as_str(), "transparent" | "opaque" | "auto")
            || matches!(
                model_family,
                OpenAiImageModelFamily::DallE2 | OpenAiImageModelFamily::DallE3
            )
            || (model_family == OpenAiImageModelFamily::GptImage2 && value == "transparent")
            || (value == "transparent"
                && !matches!(output_format.as_deref().unwrap_or("png"), "png" | "webp"))
        {
            return None;
        }
        projected.insert("background".to_string(), Value::String(value));
    }

    if let Some(value) = projected.get("moderation") {
        let value = non_empty_image_string(Some(value))?.to_ascii_lowercase();
        if operation != OpenAiImageOperation::Generate
            || !matches!(value.as_str(), "low" | "auto")
            || matches!(
                model_family,
                OpenAiImageModelFamily::DallE2 | OpenAiImageModelFamily::DallE3
            )
        {
            return None;
        }
        projected.insert("moderation".to_string(), Value::String(value));
    }

    if let Some(value) = projected.get("input_fidelity") {
        let value = non_empty_image_string(Some(value))?.to_ascii_lowercase();
        if operation != OpenAiImageOperation::Edit
            || !matches!(value.as_str(), "low" | "high")
            || matches!(
                model_family,
                OpenAiImageModelFamily::DallE2
                    | OpenAiImageModelFamily::DallE3
                    | OpenAiImageModelFamily::GptImageMini
            )
        {
            return None;
        }
        projected.insert("input_fidelity".to_string(), Value::String(value));
    }

    if let Some(value) = projected.get("output_compression") {
        let value = value.as_u64().filter(|value| *value <= 100)?;
        if !matches!(output_format.as_deref(), Some("jpeg" | "webp"))
            || matches!(
                model_family,
                OpenAiImageModelFamily::DallE2 | OpenAiImageModelFamily::DallE3
            )
        {
            return None;
        }
        projected.insert(
            "output_compression".to_string(),
            Value::Number(Number::from(value)),
        );
    }

    let stream = match projected.get("stream") {
        Some(value) => Some(value.as_bool()?),
        None => None,
    };
    if let Some(stream) = stream {
        match model_family {
            OpenAiImageModelFamily::DallE2
            | OpenAiImageModelFamily::DallE3
            | OpenAiImageModelFamily::GptImage2 => {
                if stream {
                    return None;
                }
                projected.remove("stream");
            }
            OpenAiImageModelFamily::GptImage
            | OpenAiImageModelFamily::GptImageMini
            | OpenAiImageModelFamily::Other => {}
        }
    }

    if let Some(value) = projected.get("partial_images") {
        let value = value.as_u64().filter(|value| *value <= 3)?;
        if stream != Some(true)
            || matches!(
                model_family,
                OpenAiImageModelFamily::DallE2
                    | OpenAiImageModelFamily::DallE3
                    | OpenAiImageModelFamily::GptImage2
            )
        {
            return None;
        }
        projected.insert(
            "partial_images".to_string(),
            Value::Number(Number::from(value)),
        );
    }

    if let Some(value) = projected.get("style") {
        let value = non_empty_image_string(Some(value))?.to_ascii_lowercase();
        if operation != OpenAiImageOperation::Generate
            || !matches!(value.as_str(), "vivid" | "natural")
            || !matches!(
                model_family,
                OpenAiImageModelFamily::DallE3 | OpenAiImageModelFamily::Other
            )
        {
            return None;
        }
        projected.insert("style".to_string(), Value::String(value));
    }

    if let Some(value) = projected.get("size") {
        let value = non_empty_image_string(Some(value))?.to_ascii_lowercase();
        if !openai_image_size_supported(model_family, &value) {
            return None;
        }
        projected.insert("size".to_string(), Value::String(value));
    }

    Some(Value::Object(projected))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OpenAiImageModelFamily {
    GptImage,
    GptImageMini,
    GptImage2,
    DallE3,
    DallE2,
    Other,
}

fn openai_image_model_family(model: &str) -> OpenAiImageModelFamily {
    let normalized = model.trim().to_ascii_lowercase();
    let model = normalized.rsplit('/').next().unwrap_or_default();
    if model == "gpt-image-2" || model.starts_with("gpt-image-2-") {
        OpenAiImageModelFamily::GptImage2
    } else if model == "gpt-image-1-mini" || model.starts_with("gpt-image-1-mini-") {
        OpenAiImageModelFamily::GptImageMini
    } else if model.starts_with("gpt-image-") || model == "chatgpt-image-latest" {
        OpenAiImageModelFamily::GptImage
    } else if model.starts_with("dall-e-3") {
        OpenAiImageModelFamily::DallE3
    } else if model.starts_with("dall-e-2") {
        OpenAiImageModelFamily::DallE2
    } else {
        OpenAiImageModelFamily::Other
    }
}

pub(crate) fn insert_standard_openai_image_inputs(
    object: &mut Map<String, Value>,
    images: Vec<Value>,
) {
    if images.is_empty() {
        return;
    }
    let image = if images.len() == 1 {
        images.into_iter().next().expect("one image should exist")
    } else {
        Value::Array(images)
    };
    object.insert("image".to_string(), image);
}

fn openai_image_api_inputs(object: &Map<String, Value>) -> Option<Vec<&Value>> {
    match (object.get("image"), object.get("images")) {
        (Some(_), Some(_)) => None,
        (Some(Value::Array(images)), None) | (None, Some(Value::Array(images))) => {
            Some(images.iter().collect())
        }
        (Some(image), None) => Some(vec![image]),
        (None, Some(_)) => None,
        (None, None) => Some(Vec::new()),
    }
}

fn openai_image_size_supported(family: OpenAiImageModelFamily, size: &str) -> bool {
    match family {
        OpenAiImageModelFamily::DallE2 => {
            matches!(size, "256x256" | "512x512" | "1024x1024")
        }
        OpenAiImageModelFamily::DallE3 => {
            matches!(size, "1024x1024" | "1792x1024" | "1024x1792")
        }
        OpenAiImageModelFamily::GptImage | OpenAiImageModelFamily::GptImageMini => {
            matches!(size, "auto" | "1024x1024" | "1536x1024" | "1024x1536")
        }
        OpenAiImageModelFamily::GptImage2 => {
            size == "auto" || gpt_image_2_resolution_supported(size)
        }
        OpenAiImageModelFamily::Other => !size.is_empty(),
    }
}

fn gpt_image_2_resolution_supported(size: &str) -> bool {
    let mut dimensions = size.split('x');
    let Some(width) = dimensions
        .next()
        .and_then(|value| value.parse::<u64>().ok())
    else {
        return false;
    };
    let Some(height) = dimensions
        .next()
        .and_then(|value| value.parse::<u64>().ok())
    else {
        return false;
    };
    if dimensions.next().is_some()
        || width == 0
        || height == 0
        || width % 16 != 0
        || height % 16 != 0
    {
        return false;
    }
    let shorter = width.min(height);
    let longer = width.max(height);
    longer <= shorter.saturating_mul(3)
        && longer <= 3_840
        && width.saturating_mul(height) <= 3_840 * 2_160
}

pub fn project_codex_openai_image_api_request_body(
    body: &Value,
    operation: OpenAiImageOperation,
) -> Option<Value> {
    project_codex_openai_image_api_request_body_with_max_generation_count(
        body,
        operation,
        OPENAI_IMAGE_MAX_GENERATION_COUNT,
    )
}

fn project_codex_openai_image_api_request_body_with_max_generation_count(
    body: &Value,
    operation: OpenAiImageOperation,
    max_generation_count: u64,
) -> Option<Value> {
    let source_object = body.as_object()?;
    let source_images = if operation == OpenAiImageOperation::Edit {
        let images = collect_codex_openai_image_urls(source_object)?;
        if images.is_empty() || images.len() > 5 {
            return None;
        }
        images
    } else {
        Vec::new()
    };
    let model = body.get("model").and_then(Value::as_str)?;
    let projected_body =
        project_openai_image_api_request_body(body, model, operation, max_generation_count)?;
    let object = projected_body.as_object()?;
    if object.keys().any(|key| {
        !matches!(
            key.as_str(),
            "model"
                | "prompt"
                | "background"
                | "n"
                | "quality"
                | "size"
                | "images"
                | "response_format"
                | "stream"
        )
    }) {
        return None;
    }

    let model = non_empty_image_string(object.get("model"))?;
    let prompt = non_empty_image_string(object.get("prompt"))?;
    if object
        .get("stream")
        .is_some_and(|value| !value.is_boolean())
    {
        return None;
    }
    if object
        .get("response_format")
        .is_some_and(|value| value.as_str() != Some("b64_json"))
    {
        return None;
    }

    let mut projected = Map::new();
    if operation == OpenAiImageOperation::Edit {
        projected.insert("images".to_string(), Value::Array(source_images));
    } else if object.contains_key("image") || object.contains_key("images") {
        return None;
    }
    projected.insert("prompt".to_string(), Value::String(prompt.to_string()));
    if let Some(background) =
        optional_codex_image_enum(object.get("background"), &["transparent", "opaque", "auto"])?
    {
        projected.insert("background".to_string(), Value::String(background));
    }
    projected.insert("model".to_string(), Value::String(model.to_string()));
    if let Some(n) = object.get("n") {
        let n = n
            .as_u64()
            .filter(|value| (1..=OPENAI_IMAGE_MAX_GENERATION_COUNT).contains(value))?;
        projected.insert("n".to_string(), Value::Number(Number::from(n)));
    }
    if let Some(quality) =
        optional_codex_image_enum(object.get("quality"), &["low", "medium", "high", "auto"])?
    {
        projected.insert("quality".to_string(), Value::String(quality));
    }
    if let Some(size) = object.get("size") {
        projected.insert(
            "size".to_string(),
            Value::String(non_empty_image_string(Some(size))?.to_string()),
        );
    }
    Some(Value::Object(projected))
}

fn non_empty_image_string(value: Option<&Value>) -> Option<&str> {
    value?
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn optional_codex_image_enum(value: Option<&Value>, allowed: &[&str]) -> Option<Option<String>> {
    let Some(value) = value else {
        return Some(None);
    };
    let value = non_empty_image_string(Some(value))?.to_ascii_lowercase();
    allowed
        .iter()
        .any(|allowed| value == *allowed)
        .then_some(Some(value))
}

fn collect_codex_openai_image_urls(object: &Map<String, Value>) -> Option<Vec<Value>> {
    openai_image_api_inputs(object)?
        .into_iter()
        .map(|image| {
            let image = image.as_object()?;
            if image
                .keys()
                .any(|key| !matches!(key.as_str(), "type" | "image_url"))
                || image
                    .get("type")
                    .is_some_and(|value| value.as_str() != Some("input_image"))
            {
                return None;
            }
            let image_url = non_empty_image_string(image.get("image_url"))?;
            Some(json!({ "image_url": image_url }))
        })
        .collect()
}

fn normalize_openai_image_json_request(
    body_json: &Value,
    operation: OpenAiImageOperation,
    options: OpenAiImageNormalizeOptions,
) -> Option<NormalizedOpenAiImageRequest> {
    let object = body_json.as_object()?;
    let image_count = object.get("n").and_then(image_request_count);
    if image_count.is_some_and(|value| value == 0 || value > max_generation_count(options)) {
        return None;
    }
    let requested_model =
        normalize_requested_image_model(object.get("model").and_then(Value::as_str));
    let prompt = normalize_prompt(object.get("prompt"), operation)?;
    let response_format =
        normalize_image_response_format(object.get("response_format").and_then(Value::as_str))?;
    let output_format =
        normalize_output_format(object.get("output_format").and_then(Value::as_str))?;
    let partial_images = normalize_partial_images(object.get("partial_images"))?;
    if object
        .get("stream")
        .is_some_and(|value| value_as_bool(value).is_none())
    {
        return None;
    }
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
    if matches!(operation, OpenAiImageOperation::Edit) && images.is_empty() {
        return None;
    }

    let tool = build_tool_options_from_json(object, operation, mask.as_ref())?;

    Some(NormalizedOpenAiImageRequest {
        operation,
        requested_model,
        prompt,
        images,
        tool,
        image_count,
        max_generation_count: max_generation_count(options),
        user,
        summary_json: build_image_request_summary_json(
            operation,
            response_format,
            output_format,
            partial_images,
        ),
    })
}

fn normalize_openai_image_multipart_request(
    parts: &http::request::Parts,
    body_base64: &str,
    operation: OpenAiImageOperation,
    options: OpenAiImageNormalizeOptions,
) -> Option<NormalizedOpenAiImageRequest> {
    let multipart_fields = parse_multipart_fields_from_base64(parts, body_base64)?;
    let requested_model = normalize_requested_image_model(
        find_multipart_text_field(&multipart_fields, "model").as_deref(),
    );
    let image_count = find_multipart_text_field(&multipart_fields, "n")
        .and_then(|value| value.trim().parse::<u64>().ok());
    if image_count.is_some_and(|value| value == 0 || value > max_generation_count(options)) {
        return None;
    }
    let prompt = normalize_prompt(
        find_multipart_text_field(&multipart_fields, "prompt")
            .as_ref()
            .map(|value| Value::String(value.clone()))
            .as_ref(),
        operation,
    )?;
    let response_format = normalize_image_response_format(
        find_multipart_text_field(&multipart_fields, "response_format").as_deref(),
    )?;
    let output_format = normalize_output_format(
        find_multipart_text_field(&multipart_fields, "output_format").as_deref(),
    )?;
    let partial_images = normalize_partial_images(
        find_multipart_text_field(&multipart_fields, "partial_images")
            .map(Value::String)
            .as_ref(),
    )?;
    if find_multipart_text_field(&multipart_fields, "stream")
        .as_deref()
        .is_some_and(|value| parse_bool_string(value).is_none())
    {
        return None;
    }
    let user = find_multipart_text_field(&multipart_fields, "user")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

    let mut images = Vec::new();
    let mut mask = None;
    let mut raw_tool_values = BTreeMap::new();

    for field in multipart_fields {
        let name = field.name.trim().to_string();
        if name.is_empty() {
            continue;
        }
        if matches!(name.as_str(), "image" | "image[]" | "images[]" | "images") {
            images.push(multipart_file_to_input_image(&field));
            continue;
        }
        if name == "mask" {
            mask = Some(multipart_file_to_input_image(&field));
            continue;
        }
        if matches!(
            name.as_str(),
            "size"
                | "quality"
                | "background"
                | "output_format"
                | "output_compression"
                | "moderation"
                | "input_fidelity"
                | "partial_images"
                | "style"
        ) {
            raw_tool_values.insert(
                name,
                String::from_utf8_lossy(&field.data).trim().to_string(),
            );
        }
    }

    if matches!(operation, OpenAiImageOperation::Edit) && images.is_empty() {
        return None;
    }

    let tool = build_tool_options_from_multipart(raw_tool_values, operation, mask.as_ref())?;

    Some(NormalizedOpenAiImageRequest {
        operation,
        requested_model,
        prompt,
        images,
        tool,
        image_count,
        max_generation_count: max_generation_count(options),
        user,
        summary_json: build_image_request_summary_json(
            operation,
            response_format,
            output_format,
            partial_images,
        ),
    })
}

fn normalize_requested_image_model(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn normalize_prompt(
    value: Option<&Value>,
    operation: OpenAiImageOperation,
) -> Option<Option<String>> {
    let prompt = value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let _ = operation;
    Some(prompt)
}

fn normalize_image_response_format(
    value: Option<&str>,
) -> Option<Option<OpenAiImageResponseFormat>> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Some(None);
    };
    match value.to_ascii_lowercase().as_str() {
        "b64_json" => Some(Some(OpenAiImageResponseFormat::B64Json)),
        "url" => Some(Some(OpenAiImageResponseFormat::Url)),
        _ => None,
    }
}

fn normalize_output_format(value: Option<&str>) -> Option<Option<String>> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Some(None);
    };
    match value.to_ascii_lowercase().as_str() {
        "png" => Some(Some("png".to_string())),
        "jpeg" | "jpg" => Some(Some("jpeg".to_string())),
        "webp" => Some(Some("webp".to_string())),
        _ => None,
    }
}

fn normalize_partial_images(value: Option<&Value>) -> Option<Option<u64>> {
    let Some(number) = value.and_then(image_request_count) else {
        return Some(None);
    };
    (number <= 3).then_some(Some(number))
}

fn normalize_output_format_value(value: &Value) -> Option<Value> {
    let output_format = value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    normalize_output_format(Some(output_format))
        .flatten()
        .map(Value::String)
}

fn build_image_request_summary_json(
    operation: OpenAiImageOperation,
    response_format: Option<OpenAiImageResponseFormat>,
    output_format: Option<String>,
    partial_images: Option<u64>,
) -> Value {
    let mut summary = Map::new();
    summary.insert(
        "operation".to_string(),
        Value::String(operation.as_str().to_string()),
    );
    if let Some(response_format) = response_format {
        summary.insert(
            "response_format".to_string(),
            Value::String(response_format.as_str().to_string()),
        );
    }
    if let Some(output_format) = output_format {
        summary.insert("output_format".to_string(), Value::String(output_format));
    }
    if let Some(partial_images) = partial_images {
        summary.insert(
            "partial_images".to_string(),
            Value::Number(Number::from(partial_images)),
        );
    }
    Value::Object(summary)
}

fn build_tool_options_from_json(
    object: &Map<String, Value>,
    operation: OpenAiImageOperation,
    mask: Option<&Value>,
) -> Option<Map<String, Value>> {
    let mut raw_values = BTreeMap::new();
    for key in [
        "size",
        "quality",
        "background",
        "output_format",
        "output_compression",
        "moderation",
        "input_fidelity",
        "partial_images",
        "style",
    ] {
        if let Some(value) = object.get(key) {
            raw_values.insert(key.to_string(), value.clone());
        }
    }
    build_tool_options(raw_values, operation, mask)
}

fn build_tool_options_from_multipart(
    raw_values: BTreeMap<String, String>,
    operation: OpenAiImageOperation,
    mask: Option<&Value>,
) -> Option<Map<String, Value>> {
    let mut normalized_values = BTreeMap::new();
    for (key, value) in raw_values {
        normalized_values.insert(key, Value::String(value));
    }
    build_tool_options(normalized_values, operation, mask)
}

fn build_tool_options(
    raw_values: BTreeMap<String, Value>,
    operation: OpenAiImageOperation,
    mask: Option<&Value>,
) -> Option<Map<String, Value>> {
    let mut tool = Map::new();
    tool.insert(
        "type".to_string(),
        Value::String("image_generation".to_string()),
    );
    tool.insert(
        "action".to_string(),
        Value::String(
            match operation {
                OpenAiImageOperation::Generate => "generate",
                OpenAiImageOperation::Edit => "edit",
            }
            .to_string(),
        ),
    );
    for (key, value) in raw_values {
        let normalized = match key.as_str() {
            "size" | "background" | "moderation" | "input_fidelity" | "style" => {
                normalize_non_empty_string_value(&value)
            }
            "output_format" => normalize_output_format_value(&value),
            "quality" => normalize_quality_value(&value),
            "output_compression" => normalize_output_compression_value(&value),
            "partial_images" => normalize_partial_images(Some(&value))
                .flatten()
                .map(|value| Value::Number(Number::from(value))),
            _ => Some(value),
        }?;
        tool.insert(key, normalized);
    }
    if let Some(mask) = mask {
        tool.insert("input_image_mask".to_string(), mask_payload(mask));
    }
    Some(tool)
}

fn normalize_non_empty_string_value(value: &Value) -> Option<Value> {
    value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| Value::String(value.to_string()))
}

pub fn normalize_openai_image_quality(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "low" => Some("low"),
        "medium" | "standard" => Some("medium"),
        "high" | "hd" => Some("high"),
        "auto" => Some("auto"),
        _ => None,
    }
}

fn normalize_quality_value(value: &Value) -> Option<Value> {
    let quality = value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let normalized = normalize_openai_image_quality(quality)?;
    Some(Value::String(normalized.to_string()))
}

fn normalize_output_compression_value(value: &Value) -> Option<Value> {
    let number = value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|number| u64::try_from(number).ok()))
        .or_else(|| {
            value
                .as_str()
                .and_then(|text| text.trim().parse::<u64>().ok())
        })?;
    (number <= 100).then(|| Value::Number(Number::from(number)))
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

fn max_generation_count(options: OpenAiImageNormalizeOptions) -> u64 {
    options.max_generation_count.max(1)
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
                    "image_url": format!("data:{mime_type};base64,{b64_json}"),
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

#[derive(Debug, Clone)]
struct MultipartField {
    name: String,
    content_type: Option<String>,
    data: Vec<u8>,
}

fn multipart_file_to_input_image(field: &MultipartField) -> Value {
    let content_type = field
        .content_type
        .clone()
        .unwrap_or_else(|| "application/octet-stream".to_string());
    json!({
        "type": "input_image",
        "image_url": format!(
            "data:{};base64,{}",
            content_type,
            base64::engine::general_purpose::STANDARD.encode(&field.data),
        ),
    })
}

fn find_multipart_text_field(fields: &[MultipartField], name: &str) -> Option<String> {
    fields
        .iter()
        .find(|field| field.name.trim() == name)
        .map(|field| String::from_utf8_lossy(&field.data).trim().to_string())
        .filter(|value| !value.is_empty())
}

fn parse_multipart_fields_from_base64(
    parts: &http::request::Parts,
    body_base64: &str,
) -> Option<Vec<MultipartField>> {
    let body_base64 = body_base64.trim();
    if body_base64.is_empty() {
        return None;
    }
    let content_type = parts
        .headers
        .get(http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())?;
    let boundary = multipart_boundary(content_type)?;
    let body_bytes =
        decode_multipart_body_base64_with_limit(body_base64, MAX_MULTIPART_BODY_BYTES)?;
    Some(parse_multipart_fields(&body_bytes, boundary.as_str()))
}

fn decode_multipart_body_base64_with_limit(
    body_base64: &str,
    decoded_limit: usize,
) -> Option<Vec<u8>> {
    let max_encoded_len = decoded_limit
        .checked_add(2)
        .and_then(|value| value.checked_div(3))
        .and_then(|value| value.checked_mul(4))
        .unwrap_or(usize::MAX);
    if body_base64.len() > max_encoded_len {
        return None;
    }
    let body = base64::engine::general_purpose::STANDARD
        .decode(body_base64)
        .ok()?;
    (body.len() <= decoded_limit).then_some(body)
}

fn parse_multipart_fields(body: &[u8], boundary: &str) -> Vec<MultipartField> {
    let delimiter = format!("--{boundary}").into_bytes();
    let mut parts = Vec::new();
    let mut cursor = 0usize;
    let mut part_count = 0usize;

    while let Some(index) = find_multipart_boundary(&body[cursor..], &delimiter, true) {
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
        // Do not return fields parsed before a truncated part.  Callers use
        // an empty result as the invalid-multipart signal, so retaining a
        // prefix would turn malformed input into an accepted request.
        let Some(next) = find_multipart_boundary(part, &delimiter, false) else {
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

fn multipart_boundary(content_type: &str) -> Option<String> {
    let segments = split_multipart_content_type_parameters(content_type)?;
    let media_type = segments.first()?.trim();
    if !media_type.eq_ignore_ascii_case("multipart/form-data") {
        return None;
    }

    let mut boundary = None;
    let mut seen_keys = Vec::new();
    for segment in segments.into_iter().skip(1) {
        let segment = segment.trim();
        if segment.is_empty() {
            return None;
        }
        let (raw_key, raw_value) = segment.split_once('=')?;
        let key = raw_key.trim();
        if key.is_empty() || !key.as_bytes().iter().copied().all(is_http_token_byte) {
            return None;
        }
        if seen_keys
            .iter()
            .any(|seen: &String| seen.eq_ignore_ascii_case(key))
        {
            return None;
        }
        seen_keys.push(key.to_ascii_lowercase());

        let (value, had_escape) = parse_multipart_content_type_parameter_value(raw_value.trim())?;
        if !key.eq_ignore_ascii_case("boundary") {
            continue;
        }
        if had_escape || !is_valid_multipart_boundary(&value) {
            return None;
        }
        boundary = Some(value);
    }

    boundary
}

fn split_multipart_content_type_parameters(value: &str) -> Option<Vec<&str>> {
    let mut segments = Vec::new();
    let mut start = 0usize;
    let mut in_quotes = false;
    let mut escaped = false;

    for (index, character) in value.char_indices() {
        if character.is_ascii_control() {
            return None;
        }
        if in_quotes {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_quotes = false;
            }
        } else if character == '"' {
            in_quotes = true;
        } else if character == ';' {
            segments.push(&value[start..index]);
            start = index + character.len_utf8();
        }
    }

    if in_quotes || escaped {
        return None;
    }
    segments.push(&value[start..]);
    Some(segments)
}

fn parse_multipart_content_type_parameter_value(value: &str) -> Option<(String, bool)> {
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
        let mut had_escape = false;
        for character in inner.chars() {
            if escaped {
                if character.is_ascii_control() {
                    return None;
                }
                parsed.push(character);
                escaped = false;
                had_escape = true;
            } else if character == '\\' {
                escaped = true;
            } else {
                if character == '"' || character.is_ascii_control() {
                    return None;
                }
                parsed.push(character);
            }
        }
        if escaped {
            return None;
        }
        return Some((parsed, had_escape));
    }

    value
        .as_bytes()
        .iter()
        .copied()
        .all(is_http_token_byte)
        .then(|| (value.to_string(), false))
}

fn find_multipart_boundary(haystack: &[u8], delimiter: &[u8], allow_start: bool) -> Option<usize> {
    if delimiter.is_empty() {
        return None;
    }
    let mut search_start = 0usize;
    while search_start <= haystack.len() {
        let relative = find_subslice(&haystack[search_start..], delimiter)?;
        let index = search_start + relative;
        let at_line_start = index == 0
            || (index >= 2
                && haystack
                    .get(index - 2..index)
                    .is_some_and(|prefix| prefix == b"\r\n"));
        let allowed_position = if allow_start {
            at_line_start
        } else {
            index >= 2
                && haystack
                    .get(index - 2..index)
                    .is_some_and(|prefix| prefix == b"\r\n")
        };
        if allowed_position && multipart_boundary_suffix_is_valid(haystack, index, delimiter) {
            return Some(index);
        }
        search_start = index.saturating_add(1);
    }
    None
}

fn multipart_boundary_suffix_is_valid(haystack: &[u8], index: usize, delimiter: &[u8]) -> bool {
    let suffix_start = index.saturating_add(delimiter.len());
    let Some(suffix) = haystack.get(suffix_start..) else {
        return false;
    };
    suffix.starts_with(b"\r\n")
        || suffix
            .strip_prefix(b"--")
            .is_some_and(|remaining| remaining.is_empty() || remaining.starts_with(b"\r\n"))
}

const MAX_MULTIPART_BOUNDARY_BYTES: usize = 70;

fn is_valid_multipart_boundary(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_MULTIPART_BOUNDARY_BYTES
        && value.as_bytes().iter().copied().all(is_http_token_byte)
}

fn is_http_token_byte(byte: u8) -> bool {
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

fn parse_multipart_field(raw: &[u8]) -> Option<MultipartField> {
    let header_end = find_subslice(raw, b"\r\n\r\n")?;
    let headers = &raw[..header_end];
    let data = raw.get(header_end + 4..)?.to_vec();
    let header_text = std::str::from_utf8(headers).ok()?;

    let mut name = None;
    let mut content_type = None;
    let mut disposition_seen = false;
    let mut content_type_seen = false;
    for line in header_text.split("\r\n") {
        let (header_name, header_value) = line.split_once(':')?;
        let header_name = header_name.trim();
        let header_value = header_value.trim();
        if header_name.eq_ignore_ascii_case("content-disposition") {
            if disposition_seen {
                return None;
            }
            disposition_seen = true;
            name = parse_multipart_content_disposition_name(header_value);
        } else if header_name.eq_ignore_ascii_case("content-type") {
            if content_type_seen || header_value.is_empty() {
                return None;
            }
            content_type_seen = true;
            content_type = Some(header_value.to_string());
        }
    }

    Some(MultipartField {
        name: name?,
        content_type,
        data,
    })
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
        if key.is_empty() || !key.as_bytes().iter().copied().all(is_http_token_byte) {
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
        .all(is_http_token_byte)
        .then(|| value.to_string())
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use base64::Engine as _;
    use http::{Method, Request};
    use serde_json::json;

    use super::{
        build_chatgpt_web_image_request_body, build_codex_openai_image_api_provider_request_body,
        build_openai_image_api_provider_request_body, build_openai_image_provider_request_body,
        decode_multipart_body_base64_with_limit, find_multipart_boundary,
        is_openai_image_stream_request, multipart_boundary, normalize_openai_image_quality,
        normalize_openai_image_request, normalize_openai_image_request_with_options,
        openai_image_operation_from_path, parse_multipart_fields,
        project_codex_openai_image_api_request_body, project_openai_image_api_request_body,
        OpenAiImageNormalizeOptions, OpenAiImageOperation, MAX_MULTIPART_BOUNDARY_BYTES,
        MAX_MULTIPART_PARTS, MAX_MULTIPART_PART_HEADER_BYTES,
    };
    use crate::formats::openai::image::spec::{resolve_stream_spec, resolve_sync_spec};

    fn request_parts(path: &str, content_type: Option<&str>) -> http::request::Parts {
        let mut builder = Request::builder().method(Method::POST).uri(path);
        if let Some(content_type) = content_type {
            builder = builder.header(http::header::CONTENT_TYPE, content_type);
        }
        builder
            .body(())
            .expect("request should build")
            .into_parts()
            .0
    }

    #[test]
    fn image_quality_uses_one_normalized_contract() {
        for (input, expected) in [
            ("auto", Some("auto")),
            ("low", Some("low")),
            ("medium", Some("medium")),
            ("standard", Some("medium")),
            ("high", Some("high")),
            ("hd", Some("high")),
            ("unsupported", None),
        ] {
            assert_eq!(normalize_openai_image_quality(input), expected);
        }
    }

    #[test]
    fn resolves_openai_image_sync_spec() {
        let spec = resolve_sync_spec("openai_image_sync").expect("spec");
        assert_eq!(spec.api_format, "openai:image");
        assert_eq!(spec.report_kind, "openai_image_sync_finalize");
        assert!(!spec.require_streaming);
    }

    #[test]
    fn resolves_openai_image_stream_spec() {
        let spec = resolve_stream_spec("openai_image_stream").expect("spec");
        assert_eq!(spec.api_format, "openai:image");
        assert_eq!(spec.report_kind, "openai_image_stream_success");
        assert!(spec.require_streaming);
    }

    #[test]
    fn detects_image_stream_flag_from_json_and_multipart() {
        let parts = request_parts("/v1/images/generations", Some("application/json"));
        assert!(is_openai_image_stream_request(
            &parts,
            &json!({"stream": true}),
            None
        ));

        let boundary = "boundary-stream-123";
        let body = format!(
            concat!(
                "--{boundary}\r\n",
                "Content-Disposition: form-data; name=\"stream\"\r\n\r\n",
                "true\r\n",
                "--{boundary}--\r\n"
            ),
            boundary = boundary,
        );
        let body_base64 = base64::engine::general_purpose::STANDARD.encode(body.as_bytes());
        let parts = request_parts(
            "/v1/images/edits",
            Some(&format!("multipart/form-data; boundary={boundary}")),
        );
        assert!(is_openai_image_stream_request(
            &parts,
            &json!({}),
            Some(&body_base64)
        ));
    }

    #[test]
    fn multipart_boundary_requires_rfc_token_and_length_limits() {
        assert_eq!(
            multipart_boundary("Multipart/Form-Data; boundary=quoted-token-123").as_deref(),
            Some("quoted-token-123")
        );
        assert_eq!(
            multipart_boundary("multipart/form-data; boundary=\"quoted-token-123\"").as_deref(),
            Some("quoted-token-123")
        );

        for content_type in [
            "multipart/form-data; boundary=bad boundary",
            "multipart/form-data; boundary=bad\"quote",
            "multipart/form-data; boundary=\"unterminated",
            "multipart/form-data; boundary=first; boundary=second",
            "multipart/form-data; foo",
            "multipart/form-data; foo=\"unterminated; boundary=valid",
            "multipart/form-data; boundary=valid trailing",
            "application/json; boundary=valid-token",
        ] {
            assert!(
                multipart_boundary(content_type).is_none(),
                "{content_type:?}"
            );
        }

        let oversized = "a".repeat(MAX_MULTIPART_BOUNDARY_BYTES + 1);
        assert!(
            multipart_boundary(&format!("multipart/form-data; boundary={oversized}")).is_none()
        );
    }

    #[test]
    fn multipart_boundary_accepts_quoted_unknown_parameter_with_semicolon() {
        assert_eq!(
            multipart_boundary("multipart/form-data; note=\"semi;colon\"; boundary=quoted-token")
                .as_deref(),
            Some("quoted-token")
        );
    }

    #[test]
    fn multipart_boundary_rejects_escaped_or_duplicate_content_type_parameters() {
        for content_type in [
            "multipart/form-data; boundary=\"escaped\\\"token\"",
            "multipart/form-data; note=one; NOTE=two; boundary=token",
            "multipart/form-data; note=\"unterminated; boundary=token",
            "multipart/form-data; note=\"closed\"trailing; boundary=token",
        ] {
            assert!(
                multipart_boundary(content_type).is_none(),
                "malformed content type must be rejected: {content_type:?}"
            );
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
    fn multipart_body_base64_decode_enforces_allocation_limit() {
        let exact = vec![b'x'; 6];
        let exact_encoded = base64::engine::general_purpose::STANDARD.encode(&exact);
        assert_eq!(
            decode_multipart_body_base64_with_limit(&exact_encoded, exact.len()),
            Some(exact)
        );

        let oversized = vec![b'x'; 7];
        let oversized_encoded = base64::engine::general_purpose::STANDARD.encode(oversized);
        assert_eq!(
            decode_multipart_body_base64_with_limit(&oversized_encoded, 6),
            None
        );

        let same_encoded_bucket = base64::engine::general_purpose::STANDARD.encode([b'x'; 6]);
        assert_eq!(
            decode_multipart_body_base64_with_limit(&same_encoded_bucket, 5),
            None
        );
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
    fn multipart_scanner_skips_short_prefix_before_later_valid_boundary() {
        let delimiter = b"--scanner-boundary";
        let haystack = b"x--scanner-boundary\r\ncontent\r\n--scanner-boundary\r\n";
        assert_eq!(
            find_multipart_boundary(haystack, delimiter, false),
            Some(haystack.len() - delimiter.len() - 2)
        );
    }

    #[test]
    fn openai_image_variation_path_is_not_supported() {
        let boundary = "boundary-variation-123";
        let body = format!(
            concat!(
                "--{boundary}\r\n",
                "Content-Disposition: form-data; name=\"image\"; filename=\"image.png\"\r\n",
                "Content-Type: image/png\r\n\r\n",
                "hello\r\n",
                "--{boundary}--\r\n"
            ),
            boundary = boundary,
        );
        let body_base64 = base64::engine::general_purpose::STANDARD.encode(body.as_bytes());
        let parts = request_parts(
            "/v1/images/variations",
            Some(&format!("multipart/form-data; boundary={boundary}")),
        );

        assert!(openai_image_operation_from_path("/v1/images/variations").is_none());
        assert!(normalize_openai_image_request(&parts, &json!({}), Some(&body_base64)).is_none());
    }

    #[test]
    fn normalize_edit_multipart_request_accepts_mixed_case_boundary() {
        let boundary = "------------------------OYNWsMZCt0ILTwn8naP4Gb";
        let body = format!(
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
        );
        let body_base64 = base64::engine::general_purpose::STANDARD.encode(body.as_bytes());
        let parts = request_parts(
            "/v1/images/edits",
            Some(&format!("multipart/form-data; boundary={boundary}")),
        );

        let request = normalize_openai_image_request(&parts, &json!({}), Some(&body_base64))
            .expect("edit request should normalize");

        assert_eq!(request.operation, OpenAiImageOperation::Edit);
        assert_eq!(request.requested_model.as_deref(), Some("gpt-image-2"));
        assert_eq!(request.prompt.as_deref(), Some("edit this image"));
        assert_eq!(request.images.len(), 1);
        assert_eq!(
            request.tool.get("action").and_then(|value| value.as_str()),
            Some("edit")
        );
    }

    #[test]
    fn normalize_edit_json_request_maps_mask_to_input_image_mask() {
        let parts = request_parts("/v1/images/edits", Some("application/json"));
        let request = normalize_openai_image_request(
            &parts,
            &json!({
                "model": "gpt-image-2",
                "prompt": "edit this image",
                "response_format": "b64_json",
                "image": {
                    "b64_json": "aGVsbG8=",
                    "mime_type": "image/png"
                },
                "mask": {
                    "b64_json": "d29ybGQ=",
                    "mime_type": "image/png"
                }
            }),
            None,
        )
        .expect("edit request should normalize");

        assert_eq!(request.operation, OpenAiImageOperation::Edit);
        assert_eq!(request.requested_model.as_deref(), Some("gpt-image-2"));
        assert_eq!(request.images.len(), 1);
        assert!(request.tool.get("mask").is_none());
        assert_eq!(
            request
                .tool
                .get("input_image_mask")
                .and_then(|value| value.get("image_url"))
                .and_then(|value| value.as_str()),
            Some("data:image/png;base64,d29ybGQ=")
        );
    }

    #[test]
    fn normalize_generate_json_request_accepts_custom_model_name() {
        let parts = request_parts("/v1/images/generations", Some("application/json"));
        let request = normalize_openai_image_request(
            &parts,
            &json!({
                "model": " Custom/Image-Model:V1 ",
                "prompt": "generate image"
            }),
            None,
        )
        .expect("custom image model request should normalize");

        assert_eq!(
            request.requested_model.as_deref(),
            Some("Custom/Image-Model:V1")
        );
    }

    #[test]
    fn normalize_generate_json_request_keeps_allowed_multi_image_count() {
        let parts = request_parts("/v1/images/generations", Some("application/json"));
        let request = normalize_openai_image_request_with_options(
            &parts,
            &json!({
                "model": "grok-imagine-image",
                "prompt": "generate image",
                "n": 4
            }),
            None,
            OpenAiImageNormalizeOptions::with_max_generation_count(4),
        )
        .expect("grok generation request should allow n up to four");

        let provider_request_body = build_openai_image_provider_request_body(&request);
        assert_eq!(provider_request_body["n"], json!(4));
    }

    #[test]
    fn normalize_generate_json_request_uses_the_openai_image_count_range_by_default() {
        let parts = request_parts("/v1/images/generations", Some("application/json"));
        let request = normalize_openai_image_request(
            &parts,
            &json!({
                "model": "gpt-image-2",
                "prompt": "generate image",
                "n": 10
            }),
            None,
        )
        .expect("OpenAI image generation should allow n up to ten");
        assert_eq!(request.image_count, Some(10));
        assert!(normalize_openai_image_request(
            &parts,
            &json!({
                "model": "gpt-image-2",
                "prompt": "generate image",
                "n": 11
            }),
            None,
        )
        .is_none());
    }

    #[test]
    fn normalize_edit_request_uses_the_configured_image_count_range() {
        let parts = request_parts("/v1/images/edits", Some("application/json"));
        let request = normalize_openai_image_request_with_options(
            &parts,
            &json!({
                "model": "grok-imagine-image-edit",
                "prompt": "edit image",
                "n": 4,
                "image": {
                    "b64_json": "aGVsbG8=",
                    "mime_type": "image/png"
                }
            }),
            None,
            OpenAiImageNormalizeOptions::with_max_generation_count(4),
        )
        .expect("image edits should use the configured output count range");
        assert_eq!(request.image_count, Some(4));
    }

    #[test]
    fn builds_codex_image_generation_with_the_typed_images_contract() {
        let parts = request_parts("/v1/images/generations", Some("application/json"));
        let request = normalize_openai_image_request(
            &parts,
            &json!({
                "model": "gpt-image-2",
                "prompt": "generate image",
                "background": "auto",
                "quality": "auto",
                "size": "auto",
                "stream": true,
                "response_format": "b64_json"
            }),
            None,
        )
        .expect("generation request should normalize");

        assert_eq!(
            build_codex_openai_image_api_provider_request_body(
                &request,
                Some("gpt-image-2"),
                false,
            ),
            Some(json!({
                "prompt": "generate image",
                "background": "auto",
                "model": "gpt-image-2",
                "quality": "auto",
                "size": "auto"
            }))
        );
    }

    #[test]
    fn builds_codex_image_edit_with_plural_image_urls() {
        let parts = request_parts("/v1/images/edits", Some("application/json"));
        let request = normalize_openai_image_request(
            &parts,
            &json!({
                "model": "gpt-image-2",
                "prompt": "add a red hat",
                "images": [
                    {"image_url": "data:image/png;base64,Zm9v"},
                    {"image_url": "https://example.test/reference.png"}
                ],
                "background": "auto",
                "quality": "auto",
                "size": "auto"
            }),
            None,
        )
        .expect("edit request should normalize");

        assert_eq!(
            build_codex_openai_image_api_provider_request_body(&request, None, false),
            Some(json!({
                "images": [
                    {"image_url": "data:image/png;base64,Zm9v"},
                    {"image_url": "https://example.test/reference.png"}
                ],
                "prompt": "add a red hat",
                "background": "auto",
                "model": "gpt-image-2",
                "quality": "auto",
                "size": "auto"
            }))
        );
    }

    #[test]
    fn rejects_fields_outside_the_codex_images_contract() {
        let parts = request_parts("/v1/images/edits", Some("application/json"));
        for unsupported in [
            json!({"mask": "data:image/png;base64,bWFzaw=="}),
            json!({"input_fidelity": "high"}),
            json!({"output_format": "png"}),
            json!({"partial_images": 1}),
            json!({"response_format": "url"}),
            json!({"user": "user-123"}),
        ] {
            let mut body = json!({
                "model": "gpt-image-2",
                "prompt": "edit image",
                "image": {"image_url": "data:image/png;base64,aW1hZ2U="}
            });
            body.as_object_mut()
                .expect("body object")
                .extend(unsupported.as_object().expect("unsupported object").clone());
            let request = normalize_openai_image_request(&parts, &body, None)
                .expect("request should normalize before provider projection");
            assert!(
                build_codex_openai_image_api_provider_request_body(&request, None, false).is_none(),
                "unsupported body should be rejected: {body}"
            );
        }
    }

    #[test]
    fn codex_image_projection_rejects_invalid_source_image_parts() {
        for body in [
            json!({
                "model": "gpt-image-2",
                "prompt": "edit image",
                "image": {
                    "type": "wrong_type",
                    "image_url": "https://example.test/input.png"
                }
            }),
            json!({
                "model": "gpt-image-2",
                "prompt": "edit image",
                "images": [{
                    "type": "input_image",
                    "image_url": "https://example.test/input.png",
                    "detail": "high"
                }]
            }),
            json!({
                "model": "gpt-image-2",
                "prompt": "edit image",
                "images": [{"file_id": "file_123"}]
            }),
        ] {
            assert!(
                project_codex_openai_image_api_request_body(&body, OpenAiImageOperation::Edit,)
                    .is_none(),
                "invalid Codex image part should be rejected: {body}"
            );
        }
    }

    #[test]
    fn codex_image_projection_accepts_untyped_plural_image_urls() {
        let body = json!({
            "model": "gpt-image-2",
            "prompt": "edit image",
            "images": [{
                "image_url": "https://example.test/input.png"
            }]
        });

        assert_eq!(
            project_codex_openai_image_api_request_body(&body, OpenAiImageOperation::Edit),
            Some(json!({
                "images": [{
                    "image_url": "https://example.test/input.png"
                }],
                "prompt": "edit image",
                "model": "gpt-image-2"
            }))
        );
    }

    #[test]
    fn codex_image_projection_enforces_the_openai_output_count_range() {
        let valid = json!({
            "model": "gpt-image-2",
            "prompt": "generate image",
            "n": 10
        });
        assert_eq!(
            project_codex_openai_image_api_request_body(&valid, OpenAiImageOperation::Generate)
                .and_then(|body| body.get("n").cloned()),
            Some(json!(10))
        );

        for count in [0, 11] {
            let body = json!({
                "model": "gpt-image-2",
                "prompt": "generate image",
                "n": count
            });
            assert!(project_codex_openai_image_api_request_body(
                &body,
                OpenAiImageOperation::Generate
            )
            .is_none());
        }
    }

    #[test]
    fn image_api_projection_renders_quality_for_the_mapped_model_family() {
        for (model, quality, expected) in [
            ("gpt-image-2", "standard", Some("medium")),
            ("gpt-image-2", "hd", Some("high")),
            ("dall-e-3", "medium", Some("standard")),
            ("dall-e-3", "high", Some("hd")),
            ("dall-e-3", "low", None),
            ("dall-e-2", "standard", Some("standard")),
            ("dall-e-2", "high", None),
        ] {
            let body = json!({
                "model": model,
                "prompt": "generate image",
                "quality": quality
            });
            let projected = project_openai_image_api_request_body(
                &body,
                model,
                OpenAiImageOperation::Generate,
                10,
            );
            assert_eq!(
                projected
                    .as_ref()
                    .and_then(|body| body.get("quality"))
                    .and_then(|quality| quality.as_str()),
                expected,
                "model={model}, quality={quality}"
            );
        }

        let dall_e_3_multi = json!({
            "model": "dall-e-3",
            "prompt": "generate image",
            "n": 2
        });
        assert!(project_openai_image_api_request_body(
            &dall_e_3_multi,
            "dall-e-3",
            OpenAiImageOperation::Generate,
            1,
        )
        .is_none());
    }

    #[test]
    fn image_api_projection_enforces_model_field_profiles() {
        let gpt_image_2 = project_openai_image_api_request_body(
            &json!({
                "model": "gpt-image-2",
                "prompt": "generate image",
                "background": "opaque",
                "size": "1536x864",
                "response_format": "b64_json"
            }),
            "gpt-image-2",
            OpenAiImageOperation::Generate,
            10,
        )
        .expect("GPT Image 2 should accept its synchronous generation fields");
        assert!(gpt_image_2.get("response_format").is_none());
        assert_eq!(gpt_image_2["size"], "1536x864");

        for invalid in [
            json!({
                "model": "gpt-image-2",
                "prompt": "generate image",
                "background": "transparent"
            }),
            json!({
                "model": "gpt-image-2",
                "prompt": "generate image",
                "stream": true
            }),
            json!({
                "model": "gpt-image-2",
                "prompt": "generate image",
                "size": "1537x864"
            }),
        ] {
            assert!(project_openai_image_api_request_body(
                &invalid,
                "gpt-image-2",
                OpenAiImageOperation::Generate,
                10,
            )
            .is_none());
        }

        let dall_e_3 = project_openai_image_api_request_body(
            &json!({
                "model": "dall-e-3",
                "prompt": "generate image",
                "style": "natural",
                "quality": "high"
            }),
            "dall-e-3",
            OpenAiImageOperation::Generate,
            10,
        )
        .expect("DALL-E 3 should accept style and render its quality vocabulary");
        assert_eq!(dall_e_3["style"], "natural");
        assert_eq!(dall_e_3["quality"], "hd");
        assert!(project_openai_image_api_request_body(
            &json!({
                "model": "dall-e-3",
                "prompt": "generate image",
                "output_format": "png"
            }),
            "dall-e-3",
            OpenAiImageOperation::Generate,
            10,
        )
        .is_none());

        let streaming = project_openai_image_api_request_body(
            &json!({
                "model": "gpt-image-1.5",
                "prompt": "generate image",
                "stream": true,
                "partial_images": 2,
                "output_format": "webp",
                "output_compression": 80
            }),
            "gpt-image-1.5",
            OpenAiImageOperation::Generate,
            10,
        )
        .expect("stream-capable GPT Image models should accept partial images");
        assert_eq!(streaming["partial_images"], 2);

        let edit = json!({
            "model": "gpt-image-2",
            "prompt": "edit image",
            "image": {"type": "input_image", "image_url": "data:image/png;base64,aA=="},
            "input_fidelity": "high"
        });
        assert!(project_openai_image_api_request_body(
            &edit,
            "gpt-image-2",
            OpenAiImageOperation::Edit,
            10,
        )
        .is_some());
        assert!(project_openai_image_api_request_body(
            &edit,
            "gpt-image-1-mini",
            OpenAiImageOperation::Edit,
            10,
        )
        .is_none());
    }

    #[test]
    fn image_api_builder_preserves_dall_e_3_style() {
        let parts = request_parts("/v1/images/generations", Some("application/json"));
        let request = normalize_openai_image_request(
            &parts,
            &json!({
                "model": "dall-e-3",
                "prompt": "generate image",
                "style": "vivid"
            }),
            None,
        )
        .expect("DALL-E 3 style should normalize");
        let body = build_openai_image_api_provider_request_body(&request, Some("dall-e-3"), false)
            .expect("DALL-E 3 style should project");
        assert_eq!(body["style"], "vivid");
    }

    #[test]
    fn build_image_api_provider_request_body_keeps_images_api_shape() {
        let parts = request_parts("/v1/images/generations", Some("application/json"));
        let request = normalize_openai_image_request_with_options(
            &parts,
            &json!({
                "model": "grok-imagine-image-lite",
                "prompt": "draw a cat",
                "n": 1,
                "size": "1024x1024",
                "stream": true
            }),
            None,
            OpenAiImageNormalizeOptions::with_max_generation_count(4),
        )
        .expect("generation request should normalize");

        let provider_request_body = build_openai_image_api_provider_request_body(
            &request,
            Some("mapped-image-model"),
            true,
        )
        .expect("Images API body should satisfy the mapped model contract");

        assert_eq!(provider_request_body["model"], "mapped-image-model");
        assert_eq!(provider_request_body["prompt"], "draw a cat");
        assert_eq!(provider_request_body["n"], 1);
        assert_eq!(provider_request_body["size"], "1024x1024");
        assert_eq!(provider_request_body["stream"], true);
        assert!(provider_request_body.get("input").is_none());
        assert!(provider_request_body.get("tools").is_none());
    }

    #[test]
    fn build_image_api_provider_edit_request_body_keeps_images_edit_shape() {
        let parts = request_parts("/v1/images/edits", Some("application/json"));
        let request = normalize_openai_image_request(
            &parts,
            &json!({
                "model": "gpt-image-2",
                "prompt": "replace the background",
                "image": "data:image/png;base64,aW1hZ2U=",
                "mask": "data:image/png;base64,bWFzaw==",
                "input_fidelity": "high",
                "output_format": "png",
                "response_format": "url",
                "user": "user-123"
            }),
            None,
        )
        .expect("edit request should normalize");

        let provider_request_body = build_openai_image_api_provider_request_body(
            &request,
            Some("mapped-edit-model"),
            false,
        )
        .expect("Images API edit body should satisfy the mapped model contract");

        assert_eq!(provider_request_body["model"], "mapped-edit-model");
        assert_eq!(provider_request_body["prompt"], "replace the background");
        assert_eq!(provider_request_body["input_fidelity"], "high");
        assert_eq!(provider_request_body["output_format"], "png");
        assert_eq!(provider_request_body["response_format"], "url");
        assert_eq!(provider_request_body["user"], "user-123");
        assert_eq!(
            provider_request_body["images"],
            json!([{"image_url": "data:image/png;base64,aW1hZ2U="}])
        );
        assert!(provider_request_body.get("image").is_none());
        assert_eq!(
            provider_request_body["mask"]["image_url"],
            "data:image/png;base64,bWFzaw=="
        );
        assert!(provider_request_body.get("input").is_none());
        assert!(provider_request_body.get("tools").is_none());
        assert!(provider_request_body.get("action").is_none());
    }

    #[test]
    fn build_image_api_provider_edit_request_uses_plural_images_field() {
        let parts = request_parts("/v1/images/edits", Some("application/json"));
        let request = normalize_openai_image_request(
            &parts,
            &json!({
                "model": "gpt-image-2",
                "prompt": "combine the references",
                "images": [
                    {"image_url": "data:image/png;base64,Zm9v"},
                    {"image_url": "https://example.test/reference.png"}
                ]
            }),
            None,
        )
        .expect("multi-image edit request should normalize");

        let provider_request_body =
            build_openai_image_api_provider_request_body(&request, None, false)
                .expect("standard Images edit body should project");

        assert_eq!(
            provider_request_body["images"],
            json!([
                {"image_url": "data:image/png;base64,Zm9v"},
                {"image_url": "https://example.test/reference.png"}
            ])
        );
        assert!(provider_request_body.get("image").is_none());
    }

    #[test]
    fn build_image_api_provider_edit_request_preserves_file_id() {
        let parts = request_parts("/v1/images/edits", Some("application/json"));
        let request = normalize_openai_image_request(
            &parts,
            &json!({
                "model": "gpt-image-2",
                "prompt": "edit the uploaded image",
                "image": {"file_id": "file_123"}
            }),
            None,
        )
        .expect("file ID edit request should normalize");

        let provider_request_body =
            build_openai_image_api_provider_request_body(&request, None, false)
                .expect("standard Images edit body should project");

        assert_eq!(
            provider_request_body["images"],
            json!([{"file_id": "file_123"}])
        );
        assert!(provider_request_body.get("image").is_none());
    }

    #[test]
    fn chatgpt_web_accepts_1k_tier_and_1024_size() {
        let parts = request_parts("/v1/images/generations", Some("application/json"));
        let by_tier = build_chatgpt_web_image_request_body(
            &parts,
            &json!({
                "model": "gpt-image-2",
                "prompt": "draw",
                "resolution": "1K",
                "aspect_ratio": "16:9"
            }),
            None,
        )
        .expect("1K should pass");
        assert_eq!(by_tier["size"], "1344x768");
        assert_eq!(by_tier["ratio"], "16:9");
        assert_eq!(by_tier["size_best_effort"], false);

        let by_size = build_chatgpt_web_image_request_body(
            &parts,
            &json!({
                "model": "gpt-image-2",
                "prompt": "draw",
                "size": "1024x1024"
            }),
            None,
        )
        .expect("1024x1024 should pass");
        assert_eq!(by_size["size"], "1024x1024");
    }

    #[test]
    fn chatgpt_web_preserves_quality_and_partial_images() {
        let parts = request_parts("/v1/images/generations", Some("application/json"));
        let body = build_chatgpt_web_image_request_body(
            &parts,
            &json!({
                "model": "gpt-image-2",
                "prompt": "draw",
                "size": "1024x1024",
                "quality": "high",
                "partial_images": 2,
                "output_format": "png"
            }),
            None,
        )
        .expect("request should pass");

        assert_eq!(body["quality"], "high");
        assert_eq!(body["partial_images"], 2);
        assert_eq!(body["output_format"], "png");
    }

    #[test]
    fn chatgpt_web_accepts_openai_image_edit_requests() {
        let parts = request_parts("/v1/images/edits", Some("application/json"));
        let body = build_chatgpt_web_image_request_body(
            &parts,
            &json!({
                "model": "gpt-image-2",
                "prompt": "make the sky brighter",
                "image": {
                    "b64_json": "aW1hZ2U=",
                    "mime_type": "image/png"
                },
                "size": "1024x1024",
                "quality": "high",
                "response_format": "b64_json",
                "output_format": "png",
                "user": "user-123"
            }),
            None,
        )
        .expect("ChatGPT-Web edit request should pass");

        assert_eq!(body["operation"], "edit");
        assert_eq!(body["model"], "gpt-image-2");
        assert_eq!(body["prompt"], "make the sky brighter");
        assert_eq!(body["size"], "1024x1024");
        assert_eq!(body["quality"], "high");
        assert_eq!(body["response_format"], "b64_json");
        assert_eq!(body["output_format"], "png");
        assert_eq!(body["user"], "user-123");
        assert_eq!(body["count"], 1);
        assert_eq!(body["images"], json!(["data:image/png;base64,aW1hZ2U="]));
    }

    #[test]
    fn chatgpt_web_rejects_oversized_resolution_or_size() {
        let parts = request_parts("/v1/images/generations", Some("application/json"));

        for body in [
            json!({"prompt":"draw","resolution":"2K"}),
            json!({"prompt":"draw","size_tier":"4K"}),
            json!({"prompt":"draw","size":"2048x2048"}),
        ] {
            let err = build_chatgpt_web_image_request_body(&parts, &body, None)
                .expect_err("oversized request should fail");
            assert_eq!(err.status_code, 400);
            assert_eq!(err.error_type, "invalid_request_error");
            assert!(err.message.contains("ChatGPT-Web"));
            assert!(err.message.contains("resolution"));
        }
    }

    #[test]
    fn chatgpt_web_accepts_smaller_size_as_best_effort() {
        let parts = request_parts("/v1/images/generations", Some("application/json"));
        let body = build_chatgpt_web_image_request_body(
            &parts,
            &json!({
                "model": "gpt-image-2",
                "prompt": "draw",
                "size": "512x512"
            }),
            None,
        )
        .expect("smaller size should pass");

        assert_eq!(body["size"], "512x512");
        assert_eq!(body["ratio"], "1:1");
        assert_eq!(body["size_best_effort"], true);
    }

    #[test]
    fn chatgpt_web_rejects_file_id_and_mask_inputs() {
        let edit_parts = request_parts("/v1/images/edits", Some("application/json"));

        let file_id_err = build_chatgpt_web_image_request_body(
            &edit_parts,
            &json!({
                "prompt": "edit",
                "image": {"file_id": "file_123"}
            }),
            None,
        )
        .expect_err("file_id should not be supported");
        assert!(file_id_err.message.contains("file_id"));

        let mask_err = build_chatgpt_web_image_request_body(
            &edit_parts,
            &json!({
                "prompt": "edit",
                "image": "data:image/png;base64,aW1hZ2U=",
                "mask": "data:image/png;base64,bWFzaw=="
            }),
            None,
        )
        .expect_err("mask should not be supported");
        assert!(mask_err.message.contains("mask"));
    }
}
