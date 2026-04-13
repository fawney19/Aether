use std::collections::BTreeMap;
use std::fmt::Write;

use aether_provider_transport::body_rules_handle_path;
use serde_json::{json, Value};
use sha1::{Digest as Sha1Digest, Sha1};
use sha2::Sha256;
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

fn is_openai_compact_request(provider_api_format: &str) -> bool {
    provider_api_format
        .trim()
        .eq_ignore_ascii_case("openai:compact")
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

fn btree_map_has_non_empty_value(headers: &BTreeMap<String, String>, header_name: &str) -> bool {
    let target = header_name.trim().to_ascii_lowercase();
    if target.is_empty() {
        return false;
    }

    headers
        .iter()
        .any(|(name, value)| name.trim().eq_ignore_ascii_case(&target) && !value.trim().is_empty())
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

pub fn apply_openai_compact_special_body_edits(
    provider_request_body: &mut Value,
    provider_api_format: &str,
) {
    if !is_openai_compact_request(provider_api_format) {
        return;
    }

    let Some(body_object) = provider_request_body.as_object_mut() else {
        return;
    };

    // `/v1/responses/compact` does not accept `store`.
    body_object.remove("store");
}

pub fn apply_codex_openai_cli_special_body_edits(
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
    if is_openai_compact_request(provider_api_format) {
        body_object.remove("store");
    } else if !body_rules_handle_path(body_rules, "store") {
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

pub fn apply_codex_openai_cli_special_headers(
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

    let prompt_cache_key = provider_request_body
        .get("prompt_cache_key")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());

    if !header_map_has_non_empty_value(original_headers, "chatgpt-account-id")
        && !btree_map_has_non_empty_value(provider_request_headers, "chatgpt-account-id")
    {
        if let Some(account_id) = extract_codex_account_id(decrypted_auth_config_raw) {
            provider_request_headers.insert("chatgpt-account-id".to_string(), account_id);
        }
    }

    if !header_map_has_non_empty_value(original_headers, "x-client-request-id")
        && !btree_map_has_non_empty_value(provider_request_headers, "x-client-request-id")
    {
        if let Some(request_id) = request_id.map(str::trim).filter(|value| !value.is_empty()) {
            provider_request_headers
                .insert("x-client-request-id".to_string(), request_id.to_string());
        }
    }

    let short_session_id = prompt_cache_key.and_then(build_short_codex_header_id);

    if !header_map_has_non_empty_value(original_headers, "session_id")
        && !btree_map_has_non_empty_value(provider_request_headers, "session_id")
    {
        if let Some(short_session_id) = short_session_id.as_deref() {
            provider_request_headers.insert("session_id".to_string(), short_session_id.to_string());
        }
    }

    if provider_api_format
        .trim()
        .eq_ignore_ascii_case("openai:cli")
        && !header_map_has_non_empty_value(original_headers, "conversation_id")
        && !btree_map_has_non_empty_value(provider_request_headers, "conversation_id")
    {
        if let Some(short_session_id) = short_session_id.as_deref() {
            provider_request_headers
                .insert("conversation_id".to_string(), short_session_id.to_string());
        }
    }
}
