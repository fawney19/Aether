use serde_json::json;

use super::build_cross_format_openai_cli_request_body;

#[test]
fn builds_openai_family_cross_format_request_body_from_compact_source() {
    let body_json = json!({
        "model": "gpt-5",
        "input": "hello",
    });

    let provider_request_body = build_cross_format_openai_cli_request_body(
        &body_json,
        "gpt-5-upstream",
        "openai:compact",
        "openai:cli",
        false,
        "openai",
        None,
        None,
    )
    .expect("compact to openai cli body should build");

    assert_eq!(provider_request_body["model"], "gpt-5-upstream");
    assert_eq!(provider_request_body["input"][0]["type"], "message");
    assert_eq!(provider_request_body["input"][0]["role"], "user");
}

#[test]
fn strips_metadata_for_codex_openai_cli_requests() {
    let body_json = json!({
        "model": "claude-sonnet-4-5",
        "metadata": {"trace_id": "abc"},
        "messages": [{
            "role": "user",
            "content": [{"type": "text", "text": "hello"}]
        }],
    });

    let provider_request_body = build_cross_format_openai_cli_request_body(
        &body_json,
        "gpt-5-upstream",
        "claude:cli",
        "openai:cli",
        true,
        "codex",
        None,
        None,
    )
    .expect("claude cli to codex request should build");

    assert!(provider_request_body.get("metadata").is_none());
}

#[test]
fn applies_codex_defaults_unless_body_rules_handle_the_field() {
    let body_json = json!({
        "model": "claude-sonnet-4-5",
        "messages": [{
            "role": "user",
            "content": [{"type": "text", "text": "hello"}]
        }],
        "metadata": {"trace_id": "abc"},
        "store": true
    });
    let body_rules = json!([
        {"action":"set","path":"store","value":true},
        {"action":"set","path":"instructions","value":"Custom instructions"},
        {"action":"set","path":"metadata","value":{"trace_id":"keep-me"}}
    ]);

    let provider_request_body = build_cross_format_openai_cli_request_body(
        &body_json,
        "gpt-5-upstream",
        "claude:cli",
        "openai:cli",
        true,
        "codex",
        Some(&body_rules),
        None,
    )
    .expect("claude cli to codex request should build");

    assert_eq!(provider_request_body["store"], true);
    assert_eq!(provider_request_body["instructions"], "Custom instructions");
    assert_eq!(provider_request_body["metadata"]["trace_id"], "keep-me");
}

#[test]
fn injects_codex_prompt_cache_key_for_openai_cli_cross_format_requests() {
    let body_json = json!({
        "model": "claude-sonnet-4-5",
        "messages": [{
            "role": "user",
            "content": [{"type": "text", "text": "hello"}]
        }],
    });

    let provider_request_body = build_cross_format_openai_cli_request_body(
        &body_json,
        "gpt-5-upstream",
        "claude:cli",
        "openai:cli",
        true,
        "codex",
        None,
        Some("key-123"),
    )
    .expect("claude cli to codex request should build");

    assert_eq!(
        provider_request_body["prompt_cache_key"],
        "172c39e6-c0a0-5a70-8b63-e0f8e0d185a3"
    );
}

#[test]
fn injects_codex_prompt_cache_key_for_openai_chat_cross_format_requests() {
    let body_json = json!({
        "model": "gpt-5",
        "messages": [{
            "role": "user",
            "content": "hello"
        }],
    });

    let provider_request_body = super::build_cross_format_openai_chat_request_body(
        &body_json,
        "gpt-5-upstream",
        "codex",
        "openai:cli",
        false,
        None,
        Some("key-123"),
    )
    .expect("openai chat to codex request should build");

    assert_eq!(
        provider_request_body["prompt_cache_key"],
        "172c39e6-c0a0-5a70-8b63-e0f8e0d185a3"
    );
}
