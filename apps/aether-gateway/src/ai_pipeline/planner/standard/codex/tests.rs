use std::collections::BTreeMap;

use super::{apply_codex_openai_cli_special_body_edits, apply_codex_openai_cli_special_headers};
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
fn strips_store_for_compact_even_when_body_rules_handle_it() {
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
    assert!(body.get("store").is_none());
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
        headers.get("user-agent"),
        Some(&"codex-tui/0.122.0 (Aether; x86_64) vscode/3.0.12 (codex-tui; 0.122.0)".to_string())
    );
    assert_eq!(headers.get("version"), Some(&"0.122.0".to_string()));
    assert_eq!(headers.get("originator"), Some(&"codex_cli_rs".to_string()));
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
    original_headers.insert(
        "user-agent",
        HeaderValue::from_static("user-specified-agent"),
    );
    original_headers.insert(
        "version",
        HeaderValue::from_static("user-specified-version"),
    );
    original_headers.insert(
        "originator",
        HeaderValue::from_static("user-specified-originator"),
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
    assert!(!headers.contains_key("user-agent"));
    assert!(!headers.contains_key("version"));
    assert!(!headers.contains_key("originator"));
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
        headers.get("user-agent"),
        Some(&"codex-tui/0.122.0 (Aether; x86_64) vscode/3.0.12 (codex-tui; 0.122.0)".to_string())
    );
    assert_eq!(headers.get("version"), Some(&"0.122.0".to_string()));
    assert_eq!(headers.get("originator"), Some(&"codex_cli_rs".to_string()));
    assert_eq!(
        headers.get("session_id"),
        Some(&"ab5ecce4f0d110fe".to_string())
    );
    assert!(!headers.contains_key("conversation_id"));
}
