use std::collections::BTreeMap;
use std::sync::OnceLock;

use regex::Regex;
use serde_json::{Map, Value};

use super::super::auth::build_openai_passthrough_headers;
use super::profile::{
    current_claude_code_transport_identity_profile, ClaudeCodeTransportIdentityProfile,
};

const DUMMY_THINKING_SIGNATURE: &str = "skip_thought_signature_validator";

pub fn build_claude_code_passthrough_headers(
    headers: &http::HeaderMap,
    auth_header: &str,
    auth_value: &str,
    extra_headers: &BTreeMap<String, String>,
    stream: bool,
) -> BTreeMap<String, String> {
    let mut out = build_openai_passthrough_headers(
        headers,
        auth_header,
        auth_value,
        extra_headers,
        Some("application/json"),
    );

    // The common passthrough filter intentionally strips Anthropic identity
    // headers. Restore only the client beta input; the versioned profile owns
    // all fixed identity values and the final beta policy.
    let mut incoming_beta_values = headers
        .get("anthropic-beta")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .into_iter()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    incoming_beta_values.extend(
        extra_headers
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case("anthropic-beta"))
            .map(|(_, value)| value.trim())
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
    );
    if !incoming_beta_values.is_empty() {
        out.insert("anthropic-beta".to_string(), incoming_beta_values.join(","));
    }

    let profile = current_claude_code_transport_identity_profile();
    profile.apply_fixed_headers(&mut out, stream);
    profile.apply_beta_policy(&mut out, None);

    out
}

pub fn sanitize_claude_code_request_body(body: &mut Value) {
    let profile = current_claude_code_transport_identity_profile();
    let beta_header = profile.merge_beta_tokens(None, None);
    sanitize_claude_code_request_body_for_beta_header(body, &beta_header, &profile);
}

pub fn sanitize_claude_code_request_body_for_beta_header(
    body: &mut Value,
    beta_header: &str,
    profile: &ClaudeCodeTransportIdentityProfile,
) {
    let Some(body_object) = body.as_object_mut() else {
        return;
    };

    synchronize_billing_header_version(body_object, profile.billing_cli_version());
    for gate in profile.body_capability_gates() {
        if !profile.beta_header_enables_body_field(beta_header, gate.body_field) {
            body_object.remove(gate.body_field);
        }
    }

    let thinking_enabled = body_object
        .get("thinking")
        .and_then(Value::as_object)
        .and_then(|thinking| thinking.get("type"))
        .and_then(Value::as_str)
        .map(str::trim)
        .is_some_and(|value| matches!(value.to_ascii_lowercase().as_str(), "enabled" | "adaptive"));

    if let Some(gate) = profile.body_capability_gate("context_management") {
        if thinking_enabled
            && gate.inject_when_thinking_enabled
            && profile.beta_header_enables_body_field(beta_header, gate.body_field)
            && !body_object.contains_key(gate.body_field)
        {
            if let Some(default_edit_type) = gate.default_edit_type {
                body_object.insert(
                    gate.body_field.to_string(),
                    serde_json::json!({
                        "edits": [{
                            "type": default_edit_type,
                            "keep": "all"
                        }]
                    }),
                );
            }
        }
    }

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

fn synchronize_billing_header_version(body: &mut Map<String, Value>, cli_version: &str) {
    static CC_VERSION: OnceLock<Regex> = OnceLock::new();
    let Some(system) = body.get_mut("system").and_then(Value::as_array_mut) else {
        return;
    };
    let regex = CC_VERSION.get_or_init(|| {
        Regex::new(r"cc_version=\d+\.\d+\.\d+").expect("billing version regex must compile")
    });
    let replacement = format!("cc_version={cli_version}");

    for block in system {
        let Some(block) = block.as_object_mut() else {
            continue;
        };
        let Some(text) = block
            .get("text")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
        else {
            continue;
        };
        if !text.starts_with("x-anthropic-billing-header") {
            continue;
        }
        let updated = regex.replace_all(&text, replacement.as_str());
        if updated != text {
            block.insert("text".to_string(), Value::String(updated.into_owned()));
        }
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

#[cfg(test)]
mod tests {
    use super::{
        build_claude_code_passthrough_headers, sanitize_claude_code_request_body,
        sanitize_claude_code_request_body_for_beta_header,
    };
    use crate::claude_code::current_claude_code_transport_identity_profile;
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    fn claude_code_headers_use_versioned_identity_and_merge_preserved_betas() {
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
        );

        assert_eq!(
            built.get("anthropic-beta").map(String::as_str),
            Some(
                "claude-code-20250219,oauth-2025-04-20,interleaved-thinking-2025-05-14,prompt-caching-scope-2026-01-05,effort-2025-11-24,context-management-2025-06-27,extended-cache-ttl-2025-04-11,context-1m-2025-08-07,custom-beta"
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
            Some("0.112.1")
        );
        assert_eq!(
            built.get("x-stainless-runtime-version").map(String::as_str),
            Some("v26.3.0")
        );
        assert_eq!(
            built.get("x-stainless-timeout").map(String::as_str),
            Some("600")
        );
        assert_eq!(
            built.get("user-agent").map(String::as_str),
            Some("claude-cli/2.1.284 (external, cli)")
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

    #[test]
    fn context_management_body_is_gated_by_the_matching_beta_token() {
        let profile = current_claude_code_transport_identity_profile();
        let original = json!({
            "context_management": {
                "edits": [{"type":"clear_thinking_20251015", "keep":"all"}]
            },
            "messages": []
        });

        let mut without_beta = original.clone();
        sanitize_claude_code_request_body_for_beta_header(
            &mut without_beta,
            "oauth-2025-04-20",
            &profile,
        );
        assert!(without_beta.get("context_management").is_none());

        let mut with_beta = original.clone();
        sanitize_claude_code_request_body_for_beta_header(
            &mut with_beta,
            "oauth-2025-04-20, context-management-2025-06-27",
            &profile,
        );
        assert_eq!(with_beta, original);
    }

    #[test]
    fn default_profile_keeps_header_and_injected_context_management_in_sync() {
        let headers = build_claude_code_passthrough_headers(
            &http::HeaderMap::new(),
            "authorization",
            "Bearer upstream-token",
            &BTreeMap::new(),
            false,
        );
        let mut body = json!({
            "thinking": {"type":"adaptive"},
            "messages": []
        });

        sanitize_claude_code_request_body(&mut body);

        assert!(headers["anthropic-beta"]
            .split(',')
            .any(|token| token == "context-management-2025-06-27"));
        assert_eq!(
            body["context_management"],
            json!({"edits":[{"type":"clear_thinking_20251015", "keep":"all"}]})
        );
    }

    #[test]
    fn billing_attribution_uses_the_profile_cli_version() {
        let mut body = json!({
            "system": [{
                "type":"text",
                "text":"x-anthropic-billing-header: cc_version=2.0.0.abc; cc_entrypoint=cli;"
            }],
            "messages": []
        });

        sanitize_claude_code_request_body(&mut body);

        assert_eq!(
            body["system"][0]["text"],
            "x-anthropic-billing-header: cc_version=2.1.284.abc; cc_entrypoint=cli;"
        );
    }
}
