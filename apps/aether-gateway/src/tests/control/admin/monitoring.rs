use std::sync::{Arc, Mutex};

use aether_crypto::{encrypt_python_fernet_plaintext, DEVELOPMENT_ENCRYPTION_KEY};
use aether_data::repository::auth::{
    InMemoryAuthApiKeySnapshotRepository, StoredAuthApiKeyExportRecord,
};
use aether_data::repository::candidates::InMemoryRequestCandidateRepository;
use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
use aether_data::repository::usage::InMemoryUsageReadRepository;
use aether_data::repository::users::{
    InMemoryUserReadRepository, StoredUserAuthRecord, StoredUserExportRow,
};
use aether_data_contracts::repository::candidates::{
    RequestCandidateStatus, StoredRequestCandidate,
};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use aether_data_contracts::repository::usage::StoredRequestUsageAudit;
use axum::body::Body;
use axum::routing::{any, delete, get};
use axum::{extract::Request, Router};
use http::StatusCode;
use serde_json::json;

use super::super::{build_router_with_state, start_server, AppState};
use crate::constants::{
    GATEWAY_HEADER, TRUSTED_ADMIN_SESSION_ID_HEADER, TRUSTED_ADMIN_USER_ID_HEADER,
    TRUSTED_ADMIN_USER_ROLE_HEADER,
};
use crate::data::GatewayDataState;

const ADMIN_MONITORING_DATA_UNAVAILABLE_DETAIL: &str = "Admin monitoring data unavailable";

async fn assert_admin_monitoring_route_returns_local_503(method: http::Method, path: &str) {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        path,
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(AppState::new().expect("gateway should build"));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .request(method, format!("{gateway_url}{path}"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["detail"], ADMIN_MONITORING_DATA_UNAVAILABLE_DETAIL);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_monitoring_audit_logs_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/monitoring/audit-logs",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(AppState::new().expect("gateway should build"));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/monitoring/audit-logs?days=14&limit=20&offset=5&username=alice&event_type=login_failed"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["items"], json!([]));
    assert_eq!(payload["meta"]["total"], json!(0));
    assert_eq!(payload["meta"]["limit"], json!(20));
    assert_eq!(payload["meta"]["offset"], json!(5));
    assert_eq!(payload["meta"]["count"], json!(0));
    assert_eq!(payload["filters"]["username"], json!("alice"));
    assert_eq!(payload["filters"]["event_type"], json!("login_failed"));
    assert_eq!(payload["filters"]["days"], json!(14));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

fn sample_usage(
    request_id: &str,
    provider_id: &str,
    provider_name: &str,
    total_tokens: i32,
    total_cost_usd: f64,
    status: &str,
    status_code: Option<i32>,
    created_at_unix_ms: i64,
) -> StoredRequestUsageAudit {
    let is_error = status_code.is_some_and(|value| value >= 400)
        || status.trim().eq_ignore_ascii_case("failed")
        || status.trim().eq_ignore_ascii_case("error");
    StoredRequestUsageAudit::new(
        format!("usage-{request_id}"),
        request_id.to_string(),
        Some("user-1".to_string()),
        Some("api-key-1".to_string()),
        Some("alice".to_string()),
        Some("monitoring-key".to_string()),
        provider_name.to_string(),
        "gpt-4.1".to_string(),
        None,
        Some(provider_id.to_string()),
        Some("endpoint-1".to_string()),
        Some("provider-key-1".to_string()),
        Some("chat".to_string()),
        Some("openai:chat".to_string()),
        Some("openai".to_string()),
        Some("chat".to_string()),
        Some("openai:chat".to_string()),
        Some("openai".to_string()),
        Some("chat".to_string()),
        false,
        false,
        total_tokens / 2,
        total_tokens / 2,
        total_tokens,
        total_cost_usd,
        total_cost_usd,
        status_code,
        is_error.then(|| "boom".to_string()),
        is_error.then(|| "upstream_error".to_string()),
        Some(120),
        Some(30),
        status.to_string(),
        "billed".to_string(),
        created_at_unix_ms,
        created_at_unix_ms,
        Some(created_at_unix_ms),
    )
    .expect("usage should build")
}

fn sample_inactive_provider() -> StoredProviderCatalogProvider {
    StoredProviderCatalogProvider::new(
        "provider-2".to_string(),
        "Anthropic".to_string(),
        Some("https://anthropic.com".to_string()),
        "custom".to_string(),
    )
    .expect("provider should build")
    .with_transport_fields(false, false, false, None, None, None, None, None, None)
}

#[tokio::test]
async fn gateway_handles_admin_monitoring_system_status_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/monitoring/system-status",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let now = chrono::Utc::now().timestamp();
    let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider(), sample_inactive_provider()],
        vec![],
        vec![],
    ));
    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![
        sample_usage(
            "request-today-ok",
            "provider-1",
            "OpenAI",
            20,
            0.25,
            "success",
            Some(200),
            now - 300,
        ),
        sample_usage(
            "request-today-failed",
            "provider-1",
            "OpenAI",
            10,
            0.10,
            "failed",
            Some(502),
            now - 120,
        ),
        sample_usage(
            "request-old",
            "provider-1",
            "OpenAI",
            99,
            9.99,
            "success",
            Some(200),
            now - 172_800,
        ),
    ]));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_provider_catalog_and_usage_reader_for_tests(
                    provider_catalog,
                    usage_repository,
                ),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/api/admin/monitoring/system-status"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["users"]["total"], json!(0));
    assert_eq!(payload["users"]["active"], json!(0));
    assert_eq!(payload["providers"]["total"], json!(2));
    assert_eq!(payload["providers"]["active"], json!(1));
    assert_eq!(payload["api_keys"]["total"], json!(0));
    assert_eq!(payload["api_keys"]["active"], json!(0));
    assert_eq!(payload["today_stats"]["requests"], json!(2));
    assert_eq!(payload["today_stats"]["tokens"], json!(30));
    assert_eq!(payload["today_stats"]["cost_usd"], json!("$0.3500"));
    assert_eq!(payload["tunnel"]["proxy_connections"], json!(0));
    assert_eq!(payload["tunnel"]["nodes"], json!(0));
    assert_eq!(payload["tunnel"]["active_streams"], json!(0));
    assert_eq!(payload["recent_errors"], json!(1));
    assert_eq!(payload["usage_counter"]["status"], json!("idle"));
    assert_eq!(payload["usage_counter"]["outbox_pending_rows"], json!(0));
    assert!(payload["timestamp"].as_str().is_some());
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

fn sample_candidate(
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
        Some(json!({"cache_1h": true})),
        (100 + i64::from(candidate_index)) * 1_000,
        started_at_unix_ms.map(|v| v * 1_000),
        started_at_unix_ms.map(|value| (value + 1) * 1_000),
    )
    .expect("candidate should build")
}

fn sample_provider() -> StoredProviderCatalogProvider {
    StoredProviderCatalogProvider::new(
        "provider-1".to_string(),
        "OpenAI".to_string(),
        Some("https://openai.com".to_string()),
        "custom".to_string(),
    )
    .expect("provider should build")
}

fn sample_endpoint() -> StoredProviderCatalogEndpoint {
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

fn sample_key() -> StoredProviderCatalogKey {
    StoredProviderCatalogKey::new(
        "provider-key-1".to_string(),
        "provider-1".to_string(),
        "prod-key".to_string(),
        "api_key".to_string(),
        Some(json!({"cache_1h": true})),
        true,
    )
    .expect("key should build")
}

fn sample_monitoring_auth_user(user_id: &str) -> StoredUserAuthRecord {
    StoredUserAuthRecord::new(
        user_id.to_string(),
        Some("alice@example.com".to_string()),
        true,
        "alice".to_string(),
        None,
        "user".to_string(),
        "local".to_string(),
        None,
        None,
        None,
        true,
        false,
        None,
        None,
    )
    .expect("auth user should build")
}

fn sample_monitoring_export_user(user_id: &str) -> StoredUserExportRow {
    StoredUserExportRow::new(
        user_id.to_string(),
        Some("alice@example.com".to_string()),
        true,
        "alice".to_string(),
        None,
        "user".to_string(),
        "local".to_string(),
        None,
        None,
        None,
        None,
        None,
        true,
    )
    .expect("export user should build")
}

fn sample_monitoring_export_api_key(
    user_id: &str,
    api_key_id: &str,
) -> StoredAuthApiKeyExportRecord {
    StoredAuthApiKeyExportRecord::new(
        user_id.to_string(),
        api_key_id.to_string(),
        format!("hash-{api_key_id}"),
        Some(
            encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "sk-user-monitoring-1234")
                .expect("user key should encrypt"),
        ),
        Some("Alice Key".to_string()),
        None,
        None,
        None,
        None,
        None,
        None,
        true,
        None,
        false,
        0,
        0,
        0.0,
        false,
    )
    .expect("export api key should build")
}

fn sample_monitoring_catalog_endpoint() -> StoredProviderCatalogEndpoint {
    sample_endpoint()
        .with_transport_fields(
            "https://api.openai.example/v1".to_string(),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("endpoint transport fields should build")
}

fn sample_monitoring_catalog_key() -> StoredProviderCatalogKey {
    sample_key()
        .with_transport_fields(
            None,
            encrypt_python_fernet_plaintext(
                DEVELOPMENT_ENCRYPTION_KEY,
                "sk-upstream-monitoring-5678",
            )
            .expect("provider key should encrypt"),
            None,
            Some(json!({"cache": 1.0})),
            None,
            None,
            None,
            None,
            None,
        )
        .expect("provider key transport fields should build")
}

#[tokio::test]
async fn gateway_handles_admin_monitoring_trace_request_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/monitoring/trace/request-1",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let request_candidates = Arc::new(InMemoryRequestCandidateRepository::seed(vec![
        sample_candidate(
            "cand-unused",
            "request-1",
            0,
            RequestCandidateStatus::Pending,
            None,
            None,
            None,
        ),
        sample_candidate(
            "cand-used",
            "request-1",
            1,
            RequestCandidateStatus::Failed,
            Some(101),
            Some(33),
            Some(502),
        ),
    ]));
    let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider()],
        vec![sample_endpoint()],
        vec![sample_key()],
    ));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_decision_trace_data_readers_for_tests(request_candidates, provider_catalog),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/monitoring/trace/request-1?attempted_only=true"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["request_id"], json!("request-1"));
    assert_eq!(payload["total_candidates"], json!(1));
    assert_eq!(payload["final_status"], json!("failed"));
    assert_eq!(payload["candidates"][0]["id"], json!("cand-used"));
    assert_eq!(payload["candidates"][0]["provider_name"], json!("OpenAI"));
    assert_eq!(
        payload["candidates"][0]["provider_website"],
        json!("https://openai.com")
    );
    assert_eq!(
        payload["candidates"][0]["endpoint_name"],
        json!("openai:chat")
    );
    assert_eq!(payload["candidates"][0]["key_name"], json!("prod-key"));
    assert_eq!(payload["candidates"][0]["key_auth_type"], json!("api_key"));
    assert_eq!(payload["candidates"][0]["latency_ms"], json!(33));
    assert_eq!(payload["candidates"][0]["status_code"], json!(502));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_monitoring_trace_provider_stats_locally_with_trusted_admin_principal(
) {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/monitoring/trace/stats/provider/provider-1",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let request_candidates = Arc::new(InMemoryRequestCandidateRepository::seed(vec![
        sample_candidate(
            "cand-1",
            "req-a",
            0,
            RequestCandidateStatus::Success,
            Some(101),
            Some(20),
            Some(200),
        ),
        sample_candidate(
            "cand-2",
            "req-b",
            0,
            RequestCandidateStatus::Failed,
            Some(201),
            Some(40),
            Some(502),
        ),
        sample_candidate(
            "cand-3",
            "req-c",
            0,
            RequestCandidateStatus::Cancelled,
            Some(301),
            Some(60),
            Some(499),
        ),
        sample_candidate(
            "cand-4",
            "req-d",
            0,
            RequestCandidateStatus::Available,
            None,
            None,
            None,
        ),
        sample_candidate(
            "cand-5",
            "req-e",
            0,
            RequestCandidateStatus::Unused,
            None,
            None,
            None,
        ),
    ]));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_request_candidate_data_reader_for_tests(request_candidates),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/monitoring/trace/stats/provider/provider-1?limit=10"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["provider_id"], json!("provider-1"));
    assert_eq!(payload["total_attempts"], json!(5));
    assert_eq!(payload["success_count"], json!(1));
    assert_eq!(payload["failed_count"], json!(1));
    assert_eq!(payload["cancelled_count"], json!(1));
    assert_eq!(payload["available_count"], json!(1));
    assert_eq!(payload["unused_count"], json!(1));
    assert_eq!(payload["failure_rate"], json!(50.0));
    assert_eq!(payload["avg_latency_ms"], json!(40.0));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_monitoring_cache_stats_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/monitoring/cache/stats",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let now = chrono::Utc::now().timestamp();
    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![
        sample_usage(
            "request-cache-hit",
            "provider-1",
            "OpenAI",
            20,
            0.20,
            "success",
            Some(200),
            now - 60,
        )
        .with_cache_input_tokens(10, 5),
        sample_usage(
            "request-cache-miss",
            "provider-1",
            "OpenAI",
            15,
            0.10,
            "success",
            Some(200),
            now - 120,
        ),
    ]));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_usage_reader_for_tests(usage_repository)
                    .with_system_config_values_for_tests([
                        ("scheduling_mode".to_string(), json!("cache_affinity")),
                        ("provider_priority_mode".to_string(), json!("provider")),
                    ]),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/api/admin/monitoring/cache/stats"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["status"], json!("ok"));
    assert_eq!(payload["data"]["scheduler"], json!("cache_aware"));
    assert_eq!(payload["data"]["cache_hits"], json!(1));
    assert_eq!(payload["data"]["cache_misses"], json!(1));
    assert_eq!(payload["data"]["cache_hit_rate"], json!(0.5));
    assert_eq!(
        payload["data"]["scheduler_metrics"]["provider_priority_mode"],
        json!("provider")
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_monitoring_cache_affinities_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/monitoring/cache/affinities",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let user_repository = Arc::new(
        InMemoryUserReadRepository::seed_auth_users(vec![sample_monitoring_auth_user("user-1")])
            .with_export_users(vec![sample_monitoring_export_user("user-1")]),
    );
    let auth_repository = Arc::new(
        InMemoryAuthApiKeySnapshotRepository::default().with_export_records(vec![
            sample_monitoring_export_api_key("user-1", "user-key-1"),
        ]),
    );
    let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider()],
        vec![sample_monitoring_catalog_endpoint()],
        vec![sample_monitoring_catalog_key()],
    ));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_provider_catalog_reader_for_tests(
                    provider_catalog,
                )
                .with_user_reader(user_repository)
                .with_auth_api_key_reader(auth_repository),
            )
            .with_admin_monitoring_cache_affinity_entry_for_tests(
                "cache_affinity:user-key-1:openai:model-alpha",
                json!({
                    "provider_id": "provider-1",
                    "endpoint_id": "endpoint-1",
                    "key_id": "provider-key-1",
                    "created_at": 1710000000,
                    "expire_at": 1710000300,
                    "request_count": 7,
                }),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/monitoring/cache/affinities?keyword=alice&limit=20&offset=0"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["status"], json!("ok"));
    assert_eq!(payload["data"]["matched_user_id"], json!("user-1"));
    assert_eq!(payload["data"]["meta"]["total"], json!(1));
    assert_eq!(
        payload["data"]["items"][0]["affinity_key"],
        json!("user-key-1")
    );
    assert_eq!(
        payload["data"]["items"][0]["provider_name"],
        json!("OpenAI")
    );
    assert_eq!(payload["data"]["items"][0]["key_name"], json!("prod-key"));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_monitoring_cache_affinity_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/monitoring/cache/affinity/alice",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let user_repository = Arc::new(
        InMemoryUserReadRepository::seed_auth_users(vec![sample_monitoring_auth_user("user-1")])
            .with_export_users(vec![sample_monitoring_export_user("user-1")]),
    );
    let auth_repository = Arc::new(
        InMemoryAuthApiKeySnapshotRepository::default().with_export_records(vec![
            sample_monitoring_export_api_key("user-1", "user-key-1"),
        ]),
    );
    let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider()],
        vec![sample_monitoring_catalog_endpoint()],
        vec![sample_monitoring_catalog_key()],
    ));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_provider_catalog_reader_for_tests(
                    provider_catalog,
                )
                .with_user_reader(user_repository)
                .with_auth_api_key_reader(auth_repository),
            )
            .with_admin_monitoring_cache_affinity_entry_for_tests(
                "cache_affinity:user-key-1:openai:model-alpha",
                json!({
                    "provider_id": "provider-1",
                    "endpoint_id": "endpoint-1",
                    "key_id": "provider-key-1",
                    "created_at": 1710000000,
                    "expire_at": 1710000300,
                    "request_count": 7,
                }),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/monitoring/cache/affinity/alice"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["status"], json!("ok"));
    assert_eq!(payload["user_info"]["user_id"], json!("user-1"));
    assert_eq!(payload["total_endpoints"], json!(1));
    assert_eq!(payload["affinities"][0]["api_format"], json!("openai"));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_monitoring_cache_affinities_locally_without_redis_or_entries() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/monitoring/cache/affinities",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(AppState::new().expect("gateway should build"));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/monitoring/cache/affinities?limit=20&offset=0"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["status"], json!("ok"));
    assert_eq!(payload["data"]["items"], json!([]));
    assert_eq!(payload["data"]["meta"]["total"], json!(0));
    assert_eq!(payload["data"]["meta"]["count"], json!(0));
    assert_eq!(payload["data"]["matched_user_id"], serde_json::Value::Null);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_monitoring_cache_affinity_locally_without_redis_or_entries() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/monitoring/cache/affinity/alice",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let user_repository = Arc::new(
        InMemoryUserReadRepository::seed_auth_users(vec![sample_monitoring_auth_user("user-1")])
            .with_export_users(vec![sample_monitoring_export_user("user-1")]),
    );
    let auth_repository = Arc::new(
        InMemoryAuthApiKeySnapshotRepository::default().with_export_records(vec![
            sample_monitoring_export_api_key("user-1", "user-key-1"),
        ]),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_user_reader_for_tests(user_repository)
                    .with_auth_api_key_reader(auth_repository),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/monitoring/cache/affinity/alice"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["status"], json!("not_found"));
    assert_eq!(payload["user_info"]["user_id"], json!("user-1"));
    assert_eq!(payload["affinities"], json!([]));
    assert_eq!(
        payload["message"],
        json!("用户 alice (alice@example.com) 没有缓存亲和性")
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_monitoring_cache_users_delete_locally_with_trusted_admin_principal()
{
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/monitoring/cache/users/alice",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let user_repository = Arc::new(
        InMemoryUserReadRepository::seed_auth_users(vec![sample_monitoring_auth_user("user-1")])
            .with_export_users(vec![sample_monitoring_export_user("user-1")]),
    );
    let auth_repository = Arc::new(
        InMemoryAuthApiKeySnapshotRepository::default().with_export_records(vec![
            sample_monitoring_export_api_key("user-1", "user-key-1"),
        ]),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(
            crate::data::GatewayDataState::with_user_reader_for_tests(user_repository)
                .with_auth_api_key_reader(auth_repository),
        )
        .with_admin_monitoring_cache_affinity_entry_for_tests(
            "cache_affinity:user-key-1:openai:model-alpha",
            json!({
                "provider_id": "provider-1",
                "endpoint_id": "endpoint-1",
                "key_id": "provider-key-1",
                "created_at": 1710000000,
                "expire_at": 1710000300,
                "request_count": 7,
            }),
        );
    let gateway = build_router_with_state(state.clone());
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .delete(format!(
            "{gateway_url}/api/admin/monitoring/cache/users/alice"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["status"], json!("ok"));
    assert_eq!(
        payload["message"],
        json!("已清除用户 alice 的所有缓存亲和性")
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);
    assert!(state
        .list_admin_monitoring_cache_affinity_entries_for_tests()
        .is_empty());

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_monitoring_cache_affinity_delete_locally_with_trusted_admin_principal(
) {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/monitoring/cache/affinity/user-key-1/endpoint-1/model-alpha/openai",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let user_repository = Arc::new(
        InMemoryUserReadRepository::seed_auth_users(vec![sample_monitoring_auth_user("user-1")])
            .with_export_users(vec![sample_monitoring_export_user("user-1")]),
    );
    let auth_repository = Arc::new(
        InMemoryAuthApiKeySnapshotRepository::default().with_export_records(vec![
            sample_monitoring_export_api_key("user-1", "user-key-1"),
        ]),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(
            crate::data::GatewayDataState::with_user_reader_for_tests(user_repository)
                .with_auth_api_key_reader(auth_repository),
        )
        .with_admin_monitoring_cache_affinity_entry_for_tests(
            "cache_affinity:user-key-1:openai:model-alpha",
            json!({
                "provider_id": "provider-1",
                "endpoint_id": "endpoint-1",
                "key_id": "provider-key-1",
                "created_at": 1710000000,
                "expire_at": 1710000300,
                "request_count": 7,
            }),
        );
    let gateway = build_router_with_state(state.clone());
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .delete(format!(
            "{gateway_url}/api/admin/monitoring/cache/affinity/user-key-1/endpoint-1/model-alpha/openai"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["status"], json!("ok"));
    assert_eq!(payload["message"], json!("已清除缓存亲和性: Alice Key"));
    assert_eq!(payload["affinity_key"], json!("user-key-1"));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);
    assert!(state
        .list_admin_monitoring_cache_affinity_entries_for_tests()
        .is_empty());

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_monitoring_cache_flush_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/monitoring/cache",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let state = AppState::new()
        .expect("state should build")
        .with_admin_monitoring_cache_affinity_entry_for_tests(
            "cache_affinity:user-key-1:openai:model-alpha",
            json!({
                "provider_id": "provider-1",
                "endpoint_id": "endpoint-1",
                "key_id": "provider-key-1",
                "created_at": 1710000000,
                "expire_at": 1710000300,
                "request_count": 7,
            }),
        )
        .with_admin_monitoring_cache_affinity_entry_for_tests(
            "cache_affinity:user-key-2:openai:model-beta",
            json!({
                "provider_id": "provider-2",
                "endpoint_id": "endpoint-2",
                "key_id": "provider-key-2",
                "created_at": 1710000000,
                "expire_at": 1710000300,
                "request_count": 4,
            }),
        );
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .delete(format!("{gateway_url}/api/admin/monitoring/cache"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["status"], json!("ok"));
    assert_eq!(payload["deleted_affinities"], json!(2));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_monitoring_cache_provider_delete_locally_with_trusted_admin_principal(
) {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/monitoring/cache/providers/provider-1",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let state = AppState::new()
        .expect("state should build")
        .with_admin_monitoring_cache_affinity_entry_for_tests(
            "cache_affinity:user-key-1:openai:model-alpha",
            json!({
                "provider_id": "provider-1",
                "endpoint_id": "endpoint-1",
                "key_id": "provider-key-1",
                "created_at": 1710000000,
                "expire_at": 1710000300,
                "request_count": 7,
            }),
        )
        .with_admin_monitoring_cache_affinity_entry_for_tests(
            "cache_affinity:user-key-2:openai:model-beta",
            json!({
                "provider_id": "provider-2",
                "endpoint_id": "endpoint-2",
                "key_id": "provider-key-2",
                "created_at": 1710000000,
                "expire_at": 1710000300,
                "request_count": 4,
            }),
        );
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .delete(format!(
            "{gateway_url}/api/admin/monitoring/cache/providers/provider-1"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["status"], json!("ok"));
    assert_eq!(payload["provider_id"], json!("provider-1"));
    assert_eq!(payload["deleted_affinities"], json!(1));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_monitoring_cache_redis_keys_delete_locally_with_trusted_admin_principal(
) {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/monitoring/cache/redis-keys/upstream_models",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(AppState::new().expect("gateway should build"));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .delete(format!(
            "{gateway_url}/api/admin/monitoring/cache/redis-keys/upstream_models"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["status"], json!("ok"));
    assert_eq!(payload["category"], json!("upstream_models"));
    assert_eq!(payload["deleted_count"], json!(0));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_monitoring_cache_metrics_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/monitoring/cache/metrics",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let now = chrono::Utc::now().timestamp();
    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![
        sample_usage(
            "request-cache-hit",
            "provider-1",
            "OpenAI",
            20,
            0.20,
            "success",
            Some(200),
            now - 60,
        )
        .with_cache_input_tokens(10, 5),
        sample_usage(
            "request-cache-miss",
            "provider-1",
            "OpenAI",
            15,
            0.10,
            "success",
            Some(200),
            now - 120,
        ),
    ]));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_usage_reader_for_tests(usage_repository)
                    .with_system_config_values_for_tests([
                        ("scheduling_mode".to_string(), json!("cache_affinity")),
                        ("provider_priority_mode".to_string(), json!("provider")),
                    ]),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/api/admin/monitoring/cache/metrics"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(reqwest::header::CONTENT_TYPE),
        Some(&reqwest::header::HeaderValue::from_static(
            "text/plain; version=0.0.4; charset=utf-8"
        ))
    );
    let payload = response.text().await.expect("body should read");
    assert!(payload.contains("cache_scheduler_cache_hits 1"));
    assert!(payload.contains("cache_scheduler_cache_misses 1"));
    assert!(payload.contains("cache_scheduler_cache_hit_rate 0.5"));
    assert!(payload.contains("cache_scheduler_info{scheduler=\"cache_aware\"} 1"));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_monitoring_cache_config_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/monitoring/cache/config",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(AppState::new().expect("gateway should build"));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/api/admin/monitoring/cache/config"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["status"], json!("ok"));
    assert_eq!(payload["data"]["cache_ttl_seconds"], json!(300));
    assert_eq!(payload["data"]["cache_reservation_ratio"], json!(0.1));
    assert_eq!(
        payload["data"]["dynamic_reservation"]["enabled"],
        json!(true)
    );
    assert_eq!(
        payload["data"]["dynamic_reservation"]["config"]["probe_phase_requests"],
        json!(100)
    );
    assert_eq!(
        payload["data"]["dynamic_reservation"]["config"]["high_load_threshold"],
        json!(0.8)
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_monitoring_model_mapping_stats_locally_with_trusted_admin_principal()
{
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/monitoring/cache/model-mapping/stats",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(AppState::new().expect("gateway should build"));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/monitoring/cache/model-mapping/stats"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["status"], json!("ok"));
    assert_eq!(payload["data"]["available"], json!(true));
    assert_eq!(payload["data"]["backend"], json!("memory"));
    assert_eq!(payload["data"]["total_keys"], json!(0));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_monitoring_model_mapping_delete_locally_with_trusted_admin_principal(
) {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/monitoring/cache/model-mapping",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let state = AppState::new()
        .expect("state should build")
        .with_admin_monitoring_redis_key_for_tests("model:id:model-1", json!({"id": "model-1"}))
        .with_admin_monitoring_redis_key_for_tests(
            "global_model:name:model-alpha",
            json!({"name": "model-alpha"}),
        )
        .with_admin_monitoring_redis_key_for_tests(
            "global_model:resolve:model-alpha",
            json!({"id": "model-alpha"}),
        );
    let gateway = build_router_with_state(state.clone());
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .delete(format!(
            "{gateway_url}/api/admin/monitoring/cache/model-mapping"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["status"], json!("ok"));
    assert_eq!(payload["deleted_count"], json!(3));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);
    assert!(state
        .list_admin_monitoring_redis_keys_for_tests()
        .is_empty());

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_monitoring_model_mapping_delete_model_locally_with_trusted_admin_principal(
) {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/monitoring/cache/model-mapping/model-alpha",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let state = AppState::new()
        .expect("state should build")
        .with_admin_monitoring_redis_key_for_tests(
            "global_model:name:model-alpha",
            json!({"name": "model-alpha"}),
        )
        .with_admin_monitoring_redis_key_for_tests(
            "global_model:resolve:model-alpha",
            json!({"id": "model-alpha"}),
        )
        .with_admin_monitoring_redis_key_for_tests(
            "global_model:name:model-beta",
            json!({"name": "model-beta"}),
        );
    let gateway = build_router_with_state(state.clone());
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .delete(format!(
            "{gateway_url}/api/admin/monitoring/cache/model-mapping/model-alpha"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["status"], json!("ok"));
    assert_eq!(payload["model_name"], json!("model-alpha"));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(
        state.list_admin_monitoring_redis_keys_for_tests(),
        vec!["global_model:name:model-beta".to_string()]
    );

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_monitoring_model_mapping_delete_provider_locally_with_trusted_admin_principal(
) {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/monitoring/cache/model-mapping/provider/provider-1/model-alpha",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let state = AppState::new()
        .expect("state should build")
        .with_admin_monitoring_redis_key_for_tests(
            "model:provider_global:provider-1:model-alpha",
            json!({"provider_id": "provider-1"}),
        )
        .with_admin_monitoring_redis_key_for_tests(
            "model:provider_global:hits:provider-1:model-alpha",
            json!(12),
        )
        .with_admin_monitoring_redis_key_for_tests(
            "model:provider_global:provider-2:model-alpha",
            json!({"provider_id": "provider-2"}),
        );
    let gateway = build_router_with_state(state.clone());
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .delete(format!(
            "{gateway_url}/api/admin/monitoring/cache/model-mapping/provider/provider-1/model-alpha"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["status"], json!("ok"));
    assert_eq!(payload["provider_id"], json!("provider-1"));
    assert_eq!(payload["global_model_id"], json!("model-alpha"));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(
        state.list_admin_monitoring_redis_keys_for_tests(),
        vec!["model:provider_global:provider-2:model-alpha".to_string()]
    );

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_monitoring_redis_keys_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/monitoring/cache/redis-keys",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(AppState::new().expect("gateway should build"));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/monitoring/cache/redis-keys"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["status"], json!("ok"));
    assert_eq!(payload["data"]["available"], json!(true));
    assert_eq!(payload["data"]["backend"], json!("memory"));
    assert_eq!(payload["data"]["total_keys"], json!(0));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_monitoring_redis_keys_delete_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/monitoring/cache/redis-keys/dashboard",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let state = AppState::new()
        .expect("state should build")
        .with_admin_monitoring_redis_key_for_tests("dashboard:summary:user-1", json!({"ok": true}))
        .with_admin_monitoring_redis_key_for_tests("dashboard:stats:user-1", json!({"ok": true}))
        .with_admin_monitoring_redis_key_for_tests("user:user-1", json!({"ok": true}));
    let gateway = build_router_with_state(state.clone());
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .delete(format!(
            "{gateway_url}/api/admin/monitoring/cache/redis-keys/dashboard"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["status"], json!("ok"));
    assert_eq!(payload["category"], json!("dashboard"));
    assert_eq!(payload["deleted_count"], json!(2));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(
        state.list_admin_monitoring_redis_keys_for_tests(),
        vec!["user:user-1".to_string()]
    );

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_monitoring_suspicious_activities_locally_with_trusted_admin_principal(
) {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/monitoring/suspicious-activities",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(AppState::new().expect("gateway should build"));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/monitoring/suspicious-activities?hours=48"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["activities"], json!([]));
    assert_eq!(payload["count"], json!(0));
    assert_eq!(payload["time_range_hours"], json!(48));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_monitoring_resilience_status_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/monitoring/resilience-status",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let now = chrono::Utc::now().timestamp();
    let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider()],
        vec![],
        vec![sample_key().with_health_fields(
            Some(json!({
                "openai:chat": {
                    "health_score": 0.25,
                    "consecutive_failures": 3,
                    "last_failure_at": "2026-03-30T12:00:00+00:00"
                }
            })),
            Some(json!({
                "openai:chat": {
                    "open": true
                }
            })),
        )],
    ));
    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![sample_usage(
        "request-recent-failed",
        "provider-1",
        "OpenAI",
        10,
        0.10,
        "failed",
        Some(502),
        now - 120,
    )]));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_provider_catalog_and_usage_reader_for_tests(
                    provider_catalog,
                    usage_repository,
                ),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/monitoring/resilience-status"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["health_score"], json!(78));
    assert_eq!(payload["status"], json!("degraded"));
    assert_eq!(payload["error_statistics"]["total_errors"], json!(1));
    assert_eq!(
        payload["error_statistics"]["open_circuit_breakers"],
        json!(1)
    );
    assert_eq!(
        payload["error_statistics"]["circuit_breakers"]["provider-key-1"]["state"],
        json!("open")
    );
    assert_eq!(
        payload["recent_errors"][0]["error_id"],
        json!("usage-request-recent-failed")
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_resets_admin_monitoring_error_stats_locally_with_trusted_admin_principal() {
    let now = chrono::Utc::now().timestamp();
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/monitoring/resilience/error-stats",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider()],
        vec![],
        vec![sample_key().with_health_fields(
            Some(json!({
                "openai:chat": {
                    "health_score": 0.25,
                    "consecutive_failures": 3,
                    "last_failure_at": "2026-03-30T12:00:00+00:00"
                }
            })),
            Some(json!({
                "openai:chat": {
                    "open": true
                }
            })),
        )],
    ));
    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![sample_usage(
        "request-recent-failed",
        "provider-1",
        "OpenAI",
        10,
        0.10,
        "failed",
        Some(502),
        now - 120,
    )]));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_provider_catalog_and_usage_reader_for_tests(
                    provider_catalog,
                    usage_repository,
                ),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let client = reqwest::Client::new();
    let response = client
        .delete(format!(
            "{gateway_url}/api/admin/monitoring/resilience/error-stats"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["message"], json!("错误统计已重置"));
    assert_eq!(payload["previous_stats"]["total_errors"], json!(1));
    assert_eq!(payload["reset_by"], json!("admin-user-123"));
    assert!(payload["reset_at"].as_str().is_some());

    let response = client
        .get(format!(
            "{gateway_url}/api/admin/monitoring/resilience-status"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["error_statistics"]["total_errors"], json!(0));
    assert_eq!(payload["recent_errors"], json!([]));
    assert_eq!(
        payload["error_statistics"]["open_circuit_breakers"],
        json!(1)
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_monitoring_resilience_circuit_history_locally_with_trusted_admin_principal(
) {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/monitoring/resilience/circuit-history",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider()],
        vec![],
        vec![sample_key().with_health_fields(
            Some(json!({
                "openai:chat": {
                    "health_score": 0.25,
                    "consecutive_failures": 3,
                    "last_failure_at": "2026-03-30T12:00:00+00:00"
                }
            })),
            Some(json!({
                "openai:chat": {
                    "open": true,
                    "open_at": "2026-03-30T12:00:00+00:00",
                    "next_probe_at": "2099-03-30T12:05:00+00:00",
                    "recovery_seconds": 300,
                    "reason": "错误率过高"
                }
            })),
        )],
    ));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_provider_catalog_reader_for_tests(
                    provider_catalog,
                ),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/monitoring/resilience/circuit-history?limit=10"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["count"], json!(1));
    assert_eq!(payload["items"][0]["event"], json!("opened"));
    assert_eq!(payload["items"][0]["key_id"], json!("provider-key-1"));
    assert_eq!(payload["items"][0]["provider_name"], json!("OpenAI"));
    assert_eq!(payload["items"][0]["api_format"], json!("openai:chat"));
    assert_eq!(payload["items"][0]["reason"], json!("错误率过高"));
    assert_eq!(payload["items"][0]["recovery_seconds"], json!(300));
    assert_eq!(
        payload["items"][0]["timestamp"],
        json!("2026-03-30T12:00:00+00:00")
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_monitoring_user_behavior_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/monitoring/user-behavior/user-1",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(AppState::new().expect("gateway should build"));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/monitoring/user-behavior/user-1?days=30"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["user_id"], json!("user-1"));
    assert_eq!(payload["period_days"], json!(30));
    assert_eq!(payload["event_counts"], json!({}));
    assert_eq!(payload["failed_requests"], json!(0));
    assert_eq!(payload["success_requests"], json!(0));
    assert_eq!(payload["success_rate"], json!(0.0));
    assert_eq!(payload["suspicious_activities"], json!(0));
    assert!(payload["analysis_time"].as_str().is_some());
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}
