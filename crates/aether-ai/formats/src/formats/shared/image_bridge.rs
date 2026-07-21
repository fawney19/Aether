use serde_json::{json, Map, Number, Value};

use crate::formats::shared::model_directives::extract_gemini_model_from_path;

#[derive(Clone, Debug, PartialEq)]
pub struct OpenAiImageRequestForGemini {
    pub requested_model: String,
    pub mapped_model: String,
    pub body_json: Value,
    pub summary_json: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GeminiImageRequestForOpenAi {
    pub requested_model: String,
    pub mapped_model: String,
    pub operation: crate::formats::openai::image::request::OpenAiImageOperation,
    pub body_json: Value,
    pub summary_json: Value,
}

pub fn build_gemini_image_request_body_from_openai_image_request(
    normalized_request: &crate::formats::openai::image::request::NormalizedOpenAiImageRequest,
    mapped_model: &str,
) -> Option<OpenAiImageRequestForGemini> {
    let mapped_model = mapped_model.trim();
    if mapped_model.is_empty() {
        return None;
    }
    if normalized_request_has_mask(normalized_request) {
        return None;
    }

    let prompt = normalized_request_prompt(normalized_request)
        .unwrap_or_else(|| "Generate a high quality image.".to_string());
    let mut parts = Vec::new();
    if !prompt.trim().is_empty() {
        parts.push(json!({ "text": prompt }));
    }
    for image in normalized_request_images(normalized_request) {
        if let Some(part) = openai_input_image_to_gemini_part(image) {
            parts.push(part);
        }
    }
    if parts.is_empty() {
        return None;
    }

    let mut generation_config = Map::new();
    generation_config.insert("responseModalities".to_string(), json!(["TEXT", "IMAGE"]));
    if let Some(size) = normalized_request_tool(normalized_request)
        .get("size")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        generation_config.insert("imageSize".to_string(), Value::String(size.to_string()));
    }

    let body_json = json!({
        "model": mapped_model,
        "contents": [{
            "role": "user",
            "parts": parts
        }],
        "generationConfig": Value::Object(generation_config),
    });
    let requested_model = normalized_request
        .requested_model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(mapped_model)
        .to_string();

    Some(OpenAiImageRequestForGemini {
        requested_model,
        mapped_model: mapped_model.to_string(),
        summary_json: normalized_request.summary_json.clone(),
        body_json,
    })
}

pub fn gemini_request_is_image_generation(body_json: &Value) -> bool {
    body_json
        .as_object()
        .and_then(|object| {
            object
                .get("generationConfig")
                .or_else(|| object.get("generation_config"))
        })
        .and_then(Value::as_object)
        .and_then(|generation_config| {
            generation_config
                .get("responseModalities")
                .or_else(|| generation_config.get("response_modalities"))
        })
        .is_some_and(value_has_image_modality)
}

pub fn resolve_requested_gemini_image_model_for_request(
    body_json: &Value,
    request_path: &str,
) -> Option<String> {
    body_json
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| extract_gemini_model_from_path(request_path))
}

pub fn build_openai_image_request_body_from_gemini_image_request(
    body_json: &Value,
    request_path: &str,
    mapped_model: &str,
) -> Option<GeminiImageRequestForOpenAi> {
    if !gemini_request_is_image_generation(body_json) {
        return None;
    }
    let mapped_model = mapped_model.trim();
    if mapped_model.is_empty() {
        return None;
    }
    let requested_model =
        resolve_requested_gemini_image_model_for_request(body_json, request_path)?;
    let mut content = Vec::new();
    let mut prompt_parts = Vec::new();
    collect_gemini_request_text(body_json.get("systemInstruction"), &mut prompt_parts);
    collect_gemini_request_text(body_json.get("system_instruction"), &mut prompt_parts);
    collect_gemini_contents(body_json.get("contents"), &mut prompt_parts, &mut content);

    let prompt = prompt_parts
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    if prompt.is_empty() {
        return None;
    }

    let operation = if content.iter().any(|value| {
        value
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| kind == "input_image")
    }) {
        crate::formats::openai::image::request::OpenAiImageOperation::Edit
    } else {
        crate::formats::openai::image::request::OpenAiImageOperation::Generate
    };
    let mut body = Map::new();
    body.insert("model".to_string(), Value::String(mapped_model.to_string()));
    body.insert("prompt".to_string(), Value::String(prompt));
    if operation == crate::formats::openai::image::request::OpenAiImageOperation::Edit {
        let images = content
            .iter()
            .filter(|value| value.get("type").and_then(Value::as_str) == Some("input_image"))
            .map(|value| {
                let image_url = value
                    .get("image_url")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())?;
                Some(json!({ "image_url": image_url }))
            })
            .collect::<Option<Vec<_>>>()?;
        if images.is_empty() {
            return None;
        }
        crate::formats::openai::image::request::insert_standard_openai_image_inputs(
            &mut body, images,
        );
    }
    let body_json = Value::Object(body);
    let summary_json = json!({
        "operation": operation.as_str(),
        "response_format": "b64_json",
    });

    Some(GeminiImageRequestForOpenAi {
        requested_model,
        mapped_model: mapped_model.to_string(),
        operation,
        body_json,
        summary_json,
    })
}

pub fn build_openai_image_response_from_gemini_response(
    provider_body_json: &Value,
    report_context: Option<&Value>,
) -> Option<Value> {
    let mut images = Vec::new();
    let mut revised_prompt = None::<Value>;
    for candidate in provider_body_json.get("candidates")?.as_array()? {
        let Some(parts) = candidate
            .get("content")
            .and_then(|value| value.get("parts"))
            .and_then(Value::as_array)
        else {
            continue;
        };
        for part in parts {
            if let Some(text) = part
                .get("text")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                revised_prompt = Some(Value::String(text.to_string()));
            }
            let Some((mime_type, b64_json)) = extract_gemini_inline_image(part) else {
                continue;
            };
            images.push(json!({
                "b64_json": b64_json,
                "output_format": output_format_from_mime_type(&mime_type),
                "revised_prompt": revised_prompt.clone().unwrap_or(Value::Null),
            }));
        }
    }
    if images.is_empty() {
        return None;
    }

    let created = report_context
        .and_then(|context| context.get("created"))
        .and_then(Value::as_i64)
        .unwrap_or_default();
    let mut response = Map::new();
    response.insert("created".to_string(), Value::Number(Number::from(created)));
    response.insert("data".to_string(), Value::Array(images));
    if let Some(model) = provider_body_json
        .get("modelVersion")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| report_context.and_then(context_model))
    {
        response.insert("model".to_string(), Value::String(model.to_string()));
    }
    if let Some(usage) = gemini_usage_to_openai_image_usage(provider_body_json.get("usageMetadata"))
    {
        response.insert("usage".to_string(), usage);
    }
    Some(Value::Object(response))
}

pub fn build_gemini_image_response_from_openai_image_response(
    provider_body_json: &Value,
    report_context: Option<&Value>,
) -> Option<Value> {
    let mut parts = Vec::new();
    for item in provider_body_json.get("data")?.as_array()? {
        if let Some(prompt) = item
            .get("revised_prompt")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            parts.push(json!({ "text": prompt }));
        }
        let Some((mime_type, data)) = extract_openai_image_response_item(item) else {
            continue;
        };
        parts.push(json!({
            "inlineData": {
                "mimeType": mime_type,
                "data": data,
            }
        }));
    }
    if !parts.iter().any(is_gemini_inline_image_part) {
        return None;
    }

    let model = provider_body_json
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| report_context.and_then(context_model))
        .unwrap_or("unknown");
    let mut response = Map::new();
    response.insert("modelVersion".to_string(), Value::String(model.to_string()));
    response.insert(
        "candidates".to_string(),
        json!([{
            "index": 0,
            "content": {
                "role": "model",
                "parts": parts,
            },
            "finishReason": "STOP",
        }]),
    );
    if let Some(usage) =
        openai_image_usage_to_gemini_usage_metadata(provider_body_json.get("usage"))
    {
        response.insert("usageMetadata".to_string(), usage);
    }
    Some(Value::Object(response))
}

pub fn build_gemini_image_response_from_openai_responses_image_response(
    provider_body_json: &Value,
    report_context: Option<&Value>,
) -> Option<Value> {
    let output = provider_body_json.get("output").and_then(Value::as_array)?;
    let mut parts = Vec::new();
    for item in output {
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or_default();
        if item_type == "image_generation_call" {
            if let Some(prompt) = item
                .get("revised_prompt")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                parts.push(json!({ "text": prompt }));
            }
            let Some(b64_json) = item
                .get("result")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                continue;
            };
            let mime_type = item
                .get("output_format")
                .and_then(Value::as_str)
                .map(mime_type_from_output_format)
                .unwrap_or_else(|| "image/png".to_string());
            parts.push(json!({
                "inlineData": {
                    "mimeType": mime_type,
                    "data": b64_json,
                }
            }));
            continue;
        }
        if matches!(
            item_type,
            "message" | "output_text" | "text" | "output_image" | "image_url"
        ) {
            collect_openai_response_output_item_for_gemini(item, &mut parts);
        }
    }
    if !parts.iter().any(is_gemini_inline_image_part) {
        return None;
    }

    let model = provider_body_json
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| report_context.and_then(context_model))
        .unwrap_or("unknown");
    let mut response = Map::new();
    response.insert("modelVersion".to_string(), Value::String(model.to_string()));
    response.insert(
        "candidates".to_string(),
        json!([{
            "index": 0,
            "content": {
                "role": "model",
                "parts": parts,
            },
            "finishReason": "STOP",
        }]),
    );
    if let Some(usage) =
        openai_image_usage_to_gemini_usage_metadata(provider_body_json.get("usage"))
    {
        response.insert("usageMetadata".to_string(), usage);
    }
    Some(Value::Object(response))
}

pub fn build_openai_image_response_from_response_stream_sync_body(
    provider_body_json: &Value,
    report_context: Option<&Value>,
) -> Option<Value> {
    let output = provider_body_json.get("output").and_then(Value::as_array)?;
    let images = output
        .iter()
        .filter_map(openai_response_image_generation_item_to_image_data)
        .collect::<Vec<_>>();
    if images.is_empty() {
        return None;
    }
    let created = provider_body_json
        .get("created_at")
        .or_else(|| provider_body_json.get("created"))
        .and_then(Value::as_i64)
        .unwrap_or_default();
    let mut response = Map::new();
    response.insert("created".to_string(), Value::Number(Number::from(created)));
    response.insert("data".to_string(), Value::Array(images));
    if let Some(model) = provider_body_json
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| report_context.and_then(context_model))
    {
        response.insert("model".to_string(), Value::String(model.to_string()));
    }
    if let Some(usage) = provider_body_json
        .get("tool_usage")
        .and_then(|value| value.get("image_gen"))
        .or_else(|| provider_body_json.get("usage"))
        .cloned()
    {
        response.insert("usage".to_string(), usage);
    }
    Some(Value::Object(response))
}

fn openai_response_image_generation_item_to_image_data(item: &Value) -> Option<Value> {
    if item.get("type").and_then(Value::as_str) != Some("image_generation_call") {
        return None;
    }
    let result = item
        .get("result")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let url = item
        .get("url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let mut image = Map::new();
    match result {
        Some(value) if value.starts_with("data:") => {
            let (_, b64_json) = parse_data_url(value)?;
            image.insert("b64_json".to_string(), Value::String(b64_json));
        }
        Some(value) if value.starts_with("http://") || value.starts_with("https://") => {
            image.insert("url".to_string(), Value::String(value.to_string()));
        }
        Some(value) => {
            image.insert("b64_json".to_string(), Value::String(value.to_string()));
        }
        None => {
            let url = url?;
            if let Some((_, b64_json)) = parse_data_url(url) {
                image.insert("b64_json".to_string(), Value::String(b64_json));
            } else {
                image.insert("url".to_string(), Value::String(url.to_string()));
            }
        }
    }
    image.insert(
        "revised_prompt".to_string(),
        item.get("revised_prompt").cloned().unwrap_or(Value::Null),
    );
    Some(Value::Object(image))
}

pub fn build_openai_image_provider_body_from_response_stream_sync_body(
    provider_body_json: &Value,
    report_context: Option<&Value>,
) -> Option<Value> {
    let data = provider_body_json.get("data")?.as_array()?;
    if data.is_empty() {
        return None;
    }
    let output = data
        .iter()
        .filter_map(|item| {
            extract_openai_image_response_item(item).map(|(mime_type, _)| {
                json!({
                    "type": "image_generation_call",
                    "output_format": output_format_from_mime_type(&mime_type),
                    "revised_prompt": item.get("revised_prompt").cloned().unwrap_or(Value::Null),
                })
            })
        })
        .collect::<Vec<_>>();
    if output.is_empty() {
        return None;
    }
    Some(json!({
        "id": provider_body_json.get("id").cloned().unwrap_or(Value::Null),
        "object": "response",
        "model": provider_body_json
            .get("model")
            .cloned()
            .or_else(|| report_context.and_then(context_model).map(|value| Value::String(value.to_string())))
            .unwrap_or(Value::Null),
        "status": "completed",
        "usage": provider_body_json.get("usage").cloned().unwrap_or(Value::Null),
        "output": output,
    }))
}

fn normalized_request_prompt(
    request: &crate::formats::openai::image::request::NormalizedOpenAiImageRequest,
) -> Option<String> {
    let body =
        crate::formats::openai::image::request::build_openai_image_provider_request_body(request);
    body.get("input")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|message| message.get("content"))
        .find_map(openai_input_content_text)
}

fn normalized_request_images(
    request: &crate::formats::openai::image::request::NormalizedOpenAiImageRequest,
) -> Vec<Value> {
    let body =
        crate::formats::openai::image::request::build_openai_image_provider_request_body(request);
    body.get("input")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|message| message.get("content"))
        .flat_map(openai_input_content_images)
        .collect()
}

fn normalized_request_tool(
    request: &crate::formats::openai::image::request::NormalizedOpenAiImageRequest,
) -> Map<String, Value> {
    let body =
        crate::formats::openai::image::request::build_openai_image_provider_request_body(request);
    body.get("tools")
        .and_then(Value::as_array)
        .and_then(|tools| tools.first())
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

fn normalized_request_has_mask(
    request: &crate::formats::openai::image::request::NormalizedOpenAiImageRequest,
) -> bool {
    normalized_request_tool(request).contains_key("input_image_mask")
}

fn openai_input_content_text(content: &Value) -> Option<String> {
    match content {
        Value::String(text) => text_non_empty(text),
        Value::Array(items) => items.iter().find_map(|item| {
            item.as_object()
                .filter(|object| object.get("type").and_then(Value::as_str) == Some("input_text"))
                .and_then(|object| object.get("text").and_then(Value::as_str))
                .and_then(text_non_empty)
        }),
        _ => None,
    }
}

fn openai_input_content_images(content: &Value) -> Vec<Value> {
    match content {
        Value::Array(items) => items
            .iter()
            .filter(|item| item.get("type").and_then(Value::as_str) == Some("input_image"))
            .cloned()
            .collect(),
        _ => Vec::new(),
    }
}

fn openai_input_image_to_gemini_part(image: Value) -> Option<Value> {
    let object = image.as_object()?;
    let image_url = object
        .get("image_url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    if let Some((mime_type, data)) = parse_data_url(image_url) {
        return Some(json!({
            "inlineData": {
                "mimeType": mime_type,
                "data": data,
            }
        }));
    }
    Some(json!({
        "fileData": {
            "mimeType": mime_type_from_url(image_url),
            "fileUri": image_url,
        }
    }))
}

fn collect_gemini_contents(
    value: Option<&Value>,
    text: &mut Vec<String>,
    content: &mut Vec<Value>,
) {
    let Some(contents) = value else {
        return;
    };
    match contents {
        Value::Array(items) => {
            for item in items {
                collect_gemini_content(item, text, content);
            }
        }
        other => collect_gemini_content(other, text, content),
    }
}

fn collect_gemini_content(value: &Value, text: &mut Vec<String>, content: &mut Vec<Value>) {
    let Some(parts) = value
        .get("parts")
        .and_then(Value::as_array)
        .or_else(|| value.as_array())
    else {
        return;
    };
    for part in parts {
        collect_gemini_part(part, text, content);
    }
}

fn collect_gemini_request_text(value: Option<&Value>, text: &mut Vec<String>) {
    match value {
        Some(Value::String(value)) => {
            if let Some(value) = text_non_empty(value) {
                text.push(value);
            }
        }
        Some(Value::Object(object)) => {
            if let Some(parts) = object.get("parts").and_then(Value::as_array) {
                for part in parts {
                    if let Some(value) = part
                        .get("text")
                        .and_then(Value::as_str)
                        .and_then(text_non_empty)
                    {
                        text.push(value);
                    }
                }
            } else if let Some(value) = object
                .get("text")
                .and_then(Value::as_str)
                .and_then(text_non_empty)
            {
                text.push(value);
            }
        }
        _ => {}
    }
}

fn collect_gemini_part(part: &Value, text: &mut Vec<String>, content: &mut Vec<Value>) {
    if let Some(value) = part
        .get("text")
        .and_then(Value::as_str)
        .and_then(text_non_empty)
    {
        text.push(value);
        return;
    }
    if let Some((mime_type, data)) = extract_gemini_inline_image(part) {
        content.push(json!({
            "type": "input_image",
            "image_url": format!("data:{mime_type};base64,{data}"),
        }));
        return;
    }
    if let Some(file_data) = part
        .get("fileData")
        .or_else(|| part.get("file_data"))
        .and_then(Value::as_object)
    {
        let file_uri = file_data
            .get("fileUri")
            .or_else(|| file_data.get("file_uri"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if let Some(file_uri) = file_uri {
            content.push(json!({
                "type": "input_image",
                "image_url": file_uri,
            }));
        }
    }
}

fn collect_openai_response_output_item_for_gemini(item: &Value, parts: &mut Vec<Value>) {
    let item_type = item.get("type").and_then(Value::as_str).unwrap_or_default();
    if item_type == "message" {
        if let Some(content) = item.get("content").and_then(Value::as_array) {
            for part in content {
                collect_openai_response_output_item_for_gemini(part, parts);
            }
        }
        return;
    }
    if matches!(item_type, "output_text" | "text") {
        if let Some(text) = item
            .get("text")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            parts.push(json!({ "text": text }));
        }
        return;
    }
    if matches!(item_type, "output_image" | "image_url") {
        let image_url = item
            .get("image_url")
            .and_then(Value::as_str)
            .or_else(|| {
                item.get("image_url")
                    .and_then(Value::as_object)
                    .and_then(|image| image.get("url"))
                    .and_then(Value::as_str)
            })
            .or_else(|| item.get("url").and_then(Value::as_str))
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if let Some(image_url) = image_url {
            if let Some((mime_type, data)) = parse_data_url(image_url) {
                parts.push(json!({
                    "inlineData": {
                        "mimeType": mime_type,
                        "data": data,
                    }
                }));
            }
        }
    }
}

fn extract_gemini_inline_image(part: &Value) -> Option<(String, String)> {
    let inline_data = part.get("inlineData").or_else(|| part.get("inline_data"))?;
    let object = inline_data.as_object()?;
    let mime_type = object
        .get("mimeType")
        .or_else(|| object.get("mime_type"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| value.starts_with("image/"))
        .unwrap_or("image/png")
        .to_string();
    let data = object
        .get("data")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_string();
    Some((mime_type, data))
}

fn extract_openai_image_response_item(item: &Value) -> Option<(String, String)> {
    let object = item.as_object()?;
    if let Some(b64_json) = object
        .get("b64_json")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let output_format = object
            .get("output_format")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("png");
        return Some((
            mime_type_from_output_format(output_format),
            b64_json.to_string(),
        ));
    }
    let url = object
        .get("url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    parse_data_url(url)
}

fn parse_data_url(value: &str) -> Option<(String, String)> {
    let (metadata, payload) = value.trim().split_once(',')?;
    let metadata = metadata.strip_prefix("data:")?;
    let mime_type = metadata.strip_suffix(";base64")?;
    let payload = payload.trim();
    if payload.is_empty() {
        return None;
    }
    Some((mime_type.to_string(), payload.to_string()))
}

fn value_has_image_modality(value: &Value) -> bool {
    match value {
        Value::Array(items) => items.iter().any(value_has_image_modality),
        Value::String(text) => text.trim().eq_ignore_ascii_case("IMAGE"),
        _ => false,
    }
}

fn is_gemini_inline_image_part(value: &Value) -> bool {
    extract_gemini_inline_image(value).is_some()
}

fn text_non_empty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn output_format_from_mime_type(mime_type: &str) -> &'static str {
    match mime_type.trim().to_ascii_lowercase().as_str() {
        "image/jpeg" | "image/jpg" => "jpeg",
        "image/webp" => "webp",
        _ => "png",
    }
}

fn mime_type_from_output_format(output_format: &str) -> String {
    match output_format.trim().to_ascii_lowercase().as_str() {
        "jpeg" | "jpg" => "image/jpeg".to_string(),
        "webp" => "image/webp".to_string(),
        "png" => "image/png".to_string(),
        other if other.starts_with("image/") => other.to_string(),
        _ => "image/png".to_string(),
    }
}

fn mime_type_from_url(url: &str) -> &'static str {
    let lower = url.trim().to_ascii_lowercase();
    if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        "image/jpeg"
    } else if lower.ends_with(".webp") {
        "image/webp"
    } else {
        "image/png"
    }
}

fn gemini_usage_to_openai_image_usage(value: Option<&Value>) -> Option<Value> {
    let usage = value?.as_object()?;
    let input_tokens = usage
        .get("promptTokenCount")
        .or_else(|| usage.get("prompt_token_count"))
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let output_tokens = usage
        .get("candidatesTokenCount")
        .or_else(|| usage.get("candidates_token_count"))
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let total_tokens = usage
        .get("totalTokenCount")
        .or_else(|| usage.get("total_token_count"))
        .and_then(Value::as_u64)
        .unwrap_or(input_tokens.saturating_add(output_tokens));
    Some(json!({
        "input_tokens": input_tokens,
        "output_tokens": output_tokens,
        "total_tokens": total_tokens,
    }))
}

fn openai_image_usage_to_gemini_usage_metadata(value: Option<&Value>) -> Option<Value> {
    let usage = value?.as_object()?;
    let input_tokens = usage
        .get("input_tokens")
        .or_else(|| usage.get("prompt_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let output_tokens = usage
        .get("output_tokens")
        .or_else(|| usage.get("completion_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let total_tokens = usage
        .get("total_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(input_tokens.saturating_add(output_tokens));
    Some(json!({
        "promptTokenCount": input_tokens,
        "candidatesTokenCount": output_tokens,
        "totalTokenCount": total_tokens,
    }))
}

fn context_model(context: &Value) -> Option<&str> {
    context
        .get("mapped_model")
        .or_else(|| context.get("model"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use http::{Method, Request};
    use serde_json::json;

    use super::{
        build_gemini_image_request_body_from_openai_image_request,
        build_gemini_image_response_from_openai_image_response,
        build_openai_image_request_body_from_gemini_image_request,
        build_openai_image_response_from_gemini_response,
        build_openai_image_response_from_response_stream_sync_body,
        gemini_request_is_image_generation,
    };
    use crate::formats::openai::image::request::normalize_openai_image_request;

    fn request_parts(path: &str) -> http::request::Parts {
        Request::builder()
            .method(Method::POST)
            .uri(path)
            .body(())
            .expect("request should build")
            .into_parts()
            .0
    }

    #[test]
    fn converts_openai_image_generation_request_to_gemini_image_request() {
        let parts = request_parts("/v1/images/generations");
        let normalized = normalize_openai_image_request(
            &parts,
            &json!({
                "model": "gpt-image-2",
                "prompt": "Draw a red kite",
                "size": "1024x1024"
            }),
            None,
        )
        .expect("request should normalize");

        let converted = build_gemini_image_request_body_from_openai_image_request(
            &normalized,
            "gemini-2.5-flash-image",
        )
        .expect("conversion should succeed");

        assert_eq!(converted.requested_model, "gpt-image-2");
        assert_eq!(converted.body_json["model"], "gemini-2.5-flash-image");
        assert_eq!(
            converted.body_json["contents"][0]["parts"][0]["text"],
            "Draw a red kite"
        );
        assert_eq!(
            converted.body_json["generationConfig"]["responseModalities"],
            json!(["TEXT", "IMAGE"])
        );
    }

    #[test]
    fn converts_openai_image_edit_input_to_gemini_inline_data() {
        let parts = request_parts("/v1/images/edits");
        let normalized = normalize_openai_image_request(
            &parts,
            &json!({
                "prompt": "Make it brighter",
                "image": "data:image/png;base64,aGVsbG8="
            }),
            None,
        )
        .expect("request should normalize");

        let converted =
            build_gemini_image_request_body_from_openai_image_request(&normalized, "gemini-image")
                .expect("conversion should succeed");

        assert_eq!(
            converted.body_json["contents"][0]["parts"][1]["inlineData"]["mimeType"],
            "image/png"
        );
        assert_eq!(
            converted.body_json["contents"][0]["parts"][1]["inlineData"]["data"],
            "aGVsbG8="
        );
    }

    #[test]
    fn converts_gemini_image_request_to_openai_image_provider_request() {
        let body = json!({
            "generationConfig": {"responseModalities": ["TEXT", "IMAGE"]},
            "contents": [{
                "role": "user",
                "parts": [
                    {"text": "Change the background"},
                    {"inlineData": {"mimeType": "image/png", "data": "aGVsbG8="}}
                ]
            }]
        });

        assert!(gemini_request_is_image_generation(&body));
        let converted = build_openai_image_request_body_from_gemini_image_request(
            &body,
            "/v1beta/models/gemini-image:generateContent",
            "gpt-image-2",
        )
        .expect("conversion should succeed");

        assert_eq!(converted.requested_model, "gemini-image");
        assert_eq!(converted.body_json["model"], "gpt-image-2");
        assert_eq!(converted.operation.as_str(), "edit");
        assert_eq!(
            converted.body_json["image"]["image_url"],
            "data:image/png;base64,aGVsbG8="
        );
        assert!(converted.body_json.get("input").is_none());
        assert!(converted.body_json.get("tools").is_none());
        assert!(converted.body_json.get("stream").is_none());
    }

    #[test]
    fn converts_multiple_gemini_image_inputs_to_standard_openai_image_array() {
        let body = json!({
            "generationConfig": {"responseModalities": ["TEXT", "IMAGE"]},
            "contents": [{
                "role": "user",
                "parts": [
                    {"text": "Combine these references"},
                    {"inlineData": {"mimeType": "image/png", "data": "aGVsbG8="}},
                    {"fileData": {"mimeType": "image/jpeg", "fileUri": "https://example.test/reference.jpg"}}
                ]
            }]
        });

        let converted = build_openai_image_request_body_from_gemini_image_request(
            &body,
            "/v1beta/models/gemini-image:generateContent",
            "gpt-image-2",
        )
        .expect("conversion should succeed");

        assert_eq!(
            converted.body_json["image"].as_array().map(Vec::len),
            Some(2)
        );
        assert_eq!(
            converted.body_json["image"][0]["image_url"],
            "data:image/png;base64,aGVsbG8="
        );
        assert_eq!(
            converted.body_json["image"][1]["image_url"],
            "https://example.test/reference.jpg"
        );
        assert!(converted.body_json.get("images").is_none());
    }

    #[test]
    fn converts_gemini_image_response_to_openai_image_response() {
        let converted = build_openai_image_response_from_gemini_response(
            &json!({
                "modelVersion": "gemini-image",
                "usageMetadata": {
                    "promptTokenCount": 1,
                    "candidatesTokenCount": 2,
                    "totalTokenCount": 3
                },
                "candidates": [{
                    "content": {
                        "parts": [
                            {"text": "revised"},
                            {"inlineData": {"mimeType": "image/png", "data": "aGVsbG8="}}
                        ]
                    }
                }]
            }),
            None,
        )
        .expect("conversion should succeed");

        assert_eq!(converted["data"][0]["b64_json"], "aGVsbG8=");
        assert_eq!(converted["data"][0]["revised_prompt"], "revised");
        assert_eq!(converted["usage"]["total_tokens"], 3);
    }

    #[test]
    fn converts_responses_image_generation_url_to_openai_image_url() {
        let converted = build_openai_image_response_from_response_stream_sync_body(
            &json!({
                "created_at": 1776839946,
                "model": "gpt-image-2",
                "output": [{
                    "type": "image_generation_call",
                    "status": "completed",
                    "url": "https://assets.example/generated.png"
                }]
            }),
            None,
        )
        .expect("response image output should convert");

        assert_eq!(
            converted["data"][0]["url"],
            "https://assets.example/generated.png"
        );
        assert!(converted["data"][0].get("b64_json").is_none());
    }

    #[test]
    fn converts_openai_image_response_to_gemini_image_response() {
        let converted = build_gemini_image_response_from_openai_image_response(
            &json!({
                "model": "gpt-image-2",
                "data": [{
                    "b64_json": "aGVsbG8=",
                    "output_format": "png",
                    "revised_prompt": "revised"
                }],
                "usage": {
                    "input_tokens": 1,
                    "output_tokens": 2,
                    "total_tokens": 3
                }
            }),
            None,
        )
        .expect("conversion should succeed");

        assert_eq!(converted["modelVersion"], "gpt-image-2");
        assert_eq!(
            converted["candidates"][0]["content"]["parts"][1]["inlineData"]["data"],
            "aGVsbG8="
        );
        assert_eq!(converted["usageMetadata"]["totalTokenCount"], 3);
    }
}
