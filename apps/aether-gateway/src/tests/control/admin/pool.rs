use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aether_crypto::{encrypt_python_fernet_plaintext, DEVELOPMENT_ENCRYPTION_KEY};
use aether_data::repository::pool_scores::InMemoryPoolMemberScoreRepository;
use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
use aether_data::repository::usage::InMemoryUsageReadRepository;
use aether_data_contracts::repository::pool_scores::{
    PoolMemberHardState, PoolMemberIdentity, PoolMemberProbeStatus, StoredPoolMemberScore,
};
use aether_data_contracts::repository::provider_catalog::{
    ProviderCatalogReadRepository, ProviderCatalogWriteRepository,
};
use aether_data_contracts::repository::usage::StoredRequestUsageAudit;
use axum::body::{to_bytes, Body, Bytes};
use axum::routing::{any, get, post};
use axum::{extract::Request, Router};
use http::{HeaderMap, HeaderValue, StatusCode};
use serde_json::json;

use super::super::{
    build_router_with_state, sample_endpoint, sample_key, sample_provider, start_server, AppState,
};
use crate::admin_api::{
    admin_pool_selection_snapshot_key, maybe_build_local_admin_pool_response, AdminAppState,
    AdminPoolSelectionSnapshot, AdminPoolSelectionSnapshotItem, AdminRequestContext,
    ADMIN_POOL_SELECTION_SNAPSHOT_MAX_ACTIVE_PER_ADMIN_PROVIDER,
    ADMIN_POOL_SELECTION_SNAPSHOT_MAX_TOTAL,
};
use crate::ai_serving::{provider_key_pool_score_id, provider_key_pool_score_scope};
use crate::audit::AdminAuditEvent;
use crate::constants::{
    GATEWAY_HEADER, TRUSTED_ADMIN_MANAGEMENT_TOKEN_ID_HEADER, TRUSTED_ADMIN_SESSION_ID_HEADER,
    TRUSTED_ADMIN_USER_ID_HEADER, TRUSTED_ADMIN_USER_ROLE_HEADER,
};
use crate::control::resolve_public_request_context;
use crate::data::GatewayDataState;

fn trusted_admin_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(GATEWAY_HEADER, HeaderValue::from_static("rust-phase3b"));
    headers.insert(
        TRUSTED_ADMIN_USER_ID_HEADER,
        HeaderValue::from_static("admin-user-123"),
    );
    headers.insert(
        TRUSTED_ADMIN_USER_ROLE_HEADER,
        HeaderValue::from_static("admin"),
    );
    headers.insert(
        TRUSTED_ADMIN_SESSION_ID_HEADER,
        HeaderValue::from_static("session-123"),
    );
    headers.insert(
        TRUSTED_ADMIN_MANAGEMENT_TOKEN_ID_HEADER,
        HeaderValue::from_static("management-token-123"),
    );
    headers
}

async fn local_admin_pool_response(
    state: &AppState,
    method: http::Method,
    uri: &str,
    body: Option<serde_json::Value>,
) -> axum::response::Response<Body> {
    let headers = trusted_admin_headers();
    let request_context = resolve_public_request_context(
        state,
        &method,
        &uri.parse().expect("uri should parse"),
        &headers,
        "trace-123",
    )
    .await
    .expect("request context should resolve");
    let body_bytes = body.map(|value| Bytes::from(value.to_string()));
    maybe_build_local_admin_pool_response(
        &AdminAppState::new(state),
        &AdminRequestContext::new(&request_context),
        body_bytes.as_ref(),
    )
    .await
    .expect("local pool response should build")
    .expect("pool route should resolve locally")
}

fn pool_test_state(
    provider_catalog_repository: Arc<InMemoryProviderCatalogReadRepository>,
) -> AppState {
    AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(
            GatewayDataState::with_provider_catalog_repository_for_tests(
                provider_catalog_repository,
            ),
        )
}

async fn response_json(response: axum::response::Response<Body>) -> serde_json::Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should read"),
    )
    .expect("json body should parse")
}

fn list_payload_key_ids(payload: &serde_json::Value) -> Vec<String> {
    payload["keys"]
        .as_array()
        .expect("keys should be array")
        .iter()
        .filter_map(|item| item["key_id"].as_str())
        .map(ToOwned::to_owned)
        .collect()
}

async fn create_pool_selection_snapshot_payload(
    state: &AppState,
    provider_id: &str,
    body: serde_json::Value,
) -> serde_json::Value {
    let response = local_admin_pool_response(
        state,
        http::Method::POST,
        &format!("/api/admin/pool/{provider_id}/keys/selection-snapshot"),
        Some(body),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    response_json(response).await
}

async fn read_runtime_pool_selection_snapshot(
    state: &AppState,
    snapshot_id: &str,
) -> Option<AdminPoolSelectionSnapshot> {
    state
        .runtime_kv_get(&admin_pool_selection_snapshot_key(snapshot_id))
        .await
        .expect("snapshot runtime lookup should succeed")
        .map(|raw| serde_json::from_str(&raw).expect("snapshot runtime payload should parse"))
}

async fn write_runtime_pool_selection_snapshot(
    state: &AppState,
    snapshot: AdminPoolSelectionSnapshot,
) {
    let key = admin_pool_selection_snapshot_key(&snapshot.id);
    let value = serde_json::to_string(&snapshot).expect("snapshot should serialize");
    state
        .runtime_state()
        .kv_set(&key, value, Some(Duration::from_secs(60 * 60)))
        .await
        .expect("snapshot runtime write should succeed");
}

fn sample_pool_member_score(provider_id: &str, key_id: &str, score: f64) -> StoredPoolMemberScore {
    let score_scope = provider_key_pool_score_scope();
    let score_identity = PoolMemberIdentity::provider_api_key(provider_id, key_id);
    StoredPoolMemberScore {
        id: provider_key_pool_score_id(&score_identity, &score_scope),
        pool_kind: score_identity.pool_kind.clone(),
        pool_id: score_identity.pool_id.clone(),
        member_kind: score_identity.member_kind.clone(),
        member_id: score_identity.member_id.clone(),
        capability: score_scope.capability.clone(),
        scope_kind: score_scope.scope_kind.clone(),
        scope_id: score_scope.scope_id.clone(),
        score,
        hard_state: PoolMemberHardState::Available,
        score_version: 1,
        score_reason: json!({ "weights": { "manual_priority": score } }),
        last_ranked_at: Some(1_700_000_000),
        last_scheduled_at: None,
        last_success_at: None,
        last_failure_at: None,
        failure_count: 0,
        last_probe_attempt_at: None,
        last_probe_success_at: None,
        last_probe_failure_at: None,
        probe_failure_count: 0,
        probe_status: PoolMemberProbeStatus::Ok,
        updated_at: 1_700_000_050,
    }
}

fn sample_provider_key_usage_row(
    id: &str,
    request_id: &str,
    provider_id: &str,
    provider_api_key_id: &str,
    created_at_unix_secs: i64,
    total_tokens: i32,
    total_cost_usd: f64,
) -> StoredRequestUsageAudit {
    StoredRequestUsageAudit::new(
        id.to_string(),
        request_id.to_string(),
        Some("user-1".to_string()),
        Some("api-key-1".to_string()),
        Some("alice".to_string()),
        Some("user key".to_string()),
        "codex".to_string(),
        "gpt-5".to_string(),
        None,
        Some(provider_id.to_string()),
        Some("endpoint-1".to_string()),
        Some(provider_api_key_id.to_string()),
        Some("responses".to_string()),
        Some("openai:responses".to_string()),
        Some("openai".to_string()),
        Some("responses".to_string()),
        Some("openai:responses".to_string()),
        Some("openai".to_string()),
        Some("responses".to_string()),
        false,
        false,
        total_tokens,
        0,
        total_tokens,
        total_cost_usd,
        total_cost_usd,
        Some(200),
        None,
        None,
        Some(120),
        Some(40),
        "completed".to_string(),
        "settled".to_string(),
        created_at_unix_secs,
        created_at_unix_secs + 1,
        Some(created_at_unix_secs + 2),
    )
    .expect("usage row should build")
}

#[tokio::test]
async fn gateway_handles_admin_pool_overview_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/pool/overview",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let pool_provider = sample_provider("provider-openai", "openai", 10).with_transport_fields(
        true,
        false,
        true,
        None,
        None,
        None,
        None,
        None,
        Some(json!({
            "pool_advanced": {
                "lru_enabled": true,
                "cost_window_seconds": 7200,
                "cost_limit_per_key_tokens": 12000
            }
        })),
    );
    let regular_provider = sample_provider("provider-plain", "plain", 20);
    let mut inactive_key = sample_key("key-openai-b", "provider-openai", "openai:chat", "sk-b");
    inactive_key.name = "standby".to_string();
    inactive_key.is_active = false;
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![pool_provider, regular_provider],
        Vec::new(),
        vec![
            sample_key("key-openai-a", "provider-openai", "openai:chat", "sk-a"),
            inactive_key,
            sample_key("key-plain-a", "provider-plain", "openai:chat", "sk-c"),
        ],
    ));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(GatewayDataState::with_provider_catalog_reader_for_tests(
                provider_catalog_repository,
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/api/admin/pool/overview"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    let items = payload["items"]
        .as_array()
        .expect("items should be an array");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["provider_id"], "provider-openai");
    assert_eq!(items[0]["provider_name"], "openai");
    assert_eq!(items[0]["provider_type"], "custom");
    assert_eq!(items[0]["total_keys"], 2);
    assert_eq!(items[0]["active_keys"], 1);
    assert_eq!(items[0]["cooldown_count"], 0);
    assert_eq!(items[0]["pool_enabled"], true);
    assert_eq!(items[0]["provider_hot_count"], 0);
    assert_eq!(items[0]["provider_desired_hot"], 0);
    assert_eq!(items[0]["provider_in_flight"], 0);
    assert_eq!(items[0]["provider_ema_in_flight"], 0.0);
    assert_eq!(items[0]["provider_burst_pending"], false);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_pool_scheduling_presets_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/pool/scheduling-presets",
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
        .get(format!("{gateway_url}/api/admin/pool/scheduling-presets"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    let items = payload.as_array().expect("payload should be an array");
    assert_eq!(items.len(), 14);
    assert_eq!(items[0]["name"], "lru");
    assert_eq!(items[1]["name"], "cache_affinity");
    assert_eq!(items[8]["name"], "pro_first");
    assert_eq!(items[13]["name"], "team_first");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_pool_batch_import_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/pool/provider-openai/keys/batch-import",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-openai", "openai", 10)],
        vec![sample_endpoint(
            "endpoint-openai-chat",
            "provider-openai",
            "openai:chat",
            "https://api.openai.com/v1",
        )],
        vec![],
    ));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository.clone(),
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/pool/provider-openai/keys/batch-import"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "proxy_node_id": "proxy-node-1",
            "keys": [
                {
                    "name": "batch key a",
                    "api_key": "sk-batch-a"
                },
                {
                    "name": "batch key b",
                    "api_key": ""
                }
            ]
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["imported"], 1);
    assert_eq!(payload["skipped"], 0);
    let errors = payload["errors"]
        .as_array()
        .expect("errors should be an array");
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0]["index"], 1);
    assert_eq!(errors[0]["reason"], "api_key is empty");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    let keys = provider_catalog_repository
        .list_keys_by_provider_ids(&["provider-openai".to_string()])
        .await
        .expect("keys should read");
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].name, "batch key a");
    assert_eq!(keys[0].auth_type, "api_key");
    assert_eq!(keys[0].api_formats, Some(json!(["openai:chat"])));
    assert_eq!(
        keys[0].proxy,
        Some(json!({"node_id": "proxy-node-1", "enabled": true}))
    );

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_pool_scores_locally_with_trusted_admin_principal() {
    let provider = sample_provider("provider-openai", "openai", 10).with_transport_fields(
        true,
        false,
        true,
        None,
        None,
        None,
        None,
        None,
        Some(json!({
            "pool_advanced": {
                "enabled": true
            }
        })),
    );
    let mut key = sample_key("key-openai-a", "provider-openai", "openai:chat", "sk-a");
    key.name = "score key".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        vec![key.clone()],
    ));
    let score_scope = provider_key_pool_score_scope();
    let score_identity = PoolMemberIdentity::provider_api_key("provider-openai", "key-openai-a");
    let score_id = provider_key_pool_score_id(&score_identity, &score_scope);
    let pool_score_repository = Arc::new(InMemoryPoolMemberScoreRepository::seed(vec![
        StoredPoolMemberScore {
            id: score_id,
            pool_kind: score_identity.pool_kind.clone(),
            pool_id: score_identity.pool_id.clone(),
            member_kind: score_identity.member_kind.clone(),
            member_id: score_identity.member_id.clone(),
            capability: score_scope.capability.clone(),
            scope_kind: score_scope.scope_kind.clone(),
            scope_id: score_scope.scope_id.clone(),
            score: 0.875,
            hard_state: PoolMemberHardState::Available,
            score_version: 1,
            score_reason: json!({ "weights": { "manual_priority": 0.3 } }),
            last_ranked_at: Some(1_700_000_000),
            last_scheduled_at: Some(1_700_000_010),
            last_success_at: Some(1_700_000_020),
            last_failure_at: None,
            failure_count: 0,
            last_probe_attempt_at: Some(1_700_000_030),
            last_probe_success_at: Some(1_700_000_040),
            last_probe_failure_at: None,
            probe_failure_count: 0,
            probe_status: PoolMemberProbeStatus::Ok,
            updated_at: 1_700_000_050,
        },
    ]));

    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(Arc::clone(
                    &provider_catalog_repository,
                ))
                .with_pool_score_repository_for_tests(Arc::clone(&pool_score_repository)),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/pool/provider-openai/scores"
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
    assert_eq!(
        payload["items"].as_array().map(|items| items.len()),
        Some(1)
    );
    assert_eq!(payload["items"][0]["member_id"], json!("key-openai-a"));
    assert_eq!(payload["items"][0]["capability"], json!("account"));
    assert_eq!(payload["items"][0]["scope_kind"], json!("account"));
    assert_eq!(payload["items"][0]["scope_id"], serde_json::Value::Null);
    assert_eq!(payload["items"][0]["key"]["name"], json!("score key"));
    assert_eq!(payload["items"][0]["probe_status"], json!("ok"));

    gateway_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_pool_trailing_slash_routes_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new()
        .route(
            "/api/admin/pool/provider-openai/keys",
            any({
                let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
                move |_request: Request| {
                    let upstream_hits_inner = Arc::clone(&upstream_hits_inner);
                    async move {
                        *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                        (StatusCode::OK, Body::from("unexpected upstream hit"))
                    }
                }
            }),
        )
        .route(
            "/api/admin/pool/provider-openai/keys/resolve-selection",
            any({
                let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
                move |_request: Request| {
                    let upstream_hits_inner = Arc::clone(&upstream_hits_inner);
                    async move {
                        *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                        (StatusCode::OK, Body::from("unexpected upstream hit"))
                    }
                }
            }),
        );

    let provider = sample_provider("provider-openai", "openai", 10).with_transport_fields(
        true,
        false,
        true,
        None,
        None,
        None,
        None,
        None,
        Some(json!({
            "pool_advanced": {
                "enabled": true
            }
        })),
    );
    let mut first_key = sample_key("key-openai-a", "provider-openai", "openai:chat", "sk-a");
    first_key.name = "alpha".to_string();
    let mut second_key = sample_key("key-openai-b", "provider-openai", "openai:chat", "sk-b");
    second_key.name = "beta".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        vec![first_key, second_key],
    ));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(Arc::clone(
                    &provider_catalog_repository,
                )),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let list_response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/pool/provider-openai/keys/?page=1&page_size=10"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(list_response.status(), StatusCode::OK);
    let list_payload: serde_json::Value =
        list_response.json().await.expect("json body should parse");
    assert_eq!(
        list_payload["keys"].as_array().map(|items| items.len()),
        Some(2)
    );

    let resolve_response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/pool/provider-openai/keys/resolve-selection/"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "search": "a",
            "quick_selectors": []
        }))
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(resolve_response.status(), StatusCode::OK);
    let resolve_payload: serde_json::Value = resolve_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(resolve_payload["total"], json!(2));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_pool_list_includes_usage_totals_and_nullable_lru_score() {
    let provider = sample_provider("provider-openai", "openai", 10).with_transport_fields(
        true,
        false,
        true,
        None,
        None,
        None,
        None,
        None,
        Some(json!({
            "pool_advanced": {
                "enabled": true
            }
        })),
    );
    let mut key = sample_key(
        "key-openai-usage",
        "provider-openai",
        "openai:chat",
        "sk-usage",
    );
    key.name = "usage key".to_string();
    key.request_count = Some(1566);
    key.total_tokens = 187_327_321;
    key.total_cost_usd = 93.1319297;

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        vec![key],
    ));
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(GatewayDataState::with_provider_catalog_reader_for_tests(
            provider_catalog_repository,
        ));

    let response = local_admin_pool_response(
        &state,
        http::Method::GET,
        "/api/admin/pool/provider-openai/keys?page=1&page_size=50&status=all",
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should read"),
    )
    .expect("json body should parse");
    let keys = payload["keys"].as_array().expect("keys should be array");
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0]["request_count"], json!(1566));
    assert_eq!(keys[0]["total_tokens"], json!(187_327_321u64));
    assert_eq!(keys[0]["total_cost_usd"], json!("93.13192970"));
    assert!(keys[0]["lru_score"].is_null());
}

#[tokio::test]
async fn gateway_pool_list_ignores_usage_rows_and_uses_persisted_key_stats() {
    let provider = sample_provider("provider-openai", "openai", 10).with_transport_fields(
        true,
        false,
        true,
        None,
        None,
        None,
        None,
        None,
        Some(json!({
            "pool_advanced": {
                "enabled": true
            }
        })),
    );
    let mut key = sample_key(
        "key-openai-persisted",
        "provider-openai",
        "openai:chat",
        "sk-persisted",
    );
    key.name = "persisted usage key".to_string();
    key.request_count = Some(7);
    key.total_tokens = 1_024;
    key.total_cost_usd = 3.5;
    key.created_at_unix_ms = Some(1_711_000_000);
    key.last_used_at_unix_secs = Some(1_711_000_999);

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        vec![key],
    ));
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(GatewayDataState::with_provider_catalog_reader_for_tests(
            provider_catalog_repository,
        ));

    let response = local_admin_pool_response(
        &state,
        http::Method::GET,
        "/api/admin/pool/provider-openai/keys?page=1&page_size=50&status=all",
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should read"),
    )
    .expect("json body should parse");
    let keys = payload["keys"].as_array().expect("keys should be array");
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0]["request_count"], json!(7));
    assert_eq!(keys[0]["total_tokens"], json!(1_024u64));
    assert_eq!(keys[0]["total_cost_usd"], json!("3.50000000"));
    assert_eq!(
        keys[0]["last_used_at"],
        json!(crate::handlers::shared::unix_secs_to_rfc3339(1_711_000_999))
    );
    assert_eq!(
        keys[0]["imported_at"],
        json!(crate::handlers::shared::unix_secs_to_rfc3339(1_711_000_000))
    );
}

#[tokio::test]
async fn gateway_handles_admin_pool_list_keys_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/pool/provider-openai/keys",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let provider = sample_provider("provider-openai", "openai", 10).with_transport_fields(
        true,
        false,
        true,
        None,
        None,
        None,
        None,
        None,
        Some(json!({
            "pool_advanced": {
                "enabled": true,
                "cost_limit_per_key_tokens": 12000
            }
        })),
    );
    let mut primary_key = sample_key("key-openai-a", "provider-openai", "openai:chat", "sk-a");
    primary_key.name = "alpha".to_string();
    let mut cooldown_key = sample_key("key-openai-b", "provider-openai", "openai:chat", "sk-b");
    cooldown_key.name = "beta".to_string();
    cooldown_key.status_snapshot = Some(json!({
        "account": {
            "code": "cooldown",
            "label": "冷却中",
            "blocked": false
        }
    }));
    let mut inactive_key = sample_key("key-openai-c", "provider-openai", "openai:chat", "sk-c");
    inactive_key.name = "gamma".to_string();
    inactive_key.is_active = false;
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        vec![primary_key, cooldown_key, inactive_key],
    ));
    let score_scope = provider_key_pool_score_scope();
    let score_identity = PoolMemberIdentity::provider_api_key("provider-openai", "key-openai-a");
    let pool_score_repository = Arc::new(InMemoryPoolMemberScoreRepository::seed(vec![
        StoredPoolMemberScore {
            id: provider_key_pool_score_id(&score_identity, &score_scope),
            pool_kind: score_identity.pool_kind.clone(),
            pool_id: score_identity.pool_id.clone(),
            member_kind: score_identity.member_kind.clone(),
            member_id: score_identity.member_id.clone(),
            capability: score_scope.capability.clone(),
            scope_kind: score_scope.scope_kind.clone(),
            scope_id: score_scope.scope_id.clone(),
            score: 0.875,
            hard_state: PoolMemberHardState::Available,
            score_version: 1,
            score_reason: json!({ "weights": { "manual_priority": 0.3 } }),
            last_ranked_at: Some(1_700_000_000),
            last_scheduled_at: Some(1_700_000_010),
            last_success_at: Some(1_700_000_020),
            last_failure_at: None,
            failure_count: 0,
            last_probe_attempt_at: Some(1_700_000_030),
            last_probe_success_at: Some(1_700_000_040),
            last_probe_failure_at: None,
            probe_failure_count: 0,
            probe_status: PoolMemberProbeStatus::Ok,
            updated_at: 1_700_000_050,
        },
    ]));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_reader_for_tests(
                    provider_catalog_repository,
                )
                .with_pool_score_repository_for_tests(pool_score_repository),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/pool/provider-openai/keys?page=1&page_size=2&search=a&status=all"
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
    assert_eq!(payload["total"], json!(3));
    assert_eq!(payload["page"], json!(1));
    assert_eq!(payload["page_size"], json!(2));
    let keys = payload["keys"].as_array().expect("keys should be array");
    assert_eq!(keys.len(), 2);
    assert_eq!(keys[0]["key_name"], json!("alpha"));
    assert_eq!(keys[0]["pool_score"]["score"], json!(0.875));
    assert_eq!(keys[0]["pool_score"]["scope_kind"], json!("account"));
    assert_eq!(keys[0]["pool_score"]["scope_id"], serde_json::Value::Null);
    assert_eq!(keys[0]["scheduling_status"], json!("available"));
    assert_eq!(keys[1]["key_name"], json!("beta"));
    assert_eq!(keys[1]["scheduling_reason"], json!("available"));

    let page_two_response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/pool/provider-openai/keys?page=2&page_size=2&search=a&status=all"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(page_two_response.status(), StatusCode::OK);
    let page_two_payload: serde_json::Value = page_two_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(page_two_payload["total"], json!(3));
    let page_two_keys = page_two_payload["keys"]
        .as_array()
        .expect("keys should be array");
    assert_eq!(page_two_keys.len(), 1);
    assert_eq!(page_two_keys[0]["key_name"], json!("gamma"));

    let active_response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/pool/provider-openai/keys?page=1&page_size=10&search=a&status=active"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(active_response.status(), StatusCode::OK);
    let active_payload: serde_json::Value = active_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(active_payload["total"], json!(2));
    assert_eq!(
        active_payload["keys"].as_array().map(|items| items.len()),
        Some(2)
    );

    let inactive_response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/pool/provider-openai/keys?page=1&page_size=10&search=a&status=inactive"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(inactive_response.status(), StatusCode::OK);
    let inactive_payload: serde_json::Value = inactive_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(inactive_payload["total"], json!(1));
    let inactive_keys = inactive_payload["keys"]
        .as_array()
        .expect("keys should be array");
    assert_eq!(inactive_keys.len(), 1);
    assert_eq!(inactive_keys[0]["key_name"], json!("gamma"));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_pool_list_keys_keeps_legacy_full_search_for_default_filtered_scope() {
    let provider = sample_provider("provider-openai", "openai", 10);
    let mut inactive_note_key = sample_key(
        "key-openai-note",
        "provider-openai",
        "openai:chat",
        "sk-note",
    );
    inactive_note_key.name = "plain inactive account".to_string();
    inactive_note_key.note = Some("legacy-search-marker".to_string());
    inactive_note_key.is_active = false;
    let mut active_note_key = sample_key(
        "key-openai-active",
        "provider-openai",
        "openai:chat",
        "sk-active",
    );
    active_note_key.name = "plain active account".to_string();
    active_note_key.note = Some("legacy-search-marker".to_string());

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        vec![inactive_note_key, active_note_key],
    ));
    let state = pool_test_state(provider_catalog_repository);

    let legacy_response = local_admin_pool_response(
        &state,
        http::Method::GET,
        "/api/admin/pool/provider-openai/keys?page=1&page_size=10&search=legacy-search-marker&status=inactive",
        None,
    )
    .await;
    assert_eq!(legacy_response.status(), StatusCode::OK);
    let legacy_payload = response_json(legacy_response).await;
    assert_eq!(legacy_payload["total"], json!(1));
    assert_eq!(
        list_payload_key_ids(&legacy_payload),
        vec!["key-openai-note"]
    );

    let name_scope_response = local_admin_pool_response(
        &state,
        http::Method::GET,
        "/api/admin/pool/provider-openai/keys?page=1&page_size=10&search=legacy-search-marker&status=inactive&search_scope=name",
        None,
    )
    .await;
    assert_eq!(name_scope_response.status(), StatusCode::OK);
    let name_scope_payload = response_json(name_scope_response).await;
    assert_eq!(name_scope_payload["total"], json!(0));
}

#[tokio::test]
async fn gateway_pool_create_selection_snapshot_freezes_filtered_result() {
    let provider = sample_provider("provider-openai", "openai", 10);
    let mut first_key = sample_key("key-openai-a", "provider-openai", "openai:chat", "sk-a");
    first_key.name = "alpha first".to_string();
    let mut second_key = sample_key("key-openai-b", "provider-openai", "openai:chat", "sk-b");
    second_key.name = "alpha second".to_string();
    let mut other_key = sample_key("key-openai-c", "provider-openai", "openai:chat", "sk-c");
    other_key.name = "beta other".to_string();

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        vec![first_key, second_key, other_key],
    ));
    let state = pool_test_state(Arc::clone(&provider_catalog_repository));

    let response = local_admin_pool_response(
        &state,
        http::Method::GET,
        "/api/admin/pool/provider-openai/keys?page=1&page_size=1&search=alpha&status=all",
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should read"),
    )
    .expect("json body should parse");
    assert_eq!(payload["total"], json!(2));
    assert_eq!(payload["keys"].as_array().map(Vec::len), Some(1));
    assert!(payload.get("selection_snapshot").is_none());

    let snapshot_payload = create_pool_selection_snapshot_payload(
        &state,
        "provider-openai",
        json!({
            "page": 1,
            "page_size": 1,
            "search": "alpha",
            "status": "all",
            "expected_total": payload["total"],
            "expected_page_key_ids": list_payload_key_ids(&payload),
        }),
    )
    .await;
    assert_eq!(snapshot_payload["selection_snapshot"]["total"], json!(2));
    assert_eq!(
        snapshot_payload["selection_snapshot"]["status"],
        json!("ready")
    );
    let snapshot_id = snapshot_payload["selection_snapshot"]["id"]
        .as_str()
        .expect("snapshot id should be present")
        .to_string();

    let mut new_key = sample_key("key-openai-d", "provider-openai", "openai:chat", "sk-d");
    new_key.name = "alpha newly imported".to_string();
    provider_catalog_repository
        .create_key(&new_key)
        .await
        .expect("new matching key should be stored");

    let snapshot = read_runtime_pool_selection_snapshot(&state, &snapshot_id)
        .await
        .expect("snapshot should exist");
    assert_eq!(snapshot.total, 2);
    assert_eq!(snapshot.created_by.as_deref(), Some("admin-user-123"));

    let snapshot_key_ids = snapshot
        .items
        .iter()
        .map(|item| item.key_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(snapshot_key_ids.len(), 2);
    assert!(!snapshot_key_ids.contains(&"key-openai-d"));

    let resolve_response = local_admin_pool_response(
        &state,
        http::Method::POST,
        "/api/admin/pool/provider-openai/keys/resolve-selection",
        Some(json!({
            "snapshot_id": snapshot_id,
            "expected_total": 2
        })),
    )
    .await;
    assert_eq!(resolve_response.status(), StatusCode::OK);
    let resolve_payload: serde_json::Value = serde_json::from_slice(
        &to_bytes(resolve_response.into_body(), usize::MAX)
            .await
            .expect("body should read"),
    )
    .expect("json body should parse");
    let resolved_ids = resolve_payload["items"]
        .as_array()
        .expect("resolved items should be array")
        .iter()
        .filter_map(|item| item["key_id"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(resolve_payload["total"], json!(2));
    assert_eq!(resolved_ids.len(), 2);
    assert!(!resolved_ids.contains(&"key-openai-d"));
}

#[tokio::test]
async fn gateway_pool_list_keys_stays_read_only_for_legacy_snapshot_query_params() {
    let provider = sample_provider("provider-openai", "openai", 10);
    let mut key = sample_key("key-openai-a", "provider-openai", "openai:chat", "sk-a");
    key.name = "alpha first".to_string();

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        vec![key],
    ));
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(
            GatewayDataState::with_provider_catalog_repository_for_tests(
                provider_catalog_repository,
            ),
        );

    let response = local_admin_pool_response(
        &state,
        http::Method::GET,
        "/api/admin/pool/provider-openai/keys?page=1&page_size=1&search=alpha&status=all&include_selection_snapshot=true",
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let payload = response_json(response).await;
    assert_eq!(payload["total"], json!(1));
    assert!(payload.get("selection_snapshot").is_none());
    assert!(payload.get("selection_snapshot_mismatch").is_none());
}

#[tokio::test]
async fn gateway_pool_selection_snapshot_uses_same_full_search_scope_as_list() {
    let provider = sample_provider("provider-openai", "openai", 10);
    let mut oauth_key = sample_key("key-openai-oauth", "provider-openai", "openai:chat", "sk-a");
    oauth_key.name = "plain account".to_string();
    oauth_key.auth_type = "oauth".to_string();
    let mut note_key = sample_key("key-openai-note", "provider-openai", "openai:chat", "sk-b");
    note_key.name = "ordinary account".to_string();
    note_key.note = Some("contains-search-marker".to_string());
    let mut other_key = sample_key("key-openai-other", "provider-openai", "openai:chat", "sk-c");
    other_key.name = "unrelated account".to_string();

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        vec![oauth_key, note_key, other_key],
    ));
    let state = pool_test_state(Arc::clone(&provider_catalog_repository));

    let list_response = local_admin_pool_response(
        &state,
        http::Method::GET,
        "/api/admin/pool/provider-openai/keys?page=1&page_size=10&search=oauth&search_scope=full&status=all",
        None,
    )
    .await;
    assert_eq!(list_response.status(), StatusCode::OK);
    let list_payload = response_json(list_response).await;
    assert_eq!(list_payload["total"], json!(1));
    assert_eq!(
        list_payload_key_ids(&list_payload),
        vec!["key-openai-oauth"]
    );

    let snapshot_payload = create_pool_selection_snapshot_payload(
        &state,
        "provider-openai",
        json!({
            "page": 1,
            "page_size": 10,
            "search": "oauth",
            "search_scope": "full",
            "status": "all",
            "expected_total": list_payload["total"],
            "expected_page_key_ids": list_payload_key_ids(&list_payload),
        }),
    )
    .await;
    assert!(snapshot_payload
        .get("selection_snapshot_mismatch")
        .is_none());
    assert_eq!(snapshot_payload["selection_snapshot"]["total"], json!(1));

    let snapshot_id = snapshot_payload["selection_snapshot"]["id"]
        .as_str()
        .expect("snapshot id should be present");
    let snapshot = read_runtime_pool_selection_snapshot(&state, snapshot_id)
        .await
        .expect("snapshot should exist");
    assert_eq!(snapshot.items.len(), 1);
    assert_eq!(snapshot.items[0].key_id, "key-openai-oauth");
}

#[tokio::test]
async fn gateway_pool_selection_snapshot_reports_drift_while_writing_current_snapshot() {
    let provider = sample_provider("provider-openai", "openai", 10);
    let mut first_key = sample_key("key-openai-a", "provider-openai", "openai:chat", "sk-a");
    first_key.name = "alpha first".to_string();
    let mut second_key = sample_key("key-openai-b", "provider-openai", "openai:chat", "sk-b");
    second_key.name = "alpha second".to_string();
    let mut third_key = sample_key("key-openai-c", "provider-openai", "openai:chat", "sk-c");
    third_key.name = "alpha third".to_string();

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        vec![first_key, second_key, third_key],
    ));
    let state = pool_test_state(Arc::clone(&provider_catalog_repository));

    let response = local_admin_pool_response(
        &state,
        http::Method::POST,
        "/api/admin/pool/provider-openai/keys/selection-snapshot",
        Some(json!({
            "page": 1,
            "page_size": 2,
            "search": "alpha",
            "status": "all",
            "expected_total": 2,
            "expected_page_key_ids": ["key-openai-a", "key-openai-b"],
        })),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let payload = response_json(response).await;
    assert_eq!(payload["total"], json!(3));
    assert_eq!(payload["selection_snapshot"]["total"], json!(3));
    assert_eq!(
        payload["selection_snapshot_mismatch"],
        json!({
            "reason": "total_changed",
            "expected_total": 2,
            "actual_total": 3,
        })
    );

    let response = local_admin_pool_response(
        &state,
        http::Method::POST,
        "/api/admin/pool/provider-openai/keys/selection-snapshot",
        Some(json!({
            "page": 1,
            "page_size": 2,
            "search": "alpha",
            "status": "all",
            "expected_total": 3,
            "expected_page_key_ids": ["key-openai-b", "key-openai-a"],
        })),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let payload = response_json(response).await;
    assert_eq!(payload["total"], json!(3));
    assert_eq!(payload["selection_snapshot"]["total"], json!(3));
    assert_eq!(
        payload["selection_snapshot_mismatch"],
        json!({
            "reason": "page_keys_changed",
            "expected_total": 3,
            "actual_total": 3,
        })
    );

    let snapshot_id = payload["selection_snapshot"]["id"]
        .as_str()
        .expect("snapshot id should be present");
    assert!(
        read_runtime_pool_selection_snapshot(&state, snapshot_id)
            .await
            .is_some(),
        "drifted selections should still write snapshots"
    );
}

#[tokio::test]
async fn gateway_pool_selection_snapshot_uses_same_default_page_order_as_list() {
    let provider = sample_provider("provider-openai", "openai", 10);
    let mut alpha_key = sample_key("key-openai-alpha", "provider-openai", "openai:chat", "sk-a");
    alpha_key.name = "Alpha same timestamp".to_string();
    alpha_key.created_at_unix_ms = Some(2_000);
    let mut beta_key = sample_key("key-openai-beta", "provider-openai", "openai:chat", "sk-b");
    beta_key.name = "beta same timestamp".to_string();
    beta_key.created_at_unix_ms = Some(2_000);
    let mut older_key = sample_key("key-openai-older", "provider-openai", "openai:chat", "sk-c");
    older_key.name = "older account".to_string();
    older_key.created_at_unix_ms = Some(1_000);

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        vec![older_key, beta_key, alpha_key],
    ));
    let state = pool_test_state(Arc::clone(&provider_catalog_repository));

    let list_response = local_admin_pool_response(
        &state,
        http::Method::GET,
        "/api/admin/pool/provider-openai/keys?page=1&page_size=2&status=all",
        None,
    )
    .await;
    assert_eq!(list_response.status(), StatusCode::OK);
    let list_payload = response_json(list_response).await;
    assert_eq!(list_payload["total"], json!(3));
    let list_page_key_ids = list_payload_key_ids(&list_payload);
    assert_eq!(list_page_key_ids.len(), 2);

    let snapshot_payload = create_pool_selection_snapshot_payload(
        &state,
        "provider-openai",
        json!({
            "page": 1,
            "page_size": 2,
            "status": "all",
            "expected_total": list_payload["total"],
            "expected_page_key_ids": list_page_key_ids,
        }),
    )
    .await;
    assert!(snapshot_payload
        .get("selection_snapshot_mismatch")
        .is_none());
    assert_eq!(snapshot_payload["selection_snapshot"]["total"], json!(3));

    let snapshot_id = snapshot_payload["selection_snapshot"]["id"]
        .as_str()
        .expect("snapshot id should be present");
    let snapshot = read_runtime_pool_selection_snapshot(&state, snapshot_id)
        .await
        .expect("snapshot should exist");
    let snapshot_first_page_ids = snapshot
        .items
        .into_iter()
        .take(2)
        .map(|item| item.key_id)
        .collect::<Vec<_>>();
    assert_eq!(snapshot_first_page_ids, list_payload_key_ids(&list_payload));
}

#[tokio::test]
async fn gateway_pool_batch_action_uses_selection_snapshot_targets() {
    let provider = sample_provider("provider-openai", "openai", 10);
    let mut first_key = sample_key("key-openai-a", "provider-openai", "openai:chat", "sk-a");
    first_key.name = "alpha first".to_string();
    let mut second_key = sample_key("key-openai-b", "provider-openai", "openai:chat", "sk-b");
    second_key.name = "alpha second".to_string();
    let mut other_key = sample_key("key-openai-c", "provider-openai", "openai:chat", "sk-c");
    other_key.name = "beta other".to_string();

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        vec![first_key, second_key, other_key],
    ));
    let state = pool_test_state(Arc::clone(&provider_catalog_repository));

    let list_response = local_admin_pool_response(
        &state,
        http::Method::GET,
        "/api/admin/pool/provider-openai/keys?page=1&page_size=1&search=alpha&status=all",
        None,
    )
    .await;
    assert_eq!(list_response.status(), StatusCode::OK);
    let list_payload: serde_json::Value = serde_json::from_slice(
        &to_bytes(list_response.into_body(), usize::MAX)
            .await
            .expect("body should read"),
    )
    .expect("json body should parse");
    let snapshot_payload = create_pool_selection_snapshot_payload(
        &state,
        "provider-openai",
        json!({
            "page": 1,
            "page_size": 1,
            "search": "alpha",
            "status": "all",
            "expected_total": list_payload["total"],
            "expected_page_key_ids": list_payload_key_ids(&list_payload),
        }),
    )
    .await;
    let snapshot_id = snapshot_payload["selection_snapshot"]["id"]
        .as_str()
        .expect("snapshot id should be present")
        .to_string();
    assert_eq!(snapshot_payload["selection_snapshot"]["total"], json!(2));

    let mismatch_response = local_admin_pool_response(
        &state,
        http::Method::POST,
        "/api/admin/pool/provider-openai/keys/batch-action",
        Some(json!({
            "selection": {
                "type": "snapshot",
                "snapshot_id": snapshot_id.clone(),
                "expected_total": 3
            },
            "action": "disable"
        })),
    )
    .await;
    assert_eq!(mismatch_response.status(), StatusCode::CONFLICT);

    let mut new_key = sample_key("key-openai-d", "provider-openai", "openai:chat", "sk-d");
    new_key.name = "alpha newly imported".to_string();
    provider_catalog_repository
        .create_key(&new_key)
        .await
        .expect("new matching key should be stored");

    let action_response = local_admin_pool_response(
        &state,
        http::Method::POST,
        "/api/admin/pool/provider-openai/keys/batch-action",
        Some(json!({
            "selection": {
                "type": "snapshot",
                "snapshot_id": snapshot_id,
                "expected_total": 2
            },
            "action": "disable"
        })),
    )
    .await;
    assert_eq!(action_response.status(), StatusCode::OK);
    let action_payload: serde_json::Value = serde_json::from_slice(
        &to_bytes(action_response.into_body(), usize::MAX)
            .await
            .expect("body should read"),
    )
    .expect("json body should parse");
    assert_eq!(action_payload["affected"], json!(2));

    let stored_keys = provider_catalog_repository
        .list_keys_by_ids(&[
            "key-openai-a".to_string(),
            "key-openai-b".to_string(),
            "key-openai-d".to_string(),
        ])
        .await
        .expect("keys should list")
        .into_iter()
        .map(|key| (key.id, key.is_active))
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(stored_keys.get("key-openai-a"), Some(&false));
    assert_eq!(stored_keys.get("key-openai-b"), Some(&false));
    assert_eq!(stored_keys.get("key-openai-d"), Some(&true));
}

#[tokio::test]
async fn gateway_pool_selection_snapshot_keeps_only_recent_runtime_snapshots() {
    let provider = sample_provider("provider-openai", "openai", 10);
    let mut key = sample_key("key-openai-a", "provider-openai", "openai:chat", "sk-a");
    key.name = "alpha only".to_string();

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        vec![key],
    ));
    let state = pool_test_state(Arc::clone(&provider_catalog_repository));

    let mut snapshot_ids = Vec::new();
    for _ in 0..=ADMIN_POOL_SELECTION_SNAPSHOT_MAX_ACTIVE_PER_ADMIN_PROVIDER {
        let snapshot_payload = create_pool_selection_snapshot_payload(
            &state,
            "provider-openai",
            json!({
                "page": 1,
                "page_size": 1,
                "search": "alpha",
                "status": "all",
                "expected_total": 1,
                "expected_page_key_ids": ["key-openai-a"],
            }),
        )
        .await;
        snapshot_ids.push(
            snapshot_payload["selection_snapshot"]["id"]
                .as_str()
                .expect("snapshot id should be present")
                .to_string(),
        );
    }

    assert!(
        read_runtime_pool_selection_snapshot(&state, &snapshot_ids[0])
            .await
            .is_none(),
        "oldest runtime snapshot should be pruned"
    );
    assert!(
        read_runtime_pool_selection_snapshot(
            &state,
            snapshot_ids
                .last()
                .expect("latest snapshot id should exist"),
        )
        .await
        .is_some(),
        "latest runtime snapshot should remain"
    );
}

#[tokio::test]
async fn gateway_pool_snapshot_batch_action_uses_runtime_snapshot_blob_membership() {
    let provider = sample_provider("provider-openai", "openai", 10);
    let mut first_key = sample_key("key-openai-a", "provider-openai", "openai:chat", "sk-a");
    first_key.name = "alpha first".to_string();
    let mut second_key = sample_key("key-openai-b", "provider-openai", "openai:chat", "sk-b");
    second_key.name = "alpha second".to_string();

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        vec![first_key, second_key],
    ));
    let state = pool_test_state(Arc::clone(&provider_catalog_repository));
    write_runtime_pool_selection_snapshot(
        &state,
        AdminPoolSelectionSnapshot {
            id: "snapshot-runtime".to_string(),
            provider_id: "provider-openai".to_string(),
            created_by: Some("admin-user-123".to_string()),
            filter_json: None,
            total: 2,
            status: "ready".to_string(),
            created_at_unix_secs: 100,
            expires_at_unix_secs: 4_000_000_000,
            items: vec![
                AdminPoolSelectionSnapshotItem {
                    ordinal: 0,
                    key_id: "key-openai-a".to_string(),
                    key_updated_at_unix_secs: None,
                },
                AdminPoolSelectionSnapshotItem {
                    ordinal: 1,
                    key_id: "key-openai-b".to_string(),
                    key_updated_at_unix_secs: None,
                },
            ],
        },
    )
    .await;

    let action_response = local_admin_pool_response(
        &state,
        http::Method::POST,
        "/api/admin/pool/provider-openai/keys/batch-action",
        Some(json!({
            "selection": {
                "type": "snapshot",
                "snapshot_id": "snapshot-runtime",
                "expected_total": 2
            },
            "action": "disable"
        })),
    )
    .await;
    assert_eq!(action_response.status(), StatusCode::OK);
    let action_payload = response_json(action_response).await;
    assert_eq!(action_payload["affected"], json!(2));

    let stored_keys = provider_catalog_repository
        .list_keys_by_ids(&["key-openai-a".to_string(), "key-openai-b".to_string()])
        .await
        .expect("keys should list")
        .into_iter()
        .map(|key| (key.id, key.is_active))
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(stored_keys.get("key-openai-a"), Some(&false));
    assert_eq!(stored_keys.get("key-openai-b"), Some(&false));
}

#[tokio::test]
async fn gateway_pool_selection_snapshot_handles_large_result_sets_consistently() {
    const KEY_COUNT: usize = 5_000;

    let provider = sample_provider("provider-openai", "openai", 10);
    let mut keys = Vec::with_capacity(KEY_COUNT + 25);
    for index in 0..KEY_COUNT {
        let key_id = format!("key-openai-bulk-{index:05}");
        let mut key = sample_key(
            &key_id,
            "provider-openai",
            "openai:chat",
            &format!("sk-bulk-{index:05}"),
        );
        key.name = format!("bulk alpha {index:05}");
        key.updated_at_unix_secs = Some(1_700_000_000 + index as u64);
        keys.push(key);
    }
    for index in 0..25 {
        let key_id = format!("key-openai-noise-{index:05}");
        let mut key = sample_key(
            &key_id,
            "provider-openai",
            "openai:chat",
            &format!("sk-noise-{index:05}"),
        );
        key.name = format!("noise beta {index:05}");
        keys.push(key);
    }

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        keys,
    ));
    let state = pool_test_state(Arc::clone(&provider_catalog_repository));

    let list_response = local_admin_pool_response(
        &state,
        http::Method::GET,
        "/api/admin/pool/provider-openai/keys?page=1&page_size=25&search=bulk%20alpha&status=all",
        None,
    )
    .await;
    assert_eq!(list_response.status(), StatusCode::OK);
    let list_payload = response_json(list_response).await;
    assert_eq!(list_payload["total"], json!(KEY_COUNT));
    assert_eq!(list_payload["keys"].as_array().map(Vec::len), Some(25));
    let snapshot_payload = create_pool_selection_snapshot_payload(
        &state,
        "provider-openai",
        json!({
            "page": 1,
            "page_size": 25,
            "search": "bulk alpha",
            "status": "all",
            "expected_total": list_payload["total"],
            "expected_page_key_ids": list_payload_key_ids(&list_payload),
        }),
    )
    .await;
    assert_eq!(
        snapshot_payload["selection_snapshot"]["total"],
        json!(KEY_COUNT)
    );
    let snapshot_id = snapshot_payload["selection_snapshot"]["id"]
        .as_str()
        .expect("snapshot id should be present")
        .to_string();

    let snapshot = read_runtime_pool_selection_snapshot(&state, &snapshot_id)
        .await
        .expect("snapshot should exist");
    let first_page = snapshot.items.iter().take(1_000).collect::<Vec<_>>();
    assert_eq!(first_page.len(), 1_000);
    assert_eq!(first_page.first().map(|item| item.ordinal), Some(0));
    assert_eq!(
        first_page.first().map(|item| item.key_id.as_str()),
        Some("key-openai-bulk-00000")
    );
    assert_eq!(first_page.last().map(|item| item.ordinal), Some(999));

    let tail_page = snapshot
        .items
        .iter()
        .skip(4_000)
        .take(1_100)
        .collect::<Vec<_>>();
    assert_eq!(tail_page.len(), 1_000);
    assert_eq!(tail_page.first().map(|item| item.ordinal), Some(4_000));
    assert_eq!(
        tail_page.last().map(|item| item.key_id.as_str()),
        Some("key-openai-bulk-04999")
    );

    let resolve_response = local_admin_pool_response(
        &state,
        http::Method::POST,
        "/api/admin/pool/provider-openai/keys/resolve-selection",
        Some(json!({
            "snapshot_id": snapshot_id,
            "expected_total": KEY_COUNT
        })),
    )
    .await;
    assert_eq!(resolve_response.status(), StatusCode::OK);
    let resolve_payload = response_json(resolve_response).await;
    let resolved_items = resolve_payload["items"]
        .as_array()
        .expect("resolved items should be array");
    assert_eq!(resolve_payload["total"], json!(KEY_COUNT));
    assert_eq!(resolved_items.len(), KEY_COUNT);
    assert_eq!(resolved_items[0]["key_id"], json!("key-openai-bulk-00000"));
    assert_eq!(
        resolved_items[KEY_COUNT - 1]["key_id"],
        json!("key-openai-bulk-04999")
    );

    let snapshot_id = snapshot_payload["selection_snapshot"]["id"]
        .as_str()
        .expect("snapshot id should be present")
        .to_string();
    let action_response = local_admin_pool_response(
        &state,
        http::Method::POST,
        "/api/admin/pool/provider-openai/keys/batch-action",
        Some(json!({
            "selection": {
                "type": "snapshot",
                "snapshot_id": snapshot_id,
                "expected_total": KEY_COUNT
            },
            "action": "disable"
        })),
    )
    .await;
    assert_eq!(action_response.status(), StatusCode::OK);
    let action_payload = response_json(action_response).await;
    assert_eq!(action_payload["affected"], json!(KEY_COUNT));

    let sampled_keys = provider_catalog_repository
        .list_keys_by_ids(&[
            "key-openai-bulk-00000".to_string(),
            "key-openai-bulk-02500".to_string(),
            "key-openai-bulk-04999".to_string(),
            "key-openai-noise-00000".to_string(),
        ])
        .await
        .expect("sampled keys should list")
        .into_iter()
        .map(|key| (key.id, key.is_active))
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(sampled_keys.get("key-openai-bulk-00000"), Some(&false));
    assert_eq!(sampled_keys.get("key-openai-bulk-02500"), Some(&false));
    assert_eq!(sampled_keys.get("key-openai-bulk-04999"), Some(&false));
    assert_eq!(sampled_keys.get("key-openai-noise-00000"), Some(&true));
}

#[tokio::test]
async fn gateway_pool_selection_snapshot_rejects_results_above_runtime_limit() {
    let provider = sample_provider("provider-openai", "openai", 10);
    let mut keys = Vec::with_capacity(ADMIN_POOL_SELECTION_SNAPSHOT_MAX_TOTAL + 1);
    for index in 0..=ADMIN_POOL_SELECTION_SNAPSHOT_MAX_TOTAL {
        let key_id = format!("key-openai-over-limit-{index:05}");
        let mut key = sample_key(
            &key_id,
            "provider-openai",
            "openai:chat",
            &format!("sk-over-limit-{index:05}"),
        );
        key.name = format!("over limit alpha {index:05}");
        keys.push(key);
    }

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        keys,
    ));
    let state = pool_test_state(Arc::clone(&provider_catalog_repository));

    let response = local_admin_pool_response(
        &state,
        http::Method::POST,
        "/api/admin/pool/provider-openai/keys/selection-snapshot",
        Some(json!({
            "page": 1,
            "page_size": 50,
            "search": "over limit alpha",
            "status": "all",
            "expected_total": ADMIN_POOL_SELECTION_SNAPSHOT_MAX_TOTAL + 1,
            "expected_page_key_ids": (0..50)
                .map(|index| format!("key-openai-over-limit-{index:05}"))
                .collect::<Vec<_>>(),
        })),
    )
    .await;

    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gateway_pool_selection_snapshot_stays_consistent_under_concurrent_growth() {
    const INITIAL_KEYS: usize = 80;
    const ADDED_KEYS: usize = 120;
    const SNAPSHOT_REQUESTS: usize = 24;

    let provider = sample_provider("provider-openai", "openai", 10);
    let mut keys = Vec::with_capacity(INITIAL_KEYS);
    for index in 0..INITIAL_KEYS {
        let key_id = format!("key-openai-initial-{index:03}");
        let mut key = sample_key(
            &key_id,
            "provider-openai",
            "openai:chat",
            &format!("sk-initial-{index:03}"),
        );
        key.name = format!("alpha concurrent initial {index:03}");
        keys.push(key);
    }

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        keys,
    ));
    let state = pool_test_state(Arc::clone(&provider_catalog_repository));
    let start_barrier = Arc::new(tokio::sync::Barrier::new(SNAPSHOT_REQUESTS + 1));

    let writer = tokio::spawn({
        let provider_catalog_repository = Arc::clone(&provider_catalog_repository);
        let start_barrier = Arc::clone(&start_barrier);
        async move {
            start_barrier.wait().await;
            for index in 0..ADDED_KEYS {
                let key_id = format!("key-openai-added-{index:03}");
                let mut key = sample_key(
                    &key_id,
                    "provider-openai",
                    "openai:chat",
                    &format!("sk-added-{index:03}"),
                );
                key.name = format!("alpha concurrent added {index:03}");
                provider_catalog_repository
                    .create_key(&key)
                    .await
                    .expect("concurrent key should be stored");
                if index % 5 == 0 {
                    tokio::task::yield_now().await;
                }
            }
        }
    });

    let mut handles = Vec::with_capacity(SNAPSHOT_REQUESTS);
    for _ in 0..SNAPSHOT_REQUESTS {
        let state = state.clone();
        let start_barrier = Arc::clone(&start_barrier);
        handles.push(tokio::spawn(async move {
            start_barrier.wait().await;
            for _attempt in 0..20 {
                let response = local_admin_pool_response(
                    &state,
                    http::Method::GET,
                    "/api/admin/pool/provider-openai/keys?page=1&page_size=7&search=alpha%20concurrent&status=all",
                    None,
                )
                .await;
                assert_eq!(response.status(), StatusCode::OK);
                let payload = response_json(response).await;
                let total = payload["total"]
                    .as_u64()
                    .expect("total should be numeric") as usize;
                let snapshot_payload = create_pool_selection_snapshot_payload(
                    &state,
                    "provider-openai",
                    json!({
                        "page": 1,
                        "page_size": 7,
                        "search": "alpha concurrent",
                        "status": "all",
                        "expected_total": total,
                        "expected_page_key_ids": list_payload_key_ids(&payload),
                    }),
                )
                .await;
                if let Some(snapshot) = snapshot_payload.get("selection_snapshot") {
                    let snapshot_total = snapshot["total"]
                        .as_u64()
                        .expect("snapshot total should be numeric")
                        as usize;
                    assert!(
                        (total..=INITIAL_KEYS + ADDED_KEYS).contains(&snapshot_total),
                        "snapshot total {snapshot_total} should include the listed {total} items and stay within growth bounds"
                    );
                    let snapshot_id = snapshot["id"]
                        .as_str()
                        .expect("snapshot id should be present")
                        .to_string();
                    return (snapshot_id, snapshot_total);
                }
                tokio::task::yield_now().await;
            }
            panic!("selection snapshot should eventually stabilize")
        }));
    }

    let mut snapshots = Vec::with_capacity(SNAPSHOT_REQUESTS);
    for handle in handles {
        snapshots.push(handle.await.expect("snapshot request should join"));
    }
    writer.await.expect("concurrent writer should join");

    let mut retained_snapshots = 0usize;
    for (snapshot_id, total) in snapshots {
        assert!(
            (INITIAL_KEYS..=INITIAL_KEYS + ADDED_KEYS).contains(&total),
            "snapshot {snapshot_id} total {total} should stay within concurrent growth bounds"
        );
        let Some(snapshot) = read_runtime_pool_selection_snapshot(&state, &snapshot_id).await
        else {
            continue;
        };
        retained_snapshots = retained_snapshots.saturating_add(1);
        assert_eq!(snapshot.items.len(), total);
        let unique_ids = snapshot
            .items
            .iter()
            .map(|item| item.key_id.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(unique_ids.len(), total);
    }
    assert!(
        (1..=ADMIN_POOL_SELECTION_SNAPSHOT_MAX_ACTIVE_PER_ADMIN_PROVIDER)
            .contains(&retained_snapshots),
        "runtime should retain a bounded number of recent snapshots"
    );

    let final_response = local_admin_pool_response(
        &state,
        http::Method::GET,
        "/api/admin/pool/provider-openai/keys?page=1&page_size=10&search=alpha%20concurrent&status=all",
        None,
    )
    .await;
    assert_eq!(final_response.status(), StatusCode::OK);
    let final_payload = response_json(final_response).await;
    assert_eq!(final_payload["total"], json!(INITIAL_KEYS + ADDED_KEYS));
    let final_snapshot_payload = create_pool_selection_snapshot_payload(
        &state,
        "provider-openai",
        json!({
            "page": 1,
            "page_size": 10,
            "search": "alpha concurrent",
            "status": "all",
            "expected_total": final_payload["total"],
            "expected_page_key_ids": list_payload_key_ids(&final_payload),
        }),
    )
    .await;
    assert_eq!(
        final_snapshot_payload["selection_snapshot"]["total"],
        json!(INITIAL_KEYS + ADDED_KEYS)
    );
}

#[tokio::test]
async fn gateway_pool_selection_snapshot_batch_action_uses_frozen_membership_after_churn() {
    const ORIGINAL_KEYS: usize = 10;

    let provider = sample_provider("provider-openai", "openai", 10);
    let mut keys = Vec::with_capacity(ORIGINAL_KEYS);
    let mut original_ids = Vec::with_capacity(ORIGINAL_KEYS);
    for index in 0..ORIGINAL_KEYS {
        let key_id = format!("key-openai-freeze-{index:02}");
        let mut key = sample_key(
            &key_id,
            "provider-openai",
            "openai:chat",
            &format!("sk-freeze-{index:02}"),
        );
        key.name = format!("alpha frozen member {index:02}");
        original_ids.push(key_id);
        keys.push(key);
    }

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        keys,
    ));
    let state = pool_test_state(Arc::clone(&provider_catalog_repository));

    let list_response = local_admin_pool_response(
        &state,
        http::Method::GET,
        "/api/admin/pool/provider-openai/keys?page=1&page_size=5&search=alpha%20frozen&status=all",
        None,
    )
    .await;
    assert_eq!(list_response.status(), StatusCode::OK);
    let list_payload = response_json(list_response).await;
    assert_eq!(list_payload["total"], json!(ORIGINAL_KEYS));
    let snapshot_payload = create_pool_selection_snapshot_payload(
        &state,
        "provider-openai",
        json!({
            "page": 1,
            "page_size": 5,
            "search": "alpha frozen",
            "status": "all",
            "expected_total": list_payload["total"],
            "expected_page_key_ids": list_payload_key_ids(&list_payload),
        }),
    )
    .await;
    assert_eq!(
        snapshot_payload["selection_snapshot"]["total"],
        json!(ORIGINAL_KEYS)
    );
    let snapshot_id = snapshot_payload["selection_snapshot"]["id"]
        .as_str()
        .expect("snapshot id should be present")
        .to_string();

    for index in 0..3 {
        let key_id = format!("key-openai-new-match-{index:02}");
        let mut key = sample_key(
            &key_id,
            "provider-openai",
            "openai:chat",
            &format!("sk-new-match-{index:02}"),
        );
        key.name = format!("alpha frozen newly imported {index:02}");
        provider_catalog_repository
            .create_key(&key)
            .await
            .expect("new matching key should be stored");
    }
    provider_catalog_repository
        .delete_key(&original_ids[0])
        .await
        .expect("first original should delete");
    provider_catalog_repository
        .delete_key(&original_ids[1])
        .await
        .expect("second original should delete");

    let mut renamed_original = provider_catalog_repository
        .list_keys_by_ids(&[original_ids[2].clone()])
        .await
        .expect("renamed original should list")
        .into_iter()
        .next()
        .expect("renamed original should exist");
    renamed_original.name = "beta renamed after selection".to_string();
    provider_catalog_repository
        .update_key(&renamed_original)
        .await
        .expect("renamed original should update");

    let live_response = local_admin_pool_response(
        &state,
        http::Method::GET,
        "/api/admin/pool/provider-openai/keys?page=1&page_size=20&search=alpha%20frozen&status=all",
        None,
    )
    .await;
    assert_eq!(live_response.status(), StatusCode::OK);
    let live_payload = response_json(live_response).await;
    assert_eq!(live_payload["total"], json!(ORIGINAL_KEYS));

    let action_response = local_admin_pool_response(
        &state,
        http::Method::POST,
        "/api/admin/pool/provider-openai/keys/batch-action",
        Some(json!({
            "selection": {
                "type": "snapshot",
                "snapshot_id": snapshot_id,
                "expected_total": ORIGINAL_KEYS
            },
            "action": "disable"
        })),
    )
    .await;
    assert_eq!(action_response.status(), StatusCode::OK);
    let action_payload = response_json(action_response).await;
    assert_eq!(action_payload["affected"], json!(ORIGINAL_KEYS - 2));

    let mut ids_to_check = original_ids[2..].to_vec();
    ids_to_check.extend((0..3).map(|index| format!("key-openai-new-match-{index:02}")));
    let stored_keys = provider_catalog_repository
        .list_keys_by_ids(&ids_to_check)
        .await
        .expect("post-action keys should list")
        .into_iter()
        .map(|key| (key.id, key.is_active))
        .collect::<std::collections::BTreeMap<_, _>>();

    for key_id in &original_ids[2..] {
        assert_eq!(stored_keys.get(key_id), Some(&false));
    }
    for index in 0..3 {
        assert_eq!(
            stored_keys.get(&format!("key-openai-new-match-{index:02}")),
            Some(&true)
        );
    }
}

#[tokio::test]
async fn gateway_pool_resolve_selection_reports_missing_snapshot_members_consistently() {
    let provider = sample_provider("provider-openai", "openai", 10);
    let mut keys = Vec::new();
    for index in 0..3 {
        let key_id = format!("key-openai-resolve-{index}");
        let mut key = sample_key(
            &key_id,
            "provider-openai",
            "openai:chat",
            &format!("sk-resolve-{index}"),
        );
        key.name = format!("alpha resolve member {index}");
        keys.push(key);
    }

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        keys,
    ));
    let state = pool_test_state(Arc::clone(&provider_catalog_repository));

    let list_response = local_admin_pool_response(
        &state,
        http::Method::GET,
        "/api/admin/pool/provider-openai/keys?page=1&page_size=2&search=alpha%20resolve&status=all",
        None,
    )
    .await;
    assert_eq!(list_response.status(), StatusCode::OK);
    let list_payload = response_json(list_response).await;
    assert_eq!(list_payload["total"], json!(3));
    let snapshot_payload = create_pool_selection_snapshot_payload(
        &state,
        "provider-openai",
        json!({
            "page": 1,
            "page_size": 2,
            "search": "alpha resolve",
            "status": "all",
            "expected_total": list_payload["total"],
            "expected_page_key_ids": list_payload_key_ids(&list_payload),
        }),
    )
    .await;
    assert_eq!(snapshot_payload["selection_snapshot"]["total"], json!(3));
    let snapshot_id = snapshot_payload["selection_snapshot"]["id"]
        .as_str()
        .expect("snapshot id should be present")
        .to_string();

    provider_catalog_repository
        .delete_key("key-openai-resolve-1")
        .await
        .expect("snapshot member should delete");

    let resolve_response = local_admin_pool_response(
        &state,
        http::Method::POST,
        "/api/admin/pool/provider-openai/keys/resolve-selection",
        Some(json!({
            "snapshot_id": snapshot_id,
            "expected_total": 3
        })),
    )
    .await;
    assert_eq!(resolve_response.status(), StatusCode::OK);
    let resolve_payload = response_json(resolve_response).await;
    assert_eq!(resolve_payload["total"], json!(2));
    assert_eq!(resolve_payload["snapshot_total"], json!(3));
    assert_eq!(resolve_payload["missing_count"], json!(1));
    assert_eq!(resolve_payload["items"].as_array().map(Vec::len), Some(2));
}

#[tokio::test]
async fn gateway_pool_selection_snapshot_rejects_stale_or_invalid_references() {
    fn snapshot(
        id: &str,
        provider_id: &str,
        created_by: Option<&str>,
        status: &str,
        expires_at_unix_secs: u64,
        items: Vec<AdminPoolSelectionSnapshotItem>,
    ) -> AdminPoolSelectionSnapshot {
        AdminPoolSelectionSnapshot {
            id: id.to_string(),
            provider_id: provider_id.to_string(),
            created_by: created_by.map(ToOwned::to_owned),
            filter_json: None,
            total: items.len(),
            status: status.to_string(),
            created_at_unix_secs: 100,
            expires_at_unix_secs,
            items,
        }
    }

    fn item(ordinal: u64, key_id: &str) -> AdminPoolSelectionSnapshotItem {
        AdminPoolSelectionSnapshotItem {
            ordinal,
            key_id: key_id.to_string(),
            key_updated_at_unix_secs: None,
        }
    }

    let provider = sample_provider("provider-openai", "openai", 10);
    let other_provider = sample_provider("provider-other", "openai", 20);
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider, other_provider],
        Vec::new(),
        vec![
            sample_key("key-openai-a", "provider-openai", "openai:chat", "sk-a"),
            sample_key("key-openai-b", "provider-openai", "openai:chat", "sk-b"),
        ],
    ));
    let state = pool_test_state(Arc::clone(&provider_catalog_repository));
    for snapshot in [
        snapshot(
            "snapshot-ready",
            "provider-openai",
            Some("admin-user-123"),
            "ready",
            4_000_000_000,
            vec![item(0, "key-openai-a")],
        ),
        snapshot(
            "snapshot-other-provider",
            "provider-other",
            Some("admin-user-123"),
            "ready",
            4_000_000_000,
            vec![item(0, "key-openai-a")],
        ),
        snapshot(
            "snapshot-expired",
            "provider-openai",
            Some("admin-user-123"),
            "expired",
            4_000_000_000,
            vec![item(0, "key-openai-a")],
        ),
        snapshot(
            "snapshot-other-admin",
            "provider-openai",
            Some("admin-user-456"),
            "ready",
            4_000_000_000,
            vec![item(0, "key-openai-a")],
        ),
        snapshot(
            "snapshot-duplicate",
            "provider-openai",
            Some("admin-user-123"),
            "ready",
            4_000_000_000,
            vec![item(0, "key-openai-a"), item(1, "key-openai-a")],
        ),
    ] {
        write_runtime_pool_selection_snapshot(&state, snapshot).await;
    }
    let mut incomplete_snapshot = snapshot(
        "snapshot-incomplete",
        "provider-openai",
        Some("admin-user-123"),
        "ready",
        4_000_000_000,
        vec![item(0, "key-openai-a")],
    );
    incomplete_snapshot.total = 2;
    write_runtime_pool_selection_snapshot(&state, incomplete_snapshot).await;

    let selection_with_key_ids_response = local_admin_pool_response(
        &state,
        http::Method::POST,
        "/api/admin/pool/provider-openai/keys/batch-action",
        Some(json!({
            "key_ids": ["key-openai-b"],
            "selection": {
                "type": "snapshot",
                "snapshot_id": "snapshot-ready",
                "expected_total": 1
            },
            "action": "disable"
        })),
    )
    .await;
    assert_eq!(
        selection_with_key_ids_response.status(),
        StatusCode::BAD_REQUEST
    );

    for (snapshot_id, expected_total, expected_status) in [
        ("snapshot-missing", 1usize, StatusCode::NOT_FOUND),
        ("snapshot-other-provider", 1usize, StatusCode::CONFLICT),
        ("snapshot-expired", 1usize, StatusCode::CONFLICT),
        ("snapshot-other-admin", 1usize, StatusCode::FORBIDDEN),
        ("snapshot-duplicate", 2usize, StatusCode::CONFLICT),
        ("snapshot-incomplete", 2usize, StatusCode::CONFLICT),
    ] {
        let response = local_admin_pool_response(
            &state,
            http::Method::POST,
            "/api/admin/pool/provider-openai/keys/batch-action",
            Some(json!({
                "selection": {
                    "type": "snapshot",
                    "snapshot_id": snapshot_id,
                    "expected_total": expected_total
                },
                "action": "disable"
            })),
        )
        .await;
        assert_eq!(
            response.status(),
            expected_status,
            "{snapshot_id} should be rejected with {expected_status}"
        );
    }
}

#[tokio::test]
async fn gateway_sorts_admin_pool_keys_by_imported_and_last_used_time() {
    let provider = sample_provider("provider-openai", "openai", 10).with_transport_fields(
        true,
        false,
        true,
        None,
        None,
        None,
        None,
        None,
        Some(json!({
            "pool_advanced": {
                "enabled": true
            }
        })),
    );
    let mut old_key = sample_key("key-openai-old", "provider-openai", "openai:chat", "sk-old");
    old_key.name = "old".to_string();
    old_key.created_at_unix_ms = Some(1_711_000_000);
    old_key.last_used_at_unix_secs = Some(1_711_000_500);
    let mut fresh_key = sample_key(
        "key-openai-fresh",
        "provider-openai",
        "openai:chat",
        "sk-fresh",
    );
    fresh_key.name = "fresh".to_string();
    fresh_key.created_at_unix_ms = Some(1_711_002_000);
    fresh_key.last_used_at_unix_secs = Some(1_711_000_100);
    let mut active_key = sample_key(
        "key-openai-active",
        "provider-openai",
        "openai:chat",
        "sk-active",
    );
    active_key.name = "active".to_string();
    active_key.created_at_unix_ms = Some(1_711_001_000);
    active_key.last_used_at_unix_secs = Some(1_711_003_000);

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        vec![old_key, fresh_key, active_key],
    ));
    let pool_score_repository = Arc::new(InMemoryPoolMemberScoreRepository::seed(vec![
        sample_pool_member_score("provider-openai", "key-openai-fresh", 0.35),
        sample_pool_member_score("provider-openai", "key-openai-active", 0.92),
    ]));
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(
            GatewayDataState::with_provider_catalog_reader_for_tests(provider_catalog_repository)
                .with_pool_score_repository_for_tests(pool_score_repository),
        );

    let default_response = local_admin_pool_response(
        &state,
        http::Method::GET,
        "/api/admin/pool/provider-openai/keys?page=1&page_size=50&status=all",
        None,
    )
    .await;
    assert_eq!(default_response.status(), StatusCode::OK);
    let default_payload: serde_json::Value = serde_json::from_slice(
        &to_bytes(default_response.into_body(), usize::MAX)
            .await
            .expect("body should read"),
    )
    .expect("json body should parse");
    let default_names = default_payload["keys"]
        .as_array()
        .expect("keys should be array")
        .iter()
        .map(|item| item["key_name"].as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    assert_eq!(default_names, vec!["fresh", "active", "old"]);

    let imported_response = local_admin_pool_response(
        &state,
        http::Method::GET,
        "/api/admin/pool/provider-openai/keys?page=1&page_size=50&status=all&sort_by=imported_at&sort_order=desc",
        None,
    )
    .await;
    assert_eq!(imported_response.status(), StatusCode::OK);
    let imported_payload: serde_json::Value = serde_json::from_slice(
        &to_bytes(imported_response.into_body(), usize::MAX)
            .await
            .expect("body should read"),
    )
    .expect("json body should parse");
    let imported_names = imported_payload["keys"]
        .as_array()
        .expect("keys should be array")
        .iter()
        .map(|item| item["key_name"].as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    assert_eq!(imported_names, vec!["fresh", "active", "old"]);

    let last_used_response = local_admin_pool_response(
        &state,
        http::Method::GET,
        "/api/admin/pool/provider-openai/keys?page=1&page_size=50&status=all&sort_by=last_used_at&sort_order=desc",
        None,
    )
    .await;
    assert_eq!(last_used_response.status(), StatusCode::OK);
    let last_used_payload: serde_json::Value = serde_json::from_slice(
        &to_bytes(last_used_response.into_body(), usize::MAX)
            .await
            .expect("body should read"),
    )
    .expect("json body should parse");
    let last_used_names = last_used_payload["keys"]
        .as_array()
        .expect("keys should be array")
        .iter()
        .map(|item| item["key_name"].as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    assert_eq!(last_used_names, vec!["active", "old", "fresh"]);

    let score_response = local_admin_pool_response(
        &state,
        http::Method::GET,
        "/api/admin/pool/provider-openai/keys?page=1&page_size=50&status=all&sort_by=score&sort_order=desc",
        None,
    )
    .await;
    assert_eq!(score_response.status(), StatusCode::OK);
    let score_payload: serde_json::Value = serde_json::from_slice(
        &to_bytes(score_response.into_body(), usize::MAX)
            .await
            .expect("body should read"),
    )
    .expect("json body should parse");
    let score_names = score_payload["keys"]
        .as_array()
        .expect("keys should be array")
        .iter()
        .map(|item| item["key_name"].as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    assert_eq!(score_names, vec!["active", "fresh", "old"]);
    assert_eq!(score_payload["keys"][0]["pool_score"]["score"], json!(0.92));
    assert!(score_payload["keys"][2]["pool_score"].is_null());
}

#[tokio::test]
async fn gateway_pool_list_reads_materialized_codex_cycle_usage_from_quota_windows() {
    const RESET_AT: u64 = 4_102_444_800;

    let mut provider = sample_provider("provider-codex", "codex", 10).with_transport_fields(
        true,
        false,
        true,
        None,
        None,
        None,
        None,
        None,
        Some(json!({
            "pool_advanced": {
                "enabled": true
            }
        })),
    );
    provider.provider_type = "codex".to_string();

    let mut usage_key = sample_key(
        "key-codex-cycle",
        "provider-codex",
        "openai:responses",
        "oauth-placeholder",
    );
    usage_key.name = "codex cycle usage".to_string();
    usage_key.auth_type = "oauth".to_string();
    usage_key.request_count = Some(4);
    usage_key.total_tokens = 999;
    usage_key.total_cost_usd = 9.99;
    usage_key.status_snapshot = Some(json!({
    "quota": {
        "version": 2,
        "provider_type": "codex",
        "code": "ok",
        "label": serde_json::Value::Null,
        "reason": serde_json::Value::Null,
        "freshness": "fresh",
        "source": "response_headers",
        "observed_at": RESET_AT,
        "exhausted": false,
        "usage_ratio": 0.0,
        "updated_at": RESET_AT,
        "reset_seconds": serde_json::Value::Null,
        "plan_type": "plus",
        "windows": [
                {
                    "code": "weekly",
                    "label": "周",
                    "scope": "account",
                    "unit": "percent",
                    "used_ratio": 0.0,
                    "remaining_ratio": 1.0,
                    "reset_at": RESET_AT,
                    "reset_seconds": 604_800,
                    "window_minutes": 10_080,
                    "usage": {
                        "request_count": 3,
                        "total_tokens": 375,
                        "total_cost_usd": "0.60000000"
                    }
                },
                {
                    "code": "5h",
                    "label": "5H",
                    "scope": "account",
                    "unit": "percent",
                    "used_ratio": 0.0,
                    "remaining_ratio": 1.0,
                    "reset_at": RESET_AT,
                    "reset_seconds": 18_000,
                    "window_minutes": 300,
                    "usage": {
                        "request_count": 2,
                        "total_tokens": 225,
                        "total_cost_usd": "0.30000000"
                    }
                }
            ]
        }
    }));

    let mut zero_key = sample_key(
        "key-codex-zero",
        "provider-codex",
        "openai:responses",
        "oauth-placeholder",
    );
    zero_key.name = "codex zero usage".to_string();
    zero_key.auth_type = "oauth".to_string();
    zero_key.status_snapshot = Some(json!({
        "quota": {
            "version": 2,
            "provider_type": "codex",
            "code": "ok",
            "windows": [
                {
                    "code": "weekly",
                    "label": "周",
                    "reset_at": RESET_AT,
                    "window_minutes": 10_080,
                    "usage": {
                        "request_count": 0,
                        "total_tokens": 0,
                        "total_cost_usd": "0.00000000"
                    }
                },
                {
                    "code": "5h",
                    "label": "5H",
                    "reset_at": RESET_AT,
                    "window_minutes": 300,
                    "usage": {
                        "request_count": 0,
                        "total_tokens": 0,
                        "total_cost_usd": "0.00000000"
                    }
                }
            ]
        }
    }));

    let mut invalid_key = sample_key(
        "key-codex-invalid",
        "provider-codex",
        "openai:responses",
        "oauth-placeholder",
    );
    invalid_key.name = "codex invalid window".to_string();
    invalid_key.auth_type = "oauth".to_string();
    invalid_key.status_snapshot = Some(json!({
        "quota": {
            "version": 2,
            "provider_type": "codex",
            "code": "ok",
            "windows": [
                {
                    "code": "weekly",
                    "label": "周",
                    "reset_at": serde_json::Value::Null,
                    "window_minutes": 10_080
                },
                {
                    "code": "5h",
                    "label": "5H",
                    "reset_at": RESET_AT,
                    "window_minutes": serde_json::Value::Null
                }
            ]
        }
    }));

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        vec![usage_key, zero_key, invalid_key],
    ));
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(GatewayDataState::with_provider_catalog_reader_for_tests(
            provider_catalog_repository,
        ));

    let response = local_admin_pool_response(
        &state,
        http::Method::GET,
        "/api/admin/pool/provider-codex/keys?page=1&page_size=50&status=all",
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should read"),
    )
    .expect("json body should parse");
    let keys = payload["keys"].as_array().expect("keys should be array");
    fn key_by_id<'a>(keys: &'a [serde_json::Value], key_id: &str) -> &'a serde_json::Value {
        keys.iter()
            .find(|key| key["key_id"] == json!(key_id))
            .expect("key payload should exist")
    }

    fn window_by_code<'a>(key_payload: &'a serde_json::Value, code: &str) -> &'a serde_json::Value {
        key_payload["status_snapshot"]["quota"]["windows"]
            .as_array()
            .expect("quota windows should be array")
            .iter()
            .find(|window| window["code"] == json!(code))
            .expect("quota window should exist")
    }

    let usage_key_payload = key_by_id(keys, "key-codex-cycle");
    let five_hour_window = window_by_code(usage_key_payload, "5h");
    let weekly_window = window_by_code(usage_key_payload, "weekly");
    assert_eq!(five_hour_window["usage"]["request_count"], json!(2));
    assert_eq!(five_hour_window["usage"]["total_tokens"], json!(225));
    assert_eq!(
        five_hour_window["usage"]["total_cost_usd"],
        json!("0.30000000")
    );
    assert_eq!(weekly_window["usage"]["request_count"], json!(3));
    assert_eq!(weekly_window["usage"]["total_tokens"], json!(375));
    assert_eq!(
        weekly_window["usage"]["total_cost_usd"],
        json!("0.60000000")
    );
    assert_eq!(usage_key_payload["request_count"], json!(4));
    assert_eq!(usage_key_payload["total_tokens"], json!(999));
    assert_eq!(usage_key_payload["total_cost_usd"], json!("9.99000000"));

    let zero_key_payload = key_by_id(keys, "key-codex-zero");
    assert_eq!(
        window_by_code(zero_key_payload, "5h")["usage"]["request_count"],
        json!(0)
    );
    assert_eq!(
        window_by_code(zero_key_payload, "weekly")["usage"]["total_tokens"],
        json!(0)
    );

    let invalid_key_payload = key_by_id(keys, "key-codex-invalid");
    assert!(window_by_code(invalid_key_payload, "5h")
        .get("usage")
        .is_none());
    assert!(window_by_code(invalid_key_payload, "weekly")
        .get("usage")
        .is_none());
}

#[tokio::test]
async fn gateway_pool_list_overrides_stale_codex_cycle_usage_from_usage_facts() {
    let now_unix_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_secs();
    let reset_at = now_unix_secs + 3_600;

    let mut provider = sample_provider("provider-codex", "codex", 10).with_transport_fields(
        true,
        false,
        true,
        None,
        None,
        None,
        None,
        None,
        Some(json!({
            "pool_advanced": {
                "enabled": true
            }
        })),
    );
    provider.provider_type = "codex".to_string();

    let mut key = sample_key(
        "key-codex-stale-cycle",
        "provider-codex",
        "openai:responses",
        "oauth-placeholder",
    );
    key.name = "codex stale cycle".to_string();
    key.auth_type = "oauth".to_string();
    key.status_snapshot = Some(json!({
        "quota": {
            "version": 2,
            "provider_type": "codex",
            "code": "ok",
            "updated_at": reset_at,
            "exhausted": false,
            "windows": [
                {
                    "code": "weekly",
                    "label": "周",
                    "reset_at": reset_at,
                    "window_minutes": 10_080,
                    "usage": {
                        "request_count": 1,
                        "total_tokens": 50,
                        "total_cost_usd": "0.05000000"
                    }
                },
                {
                    "code": "5h",
                    "label": "5H",
                    "reset_at": reset_at,
                    "window_minutes": 300,
                    "usage_reset_at": now_unix_secs.saturating_sub(500),
                    "usage": {
                        "request_count": 9,
                        "total_tokens": 700,
                        "total_cost_usd": "0.70000000"
                    }
                }
            ]
        }
    }));

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        vec![key],
    ));
    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![
        sample_provider_key_usage_row(
            "usage-weekly-only",
            "req-weekly-only",
            "provider-codex",
            "key-codex-stale-cycle",
            now_unix_secs.saturating_sub(400_000) as i64,
            1_000,
            1.25,
        ),
        sample_provider_key_usage_row(
            "usage-five-hour-before-manual-reset",
            "req-five-hour-before-manual-reset",
            "provider-codex",
            "key-codex-stale-cycle",
            now_unix_secs.saturating_sub(1_000) as i64,
            999,
            9.99,
        ),
        sample_provider_key_usage_row(
            "usage-five-hour",
            "req-five-hour",
            "provider-codex",
            "key-codex-stale-cycle",
            now_unix_secs.saturating_sub(100) as i64,
            200,
            0.75,
        ),
    ]));
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(
            GatewayDataState::with_provider_catalog_and_usage_reader_for_tests(
                provider_catalog_repository,
                usage_repository,
            ),
        );

    let response = local_admin_pool_response(
        &state,
        http::Method::GET,
        "/api/admin/pool/provider-codex/keys?page=1&page_size=50&status=all",
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should read"),
    )
    .expect("json body should parse");
    let key_payload = payload["keys"]
        .as_array()
        .expect("keys should be array")
        .first()
        .expect("key should exist");
    let windows = key_payload["status_snapshot"]["quota"]["windows"]
        .as_array()
        .expect("quota windows should exist");
    let weekly = windows
        .iter()
        .find(|window| window["code"] == json!("weekly"))
        .expect("weekly window should exist");
    let five_hour = windows
        .iter()
        .find(|window| window["code"] == json!("5h"))
        .expect("5h window should exist");

    assert_eq!(weekly["usage"]["request_count"], json!(3));
    assert_eq!(weekly["usage"]["total_tokens"], json!(2_199));
    assert_eq!(weekly["usage"]["total_cost_usd"], json!("11.99000000"));
    assert_eq!(five_hour["usage"]["request_count"], json!(1));
    assert_eq!(five_hour["usage"]["total_tokens"], json!(200));
    assert_eq!(five_hour["usage"]["total_cost_usd"], json!("0.75000000"));
}

#[tokio::test]
async fn gateway_marks_account_blocked_pool_key_in_list_keys_response() {
    let mut provider = sample_provider("provider-codex", "codex", 10).with_transport_fields(
        true,
        false,
        true,
        None,
        None,
        None,
        None,
        None,
        Some(json!({
            "pool_advanced": {
                "enabled": true
            }
        })),
    );
    provider.provider_type = "codex".to_string();

    let mut key = sample_key(
        "key-codex-blocked",
        "provider-codex",
        "openai:responses",
        "oauth-placeholder",
    );
    key.name = "blocked-codex".to_string();
    key.auth_type = "oauth".to_string();
    key.oauth_invalid_reason = Some("[ACCOUNT_BLOCK] account has been deactivated".to_string());
    key.oauth_invalid_at_unix_secs = Some(1_700_000_000);

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        vec![key],
    ));
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(GatewayDataState::with_provider_catalog_reader_for_tests(
            provider_catalog_repository,
        ));

    let response = local_admin_pool_response(
        &state,
        http::Method::GET,
        "/api/admin/pool/provider-codex/keys?page=1&page_size=50&status=all",
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should read"),
    )
    .expect("json body should parse");
    let keys = payload["keys"].as_array().expect("keys should be array");

    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0]["account_status_code"], json!("account_disabled"));
    assert_eq!(keys[0]["account_status_blocked"], json!(true));
    assert_eq!(keys[0]["scheduling_status"], json!("blocked"));
    assert_eq!(keys[0]["scheduling_reason"], json!("account_blocked"));
    assert_eq!(keys[0]["scheduling_label"], json!("账号停用"));
    assert_eq!(
        keys[0]["status_snapshot"]["account"]["source"],
        json!("oauth_invalid")
    );
}

#[tokio::test]
async fn gateway_ignores_health_signals_in_pool_scheduling_status() {
    let mut provider = sample_provider("provider-openai", "openai", 10).with_transport_fields(
        true,
        false,
        true,
        None,
        None,
        None,
        None,
        None,
        Some(json!({
            "pool_advanced": {
                "enabled": true
            }
        })),
    );
    provider.provider_type = "openai".to_string();

    let mut circuit_key = sample_key(
        "key-openai-circuit",
        "provider-openai",
        "openai:chat",
        "sk-circuit",
    );
    circuit_key.name = "circuit-open".to_string();
    circuit_key.circuit_breaker_by_format = Some(json!({
        "openai:chat": {
            "open": true
        }
    }));

    let mut low_health_key = sample_key(
        "key-openai-health",
        "provider-openai",
        "openai:chat",
        "sk-health",
    );
    low_health_key.name = "low-health".to_string();
    low_health_key.health_by_format = Some(json!({
        "openai:chat": {
            "health_score": 0.2
        }
    }));

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        vec![circuit_key, low_health_key],
    ));
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(GatewayDataState::with_provider_catalog_reader_for_tests(
            provider_catalog_repository,
        ));

    let response = local_admin_pool_response(
        &state,
        http::Method::GET,
        "/api/admin/pool/provider-openai/keys?page=1&page_size=50&status=all",
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should read"),
    )
    .expect("json body should parse");
    let keys = payload["keys"].as_array().expect("keys should be array");

    assert_eq!(keys.len(), 2);
    assert_eq!(keys[0]["key_name"], json!("circuit-open"));
    assert_eq!(keys[0]["circuit_breaker_open"], json!(false));
    assert_eq!(keys[0]["scheduling_status"], json!("available"));
    assert_eq!(keys[0]["scheduling_reason"], json!("available"));
    assert_eq!(keys[0]["scheduling_label"], json!("可用"));

    assert_eq!(keys[1]["key_name"], json!("low-health"));
    assert_eq!(keys[1]["health_score"], json!(0.2));
    assert_eq!(keys[1]["scheduling_status"], json!("available"));
    assert_eq!(keys[1]["scheduling_reason"], json!("available"));
    assert_eq!(keys[1]["scheduling_label"], json!("可用"));
}

#[tokio::test]
async fn gateway_handles_admin_pool_list_keys_with_quota_compatibility_fields() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/pool/provider-antigravity/keys",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let mut provider = sample_provider("provider-antigravity", "antigravity", 10)
        .with_transport_fields(
            true,
            false,
            true,
            None,
            None,
            None,
            None,
            None,
            Some(json!({
                "pool_advanced": {
                    "enabled": true
                }
            })),
        );
    provider.provider_type = "antigravity".to_string();

    let mut key = sample_key(
        "key-antigravity-a",
        "provider-antigravity",
        "gemini:generate_content",
        "sk-antigravity",
    );
    key.name = "quota-key".to_string();
    key.auth_type = "oauth".to_string();
    key.expires_at_unix_secs = Some(1_775_556_730);
    key.encrypted_auth_config = Some(
        encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            r#"{"plan_type":"pro","account_id":"acct-antigravity-1","account_name":"quota-user","account_user_id":"quota-user-1","organizations":[{"id":"org-1","name":"Org One"}]}"#,
        )
        .expect("auth config ciphertext should build"),
    );
    key.status_snapshot = Some(json!({
        "oauth": {
            "code": "expired",
            "label": "已过期",
            "reason": "Access Token 已过期，等待自动续期",
            "expires_at": 1775556730u64,
            "invalid_at": null,
            "source": "expires_at",
            "requires_reauth": false,
            "expiring_soon": false
        },
        "account": {
            "code": "ok",
            "label": null,
            "reason": null,
            "blocked": false,
            "source": null,
            "recoverable": false
        },
        "quota": {
            "code": "ok",
            "label": null,
            "reason": null,
            "exhausted": false,
            "usage_ratio": 0.0,
            "updated_at": 1775553285u64,
            "reset_seconds": null,
            "plan_type": null
        }
    }));
    key.upstream_metadata = Some(json!({
        "antigravity": {
            "updated_at": 1775553285u64,
            "quota_by_model": {
                "gemini-2.5-flash": { "used_percent": 0.0 },
                "gemini-2.5-pro": { "used_percent": 0.0 }
            }
        }
    }));

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        vec![key],
    ));

    let (_upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(GatewayDataState::with_provider_catalog_reader_for_tests(
                provider_catalog_repository,
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/pool/provider-antigravity/keys?page=1&page_size=10&status=all"
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
    let keys = payload["keys"].as_array().expect("keys should be array");
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0]["account_quota"], json!("最低剩余 100.0% (2 模型)"));
    assert_eq!(keys[0]["quota_updated_at"], json!(1775553285u64));
    assert_eq!(keys[0]["oauth_expires_at"], json!(1775556730u64));
    assert_eq!(keys[0]["oauth_plan_type"], json!("pro"));
    assert_eq!(keys[0]["oauth_account_id"], json!("acct-antigravity-1"));
    assert_eq!(keys[0]["oauth_account_name"], json!("quota-user"));
    assert_eq!(keys[0]["oauth_account_user_id"], json!("quota-user-1"));
    assert_eq!(keys[0]["oauth_organizations"][0]["id"], json!("org-1"));
    assert_eq!(keys[0]["account_status_code"], json!("ok"));
    assert_eq!(keys[0]["account_status_blocked"], json!(false));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_includes_pool_quota_and_compat_fields_in_list_keys_response() {
    let mut provider = sample_provider("provider-antigravity", "antigravity", 10)
        .with_transport_fields(
            true,
            false,
            true,
            None,
            None,
            None,
            None,
            None,
            Some(json!({
                "pool_advanced": {
                    "enabled": true
                }
            })),
        );
    provider.provider_type = "antigravity".to_string();
    let mut key = sample_key(
        "key-antigravity-oauth",
        "provider-antigravity",
        "gemini:generate_content",
        "oauth-placeholder",
    );
    key.name = "quota key".to_string();
    key.auth_type = "oauth".to_string();
    key.expires_at_unix_secs = Some(1_775_556_730);
    key.encrypted_auth_config = Some(
        encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            &json!({
                "plan_type": "pro",
                "account_id": "acct-demo-001",
                "account_name": "Demo Account",
                "account_user_id": "user-demo-001",
                "organizations": [],
            })
            .to_string(),
        )
        .expect("auth config should encrypt"),
    );
    key.upstream_metadata = Some(json!({
        "antigravity": {
            "updated_at": 1_775_553_285u64,
            "quota_by_model": {
                "gemini-2.5-pro": { "used_percent": 0 },
                "gemini-2.5-flash": { "used_percent": 0 }
            }
        }
    }));
    key.status_snapshot = Some(json!({
        "oauth": {
            "code": "expired",
            "label": "已过期",
            "reason": "Access Token 已过期，等待自动续期",
            "expires_at": 1_775_556_730u64,
            "invalid_at": serde_json::Value::Null,
            "source": "expires_at",
            "requires_reauth": false,
            "expiring_soon": false
        },
        "account": {
            "code": "ok",
            "label": serde_json::Value::Null,
            "reason": serde_json::Value::Null,
            "blocked": false,
            "source": serde_json::Value::Null,
            "recoverable": false
        },
        "quota": {
            "code": "ok",
            "label": serde_json::Value::Null,
            "reason": serde_json::Value::Null,
            "exhausted": false,
            "usage_ratio": 0.0,
            "updated_at": 1_775_553_285u64,
            "reset_seconds": serde_json::Value::Null,
            "plan_type": serde_json::Value::Null
        }
    }));

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        vec![key],
    ));
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(
            GatewayDataState::with_provider_catalog_reader_for_tests(provider_catalog_repository)
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
        );

    let response = local_admin_pool_response(
        &state,
        http::Method::GET,
        "/api/admin/pool/provider-antigravity/keys?page=1&page_size=50&status=all",
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should read"),
    )
    .expect("json body should parse");
    let keys = payload["keys"].as_array().expect("keys should be array");
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0]["account_quota"], "最低剩余 100.0% (2 模型)");
    assert_eq!(keys[0]["quota_updated_at"], json!(1_775_553_285u64));
    assert_eq!(keys[0]["oauth_expires_at"], json!(1_775_556_730u64));
    assert_eq!(keys[0]["oauth_plan_type"], "pro");
    assert_eq!(keys[0]["oauth_account_id"], "acct-demo-001");
    assert_eq!(keys[0]["oauth_account_name"], "Demo Account");
    assert_eq!(keys[0]["oauth_account_user_id"], "user-demo-001");
    assert_eq!(keys[0]["oauth_organizations"], json!([]));
    assert_eq!(keys[0]["account_status_code"], "ok");
    assert_eq!(keys[0]["account_status_blocked"], json!(false));
}

#[tokio::test]
async fn gateway_prefers_status_snapshot_antigravity_quota_over_stale_metadata() {
    let mut provider = sample_provider("provider-antigravity", "antigravity", 10)
        .with_transport_fields(
            true,
            false,
            true,
            None,
            None,
            None,
            None,
            None,
            Some(json!({
                "pool_advanced": {
                    "enabled": true,
                    "skip_exhausted_accounts": true
                }
            })),
        );
    provider.provider_type = "antigravity".to_string();

    let mut key = sample_key(
        "key-antigravity-snapshot-fresh",
        "provider-antigravity",
        "gemini:generate_content",
        "oauth-placeholder",
    );
    key.name = "antigravity snapshot fresh".to_string();
    key.auth_type = "oauth".to_string();
    key.upstream_metadata = Some(json!({
        "antigravity": {
            "quota_by_model": {
                "gemini-2.5-pro": { "used_percent": 100.0 },
                "gemini-2.5-flash": { "used_percent": 100.0 }
            }
        }
    }));
    key.status_snapshot = Some(json!({
        "quota": {
            "version": 2,
            "provider_type": "antigravity",
            "code": "ok",
            "label": serde_json::Value::Null,
            "reason": serde_json::Value::Null,
            "freshness": "fresh",
            "source": "refresh_api",
            "observed_at": 1_775_553_285u64,
            "exhausted": false,
            "usage_ratio": 0.0,
            "updated_at": 1_775_553_285u64,
            "reset_seconds": serde_json::Value::Null,
            "plan_type": serde_json::Value::Null,
            "windows": [
                {
                    "code": "model:gemini-2.5-pro",
                    "label": "Gemini 2.5 Pro",
                    "scope": "model",
                    "unit": "percent",
                    "model": "gemini-2.5-pro",
                    "used_ratio": 0.0,
                    "remaining_ratio": 1.0,
                    "reset_at": serde_json::Value::Null,
                    "reset_seconds": serde_json::Value::Null,
                    "is_exhausted": false
                },
                {
                    "code": "model:gemini-2.5-flash",
                    "label": "Gemini 2.5 Flash",
                    "scope": "model",
                    "unit": "percent",
                    "model": "gemini-2.5-flash",
                    "used_ratio": 0.0,
                    "remaining_ratio": 1.0,
                    "reset_at": serde_json::Value::Null,
                    "reset_seconds": serde_json::Value::Null,
                    "is_exhausted": false
                }
            ]
        }
    }));

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        vec![key],
    ));
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(GatewayDataState::with_provider_catalog_reader_for_tests(
            provider_catalog_repository,
        ));

    let response = local_admin_pool_response(
        &state,
        http::Method::GET,
        "/api/admin/pool/provider-antigravity/keys?page=1&page_size=50&status=all",
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should read"),
    )
    .expect("json body should parse");
    let keys = payload["keys"].as_array().expect("keys should be array");

    assert_eq!(keys[0]["scheduling_status"], json!("available"));
    assert_eq!(keys[0]["scheduling_reason"], json!("available"));
    assert_eq!(keys[0]["quota_updated_at"], json!(1_775_553_285u64));
    assert_eq!(keys[0]["account_quota"], json!("最低剩余 100.0% (2 模型)"));
}

#[tokio::test]
async fn gateway_renders_gemini_cli_account_quota_from_status_snapshot() {
    let mut provider = sample_provider("provider-gemini-cli", "gemini_cli", 10)
        .with_transport_fields(
            true,
            false,
            true,
            None,
            None,
            None,
            None,
            None,
            Some(json!({
                "pool_advanced": {
                    "enabled": true,
                    "skip_exhausted_accounts": true
                }
            })),
        );
    provider.provider_type = "gemini_cli".to_string();

    let mut key = sample_key(
        "key-gemini-cli-snapshot",
        "provider-gemini-cli",
        "gemini:generate_content",
        "oauth-placeholder",
    );
    key.name = "gemini cli snapshot".to_string();
    key.auth_type = "oauth".to_string();
    key.upstream_metadata = Some(json!({
        "gemini_cli": {
            "quota_by_model": {
                "gemini-2.5-pro": {
                    "is_exhausted": false
                }
            }
        }
    }));
    key.status_snapshot = Some(json!({
        "quota": {
            "version": 2,
            "provider_type": "gemini_cli",
            "code": "cooldown",
            "label": "冷却中",
            "reason": serde_json::Value::Null,
            "freshness": "fresh",
            "source": "background_refresh",
            "observed_at": 1_775_553_285u64,
            "exhausted": false,
            "usage_ratio": 1.0,
            "updated_at": 1_775_553_285u64,
            "reset_seconds": serde_json::Value::Null,
            "plan_type": serde_json::Value::Null,
            "windows": [
                {
                    "code": "model:gemini-2.5-pro",
                    "label": "Gemini 2.5 Pro",
                    "scope": "model",
                    "unit": "percent",
                    "model": "gemini-2.5-pro",
                    "used_ratio": 1.0,
                    "remaining_ratio": 0.0,
                    "reset_at": serde_json::Value::Null,
                    "reset_seconds": serde_json::Value::Null,
                    "is_exhausted": true
                }
            ]
        }
    }));

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        vec![key],
    ));
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(GatewayDataState::with_provider_catalog_reader_for_tests(
            provider_catalog_repository,
        ));

    let response = local_admin_pool_response(
        &state,
        http::Method::GET,
        "/api/admin/pool/provider-gemini-cli/keys?page=1&page_size=50&status=all",
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should read"),
    )
    .expect("json body should parse");
    let keys = payload["keys"].as_array().expect("keys should be array");

    assert_eq!(keys[0]["scheduling_status"], json!("available"));
    assert_eq!(keys[0]["quota_updated_at"], json!(1_775_553_285u64));
    assert_eq!(keys[0]["account_quota"], json!("Gemini 2.5 Pro 冷却中"));
}

#[tokio::test]
async fn gateway_prefers_gemini_cli_account_credits_over_model_quota_text() {
    let mut provider = sample_provider("provider-gemini-cli", "gemini_cli", 10)
        .with_transport_fields(
            true,
            false,
            true,
            None,
            None,
            None,
            None,
            None,
            Some(json!({
                "pool_advanced": {
                    "enabled": true,
                    "skip_exhausted_accounts": true
                }
            })),
        );
    provider.provider_type = "gemini_cli".to_string();

    let mut key = sample_key(
        "key-gemini-cli-credits",
        "provider-gemini-cli",
        "gemini:generate_content",
        "oauth-placeholder",
    );
    key.name = "gemini cli credits".to_string();
    key.auth_type = "oauth".to_string();
    key.status_snapshot = Some(json!({
        "quota": {
            "version": 2,
            "provider_type": "gemini_cli",
            "code": "ok",
            "freshness": "fresh",
            "source": "report_effect",
            "observed_at": 1_775_553_285u64,
            "exhausted": false,
            "usage_ratio": 0.25,
            "updated_at": 1_775_553_285u64,
            "plan_type": "g1-pro-tier",
            "credits": {
                "remaining": 123.5,
                "consumed": 7.0,
                "has_credits": true
            },
            "windows": [
                {
                    "code": "model:gemini-2.5-pro",
                    "label": "Gemini 2.5 Pro",
                    "scope": "model",
                    "unit": "percent",
                    "model": "gemini-2.5-pro",
                    "used_ratio": 0.25,
                    "remaining_ratio": 0.75,
                    "is_exhausted": false
                }
            ]
        }
    }));

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        vec![key],
    ));
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(GatewayDataState::with_provider_catalog_reader_for_tests(
            provider_catalog_repository,
        ));

    let response = local_admin_pool_response(
        &state,
        http::Method::GET,
        "/api/admin/pool/provider-gemini-cli/keys?page=1&page_size=50&status=all",
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should read"),
    )
    .expect("json body should parse");
    let keys = payload["keys"].as_array().expect("keys should be array");

    assert_eq!(keys[0]["scheduling_status"], json!("available"));
    assert_eq!(keys[0]["account_quota"], json!("AI Credits 剩余 123.5"));
}

#[tokio::test]
async fn gateway_formats_codex_quota_countdown_from_reset_after_seconds() {
    let mut provider = sample_provider("provider-codex", "codex", 10).with_transport_fields(
        true,
        false,
        true,
        None,
        None,
        None,
        None,
        None,
        Some(json!({
            "pool_advanced": {
                "enabled": true
            }
        })),
    );
    provider.provider_type = "codex".to_string();

    let mut key = sample_key(
        "key-codex-oauth",
        "provider-codex",
        "openai:responses",
        "oauth-placeholder",
    );
    key.name = "codex quota key".to_string();
    key.auth_type = "oauth".to_string();
    key.upstream_metadata = Some(json!({
        "codex": {
            "plan_type": "plus",
            "updated_at": 4_102_444_800u64,
            "primary_used_percent": 10.0,
            "primary_reset_after_seconds": 266_400,
            "secondary_used_percent": 33.0,
            "secondary_reset_after_seconds": 13_800
        }
    }));

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        vec![key],
    ));
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(GatewayDataState::with_provider_catalog_reader_for_tests(
            provider_catalog_repository,
        ));

    let response = local_admin_pool_response(
        &state,
        http::Method::GET,
        "/api/admin/pool/provider-codex/keys?page=1&page_size=50&status=all",
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should read"),
    )
    .expect("json body should parse");
    let keys = payload["keys"].as_array().expect("keys should be array");
    assert_eq!(keys.len(), 1);
    assert_eq!(
        keys[0]["account_quota"],
        "周剩余 90.0% (3天2小时后重置) | 5H剩余 67.0% (3小时50分钟后重置)"
    );
}

#[tokio::test]
async fn gateway_marks_exhausted_codex_pool_key_as_blocked_when_flag_enabled() {
    let mut provider = sample_provider("provider-codex", "codex", 10).with_transport_fields(
        true,
        false,
        true,
        None,
        None,
        None,
        None,
        None,
        Some(json!({
            "pool_advanced": {
                "enabled": true,
                "skip_exhausted_accounts": true
            }
        })),
    );
    provider.provider_type = "codex".to_string();

    let mut key = sample_key(
        "key-codex-exhausted",
        "provider-codex",
        "openai:responses",
        "oauth-placeholder",
    );
    key.name = "codex exhausted".to_string();
    key.auth_type = "oauth".to_string();
    key.upstream_metadata = Some(json!({
        "codex": {
            "secondary_used_percent": 100.0,
            "plan_type": "plus"
        }
    }));

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        vec![key],
    ));
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(GatewayDataState::with_provider_catalog_reader_for_tests(
            provider_catalog_repository,
        ));

    let response = local_admin_pool_response(
        &state,
        http::Method::GET,
        "/api/admin/pool/provider-codex/keys?page=1&page_size=50&status=all",
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should read"),
    )
    .expect("json body should parse");
    let keys = payload["keys"].as_array().expect("keys should be array");

    assert_eq!(keys[0]["scheduling_status"], json!("blocked"));
    assert_eq!(
        keys[0]["scheduling_reason"],
        json!("account_quota_exhausted")
    );
    assert_eq!(keys[0]["scheduling_label"], json!("额度耗尽"));
    assert_eq!(
        keys[0]["scheduling_reasons"][0],
        json!({
            "code": "account_quota_exhausted",
            "label": "额度耗尽",
            "blocking": true,
            "source": "quota",
            "ttl_seconds": serde_json::Value::Null,
            "detail": serde_json::Value::Null,
        })
    );
    assert_eq!(keys[0]["account_quota"], json!("5H剩余 0.0%"));
}

#[tokio::test]
async fn gateway_lists_inherited_fixed_provider_api_formats_for_pool_keys() {
    let mut provider = sample_provider("provider-codex", "codex", 10).with_transport_fields(
        true,
        false,
        true,
        None,
        None,
        None,
        None,
        None,
        Some(json!({
            "pool_advanced": {
                "enabled": true
            }
        })),
    );
    provider.provider_type = "codex".to_string();

    let mut key = sample_key(
        "key-codex-inherited",
        "provider-codex",
        "openai:responses",
        "oauth-placeholder",
    );
    key.name = "codex inherited".to_string();
    key.auth_type = "oauth".to_string();
    key.api_formats = Some(json!(["openai:responses", "openai:responses:compact"]));

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![
            sample_endpoint(
                "endpoint-codex-cli",
                "provider-codex",
                "openai:responses",
                "https://chatgpt.com/backend-api/codex",
            ),
            sample_endpoint(
                "endpoint-codex-image",
                "provider-codex",
                "openai:image",
                "https://chatgpt.com/backend-api/codex",
            ),
        ],
        vec![key],
    ));
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(GatewayDataState::with_provider_catalog_reader_for_tests(
            provider_catalog_repository,
        ));

    let response = local_admin_pool_response(
        &state,
        http::Method::GET,
        "/api/admin/pool/provider-codex/keys?page=1&page_size=50&status=all",
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should read"),
    )
    .expect("json body should parse");
    let keys = payload["keys"].as_array().expect("keys should be array");

    assert_eq!(keys.len(), 1);
    assert_eq!(
        keys[0]["api_formats"],
        json!(["openai:image", "openai:responses"])
    );
}

#[tokio::test]
async fn gateway_prefers_status_snapshot_codex_quota_over_stale_metadata() {
    let mut provider = sample_provider("provider-codex", "codex", 10).with_transport_fields(
        true,
        false,
        true,
        None,
        None,
        None,
        None,
        None,
        Some(json!({
            "pool_advanced": {
                "enabled": true,
                "skip_exhausted_accounts": true
            }
        })),
    );
    provider.provider_type = "codex".to_string();

    let mut key = sample_key(
        "key-codex-snapshot-fresh",
        "provider-codex",
        "openai:responses",
        "oauth-placeholder",
    );
    key.name = "codex snapshot fresh".to_string();
    key.auth_type = "oauth".to_string();
    key.upstream_metadata = Some(json!({
        "codex": {
            "plan_type": "plus",
            "secondary_used_percent": 100.0
        }
    }));
    key.status_snapshot = Some(json!({
        "quota": {
            "version": 2,
            "provider_type": "codex",
            "code": "ok",
            "label": serde_json::Value::Null,
            "reason": serde_json::Value::Null,
            "freshness": "fresh",
            "source": "response_headers",
            "observed_at": 1_775_553_285u64,
            "exhausted": false,
            "usage_ratio": 0.0,
            "updated_at": 1_775_553_285u64,
            "reset_seconds": serde_json::Value::Null,
            "plan_type": "plus",
            "credits": {
                "has_credits": true,
                "balance": 12.5,
                "unlimited": false
            },
            "windows": [
                {
                    "code": "weekly",
                    "label": "周",
                    "scope": "account",
                    "unit": "percent",
                    "used_ratio": 0.0,
                    "remaining_ratio": 1.0,
                    "reset_at": serde_json::Value::Null,
                    "reset_seconds": serde_json::Value::Null,
                    "window_minutes": 10_080
                },
                {
                    "code": "5h",
                    "label": "5H",
                    "scope": "account",
                    "unit": "percent",
                    "used_ratio": 0.0,
                    "remaining_ratio": 1.0,
                    "reset_at": serde_json::Value::Null,
                    "reset_seconds": serde_json::Value::Null,
                    "window_minutes": 300
                }
            ]
        }
    }));

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        vec![key],
    ));
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(GatewayDataState::with_provider_catalog_reader_for_tests(
            provider_catalog_repository,
        ));

    let response = local_admin_pool_response(
        &state,
        http::Method::GET,
        "/api/admin/pool/provider-codex/keys?page=1&page_size=50&status=all",
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should read"),
    )
    .expect("json body should parse");
    let keys = payload["keys"].as_array().expect("keys should be array");

    assert_eq!(keys[0]["scheduling_status"], json!("available"));
    assert_eq!(keys[0]["scheduling_reason"], json!("available"));
    assert_eq!(keys[0]["quota_updated_at"], json!(1_775_553_285u64));
    assert_eq!(
        keys[0]["account_quota"],
        json!("周剩余 100.0% | 5H剩余 100.0%")
    );
}

#[tokio::test]
async fn gateway_treats_stale_codex_exhausted_snapshot_as_available_when_windows_have_capacity() {
    let mut provider = sample_provider("provider-codex", "codex", 10).with_transport_fields(
        true,
        false,
        true,
        None,
        None,
        None,
        None,
        None,
        Some(json!({
            "pool_advanced": {
                "enabled": true,
                "skip_exhausted_accounts": true
            }
        })),
    );
    provider.provider_type = "codex".to_string();

    let mut key = sample_key(
        "key-codex-no-credits",
        "provider-codex",
        "openai:responses",
        "oauth-placeholder",
    );
    key.name = "codex no credits".to_string();
    key.auth_type = "oauth".to_string();
    key.status_snapshot = Some(json!({
        "quota": {
            "version": 2,
            "provider_type": "codex",
            "code": "exhausted",
            "label": "额度耗尽",
            "reason": "无可用积分",
            "freshness": "fresh",
            "source": "refresh_api",
            "observed_at": 4_102_444_800u64,
            "exhausted": true,
            "usage_ratio": 0.0,
            "updated_at": 4_102_444_800u64,
            "reset_seconds": 18_000,
            "plan_type": "plus",
            "credits": {
                "has_credits": false,
                "balance": 0.0,
                "unlimited": false
            },
            "windows": [
                {
                    "code": "weekly",
                    "label": "周",
                    "scope": "account",
                    "unit": "percent",
                    "used_ratio": 0.0,
                    "remaining_ratio": 1.0,
                    "reset_at": 4_103_049_600u64,
                    "reset_seconds": 604_800,
                    "window_minutes": 10_080
                },
                {
                    "code": "5h",
                    "label": "5H",
                    "scope": "account",
                    "unit": "percent",
                    "used_ratio": 0.0,
                    "remaining_ratio": 1.0,
                    "reset_at": 4_102_462_800u64,
                    "reset_seconds": 18_000,
                    "window_minutes": 300
                }
            ]
        }
    }));

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        vec![key],
    ));
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(GatewayDataState::with_provider_catalog_reader_for_tests(
            provider_catalog_repository,
        ));

    let response = local_admin_pool_response(
        &state,
        http::Method::GET,
        "/api/admin/pool/provider-codex/keys?page=1&page_size=50&status=all",
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should read"),
    )
    .expect("json body should parse");
    let keys = payload["keys"].as_array().expect("keys should be array");

    assert_eq!(keys[0]["scheduling_status"], json!("available"));
    assert_eq!(keys[0]["scheduling_reason"], json!("available"));
    assert_eq!(
        keys[0]["account_quota"],
        json!("周剩余 100.0% (7天0小时后重置) | 5H剩余 100.0% (5小时0分钟后重置)")
    );
}

#[tokio::test]
async fn gateway_marks_exhausted_kiro_pool_key_as_blocked_when_flag_enabled() {
    let mut provider = sample_provider("provider-kiro", "kiro", 10).with_transport_fields(
        true,
        false,
        true,
        None,
        None,
        None,
        None,
        None,
        Some(json!({
            "pool_advanced": {
                "enabled": true,
                "skip_exhausted_accounts": true
            }
        })),
    );
    provider.provider_type = "kiro".to_string();

    let mut key = sample_key(
        "key-kiro-exhausted",
        "provider-kiro",
        "claude:messages",
        "oauth-placeholder",
    );
    key.name = "kiro exhausted".to_string();
    key.auth_type = "oauth".to_string();
    key.upstream_metadata = Some(json!({
        "kiro": {
            "remaining": 0.0,
            "usage_limit": 100.0,
            "current_usage": 100.0
        }
    }));

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        vec![key],
    ));
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(GatewayDataState::with_provider_catalog_reader_for_tests(
            provider_catalog_repository,
        ));

    let response = local_admin_pool_response(
        &state,
        http::Method::GET,
        "/api/admin/pool/provider-kiro/keys?page=1&page_size=50&status=all",
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should read"),
    )
    .expect("json body should parse");
    let keys = payload["keys"].as_array().expect("keys should be array");

    assert_eq!(keys[0]["scheduling_status"], json!("blocked"));
    assert_eq!(
        keys[0]["scheduling_reason"],
        json!("account_quota_exhausted")
    );
    assert_eq!(keys[0]["scheduling_label"], json!("额度耗尽"));
    assert_eq!(keys[0]["account_quota"], json!("剩余 0/100"));
}

#[tokio::test]
async fn gateway_prefers_status_snapshot_kiro_quota_over_stale_metadata() {
    let mut provider = sample_provider("provider-kiro", "kiro", 10).with_transport_fields(
        true,
        false,
        true,
        None,
        None,
        None,
        None,
        None,
        Some(json!({
            "pool_advanced": {
                "enabled": true,
                "skip_exhausted_accounts": true
            }
        })),
    );
    provider.provider_type = "kiro".to_string();

    let mut key = sample_key(
        "key-kiro-snapshot-fresh",
        "provider-kiro",
        "claude:messages",
        "oauth-placeholder",
    );
    key.name = "kiro snapshot fresh".to_string();
    key.auth_type = "oauth".to_string();
    key.upstream_metadata = Some(json!({
        "kiro": {
            "remaining": 0.0,
            "usage_limit": 100.0,
            "current_usage": 100.0
        }
    }));
    key.status_snapshot = Some(json!({
        "quota": {
            "version": 2,
            "provider_type": "kiro",
            "code": "ok",
            "label": serde_json::Value::Null,
            "reason": serde_json::Value::Null,
            "freshness": "fresh",
            "source": "refresh_api",
            "observed_at": 1_775_553_285u64,
            "exhausted": false,
            "usage_ratio": 0.25,
            "updated_at": 1_775_553_285u64,
            "reset_seconds": 86_400u64,
            "plan_type": "KIRO PRO+",
            "windows": [
                {
                    "code": "usage",
                    "label": "额度",
                    "scope": "account",
                    "unit": "count",
                    "used_ratio": 0.25,
                    "remaining_ratio": 0.75,
                    "used_value": 5.0,
                    "remaining_value": 15.0,
                    "limit_value": 20.0,
                    "reset_at": 1_775_639_685u64,
                    "reset_seconds": 86_400u64
                }
            ]
        }
    }));

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        vec![key],
    ));
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(GatewayDataState::with_provider_catalog_reader_for_tests(
            provider_catalog_repository,
        ));

    let response = local_admin_pool_response(
        &state,
        http::Method::GET,
        "/api/admin/pool/provider-kiro/keys?page=1&page_size=50&status=all",
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should read"),
    )
    .expect("json body should parse");
    let keys = payload["keys"].as_array().expect("keys should be array");

    assert_eq!(keys[0]["scheduling_status"], json!("available"));
    assert_eq!(keys[0]["scheduling_reason"], json!("available"));
    assert_eq!(keys[0]["quota_updated_at"], json!(1_775_553_285u64));
    assert_eq!(keys[0]["account_quota"], json!("剩余 75.0% (5/20)"));
}

#[tokio::test]
async fn gateway_codex_quota_resets_to_full_after_countdown_elapsed() {
    let mut provider = sample_provider("provider-codex", "codex", 10).with_transport_fields(
        true,
        false,
        true,
        None,
        None,
        None,
        None,
        None,
        Some(json!({
            "pool_advanced": {
                "enabled": true
            }
        })),
    );
    provider.provider_type = "codex".to_string();

    let mut key = sample_key(
        "key-codex-expired",
        "provider-codex",
        "openai:responses",
        "oauth-placeholder",
    );
    key.name = "codex expired quota key".to_string();
    key.auth_type = "oauth".to_string();
    key.upstream_metadata = Some(json!({
        "codex": {
            "plan_type": "plus",
            "updated_at": 1u64,
            "primary_used_percent": 42.0,
            "primary_reset_after_seconds": 60,
            "secondary_used_percent": 77.0,
            "secondary_reset_after_seconds": 120
        }
    }));

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        vec![key],
    ));
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(GatewayDataState::with_provider_catalog_reader_for_tests(
            provider_catalog_repository,
        ));

    let response = local_admin_pool_response(
        &state,
        http::Method::GET,
        "/api/admin/pool/provider-codex/keys?page=1&page_size=50&status=all",
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should read"),
    )
    .expect("json body should parse");
    let keys = payload["keys"].as_array().expect("keys should be array");
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0]["account_quota"], "周剩余 100.0% | 5H剩余 100.0%");
}

#[tokio::test]
async fn gateway_pool_prefers_upstream_plan_type_over_auth_config() {
    let mut provider = sample_provider("provider-codex", "codex", 10).with_transport_fields(
        true,
        false,
        true,
        None,
        None,
        None,
        None,
        None,
        Some(json!({
            "pool_advanced": {
                "enabled": true
            }
        })),
    );
    provider.provider_type = "codex".to_string();
    let mut key = sample_key(
        "key-codex-precedence",
        "provider-codex",
        "openai:responses",
        "oauth-placeholder",
    );
    key.auth_type = "oauth".to_string();
    key.encrypted_auth_config = Some(
        encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            &json!({
                "plan_type": "free",
                "account_id": "acct-codex-legacy"
            })
            .to_string(),
        )
        .expect("auth config should encrypt"),
    );
    key.upstream_metadata = Some(json!({
        "codex": {
            "plan_type": "plus",
            "updated_at": 1_775_553_285u64
        }
    }));

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        vec![key],
    ));
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(
            GatewayDataState::with_provider_catalog_reader_for_tests(provider_catalog_repository)
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
        );

    let response = local_admin_pool_response(
        &state,
        http::Method::GET,
        "/api/admin/pool/provider-codex/keys?page=1&page_size=50&status=all",
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should read"),
    )
    .expect("json body should parse");
    let keys = payload["keys"].as_array().expect("keys should be array");
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0]["oauth_plan_type"], "plus");
}

#[tokio::test]
async fn gateway_pool_plan_free_selector_prefers_upstream_plan_type() {
    let mut provider = sample_provider("provider-codex", "codex", 10).with_transport_fields(
        true,
        false,
        true,
        None,
        None,
        None,
        None,
        None,
        Some(json!({
            "pool_advanced": {
                "enabled": true
            }
        })),
    );
    provider.provider_type = "codex".to_string();
    let mut key = sample_key(
        "key-codex-selector",
        "provider-codex",
        "openai:responses",
        "oauth-placeholder",
    );
    key.auth_type = "oauth".to_string();
    key.encrypted_auth_config = Some(
        encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            &json!({
                "plan_type": "free",
                "account_id": "acct-codex-legacy"
            })
            .to_string(),
        )
        .expect("auth config should encrypt"),
    );
    key.upstream_metadata = Some(json!({
        "codex": {
            "plan_type": "plus",
            "updated_at": 1_775_553_285u64
        }
    }));

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        vec![key],
    ));
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(
            GatewayDataState::with_provider_catalog_reader_for_tests(provider_catalog_repository)
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
        );

    let response = local_admin_pool_response(
        &state,
        http::Method::POST,
        "/api/admin/pool/provider-codex/keys/resolve-selection",
        Some(json!({
            "quick_selectors": ["plan_free"]
        })),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should read"),
    )
    .expect("json body should parse");
    assert_eq!(payload["total"], json!(0));
    assert_eq!(
        payload["items"]
            .as_array()
            .expect("items should be array")
            .len(),
        0
    );
}

#[tokio::test]
async fn gateway_handles_admin_pool_resolve_selection_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/pool/provider-openai/keys/resolve-selection",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let provider = sample_provider("provider-openai", "openai", 10).with_transport_fields(
        true,
        false,
        true,
        None,
        None,
        None,
        None,
        None,
        Some(json!({
            "pool_advanced": {
                "enabled": true
            }
        })),
    );
    let mut proxy_key = sample_key("key-openai-a", "provider-openai", "openai:chat", "sk-a");
    proxy_key.name = "alpha proxy".to_string();
    proxy_key.proxy = Some(json!({
        "mode": "direct",
        "url": "https://proxy.example.com"
    }));
    let mut plain_key = sample_key("key-openai-b", "provider-openai", "openai:chat", "sk-b");
    plain_key.name = "beta".to_string();
    let mut disabled_proxy_key =
        sample_key("key-openai-c", "provider-openai", "openai:chat", "sk-c");
    disabled_proxy_key.name = "alpha disabled".to_string();
    disabled_proxy_key.is_active = false;
    disabled_proxy_key.proxy = Some(json!({
        "mode": "direct",
        "url": "https://proxy-disabled.example.com"
    }));
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        vec![proxy_key, plain_key, disabled_proxy_key],
    ));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(GatewayDataState::with_provider_catalog_reader_for_tests(
                provider_catalog_repository,
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/pool/provider-openai/keys/resolve-selection"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "search": "alpha",
            "quick_selectors": ["enabled", "proxy_set"]
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["total"], json!(1));
    let items = payload["items"].as_array().expect("items should be array");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["key_id"], json!("key-openai-a"));
    assert_eq!(items[0]["key_name"], json!("alpha proxy"));
    assert_eq!(items[0]["auth_type"], json!("api_key"));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_resolve_selection_marks_legacy_kiro_bearer_keys_as_oauth_managed() {
    let mut provider = sample_provider("provider-kiro", "kiro", 10);
    provider.provider_type = "kiro".to_string();
    let mut key = sample_key(
        "key-kiro-legacy",
        "provider-kiro",
        "kiro:generateAssistantResponse",
        "kiro-access-token",
    );
    key.auth_type = "bearer".to_string();
    key.encrypted_auth_config = Some(
        encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            r#"{"provider_type":"kiro","email":"legacy-kiro@example.com","refresh_token":"legacy-kiro-refresh-token"}"#,
        )
        .expect("auth config ciphertext should build"),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        vec![key],
    ));
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(
            GatewayDataState::with_provider_catalog_reader_for_tests(provider_catalog_repository)
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
        );

    let response = local_admin_pool_response(
        &state,
        http::Method::POST,
        "/api/admin/pool/provider-kiro/keys/resolve-selection",
        Some(json!({})),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should read"),
    )
    .expect("json body should parse");
    assert_eq!(payload["total"], json!(1));
    let items = payload["items"].as_array().expect("items should be array");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["key_id"], json!("key-kiro-legacy"));
    assert_eq!(items[0]["auth_type"], json!("bearer"));
    assert_eq!(items[0]["credential_kind"], json!("oauth_session"));
    assert_eq!(items[0]["runtime_auth_kind"], json!("bearer"));
    assert_eq!(items[0]["oauth_managed"], json!(true));
    assert_eq!(items[0]["can_refresh_oauth"], json!(true));
    assert_eq!(items[0]["can_export_oauth"], json!(true));
    assert_eq!(items[0]["can_edit_oauth"], json!(true));
}

#[tokio::test]
async fn gateway_handles_admin_pool_batch_action_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/pool/provider-openai/keys/batch-action",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let provider = sample_provider("provider-openai", "openai", 10).with_transport_fields(
        true,
        false,
        true,
        None,
        None,
        None,
        None,
        None,
        Some(json!({
            "pool_advanced": {
                "enabled": true
            }
        })),
    );
    let mut first_key = sample_key("key-openai-a", "provider-openai", "openai:chat", "sk-a");
    first_key.name = "alpha".to_string();
    let mut second_key = sample_key("key-openai-b", "provider-openai", "openai:chat", "sk-b");
    second_key.name = "beta".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        vec![first_key, second_key],
    ));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(Arc::clone(
                    &provider_catalog_repository,
                )),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/pool/provider-openai/keys/batch-action"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "action": "disable",
            "key_ids": ["key-openai-a", "key-openai-b"]
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["affected"], json!(2));
    assert_eq!(payload["message"], json!("2 keys disabled"));
    let stored = provider_catalog_repository
        .list_keys_by_ids(&["key-openai-a".to_string(), "key-openai-b".to_string()])
        .await
        .expect("keys should load");
    assert_eq!(stored.len(), 2);
    assert!(stored.iter().all(|item| !item.is_active));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_pool_batch_delete_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/pool/provider-openai/keys/batch-action",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let provider = sample_provider("provider-openai", "openai", 10).with_transport_fields(
        true,
        false,
        true,
        None,
        None,
        None,
        None,
        None,
        Some(json!({
            "pool_advanced": {
                "enabled": true
            }
        })),
    );
    let mut first_key = sample_key("key-openai-a", "provider-openai", "openai:chat", "sk-a");
    first_key.name = "alpha".to_string();
    let mut second_key = sample_key("key-openai-b", "provider-openai", "openai:chat", "sk-b");
    second_key.name = "beta".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        vec![first_key, second_key],
    ));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(Arc::clone(
                    &provider_catalog_repository,
                )),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/pool/provider-openai/keys/batch-action"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "action": "delete",
            "key_ids": ["key-openai-a", "key-openai-b"]
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["affected"], json!(2));
    assert_eq!(payload["message"], json!("2 keys deleted"));
    let stored = provider_catalog_repository
        .list_keys_by_ids(&["key-openai-a".to_string(), "key-openai-b".to_string()])
        .await
        .expect("keys should load");
    assert!(stored.is_empty());
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_pool_batch_delete_task_status_locally_with_trusted_admin_principal()
{
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/pool/provider-openai/keys/batch-delete-task/task-123",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let mut state = AppState::new().expect("gateway should build");
    state.put_provider_delete_task(crate::LocalProviderDeleteTaskState {
        task_id: "task-123".to_string(),
        provider_id: "provider-openai".to_string(),
        status: "running".to_string(),
        stage: "deleting_keys".to_string(),
        total_keys: 3,
        deleted_keys: 1,
        total_endpoints: 2,
        deleted_endpoints: 0,
        message: "deleted 1 / 3 keys".to_string(),
    });
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/pool/provider-openai/keys/batch-delete-task/task-123"
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
    assert_eq!(payload["task_id"], json!("task-123"));
    assert_eq!(payload["provider_id"], json!("provider-openai"));
    assert_eq!(payload["status"], json!("running"));
    assert_eq!(payload["stage"], json!("deleting_keys"));
    assert_eq!(payload["total_keys"], json!(3));
    assert_eq!(payload["deleted_keys"], json!(1));
    assert_eq!(payload["total_endpoints"], json!(2));
    assert_eq!(payload["deleted_endpoints"], json!(0));
    assert_eq!(payload["message"], json!("deleted 1 / 3 keys"));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn local_admin_pool_batch_delete_task_status_attaches_audit_only_for_terminal_states() {
    let mut completed_state = AppState::new().expect("gateway should build");
    completed_state.put_provider_delete_task(crate::LocalProviderDeleteTaskState {
        task_id: "task-completed".to_string(),
        provider_id: "provider-openai".to_string(),
        status: "completed".to_string(),
        stage: "completed".to_string(),
        total_keys: 3,
        deleted_keys: 3,
        total_endpoints: 0,
        deleted_endpoints: 0,
        message: "keys deleted".to_string(),
    });
    let completed_response = local_admin_pool_response(
        &completed_state,
        http::Method::GET,
        "/api/admin/pool/provider-openai/keys/batch-delete-task/task-completed",
        None,
    )
    .await;
    assert_eq!(completed_response.status(), StatusCode::OK);
    let completed_audit = completed_response
        .extensions()
        .get::<AdminAuditEvent>()
        .expect("completed pool batch delete task should attach audit");
    assert_eq!(
        completed_audit.event_name,
        "admin_pool_batch_delete_task_completed_viewed"
    );
    assert_eq!(
        completed_audit.action,
        "view_pool_batch_delete_task_terminal_state"
    );
    assert_eq!(
        completed_audit.target_type,
        "provider_key_batch_delete_task"
    );
    assert_eq!(completed_audit.target_id, "provider-openai:task-completed");

    let mut running_state = AppState::new().expect("gateway should build");
    running_state.put_provider_delete_task(crate::LocalProviderDeleteTaskState {
        task_id: "task-running".to_string(),
        provider_id: "provider-openai".to_string(),
        status: "running".to_string(),
        stage: "deleting_keys".to_string(),
        total_keys: 3,
        deleted_keys: 1,
        total_endpoints: 0,
        deleted_endpoints: 0,
        message: "deleted 1 / 3 keys".to_string(),
    });
    let running_response = local_admin_pool_response(
        &running_state,
        http::Method::GET,
        "/api/admin/pool/provider-openai/keys/batch-delete-task/task-running",
        None,
    )
    .await;
    assert_eq!(running_response.status(), StatusCode::OK);
    assert!(
        running_response
            .extensions()
            .get::<AdminAuditEvent>()
            .is_none(),
        "running pool batch delete task should not attach audit"
    );
}

#[tokio::test]
async fn gateway_cleans_up_admin_pool_banned_keys_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/pool/provider-openai/keys/cleanup-banned",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let provider = sample_provider("provider-openai", "openai", 10).with_transport_fields(
        true,
        false,
        true,
        None,
        None,
        None,
        None,
        None,
        Some(json!({
            "pool_advanced": {
                "enabled": true
            }
        })),
    );
    let mut banned_key = sample_key(
        "key-openai-banned",
        "provider-openai",
        "openai:chat",
        "sk-banned",
    );
    banned_key.name = "banned".to_string();
    banned_key.oauth_invalid_reason = Some("account_banned".to_string());
    let mut oauth_expired_key = sample_key(
        "key-openai-oauth-expired",
        "provider-openai",
        "openai:chat",
        "sk-oauth-expired",
    );
    oauth_expired_key.name = "oauth-expired".to_string();
    oauth_expired_key.oauth_invalid_reason = Some("[OAUTH_EXPIRED] token invalidated".to_string());
    let mut healthy_key = sample_key(
        "key-openai-healthy",
        "provider-openai",
        "openai:chat",
        "sk-healthy",
    );
    healthy_key.name = "healthy".to_string();

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        vec![banned_key, oauth_expired_key, healthy_key],
    ));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository.clone(),
                ),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/pool/provider-openai/keys/cleanup-banned"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({}))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["affected"], 1);
    assert_eq!(payload["message"], "已清理 1 个异常账号");

    let remaining_keys = provider_catalog_repository
        .list_keys_by_provider_ids(&["provider-openai".to_string()])
        .await
        .expect("remaining keys should load");
    assert_eq!(remaining_keys.len(), 2);
    assert!(remaining_keys
        .iter()
        .any(|key| key.id == "key-openai-oauth-expired"));
    assert!(remaining_keys
        .iter()
        .any(|key| key.id == "key-openai-healthy"));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_rejects_admin_pool_list_keys_with_empty_provider_id() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/pool//keys",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        Vec::new(),
        Vec::new(),
        Vec::new(),
    ));
    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(GatewayDataState::with_provider_catalog_reader_for_tests(
                provider_catalog_repository,
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/api/admin/pool//keys"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["detail"], "provider_id 无效");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_rejects_admin_pool_batch_import_with_empty_provider_id() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/pool//keys/batch-import",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        Vec::new(),
        Vec::new(),
        Vec::new(),
    ));
    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository,
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/pool//keys/batch-import"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({ "keys": [] }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["detail"], "provider_id 无效");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_rejects_admin_pool_cleanup_banned_with_empty_provider_id() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/pool//keys/cleanup-banned",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        Vec::new(),
        Vec::new(),
        Vec::new(),
    ));
    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository,
                ),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/pool//keys/cleanup-banned"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({}))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["detail"], "provider_id 无效");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}
