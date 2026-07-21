use super::{
    any, build_router_with_state, build_state_with_execution_runtime_override, json, start_server,
    to_bytes, Arc, Body, Json, Mutex, Request, Router, StatusCode,
    EXECUTION_PATH_EXECUTION_RUNTIME_SYNC, EXECUTION_PATH_HEADER, TRACE_ID_HEADER,
};
use crate::data::GatewayDataState;
use aether_crypto::{encrypt_python_fernet_plaintext, DEVELOPMENT_ENCRYPTION_KEY};
use aether_data::repository::auth::{
    InMemoryAuthApiKeySnapshotRepository, StoredAuthApiKeyExportRecord, StoredAuthApiKeySnapshot,
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

const ORIGINAL_EMAIL: &str = "alice@example.com";

fn run_async_test_on_large_stack<F>(name: &'static str, future: F)
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    let handle = std::thread::Builder::new()
        .name(name.to_string())
        .stack_size(16 * 1024 * 1024)
        .spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime should build")
                .block_on(future);
        })
        .expect("large-stack pii redaction format test thread should spawn");

    if let Err(payload) = handle.join() {
        std::panic::resume_unwind(payload);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StandardFormat {
    OpenAiChat,
    OpenAiResponses,
    ClaudeMessages,
}

#[derive(Debug, Clone)]
struct SeenExecutionRuntimeSyncRequest {
    body: serde_json::Value,
    headers: serde_json::Value,
    url: String,
}

struct RedactionFormatCase {
    test_id: &'static str,
    trace_id: &'static str,
    client_format: StandardFormat,
    provider_format: StandardFormat,
}

impl StandardFormat {
    fn api_format(self) -> &'static str {
        match self {
            Self::OpenAiChat => "openai:chat",
            Self::OpenAiResponses => "openai:responses",
            Self::ClaudeMessages => "claude:messages",
        }
    }

    fn provider_name(self) -> &'static str {
        match self {
            Self::OpenAiChat | Self::OpenAiResponses => "openai",
            Self::ClaudeMessages => "claude",
        }
    }

    fn endpoint_kind(self) -> &'static str {
        match self {
            Self::OpenAiChat | Self::ClaudeMessages => "chat",
            Self::OpenAiResponses => "cli",
        }
    }

    fn client_path(self) -> &'static str {
        match self {
            Self::OpenAiChat => "/v1/chat/completions",
            Self::OpenAiResponses => "/v1/responses",
            Self::ClaudeMessages => "/v1/messages",
        }
    }

    fn upstream_base_url(self) -> &'static str {
        match self {
            Self::OpenAiChat | Self::OpenAiResponses => "https://api.openai.example",
            Self::ClaudeMessages => "https://api.anthropic.example",
        }
    }

    fn upstream_path(self) -> &'static str {
        match self {
            Self::OpenAiChat => "/custom/v1/chat/completions",
            Self::OpenAiResponses => "/custom/v1/responses",
            Self::ClaudeMessages => "/custom/v1/messages",
        }
    }

    fn client_model(self) -> &'static str {
        match self {
            Self::OpenAiChat | Self::OpenAiResponses => "gpt-5",
            Self::ClaudeMessages => "claude-sonnet-4-5",
        }
    }

    fn provider_model(self) -> &'static str {
        match self {
            Self::OpenAiChat | Self::OpenAiResponses => "gpt-5-upstream",
            Self::ClaudeMessages => "claude-sonnet-4-5-upstream",
        }
    }

    fn provider_auth_type(self) -> &'static str {
        match self {
            Self::OpenAiChat | Self::OpenAiResponses => "api_key",
            Self::ClaudeMessages => "api_key",
        }
    }

    fn client_request_body(self) -> serde_json::Value {
        match self {
            Self::OpenAiChat => json!({
                "model": self.client_model(),
                "messages": [
                    {"role": "system", "content": "Keep answers short."},
                    {"role": "user", "content": format!("Please contact {ORIGINAL_EMAIL}")}
                ]
            }),
            Self::OpenAiResponses => json!({
                "model": self.client_model(),
                "instructions": format!("Never expose {ORIGINAL_EMAIL}."),
                "input": [{
                    "type": "message",
                    "role": "user",
                    "content": [{
                        "type": "input_text",
                        "text": format!("Send a status update to {ORIGINAL_EMAIL}")
                    }]
                }],
                "store": false
            }),
            Self::ClaudeMessages => json!({
                "model": self.client_model(),
                "system": format!("The private contact is {ORIGINAL_EMAIL}."),
                "messages": [{
                    "role": "user",
                    "content": [{
                        "type": "text",
                        "text": format!("Draft a reply for {ORIGINAL_EMAIL}")
                    }]
                }],
                "max_tokens": 64
            }),
        }
    }

    fn execution_runtime_response_body(self, sentinel: &str) -> serde_json::Value {
        let restored_text = format!("restored {sentinel}");
        match self {
            Self::OpenAiChat => json!({
                "id": "chatcmpl-redaction-format",
                "object": "chat.completion",
                "model": self.provider_model(),
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": restored_text},
                    "finish_reason": "stop"
                }],
                "usage": {
                    "prompt_tokens": 2,
                    "completion_tokens": 3,
                    "total_tokens": 5
                }
            }),
            Self::OpenAiResponses => json!({
                "id": "resp-redaction-format",
                "object": "response",
                "status": "completed",
                "model": self.provider_model(),
                "output": [{
                    "type": "message",
                    "id": "resp-redaction-format-msg",
                    "role": "assistant",
                    "status": "completed",
                    "content": [{
                        "type": "output_text",
                        "text": restored_text,
                        "annotations": []
                    }]
                }],
                "usage": {
                    "input_tokens": 2,
                    "output_tokens": 3,
                    "total_tokens": 5
                }
            }),
            Self::ClaudeMessages => json!({
                "id": "msg_redaction_format",
                "type": "message",
                "model": self.provider_model(),
                "role": "assistant",
                "content": [{"type": "text", "text": restored_text}],
                "stop_reason": "end_turn",
                "usage": {
                    "input_tokens": 2,
                    "output_tokens": 3
                }
            }),
        }
    }
}

#[test]
fn ai_execute_openai_responses_pii_redaction_round_trip_same_format() {
    run_async_test_on_large_stack(
        "ai_execute_openai_responses_pii_redaction_round_trip_same_format",
        async {
            let (response_json, seen) = run_redaction_format_case(RedactionFormatCase {
                test_id: "openai-responses-pii-redaction-same-format",
                trace_id: "trace-openai-responses-pii-redaction-same-format",
                client_format: StandardFormat::OpenAiResponses,
                provider_format: StandardFormat::OpenAiResponses,
            })
            .await;

            assert_provider_request_redacted(&seen, StandardFormat::OpenAiResponses);
            assert!(seen.body.get("input").is_some());
            assert_restored_response(&response_json, StandardFormat::OpenAiResponses);
        },
    );
}

#[test]
fn ai_execute_claude_messages_pii_redaction_round_trip_same_format() {
    run_async_test_on_large_stack(
        "ai_execute_claude_messages_pii_redaction_round_trip_same_format",
        async {
            let (response_json, seen) = run_redaction_format_case(RedactionFormatCase {
                test_id: "claude-messages-pii-redaction-same-format",
                trace_id: "trace-claude-messages-pii-redaction-same-format",
                client_format: StandardFormat::ClaudeMessages,
                provider_format: StandardFormat::ClaudeMessages,
            })
            .await;

            assert_provider_request_redacted(&seen, StandardFormat::ClaudeMessages);
            assert!(seen.body.get("messages").is_some());
            assert_restored_response(&response_json, StandardFormat::ClaudeMessages);
        },
    );
}

#[test]
fn ai_execute_openai_chat_pii_redaction_before_claude_conversion() {
    run_async_test_on_large_stack(
        "ai_execute_openai_chat_pii_redaction_before_claude_conversion",
        async {
            let (response_json, seen) = run_redaction_format_case(RedactionFormatCase {
                test_id: "openai-chat-pii-redaction-before-claude-conversion",
                trace_id: "trace-openai-chat-pii-redaction-before-claude-conversion",
                client_format: StandardFormat::OpenAiChat,
                provider_format: StandardFormat::ClaudeMessages,
            })
            .await;

            assert_provider_request_redacted(&seen, StandardFormat::ClaudeMessages);
            assert!(seen.body.get("messages").is_some());
            assert_eq!(
                seen.body["model"],
                StandardFormat::ClaudeMessages.provider_model()
            );
            assert_restored_response(&response_json, StandardFormat::OpenAiChat);
        },
    );
}

#[test]
fn ai_execute_openai_responses_pii_redaction_before_claude_conversion() {
    run_async_test_on_large_stack(
        "ai_execute_openai_responses_pii_redaction_before_claude_conversion",
        async {
            let (response_json, seen) = run_redaction_format_case(RedactionFormatCase {
                test_id: "openai-responses-pii-redaction-before-claude-conversion",
                trace_id: "trace-openai-responses-pii-redaction-before-claude-conversion",
                client_format: StandardFormat::OpenAiResponses,
                provider_format: StandardFormat::ClaudeMessages,
            })
            .await;

            assert_provider_request_redacted(&seen, StandardFormat::ClaudeMessages);
            assert!(seen.body.get("messages").is_some());
            assert_eq!(
                seen.body["model"],
                StandardFormat::ClaudeMessages.provider_model()
            );
            assert_restored_response(&response_json, StandardFormat::OpenAiResponses);
        },
    );
}

#[test]
fn ai_execute_claude_messages_pii_redaction_before_openai_chat_conversion() {
    run_async_test_on_large_stack(
        "ai_execute_claude_messages_pii_redaction_before_openai_chat_conversion",
        async {
            let (response_json, seen) = run_redaction_format_case(RedactionFormatCase {
                test_id: "claude-messages-pii-redaction-before-openai-chat-conversion",
                trace_id: "trace-claude-messages-pii-redaction-before-openai-chat-conversion",
                client_format: StandardFormat::ClaudeMessages,
                provider_format: StandardFormat::OpenAiChat,
            })
            .await;

            assert_provider_request_redacted(&seen, StandardFormat::OpenAiChat);
            assert!(seen.body.get("messages").is_some());
            assert_eq!(
                seen.body["model"],
                StandardFormat::OpenAiChat.provider_model()
            );
            assert_restored_response(&response_json, StandardFormat::ClaudeMessages);
        },
    );
}

async fn run_redaction_format_case(
    case: RedactionFormatCase,
) -> (serde_json::Value, SeenExecutionRuntimeSyncRequest) {
    let seen_execution_runtime = Arc::new(Mutex::new(None::<SeenExecutionRuntimeSyncRequest>));
    let seen_execution_runtime_clone = Arc::clone(&seen_execution_runtime);
    let provider_format = case.provider_format;
    let trace_id = case.trace_id.to_string();

    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |request: Request| {
            let seen_execution_runtime_inner = Arc::clone(&seen_execution_runtime_clone);
            let trace_id = trace_id.clone();
            async move {
                let (_parts, body) = request.into_parts();
                let raw_body = to_bytes(body, usize::MAX).await.expect("body should read");
                let payload: serde_json::Value = serde_json::from_slice(&raw_body)
                    .expect("execution runtime payload should parse");
                let provider_body = payload
                    .get("body")
                    .and_then(|value| value.get("json_body"))
                    .cloned()
                    .expect("json body should exist");
                let provider_body_text =
                    serde_json::to_string(&provider_body).expect("json body should serialize");
                let email_sentinel = collect_sentinels(&provider_body_text, "EMAIL")
                    .into_iter()
                    .next()
                    .expect("email sentinel should exist in provider body");
                let headers = payload.get("headers").cloned().unwrap_or_else(|| json!({}));
                let url = payload
                    .get("url")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .to_string();
                *seen_execution_runtime_inner
                    .lock()
                    .expect("mutex should lock") = Some(SeenExecutionRuntimeSyncRequest {
                    body: provider_body,
                    headers,
                    url,
                });

                Json(json!({
                    "request_id": trace_id,
                    "status_code": 200,
                    "headers": {
                        "content-type": "application/json"
                    },
                    "body": {
                        "json_body": provider_format.execution_runtime_response_body(&email_sentinel)
                    },
                    "telemetry": {
                        "elapsed_ms": 19
                    }
                }))
            }
        }),
    );

    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
    let auth_repository = auth_repository(&case);
    let candidate_selection_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            candidate_row(&case),
        ]));
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider(&case)],
        vec![endpoint(&case)],
        vec![key(&case)],
    ));

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let gateway_state = build_state_with_execution_runtime_override(execution_runtime_url.clone())
        .with_data_state_for_tests(
            GatewayDataState::with_auth_candidate_selection_provider_catalog_and_request_candidate_repository_for_tests(
                auth_repository,
                candidate_selection_repository,
                provider_catalog_repository,
                Arc::clone(&request_candidate_repository),
                DEVELOPMENT_ENCRYPTION_KEY,
            )
            .with_system_config_values_for_tests(redaction_config()),
        );
    let gateway = build_router_with_state(gateway_state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let client = reqwest::Client::new();
    let mut request = client
        .post(format!("{gateway_url}{}", case.client_format.client_path()))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(http::header::ACCEPT_ENCODING, "gzip")
        .header(TRACE_ID_HEADER, case.trace_id)
        .body(case.client_format.client_request_body().to_string());
    request = match case.client_format {
        StandardFormat::OpenAiChat | StandardFormat::OpenAiResponses => request.header(
            http::header::AUTHORIZATION,
            format!("Bearer {}", client_api_key(&case)),
        ),
        StandardFormat::ClaudeMessages => request
            .header("x-api-key", client_api_key(&case))
            .header("anthropic-version", "2023-06-01"),
    };
    let response = request.send().await.expect("request should succeed");
    let status = response.status();
    let execution_path = response
        .headers()
        .get(EXECUTION_PATH_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let response_text = response.text().await.expect("response body should read");
    assert_eq!(status, StatusCode::OK, "{response_text}");
    assert_eq!(
        execution_path.as_deref(),
        Some(EXECUTION_PATH_EXECUTION_RUNTIME_SYNC)
    );
    let response_json: serde_json::Value =
        serde_json::from_str(&response_text).expect("response body should parse");

    let mut stored_candidates = Vec::new();
    for _ in 0..50 {
        stored_candidates = request_candidate_repository
            .list_by_request_id(case.trace_id)
            .await
            .expect("request candidate trace should read");
        if stored_candidates.len() == 1
            && stored_candidates[0].status == RequestCandidateStatus::Success
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(stored_candidates.len(), 1);
    assert_eq!(stored_candidates[0].status, RequestCandidateStatus::Success);

    let seen = seen_execution_runtime
        .lock()
        .expect("mutex should lock")
        .clone()
        .expect("execution runtime request should be captured");

    gateway_handle.abort();
    execution_runtime_handle.abort();

    (response_json, seen)
}

fn auth_repository(case: &RedactionFormatCase) -> Arc<InMemoryAuthApiKeySnapshotRepository> {
    let snapshot = auth_snapshot(case);
    let key_hash = hash_api_key(&client_api_key(case));
    Arc::new(
        InMemoryAuthApiKeySnapshotRepository::seed(vec![(
            Some(key_hash.clone()),
            snapshot.clone(),
        )])
        .with_export_records(vec![auth_export_record(
            &snapshot,
            key_hash,
            Some(json!({
                "chat_pii_redaction": {
                    "enabled": true
                }
            })),
        )]),
    )
}

fn auth_snapshot(case: &RedactionFormatCase) -> StoredAuthApiKeySnapshot {
    let allowed_providers = unique_json_array([
        case.client_format.provider_name(),
        case.provider_format.provider_name(),
    ]);
    StoredAuthApiKeySnapshot::new(
        format!("user-{}", case.test_id),
        "alice".to_string(),
        Some("alice@example.com".to_string()),
        "user".to_string(),
        "local".to_string(),
        true,
        false,
        Some(allowed_providers.clone()),
        Some(json!([case.client_format.api_format()])),
        Some(json!([case.client_format.client_model()])),
        format!("api-key-{}", case.test_id),
        Some("default".to_string()),
        true,
        false,
        false,
        Some(60),
        Some(5),
        Some(4_102_444_800),
        Some(allowed_providers),
        Some(json!([case.client_format.api_format()])),
        Some(json!([case.client_format.client_model()])),
    )
    .expect("auth snapshot should build")
}

fn auth_export_record(
    snapshot: &StoredAuthApiKeySnapshot,
    key_hash: String,
    feature_settings: Option<serde_json::Value>,
) -> StoredAuthApiKeyExportRecord {
    StoredAuthApiKeyExportRecord::new(
        snapshot.user_id.clone(),
        snapshot.api_key_id.clone(),
        key_hash,
        None,
        snapshot.api_key_name.clone(),
        snapshot
            .api_key_allowed_providers
            .as_ref()
            .map(|value| serde_json::json!(value)),
        snapshot
            .api_key_allowed_api_formats
            .as_ref()
            .map(|value| serde_json::json!(value)),
        snapshot
            .api_key_allowed_models
            .as_ref()
            .map(|value| serde_json::json!(value)),
        snapshot.api_key_rate_limit,
        snapshot.api_key_concurrent_limit,
        None,
        snapshot.api_key_is_active,
        snapshot
            .api_key_expires_at_unix_secs
            .map(|value| value as i64),
        false,
        0,
        0,
        0.0,
        snapshot.api_key_is_standalone,
    )
    .expect("auth api key export record should build")
    .with_feature_settings(feature_settings)
}

fn candidate_row(case: &RedactionFormatCase) -> StoredMinimalCandidateSelectionRow {
    StoredMinimalCandidateSelectionRow {
        provider_id: format!("provider-{}", case.test_id),
        provider_name: case.provider_format.provider_name().to_string(),
        provider_type: "custom".to_string(),
        provider_priority: 10,
        provider_is_active: true,
        endpoint_id: format!("endpoint-{}", case.test_id),
        endpoint_api_format: case.provider_format.api_format().to_string(),
        endpoint_api_family: Some(case.provider_format.provider_name().to_string()),
        endpoint_kind: Some(case.provider_format.endpoint_kind().to_string()),
        endpoint_is_active: true,
        key_id: format!("key-{}", case.test_id),
        key_name: "prod".to_string(),
        key_auth_type: case.provider_format.provider_auth_type().to_string(),
        key_is_active: true,
        key_api_formats: Some(vec![case.provider_format.api_format().to_string()]),
        key_allowed_models: None,
        key_capabilities: None,
        key_internal_priority: 5,
        key_global_priority_by_format: Some(json!({case.provider_format.api_format(): 1})),
        model_id: format!("model-{}", case.test_id),
        global_model_id: format!("global-model-{}", case.test_id),
        global_model_name: case.client_format.client_model().to_string(),
        global_model_mappings: None,
        global_model_supports_streaming: Some(true),
        model_provider_model_name: case.provider_format.provider_model().to_string(),
        model_provider_model_mappings: Some(vec![StoredProviderModelMapping {
            name: case.provider_format.provider_model().to_string(),
            priority: 1,
            api_formats: Some(vec![case.provider_format.api_format().to_string()]),
            endpoint_ids: Some(vec![format!("endpoint-{}", case.test_id)]),
            operations: None,
        }]),
        model_supports_streaming: Some(true),
        model_is_active: true,
        model_is_available: true,
    }
}

fn provider(case: &RedactionFormatCase) -> StoredProviderCatalogProvider {
    StoredProviderCatalogProvider::new(
        format!("provider-{}", case.test_id),
        case.provider_format.provider_name().to_string(),
        Some("https://example.com".to_string()),
        "custom".to_string(),
    )
    .expect("provider should build")
    .with_transport_fields(
        true,
        false,
        case.client_format != case.provider_format,
        None,
        Some(2),
        None,
        Some(20.0),
        None,
        None,
    )
}

fn endpoint(case: &RedactionFormatCase) -> StoredProviderCatalogEndpoint {
    StoredProviderCatalogEndpoint::new(
        format!("endpoint-{}", case.test_id),
        format!("provider-{}", case.test_id),
        case.provider_format.api_format().to_string(),
        Some(case.provider_format.provider_name().to_string()),
        Some(case.provider_format.endpoint_kind().to_string()),
        true,
    )
    .expect("endpoint should build")
    .with_transport_fields(
        case.provider_format.upstream_base_url().to_string(),
        None,
        None,
        Some(2),
        Some(case.provider_format.upstream_path().to_string()),
        None,
        None,
        None,
    )
    .expect("endpoint transport should build")
}

fn key(case: &RedactionFormatCase) -> StoredProviderCatalogKey {
    StoredProviderCatalogKey::new(
        format!("key-{}", case.test_id),
        format!("provider-{}", case.test_id),
        "prod".to_string(),
        case.provider_format.provider_auth_type().to_string(),
        None,
        true,
    )
    .expect("key should build")
    .with_transport_fields(
        Some(json!([case.provider_format.api_format()])),
        encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            &format!("sk-upstream-{}", case.test_id),
        )
        .expect("api key should encrypt"),
        None,
        None,
        Some(json!({case.provider_format.api_format(): 1})),
        None,
        None,
        None,
        None,
    )
    .expect("key transport should build")
}

fn redaction_config() -> Vec<(String, serde_json::Value)> {
    vec![
        ("module.chat_pii_redaction.enabled".to_string(), json!(true)),
        (
            "module.chat_pii_redaction.rules".to_string(),
            json!([{
                "id": "email",
                "name": "邮箱",
                "pattern": r"(?i)[A-Z0-9._%+-]{1,64}@[A-Z0-9.-]{1,253}\.[A-Z]{2,63}",
                "enabled": true,
                "features": {"validator": "email"},
                "system": true
            }]),
        ),
        (
            "module.chat_pii_redaction.cache_ttl_seconds".to_string(),
            json!(300),
        ),
    ]
}

fn assert_provider_request_redacted(
    seen: &SeenExecutionRuntimeSyncRequest,
    provider_format: StandardFormat,
) {
    let provider_body_text = serde_json::to_string(&seen.body).expect("body should serialize");
    assert!(
        provider_body_text.contains("<AETHER:EMAIL:"),
        "provider body was not redacted: {provider_body_text}"
    );
    assert!(
        !provider_body_text.contains(ORIGINAL_EMAIL),
        "provider body leaked original email: {provider_body_text}"
    );
    assert_eq!(seen.headers["accept-encoding"], "identity");
    assert!(
        seen.url.ends_with(provider_format.upstream_path()),
        "unexpected provider url {}",
        seen.url
    );
}

fn assert_restored_response(response_json: &serde_json::Value, client_format: StandardFormat) {
    match client_format {
        StandardFormat::OpenAiChat => assert!(response_json["choices"].is_array()),
        StandardFormat::ClaudeMessages => assert_eq!(response_json["type"], "message"),
        StandardFormat::OpenAiResponses => {}
    }
    let mut strings = Vec::new();
    collect_json_strings(response_json, &mut strings);
    assert!(
        strings.iter().any(|value| value.contains(ORIGINAL_EMAIL)),
        "client response did not restore original email: {response_json}"
    );
    assert!(
        strings.iter().all(|value| !value.contains("<AETHER:")),
        "client response still contains redaction sentinel: {response_json}"
    );
}

fn collect_json_strings<'a>(value: &'a serde_json::Value, strings: &mut Vec<&'a str>) {
    match value {
        serde_json::Value::String(value) => strings.push(value),
        serde_json::Value::Array(items) => {
            for item in items {
                collect_json_strings(item, strings);
            }
        }
        serde_json::Value::Object(map) => {
            for value in map.values() {
                collect_json_strings(value, strings);
            }
        }
        _ => {}
    }
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

fn hash_api_key(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn client_api_key(case: &RedactionFormatCase) -> String {
    format!("sk-client-{}", case.test_id)
}

fn unique_json_array(values: [&str; 2]) -> serde_json::Value {
    let mut unique = Vec::new();
    for value in values {
        if !unique.contains(&value) {
            unique.push(value);
        }
    }
    json!(unique)
}
