use std::sync::Arc;

use aether_crypto::{encrypt_python_fernet_plaintext, DEVELOPMENT_ENCRYPTION_KEY};
use aether_data::repository::auth::{
    InMemoryAuthApiKeySnapshotRepository, StoredAuthApiKeySnapshot,
};
use aether_data::repository::candidate_selection::InMemoryMinimalCandidateSelectionReadRepository;
use aether_data::repository::candidates::InMemoryRequestCandidateRepository;
use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
use aether_data::repository::usage::InMemoryUsageReadRepository;
use aether_data_contracts::repository::candidate_selection::{
    StoredMinimalCandidateSelectionRow, StoredProviderModelMapping,
};
use aether_data_contracts::repository::candidates::{
    RequestCandidateStatus, StoredRequestCandidate,
};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use aether_data_contracts::repository::usage::{StoredRequestUsageAudit, UsageReadRepository};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{
    any, build_router_with_state, json, start_server, AppState, Json, Request, Router, StatusCode,
    UsageRuntimeConfig, CONTROL_CANDIDATE_ID_HEADER, CONTROL_REQUEST_ID_HEADER, TRACE_ID_HEADER,
};

fn hash_api_key(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn sample_local_openai_auth_snapshot(api_key_id: &str, user_id: &str) -> StoredAuthApiKeySnapshot {
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

fn sample_local_openai_candidate_row() -> StoredMinimalCandidateSelectionRow {
    StoredMinimalCandidateSelectionRow {
        provider_id: "provider-openai-audit-local-1".to_string(),
        provider_name: "openai".to_string(),
        provider_type: "custom".to_string(),
        provider_priority: 10,
        provider_is_active: true,
        endpoint_id: "endpoint-openai-audit-local-1".to_string(),
        endpoint_api_format: "openai:chat".to_string(),
        endpoint_api_family: Some("openai".to_string()),
        endpoint_kind: Some("chat".to_string()),
        endpoint_is_active: true,
        key_id: "key-openai-audit-local-1".to_string(),
        key_name: "prod".to_string(),
        key_auth_type: "api_key".to_string(),
        key_is_active: true,
        key_api_formats: Some(vec!["openai:chat".to_string()]),
        key_allowed_models: None,
        key_capabilities: None,
        key_internal_priority: 5,
        key_global_priority_by_format: Some(serde_json::json!({"openai:chat": 1})),
        model_id: "model-openai-audit-local-1".to_string(),
        global_model_id: "global-model-openai-audit-local-1".to_string(),
        global_model_name: "gpt-5".to_string(),
        global_model_mappings: None,
        global_model_supports_streaming: Some(true),
        model_provider_model_name: "gpt-5-upstream".to_string(),
        model_provider_model_mappings: Some(vec![StoredProviderModelMapping {
            name: "gpt-5-upstream".to_string(),
            priority: 1,
            api_formats: Some(vec!["openai:chat".to_string()]),
            endpoint_ids: None,
        }]),
        model_supports_streaming: Some(true),
        model_is_active: true,
        model_is_available: true,
    }
}

fn sample_local_openai_provider() -> StoredProviderCatalogProvider {
    StoredProviderCatalogProvider::new(
        "provider-openai-audit-local-1".to_string(),
        "OpenAI".to_string(),
        Some("https://example.com".to_string()),
        "custom".to_string(),
    )
    .expect("provider should build")
    .with_transport_fields(
        true,
        false,
        false,
        None,
        Some(2),
        None,
        Some(20.0),
        None,
        None,
    )
}

fn sample_local_openai_endpoint(base_url: String) -> StoredProviderCatalogEndpoint {
    StoredProviderCatalogEndpoint::new(
        "endpoint-openai-audit-local-1".to_string(),
        "provider-openai-audit-local-1".to_string(),
        "openai:chat".to_string(),
        Some("openai".to_string()),
        Some("chat".to_string()),
        true,
    )
    .expect("endpoint should build")
    .with_transport_fields(base_url, None, None, Some(2), None, None, None, None)
    .expect("endpoint transport should build")
}

fn sample_local_openai_key() -> StoredProviderCatalogKey {
    StoredProviderCatalogKey::new(
        "key-openai-audit-local-1".to_string(),
        "provider-openai-audit-local-1".to_string(),
        "prod".to_string(),
        "api_key".to_string(),
        None,
        true,
    )
    .expect("key should build")
    .with_transport_fields(
        Some(serde_json::json!(["openai:chat"])),
        encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "sk-upstream-openai")
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

#[tokio::test]
async fn gateway_exposes_request_id_header_for_local_execution_response() {
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-client-openai-audit-bundle")),
        sample_local_openai_auth_snapshot("api-key-1", "user-1"),
    )]));
    let request_candidates = Arc::new(InMemoryRequestCandidateRepository::default());
    let usage_repository = Arc::new(InMemoryUsageReadRepository::default());
    let candidate_selection_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            sample_local_openai_candidate_row(),
        ]));

    let provider = Router::new().route(
        "/chat/completions",
        any(|_request: Request| async move {
            Json(json!({
                "id": "chatcmpl-direct-audit-123",
                "object": "chat.completion",
                "model": "gpt-5-upstream",
                "choices": [],
                "usage": {
                    "prompt_tokens": 2,
                    "completion_tokens": 3,
                    "total_tokens": 5
                }
            }))
        }),
    );

    let (provider_url, provider_handle) = start_server(provider).await;
    let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_local_openai_provider()],
        vec![sample_local_openai_endpoint(provider_url)],
        vec![sample_local_openai_key()],
    ));
    let gateway_state = AppState::new()
        .expect("gateway state should build")
        .with_data_state_for_tests(
            crate::data::GatewayDataState::with_auth_candidate_selection_provider_catalog_request_candidates_and_usage_for_tests(
                auth_repository,
                candidate_selection_repository,
                provider_catalog,
                Arc::clone(&request_candidates),
                Arc::clone(&usage_repository),
                DEVELOPMENT_ENCRYPTION_KEY,
            ),
        )
        .with_usage_runtime_for_tests(UsageRuntimeConfig {
            enabled: true,
            ..UsageRuntimeConfig::default()
        });
    let gateway = build_router_with_state(gateway_state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/chat/completions"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(
            http::header::AUTHORIZATION,
            "Bearer sk-client-openai-audit-bundle",
        )
        .header(TRACE_ID_HEADER, "req-direct-audit-123")
        .body("{\"model\":\"gpt-5\",\"messages\":[]}")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let request_id = response
        .headers()
        .get(CONTROL_REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .expect("request id header should exist")
        .to_string();
    assert_eq!(request_id, "req-direct-audit-123");

    for _ in 0..50 {
        if usage_repository
            .find_by_request_id(&request_id)
            .await
            .expect("usage lookup should succeed")
            .is_some()
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    let audit_response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/_gateway/audit/request-audit/{request_id}?attempted_only=true"
        ))
        .send()
        .await
        .expect("request audit should succeed");

    assert_eq!(audit_response.status(), StatusCode::OK);
    let payload: Value = audit_response.json().await.expect("payload should parse");
    assert_eq!(payload["request_id"], "req-direct-audit-123");
    assert_eq!(payload["usage"]["provider_name"], "OpenAI");
    assert_eq!(payload["decision_trace"]["total_candidates"], 1);
    assert_eq!(payload["auth_snapshot"]["api_key_id"], "api-key-1");

    gateway_handle.abort();
    provider_handle.abort();
}

fn sample_request_candidate(
    id: &str,
    request_id: &str,
    candidate_index: i32,
    status: RequestCandidateStatus,
    started_at_unix_ms: Option<i64>,
    latency_ms: Option<i32>,
    status_code: Option<i32>,
) -> StoredRequestCandidate {
    StoredRequestCandidate::new(
        id.to_string(),
        request_id.to_string(),
        Some("user-1".to_string()),
        Some("api-key-1".to_string()),
        Some("alice".to_string()),
        Some("default".to_string()),
        candidate_index,
        0,
        Some("provider-1".to_string()),
        Some("endpoint-1".to_string()),
        Some("provider-key-1".to_string()),
        status,
        None,
        false,
        status_code,
        None,
        None,
        latency_ms,
        Some(1),
        None,
        None,
        (100 + i64::from(candidate_index)) * 1_000,
        started_at_unix_ms.map(|v| v * 1_000),
        started_at_unix_ms.map(|value| (value + 1) * 1_000),
    )
    .expect("candidate should build")
}

fn sample_auth_snapshot(api_key_id: &str, user_id: &str) -> StoredAuthApiKeySnapshot {
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
        Some(serde_json::json!(["gpt-4.1"])),
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
        Some(serde_json::json!(["gpt-4.1"])),
    )
    .expect("auth snapshot should build")
}

fn sample_provider_catalog_provider() -> StoredProviderCatalogProvider {
    StoredProviderCatalogProvider::new(
        "provider-1".to_string(),
        "OpenAI".to_string(),
        Some("https://openai.com".to_string()),
        "custom".to_string(),
    )
    .expect("provider should build")
}

fn sample_provider_catalog_endpoint() -> StoredProviderCatalogEndpoint {
    StoredProviderCatalogEndpoint::new(
        "endpoint-1".to_string(),
        "provider-1".to_string(),
        "openai:chat".to_string(),
        Some("openai".to_string()),
        Some("chat".to_string()),
        true,
    )
    .expect("endpoint should build")
}

fn sample_provider_catalog_key() -> StoredProviderCatalogKey {
    StoredProviderCatalogKey::new(
        "provider-key-1".to_string(),
        "provider-1".to_string(),
        "prod-key".to_string(),
        "api_key".to_string(),
        Some(serde_json::json!({"cache_1h": true})),
        true,
    )
    .expect("key should build")
}

fn sample_request_usage(request_id: &str) -> StoredRequestUsageAudit {
    StoredRequestUsageAudit::new(
        "usage-1".to_string(),
        request_id.to_string(),
        Some("user-1".to_string()),
        Some("api-key-1".to_string()),
        Some("alice".to_string()),
        Some("default".to_string()),
        "OpenAI".to_string(),
        "gpt-4.1".to_string(),
        Some("gpt-4.1-mini".to_string()),
        Some("provider-1".to_string()),
        Some("endpoint-1".to_string()),
        Some("provider-key-1".to_string()),
        Some("chat".to_string()),
        Some("openai:chat".to_string()),
        Some("openai".to_string()),
        Some("chat".to_string()),
        Some("openai:chat".to_string()),
        Some("openai".to_string()),
        Some("chat".to_string()),
        true,
        false,
        120,
        40,
        160,
        0.24,
        0.36,
        Some(200),
        None,
        None,
        Some(450),
        Some(120),
        "completed".to_string(),
        "settled".to_string(),
        100,
        101,
        Some(102),
    )
    .expect("usage should build")
}

#[tokio::test]
async fn gateway_exposes_request_usage_via_internal_audit_endpoint() {
    let repository = Arc::new(InMemoryUsageReadRepository::seed(vec![
        sample_request_usage("req-usage-2"),
    ]));
    let gateway_state = AppState::new()
        .expect("gateway state should build")
        .with_usage_data_reader_for_tests(repository);
    let gateway = build_router_with_state(gateway_state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/_gateway/audit/request-usage/req-usage-2"
        ))
        .send()
        .await
        .expect("audit request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: Value = response.json().await.expect("payload should parse");
    assert_eq!(payload["request_id"], "req-usage-2");
    assert_eq!(payload["provider_name"], "OpenAI");
    assert_eq!(payload["api_format"], "openai:chat");
    assert_eq!(payload["total_tokens"], 160);
    assert_eq!(payload["total_cost_usd"], 0.24);
    assert_eq!(payload["status"], "completed");
    assert_eq!(payload["billing_status"], "settled");

    gateway_handle.abort();
}

#[tokio::test]
async fn gateway_exposes_request_audit_bundle_via_internal_audit_endpoint() {
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some("hash-1".to_string()),
        sample_auth_snapshot("api-key-1", "user-1"),
    )]));
    let request_candidates = Arc::new(InMemoryRequestCandidateRepository::seed(vec![
        sample_request_candidate(
            "cand-1",
            "req-audit-1",
            0,
            RequestCandidateStatus::Success,
            Some(101),
            Some(37),
            Some(200),
        ),
    ]));
    let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider_catalog_provider()],
        vec![sample_provider_catalog_endpoint()],
        vec![sample_provider_catalog_key()],
    ));
    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![
        sample_request_usage("req-audit-1"),
    ]));
    let gateway_state = AppState::new()
        .expect("gateway state should build")
        .with_request_audit_data_readers_for_tests(
            auth_repository,
            request_candidates,
            provider_catalog,
            usage_repository,
        );
    let gateway = build_router_with_state(gateway_state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/_gateway/audit/request-audit/req-audit-1?attempted_only=true"
        ))
        .send()
        .await
        .expect("audit request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: Value = response.json().await.expect("payload should parse");
    assert_eq!(payload["request_id"], "req-audit-1");
    assert_eq!(payload["usage"]["provider_name"], "OpenAI");
    assert_eq!(payload["usage"]["total_tokens"], 160);
    assert_eq!(payload["decision_trace"]["total_candidates"], 1);
    assert_eq!(
        payload["decision_trace"]["candidates"][0]["provider_key_name"],
        "prod-key"
    );
    assert_eq!(payload["auth_snapshot"]["api_key_id"], "api-key-1");
    assert_eq!(payload["auth_snapshot"]["currently_usable"], true);

    gateway_handle.abort();
}

#[tokio::test]
async fn gateway_exposes_request_candidate_trace_via_internal_audit_endpoint() {
    let repository = Arc::new(InMemoryRequestCandidateRepository::seed(vec![
        sample_request_candidate(
            "cand-1",
            "req-trace-1",
            0,
            RequestCandidateStatus::Pending,
            None,
            None,
            None,
        ),
        sample_request_candidate(
            "cand-2",
            "req-trace-1",
            1,
            RequestCandidateStatus::Failed,
            Some(101),
            Some(37),
            Some(502),
        ),
    ]));

    let gateway_state = AppState::new()
        .expect("gateway state should build")
        .with_request_candidate_data_reader_for_tests(repository);
    let gateway = build_router_with_state(gateway_state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/_gateway/audit/request-candidates/req-trace-1?attempted_only=true"
        ))
        .send()
        .await
        .expect("audit request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: Value = response.json().await.expect("payload should parse");
    assert_eq!(payload["request_id"], "req-trace-1");
    assert_eq!(payload["total_candidates"], 1);
    assert_eq!(payload["final_status"], "failed");
    assert_eq!(payload["total_latency_ms"], 37);
    assert_eq!(
        payload["candidates"].as_array().map(|items| items.len()),
        Some(1)
    );
    assert_eq!(payload["candidates"][0]["id"], "cand-2");
    assert_eq!(payload["candidates"][0]["status"], "failed");

    gateway_handle.abort();
}

#[tokio::test]
async fn gateway_exposes_decision_trace_via_internal_audit_endpoint() {
    let request_candidates = Arc::new(InMemoryRequestCandidateRepository::seed(vec![
        sample_request_candidate(
            "cand-1",
            "req-trace-2",
            0,
            RequestCandidateStatus::Failed,
            Some(101),
            Some(37),
            Some(502),
        ),
    ]));
    let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider_catalog_provider()],
        vec![sample_provider_catalog_endpoint()],
        vec![sample_provider_catalog_key()],
    ));

    let gateway_state = AppState::new()
        .expect("gateway state should build")
        .with_decision_trace_data_readers_for_tests(request_candidates, provider_catalog);
    let gateway = build_router_with_state(gateway_state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/_gateway/audit/decision-trace/req-trace-2?attempted_only=true"
        ))
        .send()
        .await
        .expect("audit request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: Value = response.json().await.expect("payload should parse");
    assert_eq!(payload["request_id"], "req-trace-2");
    assert_eq!(payload["total_candidates"], 1);
    assert_eq!(payload["candidates"][0]["provider_name"], "OpenAI");
    assert_eq!(
        payload["candidates"][0]["provider_website"],
        "https://openai.com"
    );
    assert_eq!(
        payload["candidates"][0]["endpoint_api_format"],
        "openai:chat"
    );
    assert_eq!(payload["candidates"][0]["provider_key_name"], "prod-key");
    assert_eq!(
        payload["candidates"][0]["provider_key_auth_type"],
        "api_key"
    );
    assert_eq!(
        payload["candidates"][0]["provider_key_capabilities"]["cache_1h"],
        true
    );

    gateway_handle.abort();
}

#[tokio::test]
async fn gateway_exposes_auth_api_key_snapshot_via_internal_audit_endpoint() {
    let repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some("hash-1".to_string()),
        sample_auth_snapshot("key-1", "user-1"),
    )]));

    let gateway_state = AppState::new()
        .expect("gateway state should build")
        .with_auth_api_key_data_reader_for_tests(repository);
    let gateway = build_router_with_state(gateway_state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/_gateway/audit/auth/users/user-1/api-keys/key-1"
        ))
        .send()
        .await
        .expect("audit request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: Value = response.json().await.expect("payload should parse");
    assert_eq!(payload["user_id"], "user-1");
    assert_eq!(payload["api_key_id"], "key-1");
    assert_eq!(payload["username"], "alice");
    assert_eq!(payload["user_role"], "user");
    assert_eq!(payload["api_key_name"], "default");
    assert_eq!(payload["currently_usable"], true);
    assert_eq!(payload["user_allowed_providers"][0], "openai");
    assert_eq!(payload["api_key_allowed_api_formats"][0], "openai:chat");

    gateway_handle.abort();
}
