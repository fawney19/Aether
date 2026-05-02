use std::collections::BTreeMap;

use base64::Engine as _;
use serde_json::{json, Map, Number, Value};

use crate::contracts::{OPENAI_IMAGE_STREAM_PLAN_KIND, OPENAI_IMAGE_SYNC_PLAN_KIND};
use crate::request::standard::{
    CODEX_OPENAI_IMAGE_DEFAULT_MODEL, CODEX_OPENAI_IMAGE_DEFAULT_VARIATION_MODEL,
};

#[derive(Debug, Clone, Copy)]
pub struct LocalOpenAiImageSpec {
    pub api_format: &'static str,
    pub decision_kind: &'static str,
    pub report_kind: &'static str,
    pub require_streaming: bool,
}

pub fn resolve_sync_spec(plan_kind: &str) -> Option<LocalOpenAiImageSpec> {
    match plan_kind {
        OPENAI_IMAGE_SYNC_PLAN_KIND => Some(LocalOpenAiImageSpec {
            api_format: "openai:image",
            decision_kind: OPENAI_IMAGE_SYNC_PLAN_KIND,
            report_kind: "openai_image_sync_finalize",
            require_streaming: false,
        }),
        _ => None,
    }
}

pub fn resolve_stream_spec(plan_kind: &str) -> Option<LocalOpenAiImageSpec> {
    match plan_kind {
        OPENAI_IMAGE_STREAM_PLAN_KIND => Some(LocalOpenAiImageSpec {
            api_format: "openai:image",
            decision_kind: OPENAI_IMAGE_STREAM_PLAN_KIND,
            report_kind: "openai_image_stream_success",
            require_streaming: true,
        }),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenAiImageOperation {
    Generate,
    Edit,
    Variation,
}

impl OpenAiImageOperation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Generate => "generate",
            Self::Edit => "edit",
            Self::Variation => "variation",
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
    user: Option<String>,
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
        "/v1/images/variations" => Some(OpenAiImageOperation::Variation),
        _ => None,
    }
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
        OpenAiImageOperation::Variation => CODEX_OPENAI_IMAGE_DEFAULT_VARIATION_MODEL,
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
    let operation = openai_image_operation_from_path(parts.uri.path())?;
    if let Some(body_base64) = body_base64 {
        normalize_openai_image_multipart_request(parts, body_base64, operation)
    } else {
        normalize_openai_image_json_request(body_json, operation)
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
    Value::Object(body)
}

fn normalize_openai_image_json_request(
    body_json: &Value,
    operation: OpenAiImageOperation,
) -> Option<NormalizedOpenAiImageRequest> {
    let object = body_json.as_object()?;
    if object
        .get("style")
        .and_then(Value::as_str)
        .map(str::trim)
        .is_some_and(|value| !value.is_empty())
    {
        return None;
    }
    if object
        .get("n")
        .and_then(image_request_count)
        .is_some_and(|value| value != 1)
    {
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
    if matches!(
        operation,
        OpenAiImageOperation::Edit | OpenAiImageOperation::Variation
    ) && images.is_empty()
    {
        return None;
    }

    let tool = build_tool_options_from_json(object, operation, mask.as_ref())?;

    Some(NormalizedOpenAiImageRequest {
        operation,
        requested_model,
        prompt,
        images,
        tool,
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
) -> Option<NormalizedOpenAiImageRequest> {
    let multipart_fields = parse_multipart_fields_from_base64(parts, body_base64)?;
    let requested_model = normalize_requested_image_model(
        find_multipart_text_field(&multipart_fields, "model").as_deref(),
    );
    if find_multipart_text_field(&multipart_fields, "style").is_some() {
        return None;
    }
    if find_multipart_text_field(&multipart_fields, "n")
        .and_then(|value| value.trim().parse::<u64>().ok())
        .is_some_and(|value| value != 1)
    {
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
        ) {
            raw_tool_values.insert(
                name,
                String::from_utf8_lossy(&field.data).trim().to_string(),
            );
        }
    }

    if matches!(
        operation,
        OpenAiImageOperation::Edit | OpenAiImageOperation::Variation
    ) && images.is_empty()
    {
        return None;
    }

    let tool = build_tool_options_from_multipart(raw_tool_values, operation, mask.as_ref())?;

    Some(NormalizedOpenAiImageRequest {
        operation,
        requested_model,
        prompt,
        images,
        tool,
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
    match operation {
        OpenAiImageOperation::Generate | OpenAiImageOperation::Edit => prompt.map(Some),
        OpenAiImageOperation::Variation => Some(prompt),
    }
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
                OpenAiImageOperation::Edit | OpenAiImageOperation::Variation => "edit",
            }
            .to_string(),
        ),
    );
    for (key, value) in raw_values {
        let normalized = match key.as_str() {
            "size" | "background" | "moderation" | "input_fidelity" => {
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

fn normalize_quality_value(value: &Value) -> Option<Value> {
    let quality = value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_ascii_lowercase();
    let normalized = match quality.as_str() {
        "low" => "low",
        "medium" => "medium",
        "high" => "high",
        "standard" => "medium",
        "hd" => "high",
        _ => return None,
    };
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
    let body_bytes = base64::engine::general_purpose::STANDARD
        .decode(body_base64)
        .ok()?;
    Some(parse_multipart_fields(&body_bytes, boundary.as_str()))
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
    let mut content_type = None;
    for line in header_text.lines() {
        let trimmed = line.trim();
        let lower = trimmed.to_ascii_lowercase();
        if lower.starts_with("content-disposition:") {
            name = extract_quoted_header_value(trimmed, "name");
        } else if lower.starts_with("content-type:") {
            content_type = trimmed
                .split_once(':')
                .map(|(_, value)| value.trim().to_string())
                .filter(|value| !value.is_empty());
        }
    }

    Some(MultipartField {
        name: name?,
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

#[cfg(test)]
mod tests {
    use base64::Engine as _;
    use http::{Method, Request};
    use serde_json::json;

    use super::{
        build_openai_image_provider_request_body, is_openai_image_stream_request,
        normalize_openai_image_request, resolve_stream_spec, resolve_sync_spec,
        OpenAiImageOperation,
    };
    use crate::request::standard::{
        apply_codex_openai_responses_special_body_edits, CODEX_OPENAI_IMAGE_INTERNAL_MODEL,
    };

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
    fn normalize_variation_multipart_request_leaves_defaults_empty_until_codex_adapter() {
        let boundary = "boundary-variation-123";
        let body = format!(
            concat!(
                "--{boundary}\r\n",
                "Content-Disposition: form-data; name=\"image\"; filename=\"image.png\"\r\n",
                "Content-Type: image/png\r\n\r\n",
                "hello\r\n",
                "--{boundary}\r\n",
                "Content-Disposition: form-data; name=\"response_format\"\r\n\r\n",
                "url\r\n",
                "--{boundary}--\r\n"
            ),
            boundary = boundary,
        );
        let body_base64 = base64::engine::general_purpose::STANDARD.encode(body.as_bytes());
        let parts = request_parts(
            "/v1/images/variations",
            Some(&format!("multipart/form-data; boundary={boundary}")),
        );

        let request = normalize_openai_image_request(&parts, &json!({}), Some(&body_base64))
            .expect("variation request should normalize");

        assert_eq!(request.operation, OpenAiImageOperation::Variation);
        assert!(request.requested_model.is_none());
        assert_eq!(request.summary_json["response_format"], json!("url"));
        assert_eq!(
            request.tool.get("action").and_then(|value| value.as_str()),
            Some("edit")
        );
        assert!(request.tool.get("output_format").is_none());
        assert_eq!(request.images.len(), 1);

        let mut provider_request_body = build_openai_image_provider_request_body(&request);
        apply_codex_openai_responses_special_body_edits(
            &mut provider_request_body,
            "codex",
            "openai:image",
            None,
            None,
        );

        assert_eq!(
            provider_request_body["input"][0]["content"][0]["text"],
            json!("Create a faithful variation of the provided image.")
        );
        assert_eq!(
            provider_request_body["model"],
            json!(CODEX_OPENAI_IMAGE_INTERNAL_MODEL)
        );
        assert_eq!(
            provider_request_body["tools"][0]["output_format"],
            json!("png")
        );
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
    fn build_generate_request_defaults_codex_image_tool_and_tool_choice() {
        let parts = request_parts("/v1/images/generations", Some("application/json"));
        let request = normalize_openai_image_request(
            &parts,
            &json!({
                "model": "gpt-image-2",
                "prompt": "generate image"
            }),
            None,
        )
        .expect("generation request should normalize");

        assert!(request.tool.get("size").is_none());
        assert!(request.tool.get("quality").is_none());
        assert!(request.tool.get("background").is_none());
        assert!(request.tool.get("output_format").is_none());
        assert_eq!(
            request.tool.get("action").and_then(|value| value.as_str()),
            Some("generate")
        );

        let mut provider_request_body = build_openai_image_provider_request_body(&request);
        assert!(provider_request_body.get("model").is_none());
        assert!(provider_request_body.get("tool_choice").is_none());
        assert!(provider_request_body.get("stream").is_none());
        apply_codex_openai_responses_special_body_edits(
            &mut provider_request_body,
            "codex",
            "openai:image",
            None,
            None,
        );

        assert_eq!(
            provider_request_body
                .get("tools")
                .and_then(|value| value.get(0))
                .and_then(|value| value.get("type"))
                .and_then(|value| value.as_str()),
            Some("image_generation")
        );
        assert_eq!(
            provider_request_body
                .get("tools")
                .and_then(|value| value.get(0))
                .and_then(|value| value.get("action"))
                .and_then(|value| value.as_str()),
            Some("generate")
        );
        assert_eq!(
            provider_request_body
                .get("tools")
                .and_then(|value| value.get(0))
                .and_then(|value| value.get("size"))
                .and_then(|value| value.as_str()),
            Some("1024x1024")
        );
        assert_eq!(
            provider_request_body
                .get("tools")
                .and_then(|value| value.get(0))
                .and_then(|value| value.get("quality"))
                .and_then(|value| value.as_str()),
            Some("high")
        );
        assert_eq!(
            provider_request_body
                .get("tools")
                .and_then(|value| value.get(0))
                .and_then(|value| value.get("background"))
                .and_then(|value| value.as_str()),
            Some("auto")
        );
        assert_eq!(
            provider_request_body
                .get("tools")
                .and_then(|value| value.get(0))
                .and_then(|value| value.get("output_format"))
                .and_then(|value| value.as_str()),
            Some("png")
        );
        assert_eq!(
            provider_request_body
                .get("model")
                .and_then(|value| value.as_str()),
            Some(CODEX_OPENAI_IMAGE_INTERNAL_MODEL)
        );
        assert_eq!(
            provider_request_body
                .get("stream")
                .and_then(|value| value.as_bool()),
            Some(true)
        );
        assert_eq!(
            provider_request_body
                .get("tool_choice")
                .and_then(|value| value.get("type"))
                .and_then(|value| value.as_str()),
            Some("image_generation")
        );
    }
}
