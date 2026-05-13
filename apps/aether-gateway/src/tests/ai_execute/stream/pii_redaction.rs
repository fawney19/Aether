use super::*;
use aether_crypto::{encrypt_python_fernet_plaintext, DEVELOPMENT_ENCRYPTION_KEY};
use aether_data::repository::auth::{
    InMemoryAuthApiKeySnapshotRepository, StoredAuthApiKeySnapshot,
};
use aether_data::repository::candidate_selection::InMemoryMinimalCandidateSelectionReadRepository;
use aether_data::repository::candidates::InMemoryRequestCandidateRepository;
use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
use aether_data_contracts::repository::candidate_selection::{
    StoredMinimalCandidateSelectionRow, StoredProviderModelMapping,
};
use aether_data_contracts::repository::candidates::{
    RequestCandidateReadRepository, RequestCandidateStatus,
};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone)]
struct SeenProviderStreamRequest {
    body: serde_json::Value,
    authorization: String,
    accept_encoding: String,
    accept: String,
}

fn hash_api_key(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn auth_snapshot() -> StoredAuthApiKeySnapshot {
    StoredAuthApiKeySnapshot::new(
        "user-ai-execute-stream-pii-redaction".to_string(),
        "alice".to_string(),
        Some("alice@example.com".to_string()),
        "user".to_string(),
        "local".to_string(),
        true,
        false,
        Some(serde_json::json!(["openai"])),
        Some(serde_json::json!(["openai:chat"])),
        Some(serde_json::json!(["gpt-5"])),
        "api-key-ai-execute-stream-pii-redaction".to_string(),
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

fn candidate_row() -> StoredMinimalCandidateSelectionRow {
    StoredMinimalCandidateSelectionRow {
        provider_id: "provider-ai-execute-stream-pii-redaction".to_string(),
        provider_name: "openai".to_string(),
        provider_type: "custom".to_string(),
        provider_priority: 10,
        provider_is_active: true,
        endpoint_id: "endpoint-ai-execute-stream-pii-redaction".to_string(),
        endpoint_api_format: "openai:chat".to_string(),
        endpoint_api_family: Some("openai".to_string()),
        endpoint_kind: Some("chat".to_string()),
        endpoint_is_active: true,
        key_id: "key-ai-execute-stream-pii-redaction".to_string(),
        key_name: "prod".to_string(),
        key_auth_type: "api_key".to_string(),
        key_is_active: true,
        key_api_formats: Some(vec!["openai:chat".to_string()]),
        key_allowed_models: None,
        key_capabilities: None,
        key_internal_priority: 5,
        key_global_priority_by_format: Some(serde_json::json!({"openai:chat": 1})),
        model_id: "model-ai-execute-stream-pii-redaction".to_string(),
        global_model_id: "global-model-ai-execute-stream-pii-redaction".to_string(),
        global_model_name: "gpt-5".to_string(),
        global_model_mappings: None,
        global_model_supports_streaming: Some(true),
        model_provider_model_name: "gpt-5-upstream".to_string(),
        model_provider_model_mappings: Some(vec![StoredProviderModelMapping {
            name: "gpt-5-upstream".to_string(),
            priority: 1,
            api_formats: Some(vec!["openai:chat".to_string()]),
            endpoint_ids: Some(vec!["endpoint-ai-execute-stream-pii-redaction".to_string()]),
        }]),
        model_supports_streaming: Some(true),
        model_is_active: true,
        model_is_available: true,
    }
}

fn provider() -> StoredProviderCatalogProvider {
    StoredProviderCatalogProvider::new(
        "provider-ai-execute-stream-pii-redaction".to_string(),
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
        Some(serde_json::json!({"chat_pii_redaction": {"enabled": true}})),
    )
}

fn endpoint(base_url: String) -> StoredProviderCatalogEndpoint {
    StoredProviderCatalogEndpoint::new(
        "endpoint-ai-execute-stream-pii-redaction".to_string(),
        "provider-ai-execute-stream-pii-redaction".to_string(),
        "openai:chat".to_string(),
        Some("openai".to_string()),
        Some("chat".to_string()),
        true,
    )
    .expect("endpoint should build")
    .with_transport_fields(base_url, None, None, Some(2), None, None, None, None)
    .expect("endpoint transport should build")
}

fn key() -> StoredProviderCatalogKey {
    StoredProviderCatalogKey::new(
        "key-ai-execute-stream-pii-redaction".to_string(),
        "provider-ai-execute-stream-pii-redaction".to_string(),
        "prod".to_string(),
        "api_key".to_string(),
        None,
        true,
    )
    .expect("key should build")
    .with_transport_fields(
        Some(serde_json::json!(["openai:chat"])),
        encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "sk-upstream-stream-pii")
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

fn collect_email_sentinel(text: &str) -> String {
    let start = text
        .find("<AETHER:EMAIL:")
        .expect("email sentinel should exist");
    let end = text[start..]
        .find('>')
        .map(|index| start + index + 1)
        .expect("sentinel should close");
    text[start..end].to_string()
}

#[tokio::test]
async fn ai_execute_stream_pii_redaction_round_trip() {
    let seen_provider_request = Arc::new(Mutex::new(None::<SeenProviderStreamRequest>));
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
                let sentinel = collect_email_sentinel(&payload_text);
                *seen_provider_request_inner
                    .lock()
                    .expect("mutex should lock") = Some(SeenProviderStreamRequest {
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
                    accept: parts
                        .headers
                        .get(http::header::ACCEPT)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                });

                let split_at = sentinel.len() / 2;
                let (first_half, second_half) = sentinel.split_at(split_at);
                let first_chunk = format!(
                    "data: {{\"id\":\"chatcmpl-stream-pii\",\"object\":\"chat.completion.chunk\",\"model\":\"gpt-5-upstream\",\"choices\":[{{\"index\":0,\"delta\":{{\"role\":\"assistant\",\"content\":\"stream {first_half}"
                );
                let second_chunk = format!(
                    "{second_half} restored\"}},\"finish_reason\":null}}]}}\n\n"
                );
                let stream = futures_util::stream::iter([
                    Ok::<_, Infallible>(Bytes::from(first_chunk)),
                    Ok::<_, Infallible>(Bytes::from(second_chunk)),
                    Ok::<_, Infallible>(Bytes::from_static(b"data: [DONE]\n\n")),
                ]);
                let mut response = Response::builder()
                    .status(StatusCode::OK)
                    .body(Body::from_stream(stream))
                    .expect("response should build");
                response.headers_mut().insert(
                    http::header::CONTENT_TYPE,
                    HeaderValue::from_static("text/event-stream"),
                );
                response
            }
        }),
    );
    let (provider_url, provider_handle) = start_server(provider_app).await;
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-client-ai-execute-stream-pii-redaction")),
        auth_snapshot(),
    )]));
    let candidate_selection_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            candidate_row(),
        ]));
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider()],
        vec![endpoint(provider_url)],
        vec![key()],
    ));
    let data_state = crate::data::GatewayDataState::with_auth_candidate_selection_provider_catalog_and_request_candidate_repository_for_tests(
        auth_repository,
        candidate_selection_repository,
        provider_catalog_repository,
        Arc::clone(&request_candidate_repository),
        DEVELOPMENT_ENCRYPTION_KEY,
    )
    .with_system_config_values_for_tests(vec![
        ("module.chat_pii_redaction.enabled".to_string(), json!(true)),
        (
            "module.chat_pii_redaction.provider_scope".to_string(),
            json!("selected_providers"),
        ),
        (
            "module.chat_pii_redaction.entities".to_string(),
            json!(["email"]),
        ),
        (
            "module.chat_pii_redaction.cache_ttl_seconds".to_string(),
            json!(300),
        ),
        (
            "module.chat_pii_redaction.inject_model_instruction".to_string(),
            json!(true),
        ),
    ]);
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
            "Bearer sk-client-ai-execute-stream-pii-redaction",
        )
        .header(http::header::ACCEPT_ENCODING, "gzip")
        .header(TRACE_ID_HEADER, "trace-ai-execute-stream-pii-redaction")
        .body(
            json!({
                "model": "gpt-5",
                "messages": [{"role": "user", "content": "Email stream.user@example.com"}],
                "stream": true
            })
            .to_string(),
        )
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
        Some(EXECUTION_PATH_EXECUTION_RUNTIME_STREAM)
    );
    assert!(response_text.contains("stream stream.user@example.com restored"));
    assert!(response_text.contains("data: [DONE]"));
    assert!(!response_text.contains("<AETHER:EMAIL:"));

    let seen = seen_provider_request
        .lock()
        .expect("mutex should lock")
        .clone()
        .expect("provider stream request should be captured");
    assert_eq!(seen.authorization, "Bearer sk-upstream-stream-pii");
    assert_eq!(seen.accept, "text/event-stream");
    assert_eq!(seen.accept_encoding, "identity");
    let provider_body_text = serde_json::to_string(&seen.body).expect("body should serialize");
    assert!(!provider_body_text.contains("stream.user@example.com"));
    assert!(provider_body_text.contains("<AETHER:EMAIL:"));

    let stored_candidates = request_candidate_repository
        .list_by_request_id("trace-ai-execute-stream-pii-redaction")
        .await
        .expect("request candidate trace should read");
    assert_eq!(stored_candidates.len(), 1);
    assert_eq!(stored_candidates[0].status, RequestCandidateStatus::Success);

    gateway_handle.abort();
    provider_handle.abort();
}
