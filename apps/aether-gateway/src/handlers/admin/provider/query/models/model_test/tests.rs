use super::*;
use crate::handlers::admin::request::AdminGatewayProviderTransportSnapshot;
use serde_json::json;

fn sample_openai_image_transport(provider_type: &str) -> AdminGatewayProviderTransportSnapshot {
    let base_url = if provider_type == "custom" {
        "https://grok.com/v1/"
    } else {
        "https://grok.com/"
    };
    AdminGatewayProviderTransportSnapshot {
        provider: crate::provider_transport::snapshot::GatewayProviderTransportProvider {
            id: "provider-1".to_string(),
            name: "Provider".to_string(),
            provider_type: provider_type.to_string(),
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
        endpoint: crate::provider_transport::snapshot::GatewayProviderTransportEndpoint {
            id: "endpoint-1".to_string(),
            provider_id: "provider-1".to_string(),
            api_format: "openai:image".to_string(),
            api_family: None,
            endpoint_kind: None,
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
        key: crate::provider_transport::snapshot::GatewayProviderTransportKey {
            id: "key-1".to_string(),
            provider_id: "provider-1".to_string(),
            name: "key".to_string(),
            auth_type: "oauth".to_string(),
            is_active: true,
            api_formats: None,
            auth_type_by_format: None,
            allow_auth_channel_mismatch_formats: None,
            allowed_models: None,
            capabilities: None,
            rate_multipliers: None,
            global_priority_by_format: None,
            expires_at_unix_secs: None,
            proxy: None,
            fingerprint: None,
            upstream_metadata: None,
            decrypted_api_key: String::new(),
            decrypted_auth_config: Some(
                json!({
                    "sso_token": "abc",
                    "sso_rw_token": "rw"
                })
                .to_string(),
            ),
        },
    }
}

fn sample_catalog_key_with_allowed_models(
    allowed_models: Option<serde_json::Value>,
) -> aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey {
    let mut key =
        aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey::new(
            "key-1".to_string(),
            "provider-1".to_string(),
            "key".to_string(),
            "api_key".to_string(),
            None,
            true,
        )
        .expect("sample provider key should build");
    key.allowed_models = allowed_models;
    key
}

#[test]
fn provider_query_model_test_allows_keys_without_model_restrictions() {
    let unrestricted = sample_catalog_key_with_allowed_models(None);
    let empty = sample_catalog_key_with_allowed_models(Some(json!([])));

    assert!(provider_query_key_allows_effective_test_model(
        &unrestricted,
        "model-b",
        "model-b-upstream",
    ));
    assert!(provider_query_key_allows_effective_test_model(
        &empty,
        "model-b",
        "model-b-upstream",
    ));
}

#[test]
fn provider_query_model_test_filters_key_disallowed_for_requested_model() {
    let key = sample_catalog_key_with_allowed_models(Some(json!(["model-a"])));

    assert!(!provider_query_key_allows_effective_test_model(
        &key,
        "model-b",
        "model-b-upstream",
    ));
}

#[test]
fn provider_query_model_test_allows_key_for_requested_or_mapped_model() {
    let requested_allowed = sample_catalog_key_with_allowed_models(Some(json!(["model-b"])));
    let mapped_allowed = sample_catalog_key_with_allowed_models(Some(json!(["MODEL-B-UPSTREAM"])));

    assert!(provider_query_key_allows_effective_test_model(
        &requested_allowed,
        "model-b",
        "model-b-upstream",
    ));
    assert!(provider_query_key_allows_effective_test_model(
        &mapped_allowed,
        "model-b",
        "model-b-upstream",
    ));
}

#[test]
fn provider_query_test_request_body_preserves_custom_model() {
    let payload = json!({
        "request_body": {
            "model": "custom-upstream-model",
            "messages": []
        }
    });

    let body = provider_query_build_test_request_body(&payload, "fallback-model");

    assert_eq!(body["model"], json!("custom-upstream-model"));
}

#[test]
fn provider_query_test_request_body_defaults_missing_model() {
    let payload = json!({
        "request_body": {
            "messages": []
        }
    });

    let body = provider_query_build_test_request_body(&payload, "fallback-model");

    assert_eq!(body["model"], json!("fallback-model"));
}

#[test]
fn provider_query_execution_json_body_decodes_stream_encoded_json_response() {
    use base64::Engine as _;

    let body = json!({
        "created": 1,
        "data": [{
            "url": "https://example.test/image.png"
        }]
    });
    let encoded_body = base64::engine::general_purpose::STANDARD
        .encode(serde_json::to_vec(&body).expect("test body should serialize"));
    let result = aether_contracts::ExecutionResult {
        request_id: "request-1".to_string(),
        candidate_id: None,
        status_code: 200,
        headers: std::collections::BTreeMap::from([(
            "content-type".to_string(),
            "application/json".to_string(),
        )]),
        body: Some(aether_contracts::ResponseBody {
            json_body: None,
            body_bytes_b64: Some(encoded_body),
        }),
        telemetry: None,
        error: None,
    };

    assert_eq!(
        provider_query_execution_json_body(&result),
        Some(body.clone())
    );
    assert_eq!(
        provider_query_standard_execution_response_body("openai:image", &result, None),
        Some(body)
    );
}

#[test]
fn provider_query_test_request_body_fills_empty_conversation() {
    let payload = json!({
        "request_body": {
            "model": "custom-upstream-model",
            "messages": []
        }
    });

    let body = provider_query_build_test_request_body(&payload, "fallback-model");

    assert_eq!(body["model"], json!("custom-upstream-model"));
    assert_eq!(
        body["messages"],
        json!([{ "role": "user", "content": DEFAULT_PROVIDER_QUERY_TEST_MESSAGE }])
    );
}

#[test]
fn provider_query_test_request_body_keeps_non_empty_conversation() {
    let payload = json!({
        "request_body": {
            "model": "custom-upstream-model",
            "messages": [{ "role": "user", "content": "custom prompt" }]
        }
    });

    let body = provider_query_build_test_request_body(&payload, "fallback-model");

    assert_eq!(
        body["messages"],
        json!([{ "role": "user", "content": "custom prompt" }])
    );
}

#[test]
fn provider_query_default_test_request_body_does_not_set_max_tokens() {
    let body = provider_query_build_test_request_body(&json!({}), "fallback-model");

    assert_eq!(body["model"], json!("fallback-model"));
    assert!(
        body.get("max_tokens").is_none(),
        "admin model test must not silently force a low max_tokens value"
    );
}

#[test]
fn provider_query_failover_request_body_overrides_custom_model() {
    let payload = json!({
        "request_body": {
            "model": "custom-upstream-model",
            "messages": []
        }
    });

    let body = provider_query_build_test_request_body_for_route(
        &payload,
        "failover-model",
        "/api/admin/provider-query/test-model-failover",
    );

    assert_eq!(body["model"], json!("failover-model"));
}

#[test]
fn provider_query_failover_request_body_uses_explicit_mapped_model() {
    let payload = json!({
        "mapped_model_name": "upstream-mapped-model",
        "request_body": {
            "model": "original-selected-model",
            "messages": []
        }
    });

    let body = provider_query_build_test_request_body_for_route(
        &payload,
        "upstream-mapped-model",
        "/api/admin/provider-query/test-model-failover",
    );

    assert_eq!(body["model"], json!("upstream-mapped-model"));
}

#[test]
fn provider_query_request_body_model_uses_non_empty_string_only() {
    let custom = json!({ "model": " custom-model " });
    let blank = json!({ "model": " " });
    let non_string = json!({ "model": 123 });

    assert_eq!(
        provider_query_request_body_model(&custom, "fallback-model"),
        "custom-model"
    );
    assert_eq!(
        provider_query_request_body_model(&blank, "fallback-model"),
        "fallback-model"
    );
    assert_eq!(
        provider_query_request_body_model(&non_string, "fallback-model"),
        "fallback-model"
    );
}

#[test]
fn provider_query_model_test_extracts_multiple_selected_key_ids() {
    let payload = json!({
        "api_key_ids": [" key-b ", "", "key-a", "key-b"],
        "api_key_id": "key-c"
    });

    let ids = provider_query_extract_api_key_ids(&payload)
        .expect("non-empty key selection should be extracted")
        .into_iter()
        .collect::<Vec<_>>();

    assert_eq!(ids, vec!["key-a", "key-b", "key-c"]);
}

#[test]
fn provider_query_model_test_empty_selected_key_ids_keep_default_selection() {
    assert!(provider_query_extract_api_key_ids(&json!({})).is_none());
    assert!(provider_query_extract_api_key_ids(&json!({ "api_key_ids": [] })).is_none());
}

#[test]
fn provider_query_standard_test_resolves_codex_responses_upstream_streaming() {
    assert!(provider_query_resolve_standard_test_upstream_is_stream(
        None,
        "codex",
        "openai:responses",
    ));
    assert!(provider_query_resolve_standard_test_upstream_is_stream(
        Some(&json!({"upstream_stream_policy": "force_non_stream"})),
        "codex",
        "openai:responses",
    ));
    assert!(!provider_query_resolve_standard_test_upstream_is_stream(
        None,
        "codex",
        "openai:responses:compact",
    ));
    assert!(!provider_query_resolve_standard_test_upstream_is_stream(
        Some(&json!({"upstream_stream_policy": "force_stream"})),
        "codex",
        "openai:responses:compact",
    ));
    assert!(!provider_query_resolve_standard_test_upstream_is_stream(
        None,
        "custom",
        "openai:responses",
    ));
    assert!(provider_query_resolve_standard_test_upstream_is_stream(
        Some(&json!({"upstream_stream_policy": "force_stream"})),
        "custom",
        "openai:responses",
    ));
}

#[test]
fn provider_query_standard_test_reenforces_upstream_stream_body_field() {
    let endpoint_config = json!({"upstream_stream_policy": "force_stream"});
    let mut body = json!({"model": "gpt-5", "input": "hello", "stream": false});
    let upstream_is_stream = provider_query_resolve_standard_test_upstream_is_stream(
        Some(&endpoint_config),
        "codex",
        "openai:responses",
    );
    let require_body_stream_field =
        provider_query_request_requires_body_stream_field(&body, Some(&endpoint_config));

    crate::ai_serving::enforce_request_body_stream_field(
        &mut body,
        "openai:responses",
        upstream_is_stream,
        require_body_stream_field,
    );

    assert_eq!(body["stream"], json!(true));
}

#[test]
fn provider_query_standard_test_aggregates_responses_stream_body() {
    let stream_body = concat!(
        "event: response.created\n",
        "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_123\",\"object\":\"response\",\"model\":\"gpt-5.4-mini\",\"status\":\"in_progress\",\"output\":[]}}\n\n",
        "event: response.output_text.delta\n",
        "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"content_index\":0,\"delta\":\"Hello\"}\n\n",
        "event: response.completed\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_123\",\"object\":\"response\",\"model\":\"gpt-5.4-mini\",\"status\":\"completed\",\"output\":[]}}\n\n",
    );
    let result = aether_contracts::ExecutionResult {
        request_id: "provider-test".to_string(),
        candidate_id: Some("candidate-0".to_string()),
        status_code: 200,
        headers: BTreeMap::new(),
        body: Some(aether_contracts::ResponseBody {
            json_body: None,
            body_bytes_b64: Some(
                base64::engine::general_purpose::STANDARD.encode(stream_body.as_bytes()),
            ),
        }),
        telemetry: None,
        error: None,
    };

    let body = provider_query_standard_execution_response_body("openai:responses", &result, None)
        .expect("stream body should aggregate");

    assert_eq!(body["model"], json!("gpt-5.4-mini"));
    assert_eq!(body["output"][0]["content"][0]["text"], json!("Hello"));
}

#[test]
fn provider_query_standard_test_aggregates_responses_image_generation_call() {
    let stream_body = concat!(
        "event: response.created\n",
        "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_img_123\",\"object\":\"response\",\"model\":\"gpt-5.4-mini\",\"status\":\"in_progress\",\"output\":[]}}\n\n",
        "event: response.output_item.done\n",
        "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"id\":\"ig_123\",\"type\":\"image_generation_call\",\"status\":\"completed\",\"output_format\":\"png\",\"result\":\"aGVsbG8=\"}}\n\n",
        "event: response.completed\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_img_123\",\"object\":\"response\",\"model\":\"gpt-5.4-mini\",\"status\":\"completed\",\"output\":[]}}\n\n",
    );
    let result = aether_contracts::ExecutionResult {
        request_id: "provider-test".to_string(),
        candidate_id: Some("candidate-0".to_string()),
        status_code: 200,
        headers: BTreeMap::new(),
        body: Some(aether_contracts::ResponseBody {
            json_body: None,
            body_bytes_b64: Some(
                base64::engine::general_purpose::STANDARD.encode(stream_body.as_bytes()),
            ),
        }),
        telemetry: None,
        error: None,
    };

    let body = provider_query_standard_execution_response_body("openai:responses", &result, None)
        .expect("responses image stream body should aggregate");

    assert_eq!(body["output"][0]["type"], json!("image_generation_call"));
    assert_eq!(body["output"][0]["result"], json!("aGVsbG8="));
}

#[test]
fn provider_query_responses_test_request_body_defaults_to_responses_input() {
    let payload = json!({"message": "hello from responses"});

    let body = provider_query_build_test_request_body_for_api_format(
        &payload,
        "gpt-5.4-mini",
        "/api/admin/provider-query/test-model",
        "openai:responses",
    );

    assert_eq!(body["model"], json!("gpt-5.4-mini"));
    assert_eq!(body["input"], json!("hello from responses"));
    assert!(body.get("messages").is_none());
}

#[test]
fn provider_query_compact_test_request_body_defaults_to_responses_input() {
    let payload = json!({"message": "hello from compact"});

    let client_api_format =
        provider_query_standard_test_client_api_format("openai:responses:compact");
    let body = provider_query_build_test_request_body_for_api_format(
        &payload,
        "gpt-5.4-mini",
        "/api/admin/provider-query/test-model",
        client_api_format,
    );

    assert_eq!(client_api_format, "openai:responses:compact");
    assert_eq!(body["model"], json!("gpt-5.4-mini"));
    assert_eq!(body["input"], json!("hello from compact"));
    assert!(body.get("messages").is_none());
}

#[test]
fn provider_query_search_test_request_body_defaults_to_typed_search_input() {
    let payload = json!({"message": "find current documentation"});

    let client_api_format = provider_query_standard_test_client_api_format("openai:search");
    let body = provider_query_build_test_request_body_for_api_format(
        &payload,
        "gpt-5.6-sol",
        "/api/admin/provider-query/test-model",
        client_api_format,
    );

    assert_eq!(client_api_format, "openai:search");
    assert!(body["id"]
        .as_str()
        .is_some_and(|value| value.starts_with("aether-model-test-")));
    assert_eq!(body["model"], json!("gpt-5.6-sol"));
    assert_eq!(body["input"], json!("find current documentation"));
    assert_eq!(
        body["commands"]["search_query"][0]["q"],
        json!("find current documentation")
    );
    assert_eq!(body["max_output_tokens"], json!(256));
    assert!(body.get("messages").is_none());
    assert!(body.get("stream").is_none());
}

#[test]
fn provider_query_search_test_completes_missing_protocol_fields() {
    let payload = json!({
        "request_body": {
            "model": "gpt-5.6-luna",
            "input": "find release notes"
        }
    });

    let body = provider_query_build_test_request_body_for_api_format_with_search_session(
        &payload,
        "gpt-5.6-luna",
        "/api/admin/provider-query/test-model",
        "openai:search",
        Some("trace-model-test-1"),
    );

    assert_eq!(body["id"], json!("aether-model-test-trace-model-test-1"));
    assert_eq!(
        body["commands"]["search_query"][0]["q"],
        json!("find release notes")
    );
    assert_eq!(body["max_output_tokens"], json!(256));
}

#[test]
fn provider_query_search_test_preserves_a_non_empty_client_session_id() {
    let payload = json!({
        "request_body": {
            "id": "client-search-session",
            "model": "gpt-5.6-luna",
            "input": "find release notes"
        }
    });

    let body = provider_query_build_test_request_body_for_api_format_with_search_session(
        &payload,
        "gpt-5.6-luna",
        "/api/admin/provider-query/test-model",
        "openai:search",
        Some("trace-model-test-1"),
    );

    assert_eq!(body["id"], json!("client-search-session"));
}

#[test]
fn provider_query_search_success_requires_non_empty_output() {
    fn result(body: Value) -> aether_contracts::ExecutionResult {
        aether_contracts::ExecutionResult {
            request_id: "provider-search-test".to_string(),
            candidate_id: Some("candidate-0".to_string()),
            status_code: 200,
            headers: BTreeMap::new(),
            body: Some(aether_contracts::ResponseBody {
                json_body: Some(body),
                body_bytes_b64: None,
            }),
            telemetry: None,
            error: None,
        }
    }

    assert!(provider_query_standard_execution_response_body(
        "openai:search",
        &result(json!({})),
        None,
    )
    .is_none());
    assert!(provider_query_standard_execution_response_body(
        "openai:search",
        &result(json!({"output": "  "})),
        None,
    )
    .is_none());
    assert_eq!(
        provider_query_standard_execution_response_body(
            "openai:search",
            &result(json!({"output": "search result", "encrypted_output": "ciphertext"})),
            None,
        ),
        Some(json!({"output": "search result", "encrypted_output": "ciphertext"}))
    );
}

#[test]
fn provider_query_embedding_test_request_body_defaults_to_embedding_input() {
    let payload = json!({"message": "hello from embedding"});

    let client_api_format =
        provider_query_standard_test_client_api_format("aliyun:multimodal_embedding");
    let body = provider_query_build_test_request_body_for_api_format(
        &payload,
        "qwen3-vl-embedding",
        "/api/admin/provider-query/test-model",
        client_api_format,
    );

    assert_eq!(client_api_format, "openai:embedding");
    assert_eq!(body["model"], json!("qwen3-vl-embedding"));
    assert_eq!(body["input"], json!("hello from embedding"));
    assert!(body.get("messages").is_none());
    assert!(body.get("stream").is_none());
}

#[test]
fn provider_query_compact_test_request_body_promotes_prompt_to_input() {
    let payload = json!({
        "request_body": {
            "model": "custom-model",
            "prompt": "hello from prompt"
        }
    });

    let body = provider_query_build_test_request_body_for_api_format(
        &payload,
        "fallback-model",
        "/api/admin/provider-query/test-model",
        "openai:responses:compact",
    );

    assert_eq!(body["model"], json!("custom-model"));
    assert_eq!(body["input"], json!("hello from prompt"));
    assert!(body.get("prompt").is_none());
    assert!(provider_query_request_body_is_openai_responses_shape(&body));
}

#[test]
fn provider_query_compact_test_request_body_strips_stale_chat_fields() {
    let payload = json!({
        "request_body": {
            "model": "custom-model",
            "input": "hello from input",
            "messages": [{ "role": "user", "content": "stale chat body" }],
            "prompt": "stale prompt"
        }
    });

    let body = provider_query_build_test_request_body_for_api_format(
        &payload,
        "fallback-model",
        "/api/admin/provider-query/test-model",
        "openai:responses:compact",
    );

    assert_eq!(body["model"], json!("custom-model"));
    assert_eq!(body["input"], json!("hello from input"));
    assert!(body.get("messages").is_none());
    assert!(body.get("prompt").is_none());
}

#[test]
fn provider_query_compact_provider_body_builds_without_chat_conversion() {
    let payload = json!({"message": "hello compact provider"});
    let client_api_format =
        provider_query_standard_test_client_api_format("openai:responses:compact");
    let mut request_body = provider_query_build_test_request_body_for_api_format(
        &payload,
        "gpt-5.4-mini",
        "/api/admin/provider-query/test-model",
        client_api_format,
    );
    if let Some(object) = request_body.as_object_mut() {
        object.insert("stream".to_string(), serde_json::Value::Bool(false));
    }

    assert!(provider_query_request_body_is_openai_responses_shape(
        &request_body
    ));

    let mut provider_request_body = crate::ai_serving::build_local_openai_responses_request_body(
        &request_body,
        "upstream-gpt",
        false,
    )
    .expect("compact model test body should build from responses shape");
    crate::ai_serving::apply_openai_responses_compact_special_body_edits(
        &mut provider_request_body,
        "openai:responses:compact",
    );
    crate::ai_serving::enforce_request_body_stream_field(
        &mut provider_request_body,
        "openai:responses:compact",
        false,
        true,
    );

    assert_eq!(provider_request_body["model"], json!("upstream-gpt"));
    assert_eq!(
        provider_request_body["input"],
        json!("hello compact provider")
    );
    assert!(provider_request_body.get("messages").is_none());
    assert!(provider_request_body.get("stream").is_none());
    assert!(provider_request_body.get("store").is_none());
}

#[test]
fn provider_query_standard_test_rejects_gemini_success_without_visible_output() {
    let result = aether_contracts::ExecutionResult {
        request_id: "provider-test".to_string(),
        candidate_id: Some("candidate-0".to_string()),
        status_code: 200,
        headers: BTreeMap::new(),
        body: Some(aether_contracts::ResponseBody {
            json_body: Some(json!({
                "candidates": [{
                    "content": {"role": "model"},
                    "finishReason": "MAX_TOKENS"
                }],
                "usageMetadata": {
                    "promptTokenCount": 8,
                    "candidatesTokenCount": 1,
                    "thoughtsTokenCount": 25,
                    "totalTokenCount": 34
                },
                "modelVersion": "gemini-3-flash-preview",
                "responseId": "resp-empty"
            })),
            body_bytes_b64: None,
        }),
        telemetry: None,
        error: None,
    };

    assert!(provider_query_standard_execution_response_body(
        "gemini:generate_content",
        &result,
        None
    )
    .is_none());
}

#[test]
fn provider_query_test_adapter_routes_fixed_provider_endpoint_types() {
    assert_eq!(
        provider_query_test_adapter_for_provider_api_format("custom", "openai:chat"),
        Some(ProviderQueryTestAdapter::Standard)
    );
    assert_eq!(
        provider_query_test_adapter_for_provider_api_format("codex", "openai:responses"),
        Some(ProviderQueryTestAdapter::Standard)
    );
    assert_eq!(
        provider_query_test_adapter_for_provider_api_format("codex", "openai:responses:compact"),
        Some(ProviderQueryTestAdapter::Standard)
    );
    assert_eq!(
        provider_query_test_adapter_for_provider_api_format("codex", "openai:search"),
        Some(ProviderQueryTestAdapter::Standard)
    );
    assert_eq!(
        provider_query_test_adapter_for_provider_api_format("chatgpt_web", "openai:image"),
        Some(ProviderQueryTestAdapter::OpenAiImage)
    );
    assert_eq!(
        provider_query_test_adapter_for_provider_api_format("kiro", "claude:messages"),
        Some(ProviderQueryTestAdapter::Kiro)
    );
    assert_eq!(
        provider_query_test_adapter_for_provider_api_format(
            "gemini_cli",
            "gemini:generate_content"
        ),
        Some(ProviderQueryTestAdapter::Standard)
    );
    assert_eq!(
        provider_query_test_adapter_for_provider_api_format(
            "antigravity",
            "gemini:generate_content"
        ),
        Some(ProviderQueryTestAdapter::Antigravity)
    );
    assert_eq!(
        provider_query_test_adapter_for_provider_api_format("grok", "openai:chat"),
        Some(ProviderQueryTestAdapter::Grok)
    );
    assert_eq!(
        provider_query_test_adapter_for_provider_api_format("grok", "openai:responses"),
        Some(ProviderQueryTestAdapter::Grok)
    );
    assert_eq!(
        provider_query_test_adapter_for_provider_api_format("grok", "claude:messages"),
        Some(ProviderQueryTestAdapter::Grok)
    );
    assert_eq!(
        provider_query_test_adapter_for_provider_api_format("grok", "openai:image"),
        Some(ProviderQueryTestAdapter::OpenAiImage)
    );
    assert_eq!(
        provider_query_test_adapter_for_provider_api_format("custom", "openai:embedding"),
        Some(ProviderQueryTestAdapter::Standard)
    );
    assert_eq!(
        provider_query_test_adapter_for_provider_api_format("custom", "gemini:embedding"),
        Some(ProviderQueryTestAdapter::Standard)
    );
    assert_eq!(
        provider_query_test_adapter_for_provider_api_format("custom", "gemini:interactions"),
        Some(ProviderQueryTestAdapter::Standard)
    );
    assert_eq!(
        provider_query_test_adapter_for_provider_api_format(
            "aliyun",
            "aliyun:multimodal_embedding"
        ),
        Some(ProviderQueryTestAdapter::Standard)
    );
    assert_eq!(
        provider_query_test_adapter_for_provider_api_format("jina", "jina:rerank"),
        Some(ProviderQueryTestAdapter::Standard)
    );
    assert_eq!(
        provider_query_test_adapter_for_provider_api_format("custom", "openai:video"),
        None
    );
    assert_eq!(
        provider_query_test_adapter_for_provider_api_format("custom", "gemini:video"),
        None
    );
    assert_eq!(
        provider_query_test_adapter_for_provider_api_format("gemini", "gemini:files"),
        None
    );
}

#[test]
fn provider_query_endpoint_priority_prefers_text_before_cli_and_image() {
    assert_eq!(
        provider_query_model_test_endpoint_priority("custom", "openai:chat"),
        Some(0)
    );
    assert_eq!(
        provider_query_model_test_endpoint_priority("codex", "openai:responses:compact"),
        Some(1)
    );
    assert_eq!(
        provider_query_model_test_endpoint_priority("codex", "openai:search"),
        Some(1)
    );
    assert_eq!(
        provider_query_model_test_endpoint_priority("chatgpt_web", "openai:image"),
        Some(2)
    );
    assert_eq!(
        provider_query_model_test_endpoint_priority("grok", "openai:chat"),
        Some(0)
    );
    assert_eq!(
        provider_query_model_test_endpoint_priority("grok", "openai:responses"),
        Some(0)
    );
    assert_eq!(
        provider_query_model_test_endpoint_priority("antigravity", "gemini:generate_content"),
        Some(1)
    );
}

#[test]
fn provider_query_grok_model_test_body_maps_non_reasoning_model_to_fast_mode() {
    let payload = json!({
        "request_body": {
            "model": "grok-4.20-0309-non-reasoning",
            "messages": [
                {"role": "system", "content": "be concise"},
                {"role": "user", "content": "hello"}
            ]
        }
    });
    let request_body = provider_query_build_test_request_body_for_route(
        &payload,
        "grok-4.20-0309-non-reasoning",
        "/api/admin/provider-query/test-model",
    );

    let upstream_body = crate::provider_transport::build_grok_app_chat_body(
        "openai:chat",
        Some(provider_query_request_body_model(
            &request_body,
            "grok-4.20-0309-non-reasoning",
        )),
        &request_body,
    );

    assert_eq!(upstream_body["modeId"], json!("fast"));
    assert_eq!(
        upstream_body["message"],
        json!("[system]: be concise\n\n[user]: hello")
    );
}

#[test]
fn provider_query_grok_model_test_uses_responses_client_body_for_responses_endpoint() {
    let payload = json!({
        "request_body": {
            "model": "grok-4.20-0309-non-reasoning",
            "input": "hello from responses body"
        }
    });
    let request_body = provider_query_build_grok_test_request_body_for_api_format(
        &payload,
        "grok-4.20-0309-non-reasoning",
        "/api/admin/provider-query/test-model",
        "openai:responses",
    );

    let upstream_body = crate::provider_transport::build_grok_app_chat_body(
        provider_query_grok_test_client_api_format("openai:responses"),
        Some(provider_query_request_body_model(
            &request_body,
            "grok-4.20-0309-non-reasoning",
        )),
        &request_body,
    );

    assert_eq!(upstream_body["modeId"], json!("fast"));
    assert_eq!(upstream_body["message"], json!("hello from responses body"));
}

#[test]
fn provider_query_grok_model_test_uses_responses_input_when_existing_body_has_messages() {
    let payload = json!({
        "request_body": {
            "model": "grok-4.20-0309-non-reasoning",
            "messages": [{
                "role": "user",
                "content": "hello from stale chat body"
            }]
        }
    });
    let request_body = provider_query_build_grok_test_request_body_for_api_format(
        &payload,
        "grok-4.20-0309-non-reasoning",
        "/api/admin/provider-query/test-model",
        "openai:responses",
    );

    assert_eq!(request_body["model"], json!("grok-4.20-0309-non-reasoning"));
    assert_eq!(
        request_body["input"],
        json!("Hello! This is a test message.")
    );
    assert!(request_body.get("messages").is_some());

    let upstream_body = crate::provider_transport::build_grok_app_chat_body(
        provider_query_grok_test_client_api_format("openai:responses"),
        Some(provider_query_request_body_model(
            &request_body,
            "grok-4.20-0309-non-reasoning",
        )),
        &request_body,
    );

    assert_eq!(upstream_body["modeId"], json!("fast"));
    assert_eq!(
        upstream_body["message"],
        json!("Hello! This is a test message.")
    );
}

#[test]
fn provider_query_grok_model_test_defaults_claude_messages_body_for_claude_endpoint() {
    let payload = json!({});
    let request_body = provider_query_build_grok_test_request_body_for_api_format(
        &payload,
        "grok-4.20-0309-non-reasoning",
        "/api/admin/provider-query/test-model",
        "claude:messages",
    );

    assert_eq!(request_body["model"], json!("grok-4.20-0309-non-reasoning"));
    assert_eq!(
        request_body["messages"],
        json!([{ "role": "user", "content": DEFAULT_PROVIDER_QUERY_TEST_MESSAGE }])
    );

    let upstream_body = crate::provider_transport::build_grok_app_chat_body(
        provider_query_grok_test_client_api_format("claude:messages"),
        Some(provider_query_request_body_model(
            &request_body,
            "grok-4.20-0309-non-reasoning",
        )),
        &request_body,
    );

    assert_eq!(upstream_body["modeId"], json!("fast"));
    assert_eq!(
        upstream_body["message"],
        json!("[user]: Hello! This is a test message.")
    );
}

#[test]
fn provider_query_candidate_summary_marks_unused_after_first_success() {
    let attempts = vec![json!({
        "candidate_index": 0,
        "key_name": "winning-key",
        "key_id": "key-1",
        "auth_type": "api_key",
        "effective_model": "claude-haiku",
        "endpoint_api_format": "claude:messages",
        "endpoint_base_url": "https://api.example",
        "status": "success",
        "latency_ms": 123,
        "status_code": 200
    })];

    let summary = provider_query_candidate_summary_payload(3, 1, &attempts);

    assert_eq!(summary["total_candidates"], json!(3));
    assert_eq!(summary["attempted"], json!(1));
    assert_eq!(summary["success"], json!(1));
    assert_eq!(summary["unused"], json!(2));
    assert_eq!(summary["stop_reason"], json!("first_success"));
    assert_eq!(summary["winning_key_name"], json!("winning-key"));
    assert_eq!(
        summary["winning_endpoint_api_format"],
        json!("claude:messages")
    );
}

#[test]
fn provider_query_candidate_summary_reports_skipped_exhaustion() {
    let attempts = vec![json!({
        "candidate_index": 0,
        "status": "skipped",
        "skip_reason": "transport_api_format_mismatch"
    })];

    let summary = provider_query_candidate_summary_payload(1, 0, &attempts);

    assert_eq!(summary["total_candidates"], json!(1));
    assert_eq!(summary["attempted"], json!(0));
    assert_eq!(summary["skipped"], json!(1));
    assert_eq!(summary["unused"], json!(0));
    assert_eq!(summary["stop_reason"], json!("all_skipped"));
}

#[test]
fn provider_query_candidate_summary_counts_scheduler_skips_before_success() {
    let attempts = vec![
        json!({
            "candidate_index": 0,
            "status": "skipped",
            "skip_reason": "pool_account_exhausted"
        }),
        json!({
            "candidate_index": 1,
            "key_name": "winning-key",
            "key_id": "key-1",
            "auth_type": "oauth",
            "effective_model": "gpt-5.4-mini",
            "endpoint_api_format": "openai:responses",
            "endpoint_base_url": "https://chatgpt.com/backend-api/codex",
            "status": "success",
            "latency_ms": 123,
            "status_code": 200
        }),
    ];

    let summary = provider_query_candidate_summary_payload(4, 1, &attempts);

    assert_eq!(summary["total_candidates"], json!(4));
    assert_eq!(summary["attempted"], json!(1));
    assert_eq!(summary["success"], json!(1));
    assert_eq!(summary["skipped"], json!(1));
    assert_eq!(summary["unused"], json!(2));
    assert_eq!(summary["stop_reason"], json!("first_success"));
}

#[test]
fn provider_query_image_test_request_body_defaults_generation_prompt() {
    let payload = json!({"message": "draw a small icon"});

    let body = provider_query_build_openai_image_test_request_body_for_route(
        &payload,
        "gpt-image-1",
        "/api/admin/provider-query/test-model",
    );

    assert_eq!(body["model"], json!("gpt-image-1"));
    assert_eq!(body["prompt"], json!("draw a small icon"));
    assert_eq!(body["stream"], json!(true));
}

#[test]
fn provider_query_failover_image_test_request_body_overrides_model() {
    let payload = json!({
        "request_body": {
            "model": "old-image-model",
            "prompt": "draw"
        }
    });

    let body = provider_query_build_openai_image_test_request_body_for_route(
        &payload,
        "new-image-model",
        "/api/admin/provider-query/test-model-failover",
    );

    assert_eq!(body["model"], json!("new-image-model"));
}

#[test]
fn provider_query_grok_image_test_allows_multi_generation_count() {
    let request = http::Request::builder()
        .uri("/v1/images/generations")
        .body(())
        .expect("request should build");
    let (parts, _) = request.into_parts();
    let body = json!({
        "model": "grok-imagine-image",
        "prompt": "draw",
        "n": 2
    });

    let normalized = crate::ai_serving::normalize_openai_image_request_with_options(
        &parts,
        &body,
        None,
        provider_query_openai_image_normalize_options("grok", Some("grok-imagine-image")),
    )
    .expect("grok image model tests should allow multi-image generation");
    let provider_body = crate::ai_serving::build_openai_image_provider_request_body(&normalized);

    assert_eq!(provider_body["n"], json!(2));
}

#[test]
fn provider_query_grok_image_test_uses_grok_app_chat_upstream_url() {
    let transport = sample_openai_image_transport("grok");

    assert_eq!(
        provider_query_openai_image_test_upstream_url(
            &transport,
            Some("/v1/images/generations"),
            Some("trace=1"),
        ),
        "https://grok.com/rest/app-chat/conversations/new"
    );
}

#[test]
fn provider_query_custom_image_test_uses_images_upstream_url() {
    let transport = sample_openai_image_transport("custom");

    assert_eq!(
        provider_query_openai_image_test_upstream_url(
            &transport,
            Some("/v1/images/generations"),
            Some("trace=1"),
        ),
        "https://grok.com/v1/images/generations?trace=1"
    );
}

#[test]
fn provider_query_chatgpt_web_image_test_uses_internal_upstream_url() {
    let transport = sample_openai_image_transport("chatgpt_web");

    assert_eq!(
        provider_query_openai_image_test_upstream_url(
            &transport,
            Some("/v1/images/generations"),
            Some("trace=1"),
        ),
        "https://grok.com/__aether/chatgpt-web-image"
    );
}

#[test]
fn provider_query_non_grok_image_test_keeps_single_generation_boundary() {
    let request = http::Request::builder()
        .uri("/v1/images/generations")
        .body(())
        .expect("request should build");
    let (parts, _) = request.into_parts();
    let body = json!({
        "model": "gpt-image-2",
        "prompt": "draw",
        "n": 2
    });

    assert!(
        crate::ai_serving::normalize_openai_image_request_with_options(
            &parts,
            &body,
            None,
            provider_query_openai_image_normalize_options("chatgpt_web", Some("gpt-image-2")),
        )
        .is_none()
    );
    assert_eq!(
        provider_query_openai_image_normalize_failure_message(
            "chatgpt_web",
            Some("gpt-image-2"),
            &body,
        ),
        "Provider request body could not be normalized for openai:image: selected provider supports n=1..1 for generation"
    );
}

#[test]
fn provider_query_dall_e_3_image_test_keeps_single_generation_boundary() {
    let request = http::Request::builder()
        .uri("/v1/images/generations")
        .body(())
        .expect("request should build");
    let (parts, _) = request.into_parts();
    let body = json!({
        "model": "dall-e-3",
        "prompt": "draw",
        "n": 2
    });

    assert!(
        crate::ai_serving::normalize_openai_image_request_with_options(
            &parts,
            &body,
            None,
            provider_query_openai_image_normalize_options("openai", Some("dall-e-3")),
        )
        .is_none()
    );
    assert_eq!(
        provider_query_openai_image_normalize_failure_message(
            "openai",
            Some("dall-e-3"),
            &body,
        ),
        "Provider request body could not be normalized for openai:image: selected provider supports n=1..1 for generation"
    );
}
