use serde_json::{json, Map, Value};

use crate::formats::shared::model_directives::model_directive_display_model_from_report_context;

pub use aether_ai_formats::protocol::stream::{
    CanonicalContentPart, CanonicalStreamEvent, CanonicalStreamFrame, CanonicalUsage,
};

pub fn decode_json_data_line(line: &[u8]) -> Option<Value> {
    let text = std::str::from_utf8(line).ok()?;
    let trimmed = text.trim_matches('\r').trim();
    if trimmed.is_empty() || trimmed.starts_with(':') || trimmed.starts_with("event:") {
        return None;
    }
    let data_line = trimmed.strip_prefix("data:")?.trim();
    if data_line.is_empty() || data_line == "[DONE]" {
        return None;
    }
    serde_json::from_str(data_line).ok()
}

pub fn resolve_identity(
    response_id: Option<&str>,
    model: Option<&str>,
    report_context: &Value,
    default_id: &str,
) -> (String, String) {
    let id = response_id
        .filter(|value| !value.is_empty())
        .unwrap_or(default_id)
        .to_string();
    if let Some(display_model) = model_directive_display_model_from_report_context(report_context) {
        return (id, display_model);
    }
    let model = model
        .filter(|value| !value.is_empty())
        .or_else(|| report_context.get("mapped_model").and_then(Value::as_str))
        .or_else(|| report_context.get("model").and_then(Value::as_str))
        .unwrap_or("unknown")
        .to_string();
    (id, model)
}

pub fn canonical_usage_from_openai_usage(value: Option<&Value>) -> Option<CanonicalUsage> {
    let usage = value?.as_object()?;
    let mut input_tokens = usage
        .get("input_tokens")
        .or_else(|| usage.get("prompt_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output_tokens = usage
        .get("output_tokens")
        .or_else(|| usage.get("completion_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cache_creation_tokens = usage
        .get("cache_creation_input_tokens")
        .and_then(Value::as_u64)
        .or_else(|| {
            usage
                .get("input_tokens_details")
                .or_else(|| usage.get("prompt_tokens_details"))
                .and_then(Value::as_object)
                .and_then(|details| details.get("cached_creation_tokens"))
                .and_then(Value::as_u64)
        })
        .unwrap_or(0);
    let cache_read_tokens = usage
        .get("cache_read_input_tokens")
        .and_then(Value::as_u64)
        .or_else(|| {
            usage
                .get("input_tokens_details")
                .or_else(|| usage.get("prompt_tokens_details"))
                .and_then(Value::as_object)
                .and_then(|details| details.get("cached_tokens"))
                .and_then(Value::as_u64)
        })
        .unwrap_or(0);
    let reasoning_tokens = usage
        .get("reasoning_tokens")
        .and_then(Value::as_u64)
        .or_else(|| {
            usage
                .get("output_tokens_details")
                .or_else(|| usage.get("completion_tokens_details"))
                .and_then(Value::as_object)
                .and_then(|details| details.get("reasoning_tokens"))
                .and_then(Value::as_u64)
        })
        .unwrap_or(0);
    let total_tokens = usage.get("total_tokens").and_then(Value::as_u64).unwrap_or(
        input_tokens
            .saturating_add(output_tokens)
            .saturating_add(reasoning_tokens),
    );
    if input_tokens == 0 && total_tokens > output_tokens {
        input_tokens = total_tokens.saturating_sub(output_tokens);
    }
    Some(CanonicalUsage {
        input_tokens,
        input_tokens_include_cache: cache_read_tokens > 0 || cache_creation_tokens > 0,
        output_tokens,
        total_tokens,
        cache_creation_tokens,
        cache_read_tokens,
        reasoning_tokens,
        ..CanonicalUsage::default()
    })
}

pub fn openai_stream_payload_is_terminal_error(payload: &Value) -> bool {
    let event_type = payload
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if payload.get("error").is_some() {
        return true;
    }
    if matches!(
        event_type,
        "error" | "response.failed" | "response.incomplete"
    ) {
        return true;
    }

    payload
        .get("response")
        .and_then(Value::as_object)
        .and_then(|response| response.get("status"))
        .and_then(Value::as_str)
        .is_some_and(|status| matches!(status, "failed" | "incomplete"))
}

pub fn openai_stream_terminal_error_body(payload: &Value) -> Option<Value> {
    if !openai_stream_payload_is_terminal_error(payload) {
        return None;
    }

    let event_type = payload
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let response = payload.get("response").and_then(Value::as_object);
    let status = response
        .and_then(|response| response.get("status"))
        .and_then(Value::as_str);
    let raw_error = response
        .and_then(|response| response.get("error"))
        .or_else(|| payload.get("error"));

    let mut error = raw_error
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    let message = error
        .get("message")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .or_else(|| raw_error.and_then(Value::as_str).map(ToOwned::to_owned))
        .or_else(|| {
            payload
                .get("message")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        })
        .or_else(|| {
            response
                .and_then(|response| response.get("incomplete_details"))
                .and_then(|details| details.get("reason"))
                .and_then(Value::as_str)
                .map(|reason| format!("Response incomplete: {reason}"))
        })
        .or_else(|| status.map(|status| format!("Response ended with status {status}")))
        .unwrap_or_else(|| "Upstream stream ended with an error".to_string());

    error
        .entry("message".to_string())
        .or_insert_with(|| Value::String(message));
    error.entry("type".to_string()).or_insert_with(|| {
        if event_type == "response.incomplete" || status == Some("incomplete") {
            Value::String("incomplete".to_string())
        } else {
            Value::String("server_error".to_string())
        }
    });

    if !error.contains_key("code") {
        if let Some(reason) = response
            .and_then(|response| response.get("incomplete_details"))
            .and_then(|details| details.get("reason"))
            .and_then(Value::as_str)
        {
            error.insert("code".to_string(), Value::String(reason.to_string()));
        }
    }

    Some(json!({ "error": Value::Object(error) }))
}

pub fn openai_stream_terminal_error_message(payload: &Value) -> Option<String> {
    openai_stream_terminal_error_body(payload)
        .and_then(|body| body.get("error").cloned())
        .and_then(|error| {
            error
                .get("message")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        })
}

pub fn canonical_usage_from_claude_usage(value: Option<&Value>) -> Option<CanonicalUsage> {
    let usage = value?.as_object()?;
    let input_tokens = usage
        .get("input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output_tokens = usage
        .get("output_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cache_creation_ephemeral_5m_tokens = usage
        .get("cache_creation")
        .and_then(Value::as_object)
        .and_then(|value| value.get("ephemeral_5m_input_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cache_creation_ephemeral_1h_tokens = usage
        .get("cache_creation")
        .and_then(Value::as_object)
        .and_then(|value| value.get("ephemeral_1h_input_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cache_creation_tokens = usage
        .get("cache_creation_input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(
            cache_creation_ephemeral_5m_tokens.saturating_add(cache_creation_ephemeral_1h_tokens),
        );
    let cache_read_tokens = usage
        .get("cache_read_input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let reasoning_tokens = usage
        .get("reasoning_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    Some(CanonicalUsage {
        input_tokens,
        input_tokens_include_cache: false,
        output_tokens,
        total_tokens: input_tokens
            .saturating_add(output_tokens)
            .saturating_add(reasoning_tokens),
        cache_creation_tokens,
        cache_creation_ephemeral_5m_tokens,
        cache_creation_ephemeral_1h_tokens,
        cache_read_tokens,
        reasoning_tokens,
    })
}

pub fn content_part_from_openai_image_generation_item(
    item: &Value,
) -> Option<CanonicalContentPart> {
    let item = item.as_object()?;
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
    let image = if let Some(result) = result {
        if result.starts_with("data:image/")
            || result.starts_with("http://")
            || result.starts_with("https://")
        {
            result.to_string()
        } else {
            let mime_type = item
                .get("mime_type")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .or_else(|| {
                    item.get("output_format")
                        .and_then(Value::as_str)
                        .map(openai_image_output_format_to_mime_type)
                })
                .unwrap_or_else(|| "image/png".to_string());
            format!("data:{mime_type};base64,{result}")
        }
    } else {
        url?.to_string()
    };
    Some(CanonicalContentPart::ImageUrl(image))
}

fn openai_image_output_format_to_mime_type(output_format: &str) -> String {
    match output_format.trim().to_ascii_lowercase().as_str() {
        "jpeg" | "jpg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        _ => "image/png",
    }
    .to_string()
}

pub fn canonical_usage_from_gemini_usage(value: Option<&Value>) -> Option<CanonicalUsage> {
    let usage = value?.as_object()?;
    let input_tokens = usage
        .get("promptTokenCount")
        .or_else(|| usage.get("prompt_token_count"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output_tokens = usage
        .get("candidatesTokenCount")
        .or_else(|| usage.get("candidates_token_count"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let reasoning_tokens = usage
        .get("thoughtsTokenCount")
        .or_else(|| usage.get("thoughts_token_count"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cache_read_tokens = usage
        .get("cachedContentTokenCount")
        .or_else(|| usage.get("cached_content_token_count"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let total_tokens = usage
        .get("totalTokenCount")
        .or_else(|| usage.get("total_token_count"))
        .and_then(Value::as_u64)
        .unwrap_or(
            input_tokens
                .saturating_add(output_tokens)
                .saturating_add(reasoning_tokens),
        );
    Some(CanonicalUsage {
        input_tokens,
        input_tokens_include_cache: cache_read_tokens > 0,
        output_tokens: output_tokens.saturating_add(reasoning_tokens),
        total_tokens,
        cache_read_tokens,
        reasoning_tokens,
        ..CanonicalUsage::default()
    })
}

pub fn normalize_openai_finish_reason(value: Option<&str>) -> Option<String> {
    match value {
        Some("function_call") => Some("tool_calls".to_string()),
        Some(other) if !other.trim().is_empty() => Some(other.to_string()),
        _ => None,
    }
}

pub fn map_openai_finish_reason_to_claude(value: Option<&str>) -> &'static str {
    match value {
        Some("length") => "max_tokens",
        Some("tool_calls") | Some("function_call") => "tool_use",
        Some("content_filter") => "content_filtered",
        _ => "end_turn",
    }
}

pub fn map_openai_finish_reason_to_gemini(value: Option<&str>) -> &'static str {
    match value {
        Some("length") => "MAX_TOKENS",
        Some("content_filter") => "SAFETY",
        _ => "STOP",
    }
}

pub fn parse_json_arguments_value(arguments: &str) -> Option<Value> {
    let trimmed = arguments.trim();
    if trimmed.is_empty() {
        return Some(Value::Object(Map::new()));
    }
    serde_json::from_str(trimmed).ok()
}

pub fn build_openai_chat_chunk(
    id: &str,
    model: &str,
    text: String,
    tool_calls: Option<Vec<Value>>,
    finish_reason: Option<&str>,
) -> Value {
    let mut delta = Map::new();
    delta.insert("role".to_string(), Value::String("assistant".to_string()));
    if !text.is_empty() {
        delta.insert("content".to_string(), Value::String(text));
    } else if tool_calls.is_none() {
        delta.insert("content".to_string(), Value::String(String::new()));
    }
    if let Some(tool_calls) = tool_calls {
        delta.insert("tool_calls".to_string(), Value::Array(tool_calls));
    }

    json!({
        "id": id,
        "object": "chat.completion.chunk",
        "model": model,
        "choices": [{
            "index": 0,
            "delta": Value::Object(delta),
            "finish_reason": finish_reason,
        }]
    })
}

pub fn build_openai_chat_role_chunk(id: &str, model: &str) -> Value {
    json!({
        "id": id,
        "object": "chat.completion.chunk",
        "model": model,
        "choices": [{
            "index": 0,
            "delta": {
                "role": "assistant"
            },
            "finish_reason": Value::Null
        }]
    })
}

pub fn build_openai_chat_finish_chunk(id: &str, model: &str, finish_reason: Option<&str>) -> Value {
    json!({
        "id": id,
        "object": "chat.completion.chunk",
        "model": model,
        "choices": [{
            "index": 0,
            "delta": {},
            "finish_reason": finish_reason,
        }]
    })
}

pub fn build_openai_chat_usage_chunk(
    id: &str,
    model: &str,
    prompt_tokens: u64,
    completion_tokens: u64,
    total_tokens: u64,
    reasoning_tokens: u64,
) -> Value {
    build_openai_chat_usage_chunk_with_cache(
        id,
        model,
        prompt_tokens,
        completion_tokens,
        total_tokens,
        reasoning_tokens,
        0,
        0,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn build_openai_chat_usage_chunk_with_cache(
    id: &str,
    model: &str,
    prompt_tokens: u64,
    completion_tokens: u64,
    total_tokens: u64,
    reasoning_tokens: u64,
    cache_creation_tokens: u64,
    cache_read_tokens: u64,
) -> Value {
    let usage = openai_chat_usage_payload(
        prompt_tokens,
        completion_tokens,
        total_tokens,
        reasoning_tokens,
        cache_creation_tokens,
        cache_read_tokens,
    );
    json!({
        "id": id,
        "object": "chat.completion.chunk",
        "model": model,
        "choices": [],
        "usage": usage,
    })
}

pub fn build_openai_chat_usage_chunk_from_usage(
    id: &str,
    model: &str,
    usage: &CanonicalUsage,
) -> Value {
    let input_tokens = inclusive_input_tokens_from_usage(usage);
    build_openai_chat_usage_chunk_with_cache(
        id,
        model,
        input_tokens,
        usage.output_tokens,
        inclusive_total_tokens_from_usage(usage, input_tokens),
        usage.reasoning_tokens,
        cache_creation_tokens_for_usage(usage),
        usage.cache_read_tokens,
    )
}

pub fn openai_responses_usage_from_usage(usage: &CanonicalUsage) -> Value {
    let mut output = Map::new();
    let input_tokens = inclusive_input_tokens_from_usage(usage);
    output.insert("input_tokens".to_string(), Value::from(input_tokens));
    output.insert(
        "output_tokens".to_string(),
        Value::from(usage.output_tokens),
    );
    output.insert(
        "total_tokens".to_string(),
        Value::from(inclusive_total_tokens_from_usage(usage, input_tokens)),
    );
    if usage.reasoning_tokens > 0 {
        output.insert(
            "output_tokens_details".to_string(),
            json!({ "reasoning_tokens": usage.reasoning_tokens }),
        );
    }
    insert_openai_token_details(
        &mut output,
        "input_tokens_details",
        cache_creation_tokens_for_usage(usage),
        usage.cache_read_tokens,
    );
    Value::Object(output)
}

pub fn claude_usage_from_usage(usage: &CanonicalUsage) -> Value {
    let mut output = Map::new();
    output.insert(
        "input_tokens".to_string(),
        Value::from(claude_input_tokens_from_usage(usage)),
    );
    output.insert(
        "output_tokens".to_string(),
        Value::from(usage.output_tokens),
    );
    if usage.cache_read_tokens > 0 {
        output.insert(
            "cache_read_input_tokens".to_string(),
            Value::from(usage.cache_read_tokens),
        );
    }
    let cache_creation_tokens = cache_creation_tokens_for_usage(usage);
    if cache_creation_tokens > 0 {
        output.insert(
            "cache_creation_input_tokens".to_string(),
            Value::from(cache_creation_tokens),
        );
    }
    if usage.cache_creation_ephemeral_5m_tokens > 0 || usage.cache_creation_ephemeral_1h_tokens > 0
    {
        output.insert(
            "cache_creation".to_string(),
            json!({
                "ephemeral_5m_input_tokens": usage.cache_creation_ephemeral_5m_tokens,
                "ephemeral_1h_input_tokens": usage.cache_creation_ephemeral_1h_tokens,
            }),
        );
    }
    Value::Object(output)
}

pub fn gemini_usage_metadata_from_usage(usage: &CanonicalUsage) -> Value {
    let visible_output_tokens = usage.output_tokens.saturating_sub(usage.reasoning_tokens);
    let input_tokens = inclusive_input_tokens_from_usage(usage);
    let mut output = Map::new();
    output.insert("promptTokenCount".to_string(), Value::from(input_tokens));
    output.insert(
        "candidatesTokenCount".to_string(),
        Value::from(visible_output_tokens),
    );
    output.insert(
        "totalTokenCount".to_string(),
        Value::from(inclusive_total_tokens_from_usage(usage, input_tokens)),
    );
    if usage.reasoning_tokens > 0 {
        output.insert(
            "thoughtsTokenCount".to_string(),
            Value::from(usage.reasoning_tokens),
        );
    }
    if usage.cache_read_tokens > 0 {
        output.insert(
            "cachedContentTokenCount".to_string(),
            Value::from(usage.cache_read_tokens),
        );
    }
    Value::Object(output)
}

fn openai_chat_usage_payload(
    prompt_tokens: u64,
    completion_tokens: u64,
    total_tokens: u64,
    reasoning_tokens: u64,
    cache_creation_tokens: u64,
    cache_read_tokens: u64,
) -> Map<String, Value> {
    let mut usage = Map::new();
    usage.insert("prompt_tokens".to_string(), Value::from(prompt_tokens));
    usage.insert(
        "completion_tokens".to_string(),
        Value::from(completion_tokens),
    );
    usage.insert("total_tokens".to_string(), Value::from(total_tokens));
    if reasoning_tokens > 0 {
        usage.insert(
            "completion_tokens_details".to_string(),
            json!({ "reasoning_tokens": reasoning_tokens }),
        );
    }
    insert_openai_token_details(
        &mut usage,
        "prompt_tokens_details",
        cache_creation_tokens,
        cache_read_tokens,
    );
    usage
}

fn insert_openai_token_details(
    output: &mut Map<String, Value>,
    details_key: &str,
    cache_creation_tokens: u64,
    cache_read_tokens: u64,
) {
    if cache_creation_tokens == 0 && cache_read_tokens == 0 {
        return;
    }
    let mut details = Map::new();
    if cache_read_tokens > 0 {
        details.insert("cached_tokens".to_string(), Value::from(cache_read_tokens));
    }
    if cache_creation_tokens > 0 {
        details.insert(
            "cached_creation_tokens".to_string(),
            Value::from(cache_creation_tokens),
        );
    }
    output.insert(details_key.to_string(), Value::Object(details));
}

fn cache_creation_tokens_for_usage(usage: &CanonicalUsage) -> u64 {
    if usage.cache_creation_tokens > 0 {
        usage.cache_creation_tokens
    } else {
        usage
            .cache_creation_ephemeral_5m_tokens
            .saturating_add(usage.cache_creation_ephemeral_1h_tokens)
    }
}

fn cache_input_tokens_for_usage(usage: &CanonicalUsage) -> u64 {
    usage
        .cache_read_tokens
        .saturating_add(cache_creation_tokens_for_usage(usage))
}

fn claude_input_tokens_from_usage(usage: &CanonicalUsage) -> u64 {
    if usage.input_tokens_include_cache {
        usage
            .input_tokens
            .saturating_sub(cache_input_tokens_for_usage(usage))
    } else {
        usage.input_tokens
    }
}

fn inclusive_input_tokens_from_usage(usage: &CanonicalUsage) -> u64 {
    if usage.input_tokens_include_cache {
        usage.input_tokens
    } else {
        usage
            .input_tokens
            .saturating_add(cache_input_tokens_for_usage(usage))
    }
}

fn inclusive_total_tokens_from_usage(usage: &CanonicalUsage, input_tokens: u64) -> u64 {
    if usage.total_tokens > 0
        && (usage.input_tokens_include_cache || cache_input_tokens_for_usage(usage) == 0)
    {
        usage.total_tokens
    } else {
        input_tokens.saturating_add(usage.output_tokens)
    }
}
