use std::collections::BTreeMap;
use std::fmt::Write;

use crate::ai_pipeline::provider_transport_facade::body_rules_handle_path;
use serde_json::{json, Value};
use sha1::{Digest as Sha1Digest, Sha1};
use sha2::{Digest as Sha2Digest, Sha256};
use uuid::Uuid;

const CODEX_PROMPT_CACHE_NAMESPACE_VERSION: &str = "v3";
const UUID_NAMESPACE_OID_BYTES: [u8; 16] = [
    0x6b, 0xa7, 0xb8, 0x12, 0x9d, 0xad, 0x11, 0xd1, 0x80, 0xb4, 0x00, 0xc0, 0x4f, 0xd4, 0x30, 0xc8,
];

fn is_codex_openai_cli_request(provider_type: &str, provider_api_format: &str) -> bool {
    provider_type.trim().eq_ignore_ascii_case("codex")
        && matches!(
            provider_api_format.trim().to_ascii_lowercase().as_str(),
            "openai:cli" | "openai:compact"
        )
}

fn build_stable_codex_prompt_cache_key(user_api_key_id: &str) -> Option<String> {
    let normalized = user_api_key_id.trim();
    if normalized.is_empty() {
        return None;
    }

    let namespace = format!(
        "aether:codex:prompt-cache:{CODEX_PROMPT_CACHE_NAMESPACE_VERSION}:user:{normalized}"
    );
    let mut hasher = Sha1::new();
    hasher.update(UUID_NAMESPACE_OID_BYTES);
    hasher.update(namespace.as_bytes());

    let digest = hasher.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Some(Uuid::from_bytes(bytes).to_string())
}

fn maybe_inject_codex_prompt_cache_key(
    provider_request_body: &mut Value,
    provider_type: &str,
    provider_api_format: &str,
    user_api_key_id: Option<&str>,
) {
    if !is_codex_openai_cli_request(provider_type, provider_api_format) {
        return;
    }

    let Some(body_object) = provider_request_body.as_object_mut() else {
        return;
    };

    let existing = body_object
        .get("prompt_cache_key")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if !existing.is_empty() {
        return;
    }

    let Some(prompt_cache_key) = user_api_key_id.and_then(build_stable_codex_prompt_cache_key)
    else {
        return;
    };

    body_object.insert(
        "prompt_cache_key".to_string(),
        Value::String(prompt_cache_key),
    );
}

fn build_short_codex_header_id(seed: &str) -> Option<String> {
    let normalized = seed.trim();
    if normalized.is_empty() {
        return None;
    }

    let digest = Sha256::digest(normalized.as_bytes());
    let mut short_id = String::with_capacity(16);
    for byte in digest.iter().take(8) {
        let _ = write!(&mut short_id, "{byte:02x}");
    }
    Some(short_id)
}

fn header_map_has_non_empty_value(headers: &http::HeaderMap, header_name: &str) -> bool {
    let target = header_name.trim().to_ascii_lowercase();
    if target.is_empty() {
        return false;
    }

    headers.iter().any(|(name, value)| {
        if name.as_str().trim().to_ascii_lowercase() != target {
            return false;
        }
        value
            .to_str()
            .ok()
            .map(str::trim)
            .map(|value| !value.is_empty())
            .unwrap_or(false)
    })
}

fn extract_codex_account_id(decrypted_auth_config_raw: Option<&str>) -> Option<String> {
    let raw = decrypted_auth_config_raw?.trim();
    if raw.is_empty() {
        return None;
    }

    serde_json::from_str::<Value>(raw).ok().and_then(|value| {
        value
            .get("account_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    })
}

pub(crate) fn apply_codex_openai_cli_special_headers(
    provider_request_headers: &mut BTreeMap<String, String>,
    provider_request_body: &Value,
    original_headers: &http::HeaderMap,
    provider_type: &str,
    provider_api_format: &str,
    request_id: Option<&str>,
    decrypted_auth_config_raw: Option<&str>,
) {
    if !is_codex_openai_cli_request(provider_type, provider_api_format) {
        return;
    }

    if let Some(account_id) = extract_codex_account_id(decrypted_auth_config_raw) {
        provider_request_headers.insert("chatgpt-account-id".to_string(), account_id);
    }
    if !provider_request_headers
        .get("x-client-request-id")
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
    {
        if let Some(request_id) = request_id.map(str::trim).filter(|value| !value.is_empty()) {
            provider_request_headers
                .insert("x-client-request-id".to_string(), request_id.to_string());
        }
    }

    let prompt_cache_key = provider_request_body
        .get("prompt_cache_key")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let Some(short_id) = prompt_cache_key.and_then(build_short_codex_header_id) else {
        return;
    };

    if !header_map_has_non_empty_value(original_headers, "session_id") {
        provider_request_headers.insert("session_id".to_string(), short_id.clone());
    }

    if !provider_api_format
        .trim()
        .eq_ignore_ascii_case("openai:compact")
        && !header_map_has_non_empty_value(original_headers, "conversation_id")
    {
        provider_request_headers.insert("conversation_id".to_string(), short_id);
    }
}

pub(crate) fn apply_codex_openai_cli_special_body_edits(
    provider_request_body: &mut Value,
    provider_type: &str,
    provider_api_format: &str,
    body_rules: Option<&Value>,
    user_api_key_id: Option<&str>,
) {
    if !is_codex_openai_cli_request(provider_type, provider_api_format) {
        return;
    }

    let Some(body_object) = provider_request_body.as_object_mut() else {
        return;
    };

    if !body_rules_handle_path(body_rules, "max_output_tokens") {
        body_object.remove("max_output_tokens");
    }
    if !body_rules_handle_path(body_rules, "temperature") {
        body_object.remove("temperature");
    }
    if !body_rules_handle_path(body_rules, "top_p") {
        body_object.remove("top_p");
    }
    if !body_rules_handle_path(body_rules, "metadata") {
        body_object.remove("metadata");
    }
    if !body_rules_handle_path(body_rules, "store") {
        body_object.insert("store".to_string(), json!(false));
    }
    if !body_rules_handle_path(body_rules, "instructions")
        && !body_object.contains_key("instructions")
    {
        body_object.insert("instructions".to_string(), json!("You are GPT-5."));
    }

    maybe_inject_codex_prompt_cache_key(
        provider_request_body,
        provider_type,
        provider_api_format,
        user_api_key_id,
    );
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        apply_codex_openai_cli_special_body_edits, apply_codex_openai_cli_special_headers,
    };
    use http::{HeaderMap, HeaderValue};
    use serde_json::json;

    #[test]
    fn applies_codex_defaults_when_body_rules_do_not_handle_fields() {
        let mut body = json!({
            "model": "gpt-5",
            "max_output_tokens": 128,
            "temperature": 0.3,
            "top_p": 0.9,
            "metadata": {"client": "desktop"},
            "store": true
        });

        apply_codex_openai_cli_special_body_edits(&mut body, "codex", "openai:cli", None, None);

        assert!(body.get("max_output_tokens").is_none());
        assert!(body.get("temperature").is_none());
        assert!(body.get("top_p").is_none());
        assert!(body.get("metadata").is_none());
        assert_eq!(body["store"], false);
        assert_eq!(body["instructions"], "You are GPT-5.");
    }

    #[test]
    fn defers_to_user_body_rules_for_handled_fields() {
        let body_rules = json!([
            {"action":"set","path":"store","value":true},
            {"action":"set","path":"instructions","value":"Keep custom"},
            {"action":"set","path":"metadata","value":{"client":"desktop","mode":"custom"}},
            {"action":"set","path":"top_p","value":0.5}
        ]);
        let mut body = json!({
            "model": "gpt-5",
            "max_output_tokens": 128,
            "metadata": {"client": "desktop", "mode": "custom"},
            "store": true,
            "instructions": "Keep custom",
            "top_p": 0.5
        });

        apply_codex_openai_cli_special_body_edits(
            &mut body,
            "codex",
            "openai:compact",
            Some(&body_rules),
            None,
        );

        assert!(body.get("max_output_tokens").is_none());
        assert_eq!(body["store"], true);
        assert_eq!(body["instructions"], "Keep custom");
        assert_eq!(body["metadata"]["mode"], "custom");
        assert_eq!(body["top_p"], 0.5);
    }

    #[test]
    fn injects_stable_prompt_cache_key_for_codex_requests() {
        let mut body = json!({
            "model": "gpt-5",
            "input": "hello",
        });

        apply_codex_openai_cli_special_body_edits(
            &mut body,
            "codex",
            "openai:cli",
            None,
            Some("key-123"),
        );

        assert_eq!(
            body["prompt_cache_key"],
            "172c39e6-c0a0-5a70-8b63-e0f8e0d185a3"
        );
    }

    #[test]
    fn keeps_existing_prompt_cache_key_for_codex_requests() {
        let mut body = json!({
            "model": "gpt-5",
            "input": "hello",
            "prompt_cache_key": "existing-key",
        });

        apply_codex_openai_cli_special_body_edits(
            &mut body,
            "codex",
            "openai:cli",
            None,
            Some("key-123"),
        );

        assert_eq!(body["prompt_cache_key"], "existing-key");
    }

    #[test]
    fn injects_chatgpt_account_id_and_session_headers_for_codex_requests() {
        let mut headers = BTreeMap::new();
        let body = json!({
            "model": "gpt-5",
            "prompt_cache_key": "172c39e6-c0a0-5a70-8b63-e0f8e0d185a3",
        });

        apply_codex_openai_cli_special_headers(
            &mut headers,
            &body,
            &HeaderMap::new(),
            "codex",
            "openai:cli",
            Some("trace-codex-123"),
            Some(r#"{"account_id":"acc-123"}"#),
        );

        assert_eq!(
            headers.get("chatgpt-account-id"),
            Some(&"acc-123".to_string())
        );
        assert_eq!(
            headers.get("x-client-request-id"),
            Some(&"trace-codex-123".to_string())
        );
        assert_eq!(
            headers.get("session_id"),
            Some(&"ab5ecce4f0d110fe".to_string())
        );
        assert_eq!(
            headers.get("conversation_id"),
            Some(&"ab5ecce4f0d110fe".to_string())
        );
    }

    #[test]
    fn respects_existing_codex_request_and_session_headers() {
        let mut headers = BTreeMap::new();
        headers.insert(
            "x-client-request-id".to_string(),
            "kept-by-rule-request".to_string(),
        );
        headers.insert("session_id".to_string(), "kept-by-rule".to_string());
        let body = json!({
            "model": "gpt-5",
            "prompt_cache_key": "172c39e6-c0a0-5a70-8b63-e0f8e0d185a3",
        });
        let mut original_headers = HeaderMap::new();
        original_headers.insert(
            "x-client-request-id",
            HeaderValue::from_static("user-specified-request"),
        );
        original_headers.insert(
            "session_id",
            HeaderValue::from_static("user-specified-session"),
        );
        original_headers.insert(
            "conversation_id",
            HeaderValue::from_static("user-specified-conversation"),
        );

        apply_codex_openai_cli_special_headers(
            &mut headers,
            &body,
            &original_headers,
            "codex",
            "openai:cli",
            Some("trace-codex-123"),
            Some(r#"{"account_id":"acc-123"}"#),
        );

        assert_eq!(
            headers.get("x-client-request-id"),
            Some(&"kept-by-rule-request".to_string())
        );
        assert_eq!(headers.get("session_id"), Some(&"kept-by-rule".to_string()));
        assert!(!headers.contains_key("conversation_id"));
    }

    #[test]
    fn skips_conversation_id_for_compact_codex_requests() {
        let mut headers = BTreeMap::new();
        let body = json!({
            "model": "gpt-5",
            "prompt_cache_key": "172c39e6-c0a0-5a70-8b63-e0f8e0d185a3",
        });

        apply_codex_openai_cli_special_headers(
            &mut headers,
            &body,
            &HeaderMap::new(),
            "codex",
            "openai:compact",
            Some("trace-codex-compact-123"),
            Some(r#"{"account_id":"acc-123"}"#),
        );

        assert_eq!(
            headers.get("chatgpt-account-id"),
            Some(&"acc-123".to_string())
        );
        assert_eq!(
            headers.get("x-client-request-id"),
            Some(&"trace-codex-compact-123".to_string())
        );
        assert_eq!(
            headers.get("session_id"),
            Some(&"ab5ecce4f0d110fe".to_string())
        );
        assert!(!headers.contains_key("conversation_id"));
    }
}
