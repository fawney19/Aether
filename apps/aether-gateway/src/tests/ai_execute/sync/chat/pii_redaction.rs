use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Debug, Clone)]
struct SeenProviderRequest {
    body: serde_json::Value,
    authorization: String,
    accept_encoding: String,
}

fn hash_api_key(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn auth_snapshot(api_key_id: &str, user_id: &str) -> StoredAuthApiKeySnapshot {
    StoredAuthApiKeySnapshot::new(
        user_id.to_string(),
        "alice".to_string(),
        Some("alice@example.com".to_string()),
        "user".to_string(),
        "local".to_string(),
        true,
        false,
        Some(serde_json::json!(["openai"])),
        Some(serde_json::json!(["openai:chat"])),
        Some(serde_json::json!(["gpt-5"])),
        api_key_id.to_string(),
        Some("default".to_string()),
        true,
        false,
        false,
        Some(60),
        Some(5),
        Some(4_102_444_800),
        Some(serde_json::json!(["openai"])),
        Some(serde_json::json!(["openai:chat"])),
        Some(serde_json::json!(["gpt-5"])),
    )
    .expect("auth snapshot should build")
}

fn candidate_row(test_id: &str) -> StoredMinimalCandidateSelectionRow {
    StoredMinimalCandidateSelectionRow {
        provider_id: format!("provider-{test_id}"),
        provider_name: "openai".to_string(),
        provider_type: "custom".to_string(),
        provider_priority: 10,
        provider_is_active: true,
        endpoint_id: format!("endpoint-{test_id}"),
        endpoint_api_format: "openai:chat".to_string(),
        endpoint_api_family: Some("openai".to_string()),
        endpoint_kind: Some("chat".to_string()),
        endpoint_is_active: true,
        key_id: format!("key-{test_id}"),
        key_name: "prod".to_string(),
        key_auth_type: "api_key".to_string(),
        key_is_active: true,
        key_api_formats: Some(vec!["openai:chat".to_string()]),
        key_allowed_models: None,
        key_capabilities: None,
        key_internal_priority: 5,
        key_global_priority_by_format: Some(serde_json::json!({"openai:chat": 1})),
        model_id: format!("model-{test_id}"),
        global_model_id: format!("global-model-{test_id}"),
        global_model_name: "gpt-5".to_string(),
        global_model_mappings: None,
        global_model_supports_streaming: Some(true),
        model_provider_model_name: "gpt-5-upstream".to_string(),
        model_provider_model_mappings: Some(vec![StoredProviderModelMapping {
            name: "gpt-5-upstream".to_string(),
            priority: 1,
            api_formats: Some(vec!["openai:chat".to_string()]),
            endpoint_ids: Some(vec![format!("endpoint-{test_id}")]),
        }]),
        model_supports_streaming: Some(true),
        model_is_active: true,
        model_is_available: true,
    }
}

fn provider(test_id: &str, redaction_enabled: bool) -> StoredProviderCatalogProvider {
    StoredProviderCatalogProvider::new(
        format!("provider-{test_id}"),
        "openai".to_string(),
        Some("https://example.com".to_string()),
        "custom".to_string(),
    )
    .expect("provider should build")
    .with_transport_fields(
        true,
        false,
        true,
        None,
        Some(2),
        None,
        Some(20.0),
        None,
        Some(serde_json::json!({"chat_pii_redaction": {"enabled": redaction_enabled}})),
    )
}

fn endpoint(test_id: &str, base_url: String) -> StoredProviderCatalogEndpoint {
    StoredProviderCatalogEndpoint::new(
        format!("endpoint-{test_id}"),
        format!("provider-{test_id}"),
        "openai:chat".to_string(),
        Some("openai".to_string()),
        Some("chat".to_string()),
        true,
    )
    .expect("endpoint should build")
    .with_transport_fields(base_url, None, None, Some(2), None, None, None, None)
    .expect("endpoint transport should build")
}

fn key(test_id: &str) -> StoredProviderCatalogKey {
    StoredProviderCatalogKey::new(
        format!("key-{test_id}"),
        format!("provider-{test_id}"),
        "prod".to_string(),
        "api_key".to_string(),
        None,
        true,
    )
    .expect("key should build")
    .with_transport_fields(
        Some(serde_json::json!(["openai:chat"])),
        encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "sk-upstream-pii-redaction")
            .expect("api key should encrypt"),
        None,
        None,
        Some(serde_json::json!({"openai:chat": 1})),
        None,
        None,
        None,
        None,
    )
    .expect("key transport should build")
}

fn redaction_config(module_enabled: bool) -> Vec<(String, serde_json::Value)> {
    redaction_config_with_entities(
        module_enabled,
        json!(["email", "access_token", "secret_key"]),
    )
}

fn redaction_config_with_entities(
    module_enabled: bool,
    entities: serde_json::Value,
) -> Vec<(String, serde_json::Value)> {
    vec![
        (
            "module.chat_pii_redaction.enabled".to_string(),
            json!(module_enabled),
        ),
        (
            "module.chat_pii_redaction.provider_scope".to_string(),
            json!("selected_providers"),
        ),
        ("module.chat_pii_redaction.entities".to_string(), entities),
        (
            "module.chat_pii_redaction.cache_ttl_seconds".to_string(),
            json!(300),
        ),
        (
            "module.chat_pii_redaction.inject_model_instruction".to_string(),
            json!(true),
        ),
    ]
}

fn collect_sentinels(text: &str, kind: &str) -> Vec<String> {
    let prefix = format!("<AETHER:{kind}:");
    let mut sentinels = Vec::new();
    let mut offset = 0;
    while let Some(relative_start) = text[offset..].find(&prefix) {
        let start = offset + relative_start;
        let Some(relative_end) = text[start..].find('>') else {
            break;
        };
        let end = start + relative_end + 1;
        sentinels.push(text[start..end].to_string());
        offset = end;
    }
    sentinels
}

async fn run_sync_redaction_case(
    test_id: &str,
    module_enabled: bool,
    provider_enabled: bool,
    provider_response: &'static str,
    request_body: serde_json::Value,
) -> (serde_json::Value, SeenProviderRequest) {
    run_sync_redaction_case_with_system_config(
        test_id,
        provider_enabled,
        provider_response,
        request_body,
        redaction_config(module_enabled),
    )
    .await
}

async fn run_sync_redaction_case_with_system_config(
    test_id: &str,
    provider_enabled: bool,
    provider_response: &'static str,
    request_body: serde_json::Value,
    system_config: Vec<(String, serde_json::Value)>,
) -> (serde_json::Value, SeenProviderRequest) {
    let seen_provider_request = Arc::new(Mutex::new(None::<SeenProviderRequest>));
    let seen_provider_request_clone = Arc::clone(&seen_provider_request);
    let provider_app = Router::new().route(
        "/v1/chat/completions",
        any(move |request: Request| {
            let seen_provider_request_inner = Arc::clone(&seen_provider_request_clone);
            async move {
                let (parts, body) = request.into_parts();
                let raw_body = to_bytes(body, usize::MAX).await.expect("body should read");
                let payload: serde_json::Value =
                    serde_json::from_slice(&raw_body).expect("provider payload should parse");
                let payload_text =
                    serde_json::to_string(&payload).expect("payload should serialize");
                let email_sentinels = collect_sentinels(&payload_text, "EMAIL");
                let access_token_sentinels = collect_sentinels(&payload_text, "ACCESS_TOKEN");
                let secret_key_sentinels = collect_sentinels(&payload_text, "SECRET_KEY");
                *seen_provider_request_inner
                    .lock()
                    .expect("mutex should lock") = Some(SeenProviderRequest {
                    body: payload,
                    authorization: parts
                        .headers
                        .get(http::header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                    accept_encoding: parts
                        .headers
                        .get(http::header::ACCEPT_ENCODING)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                });

                let content = match provider_response {
                    "known" => format!(
                        "restored {} {} {}",
                        email_sentinels
                            .first()
                            .expect("user email sentinel should exist"),
                        access_token_sentinels
                            .first()
                            .expect("access token sentinel should exist"),
                        secret_key_sentinels
                            .first()
                            .expect("secret key sentinel should exist")
                    ),
                    "unknown" => "unknown <AETHER:EMAIL:TSRQPONMLKJIHGFEDCBA>".to_string(),
                    "pass_through" => "pass alice@example.com".to_string(),
                    _ => unreachable!("provider response mode should be known"),
                };

                Json(json!({
                    "id": format!("chatcmpl-{provider_response}"),
                    "object": "chat.completion",
                    "model": "gpt-5-upstream",
                    "choices": [{
                        "index": 0,
                        "message": {"role": "assistant", "content": content},
                        "finish_reason": "stop"
                    }],
                    "usage": {"prompt_tokens": 2, "completion_tokens": 3, "total_tokens": 5}
                }))
            }
        }),
    );
    let (provider_url, provider_handle) = start_server(provider_app).await;
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key(&format!("sk-client-{test_id}"))),
        auth_snapshot(&format!("api-key-{test_id}"), &format!("user-{test_id}")),
    )]));
    let candidate_selection_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            candidate_row(test_id),
        ]));
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider(test_id, provider_enabled)],
        vec![endpoint(test_id, provider_url)],
        vec![key(test_id)],
    ));
    let data_state = crate::data::GatewayDataState::with_auth_candidate_selection_provider_catalog_and_request_candidate_repository_for_tests(
        auth_repository,
        candidate_selection_repository,
        provider_catalog_repository,
        Arc::clone(&request_candidate_repository),
        DEVELOPMENT_ENCRYPTION_KEY,
    )
    .with_system_config_values_for_tests(system_config);
    let gateway_state = AppState::new()
        .expect("gateway state should build")
        .with_data_state_for_tests(data_state);
    let gateway = build_router_with_state(gateway_state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/chat/completions"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(
            http::header::AUTHORIZATION,
            format!("Bearer sk-client-{test_id}"),
        )
        .header(http::header::ACCEPT_ENCODING, "gzip")
        .header(TRACE_ID_HEADER, format!("trace-{test_id}"))
        .body(request_body.to_string())
        .send()
        .await
        .expect("request should succeed");

    let status = response.status();
    let execution_path = response
        .headers()
        .get(EXECUTION_PATH_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let response_text = response.text().await.expect("body should read");
    assert_eq!(status, StatusCode::OK, "{response_text}");
    assert_eq!(
        execution_path.as_deref(),
        Some(EXECUTION_PATH_EXECUTION_RUNTIME_SYNC)
    );
    let response_json: serde_json::Value =
        serde_json::from_str(&response_text).expect("response body should parse");

    let stored_candidates = request_candidate_repository
        .list_by_request_id(&format!("trace-{test_id}"))
        .await
        .expect("request candidate trace should read");
    assert_eq!(stored_candidates.len(), 1);
    assert_eq!(stored_candidates[0].status, RequestCandidateStatus::Success);

    let seen = seen_provider_request
        .lock()
        .expect("mutex should lock")
        .clone()
        .expect("provider request should be captured");

    gateway_handle.abort();
    provider_handle.abort();

    (response_json, seen)
}

fn rich_pii_request() -> serde_json::Value {
    json!({
        "model": "gpt-5",
        "messages": [
            {"role": "system", "content": "Be concise."},
            {
                "role": "user",
                "content": [
                    {"type": "text", "text": "Contact alice@example.com now"},
                    {"type": "input_audio", "input_audio": {"data": "AAAA", "format": "wav"}}
                ]
            },
            {
                "role": "assistant",
                "content": "Preparing lookup.",
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {
                        "name": "lookup_contact",
                        "arguments": "{\"email\":\"bob@example.net\"}"
                    }
                }]
            },
            {
                "role": "tool",
                "tool_call_id": "call_1",
                "content": "Tool returned access_token=accessValueABCDEF1234567890abcdef secret_key=secretValueABCDEF1234567890abcdef"
            }
        ],
        "tools": [{
            "type": "function",
            "function": {
                "name": "lookup_contact",
                "parameters": {
                    "type": "object",
                    "properties": {"email": {"type": "string"}}
                }
            }
        }]
    })
}

#[tokio::test]
async fn ai_execute_sync_pii_redaction_round_trip() {
    let (response_json, seen) = run_sync_redaction_case(
        "ai-execute-sync-pii-redaction-round-trip",
        true,
        true,
        "known",
        rich_pii_request(),
    )
    .await;

    assert_eq!(seen.authorization, "Bearer sk-upstream-pii-redaction");
    assert_eq!(seen.accept_encoding, "identity");
    let provider_body_text = serde_json::to_string(&seen.body).expect("body should serialize");
    for original in [
        "alice@example.com",
        "bob@example.net",
        "access_token=accessValueABCDEF1234567890abcdef",
        "secret_key\\\":\\\"secretValueABCDEF1234567890abcdef",
    ] {
        assert!(
            !provider_body_text.contains(original),
            "leaked {original} in {provider_body_text}"
        );
    }
    assert!(provider_body_text.contains("<AETHER:EMAIL:"));
    assert!(provider_body_text.contains("<AETHER:ACCESS_TOKEN:"));
    assert!(provider_body_text.contains("<AETHER:SECRET_KEY:"));
    assert_eq!(seen.body["messages"][0]["role"], "system");
    assert_eq!(seen.body["messages"][1]["role"], "assistant");
    let notice = seen.body["messages"][1]["content"]
        .as_str()
        .expect("notice should be text");
    assert!(notice.contains("not a user request"));
    assert_eq!(seen.body["messages"][2]["role"], "user");
    assert_eq!(seen.body["messages"][3]["role"], "assistant");
    assert_eq!(seen.body["messages"][4]["role"], "tool");

    let response_content = response_json["choices"][0]["message"]["content"]
        .as_str()
        .expect("assistant content should be text");
    assert!(response_content.contains("alice@example.com"));
    assert!(response_content.contains("access_token=accessValueABCDEF1234567890abcdef"));
    assert!(response_content.contains("secretValueABCDEF1234567890abcdef"));
    assert!(!response_content.contains("<AETHER:"));
}

#[tokio::test]
async fn ai_execute_pii_redaction_disabled_module_passes_original_chat_through() {
    let (response_json, seen) = run_sync_redaction_case(
        "ai-execute-pii-redaction-disabled-module",
        false,
        true,
        "pass_through",
        rich_pii_request(),
    )
    .await;

    let provider_body_text = serde_json::to_string(&seen.body).expect("body should serialize");
    assert!(provider_body_text.contains("alice@example.com"));
    assert!(provider_body_text.contains("bob@example.net"));
    assert!(provider_body_text.contains("access_token=accessValueABCDEF1234567890abcdef"));
    assert!(provider_body_text.contains("secretValueABCDEF1234567890abcdef"));
    assert!(!provider_body_text.contains("<AETHER:"));
    assert_eq!(
        response_json["choices"][0]["message"]["content"],
        "pass alice@example.com"
    );
}

#[tokio::test]
async fn ai_execute_pii_redaction_disabled_provider_passes_original_chat_through() {
    let (response_json, seen) = run_sync_redaction_case(
        "ai-execute-pii-redaction-disabled-provider",
        true,
        false,
        "pass_through",
        rich_pii_request(),
    )
    .await;

    let provider_body_text = serde_json::to_string(&seen.body).expect("body should serialize");
    assert!(provider_body_text.contains("alice@example.com"));
    assert!(provider_body_text.contains("bob@example.net"));
    assert!(provider_body_text.contains("access_token=accessValueABCDEF1234567890abcdef"));
    assert!(provider_body_text.contains("secretValueABCDEF1234567890abcdef"));
    assert!(!provider_body_text.contains("<AETHER:"));
    assert_eq!(
        response_json["choices"][0]["message"]["content"],
        "pass alice@example.com"
    );
}

#[tokio::test]
async fn ai_execute_pii_redaction_empty_entities_passes_original_chat_through() {
    let (response_json, seen) = run_sync_redaction_case_with_system_config(
        "ai-execute-pii-redaction-empty-entities",
        true,
        "pass_through",
        rich_pii_request(),
        redaction_config_with_entities(true, json!([])),
    )
    .await;

    let provider_body_text = serde_json::to_string(&seen.body).expect("body should serialize");
    assert!(provider_body_text.contains("alice@example.com"));
    assert!(provider_body_text.contains("bob@example.net"));
    assert!(provider_body_text.contains("access_token=accessValueABCDEF1234567890abcdef"));
    assert!(provider_body_text.contains("secretValueABCDEF1234567890abcdef"));
    assert!(!provider_body_text.contains("<AETHER:"));
    assert_eq!(
        response_json["choices"][0]["message"]["content"],
        "pass alice@example.com"
    );
}

#[tokio::test]
async fn ai_execute_pii_redaction_unknown_sentinel_like_output_is_not_restored() {
    let (response_json, seen) = run_sync_redaction_case(
        "ai-execute-pii-redaction-unknown-sentinel",
        true,
        true,
        "unknown",
        rich_pii_request(),
    )
    .await;

    let provider_body_text = serde_json::to_string(&seen.body).expect("body should serialize");
    assert!(!provider_body_text.contains("alice@example.com"));
    assert!(provider_body_text.contains("<AETHER:EMAIL:"));
    assert_eq!(
        response_json["choices"][0]["message"]["content"],
        "unknown <AETHER:EMAIL:TSRQPONMLKJIHGFEDCBA>"
    );
}

#[tokio::test]
async fn ai_execute_pii_redaction_restores_executed_candidate_session_after_later_candidate_planning(
) {
    let seen_provider_request = Arc::new(Mutex::new(None::<SeenProviderRequest>));
    let seen_provider_request_clone = Arc::clone(&seen_provider_request);
    let provider_app = Router::new().route(
        "/v1/chat/completions",
        any(move |request: Request| {
            let seen_provider_request_inner = Arc::clone(&seen_provider_request_clone);
            async move {
                let (parts, body) = request.into_parts();
                let raw_body = to_bytes(body, usize::MAX).await.expect("body should read");
                let payload: serde_json::Value =
                    serde_json::from_slice(&raw_body).expect("provider payload should parse");
                let payload_text = serde_json::to_string(&payload).expect("payload should serialize");
                let email_sentinel = collect_sentinels(&payload_text, "EMAIL")
                    .into_iter()
                    .next()
                    .expect("email sentinel should exist");
                *seen_provider_request_inner.lock().expect("mutex should lock") = Some(
                    SeenProviderRequest {
                        body: payload,
                        authorization: parts
                            .headers
                            .get(http::header::AUTHORIZATION)
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_string(),
                        accept_encoding: parts
                            .headers
                            .get(http::header::ACCEPT_ENCODING)
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_string(),
                    },
                );

                Json(json!({
                    "id": "chatcmpl-redaction-candidate-session",
                    "object": "chat.completion",
                    "model": "gpt-5-upstream",
                    "choices": [{
                        "index": 0,
                        "message": {"role": "assistant", "content": format!("restored {email_sentinel}")},
                        "finish_reason": "stop"
                    }],
                    "usage": {"prompt_tokens": 2, "completion_tokens": 3, "total_tokens": 5}
                }))
            }
        }),
    );
    let (provider_url, provider_handle) = start_server(provider_app).await;
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-client-redaction-candidate-session")),
        auth_snapshot(
            "api-key-redaction-candidate-session",
            "user-redaction-candidate-session",
        ),
    )]));
    let mut later_candidate = candidate_row("redaction-candidate-session");
    later_candidate.provider_id = "provider-redaction-candidate-session-later".to_string();
    later_candidate.endpoint_id = "endpoint-redaction-candidate-session-later".to_string();
    later_candidate.key_id = "key-redaction-candidate-session-later".to_string();
    later_candidate.provider_priority = 20;
    later_candidate.key_internal_priority = 6;
    later_candidate.model_id = "model-redaction-candidate-session-later".to_string();
    later_candidate.global_model_id = "global-model-redaction-candidate-session-later".to_string();
    let candidate_selection_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            candidate_row("redaction-candidate-session"),
            later_candidate,
        ]));
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
    let mut later_provider = provider("redaction-candidate-session", false);
    later_provider.id = "provider-redaction-candidate-session-later".to_string();
    let mut later_endpoint = endpoint(
        "redaction-candidate-session",
        "http://127.0.0.1:9".to_string(),
    );
    later_endpoint.id = "endpoint-redaction-candidate-session-later".to_string();
    later_endpoint.provider_id = "provider-redaction-candidate-session-later".to_string();
    let mut later_key = key("redaction-candidate-session");
    later_key.id = "key-redaction-candidate-session-later".to_string();
    later_key.provider_id = "provider-redaction-candidate-session-later".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![
            provider("redaction-candidate-session", true),
            later_provider,
        ],
        vec![
            endpoint("redaction-candidate-session", provider_url),
            later_endpoint,
        ],
        vec![key("redaction-candidate-session"), later_key],
    ));
    let data_state = crate::data::GatewayDataState::with_auth_candidate_selection_provider_catalog_and_request_candidate_repository_for_tests(
        auth_repository,
        candidate_selection_repository,
        provider_catalog_repository,
        Arc::clone(&request_candidate_repository),
        DEVELOPMENT_ENCRYPTION_KEY,
    )
    .with_system_config_values_for_tests(redaction_config(true));
    let gateway_state = AppState::new()
        .expect("gateway state should build")
        .with_data_state_for_tests(data_state);
    let gateway = build_router_with_state(gateway_state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/chat/completions"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(
            http::header::AUTHORIZATION,
            "Bearer sk-client-redaction-candidate-session",
        )
        .header(TRACE_ID_HEADER, "trace-redaction-candidate-session")
        .body(
            json!({
                "model": "gpt-5",
                "messages": [{"role": "user", "content": "Contact alice@example.com"}]
            })
            .to_string(),
        )
        .send()
        .await
        .expect("request should succeed");

    let status = response.status();
    let response_text = response.text().await.expect("body should read");
    assert_eq!(status, StatusCode::OK, "{response_text}");
    let response_json: serde_json::Value =
        serde_json::from_str(&response_text).expect("response body should parse");
    assert_eq!(
        response_json["choices"][0]["message"]["content"],
        "restored alice@example.com"
    );
    let seen = seen_provider_request
        .lock()
        .expect("mutex should lock")
        .clone()
        .expect("provider request should be captured");
    let provider_body_text = serde_json::to_string(&seen.body).expect("body should serialize");
    assert!(!provider_body_text.contains("alice@example.com"));
    assert!(provider_body_text.contains("<AETHER:EMAIL:"));

    gateway_handle.abort();
    provider_handle.abort();
}

#[tokio::test]
async fn pii_redaction_performance_limits_do_not_forward_unredacted_body_upstream() {
    let provider_hits = Arc::new(AtomicUsize::new(0));
    let provider_hits_clone = Arc::clone(&provider_hits);
    let provider_app = Router::new().route(
        "/v1/chat/completions",
        any(move |_request: Request| {
            let provider_hits_inner = Arc::clone(&provider_hits_clone);
            async move {
                provider_hits_inner.fetch_add(1, Ordering::SeqCst);
                Json(json!({
                    "id": "unexpected",
                    "choices": [{"message": {"role": "assistant", "content": "unexpected"}}]
                }))
            }
        }),
    );
    let (provider_url, provider_handle) = start_server(provider_app).await;
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-client-pii-redaction-limit")),
        auth_snapshot("api-key-pii-redaction-limit", "user-pii-redaction-limit"),
    )]));
    let candidate_selection_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            candidate_row("pii-redaction-limit"),
        ]));
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider("pii-redaction-limit", true)],
        vec![endpoint("pii-redaction-limit", provider_url)],
        vec![key("pii-redaction-limit")],
    ));
    let data_state = crate::data::GatewayDataState::with_auth_candidate_selection_provider_catalog_and_request_candidate_repository_for_tests(
        auth_repository,
        candidate_selection_repository,
        provider_catalog_repository,
        Arc::clone(&request_candidate_repository),
        DEVELOPMENT_ENCRYPTION_KEY,
    )
    .with_system_config_values_for_tests(redaction_config(true));
    let gateway_state = AppState::new()
        .expect("gateway state should build")
        .with_data_state_for_tests(data_state);
    let gateway = build_router_with_state(gateway_state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let original = format!("alice@example.com {}", "x".repeat(2 * 1024 * 1024));

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/chat/completions"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(
            http::header::AUTHORIZATION,
            "Bearer sk-client-pii-redaction-limit",
        )
        .header(TRACE_ID_HEADER, "trace-pii-redaction-limit")
        .body(
            json!({
                "model": "gpt-5",
                "messages": [{"role": "user", "content": original}]
            })
            .to_string(),
        )
        .send()
        .await
        .expect("request should complete");

    let status = response.status();
    let response_text = response.text().await.expect("body should read");
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{response_text}");
    assert!(response_text.contains("scanned text limit exceeded"));
    assert!(!response_text.contains("alice@example.com"));
    assert_eq!(provider_hits.load(Ordering::SeqCst), 0);

    gateway_handle.abort();
    provider_handle.abort();
}

#[tokio::test]
async fn ai_execute_pii_redaction_missing_encryption_key_fails_closed_before_provider() {
    let execution_runtime_hits = Arc::new(AtomicUsize::new(0));
    let execution_runtime_hits_clone = Arc::clone(&execution_runtime_hits);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |_request: Request| {
            let execution_runtime_hits_inner = Arc::clone(&execution_runtime_hits_clone);
            async move {
                execution_runtime_hits_inner.fetch_add(1, Ordering::SeqCst);
                Json(json!({"ok": true}))
            }
        }),
    );
    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let test_id = "ai-execute-pii-redaction-missing-encryption-key";
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key(&format!("sk-client-{test_id}"))),
        auth_snapshot(&format!("api-key-{test_id}"), &format!("user-{test_id}")),
    )]));
    let candidate_selection_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            candidate_row(test_id),
        ]));
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider(test_id, true)],
        vec![endpoint(test_id, "https://example.com".to_string())],
        vec![key(test_id)],
    ));
    let gateway_state = build_state_with_execution_runtime_override(execution_runtime_url.clone())
        .with_data_state_for_tests(
            crate::data::GatewayDataState::with_auth_candidate_selection_provider_catalog_and_request_candidate_repository_for_tests(
                auth_repository,
                candidate_selection_repository,
                provider_catalog_repository,
                Arc::clone(&request_candidate_repository),
                "",
            )
            .with_system_config_values_for_tests(redaction_config(true)),
        );
    let gateway = build_router_with_state(gateway_state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/chat/completions"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(
            http::header::AUTHORIZATION,
            format!("Bearer sk-client-{test_id}"),
        )
        .header(TRACE_ID_HEADER, format!("trace-{test_id}"))
        .body(rich_pii_request().to_string())
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let response_text = response.text().await.expect("body should read");
    for original in [
        "alice@example.com",
        "bob@example.net",
        "accessValueABCDEF1234567890abcdef",
        "secretValueABCDEF1234567890abcdef",
    ] {
        assert!(!response_text.contains(original));
    }
    assert!(!response_text.contains("<AETHER:"));
    assert_eq!(execution_runtime_hits.load(Ordering::SeqCst), 0);

    gateway_handle.abort();
    execution_runtime_handle.abort();
}
