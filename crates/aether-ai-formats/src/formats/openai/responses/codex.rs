use std::collections::BTreeMap;
use std::fmt::Write;

use aether_ai_formats::provider_compat::proxy::rules::body_rules_handle_path;
use serde_json::{json, Value};
use sha1::{Digest as Sha1Digest, Sha1};
use sha2::Sha256;
use uuid::Uuid;

const CODEX_PROMPT_CACHE_NAMESPACE_VERSION: &str = "v3";
const CODEX_DEFAULT_INSTRUCTIONS: &str = "";
const CODEX_DEFAULT_REASONING_EFFORT: &str = "medium";
const CODEX_DEFAULT_REASONING_SUMMARY: &str = "auto";
const CODEX_REASONING_ENCRYPTED_CONTENT_INCLUDE: &str = "reasoning.encrypted_content";
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

/// Returns true only when `tool_choice` *explicitly* targets image_generation,
/// matching either the `"image_generation"` string form or the
/// `{"type":"image_generation"}` object form.
///
/// Intentionally does NOT inspect the `tools` array: tools merely advertise
/// what is available, while only `tool_choice` expresses the caller's
/// selection. Treating "image_generation present in tools" as a trigger
/// caused codex CLI requests (which list image_generation alongside ~20
/// other tools under `tool_choice: "auto"`) to be incorrectly rewritten
/// into image-generation-only requests, leading to upstream 400s.
fn codex_openai_responses_tool_choice_references_image_generation(
    body_object: &serde_json::Map<String, Value>,
) -> bool {
    match body_object.get("tool_choice") {
        Some(Value::String(name)) => name.trim().eq_ignore_ascii_case("image_generation"),
        Some(Value::Object(choice)) => choice
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| kind.trim().eq_ignore_ascii_case("image_generation")),
        _ => false,
    }
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

fn build_stable_codex_prompt_cache_key_from_seed(kind: &str, seed: &str) -> Option<String> {
    let normalized = seed.trim();
    if normalized.is_empty() {
        return None;
    }

    let normalized_kind = kind
        .trim()
        .to_ascii_lowercase()
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '_' || *ch == '-')
        .collect::<String>();
    let normalized_kind = if normalized_kind.is_empty() {
        "seed".to_string()
    } else {
        normalized_kind
    };
    let namespace = format!(
        "aether:codex:prompt-cache:{CODEX_PROMPT_CACHE_NAMESPACE_VERSION}:{normalized_kind}:{normalized}"
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

fn build_stable_codex_prompt_cache_key(user_api_key_id: &str) -> Option<String> {
    build_stable_codex_prompt_cache_key_from_seed("user", user_api_key_id)
}

fn extract_codex_prompt_cache_session_seed(provider_request_body: &Value) -> Option<String> {
    fn non_empty_str(value: Option<&Value>) -> Option<&str> {
        value
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }

    fn session_seed_from_metadata(metadata: &Value) -> Option<String> {
        let object = metadata.as_object()?;
        non_empty_str(object.get("session_id"))
            .or_else(|| non_empty_str(object.get("sessionId")))
            .or_else(|| non_empty_str(object.get("conversation_id")))
            .or_else(|| non_empty_str(object.get("conversationId")))
            .map(|value| format!("metadata:{value}"))
            .or_else(|| {
                let user_id = non_empty_str(object.get("user_id"))?;
                serde_json::from_str::<Value>(user_id)
                    .ok()
                    .and_then(|decoded| {
                        non_empty_str(decoded.get("session_id"))
                            .or_else(|| non_empty_str(decoded.get("sessionId")))
                            .or_else(|| non_empty_str(decoded.get("conversation_id")))
                            .or_else(|| non_empty_str(decoded.get("conversationId")))
                            .map(|value| format!("metadata.user_id:{value}"))
                    })
            })
    }

    let object = provider_request_body.as_object()?;
    non_empty_str(object.get("session_id"))
        .or_else(|| non_empty_str(object.get("sessionId")))
        .or_else(|| non_empty_str(object.get("conversation_id")))
        .or_else(|| non_empty_str(object.get("conversationId")))
        .map(|value| format!("body:{value}"))
        .or_else(|| object.get("metadata").and_then(session_seed_from_metadata))
}

fn sha256_hex(input: &[u8]) -> String {
    let digest = Sha256::digest(input);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(&mut output, "{byte:02x}");
    }
    output
}

fn stable_json_digest(value: &Value) -> Option<String> {
    serde_json::to_vec(value)
        .ok()
        .map(|serialized| sha256_hex(&serialized))
}

fn compact_prompt_cache_text(value: &str) -> Option<Value> {
    const MAX_PROMPT_CACHE_TEXT_CHARS: usize = 4096;
    let normalized = value.trim();
    if normalized.is_empty() {
        return None;
    }
    let mut text = normalized
        .chars()
        .take(MAX_PROMPT_CACHE_TEXT_CHARS)
        .collect::<String>();
    if normalized.chars().count() > MAX_PROMPT_CACHE_TEXT_CHARS {
        text.push_str("...");
    }
    Some(Value::String(text))
}

fn compact_prompt_cache_anchor(value: &Value) -> Value {
    match value {
        Value::String(text) => compact_prompt_cache_text(text).unwrap_or(Value::Null),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .take(16)
                .map(compact_prompt_cache_anchor)
                .filter(|value| !value.is_null())
                .collect(),
        ),
        Value::Object(object) => {
            let mut compacted = serde_json::Map::new();
            for key in [
                "type",
                "role",
                "id",
                "name",
                "description",
                "text",
                "input_text",
                "output_text",
                "content",
                "call_id",
                "arguments",
                "output",
                "parameters",
                "strict",
                "function",
                "effort",
                "summary",
            ] {
                let Some(value) = object.get(key) else {
                    continue;
                };
                let value = compact_prompt_cache_anchor(value);
                if !value.is_null() {
                    compacted.insert(key.to_string(), value);
                }
            }
            Value::Object(compacted)
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => value.clone(),
    }
}

fn compact_prompt_cache_json_anchor(value: &Value) -> Value {
    match value {
        Value::String(text) => compact_prompt_cache_text(text).unwrap_or(Value::Null),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .take(16)
                .map(compact_prompt_cache_json_anchor)
                .filter(|value| !value.is_null())
                .collect(),
        ),
        Value::Object(object) => {
            let mut compacted = serde_json::Map::new();
            let mut keys = object.keys().collect::<Vec<_>>();
            keys.sort();
            for key in keys {
                if key == "cache_control" {
                    continue;
                }
                let Some(value) = object.get(key) else {
                    continue;
                };
                let value = compact_prompt_cache_json_anchor(value);
                if !value.is_null() {
                    compacted.insert(key.clone(), value);
                }
            }
            Value::Object(compacted)
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => value.clone(),
    }
}

fn collect_codex_prompt_cache_control_anchors(value: &Value, anchors: &mut Vec<Value>) {
    const MAX_PROMPT_CACHE_CONTROL_ANCHORS: usize = 16;
    if anchors.len() >= MAX_PROMPT_CACHE_CONTROL_ANCHORS {
        return;
    }

    match value {
        Value::Object(object) => {
            if object.contains_key("cache_control") {
                let mut anchor = object.clone();
                anchor.remove("cache_control");
                let anchor = compact_prompt_cache_anchor(&Value::Object(anchor));
                if !anchor.is_null() {
                    anchors.push(anchor);
                }
            }
            for child in object.values() {
                if anchors.len() >= MAX_PROMPT_CACHE_CONTROL_ANCHORS {
                    break;
                }
                collect_codex_prompt_cache_control_anchors(child, anchors);
            }
        }
        Value::Array(items) => {
            for child in items {
                if anchors.len() >= MAX_PROMPT_CACHE_CONTROL_ANCHORS {
                    break;
                }
                collect_codex_prompt_cache_control_anchors(child, anchors);
            }
        }
        _ => {}
    }
}

fn extract_codex_prompt_cache_control_seed(provider_request_body: &Value) -> Option<String> {
    let mut anchors = Vec::new();
    collect_codex_prompt_cache_control_anchors(provider_request_body, &mut anchors);
    if anchors.is_empty() {
        return None;
    }

    let seed = json!({
        "model": provider_request_body.get("model"),
        "anchors": anchors,
    });
    stable_json_digest(&seed).map(|digest| format!("cache_control:{digest}"))
}

fn first_responses_input_anchor(input: &Value) -> Option<Value> {
    let items = input.as_array()?;
    let first_user_message = items.iter().find(|item| {
        item.get("type")
            .and_then(Value::as_str)
            .is_some_and(|value| value == "message")
            && item
                .get("role")
                .and_then(Value::as_str)
                .is_some_and(|value| value == "user")
    });
    let first_item = first_user_message.or_else(|| items.first())?;
    let anchor = compact_prompt_cache_anchor(first_item);
    (!anchor.is_null()).then_some(anchor)
}

fn extract_codex_stable_request_prompt_cache_seed(
    provider_request_body: &Value,
    user_api_key_id: Option<&str>,
) -> Option<String> {
    let object = provider_request_body.as_object()?;
    let mut seed = serde_json::Map::new();

    for key in [
        "model",
        "instructions",
        "reasoning",
        "tools",
        "tool_choice",
        "parallel_tool_calls",
    ] {
        if let Some(value) = object.get(key).filter(|value| !value.is_null()) {
            let value = if key == "tools" {
                compact_prompt_cache_json_anchor(value)
            } else {
                compact_prompt_cache_anchor(value)
            };
            seed.insert(key.to_string(), value);
        }
    }
    if let Some(input_anchor) = object.get("input").and_then(first_responses_input_anchor) {
        seed.insert("first_input".to_string(), input_anchor);
    }
    if let Some(user_api_key_id) = user_api_key_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        seed.insert(
            "api_key_id".to_string(),
            Value::String(user_api_key_id.to_string()),
        );
    }

    if seed.len() < 2 {
        return None;
    }
    stable_json_digest(&Value::Object(seed)).map(|digest| format!("stable_request:{digest}"))
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

fn codex_prompt_cache_key_to_insert(
    provider_request_body: &Value,
    provider_type: &str,
    provider_api_format: &str,
    user_api_key_id: Option<&str>,
) -> Option<String> {
    if !is_codex_openai_responses_request(provider_type, provider_api_format) {
        return None;
    }

    let existing = provider_request_body
        .get("prompt_cache_key")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if !existing.is_empty() {
        return None;
    }

    extract_codex_prompt_cache_session_seed(provider_request_body)
        .and_then(|seed| build_stable_codex_prompt_cache_key_from_seed("session", &seed))
        .or_else(|| {
            extract_codex_prompt_cache_control_seed(provider_request_body)
                .and_then(|seed| build_stable_codex_prompt_cache_key_from_seed("anchor", &seed))
        })
        .or_else(|| {
            extract_codex_stable_request_prompt_cache_seed(provider_request_body, user_api_key_id)
                .and_then(|seed| build_stable_codex_prompt_cache_key_from_seed("request", &seed))
        })
        .or_else(|| user_api_key_id.and_then(build_stable_codex_prompt_cache_key))
}

fn insert_codex_prompt_cache_key(
    provider_request_body: &mut Value,
    prompt_cache_key: Option<String>,
) {
    let Some(prompt_cache_key) = prompt_cache_key else {
        return;
    };

    let Some(body_object) = provider_request_body.as_object_mut() else {
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

    // `/v1/responses/compact` does not accept `include`, `store`, or body-level `stream`.
    body_object.remove("include");
    body_object.remove("store");
    body_object.remove("stream");
}

fn ensure_codex_responses_passthrough_fields(
    body_object: &mut serde_json::Map<String, Value>,
    provider_api_format: &str,
    body_rules: Option<&Value>,
) {
    if is_openai_responses_compact_request(provider_api_format)
        || is_openai_image_request(provider_api_format)
    {
        return;
    }
    if !body_rules_handle_path(body_rules, "parallel_tool_calls") {
        body_object
            .entry("parallel_tool_calls".to_string())
            .or_insert_with(|| json!(true));
    }
    if !body_rules_handle_path(body_rules, "include") {
        match body_object.get_mut("include") {
            Some(Value::Array(include)) => {
                let has_reasoning_encrypted_content = include
                    .iter()
                    .any(|value| value.as_str() == Some(CODEX_REASONING_ENCRYPTED_CONTENT_INCLUDE));
                if !has_reasoning_encrypted_content {
                    include.push(json!(CODEX_REASONING_ENCRYPTED_CONTENT_INCLUDE));
                }
            }
            Some(_) | None => {
                body_object.insert(
                    "include".to_string(),
                    json!([CODEX_REASONING_ENCRYPTED_CONTENT_INCLUDE]),
                );
            }
        }
    }
}

fn ensure_codex_chat_reasoning_defaults(
    body_object: &mut serde_json::Map<String, Value>,
    provider_api_format: &str,
    body_rules: Option<&Value>,
) {
    if is_openai_responses_compact_request(provider_api_format)
        || is_openai_image_request(provider_api_format)
    {
        return;
    }
    if body_rules_handle_path(body_rules, "reasoning") {
        return;
    }
    let reasoning = body_object
        .entry("reasoning".to_string())
        .or_insert_with(|| json!({}));
    let Some(reasoning_object) = reasoning.as_object_mut() else {
        return;
    };
    reasoning_object
        .entry("effort".to_string())
        .or_insert_with(|| json!(CODEX_DEFAULT_REASONING_EFFORT));
    reasoning_object
        .entry("summary".to_string())
        .or_insert_with(|| json!(CODEX_DEFAULT_REASONING_SUMMARY));
}

fn codex_tool_type_rejects_top_level_name(tool_type: &str) -> bool {
    let normalized = tool_type.trim().to_ascii_lowercase();
    !normalized.is_empty()
        && normalized != "function"
        && normalized != "custom"
        && normalized != "namespace"
}

fn strip_codex_hosted_tool_names_for_backend(body_object: &mut serde_json::Map<String, Value>) {
    let Some(tools) = body_object.get_mut("tools").and_then(Value::as_array_mut) else {
        return;
    };

    for tool in tools {
        let Some(tool_object) = tool.as_object_mut() else {
            continue;
        };
        if tool_object
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(codex_tool_type_rejects_top_level_name)
        {
            tool_object.remove("name");
        }
    }
}

fn strip_codex_hosted_tool_choice_name_for_backend(
    body_object: &mut serde_json::Map<String, Value>,
) {
    let Some(tool_choice_object) = body_object
        .get_mut("tool_choice")
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    if tool_choice_object
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(codex_tool_type_rejects_top_level_name)
    {
        tool_choice_object.remove("name");
    }
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

    let prompt_cache_key = codex_prompt_cache_key_to_insert(
        provider_request_body,
        provider_type,
        provider_api_format,
        user_api_key_id,
    );

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
    ensure_codex_responses_passthrough_fields(body_object, provider_api_format, body_rules);
    if !body_rules_handle_path(body_rules, "instructions")
        && !body_object.contains_key("instructions")
    {
        body_object.insert(
            "instructions".to_string(),
            json!(CODEX_DEFAULT_INSTRUCTIONS),
        );
    } else if body_object.contains_key("instructions")
        && body_object.get("instructions").is_some_and(|v| v.is_null())
    {
        body_object.insert("instructions".to_string(), json!(""));
    }
    strip_codex_hosted_tool_names_for_backend(body_object);
    strip_codex_hosted_tool_choice_name_for_backend(body_object);
    if is_openai_image_request(provider_api_format)
        || codex_openai_responses_tool_choice_references_image_generation(body_object)
    {
        body_object.insert(
            "model".to_string(),
            json!(CODEX_OPENAI_IMAGE_INTERNAL_MODEL),
        );
        body_object.insert("stream".to_string(), json!(true));
        apply_codex_openai_image_tool_overrides(body_object);
        inject_codex_default_variation_prompt(body_object);
    }

    insert_codex_prompt_cache_key(provider_request_body, prompt_cache_key);
}

pub fn apply_codex_openai_responses_chat_body_edits(
    provider_request_body: &mut Value,
    provider_type: &str,
    provider_api_format: &str,
    body_rules: Option<&Value>,
    user_api_key_id: Option<&str>,
) {
    apply_codex_openai_responses_special_body_edits(
        provider_request_body,
        provider_type,
        provider_api_format,
        body_rules,
        user_api_key_id,
    );

    if !is_codex_openai_responses_request(provider_type, provider_api_format) {
        return;
    }
    let Some(body_object) = provider_request_body.as_object_mut() else {
        return;
    };
    ensure_codex_chat_reasoning_defaults(body_object, provider_api_format, body_rules);
    if let Some(prompt_cache_key) = body_object.remove("prompt_cache_key") {
        body_object.insert("prompt_cache_key".to_string(), prompt_cache_key);
    }
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
        apply_codex_openai_responses_chat_body_edits,
        apply_codex_openai_responses_special_body_edits,
        apply_openai_responses_compact_special_body_edits, CODEX_OPENAI_IMAGE_INTERNAL_MODEL,
    };
    use serde_json::json;

    #[test]
    fn codex_responses_body_edits_inject_passthrough_fields_without_reasoning_summary() {
        let mut provider_request_body = json!( {
            "input": [{
                "role": "user",
                "content": "hello"
            }],
            "model": "gpt-5.4",
            "stream": true
        });

        apply_codex_openai_responses_special_body_edits(
            &mut provider_request_body,
            "codex",
            "openai:responses",
            None,
            None,
        );

        assert!(provider_request_body.get("reasoning").is_none());
        assert_eq!(
            provider_request_body["include"],
            json!(["reasoning.encrypted_content"])
        );
        assert_eq!(provider_request_body["parallel_tool_calls"], json!(true));
        assert_eq!(provider_request_body["instructions"], json!(""));
    }

    #[test]
    fn codex_responses_body_edits_preserve_existing_include_and_parallel_tool_calls() {
        let mut provider_request_body = json!( {
            "input": [],
            "model": "gpt-5.4",
            "include": [
                "file_search_call.results",
                "web_search_call.results",
                "web_search_call.action.sources",
                "message.input_image.image_url",
                "computer_call_output.output.image_url",
                "code_interpreter_call.outputs",
                "message.output_text.logprobs"
            ],
            "reasoning": {"effort": "high", "summary": "detailed"},
            "parallel_tool_calls": false
        });

        apply_codex_openai_responses_special_body_edits(
            &mut provider_request_body,
            "codex",
            "openai:responses",
            None,
            None,
        );

        assert_eq!(provider_request_body["reasoning"]["effort"], json!("high"));
        assert_eq!(
            provider_request_body["reasoning"]["summary"],
            json!("detailed")
        );
        assert_eq!(
            provider_request_body["include"],
            json!([
                "file_search_call.results",
                "web_search_call.results",
                "web_search_call.action.sources",
                "message.input_image.image_url",
                "computer_call_output.output.image_url",
                "code_interpreter_call.outputs",
                "message.output_text.logprobs",
                "reasoning.encrypted_content"
            ])
        );
        assert_eq!(provider_request_body["parallel_tool_calls"], json!(false));
    }

    #[test]
    fn codex_responses_body_edits_preserve_function_tools_for_codex_backend() {
        let mut provider_request_body = json!({
            "input": [],
            "model": "gpt-5.4",
            "tools": [{
                "type": "function",
                "name": "lookup_account",
                "description": "Lookup an account by id.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "account_id": {
                            "type": "string"
                        }
                    },
                    "required": ["account_id"],
                    "additionalProperties": false
                },
                "strict": true
            }],
            "tool_choice": {
                "type": "function",
                "name": "lookup_account"
            }
        });

        apply_codex_openai_responses_special_body_edits(
            &mut provider_request_body,
            "codex",
            "openai:responses",
            None,
            None,
        );

        assert_eq!(
            provider_request_body["tools"][0]["name"],
            json!("lookup_account")
        );
        assert_eq!(
            provider_request_body["tools"][0]["parameters"]["properties"]["account_id"]["type"],
            json!("string")
        );
        assert_eq!(
            provider_request_body["tool_choice"]["name"],
            json!("lookup_account")
        );
        assert!(provider_request_body["tools"][0].get("function").is_none());
    }

    #[test]
    fn codex_responses_body_edits_strip_name_from_hosted_web_search_tool() {
        let mut provider_request_body = json!({
            "input": [],
            "model": "gpt-5.4",
            "tools": [{
                "type": "web_search",
                "name": "web_search"
            }],
            "tool_choice": {
                "type": "web_search",
                "name": "web_search"
            }
        });

        apply_codex_openai_responses_special_body_edits(
            &mut provider_request_body,
            "codex",
            "openai:responses",
            None,
            None,
        );

        assert!(provider_request_body["tools"][0].get("name").is_none());
        assert!(provider_request_body["tool_choice"].get("name").is_none());
        assert_eq!(
            provider_request_body["tool_choice"]["type"],
            json!("web_search")
        );
    }

    #[test]
    fn codex_responses_body_edits_derive_prompt_cache_key_from_session_metadata() {
        let mut body_a = json!({
            "input": [{"role": "user", "content": "hello"}],
            "model": "gpt-5.4",
            "metadata": {
                "user_id": "{\"session_id\":\"session-a\",\"device_id\":\"device-a\"}"
            }
        });
        let mut body_b = json!({
            "input": [{"role": "user", "content": "hello again"}],
            "model": "gpt-5.4",
            "metadata": {
                "user_id": "{\"session_id\":\"session-a\",\"device_id\":\"device-b\"}"
            }
        });
        let mut body_c = json!({
            "input": [{"role": "user", "content": "hello"}],
            "model": "gpt-5.4",
            "metadata": {"session_id": "session-b"}
        });

        apply_codex_openai_responses_special_body_edits(
            &mut body_a,
            "codex",
            "openai:responses",
            None,
            Some("key-123"),
        );
        apply_codex_openai_responses_special_body_edits(
            &mut body_b,
            "codex",
            "openai:responses",
            None,
            Some("different-key"),
        );
        apply_codex_openai_responses_special_body_edits(
            &mut body_c,
            "codex",
            "openai:responses",
            None,
            Some("key-123"),
        );

        assert_eq!(body_a["prompt_cache_key"], body_b["prompt_cache_key"]);
        assert_ne!(body_a["prompt_cache_key"], body_c["prompt_cache_key"]);
        assert!(body_a.get("metadata").is_none());
        assert!(body_b.get("metadata").is_none());
        assert!(body_c.get("metadata").is_none());
    }

    #[test]
    fn codex_responses_body_edits_derive_prompt_cache_key_from_cache_control_anchor() {
        let mut body_a = json!({
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{
                    "type": "input_text",
                    "text": "stable project brief",
                    "cache_control": {"type": "ephemeral"}
                }]
            }, {
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "new turn A"}]
            }],
            "model": "gpt-5.4"
        });
        let mut body_b = json!({
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{
                    "type": "input_text",
                    "text": "stable project brief",
                    "cache_control": {"type": "ephemeral"}
                }]
            }, {
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "new turn B"}]
            }],
            "model": "gpt-5.4"
        });
        let mut body_c = json!({
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{
                    "type": "input_text",
                    "text": "different project brief",
                    "cache_control": {"type": "ephemeral"}
                }]
            }],
            "model": "gpt-5.4"
        });

        apply_codex_openai_responses_special_body_edits(
            &mut body_a,
            "codex",
            "openai:responses",
            None,
            Some("key-a"),
        );
        apply_codex_openai_responses_special_body_edits(
            &mut body_b,
            "codex",
            "openai:responses",
            None,
            Some("key-b"),
        );
        apply_codex_openai_responses_special_body_edits(
            &mut body_c,
            "codex",
            "openai:responses",
            None,
            Some("key-a"),
        );

        assert_eq!(body_a["prompt_cache_key"], body_b["prompt_cache_key"]);
        assert_ne!(body_a["prompt_cache_key"], body_c["prompt_cache_key"]);
    }

    #[test]
    fn codex_responses_body_edits_derive_prompt_cache_key_from_stable_request_anchor() {
        let mut body_a = json!({
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "open workspace"}]
            }, {
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "new turn A"}]
            }],
            "model": "gpt-5.4",
            "instructions": "Be concise.",
            "tools": [{
                "type": "function",
                "name": "shell",
                "parameters": {"type": "object", "properties": {}}
            }],
            "reasoning": {"effort": "medium"}
        });
        let mut body_b = json!({
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "open workspace"}]
            }, {
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "new turn B"}]
            }],
            "model": "gpt-5.4",
            "instructions": "Be concise.",
            "tools": [{
                "type": "function",
                "name": "shell",
                "parameters": {"type": "object", "properties": {}}
            }],
            "reasoning": {"effort": "medium"}
        });
        let mut body_c = json!({
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "open another workspace"}]
            }],
            "model": "gpt-5.4",
            "instructions": "Be concise.",
            "tools": [{"type": "function", "name": "shell"}],
            "reasoning": {"effort": "medium"}
        });

        apply_codex_openai_responses_special_body_edits(
            &mut body_a,
            "codex",
            "openai:responses",
            None,
            Some("key-a"),
        );
        apply_codex_openai_responses_special_body_edits(
            &mut body_b,
            "codex",
            "openai:responses",
            None,
            Some("key-a"),
        );
        apply_codex_openai_responses_special_body_edits(
            &mut body_c,
            "codex",
            "openai:responses",
            None,
            Some("key-a"),
        );

        assert_eq!(body_a["prompt_cache_key"], body_b["prompt_cache_key"]);
        assert_ne!(body_a["prompt_cache_key"], body_c["prompt_cache_key"]);
    }

    #[test]
    fn compact_body_edits_strip_include_store_and_stream() {
        let mut provider_request_body = json!({
            "input": [],
            "model": "gpt-5.4",
            "include": ["reasoning.encrypted_content"],
            "store": true,
            "stream": true,
        });

        apply_openai_responses_compact_special_body_edits(
            &mut provider_request_body,
            "openai:responses:compact",
        );

        assert!(provider_request_body.get("include").is_none());
        assert!(provider_request_body.get("store").is_none());
        assert!(provider_request_body.get("stream").is_none());
        assert_eq!(provider_request_body["model"], json!("gpt-5.4"));
    }

    #[test]
    fn codex_chat_body_edits_inject_reasoning_summary_defaults() {
        let mut provider_request_body = json!({
            "input": [],
            "model": "gpt-5.4"
        });

        apply_codex_openai_responses_chat_body_edits(
            &mut provider_request_body,
            "codex",
            "openai:responses",
            None,
            None,
        );

        assert_eq!(
            provider_request_body["reasoning"]["effort"],
            json!("medium")
        );
        assert_eq!(provider_request_body["reasoning"]["summary"], json!("auto"));
        assert_eq!(
            provider_request_body["include"],
            json!(["reasoning.encrypted_content"])
        );
        assert_eq!(provider_request_body["parallel_tool_calls"], json!(true));
    }

    #[test]
    fn codex_chat_body_edits_preserve_existing_reasoning_effort() {
        let mut provider_request_body = json!({
            "input": [],
            "model": "gpt-5.4",
            "reasoning": {"effort": "low"}
        });

        apply_codex_openai_responses_chat_body_edits(
            &mut provider_request_body,
            "codex",
            "openai:responses",
            None,
            None,
        );

        assert_eq!(provider_request_body["reasoning"]["effort"], json!("low"));
        assert_eq!(provider_request_body["reasoning"]["summary"], json!("auto"));
    }

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
    fn codex_responses_image_tool_edits_force_internal_model_and_tool_defaults() {
        let mut provider_request_body = json!({
            "model": "gpt-image-2",
            "input": "generate image",
            "tools": [{
                "type": "image_generation"
            }],
            "tool_choice": {"type": "image_generation"}
        });

        apply_codex_openai_responses_special_body_edits(
            &mut provider_request_body,
            "codex",
            "openai:responses",
            None,
            None,
        );

        assert_eq!(
            provider_request_body["model"],
            json!(CODEX_OPENAI_IMAGE_INTERNAL_MODEL)
        );
        assert_eq!(provider_request_body["stream"], json!(true));
        assert_eq!(
            provider_request_body["tools"][0]["type"],
            json!("image_generation")
        );
        assert_eq!(
            provider_request_body["tool_choice"]["type"],
            json!("image_generation")
        );
    }

    #[test]
    fn codex_responses_image_tool_edits_triggered_by_string_tool_choice() {
        let mut provider_request_body = json!({
            "model": "gpt-image-2",
            "input": "generate image",
            "tools": [{"type": "image_generation"}],
            "tool_choice": "image_generation"
        });

        apply_codex_openai_responses_special_body_edits(
            &mut provider_request_body,
            "codex",
            "openai:responses",
            None,
            None,
        );

        assert_eq!(
            provider_request_body["model"],
            json!(CODEX_OPENAI_IMAGE_INTERNAL_MODEL)
        );
        assert_eq!(
            provider_request_body["tool_choice"]["type"],
            json!("image_generation")
        );
    }

    #[test]
    fn codex_responses_image_tool_edits_skipped_when_tool_choice_is_auto() {
        let original_model = "gpt-5.5";
        let mut provider_request_body = json!({
            "model": original_model,
            "input": [{"role": "user", "content": "hi"}],
            "tools": [
                {"type": "function", "name": "shell"},
                {"type": "image_generation"},
                {"type": "web_search"}
            ],
            "tool_choice": "auto"
        });

        apply_codex_openai_responses_special_body_edits(
            &mut provider_request_body,
            "codex",
            "openai:responses",
            None,
            None,
        );

        assert_eq!(provider_request_body["model"], json!(original_model));
        assert_eq!(provider_request_body["tool_choice"], json!("auto"));
        assert_eq!(
            provider_request_body["tools"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default(),
            3,
            "tools array should be preserved when tool_choice is auto"
        );
    }

    #[test]
    fn codex_responses_image_tool_edits_skipped_when_tool_choice_absent() {
        let original_model = "gpt-5.5";
        let mut provider_request_body = json!({
            "model": original_model,
            "input": [{"role": "user", "content": "hi"}],
            "tools": [{"type": "image_generation"}]
        });

        apply_codex_openai_responses_special_body_edits(
            &mut provider_request_body,
            "codex",
            "openai:responses",
            None,
            None,
        );

        assert_eq!(provider_request_body["model"], json!(original_model));
        assert!(
            provider_request_body.get("tool_choice").is_none(),
            "tool_choice should not be injected when caller did not set it"
        );
        assert_eq!(
            provider_request_body["tools"][0]["type"],
            json!("image_generation")
        );
        assert_eq!(
            provider_request_body["tools"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default(),
            1,
            "tools array should be preserved verbatim when tool_choice is absent"
        );
    }

    #[test]
    fn codex_responses_image_tool_edits_skipped_when_tool_choice_targets_other_tool() {
        let original_model = "gpt-5.5";
        let mut provider_request_body = json!({
            "model": original_model,
            "input": [{"role": "user", "content": "hi"}],
            "tools": [
                {"type": "function", "name": "shell"},
                {"type": "image_generation"}
            ],
            "tool_choice": {"type": "function", "name": "shell"}
        });

        apply_codex_openai_responses_special_body_edits(
            &mut provider_request_body,
            "codex",
            "openai:responses",
            None,
            None,
        );

        assert_eq!(provider_request_body["model"], json!(original_model));
        assert_eq!(
            provider_request_body["tool_choice"],
            json!({"type": "function", "name": "shell"})
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
