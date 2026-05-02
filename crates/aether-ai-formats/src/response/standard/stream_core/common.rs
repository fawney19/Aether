use serde_json::{json, Map, Value};

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
            .saturating_add(cache_creation_tokens)
            .saturating_add(cache_read_tokens),
    );
    if input_tokens == 0 && total_tokens > output_tokens {
        input_tokens = total_tokens.saturating_sub(output_tokens);
    }
    Some(CanonicalUsage {
        input_tokens,
        output_tokens,
        total_tokens,
        cache_creation_tokens,
        cache_read_tokens,
        reasoning_tokens,
        ..CanonicalUsage::default()
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
        output_tokens,
        total_tokens: input_tokens
            .saturating_add(output_tokens)
            .saturating_add(cache_creation_tokens)
            .saturating_add(cache_read_tokens),
        cache_creation_tokens,
        cache_creation_ephemeral_5m_tokens,
        cache_creation_ephemeral_1h_tokens,
        cache_read_tokens,
        reasoning_tokens,
    })
}

pub fn canonical_usage_from_gemini_usage(value: Option<&Value>) -> Option<CanonicalUsage> {
    let usage = value?.as_object()?;
    let input_tokens = usage
        .get("promptTokenCount")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output_tokens = usage
        .get("candidatesTokenCount")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let reasoning_tokens = usage
        .get("thoughtsTokenCount")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cache_read_tokens = usage
        .get("cachedContentTokenCount")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let total_tokens = usage
        .get("totalTokenCount")
        .and_then(Value::as_u64)
        .unwrap_or(
            input_tokens
                .saturating_add(output_tokens)
                .saturating_add(cache_read_tokens),
        );
    Some(CanonicalUsage {
        input_tokens,
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
    json!({
        "id": id,
        "object": "chat.completion.chunk",
        "model": model,
        "choices": [],
        "usage": usage,
    })
}
