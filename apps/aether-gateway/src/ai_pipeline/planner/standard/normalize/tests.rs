use aether_provider_transport::snapshot::{
    GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
    GatewayProviderTransportProvider, GatewayProviderTransportSnapshot,
};
use http::Request;
use serde_json::{json, Value};

use super::{
    build_cross_format_openai_responses_request_body, build_local_openai_responses_request_body,
    build_local_openai_responses_upstream_url,
};

fn object_keys(value: &Value) -> Vec<&str> {
    value
        .as_object()
        .expect("json object")
        .keys()
        .map(String::as_str)
        .collect()
}

fn sample_transport(base_url: &str, api_format: &str) -> GatewayProviderTransportSnapshot {
    GatewayProviderTransportSnapshot {
        provider: GatewayProviderTransportProvider {
            id: "provider-codex".to_string(),
            name: "codex".to_string(),
            provider_type: "codex".to_string(),
            website: None,
            is_active: true,
            keep_priority_on_conversion: false,
            enable_format_conversion: false,
            concurrent_limit: None,
            max_retries: None,
            proxy: None,
            request_timeout_secs: None,
            stream_first_byte_timeout_secs: None,
            config: None,
        },
        endpoint: GatewayProviderTransportEndpoint {
            id: "endpoint-codex".to_string(),
            provider_id: "provider-codex".to_string(),
            api_format: api_format.to_string(),
            api_family: Some("openai".to_string()),
            endpoint_kind: Some("cli".to_string()),
            is_active: true,
            base_url: base_url.to_string(),
            header_rules: None,
            body_rules: None,
            max_retries: None,
            custom_path: None,
            config: None,
            format_acceptance_config: None,
            proxy: None,
        },
        key: GatewayProviderTransportKey {
            id: "key-codex".to_string(),
            provider_id: "provider-codex".to_string(),
            name: "oauth".to_string(),
            auth_type: "oauth".to_string(),
            is_active: true,
            api_formats: Some(vec![api_format.to_string()]),
            allowed_models: None,
            capabilities: None,
            rate_multipliers: None,
            global_priority_by_format: None,
            expires_at_unix_secs: None,
            proxy: None,
            fingerprint: None,
            decrypted_api_key: "__placeholder__".to_string(),
            decrypted_auth_config: None,
        },
    }
}

#[test]
fn builds_openai_chat_cross_format_request_body_from_openai_responses_source() {
    let body_json = json!({
        "model": "gpt-5",
        "input": "hello",
    });

    let provider_request_body = build_cross_format_openai_responses_request_body(
        &body_json,
        "gpt-5-upstream",
        "openai:responses",
        "openai:chat",
        false,
        "openai",
        None,
        None,
    )
    .expect("openai responses to openai chat body should build");

    assert_eq!(provider_request_body["model"], "gpt-5-upstream");
    assert_eq!(provider_request_body["messages"][0]["role"], "user");
    assert_eq!(provider_request_body["messages"][0]["content"], "hello");
}

#[test]
fn local_openai_responses_wrapper_preserves_body_order_after_edits() {
    let body_json: Value = serde_json::from_str(
        r#"{
            "text": {"format": {"type": "text"}},
            "input": [],
            "model": "gpt-5.4",
            "store": false,
            "tools": [],
            "stream": true,
            "include": ["reasoning.encrypted_content"],
            "reasoning": {"effort": "high"},
            "tool_choice": "auto"
        }"#,
    )
    .expect("request body should parse");

    let provider_request_body = build_local_openai_responses_request_body(
        &body_json,
        "gpt-5.4",
        true,
        "codex",
        "openai:responses",
        None,
        Some("key-123"),
    )
    .expect("local openai responses body should build");

    assert_eq!(
        object_keys(&provider_request_body),
        vec![
            "text",
            "input",
            "model",
            "store",
            "tools",
            "stream",
            "include",
            "reasoning",
            "tool_choice",
            "instructions",
            "prompt_cache_key",
        ]
    );
}

#[test]
fn local_openai_responses_compact_wrapper_strips_store_for_same_format_requests() {
    let body_json = json!({
        "model": "gpt-5.4",
        "input": [],
        "store": true
    });

    let provider_request_body = build_local_openai_responses_request_body(
        &body_json,
        "gpt-5.4",
        false,
        "openai",
        "openai:responses:compact",
        None,
        None,
    )
    .expect("local openai compact body should build");

    assert!(provider_request_body.get("store").is_none());
}

#[test]
fn local_openai_responses_upstream_url_preserves_codex_base_path() {
    let request = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .body(())
        .expect("request should build");
    let (parts, _) = request.into_parts();

    let upstream_url = build_local_openai_responses_upstream_url(
        &parts,
        &sample_transport("https://tiger.bookapi.cc/codex", "openai:responses"),
        false,
    )
    .expect("openai responses upstream url should build");

    assert_eq!(upstream_url, "https://tiger.bookapi.cc/codex/responses");
}

#[test]
fn strips_metadata_for_codex_openai_responses_requests() {
    let body_json = json!({
        "model": "claude-sonnet-4-5",
        "metadata": {"trace_id": "abc"},
        "messages": [{
            "role": "user",
            "content": [{"type": "text", "text": "hello"}]
        }],
    });

    let provider_request_body = build_cross_format_openai_responses_request_body(
        &body_json,
        "gpt-5-upstream",
        "claude:cli",
        "openai:responses",
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

    let provider_request_body = build_cross_format_openai_responses_request_body(
        &body_json,
        "gpt-5-upstream",
        "claude:cli",
        "openai:responses",
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
fn injects_codex_prompt_cache_key_for_openai_responses_cross_format_requests() {
    let body_json = json!({
        "model": "claude-sonnet-4-5",
        "messages": [{
            "role": "user",
            "content": [{"type": "text", "text": "hello"}]
        }],
    });

    let provider_request_body = build_cross_format_openai_responses_request_body(
        &body_json,
        "gpt-5-upstream",
        "claude:cli",
        "openai:responses",
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
        "openai:responses",
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
