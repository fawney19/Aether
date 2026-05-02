//! Pairwise request conversion helpers.
//!
//! These helpers keep the call sites readable while delegating wire-format
//! parsing and emitting to `formats::<format>::request` through the registry's
//! canonical IR path.

use serde_json::Value;

use crate::protocol::{context::FormatContext, registry};

pub fn convert_openai_chat_request_to_claude_request(
    body_json: &Value,
    mapped_model: &str,
    upstream_is_stream: bool,
) -> Option<Value> {
    registry::convert_request(
        "openai:chat",
        "claude:messages",
        body_json,
        &request_context(mapped_model, upstream_is_stream),
    )
    .ok()
}

pub fn convert_openai_chat_request_to_gemini_request(
    body_json: &Value,
    mapped_model: &str,
    upstream_is_stream: bool,
) -> Option<Value> {
    registry::convert_request(
        "openai:chat",
        "gemini:generate_content",
        body_json,
        &request_context(mapped_model, upstream_is_stream),
    )
    .ok()
}

pub fn convert_openai_chat_request_to_openai_responses_request(
    body_json: &Value,
    mapped_model: &str,
    upstream_is_stream: bool,
    compact: bool,
) -> Option<Value> {
    let target_format = if compact {
        "openai:responses:compact"
    } else {
        "openai:responses"
    };
    registry::convert_request(
        "openai:chat",
        target_format,
        body_json,
        &request_context(mapped_model, upstream_is_stream),
    )
    .ok()
}

pub fn normalize_openai_responses_request_to_openai_chat_request(
    body_json: &Value,
) -> Option<Value> {
    registry::convert_request(
        "openai:responses",
        "openai:chat",
        body_json,
        &FormatContext::default(),
    )
    .ok()
}

pub fn normalize_claude_request_to_openai_chat_request(body_json: &Value) -> Option<Value> {
    registry::convert_request(
        "claude:messages",
        "openai:chat",
        body_json,
        &FormatContext::default(),
    )
    .ok()
}

pub fn normalize_gemini_request_to_openai_chat_request(
    body_json: &Value,
    request_path: &str,
) -> Option<Value> {
    registry::convert_request(
        "gemini:generate_content",
        "openai:chat",
        body_json,
        &FormatContext::default().with_request_path(request_path),
    )
    .ok()
}

pub fn extract_openai_text_content(content: Option<&Value>) -> Option<String> {
    match content {
        None | Some(Value::Null) => Some(String::new()),
        Some(Value::String(text)) => Some(text.clone()),
        Some(Value::Array(parts)) => {
            let mut collected = Vec::new();
            for part in parts {
                let part_object = part.as_object()?;
                let part_type = part_object
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if matches!(part_type, "text" | "input_text") {
                    if let Some(text) = part_object.get("text").and_then(Value::as_str) {
                        if !text.trim().is_empty() {
                            collected.push(text.to_string());
                        }
                    }
                }
            }
            Some(collected.join("\n"))
        }
        _ => None,
    }
}

pub fn parse_openai_tool_result_content(content: Option<&Value>) -> Value {
    match content {
        Some(Value::String(raw)) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                Value::String(String::new())
            } else {
                serde_json::from_str::<Value>(trimmed)
                    .unwrap_or_else(|_| Value::String(raw.clone()))
            }
        }
        Some(Value::Array(parts)) => {
            let texts = parts
                .iter()
                .filter_map(|part| {
                    part.as_object()
                        .and_then(|object| object.get("text"))
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned)
                })
                .collect::<Vec<_>>();
            if texts.is_empty() {
                Value::Array(parts.clone())
            } else {
                Value::String(texts.join("\n"))
            }
        }
        Some(value) => value.clone(),
        None => Value::String(String::new()),
    }
}

fn request_context(mapped_model: &str, upstream_is_stream: bool) -> FormatContext {
    FormatContext::default()
        .with_mapped_model(mapped_model)
        .with_upstream_stream(upstream_is_stream)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        convert_openai_chat_request_to_claude_request,
        convert_openai_chat_request_to_openai_responses_request,
        normalize_claude_request_to_openai_chat_request,
    };

    #[test]
    fn pairwise_request_helper_routes_through_registry() {
        let body = json!({
            "model": "gpt-source",
            "messages": [{"role": "user", "content": "hello"}],
        });

        let converted = convert_openai_chat_request_to_openai_responses_request(
            &body,
            "gpt-target",
            true,
            false,
        )
        .expect("responses request");

        assert_eq!(converted["model"], "gpt-target");
        assert_eq!(converted["stream"], true);
        assert_eq!(converted["input"][0]["type"], "message");
    }

    #[test]
    fn pairwise_request_helper_keeps_claude_shape() {
        let body = json!({
            "model": "gpt-source",
            "messages": [{"role": "user", "content": "hello"}],
        });

        let converted =
            convert_openai_chat_request_to_claude_request(&body, "claude-target", false)
                .expect("claude request");

        assert_eq!(converted["model"], "claude-target");
        assert_eq!(converted["messages"][0]["role"], "user");
    }

    #[test]
    fn request_normalizer_uses_format_adapter() {
        let body = json!({
            "model": "claude-sonnet",
            "messages": [{"role": "user", "content": [{"type": "text", "text": "hello"}]}],
            "max_tokens": 128,
        });

        let converted =
            normalize_claude_request_to_openai_chat_request(&body).expect("openai chat request");

        assert_eq!(converted["model"], "claude-sonnet");
        assert_eq!(converted["messages"][0]["role"], "user");
        assert_eq!(converted["messages"][0]["content"], "hello");
    }
}
