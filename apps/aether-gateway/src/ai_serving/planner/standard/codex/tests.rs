use std::collections::BTreeMap;

use super::{
    apply_codex_openai_responses_identity_headers, apply_codex_openai_responses_special_body_edits,
    apply_codex_openai_special_headers, codex_model_capabilities,
};
use crate::ai_serving::planner::standard::{
    build_cross_format_openai_responses_request_body, build_local_openai_responses_request_body,
};
use http::{HeaderMap, HeaderValue};
use serde_json::json;

#[test]
fn search_uses_live_codex_model_catalog_capabilities() {
    let metadata = crate::ai_serving::build_codex_model_catalog_metadata(&[json!({
        "slug": "gpt-search-custom",
        "default_reasoning_level": "low",
        "supported_reasoning_levels": [
            {"effort": "low"},
            {"effort": "max"}
        ],
        "supports_parallel_tool_calls": true
    })]);

    let capabilities = codex_model_capabilities(
        "codex",
        "openai:search",
        "gpt-search-custom",
        "gpt-search-custom",
        Some(&metadata),
    )
    .expect("Search should resolve capabilities from the Codex model catalog");

    assert_eq!(
        capabilities.default_reasoning_effort.as_deref(),
        Some("low")
    );
    assert_eq!(
        capabilities.supported_reasoning_efforts,
        vec!["low".to_string(), "max".to_string()]
    );
    assert!(codex_model_capabilities(
        "codex",
        "openai:chat",
        "gpt-search-custom",
        "gpt-search-custom",
        Some(&metadata),
    )
    .is_none());
}

#[test]
fn applies_codex_defaults_when_body_rules_do_not_handle_fields() {
    let mut body = json!({
        "model": "gpt-5.4",
        "max_output_tokens": 128,
        "temperature": 0.3,
        "top_p": 0.9,
        "metadata": {"client": "desktop"},
        "store": true
    });

    apply_codex_openai_responses_special_body_edits(
        &mut body,
        "codex",
        "openai:responses",
        None,
        None,
    );

    assert!(body.get("max_output_tokens").is_none());
    assert!(body.get("temperature").is_none());
    assert!(body.get("top_p").is_none());
    assert!(body.get("metadata").is_none());
    assert_eq!(body["store"], false);
    assert!(body.get("instructions").is_none());
    assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
    assert_eq!(body["parallel_tool_calls"], true);
    assert_eq!(body["reasoning"]["effort"], "medium");
    assert!(body["reasoning"].get("summary").is_none());
}

#[test]
fn local_openai_responses_codex_body_wraps_string_input_for_backend() {
    let body = json!({
        "model": "gpt-5",
        "input": "hello"
    });

    let provider_request_body = build_local_openai_responses_request_body(
        &body,
        "gpt-5-upstream",
        false,
        false,
        "codex",
        "openai:responses",
        None,
        Some("key-123"),
        &HeaderMap::new(),
        false,
    )
    .expect("codex local openai responses body should build");

    assert_eq!(
        provider_request_body["input"],
        json!([{
            "type": "message",
            "role": "user",
            "content": [{
                "type": "input_text",
                "text": "hello"
            }]
        }])
    );
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
        "model": "gpt-5.4",
        "max_output_tokens": 128,
        "metadata": {"client": "desktop", "mode": "custom"},
        "store": true,
        "instructions": "Keep custom",
        "top_p": 0.5
    });

    apply_codex_openai_responses_special_body_edits(
        &mut body,
        "codex",
        "openai:responses:compact",
        Some(&body_rules),
        None,
    );

    assert!(body.get("max_output_tokens").is_none());
    assert!(body.get("store").is_none());
    assert_eq!(body["instructions"], "Keep custom");
    assert!(body.get("metadata").is_none());
    assert!(body.get("top_p").is_none());
    assert_eq!(body["parallel_tool_calls"], true);
    assert!(body.as_object().is_some_and(|object| {
        object.keys().all(|field| {
            matches!(
                field.as_str(),
                "model"
                    | "input"
                    | "instructions"
                    | "tools"
                    | "parallel_tool_calls"
                    | "reasoning"
                    | "service_tier"
                    | "prompt_cache_key"
                    | "text"
            )
        })
    }));
}

#[test]
fn does_not_synthesize_prompt_cache_key_from_api_key_identity() {
    let mut body = json!({
        "model": "gpt-5",
        "input": "hello",
    });

    apply_codex_openai_responses_special_body_edits(
        &mut body,
        "codex",
        "openai:responses",
        None,
        Some("key-123"),
    );

    assert!(body.get("prompt_cache_key").is_none());
}

#[test]
fn adapts_generic_prompt_cache_key_to_codex_native_identity() {
    let mut body = json!({
        "model": "gpt-5",
        "input": "hello",
        "prompt_cache_key": "ltm-pc-v2-5557e02f5c9b447a97673ba330dbe77a",
    });

    apply_codex_openai_responses_special_body_edits(
        &mut body,
        "codex",
        "openai:responses",
        None,
        Some("key-123"),
    );

    let expected_identity = "d9c5d122-7c1c-5fb1-ba9d-656062eda44e";
    assert_eq!(body["prompt_cache_key"], expected_identity);
    assert_eq!(body["client_metadata"]["session_id"], expected_identity);
    assert_eq!(body["client_metadata"]["thread_id"], expected_identity);
}

#[test]
fn preserves_native_codex_cache_identity_and_metadata() {
    let mut body = json!({
        "model": "gpt-5",
        "input": "hello",
        "prompt_cache_key": "guardian:parent-thread",
        "client_metadata": {
            "session_id": "native-session",
            "thread_id": "native-thread",
            "turn_id": "native-turn"
        }
    });
    let expected = body.clone();

    apply_codex_openai_responses_special_body_edits(
        &mut body,
        "codex",
        "openai:responses",
        None,
        Some("key-123"),
    );

    assert_eq!(body["prompt_cache_key"], expected["prompt_cache_key"]);
    assert_eq!(body["client_metadata"], expected["client_metadata"]);
}

#[test]
fn preserves_uuid_prompt_cache_key_while_completing_codex_identity() {
    let identity = "172c39e6-c0a0-5a70-8b63-e0f8e0d185a3";
    let mut body = json!({
        "model": "gpt-5.6-luna",
        "input": "hello",
        "prompt_cache_key": identity
    });

    apply_codex_openai_responses_special_body_edits(
        &mut body,
        "codex",
        "openai:responses",
        None,
        None,
    );

    assert_eq!(body["prompt_cache_key"], identity);
    assert_eq!(body["client_metadata"]["session_id"], identity);
    assert_eq!(body["client_metadata"]["thread_id"], identity);
}

#[test]
fn keeps_codex_prompt_cache_domains_distinct() {
    let mut first = json!({
        "model": "gpt-5.6-luna",
        "input": "hello",
        "prompt_cache_key": "tenant-a"
    });
    let mut second = json!({
        "model": "gpt-5.6-luna",
        "input": "hello",
        "prompt_cache_key": "tenant-b"
    });

    for body in [&mut first, &mut second] {
        apply_codex_openai_responses_special_body_edits(
            body,
            "codex",
            "openai:responses",
            None,
            None,
        );
    }

    assert_ne!(first["prompt_cache_key"], second["prompt_cache_key"]);
    assert_eq!(
        first["prompt_cache_key"],
        first["client_metadata"]["session_id"]
    );
    assert_eq!(
        second["prompt_cache_key"],
        second["client_metadata"]["session_id"]
    );
}

#[test]
fn completes_partial_and_null_codex_client_metadata() {
    let mut partial = json!({
        "model": "gpt-5.6-luna",
        "input": "hello",
        "prompt_cache_key": "generic-affinity",
        "client_metadata": {
            "thread_id": "native-thread",
            "caller": "sdk"
        }
    });
    let mut null_metadata = json!({
        "model": "gpt-5.6-luna",
        "input": "hello",
        "prompt_cache_key": "generic-affinity",
        "client_metadata": null
    });
    let mut null_session = json!({
        "model": "gpt-5.6-luna",
        "input": "hello",
        "prompt_cache_key": "generic-affinity",
        "client_metadata": {
            "session_id": null,
            "thread_id": null,
            "caller": "sdk"
        }
    });

    for body in [&mut partial, &mut null_metadata, &mut null_session] {
        apply_codex_openai_responses_special_body_edits(
            body,
            "codex",
            "openai:responses",
            None,
            None,
        );
    }

    assert_eq!(partial["client_metadata"]["thread_id"], "native-thread");
    assert_eq!(partial["client_metadata"]["caller"], "sdk");
    assert_eq!(
        partial["client_metadata"]["session_id"],
        partial["prompt_cache_key"]
    );
    assert_eq!(
        null_metadata["client_metadata"]["session_id"],
        null_metadata["prompt_cache_key"]
    );
    assert_eq!(
        null_metadata["client_metadata"]["thread_id"],
        null_metadata["prompt_cache_key"]
    );
    assert_eq!(
        null_session["client_metadata"]["session_id"],
        null_session["prompt_cache_key"]
    );
    assert_eq!(
        null_session["client_metadata"]["thread_id"],
        null_session["prompt_cache_key"]
    );
    assert_eq!(null_session["client_metadata"]["caller"], "sdk");
}

#[test]
fn leaves_malformed_codex_client_metadata_unchanged() {
    let mut body = json!({
        "model": "gpt-5.6-luna",
        "input": "hello",
        "prompt_cache_key": "generic-affinity",
        "client_metadata": "invalid"
    });
    let mut malformed_fields = json!({
        "model": "gpt-5.6-luna",
        "input": "hello",
        "prompt_cache_key": "generic-affinity",
        "client_metadata": {
            "session_id": 42,
            "thread_id": ""
        }
    });
    let expected_malformed_metadata = malformed_fields["client_metadata"].clone();

    for candidate in [&mut body, &mut malformed_fields] {
        apply_codex_openai_responses_special_body_edits(
            candidate,
            "codex",
            "openai:responses",
            None,
            None,
        );
    }

    assert_eq!(body["prompt_cache_key"], "generic-affinity");
    assert_eq!(body["client_metadata"], "invalid");
    assert_eq!(malformed_fields["prompt_cache_key"], "generic-affinity");
    assert_eq!(
        malformed_fields["client_metadata"],
        expected_malformed_metadata
    );
}

#[test]
fn limits_prompt_cache_identity_adaptation_to_codex_responses_family() {
    let original = json!({
        "model": "gpt-5.6-luna",
        "input": "hello",
        "prompt_cache_key": "generic-affinity"
    });
    let mut standard_openai = original.clone();
    let mut codex_compact = original.clone();

    apply_codex_openai_responses_special_body_edits(
        &mut standard_openai,
        "openai",
        "openai:responses",
        None,
        None,
    );
    apply_codex_openai_responses_special_body_edits(
        &mut codex_compact,
        "codex",
        "openai:responses:compact",
        None,
        None,
    );

    assert_eq!(standard_openai, original);
    assert_ne!(codex_compact["prompt_cache_key"], "generic-affinity");
    assert!(codex_compact.get("client_metadata").is_none());
}

#[test]
fn chat_to_codex_responses_adapts_prompt_cache_identity_end_to_end() {
    let body = json!({
        "model": "gpt-5.6-luna",
        "messages": [{"role": "user", "content": "hello"}],
        "prompt_cache_key": "ltm-pc-v2-5557e02f5c9b447a97673ba330dbe77a"
    });

    let provider_request_body = build_cross_format_openai_responses_request_body(
        &body,
        "gpt-5.6-luna",
        "openai:chat",
        "openai:responses",
        true,
        false,
        "codex",
        None,
        None,
        &HeaderMap::new(),
        false,
    )
    .expect("chat to Codex Responses request should build");

    let expected_identity = "d9c5d122-7c1c-5fb1-ba9d-656062eda44e";
    assert_eq!(provider_request_body["prompt_cache_key"], expected_identity);
    assert_eq!(
        provider_request_body["client_metadata"]["session_id"],
        expected_identity
    );
    assert_eq!(
        provider_request_body["client_metadata"]["thread_id"],
        expected_identity
    );

    let mut provider_request_headers = BTreeMap::new();
    apply_codex_openai_special_headers(
        &mut provider_request_headers,
        &provider_request_body,
        &HeaderMap::new(),
        "codex",
        "openai:responses",
        Some("trace-codex-cache-identity"),
        None,
    );
    apply_codex_openai_responses_identity_headers(
        &mut provider_request_headers,
        &provider_request_body,
        "codex",
        "openai:responses",
    );
    assert_eq!(
        provider_request_headers
            .get("session-id")
            .map(String::as_str),
        Some(expected_identity)
    );
    assert_eq!(
        provider_request_headers
            .get("thread-id")
            .map(String::as_str),
        Some(expected_identity)
    );
}

#[test]
fn projects_uuid_prompt_cache_identity_into_missing_session_headers() {
    let mut headers = BTreeMap::new();
    let body = json!({
        "model": "gpt-5",
        "prompt_cache_key": "172c39e6-c0a0-5a70-8b63-e0f8e0d185a3",
    });

    apply_codex_openai_special_headers(
        &mut headers,
        &body,
        &HeaderMap::new(),
        "codex",
        "openai:responses",
        Some("trace-codex-123"),
        Some(r#"{"account_id":"acc-123","is_fedramp":true}"#),
    );
    apply_codex_openai_responses_identity_headers(&mut headers, &body, "codex", "openai:responses");
    assert_eq!(
        headers.get("chatgpt-account-id"),
        Some(&"acc-123".to_string())
    );
    assert_eq!(headers.get("x-client-request-id"), None);
    assert_eq!(
        headers.get("user-agent"),
        Some(&"codex_cli_rs/0.144.1".to_string())
    );
    assert_eq!(headers.get("originator"), Some(&"codex_cli_rs".to_string()));
    assert!(!headers.contains_key("version"));
    assert_eq!(headers.get("x-openai-fedramp"), Some(&"true".to_string()));
    assert_eq!(
        headers.get("session-id").map(String::as_str),
        Some("172c39e6-c0a0-5a70-8b63-e0f8e0d185a3")
    );
    assert_eq!(
        headers.get("thread-id").map(String::as_str),
        Some("172c39e6-c0a0-5a70-8b63-e0f8e0d185a3")
    );
}

#[test]
fn projects_native_codex_metadata_for_non_uuid_cache_overrides() {
    let mut headers = BTreeMap::new();
    let body = json!({
        "model": "gpt-5.6-luna",
        "prompt_cache_key": "guardian:parent-thread",
        "client_metadata": {
            "session_id": "019f687b-8e92-7842-9631-d5bf0dba0a3b",
            "thread_id": "019f6d20-1111-7222-8333-444455556666"
        }
    });

    apply_codex_openai_special_headers(
        &mut headers,
        &body,
        &HeaderMap::new(),
        "codex",
        "openai:responses",
        None,
        None,
    );
    apply_codex_openai_responses_identity_headers(&mut headers, &body, "codex", "openai:responses");

    assert_eq!(
        headers.get("session-id").map(String::as_str),
        Some("019f687b-8e92-7842-9631-d5bf0dba0a3b")
    );
    assert_eq!(
        headers.get("thread-id").map(String::as_str),
        Some("019f6d20-1111-7222-8333-444455556666")
    );
}

#[test]
fn leaves_non_native_cache_keys_out_of_identity_headers() {
    let mut headers = BTreeMap::new();
    let body = json!({
        "model": "gpt-5.6-luna",
        "prompt_cache_key": "generic-cache-key"
    });

    apply_codex_openai_special_headers(
        &mut headers,
        &body,
        &HeaderMap::new(),
        "codex",
        "openai:responses",
        None,
        None,
    );
    apply_codex_openai_responses_identity_headers(&mut headers, &body, "codex", "openai:responses");

    assert!(!headers.contains_key("session-id"));
    assert!(!headers.contains_key("thread-id"));
}

#[test]
fn leaves_malformed_native_metadata_out_of_identity_headers() {
    let mut headers = BTreeMap::new();
    let body = json!({
        "model": "gpt-5.6-luna",
        "prompt_cache_key": "172c39e6-c0a0-5a70-8b63-e0f8e0d185a3",
        "client_metadata": {
            "session_id": 42,
            "thread_id": ""
        }
    });

    apply_codex_openai_responses_identity_headers(&mut headers, &body, "codex", "openai:responses");

    assert!(!headers.contains_key("session-id"));
    assert!(!headers.contains_key("thread-id"));
}

#[test]
fn injects_only_codex_client_headers_for_images_requests() {
    let mut headers = BTreeMap::new();
    apply_codex_openai_special_headers(
        &mut headers,
        &json!({
            "model": "gpt-image-2",
            "prompt": "draw a city"
        }),
        &HeaderMap::new(),
        "codex",
        "openai:image",
        Some("trace-codex-image-123"),
        Some(r#"{"account_id":"acc-123","is_fedramp":true}"#),
    );
    assert_eq!(
        headers.get("chatgpt-account-id"),
        Some(&"acc-123".to_string())
    );
    assert_eq!(
        headers.get("user-agent"),
        Some(&"codex_cli_rs/0.144.1".to_string())
    );
    assert_eq!(headers.get("originator"), Some(&"codex_cli_rs".to_string()));
    assert!(!headers.contains_key("version"));
    assert_eq!(headers.get("x-openai-fedramp"), Some(&"true".to_string()));
    for name in ["x-client-request-id", "session-id", "thread-id"] {
        assert!(
            !headers.contains_key(name),
            "unexpected Images header: {name}"
        );
    }
}

#[test]
fn preserves_client_context_headers_and_enforces_codex_provider_identity() {
    let mut headers = BTreeMap::new();
    headers.insert(
        "x-client-request-id".to_string(),
        "kept-by-rule-request".to_string(),
    );
    headers.insert("session-id".to_string(), "kept-by-rule-session".to_string());
    headers.insert("thread-id".to_string(), "kept-by-rule-thread".to_string());
    headers.insert(
        "chatgpt-account-id".to_string(),
        "configured-spoof".to_string(),
    );
    headers.insert(
        "x-openai-fedramp".to_string(),
        "configured-false".to_string(),
    );
    headers.insert(
        "User-Agent".to_string(),
        "AsyncOpenAI/Python 2.44.0".to_string(),
    );
    headers.insert("ORIGINATOR".to_string(), "sdk-client".to_string());
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
        "session-id",
        HeaderValue::from_static("user-specified-session"),
    );
    original_headers.insert(
        "thread-id",
        HeaderValue::from_static("user-specified-thread"),
    );
    original_headers.insert(
        "user-agent",
        HeaderValue::from_static("user-specified-agent"),
    );
    original_headers.insert(
        "originator",
        HeaderValue::from_static("user-specified-originator"),
    );
    original_headers.insert("version", HeaderValue::from_static("user-version"));
    original_headers.insert("x-openai-fedramp", HeaderValue::from_static("user-fedramp"));
    original_headers.insert(
        "chatgpt-account-id",
        HeaderValue::from_static("user-account"),
    );

    apply_codex_openai_special_headers(
        &mut headers,
        &body,
        &original_headers,
        "codex",
        "openai:responses",
        Some("trace-codex-123"),
        Some(r#"{"account_id":"acc-123","is_fedramp":true}"#),
    );
    apply_codex_openai_responses_identity_headers(&mut headers, &body, "codex", "openai:responses");

    assert_eq!(
        headers.get("x-client-request-id"),
        Some(&"kept-by-rule-request".to_string())
    );
    assert_eq!(
        headers.get("user-agent"),
        Some(&"codex_cli_rs/0.144.1".to_string())
    );
    assert_eq!(headers.get("originator"), Some(&"codex_cli_rs".to_string()));
    assert_eq!(
        headers
            .keys()
            .filter(|name| name.eq_ignore_ascii_case("user-agent"))
            .count(),
        1
    );
    assert_eq!(
        headers
            .keys()
            .filter(|name| name.eq_ignore_ascii_case("originator"))
            .count(),
        1
    );
    assert!(!headers.contains_key("version"));
    assert_eq!(
        headers.get("chatgpt-account-id"),
        Some(&"acc-123".to_string())
    );
    assert_eq!(headers.get("x-openai-fedramp"), Some(&"true".to_string()));
    assert_eq!(
        headers.get("session-id"),
        Some(&"kept-by-rule-session".to_string())
    );
    assert_eq!(
        headers.get("thread-id"),
        Some(&"kept-by-rule-thread".to_string())
    );
}

#[test]
fn compact_projects_uuid_prompt_cache_identity_into_session_headers() {
    let mut headers = BTreeMap::new();
    let body = json!({
        "model": "gpt-5",
        "prompt_cache_key": "172c39e6-c0a0-5a70-8b63-e0f8e0d185a3",
    });

    apply_codex_openai_special_headers(
        &mut headers,
        &body,
        &HeaderMap::new(),
        "codex",
        "openai:responses:compact",
        Some("trace-codex-compact-123"),
        Some(r#"{"account_id":"acc-123","is_fedramp":true}"#),
    );
    apply_codex_openai_responses_identity_headers(
        &mut headers,
        &body,
        "codex",
        "openai:responses:compact",
    );

    assert_eq!(
        headers.get("chatgpt-account-id"),
        Some(&"acc-123".to_string())
    );
    assert_eq!(headers.get("x-client-request-id"), None);
    assert_eq!(
        headers.get("user-agent"),
        Some(&"codex_cli_rs/0.144.1".to_string())
    );
    assert_eq!(headers.get("originator"), Some(&"codex_cli_rs".to_string()));
    assert!(!headers.contains_key("version"));
    assert_eq!(headers.get("x-openai-fedramp"), Some(&"true".to_string()));
    assert_eq!(
        headers.get("session-id").map(String::as_str),
        Some("172c39e6-c0a0-5a70-8b63-e0f8e0d185a3")
    );
    assert_eq!(
        headers.get("thread-id").map(String::as_str),
        Some("172c39e6-c0a0-5a70-8b63-e0f8e0d185a3")
    );
}
