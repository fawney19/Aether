use std::sync::{Arc, Mutex};

use aether_data::repository::global_models::InMemoryGlobalModelReadRepository;
use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
use aether_data_contracts::repository::global_models::{
    AdminProviderModelListQuery, GlobalModelReadRepository,
};
use axum::body::Body;
use axum::routing::any;
use axum::{extract::Request, Router};
use http::StatusCode;
use serde_json::json;

use super::super::super::{
    build_router_with_state, issue_test_admin_access_token, sample_admin_global_model,
    sample_admin_provider_model, sample_endpoint, sample_key, sample_provider, start_server,
    AppState,
};
use crate::constants::{
    GATEWAY_HEADER, TRUSTED_ADMIN_SESSION_ID_HEADER, TRUSTED_ADMIN_USER_ID_HEADER,
    TRUSTED_ADMIN_USER_ROLE_HEADER,
};
use crate::data::GatewayDataState;

const ADMIN_GLOBAL_MODELS_DATA_UNAVAILABLE_DETAIL: &str = "Admin global model data unavailable";
const ADMIN_MODEL_CATALOG_DATA_UNAVAILABLE_DETAIL: &str = "Admin model catalog data unavailable";

#[tokio::test]
async fn gateway_handles_admin_global_models_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/models/global",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let global_model_repository = Arc::new(
        InMemoryGlobalModelReadRepository::seed(Vec::new())
            .with_admin_global_models(vec![
                sample_admin_global_model("global-gpt-5", "gpt-5", "GPT 5"),
                sample_admin_global_model("global-gpt-4.1", "gpt-4.1", "GPT 4.1"),
            ])
            .with_admin_provider_models(vec![
                sample_admin_provider_model(
                    "model-openai-gpt5",
                    "provider-openai",
                    "global-gpt-5",
                    "gpt-5-upstream",
                ),
                sample_admin_provider_model(
                    "model-openai-gpt41",
                    "provider-openai",
                    "global-gpt-4.1",
                    "gpt-4.1-upstream",
                ),
            ]),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::disabled()
                    .with_global_model_repository_for_tests(global_model_repository),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/models/global?skip=0&limit=20"
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
    assert_eq!(payload["total"], 2);
    assert_eq!(
        payload["models"].as_array().expect("models array")[0]["name"],
        "gpt-4.1"
    );
    assert_eq!(payload["models"][0]["provider_count"], 1);
    assert_eq!(payload["models"][0]["active_provider_count"], 1);
    assert_eq!(payload["models"][0]["usage_count"], 0);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_global_models_locally_with_local_503_when_reader_unavailable() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/models/global",
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
    let access_token = issue_test_admin_access_token(&state, "device-admin-models-503").await;
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/models/global?is_active=true&limit=1000"
        ))
        .header("authorization", format!("Bearer {access_token}"))
        .header("x-client-device-id", "device-admin-models-503")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(
        payload["detail"],
        ADMIN_GLOBAL_MODELS_DATA_UNAVAILABLE_DETAIL
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_global_models_locally_with_bearer_admin_session() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/models/global",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let global_model_repository = Arc::new(
        InMemoryGlobalModelReadRepository::seed(Vec::new())
            .with_admin_global_models(vec![
                sample_admin_global_model("global-gpt-5", "gpt-5", "GPT 5"),
                sample_admin_global_model("global-gpt-4.1", "gpt-4.1", "GPT 4.1"),
            ])
            .with_admin_provider_models(vec![
                sample_admin_provider_model(
                    "model-openai-gpt5",
                    "provider-openai",
                    "global-gpt-5",
                    "gpt-5-upstream",
                ),
                sample_admin_provider_model(
                    "model-openai-gpt41",
                    "provider-openai",
                    "global-gpt-4.1",
                    "gpt-4.1-upstream",
                ),
            ]),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(
            GatewayDataState::disabled()
                .with_global_model_repository_for_tests(global_model_repository),
        );
    let access_token = issue_test_admin_access_token(&state, "device-admin-models").await;
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/models/global?is_active=true&limit=1000"
        ))
        .header("authorization", format!("Bearer {access_token}"))
        .header("x-client-device-id", "device-admin-models")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["total"], 2);
    assert_eq!(
        payload["models"].as_array().expect("models array")[0]["name"],
        "gpt-4.1"
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_returns_service_unavailable_for_admin_global_models_without_global_model_reader() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/models/global",
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
            "{gateway_url}/api/admin/models/global?is_active=true&limit=1000"
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
    assert_eq!(
        payload["detail"],
        ADMIN_GLOBAL_MODELS_DATA_UNAVAILABLE_DETAIL
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_global_model_detail_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/models/global/global-gpt-5",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let global_model_repository = Arc::new(
        InMemoryGlobalModelReadRepository::seed(Vec::new())
            .with_admin_global_models(vec![sample_admin_global_model(
                "global-gpt-5",
                "gpt-5",
                "GPT 5",
            )])
            .with_admin_provider_models(vec![sample_admin_provider_model(
                "model-openai-gpt5",
                "provider-openai",
                "global-gpt-5",
                "gpt-5-upstream",
            )]),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::disabled()
                    .with_global_model_repository_for_tests(global_model_repository),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/models/global/global-gpt-5"
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
    assert_eq!(payload["id"], "global-gpt-5");
    assert_eq!(payload["provider_count"], 1);
    assert_eq!(payload["active_provider_count"], 1);
    assert_eq!(payload["usage_count"], 0);
    assert_eq!(payload["total_models"], 1);
    assert_eq!(payload["total_providers"], 1);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_model_catalog_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/models/catalog",
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
            sample_provider("provider-openai", "openai", 10),
            sample_provider("provider-anthropic", "anthropic", 20),
        ],
        Vec::new(),
        Vec::new(),
    ));
    let global_model_repository = Arc::new(
        InMemoryGlobalModelReadRepository::seed(Vec::new())
            .with_admin_global_models(vec![
                sample_admin_global_model("global-gpt-5", "gpt-5", "GPT 5"),
                sample_admin_global_model("global-claude-4", "claude-4", "Claude 4"),
            ])
            .with_admin_provider_models(vec![
                sample_admin_provider_model(
                    "model-openai-gpt5",
                    "provider-openai",
                    "global-gpt-5",
                    "gpt-5-upstream",
                ),
                sample_admin_provider_model(
                    "model-anthropic-claude4",
                    "provider-anthropic",
                    "global-claude-4",
                    "claude-4-upstream",
                ),
            ]),
    );

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
        .get(format!("{gateway_url}/api/admin/models/catalog"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["total"], 2);
    assert_eq!(payload["models"].as_array().expect("models array").len(), 2);
    assert_eq!(payload["models"][0]["global_model_name"], "claude-4");
    assert_eq!(
        payload["models"][0]["providers"][0]["provider_name"],
        "anthropic"
    );
    assert_eq!(
        payload["models"][0]["providers"][0]["target_model"],
        "claude-4-upstream"
    );
    assert_eq!(
        payload["models"][0]["capabilities"]["supports_function_calling"],
        true
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_returns_service_unavailable_for_admin_model_catalog_without_required_readers() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/models/catalog",
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
        .get(format!("{gateway_url}/api/admin/models/catalog"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(
        payload["detail"],
        ADMIN_MODEL_CATALOG_DATA_UNAVAILABLE_DETAIL
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_returns_service_unavailable_for_admin_global_model_create_without_global_model_writer(
) {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/models/global",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let global_model_repository = Arc::new(
        InMemoryGlobalModelReadRepository::seed(Vec::new()).with_admin_global_models(Vec::new()),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::disabled().with_global_model_reader(global_model_repository),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/models/global"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "name": "gpt-5",
            "display_name": "GPT 5"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(
        payload["detail"],
        ADMIN_GLOBAL_MODELS_DATA_UNAVAILABLE_DETAIL
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_model_catalog_locally_with_local_503_when_readers_unavailable() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/models/catalog",
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
        .get(format!("{gateway_url}/api/admin/models/catalog"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(
        payload["detail"],
        ADMIN_MODEL_CATALOG_DATA_UNAVAILABLE_DETAIL
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_global_model_routing_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/models/global/global-gpt-5/routing",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let openai_provider = sample_provider("provider-openai", "openai", 10).with_billing_fields(
        Some("subscription".to_string()),
        Some(120.0),
        Some(48.0),
        None,
        None,
        None,
    );
    let alt_provider = sample_provider("provider-alt", "alt", 20).with_billing_fields(
        Some("quota".to_string()),
        Some(50.0),
        Some(12.0),
        None,
        None,
        None,
    );
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![openai_provider, alt_provider],
        vec![
            sample_endpoint(
                "endpoint-openai-chat",
                "provider-openai",
                "openai:chat",
                "https://api.openai.example",
            ),
            sample_endpoint(
                "endpoint-alt-chat",
                "provider-alt",
                "openai:chat",
                "https://api.alt.example",
            ),
        ],
        {
            let mut primary_key = sample_key(
                "key-openai-routing",
                "provider-openai",
                "openai:chat",
                "sk-openai-routing-1234",
            );
            primary_key.name = "primary".to_string();
            primary_key.internal_priority = 30;
            primary_key.global_priority_by_format = Some(json!({"openai:chat": 3}));
            primary_key.allowed_models = Some(json!(["gpt-5"]));
            primary_key.rpm_limit = None;
            primary_key.learned_rpm_limit = Some(77);
            primary_key.cache_ttl_minutes = 9;
            primary_key.health_by_format = Some(json!({
                "openai:chat": {"health_score": 0.66}
            }));
            primary_key.circuit_breaker_by_format = Some(json!({
                "openai:chat": {"open": true, "next_probe_at": "2026-03-27T15:00:00Z"}
            }));

            let mut mapped_key = sample_key(
                "key-alt-routing",
                "provider-alt",
                "openai:chat",
                "sk-alt-routing-5678",
            );
            mapped_key.name = "mapped".to_string();
            mapped_key.internal_priority = 10;
            mapped_key.global_priority_by_format = Some(json!({"openai:chat": 1}));
            mapped_key.allowed_models = Some(json!(["gpt-5-upstream"]));
            mapped_key.rpm_limit = Some(120);

            vec![primary_key, mapped_key]
        },
    ));
    let global_model_repository = Arc::new(
        InMemoryGlobalModelReadRepository::seed(Vec::new())
            .with_admin_global_models(vec![{
                let mut global_model = sample_admin_global_model("global-gpt-5", "gpt-5", "GPT 5");
                global_model.config =
                    Some(json!({"streaming": true, "vision": false, "model_mappings": ["gpt-5-upstream"]}));
                global_model
            }])
            .with_admin_provider_models(vec![
                sample_admin_provider_model(
                    "model-openai-gpt5",
                    "provider-openai",
                    "global-gpt-5",
                    "gpt-5-upstream",
                ),
                sample_admin_provider_model(
                    "model-alt-gpt5",
                    "provider-alt",
                    "global-gpt-5",
                    "gpt-5-alt",
                ),
            ]),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_reader_for_tests(
                    provider_catalog_repository,
                )
                .with_global_model_repository_for_tests(global_model_repository)
                .with_system_config_values_for_tests(vec![
                    ("scheduling_mode".to_string(), json!("fixed_order")),
                    ("provider_priority_mode".to_string(), json!("global_key")),
                ]),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/models/global/global-gpt-5/routing"
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
    assert_eq!(payload["global_model_id"], "global-gpt-5");
    assert_eq!(payload["global_model_name"], "gpt-5");
    assert_eq!(payload["display_name"], "GPT 5");
    assert_eq!(payload["global_model_mappings"], json!(["gpt-5-upstream"]));
    assert_eq!(payload["scheduling_mode"], "fixed_order");
    assert_eq!(payload["priority_mode"], "global_key");
    assert_eq!(payload["total_providers"], 2);
    assert_eq!(payload["active_providers"], 2);

    let providers = payload["providers"].as_array().expect("providers array");
    assert_eq!(providers.len(), 2);
    assert_eq!(providers[0]["id"], "provider-openai");
    assert_eq!(providers[0]["monthly_quota_usd"], 120.0);
    assert_eq!(
        providers[0]["model_mappings"][0]["name"],
        "gpt-5-upstream-alias"
    );

    let openai_endpoints = providers[0]["endpoints"]
        .as_array()
        .expect("endpoints array");
    assert_eq!(openai_endpoints.len(), 1);
    assert_eq!(openai_endpoints[0]["api_format"], "openai:chat");
    let openai_keys = openai_endpoints[0]["keys"].as_array().expect("keys array");
    assert_eq!(openai_keys.len(), 1);
    assert_eq!(openai_keys[0]["name"], "primary");
    assert_eq!(openai_keys[0]["masked_key"], "sk-opena***1234");
    assert_eq!(openai_keys[0]["is_adaptive"], true);
    assert_eq!(openai_keys[0]["effective_rpm"], 77);
    assert_eq!(openai_keys[0]["allowed_models"], json!(["gpt-5"]));
    assert_eq!(openai_keys[0]["circuit_breaker_open"], true);
    assert_eq!(
        openai_keys[0]["circuit_breaker_formats"],
        json!(["openai:chat"])
    );
    assert_eq!(openai_keys[0]["next_probe_at"], "2026-03-27T15:00:00Z");

    let alt_endpoints = providers[1]["endpoints"]
        .as_array()
        .expect("endpoints array");
    assert_eq!(alt_endpoints.len(), 1);
    let alt_keys = alt_endpoints[0]["keys"].as_array().expect("keys array");
    assert_eq!(alt_keys.len(), 1);
    assert_eq!(alt_keys[0]["allowed_models"], json!(["gpt-5-upstream"]));

    let whitelist = payload["all_keys_whitelist"]
        .as_array()
        .expect("whitelist array");
    assert_eq!(whitelist.len(), 2);
    assert!(whitelist
        .iter()
        .any(|item| item["key_id"] == "key-openai-routing"));
    assert!(whitelist
        .iter()
        .any(|item| item["key_id"] == "key-alt-routing"));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_creates_admin_global_model_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/models/global",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let global_model_repository = Arc::new(InMemoryGlobalModelReadRepository::seed(Vec::new()));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::disabled()
                    .with_global_model_repository_for_tests(global_model_repository.clone()),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/models/global"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "name": "gpt-5-pro",
            "display_name": "GPT 5 Pro",
            "default_price_per_request": 0.2,
            "default_tiered_pricing": {
                "tiers": [{
                    "up_to": null,
                    "input_price_per_1m": 8.0,
                    "output_price_per_1m": 24.0
                }]
            },
            "supported_capabilities": ["streaming", "vision"],
            "config": {"streaming": true}
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::CREATED);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["name"], "gpt-5-pro");
    assert_eq!(payload["display_name"], "GPT 5 Pro");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    let created = global_model_repository
        .get_admin_global_model_by_name("gpt-5-pro")
        .await
        .expect("model lookup should succeed")
        .expect("model should exist");
    assert_eq!(created.display_name, "GPT 5 Pro");

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_updates_and_deletes_admin_global_model_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/models/global/global-gpt-5",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let global_model_repository = Arc::new(
        InMemoryGlobalModelReadRepository::seed(Vec::new()).with_admin_global_models(vec![
            sample_admin_global_model("global-gpt-5", "gpt-5", "GPT 5"),
        ]),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::disabled()
                    .with_global_model_repository_for_tests(global_model_repository.clone()),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let update_response = reqwest::Client::new()
        .patch(format!(
            "{gateway_url}/api/admin/models/global/global-gpt-5"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "display_name": "GPT 5 Updated",
            "is_active": false,
            "config": {"streaming": false}
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(update_response.status(), StatusCode::OK);
    let update_payload: serde_json::Value = update_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(update_payload["display_name"], "GPT 5 Updated");
    assert_eq!(update_payload["is_active"], false);

    let delete_response = reqwest::Client::new()
        .delete(format!(
            "{gateway_url}/api/admin/models/global/global-gpt-5"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(delete_response.status(), StatusCode::NO_CONTENT);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    let deleted = global_model_repository
        .get_admin_global_model_by_id("global-gpt-5")
        .await
        .expect("model lookup should succeed");
    assert!(deleted.is_none());

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_batch_deletes_admin_global_models_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/models/global/batch-delete",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let global_model_repository = Arc::new(
        InMemoryGlobalModelReadRepository::seed(Vec::new()).with_admin_global_models(vec![
            sample_admin_global_model("global-gpt-5", "gpt-5", "GPT 5"),
            sample_admin_global_model("global-gpt-4.1", "gpt-4.1", "GPT 4.1"),
        ]),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::disabled()
                    .with_global_model_repository_for_tests(global_model_repository.clone()),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/models/global/batch-delete"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({"ids": ["global-gpt-5", "missing-global-model"]}))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success_count"], 1);
    assert_eq!(payload["failed"].as_array().expect("failed array").len(), 1);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    let deleted = global_model_repository
        .get_admin_global_model_by_id("global-gpt-5")
        .await
        .expect("model lookup should succeed");
    assert!(deleted.is_none());

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_assigns_admin_global_model_to_providers_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/models/global/global-gpt-5/assign-to-providers",
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
        Vec::new(),
    ));
    let global_model_repository = Arc::new(
        InMemoryGlobalModelReadRepository::seed(Vec::new()).with_admin_global_models(vec![
            sample_admin_global_model("global-gpt-5", "gpt-5", "GPT 5"),
        ]),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_reader_for_tests(
                    provider_catalog_repository,
                )
                .with_global_model_repository_for_tests(global_model_repository.clone()),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/models/global/global-gpt-5/assign-to-providers"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_ids": ["provider-openai"],
            "create_models": true
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["total_success"], 1);
    assert_eq!(payload["total_errors"], 0);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    let created = global_model_repository
        .list_admin_provider_models(&AdminProviderModelListQuery {
            provider_id: "provider-openai".to_string(),
            is_active: None,
            offset: 0,
            limit: 20,
        })
        .await
        .expect("models should read");
    assert_eq!(created.len(), 1);
    assert_eq!(created[0].global_model_id, "global-gpt-5");

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_global_model_providers_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/models/global/global-gpt-5/providers",
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
        Vec::new(),
    ));
    let global_model_repository = Arc::new(
        InMemoryGlobalModelReadRepository::seed(Vec::new())
            .with_admin_global_models(vec![sample_admin_global_model(
                "global-gpt-5",
                "gpt-5",
                "GPT 5",
            )])
            .with_admin_provider_models(vec![sample_admin_provider_model(
                "model-openai-gpt5",
                "provider-openai",
                "global-gpt-5",
                "gpt-5-upstream",
            )]),
    );

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
            "{gateway_url}/api/admin/models/global/global-gpt-5/providers"
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
    assert_eq!(payload["providers"][0]["provider_name"], "openai");
    assert_eq!(payload["providers"][0]["target_model"], "gpt-5-upstream");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}
