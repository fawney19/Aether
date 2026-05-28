use std::sync::OnceLock;

use axum::body::Bytes;
use axum::http;
use regex::Regex;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::control::GatewayControlDecision;
use crate::headers::decoded_request_body_bytes;
use crate::GatewayError;

#[derive(Clone, Debug)]
pub(crate) struct RiskControlInspectionInput {
    pub(crate) endpoint: String,
    pub(crate) model: Option<String>,
    pub(crate) text: String,
    pub(crate) keyword_text: String,
    pub(crate) normalized_hash_input: String,
    pub(crate) input_hash: String,
    pub(crate) excerpt: String,
}

pub(crate) fn risk_control_requires_buffered_body(
    request_context: &crate::control::GatewayPublicRequestContext,
) -> bool {
    let Some(decision) = request_context.control_decision.as_ref() else {
        return false;
    };
    if decision.route_class.as_deref() != Some("ai_public") {
        return false;
    }
    if request_context.request_method != http::Method::POST {
        return false;
    }
    let content_type = request_context
        .request_content_type
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    content_type.is_empty() || content_type.contains("json")
}

pub(crate) fn extract_inspection_input(
    decision: Option<&GatewayControlDecision>,
    headers: &http::HeaderMap,
    body: &Bytes,
    max_text_chars: usize,
    excerpt_chars: usize,
) -> Result<Option<RiskControlInspectionInput>, GatewayError> {
    if body.is_empty() {
        return Ok(None);
    }
    let decoded = match decoded_request_body_bytes(headers, body.as_ref()) {
        Ok(value) => value,
        Err(_) => return Ok(None),
    };
    let value = match serde_json::from_slice::<Value>(decoded.as_ref()) {
        Ok(value) => value,
        Err(_) => return Ok(None),
    };
    let mut parts = extract_text_parts_for_signature(
        decision.and_then(|decision| decision.auth_endpoint_signature.as_deref()),
        &value,
    );
    let text = normalize_extracted_text(&parts.join("\n"));
    parts.clear();
    let keyword_text = text.trim().to_string();
    if keyword_text.is_empty() {
        return Ok(None);
    }
    let text = truncate_chars(keyword_text.as_str(), max_text_chars);
    let normalized_hash_input = normalize_for_hash(&keyword_text);
    let input_hash = sha256_hex(normalized_hash_input.as_bytes());
    let excerpt = truncate_chars(keyword_text.as_str(), excerpt_chars);
    Ok(Some(RiskControlInspectionInput {
        endpoint: decision
            .and_then(|decision| decision.auth_endpoint_signature.clone())
            .unwrap_or_else(|| "unknown".to_string()),
        model: value
            .get("model")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        text,
        keyword_text,
        normalized_hash_input,
        input_hash,
        excerpt,
    }))
}

fn extract_text_parts_for_signature(signature: Option<&str>, value: &Value) -> Vec<String> {
    let signature = signature
        .map(str::trim)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let mut parts = Vec::new();
    match signature.as_str() {
        "openai:chat" => collect_role_messages(value.get("messages"), "user", &mut parts),
        "claude:messages" => collect_anthropic_user_messages(value.get("messages"), &mut parts),
        "openai:responses" | "openai:responses:compact" => {
            collect_responses_input(value.get("input"), &mut parts);
        }
        "gemini:generate_content" => collect_gemini_contents(value.get("contents"), &mut parts),
        "openai:image" => {
            add_moderation_text(&mut parts, value.get("prompt").and_then(Value::as_str))
        }
        _ => {
            collect_responses_input(value.get("input"), &mut parts);
            collect_role_messages(value.get("messages"), "user", &mut parts);
            collect_gemini_contents(value.get("contents"), &mut parts);
            add_moderation_text(&mut parts, value.get("prompt").and_then(Value::as_str));
        }
    }
    parts
}

fn collect_role_messages(messages: Option<&Value>, role: &str, parts: &mut Vec<String>) {
    let Some(items) = messages.and_then(Value::as_array) else {
        return;
    };
    for item in items {
        if item
            .get("role")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default()
            .eq_ignore_ascii_case(role)
        {
            collect_content_value(item.get("content"), parts);
        }
    }
}

fn collect_anthropic_user_messages(messages: Option<&Value>, parts: &mut Vec<String>) {
    let Some(items) = messages.and_then(Value::as_array) else {
        return;
    };
    for item in items {
        if item
            .get("role")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default()
            .eq_ignore_ascii_case("user")
        {
            collect_anthropic_content_value(item.get("content"), parts);
        }
    }
}

fn collect_responses_input(input: Option<&Value>, parts: &mut Vec<String>) {
    match input {
        Some(Value::String(value)) => add_moderation_text(parts, Some(value)),
        Some(Value::Array(items)) => {
            for item in items
                .iter()
                .filter(|item| is_responses_user_text_item(item))
            {
                collect_content_value(item.get("content"), parts);
                if item.get("type").and_then(Value::as_str) == Some("input_text")
                    || item.get("text").is_some()
                {
                    collect_content_value(Some(item), parts);
                }
            }
        }
        Some(Value::Object(_)) => {
            if let Some(input) = input.filter(|item| is_responses_user_text_item(item)) {
                collect_content_value(input.get("content"), parts);
                if input.get("type").and_then(Value::as_str) == Some("input_text")
                    || input.get("text").is_some()
                {
                    collect_content_value(Some(input), parts);
                }
            }
        }
        _ => {}
    }
}

fn is_responses_user_text_item(item: &Value) -> bool {
    let role = item
        .get("role")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if !role.is_empty() && !role.eq_ignore_ascii_case("user") {
        return false;
    }
    response_item_has_text(item)
}

fn response_item_has_text(item: &Value) -> bool {
    let mut parts = Vec::new();
    collect_content_value(item.get("content"), &mut parts);
    if item.get("type").and_then(Value::as_str) == Some("input_text") || item.get("text").is_some()
    {
        collect_content_value(Some(item), &mut parts);
    }
    !normalize_extracted_text(&parts.join("\n")).is_empty()
}

fn collect_gemini_contents(contents: Option<&Value>, parts: &mut Vec<String>) {
    let Some(items) = contents.and_then(Value::as_array) else {
        return;
    };
    for item in items {
        let role = item
            .get("role")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default();
        if !role.is_empty() && !role.eq_ignore_ascii_case("user") {
            continue;
        }
        if let Some(gemini_parts) = item.get("parts").and_then(Value::as_array) {
            for part in gemini_parts {
                add_moderation_text(parts, part.get("text").and_then(Value::as_str));
            }
        }
    }
}

fn collect_content_value(value: Option<&Value>, parts: &mut Vec<String>) {
    match value {
        Some(Value::String(value)) => add_moderation_text(parts, Some(value)),
        Some(Value::Array(items)) => {
            for item in items {
                collect_content_value(Some(item), parts);
            }
        }
        Some(Value::Object(map)) => {
            let item_type = map
                .get("type")
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or_default()
                .to_ascii_lowercase();
            if matches!(item_type.as_str(), "" | "text" | "input_text" | "message") {
                add_moderation_text(parts, map.get("text").and_then(Value::as_str));
                collect_content_value(map.get("content"), parts);
            }
        }
        _ => {}
    }
}

fn collect_anthropic_content_value(value: Option<&Value>, parts: &mut Vec<String>) {
    match value {
        Some(Value::String(value)) => add_moderation_text(parts, Some(value)),
        Some(Value::Array(items)) => {
            for item in items {
                collect_anthropic_content_value(Some(item), parts);
            }
        }
        Some(Value::Object(map)) => {
            let item_type = map
                .get("type")
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or_default()
                .to_ascii_lowercase();
            if matches!(item_type.as_str(), "" | "text" | "input_text" | "message") {
                add_moderation_text(parts, map.get("text").and_then(Value::as_str));
                collect_anthropic_content_value(map.get("content"), parts);
            }
        }
        _ => {}
    }
}

fn add_moderation_text(parts: &mut Vec<String>, value: Option<&str>) {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return;
    };
    let stripped = strip_system_reminder_tags(value);
    let stripped = stripped.trim();
    if stripped.is_empty() {
        return;
    }
    parts.push(stripped.to_string());
}

fn strip_system_reminder_tags(value: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(?is)<\s*/?\s*system-reminder\b[^>]*>").expect("valid system-reminder regex")
    });
    re.replace_all(value, " ").into_owned()
}

fn normalize_extracted_text(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(crate) fn truncate_chars(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

pub(crate) fn normalize_for_hash(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn sha256_hex(value: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value);
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;
    use serde_json::json;

    #[test]
    fn extracts_all_openai_user_messages_and_hashes_normalized_input() {
        let body = Bytes::from(
            serde_json::to_vec(&json!({
                "model": "gpt-5",
                "messages": [
                    {"role": "user", "content": " First "},
                    {"role": "assistant", "content": [{"type": "text", "text": "Second"}]},
                    {"role": "user", "content": " Hello   World "}
                ]
            }))
            .expect("json should serialize"),
        );
        let decision = GatewayControlDecision::synthetic(
            "/v1/chat/completions",
            Some("ai_public".to_string()),
            Some("openai".to_string()),
            Some("chat_completions".to_string()),
            Some("openai:chat".to_string()),
        );

        let input = extract_inspection_input(Some(&decision), &HeaderMap::new(), &body, 1024, 128)
            .expect("input should parse")
            .expect("input should exist");

        assert_eq!(input.model.as_deref(), Some("gpt-5"));
        assert!(input.text.contains("First"));
        assert!(input.text.contains("Hello"));
        assert!(!input.text.contains("Second"));
        assert_eq!(input.normalized_hash_input, "first hello world");
    }

    #[test]
    fn extracts_all_responses_user_input_only() {
        let body = Bytes::from(
            serde_json::to_vec(&json!({
                "model": "gpt-5",
                "input": [
                    {"role": "user", "content": [{"type": "input_text", "text": "first"}]},
                    {"role": "assistant", "content": [{"type": "output_text", "text": "ignore"}]},
                    {"role": "user", "content": [{"type": "input_text", "text": "last user"}]}
                ]
            }))
            .expect("json should serialize"),
        );
        let decision = GatewayControlDecision::synthetic(
            "/v1/responses",
            Some("ai_public".to_string()),
            Some("openai".to_string()),
            Some("responses".to_string()),
            Some("openai:responses".to_string()),
        );

        let input = extract_inspection_input(Some(&decision), &HeaderMap::new(), &body, 1024, 128)
            .expect("input should parse")
            .expect("input should exist");

        assert_eq!(input.normalized_hash_input, "first last user");
    }

    #[test]
    fn skips_when_chat_has_no_user_messages() {
        let body = Bytes::from(
            serde_json::to_vec(&json!({
                "messages": [
                    {"role": "assistant", "content": "latest assistant"}
                ]
            }))
            .expect("json should serialize"),
        );
        let decision = GatewayControlDecision::synthetic(
            "/v1/chat/completions",
            Some("ai_public".to_string()),
            Some("openai".to_string()),
            Some("chat_completions".to_string()),
            Some("openai:chat".to_string()),
        );

        let input = extract_inspection_input(Some(&decision), &HeaderMap::new(), &body, 1024, 128)
            .expect("input should parse");

        assert!(input.is_none());
    }

    #[test]
    fn keeps_full_keyword_text_when_provider_text_is_truncated() {
        let body = Bytes::from(
            serde_json::to_vec(&json!({
                "messages": [
                    {"role": "user", "content": "safe safe blocked"}
                ]
            }))
            .expect("json should serialize"),
        );
        let decision = GatewayControlDecision::synthetic(
            "/v1/chat/completions",
            Some("ai_public".to_string()),
            Some("openai".to_string()),
            Some("chat_completions".to_string()),
            Some("openai:chat".to_string()),
        );

        let input = extract_inspection_input(Some(&decision), &HeaderMap::new(), &body, 8, 128)
            .expect("input should parse")
            .expect("input should exist");

        assert_eq!(input.text, "safe saf");
        assert_eq!(input.keyword_text, "safe safe blocked");
        assert_eq!(input.normalized_hash_input, "safe safe blocked");
    }

    #[test]
    fn system_reminder_tags_are_stripped_so_inner_text_still_inspected() {
        let body = Bytes::from(
            serde_json::to_vec(&json!({
                "messages": [
                    {"role": "user", "content": "<system-reminder>secret payload</system-reminder>"}
                ]
            }))
            .expect("json should serialize"),
        );
        let decision = GatewayControlDecision::synthetic(
            "/v1/chat/completions",
            Some("ai_public".to_string()),
            Some("openai".to_string()),
            Some("chat_completions".to_string()),
            Some("openai:chat".to_string()),
        );

        let input = extract_inspection_input(Some(&decision), &HeaderMap::new(), &body, 1024, 128)
            .expect("input should parse")
            .expect("text inside system-reminder must still be inspected");

        assert!(input.text.contains("secret payload"));
        assert!(!input.text.contains("system-reminder"));
        assert_eq!(input.normalized_hash_input, "secret payload");
    }

    #[test]
    fn system_reminder_tags_with_attributes_or_uppercase_are_stripped() {
        let raw = "before <System-Reminder kind=\"x\">hidden</SYSTEM-REMINDER> after";
        let stripped = strip_system_reminder_tags(raw);
        let normalized = normalize_extracted_text(&stripped);
        assert_eq!(normalized, "before hidden after");
    }
}
