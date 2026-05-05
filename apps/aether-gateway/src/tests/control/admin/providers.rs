use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use aether_crypto::{encrypt_python_fernet_plaintext, DEVELOPMENT_ENCRYPTION_KEY};
use aether_data::repository::candidates::InMemoryRequestCandidateRepository;
use aether_data::repository::global_models::InMemoryGlobalModelReadRepository;
use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
use aether_data::repository::quota::InMemoryProviderQuotaRepository;
use aether_data_contracts::repository::candidates::RequestCandidateStatus;
use aether_data_contracts::repository::global_models::{
    GlobalModelReadRepository, StoredAdminGlobalModel, StoredAdminProviderModel,
    StoredProviderActiveGlobalModel, StoredProviderModelStats, StoredPublicGlobalModel,
};
use aether_data_contracts::repository::provider_catalog::{
    ProviderCatalogReadRepository, StoredProviderCatalogEndpoint, StoredProviderCatalogKey,
    StoredProviderCatalogProvider,
};
use axum::body::{Body, Bytes};
use axum::routing::any;
use axum::{extract::Request, Router};
use http::{HeaderMap, HeaderValue, StatusCode};
use serde_json::json;

use super::super::{
    build_router_with_state, issue_test_admin_access_token, sample_admin_provider_model,
    sample_endpoint, sample_key, sample_provider, sample_provider_active_global_model,
    sample_provider_model_stats, sample_provider_quota, sample_public_global_model_with_mappings,
    sample_request_candidate, start_server, AppState,
};
use crate::admin_api::{
    maybe_build_local_admin_providers_response, AdminAppState, AdminRequestContext,
};
use crate::audit::AdminAuditEvent;
use crate::constants::{
    GATEWAY_HEADER, TRUSTED_ADMIN_MANAGEMENT_TOKEN_ID_HEADER, TRUSTED_ADMIN_SESSION_ID_HEADER,
    TRUSTED_ADMIN_USER_ID_HEADER, TRUSTED_ADMIN_USER_ROLE_HEADER,
};
use crate::control::resolve_public_request_context;
use crate::data::GatewayDataState;

const ADMIN_PROVIDERS_DATA_UNAVAILABLE_DETAIL: &str = "Admin provider catalog data unavailable";

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

async fn local_admin_providers_response(
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
    maybe_build_local_admin_providers_response(
        &AdminAppState::new(state),
        &AdminRequestContext::new(&request_context),
        body_bytes.as_ref(),
    )
    .await
    .expect("local providers response should build")
    .expect("providers route should resolve locally")
}

#[tokio::test]
async fn gateway_handles_admin_providers_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/providers/",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![
            sample_provider("provider-openai", "openai", 10)
                .with_timestamps(Some(1_711_000_000), Some(1_711_000_100)),
            sample_provider("provider-anthropic", "anthropic", 20)
                .with_timestamps(Some(1_710_000_000), Some(1_710_000_100)),
        ],
        vec![sample_endpoint(
            "endpoint-openai-chat",
            "provider-openai",
            "openai:chat",
            "https://api.openai.example",
        )],
        vec![sample_key(
            "key-openai-a",
            "provider-openai",
            "openai:chat",
            "sk-test-a",
        )],
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
        .get(format!(
            "{gateway_url}/api/admin/providers/?skip=0&limit=50"
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
    let items = payload.as_array().expect("payload should be an array");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["id"], "provider-openai");
    assert_eq!(items[0]["name"], "openai");
    assert_eq!(items[0]["api_format"], "openai:chat");
    assert_eq!(items[0]["base_url"], "https://api.openai.example");
    assert_eq!(items[0]["api_key"], "***");
    assert_eq!(items[0]["priority"], 10);
    assert_eq!(items[0]["created_at"], "2024-03-21T05:46:40Z");
    assert_eq!(items[0]["updated_at"], "2024-03-21T05:48:20Z");
    assert_eq!(items[1]["id"], "provider-anthropic");
    assert_eq!(items[1]["api_format"], serde_json::Value::Null);
    assert_eq!(items[1]["base_url"], serde_json::Value::Null);
    assert_eq!(items[1]["api_key"], serde_json::Value::Null);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_providers_locally_with_local_503_when_catalog_reader_unavailable() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/providers/",
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
            "{gateway_url}/api/admin/providers/?skip=0&limit=50"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["detail"], ADMIN_PROVIDERS_DATA_UNAVAILABLE_DETAIL);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}
#[tokio::test]
async fn gateway_handles_admin_provider_summary_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/providers/provider-openai/summary",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let provider = sample_provider("provider-openai", "openai", 10)
        .with_description(Some("OpenAI primary provider".to_string()))
        .with_transport_fields(
            true,
            false,
            true,
            None,
            Some(4),
            Some(json!({"host": "proxy.example", "password": "secret"})),
            Some(45.0),
            Some(12.0),
            Some(json!({
                "claude_code_advanced": {"pool_size": 3},
                "pool_advanced": {"enabled": true},
                "failover_rules": {"strategy": "ordered"},
                "chat_pii_redaction": {"enabled": true},
                "provider_ops": {"architecture_id": "anyrouter"}
            })),
        );
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider.with_timestamps(Some(1_711_000_000), Some(1_711_000_100))],
        vec![
            sample_endpoint(
                "endpoint-openai-chat",
                "provider-openai",
                "openai:chat",
                "https://api.openai.example",
            ),
            sample_endpoint(
                "endpoint-openai-cli",
                "provider-openai",
                "openai:responses",
                "https://api.openai.example",
            ),
        ],
        vec![
            sample_key(
                "key-openai-chat",
                "provider-openai",
                "openai:chat",
                "sk-test-chat",
            )
            .with_health_fields(Some(json!({"openai:chat": {"health_score": 0.25}})), None),
            sample_key(
                "key-openai-cli",
                "provider-openai",
                "openai:responses",
                "sk-test-cli",
            )
            .with_transport_fields(
                Some(json!(["openai:responses"])),
                encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "sk-test-cli-2")
                    .expect("api key ciphertext should build"),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .expect("key transport should build")
            .with_health_fields(
                Some(json!({"openai:responses": {"health_score": 0.75}})),
                None,
            ),
        ],
    ));
    let global_model_repository = Arc::new(
        InMemoryGlobalModelReadRepository::seed(Vec::new())
            .with_provider_model_stats(vec![sample_provider_model_stats("provider-openai", 5, 3)])
            .with_active_global_model_refs(vec![
                sample_provider_active_global_model("provider-openai", "gpt-5"),
                sample_provider_active_global_model("provider-openai", "gpt-5-mini"),
            ]),
    );
    let quota_repository = Arc::new(InMemoryProviderQuotaRepository::seed(vec![
        sample_provider_quota("provider-openai"),
    ]));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_global_model_and_quota_readers_for_tests(
                    provider_catalog_repository,
                    global_model_repository,
                    quota_repository,
                ),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/providers/provider-openai/summary"
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
    assert_eq!(payload["id"], "provider-openai");
    assert_eq!(payload["name"], "openai");
    assert_eq!(payload["description"], "OpenAI primary provider");
    assert_eq!(payload["billing_type"], "monthly_quota");
    assert_eq!(payload["monthly_quota_usd"], 100.0);
    assert_eq!(payload["monthly_used_usd"], 12.5);
    assert_eq!(payload["total_endpoints"], 2);
    assert_eq!(payload["active_endpoints"], 2);
    assert_eq!(payload["total_keys"], 2);
    assert_eq!(payload["active_keys"], 2);
    assert_eq!(payload["total_models"], 5);
    assert_eq!(payload["active_models"], 3);
    assert_eq!(payload["global_model_ids"], json!(["gpt-5", "gpt-5-mini"]));
    assert_eq!(
        payload["api_formats"],
        json!(["openai:chat", "openai:responses"])
    );
    assert_eq!(payload["ops_configured"], true);
    assert_eq!(payload["ops_architecture_id"], "anyrouter");
    assert_eq!(payload["chat_pii_redaction"], json!({"enabled": true}));
    assert_eq!(payload["created_at"], "2024-03-21T05:46:40Z");
    assert_eq!(payload["updated_at"], "2024-03-21T05:48:20Z");
    assert_eq!(
        payload["endpoint_health_details"],
        json!([
            {
                "api_format": "openai:chat",
                "health_score": 0.25,
                "is_active": true,
                "total_keys": 1,
                "active_keys": 1
            },
            {
                "api_format": "openai:responses",
                "health_score": 0.75,
                "is_active": true,
                "total_keys": 1,
                "active_keys": 1
            }
        ])
    );
    assert_eq!(payload["avg_health_score"], 0.5);
    assert_eq!(payload["unhealthy_endpoints"], 1);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_providers_summary_list_locally_with_bearer_admin_session() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/providers/summary",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![
            sample_provider("provider-openai", "openai", 10)
                .with_timestamps(Some(1_711_000_000), Some(1_711_000_100)),
            sample_provider("provider-anthropic", "anthropic", 20)
                .with_timestamps(Some(1_710_000_000), Some(1_710_000_100)),
        ],
        vec![],
        vec![],
    ));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(GatewayDataState::with_provider_catalog_reader_for_tests(
            provider_catalog_repository,
        ));
    let access_token = issue_test_admin_access_token(&state, "device-admin-providers").await;
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/providers/summary?page=1&page_size=20"
        ))
        .header("authorization", format!("Bearer {access_token}"))
        .header("x-client-device-id", "device-admin-providers")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    let items = payload["items"].as_array().expect("items should be array");
    assert_eq!(items.len(), 2);
    let mut ids = items
        .iter()
        .filter_map(|item| item.get("id").and_then(serde_json::Value::as_str))
        .collect::<Vec<_>>();
    ids.sort_unstable();
    assert_eq!(ids, vec!["provider-anthropic", "provider-openai"]);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_provider_summary_locally_with_bearer_admin_session() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/providers/provider-openai/summary",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-openai", "openai", 10)
            .with_description(Some("OpenAI primary provider".to_string()))
            .with_timestamps(Some(1_711_000_000), Some(1_711_000_100))],
        vec![sample_endpoint(
            "endpoint-openai-chat",
            "provider-openai",
            "openai:chat",
            "https://api.openai.example",
        )],
        vec![sample_key(
            "key-openai-chat",
            "provider-openai",
            "openai:chat",
            "sk-test-chat",
        )],
    ));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(GatewayDataState::with_provider_catalog_reader_for_tests(
            provider_catalog_repository,
        ));
    let access_token = issue_test_admin_access_token(&state, "device-admin-provider-summary").await;
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/providers/provider-openai/summary"
        ))
        .header("authorization", format!("Bearer {access_token}"))
        .header("x-client-device-id", "device-admin-provider-summary")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["id"], "provider-openai");
    assert_eq!(payload["name"], "openai");
    assert_eq!(payload["description"], "OpenAI primary provider");
    assert_eq!(payload["total_endpoints"], 1);
    assert_eq!(payload["total_keys"], 1);
    assert_eq!(payload["api_formats"], json!(["openai:chat"]));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_providers_summary_list_locally_with_local_503_when_catalog_reader_unavailable(
) {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/providers/summary",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let state = AppState::new().expect("gateway should build");
    let access_token = issue_test_admin_access_token(&state, "device-admin-providers-503").await;
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/providers/summary?page=1&page_size=20"
        ))
        .header("authorization", format!("Bearer {access_token}"))
        .header("x-client-device-id", "device-admin-providers-503")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["detail"], ADMIN_PROVIDERS_DATA_UNAVAILABLE_DETAIL);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_providers_summary_list_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/providers/summary",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![
            sample_provider("provider-openai", "openai", 10)
                .with_description(Some("OpenAI primary provider".to_string()))
                .with_timestamps(Some(1_711_000_000), Some(1_711_000_100)),
            sample_provider("provider-anthropic", "anthropic", 20)
                .with_description(Some("Anthropic backup provider".to_string()))
                .with_transport_fields(false, false, true, None, Some(2), None, None, None, None)
                .with_timestamps(Some(1_710_000_000), Some(1_710_000_100)),
        ],
        vec![
            sample_endpoint(
                "endpoint-openai-chat",
                "provider-openai",
                "openai:chat",
                "https://api.openai.example",
            ),
            sample_endpoint(
                "endpoint-anthropic-chat",
                "provider-anthropic",
                "claude:messages",
                "https://api.anthropic.example",
            ),
        ],
        vec![sample_key(
            "key-openai-chat",
            "provider-openai",
            "openai:chat",
            "sk-test-chat",
        )
        .with_health_fields(Some(json!({"openai:chat": {"health_score": 0.8}})), None)],
    ));
    let global_model_repository = Arc::new(
        InMemoryGlobalModelReadRepository::seed(Vec::new())
            .with_provider_model_stats(vec![
                sample_provider_model_stats("provider-openai", 5, 3),
                sample_provider_model_stats("provider-anthropic", 2, 1),
            ])
            .with_active_global_model_refs(vec![
                sample_provider_active_global_model("provider-openai", "gpt-5"),
                sample_provider_active_global_model("provider-anthropic", "claude-sonnet-4-5"),
            ]),
    );
    let quota_repository = Arc::new(InMemoryProviderQuotaRepository::seed(vec![
        sample_provider_quota("provider-openai"),
        sample_provider_quota("provider-anthropic"),
    ]));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_global_model_and_quota_readers_for_tests(
                    provider_catalog_repository,
                    global_model_repository,
                    quota_repository,
                ),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/providers/summary?page=1&page_size=20&search=open&status=active&api_format=openai:chat&model_id=gpt-5"
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
    assert_eq!(payload["total"], 1);
    assert_eq!(payload["page"], 1);
    assert_eq!(payload["page_size"], 20);
    let items = payload["items"]
        .as_array()
        .expect("items should be an array");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], "provider-openai");
    assert_eq!(items[0]["name"], "openai");
    assert_eq!(items[0]["total_models"], 5);
    assert_eq!(items[0]["active_models"], 3);
    assert_eq!(items[0]["global_model_ids"], json!(["gpt-5"]));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_returns_service_unavailable_for_admin_providers_summary_without_provider_catalog_reader(
) {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/providers/summary",
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
            "{gateway_url}/api/admin/providers/summary?page=1&page_size=20"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["detail"], "Admin provider catalog data unavailable");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_returns_service_unavailable_for_admin_provider_create_without_provider_catalog_writer(
) {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/providers",
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
        .post(format!("{gateway_url}/api/admin/providers"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "name": "openai",
            "provider_type": "openai",
            "provider_priority": 10
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["detail"], "Admin provider catalog data unavailable");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_updates_admin_provider_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/providers/provider-openai",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![
            sample_provider("provider-openai", "openai", 10)
                .with_transport_fields(
                    true,
                    false,
                    false,
                    None,
                    None,
                    None,
                    None,
                    None,
                    Some(json!({
                        "pool_advanced": {},
                        "failover_rules": {"strategy": "ordered"}
                    })),
                )
                .with_timestamps(Some(1_711_000_000), Some(1_711_000_100)),
            sample_provider("provider-other", "other", 20),
        ],
        vec![sample_endpoint(
            "endpoint-openai-chat",
            "provider-openai",
            "openai:chat",
            "https://api.openai.example",
        )],
        Vec::new(),
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
        .patch(format!("{gateway_url}/api/admin/providers/provider-openai"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "name": "openai-renamed",
            "provider_type": "claude_code",
            "description": "Updated provider",
            "website": "https://updated.example",
            "provider_priority": 3,
            "keep_priority_on_conversion": true,
            "is_active": false,
            "concurrent_limit": 8,
            "max_retries": 6,
            "request_timeout": 55.0,
            "stream_first_byte_timeout": 11.0,
            "enable_format_conversion": false,
            "config": {
                "provider_ops": {"architecture_id": "cubence"},
                "chat_pii_redaction": {"enabled": true}
            },
            "claude_code_advanced": {"pool_size": 2},
            "proxy": {"url": "https://proxy.example"}
        }))
        .send()
        .await
        .expect("request should succeed");

    let status = response.status();
    let body = response.text().await.expect("body should read");
    assert_eq!(status, StatusCode::OK, "body={body}");
    let payload: serde_json::Value = serde_json::from_str(&body).expect("json body should parse");
    assert_eq!(payload["id"], "provider-openai");
    assert_eq!(payload["name"], "openai-renamed");
    assert_eq!(payload["provider_type"], "claude_code");
    assert_eq!(payload["description"], "Updated provider");
    assert_eq!(payload["website"], "https://updated.example");
    assert_eq!(payload["provider_priority"], 3);
    assert_eq!(payload["keep_priority_on_conversion"], true);
    assert_eq!(payload["enable_format_conversion"], false);
    assert_eq!(payload["is_active"], false);
    assert_eq!(payload["max_retries"], 6);
    assert_eq!(payload["request_timeout"], 55.0);
    assert_eq!(payload["stream_first_byte_timeout"], 11.0);
    assert_eq!(payload["proxy"], json!({"url": "https://proxy.example"}));
    assert_eq!(payload["claude_code_advanced"], json!({"pool_size": 2}));
    assert_eq!(payload["pool_advanced"], json!({}));
    assert_eq!(payload["failover_rules"], json!({"strategy": "ordered"}));
    assert_eq!(payload["chat_pii_redaction"], json!({"enabled": true}));
    assert_eq!(payload["ops_configured"], true);
    assert_eq!(payload["ops_architecture_id"], "cubence");

    let disable_response = reqwest::Client::new()
        .patch(format!("{gateway_url}/api/admin/providers/provider-openai"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "config": {
                "chat_pii_redaction": {"enabled": false}
            }
        }))
        .send()
        .await
        .expect("request should succeed");
    let disable_status = disable_response.status();
    let disable_body = disable_response.text().await.expect("body should read");
    assert_eq!(disable_status, StatusCode::OK, "body={disable_body}");
    let disable_payload: serde_json::Value =
        serde_json::from_str(&disable_body).expect("json body should parse");
    assert_eq!(
        disable_payload["chat_pii_redaction"],
        json!({"enabled": false})
    );
    assert_eq!(disable_payload["pool_advanced"], json!({}));
    assert_eq!(
        disable_payload["failover_rules"],
        json!({"strategy": "ordered"})
    );
    assert_eq!(disable_payload["ops_architecture_id"], "cubence");

    let invalid_response = reqwest::Client::new()
        .patch(format!("{gateway_url}/api/admin/providers/provider-openai"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "config": {
                "chat_pii_redaction": {"enabled": true, "entities": ["email"]}
            }
        }))
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(invalid_response.status(), StatusCode::BAD_REQUEST);

    let providers = provider_catalog_repository
        .list_providers(false)
        .await
        .expect("providers should list");
    let updated_provider = providers
        .iter()
        .find(|provider| provider.id == "provider-openai")
        .expect("provider should exist");
    assert_eq!(
        updated_provider
            .config
            .as_ref()
            .and_then(|value| value.get("chat_pii_redaction"))
            .cloned(),
        Some(json!({"enabled": false}))
    );
    assert_eq!(
        updated_provider
            .config
            .as_ref()
            .and_then(|value| value.get("pool_advanced"))
            .cloned(),
        Some(json!({}))
    );
    assert_eq!(
        updated_provider
            .config
            .as_ref()
            .and_then(|value| value.get("failover_rules"))
            .cloned(),
        Some(json!({"strategy": "ordered"}))
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_creates_admin_provider_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/providers/",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-existing", "existing", 0)],
        Vec::new(),
        Vec::new(),
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
        .post(format!("{gateway_url}/api/admin/providers/"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "name": "codex-provider",
            "provider_type": "codex",
            "description": "Codex managed provider",
            "website": "codex.example",
            "keep_priority_on_conversion": true,
            "max_retries": 7,
            "config": {"chat_pii_redaction": {"enabled": true}},
            "pool_advanced": {},
            "failover_rules": {"strategy": "ordered"},
            "proxy": {"url": "https://proxy.example"}
        }))
        .send()
        .await
        .expect("request should succeed");

    let status = response.status();
    let body = response.text().await.expect("body should read");
    assert_eq!(status, StatusCode::OK, "body={body}");
    let payload: serde_json::Value = serde_json::from_str(&body).expect("json body should parse");
    assert_eq!(payload["name"], "codex-provider");
    assert_eq!(payload["message"], "提供商创建成功");

    let providers = provider_catalog_repository
        .list_providers(false)
        .await
        .expect("providers should list");
    let created = providers
        .iter()
        .find(|provider| provider.name == "codex-provider")
        .expect("created provider should exist");
    let existing = providers
        .iter()
        .find(|provider| provider.id == "provider-existing")
        .expect("existing provider should remain");
    assert_eq!(created.provider_type, "codex");
    assert_eq!(created.provider_priority, 0);
    assert_eq!(existing.provider_priority, 1);
    assert_eq!(created.website.as_deref(), Some("https://codex.example"));
    assert!(created.enable_format_conversion);
    assert_eq!(created.max_retries, Some(7));
    assert_eq!(created.keep_priority_on_conversion, true);
    assert_eq!(
        created
            .config
            .as_ref()
            .and_then(|value| value.get("pool_advanced"))
            .cloned(),
        Some(json!({}))
    );
    assert_eq!(
        created
            .config
            .as_ref()
            .and_then(|value| value.get("chat_pii_redaction"))
            .cloned(),
        Some(json!({"enabled": true}))
    );
    assert_eq!(
        created
            .config
            .as_ref()
            .and_then(|value| value.get("failover_rules"))
            .cloned(),
        Some(json!({"strategy": "ordered"}))
    );

    let invalid_response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/providers/"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "name": "invalid-redaction-provider",
            "provider_type": "custom",
            "config": {"chat_pii_redaction": {"enabled": true, "entities": ["email"]}}
        }))
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(invalid_response.status(), StatusCode::BAD_REQUEST);

    let endpoints = provider_catalog_repository
        .list_endpoints_by_provider_ids(std::slice::from_ref(&created.id))
        .await
        .expect("endpoints should list");
    assert_eq!(endpoints.len(), 3);
    let responses_endpoint = endpoints
        .iter()
        .find(|endpoint| endpoint.api_format == "openai:responses")
        .expect("responses endpoint should exist");
    let compact_endpoint = endpoints
        .iter()
        .find(|endpoint| endpoint.api_format == "openai:responses:compact")
        .expect("compact endpoint should exist");
    let image_endpoint = endpoints
        .iter()
        .find(|endpoint| endpoint.api_format == "openai:image")
        .expect("image endpoint should exist");
    assert_eq!(
        responses_endpoint.base_url,
        "https://chatgpt.com/backend-api/codex"
    );
    assert_eq!(
        compact_endpoint.base_url,
        "https://chatgpt.com/backend-api/codex"
    );
    assert_eq!(
        image_endpoint.base_url,
        "https://chatgpt.com/backend-api/codex"
    );
    assert_eq!(responses_endpoint.max_retries, Some(7));
    assert_eq!(compact_endpoint.max_retries, Some(7));
    assert_eq!(image_endpoint.max_retries, Some(7));
    assert_eq!(
        responses_endpoint
            .config
            .as_ref()
            .and_then(|value| value.get("upstream_stream_policy"))
            .and_then(serde_json::Value::as_str),
        Some("force_stream")
    );
    assert_eq!(
        image_endpoint
            .config
            .as_ref()
            .and_then(|value| value.get("upstream_stream_policy"))
            .and_then(serde_json::Value::as_str),
        Some("force_stream")
    );
    assert!(responses_endpoint.body_rules.is_none());
    assert!(compact_endpoint.body_rules.is_none());
    assert!(image_endpoint.body_rules.is_none());
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_updates_fixed_provider_and_reconciles_template_managed_endpoints() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/providers/provider-codex",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let mut provider = sample_provider("provider-codex", "codex", 10).with_transport_fields(
        true,
        false,
        true,
        None,
        Some(2),
        None,
        None,
        None,
        None,
    );
    provider.provider_type = "codex".to_string();
    let mut cli_endpoint = sample_endpoint(
        "endpoint-codex-cli",
        "provider-codex",
        "openai:responses",
        "https://chatgpt.com/backend-api/codex",
    );
    cli_endpoint.max_retries = Some(2);
    cli_endpoint.config = Some(json!({"upstream_stream_policy": "force_stream"}));
    let mut key = sample_key(
        "key-codex-oauth",
        "provider-codex",
        "openai:responses",
        "oauth-placeholder",
    );
    key.auth_type = "oauth".to_string();
    key.api_formats = Some(json!(["openai:responses"]));

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![cli_endpoint],
        vec![key],
    ));

    let (_upstream_url, upstream_handle) = start_server(upstream).await;
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
        .patch(format!("{gateway_url}/api/admin/providers/provider-codex"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "max_retries": 9
        }))
        .send()
        .await
        .expect("request should succeed");

    let status = response.status();
    let body = response.text().await.expect("body should read");
    assert_eq!(status, StatusCode::OK, "body={body}");

    let endpoints = provider_catalog_repository
        .list_endpoints_by_provider_ids(&["provider-codex".to_string()])
        .await
        .expect("endpoints should list");
    assert_eq!(endpoints.len(), 3);
    let responses_endpoint = endpoints
        .iter()
        .find(|endpoint| endpoint.api_format == "openai:responses")
        .expect("responses endpoint should exist");
    let compact_endpoint = endpoints
        .iter()
        .find(|endpoint| endpoint.api_format == "openai:responses:compact")
        .expect("compact endpoint should exist");
    let image_endpoint = endpoints
        .iter()
        .find(|endpoint| endpoint.api_format == "openai:image")
        .expect("image endpoint should exist");

    assert_eq!(responses_endpoint.max_retries, Some(9));
    assert_eq!(compact_endpoint.max_retries, Some(9));
    assert_eq!(image_endpoint.max_retries, Some(9));
    assert_eq!(
        responses_endpoint
            .config
            .as_ref()
            .and_then(|value| value.get("_aether_fixed_provider_template"))
            .and_then(|value| value.get("managed"))
            .and_then(serde_json::Value::as_bool),
        Some(true)
    );
    assert_eq!(
        image_endpoint
            .config
            .as_ref()
            .and_then(|value| value.get("upstream_stream_policy"))
            .and_then(serde_json::Value::as_str),
        Some("force_stream")
    );
    let keys = provider_catalog_repository
        .list_keys_by_provider_ids(&["provider-codex".to_string()])
        .await
        .expect("keys should list");
    assert_eq!(keys.len(), 1);
    assert!(keys[0].api_formats.is_none());
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_lists_effective_api_formats_for_fixed_oauth_provider_keys() {
    let mut provider = sample_provider("provider-codex", "codex", 10)
        .with_transport_fields(true, false, true, None, None, None, None, None, None);
    provider.provider_type = "codex".to_string();

    let mut key = sample_key(
        "key-codex-legacy",
        "provider-codex",
        "openai:responses",
        "oauth-placeholder",
    );
    key.name = "codex legacy".to_string();
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
            "{gateway_url}/api/admin/endpoints/providers/provider-codex/keys?skip=0&limit=100"
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
    let keys = payload.as_array().expect("keys payload should be array");

    assert_eq!(keys.len(), 1);
    assert_eq!(
        keys[0]["api_formats"],
        json!(["openai:image", "openai:responses"])
    );

    gateway_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_provider_health_monitor_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/providers/provider-openai/health-monitor",
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
        vec![
            sample_endpoint(
                "endpoint-openai-chat",
                "provider-openai",
                "openai:chat",
                "https://api.openai.example",
            ),
            sample_endpoint(
                "endpoint-openai-cli",
                "provider-openai",
                "openai:responses",
                "https://api.openai.example",
            ),
        ],
        Vec::new(),
    ));
    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("current time should be after epoch")
        .as_secs() as i64;
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::seed(vec![
        sample_request_candidate(
            "cand-chat-success",
            "req-chat-success",
            "endpoint-openai-chat",
            RequestCandidateStatus::Success,
            now_unix_secs - 3_000,
            Some(now_unix_secs - 2_980),
        ),
        sample_request_candidate(
            "cand-chat-failed",
            "req-chat-failed",
            "endpoint-openai-chat",
            RequestCandidateStatus::Failed,
            now_unix_secs - 2_000,
            Some(now_unix_secs - 1_980),
        ),
        sample_request_candidate(
            "cand-cli-skipped",
            "req-cli-skipped",
            "endpoint-openai-cli",
            RequestCandidateStatus::Skipped,
            now_unix_secs - 1_500,
            Some(now_unix_secs - 1_490),
        ),
    ]));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_and_request_candidate_reader_for_tests(
                    provider_catalog_repository,
                    request_candidate_repository,
                ),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/providers/provider-openai/health-monitor?lookback_hours=6&per_endpoint_limit=48"
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
    assert_eq!(payload["provider_id"], "provider-openai");
    assert_eq!(payload["provider_name"], "openai");
    assert!(payload["generated_at"].as_str().is_some());
    let endpoints = payload["endpoints"]
        .as_array()
        .expect("endpoints should be an array");
    assert_eq!(endpoints.len(), 2);
    assert_eq!(endpoints[0]["endpoint_id"], "endpoint-openai-chat");
    assert_eq!(endpoints[0]["api_format"], "openai:chat");
    assert_eq!(endpoints[0]["total_attempts"], 2);
    assert_eq!(endpoints[0]["success_count"], 1);
    assert_eq!(endpoints[0]["failed_count"], 1);
    assert_eq!(endpoints[0]["skipped_count"], 0);
    assert_eq!(endpoints[0]["success_rate"], 0.5);
    assert!(endpoints[0]["last_event_at"]
        .as_str()
        .is_some_and(|value| value.ends_with(".000Z")));
    assert_eq!(
        endpoints[0]["events"]
            .as_array()
            .expect("events should be an array")
            .len(),
        2
    );
    assert!(endpoints[0]["events"][0]["timestamp"]
        .as_str()
        .is_some_and(|value| value.ends_with(".000Z")));
    assert_eq!(endpoints[1]["endpoint_id"], "endpoint-openai-cli");
    assert_eq!(endpoints[1]["api_format"], "openai:responses");
    assert_eq!(endpoints[1]["total_attempts"], 1);
    assert_eq!(endpoints[1]["success_count"], 0);
    assert_eq!(endpoints[1]["failed_count"], 0);
    assert_eq!(endpoints[1]["skipped_count"], 1);
    assert_eq!(endpoints[1]["success_rate"], 0.0);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_provider_mapping_preview_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/providers/provider-openai/mapping-preview",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let mut mapping_key = sample_key(
        "key-openai-preview",
        "provider-openai",
        "openai:chat",
        "sk-preview-1234",
    );
    mapping_key.allowed_models = Some(json!(["gpt-5", "gpt-4.1-mini"]));

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-openai", "openai", 10)],
        Vec::new(),
        vec![mapping_key],
    ));
    let global_model_repository = Arc::new(InMemoryGlobalModelReadRepository::seed(vec![
        sample_public_global_model_with_mappings("global-gpt-5", "gpt-5", "GPT 5", &["gpt-5"]),
        sample_public_global_model_with_mappings(
            "global-gpt-4.1-mini",
            "gpt-4.1-mini",
            "GPT 4.1 mini",
            &["gpt-4\\.1-.*"],
        ),
    ]));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_reader_for_tests(
                    provider_catalog_repository,
                )
                .with_global_model_repository_for_tests(global_model_repository),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/providers/provider-openai/mapping-preview"
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
    assert_eq!(payload["provider_id"], "provider-openai");
    assert_eq!(payload["provider_name"], "openai");
    assert_eq!(payload["total_keys"], 1);
    assert_eq!(payload["total_matches"], 2);
    assert_eq!(payload["truncated"], false);

    let keys = payload["keys"].as_array().expect("keys should be an array");
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0]["key_id"], "key-openai-preview");
    assert_eq!(keys[0]["masked_key"], "sk-p***1234");
    assert_eq!(keys[0]["allowed_models"], json!(["gpt-5", "gpt-4.1-mini"]));

    let matches = keys[0]["matching_global_models"]
        .as_array()
        .expect("matching models should be an array");
    assert_eq!(matches.len(), 2);
    let gpt5 = matches
        .iter()
        .find(|item| item["global_model_name"] == "gpt-5")
        .expect("gpt-5 match should exist");
    let gpt41mini = matches
        .iter()
        .find(|item| item["global_model_name"] == "gpt-4.1-mini")
        .expect("gpt-4.1-mini match should exist");
    assert_eq!(gpt5["global_model_name"], "gpt-5");
    assert_eq!(gpt5["matched_models"][0]["mapping_pattern"], "gpt-5");
    assert_eq!(gpt41mini["global_model_name"], "gpt-4.1-mini");
    assert_eq!(
        gpt41mini["matched_models"][0]["mapping_pattern"],
        "gpt-4\\.1-.*"
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_submits_admin_provider_delete_task_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new()
        .route(
            "/api/admin/providers/provider-openai",
            any(move |_request: Request| {
                let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
                async move {
                    *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                    (StatusCode::OK, Body::from("unexpected upstream hit"))
                }
            }),
        )
        .route(
            "/api/admin/providers/provider-openai/delete-task/{task_id}",
            any(|_request: Request| async move {
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }),
        );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-openai", "openai", 10)],
        vec![sample_endpoint(
            "endpoint-openai-chat",
            "provider-openai",
            "openai:chat",
            "https://api.openai.example",
        )],
        vec![sample_key(
            "key-openai-delete",
            "provider-openai",
            "openai:chat",
            "sk-delete",
        )],
    ));
    let global_model_repository = Arc::new(
        InMemoryGlobalModelReadRepository::seed(Vec::new()).with_admin_provider_models(vec![
            sample_admin_provider_model(
                "model-openai-delete",
                "provider-openai",
                "global-gpt-5",
                "gpt-5",
            ),
        ]),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository.clone(),
                )
                .with_global_model_repository_for_tests(global_model_repository.clone()),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .delete(format!("{gateway_url}/api/admin/providers/provider-openai"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let submit_payload: serde_json::Value = response.json().await.expect("json body should parse");
    let task_id = submit_payload["task_id"]
        .as_str()
        .expect("task id should be present")
        .to_string();
    assert_eq!(submit_payload["status"], "pending");
    assert_eq!(
        submit_payload["message"],
        "删除任务已提交，提供商已进入后台删除队列"
    );

    let task_response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/providers/provider-openai/delete-task/{task_id}"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(task_response.status(), StatusCode::OK);
    let task_payload: serde_json::Value =
        task_response.json().await.expect("json body should parse");
    assert_eq!(task_payload["task_id"], task_id);
    assert_eq!(task_payload["provider_id"], "provider-openai");
    assert_eq!(task_payload["status"], "completed");
    assert_eq!(task_payload["stage"], "completed");
    assert_eq!(task_payload["total_keys"], 1);
    assert_eq!(task_payload["deleted_keys"], 1);
    assert_eq!(task_payload["total_endpoints"], 1);
    assert_eq!(task_payload["deleted_endpoints"], 1);

    let remaining_providers = provider_catalog_repository
        .list_providers(false)
        .await
        .expect("provider list should load");
    assert!(remaining_providers.is_empty());
    let remaining_endpoints = provider_catalog_repository
        .list_endpoints_by_provider_ids(&["provider-openai".to_string()])
        .await
        .expect("endpoint list should load");
    assert!(remaining_endpoints.is_empty());
    let remaining_keys = provider_catalog_repository
        .list_keys_by_provider_ids(&["provider-openai".to_string()])
        .await
        .expect("key list should load");
    assert!(remaining_keys.is_empty());
    let remaining_models = global_model_repository
        .list_admin_provider_models(
            &aether_data_contracts::repository::global_models::AdminProviderModelListQuery {
                provider_id: "provider-openai".to_string(),
                offset: 0,
                limit: 100,
                is_active: None,
            },
        )
        .await
        .expect("provider model list should load");
    assert!(remaining_models.is_empty());
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn local_admin_provider_delete_task_status_attaches_audit_only_for_terminal_states() {
    let mut completed_state = AppState::new().expect("gateway should build");
    completed_state.put_provider_delete_task(crate::LocalProviderDeleteTaskState {
        task_id: "task-completed".to_string(),
        provider_id: "provider-openai".to_string(),
        status: "completed".to_string(),
        stage: "completed".to_string(),
        total_keys: 3,
        deleted_keys: 3,
        total_endpoints: 2,
        deleted_endpoints: 2,
        message: "provider deleted".to_string(),
    });
    let completed_response = local_admin_providers_response(
        &completed_state,
        http::Method::GET,
        "/api/admin/providers/provider-openai/delete-task/task-completed",
        None,
    )
    .await;
    assert_eq!(completed_response.status(), StatusCode::OK);
    let completed_audit = completed_response
        .extensions()
        .get::<AdminAuditEvent>()
        .expect("completed delete task should attach audit");
    assert_eq!(
        completed_audit.event_name,
        "admin_provider_delete_task_completed_viewed"
    );
    assert_eq!(
        completed_audit.action,
        "view_provider_delete_task_terminal_state"
    );
    assert_eq!(completed_audit.target_type, "provider_delete_task");
    assert_eq!(completed_audit.target_id, "provider-openai:task-completed");

    let mut running_state = AppState::new().expect("gateway should build");
    running_state.put_provider_delete_task(crate::LocalProviderDeleteTaskState {
        task_id: "task-running".to_string(),
        provider_id: "provider-openai".to_string(),
        status: "running".to_string(),
        stage: "deleting_keys".to_string(),
        total_keys: 3,
        deleted_keys: 1,
        total_endpoints: 2,
        deleted_endpoints: 0,
        message: "deleted 1 / 3 keys".to_string(),
    });
    let running_response = local_admin_providers_response(
        &running_state,
        http::Method::GET,
        "/api/admin/providers/provider-openai/delete-task/task-running",
        None,
    )
    .await;
    assert_eq!(running_response.status(), StatusCode::OK);
    assert!(
        running_response
            .extensions()
            .get::<AdminAuditEvent>()
            .is_none(),
        "running delete task should not attach audit"
    );
}

#[tokio::test]
async fn gateway_handles_admin_provider_pool_status_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/providers/provider-openai/pool-status",
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
                "lru_enabled": true,
                "cost_window_seconds": 7200,
                "cost_limit_per_key_tokens": 12000
            }
        })),
    );
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        vec![sample_key(
            "key-openai-pool",
            "provider-openai",
            "openai:chat",
            "sk-test",
        )],
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
            "{gateway_url}/api/admin/providers/provider-openai/pool-status"
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
    assert_eq!(payload["provider_id"], "provider-openai");
    assert_eq!(payload["provider_name"], "openai");
    assert_eq!(payload["pool_enabled"], true);
    assert_eq!(payload["total_keys"], 1);
    assert_eq!(payload["total_sticky_sessions"], 0);
    let keys = payload["keys"].as_array().expect("keys should be an array");
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0]["key_id"], "key-openai-pool");
    assert_eq!(keys[0]["key_name"], "default");
    assert_eq!(keys[0]["cost_window_usage"], 0);
    assert_eq!(keys[0]["cost_limit"], 12000);
    assert_eq!(keys[0]["sticky_sessions"], 0);
    assert!(keys[0]["lru_score"].is_null());
    assert!(keys[0]["cooldown_reason"].is_null());
    assert!(keys[0]["cooldown_ttl_seconds"].is_null());
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_clears_admin_provider_pool_cooldown_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/providers/provider-openai/pool/clear-cooldown/key-openai-pool",
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
        Vec::new(),
        vec![sample_key(
            "key-openai-pool",
            "provider-openai",
            "openai:chat",
            "sk-test",
        )],
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
            "{gateway_url}/api/admin/providers/provider-openai/pool/clear-cooldown/key-openai-pool"
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
    assert_eq!(payload["message"], "已清除 Key default 的冷却状态");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_resets_admin_provider_pool_cost_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/providers/provider-openai/pool/reset-cost/key-openai-pool",
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
        Vec::new(),
        vec![sample_key(
            "key-openai-pool",
            "provider-openai",
            "openai:chat",
            "sk-test",
        )],
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
            "{gateway_url}/api/admin/providers/provider-openai/pool/reset-cost/key-openai-pool"
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
    assert_eq!(payload["message"], "已重置 Key default 的成本窗口");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}
