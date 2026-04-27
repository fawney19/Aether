use std::sync::{Arc, Mutex};

use aether_crypto::{encrypt_python_fernet_plaintext, DEVELOPMENT_ENCRYPTION_KEY};
use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
use aether_data_contracts::repository::provider_catalog::ProviderCatalogReadRepository;
use axum::body::{to_bytes, Body, Bytes};
use axum::routing::{any, get, post};
use axum::{extract::Request, Router};
use http::{HeaderMap, HeaderValue, StatusCode};
use serde_json::json;

use super::super::{
    build_router_with_state, sample_endpoint, sample_key, sample_provider, start_server, AppState,
};
use crate::admin_api::{maybe_build_local_admin_pool_response, AdminAppState, AdminRequestContext};
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
    assert_eq!(keys[0]["circuit_breaker_open"], json!(true));
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
        "gemini:chat",
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
            "reason": "Token 已过期，请重新授权",
            "expires_at": 1775556730u64,
            "invalid_at": null,
            "source": "expires_at",
            "requires_reauth": true,
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
        "gemini:chat",
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
            "reason": "Token 已过期，请重新授权",
            "expires_at": 1_775_556_730u64,
            "invalid_at": serde_json::Value::Null,
            "source": "expires_at",
            "requires_reauth": true,
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
        "gemini:chat",
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
        "gemini:chat",
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
        "claude:cli",
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
        "claude:cli",
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
        vec![banned_key, healthy_key],
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
    assert_eq!(remaining_keys.len(), 1);
    assert_eq!(remaining_keys[0].id, "key-openai-healthy");
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
