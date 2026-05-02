use serde_json::{Map, Value};

pub fn parse_openai_stop_sequences(stop: Option<&Value>) -> Option<Vec<Value>> {
    match stop {
        Some(Value::String(value)) if !value.trim().is_empty() => {
            Some(vec![Value::String(value.clone())])
        }
        Some(Value::Array(values)) => Some(
            values
                .iter()
                .filter_map(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| Value::String(value.to_string()))
                .collect::<Vec<_>>(),
        )
        .filter(|values| !values.is_empty()),
        _ => None,
    }
}

pub fn resolve_openai_chat_max_tokens(request: &Map<String, Value>) -> u64 {
    request
        .get("max_completion_tokens")
        .and_then(value_as_u64)
        .or_else(|| request.get("max_tokens").and_then(value_as_u64))
        .unwrap_or(4096)
}

pub fn value_as_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|value| u64::try_from(value).ok()))
}

pub fn copy_request_number_field(
    request: &Map<String, Value>,
    target: &mut Map<String, Value>,
    key: &str,
) {
    copy_request_number_field_as(request, target, key, key);
}

pub fn copy_request_number_field_as(
    request: &Map<String, Value>,
    target: &mut Map<String, Value>,
    source_key: &str,
    target_key: &str,
) {
    if let Some(value) = request.get(source_key).cloned() {
        if value.is_number() {
            target.insert(target_key.to_string(), value);
        }
    }
}

pub fn map_openai_reasoning_effort_to_claude_output(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "low" => Some("low"),
        "medium" => Some("medium"),
        "high" => Some("high"),
        "xhigh" => Some("max"),
        _ => None,
    }
}

pub fn map_openai_reasoning_effort_to_thinking_budget(value: &str) -> Option<u64> {
    match value.trim().to_ascii_lowercase().as_str() {
        "low" => Some(1280),
        "medium" => Some(2048),
        "high" => Some(4096),
        "xhigh" => Some(8192),
        _ => None,
    }
}

pub fn map_openai_reasoning_effort_to_gemini_budget(value: &str) -> Option<u64> {
    map_openai_reasoning_effort_to_thinking_budget(value)
}

pub fn map_thinking_budget_to_openai_reasoning_effort(value: u64) -> &'static str {
    match value {
        0..=1664 => "low",
        1665..=3072 => "medium",
        3073..=6144 => "high",
        _ => "xhigh",
    }
}

pub fn extract_openai_reasoning_effort(request: &Map<String, Value>) -> Option<String> {
    request
        .get("reasoning_effort")
        .and_then(Value::as_str)
        .or_else(|| {
            request
                .get("reasoning")
                .and_then(Value::as_object)
                .and_then(|reasoning| reasoning.get("effort"))
                .and_then(Value::as_str)
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase())
}
