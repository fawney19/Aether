use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};

use super::super::auth::build_openai_passthrough_headers;
use super::fingerprint::header_fingerprint_from_fingerprint;

const DEFAULT_ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_ACCEPT: &str = "application/json";
const STREAM_HELPER_METHOD: &str = "stream";
const DUMMY_THINKING_SIGNATURE: &str = "skip_thought_signature_validator";
const REQUIRED_BETA_TOKENS: &[&str] = &[
    "claude-code-20250219",
    "oauth-2025-04-20",
    "interleaved-thinking-2025-05-14",
];
const EXCLUDED_BETA_TOKENS: &[&str] = &["context-1m-2025-08-07"];

/// Fingerprint field -> HTTP header mapping.
/// Every stainless / identity dimension that can vary per-key is listed here.
const FINGERPRINT_HEADER_MAP: &[(&str, &str)] = &[
    ("stainless_package_version", "x-stainless-package-version"),
    ("stainless_os", "x-stainless-os"),
    ("stainless_arch", "x-stainless-arch"),
    ("stainless_runtime_version", "x-stainless-runtime-version"),
    ("stainless_timeout", "x-stainless-timeout"),
    ("user_agent", "user-agent"),
];

pub fn build_claude_code_passthrough_headers(
    headers: &http::HeaderMap,
    auth_header: &str,
    auth_value: &str,
    extra_headers: &BTreeMap<String, String>,
    stream: bool,
    fingerprint: Option<&Value>,
) -> BTreeMap<String, String> {
    let mut out = build_openai_passthrough_headers(
        headers,
        auth_header,
        auth_value,
        extra_headers,
        Some("application/json"),
    );

    // -- Anthropic protocol headers --
    out.insert("accept".to_string(), DEFAULT_ACCEPT.to_string());
    out.insert(
        "anthropic-version".to_string(),
        DEFAULT_ANTHROPIC_VERSION.to_string(),
    );
    // Read incoming anthropic-beta directly from the original HeaderMap because the
    // upstream passthrough filter now strips `anthropic-*` headers to avoid leaking
    // them to non-Anthropic upstreams.
    let incoming_anthropic_beta = headers
        .get("anthropic-beta")
        .and_then(|value| value.to_str().ok());
    out.insert(
        "anthropic-beta".to_string(),
        merge_anthropic_beta_tokens(incoming_anthropic_beta),
    );
    out.insert(
        "anthropic-dangerous-direct-browser-access".to_string(),
        "true".to_string(),
    );
    out.insert("x-app".to_string(), "cli".to_string());

    // -- Stainless SDK identity headers --
    // Fixed values: these don't vary per fingerprint.
    out.insert("x-stainless-lang".to_string(), "js".to_string());
    out.insert("x-stainless-runtime".to_string(), "node".to_string());
    out.insert("x-stainless-retry-count".to_string(), "0".to_string());

    // Defaults for fingerprint-overridable fields (used when no fingerprint is present).
    out.insert(
        "x-stainless-package-version".to_string(),
        "0.70.0".to_string(),
    );
    out.insert("x-stainless-os".to_string(), "Linux".to_string());
    out.insert("x-stainless-arch".to_string(), "arm64".to_string());
    out.insert(
        "x-stainless-runtime-version".to_string(),
        "v24.13.0".to_string(),
    );
    out.insert("x-stainless-timeout".to_string(), "600".to_string());

    if stream {
        out.insert(
            "x-stainless-helper-method".to_string(),
            STREAM_HELPER_METHOD.to_string(),
        );
    } else {
        out.remove("x-stainless-helper-method");
    }

    // Override from the formal transport profile header fingerprint.
    if let Some(fp) = fingerprint.and_then(header_fingerprint_from_fingerprint) {
        for &(fp_key, header_key) in FINGERPRINT_HEADER_MAP {
            if let Some(value) = fp
                .get(fp_key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|v| !v.is_empty())
            {
                out.insert(header_key.to_string(), value.to_string());
            }
        }
    }

    out
}

pub fn sanitize_claude_code_request_body(body: &mut Value) {
    let Some(body_object) = body.as_object_mut() else {
        return;
    };
    let thinking_enabled = body_object
        .get("thinking")
        .and_then(Value::as_object)
        .and_then(|thinking| thinking.get("type"))
        .and_then(Value::as_str)
        .map(str::trim)
        .is_some_and(|value| matches!(value.to_ascii_lowercase().as_str(), "enabled" | "adaptive"));

    let Some(messages) = body_object
        .get_mut("messages")
        .and_then(Value::as_array_mut)
    else {
        return;
    };

    for message in messages {
        let Some(message_object) = message.as_object_mut() else {
            continue;
        };
        let role = message_object
            .get("role")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default()
            .to_string();
        let Some(content) = message_object
            .get_mut("content")
            .and_then(Value::as_array_mut)
        else {
            continue;
        };

        let mut filtered = Vec::with_capacity(content.len());
        for block in std::mem::take(content) {
            let Value::Object(block_object) = block else {
                filtered.push(block);
                continue;
            };
            if keep_claude_code_block(&block_object, &role, thinking_enabled) {
                filtered.push(Value::Object(block_object));
            }
        }
        *content = filtered;
    }
}

fn keep_claude_code_block(
    block_object: &Map<String, Value>,
    role: &str,
    thinking_enabled: bool,
) -> bool {
    let block_type = block_object
        .get("type")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if matches!(block_type, "thinking" | "redacted_thinking") {
        let signature = block_object
            .get("signature")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default();
        return thinking_enabled
            && role.eq_ignore_ascii_case("assistant")
            && !signature.is_empty()
            && signature != DUMMY_THINKING_SIGNATURE;
    }
    if block_type.is_empty() && block_object.contains_key("thinking") {
        return false;
    }
    true
}

fn merge_anthropic_beta_tokens(incoming: Option<&str>) -> String {
    let mut seen = BTreeSet::new();
    let mut merged = Vec::new();

    for token in REQUIRED_BETA_TOKENS {
        append_beta_token(&mut seen, &mut merged, token);
    }
    for token in incoming.unwrap_or_default().split(',') {
        let token = token.trim();
        if EXCLUDED_BETA_TOKENS
            .iter()
            .any(|excluded| token.eq_ignore_ascii_case(excluded))
        {
            continue;
        }
        append_beta_token(&mut seen, &mut merged, token);
    }

    merged.join(",")
}

fn append_beta_token(seen: &mut BTreeSet<String>, merged: &mut Vec<String>, token: &str) {
    let normalized = token.trim();
    if normalized.is_empty() {
        return;
    }
    let key = normalized.to_ascii_lowercase();
    if seen.insert(key) {
        merged.push(normalized.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::{build_claude_code_passthrough_headers, sanitize_claude_code_request_body};
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    fn claude_code_headers_use_transport_profile_header_fingerprint_and_merge_required_betas() {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            "anthropic-beta",
            http::HeaderValue::from_static("context-1m-2025-08-07,custom-beta"),
        );
        headers.insert(
            "user-agent",
            http::HeaderValue::from_static("Claude-Code/Test"),
        );
        let built = build_claude_code_passthrough_headers(
            &headers,
            "authorization",
            "Bearer upstream-token",
            &BTreeMap::new(),
            true,
            Some(&json!({
                "transport_profile": {
                    "profile_id": "claude_code_nodejs",
                    "header_fingerprint": {
                        "user_agent":"Claude-Code/9.9",
                        "stainless_package_version":"1.0.5",
                        "stainless_runtime_version":"v22.12.0",
                        "stainless_timeout":"900"
                    }
                }
            })),
        );

        assert_eq!(
            built.get("anthropic-beta").map(String::as_str),
            Some(
                "claude-code-20250219,oauth-2025-04-20,interleaved-thinking-2025-05-14,custom-beta"
            )
        );
        assert_eq!(
            built.get("anthropic-version").map(String::as_str),
            Some("2023-06-01")
        );
        assert_eq!(
            built.get("accept").map(String::as_str),
            Some("application/json")
        );
        assert_eq!(
            built.get("x-stainless-helper-method").map(String::as_str),
            Some("stream")
        );
        assert_eq!(built.get("x-app").map(String::as_str), Some("cli"));
        assert_eq!(
            built.get("x-stainless-package-version").map(String::as_str),
            Some("1.0.5")
        );
        assert_eq!(
            built.get("x-stainless-runtime-version").map(String::as_str),
            Some("v22.12.0")
        );
        assert_eq!(
            built.get("x-stainless-timeout").map(String::as_str),
            Some("900")
        );
        assert_eq!(
            built.get("user-agent").map(String::as_str),
            Some("Claude-Code/9.9")
        );
        assert_eq!(
            built.get("authorization").map(String::as_str),
            Some("Bearer upstream-token")
        );
    }

    #[test]
    fn claude_code_body_sanitizer_drops_invalid_thinking_blocks() {
        let mut body = json!({
            "thinking": {"type":"enabled"},
            "messages": [{
                "role":"assistant",
                "content":[
                    {"type":"thinking","thinking":"keep","signature":"sig_valid"},
                    {"type":"thinking","thinking":"drop-empty","signature":""},
                    {"type":"redacted_thinking","data":"keep-redacted","signature":"sig_redacted"},
                    {"type":"redacted_thinking","data":"drop-no-signature"},
                    {"thinking":"drop-no-type"},
                    {"type":"text","text":"ok"}
                ]
            }]
        });

        sanitize_claude_code_request_body(&mut body);

        assert_eq!(
            body["messages"][0]["content"],
            json!([
                {"type":"thinking","thinking":"keep","signature":"sig_valid"},
                {"type":"redacted_thinking","data":"keep-redacted","signature":"sig_redacted"},
                {"type":"text","text":"ok"}
            ])
        );
    }
}
