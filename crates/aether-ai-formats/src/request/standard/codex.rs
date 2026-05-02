use std::collections::BTreeMap;
use std::fmt::Write;

use aether_ai_formats::provider_compat::proxy::rules::body_rules_handle_path;
use serde_json::{json, Value};
use sha1::{Digest as Sha1Digest, Sha1};
use sha2::Sha256;
use uuid::Uuid;

const CODEX_PROMPT_CACHE_NAMESPACE_VERSION: &str = "v3";
const CODEX_DEFAULT_INSTRUCTIONS: &str = "You are ChatGPT.";
const CODEX_DEFAULT_USER_AGENT: &str =
    "codex-tui/0.122.0 (Mac OS 15.2.0; arm64) vscode/2.6.11 (codex-tui; 0.122.0)";
const CODEX_DEFAULT_ORIGINATOR: &str = "codex-tui";
pub const CODEX_OPENAI_IMAGE_INTERNAL_MODEL: &str = "gpt-5.4-mini";
pub const CODEX_OPENAI_IMAGE_DEFAULT_MODEL: &str = "gpt-image-2";
pub const CODEX_OPENAI_IMAGE_DEFAULT_VARIATION_MODEL: &str = "dall-e-2";
pub const CODEX_OPENAI_IMAGE_DEFAULT_OUTPUT_FORMAT: &str = "png";
pub const CODEX_OPENAI_IMAGE_DEFAULT_VARIATION_PROMPT: &str =
    "Create a faithful variation of the provided image.";
const CODEX_IMAGE_TOOL_DEFAULT_SIZE: &str = "1024x1024";
const CODEX_IMAGE_TOOL_DEFAULT_QUALITY: &str = "high";
const CODEX_IMAGE_TOOL_DEFAULT_BACKGROUND: &str = "auto";
const UUID_NAMESPACE_OID_BYTES: [u8; 16] = [
    0x6b, 0xa7, 0xb8, 0x12, 0x9d, 0xad, 0x11, 0xd1, 0x80, 0xb4, 0x00, 0xc0, 0x4f, 0xd4, 0x30, 0xc8,
];

fn is_codex_openai_responses_request(provider_type: &str, provider_api_format: &str) -> bool {
    provider_type.trim().eq_ignore_ascii_case("codex")
        && (aether_ai_formats::is_openai_responses_family_format(provider_api_format)
            || is_openai_image_request(provider_api_format))
}

fn is_openai_responses_compact_request(provider_api_format: &str) -> bool {
    aether_ai_formats::is_openai_responses_compact_format(provider_api_format)
}

fn is_openai_image_request(provider_api_format: &str) -> bool {
    provider_api_format
        .trim()
        .eq_ignore_ascii_case("openai:image")
}

fn apply_codex_openai_image_tool_overrides(body_object: &mut serde_json::Map<String, Value>) {
    let mut tool = body_object
        .get("tools")
        .and_then(Value::as_array)
        .and_then(|tools| tools.first())
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    tool.insert("type".to_string(), json!("image_generation"));
    tool.entry("output_format".to_string())
        .or_insert_with(|| json!(CODEX_OPENAI_IMAGE_DEFAULT_OUTPUT_FORMAT));
    let action = tool
        .get("action")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("generate")
        .to_string();
    if !tool.contains_key("action") {
        tool.insert("action".to_string(), json!("generate"));
    }
    if action == "generate" {
        tool.entry("size".to_string())
            .or_insert_with(|| json!(CODEX_IMAGE_TOOL_DEFAULT_SIZE));
        tool.entry("quality".to_string())
            .or_insert_with(|| json!(CODEX_IMAGE_TOOL_DEFAULT_QUALITY));
        tool.entry("background".to_string())
            .or_insert_with(|| json!(CODEX_IMAGE_TOOL_DEFAULT_BACKGROUND));
    }

    body_object.insert("tools".to_string(), json!([tool]));
    body_object.insert(
        "tool_choice".to_string(),
        json!({
            "type": "image_generation"
        }),
    );
}

fn codex_openai_image_has_prompt(body_object: &serde_json::Map<String, Value>) -> bool {
    body_object
        .get("input")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .filter_map(|item| item.get("content"))
        .any(|content| match content {
            Value::String(text) => !text.trim().is_empty(),
            Value::Array(items) => items.iter().any(|item| {
                item.as_object()
                    .filter(|item| item.get("type").and_then(Value::as_str) == Some("input_text"))
                    .and_then(|item| item.get("text").and_then(Value::as_str))
                    .map(str::trim)
                    .is_some_and(|text| !text.is_empty())
            }),
            _ => false,
        })
}

fn inject_codex_default_variation_prompt(body_object: &mut serde_json::Map<String, Value>) {
    let Some(action) = body_object
        .get("tools")
        .and_then(Value::as_array)
        .and_then(|tools| tools.first())
        .and_then(Value::as_object)
        .and_then(|tool| tool.get("action"))
        .and_then(Value::as_str)
    else {
        return;
    };
    if action != "edit" || codex_openai_image_has_prompt(body_object) {
        return;
    }

    let Some(input) = body_object.get_mut("input").and_then(Value::as_array_mut) else {
        return;
    };
    let Some(first_message) = input.first_mut().and_then(Value::as_object_mut) else {
        return;
    };
    let Some(content) = first_message
        .get_mut("content")
        .and_then(Value::as_array_mut)
    else {
        return;
    };

    content.insert(
        0,
        json!({
            "type": "input_text",
            "text": CODEX_OPENAI_IMAGE_DEFAULT_VARIATION_PROMPT,
        }),
    );
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

fn maybe_insert_default_codex_header(
    provider_request_headers: &mut BTreeMap<String, String>,
    original_headers: &http::HeaderMap,
    header_name: &str,
    header_value: &str,
) {
    if header_map_has_non_empty_value(original_headers, header_name)
        || btree_map_has_non_empty_value(provider_request_headers, header_name)
    {
        return;
    }

    provider_request_headers.insert(header_name.to_string(), header_value.to_string());
}

fn maybe_inject_codex_prompt_cache_key(
    provider_request_body: &mut Value,
    provider_type: &str,
    provider_api_format: &str,
    user_api_key_id: Option<&str>,
) {
    if !is_codex_openai_responses_request(provider_type, provider_api_format) {
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

pub fn apply_openai_responses_compact_special_body_edits(
    provider_request_body: &mut Value,
    provider_api_format: &str,
) {
    if !is_openai_responses_compact_request(provider_api_format) {
        return;
    }

    let Some(body_object) = provider_request_body.as_object_mut() else {
        return;
    };

    // `/v1/responses/compact` does not accept `store`.
    body_object.remove("store");
}

pub fn apply_codex_openai_responses_special_body_edits(
    provider_request_body: &mut Value,
    provider_type: &str,
    provider_api_format: &str,
    body_rules: Option<&Value>,
    user_api_key_id: Option<&str>,
) {
    if !is_codex_openai_responses_request(provider_type, provider_api_format) {
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
    if is_openai_responses_compact_request(provider_api_format) {
        body_object.remove("store");
    } else if !body_rules_handle_path(body_rules, "store") {
        body_object.insert("store".to_string(), json!(false));
    }
    if !body_rules_handle_path(body_rules, "instructions")
        && !body_object.contains_key("instructions")
    {
        body_object.insert(
            "instructions".to_string(),
            json!(CODEX_DEFAULT_INSTRUCTIONS),
        );
    }
    if is_openai_image_request(provider_api_format) {
        body_object.insert(
            "model".to_string(),
            json!(CODEX_OPENAI_IMAGE_INTERNAL_MODEL),
        );
        body_object.insert("stream".to_string(), json!(true));
        apply_codex_openai_image_tool_overrides(body_object);
        inject_codex_default_variation_prompt(body_object);
    }

    maybe_inject_codex_prompt_cache_key(
        provider_request_body,
        provider_type,
        provider_api_format,
        user_api_key_id,
    );
}

pub fn apply_codex_openai_responses_special_headers(
    provider_request_headers: &mut BTreeMap<String, String>,
    provider_request_body: &Value,
    original_headers: &http::HeaderMap,
    provider_type: &str,
    provider_api_format: &str,
    request_id: Option<&str>,
    decrypted_auth_config_raw: Option<&str>,
) {
    if !is_codex_openai_responses_request(provider_type, provider_api_format) {
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

    if !is_openai_image_request(provider_api_format) {
        maybe_insert_default_codex_header(
            provider_request_headers,
            original_headers,
            "user-agent",
            CODEX_DEFAULT_USER_AGENT,
        );
        maybe_insert_default_codex_header(
            provider_request_headers,
            original_headers,
            "originator",
            CODEX_DEFAULT_ORIGINATOR,
        );
    }

    let short_session_id = prompt_cache_key.and_then(build_short_codex_header_id);

    if !header_map_has_non_empty_value(original_headers, "session_id")
        && !btree_map_has_non_empty_value(provider_request_headers, "session_id")
    {
        if let Some(short_session_id) = short_session_id.as_deref() {
            provider_request_headers.insert("session_id".to_string(), short_session_id.to_string());
        }
    }

    if aether_ai_formats::is_openai_responses_format(provider_api_format)
        && !header_map_has_non_empty_value(original_headers, "conversation_id")
        && !btree_map_has_non_empty_value(provider_request_headers, "conversation_id")
    {
        if let Some(short_session_id) = short_session_id.as_deref() {
            provider_request_headers
                .insert("conversation_id".to_string(), short_session_id.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        apply_codex_openai_responses_special_body_edits, CODEX_OPENAI_IMAGE_INTERNAL_MODEL,
    };
    use serde_json::json;

    #[test]
    fn codex_image_body_edits_force_tool_choice_and_default_generate_tool_fields() {
        let mut provider_request_body = json!({
            "input": [{
                "role": "user",
                "content": "generate image"
            }],
            "tools": [{
                "type": "image_generation"
            }],
            "tool_choice": "auto"
        });

        apply_codex_openai_responses_special_body_edits(
            &mut provider_request_body,
            "codex",
            "openai:image",
            None,
            None,
        );

        assert_eq!(
            provider_request_body["tools"][0]["size"],
            json!("1024x1024")
        );
        assert_eq!(provider_request_body["tools"][0]["quality"], json!("high"));
        assert_eq!(
            provider_request_body["tools"][0]["background"],
            json!("auto")
        );
        assert_eq!(
            provider_request_body["tools"][0]["output_format"],
            json!("png")
        );
        assert_eq!(
            provider_request_body["tools"][0]["action"],
            json!("generate")
        );
        assert_eq!(
            provider_request_body["model"],
            json!(CODEX_OPENAI_IMAGE_INTERNAL_MODEL)
        );
        assert_eq!(provider_request_body["stream"], json!(true));
        assert_eq!(
            provider_request_body["tool_choice"]["type"],
            json!("image_generation")
        );
    }

    #[test]
    fn codex_image_body_edits_preserve_edit_action_without_generate_defaults() {
        let mut provider_request_body = json!({
            "tools": [{
                "type": "image_generation",
                "action": "edit",
                "input_image_mask": { "image_url": "data:image/png;base64,mask" }
            }],
            "input": [{
                "role": "user",
                "content": [{
                    "type": "input_image",
                    "image_url": "data:image/png;base64,image"
                }]
            }],
            "tool_choice": "auto"
        });

        apply_codex_openai_responses_special_body_edits(
            &mut provider_request_body,
            "codex",
            "openai:image",
            None,
            None,
        );

        assert_eq!(provider_request_body["tools"][0]["action"], json!("edit"));
        assert!(provider_request_body["tools"][0].get("size").is_none());
        assert!(provider_request_body["tools"][0].get("quality").is_none());
        assert!(provider_request_body["tools"][0]
            .get("background")
            .is_none());
        assert_eq!(
            provider_request_body["tools"][0]["output_format"],
            json!("png")
        );
        assert_eq!(
            provider_request_body["input"][0]["content"][0]["text"],
            json!("Create a faithful variation of the provided image.")
        );
        assert_eq!(
            provider_request_body["tool_choice"]["type"],
            json!("image_generation")
        );
    }
}
