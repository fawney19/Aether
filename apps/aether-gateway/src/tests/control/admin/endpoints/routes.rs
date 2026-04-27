use std::sync::{Arc, Mutex};

use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
use aether_data_contracts::repository::provider_catalog::ProviderCatalogReadRepository;
use axum::body::Body;
use axum::routing::any;
use axum::{extract::Request, Router};
use http::StatusCode;
use serde_json::json;

use super::super::super::{
    build_router_with_state, sample_endpoint, sample_key, sample_provider, start_server, AppState,
};
use crate::constants::{
    GATEWAY_HEADER, TRUSTED_ADMIN_SESSION_ID_HEADER, TRUSTED_ADMIN_USER_ID_HEADER,
    TRUSTED_ADMIN_USER_ROLE_HEADER,
};
use crate::data::GatewayDataState;

const ADMIN_ENDPOINTS_DATA_UNAVAILABLE_DETAIL: &str = "Admin endpoint data unavailable";

#[tokio::test]
async fn gateway_returns_service_unavailable_for_admin_provider_endpoints_when_catalog_reader_unavailable(
) {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/endpoints/providers/provider-openai/endpoints",
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
            "{gateway_url}/api/admin/endpoints/providers/provider-openai/endpoints?skip=0&limit=50"
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
    assert_eq!(payload["detail"], ADMIN_ENDPOINTS_DATA_UNAVAILABLE_DETAIL);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_provider_endpoints_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/endpoints/providers/provider-openai/endpoints",
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
            )
            .with_timestamps(Some(1_711_000_000), Some(1_711_000_100)),
            sample_endpoint(
                "endpoint-openai-embed",
                "provider-openai",
                "openai:embedding",
                "https://api.openai.example",
            )
            .with_timestamps(Some(1_710_000_000), Some(1_710_000_100)),
        ],
        vec![
            sample_key(
                "key-openai-a",
                "provider-openai",
                "openai:chat",
                "sk-test-a",
            ),
            sample_key(
                "key-openai-b",
                "provider-openai",
                "openai:chat",
                "sk-test-b",
            ),
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
        .get(format!(
            "{gateway_url}/api/admin/endpoints/providers/provider-openai/endpoints?skip=0&limit=50"
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
    assert_eq!(items[0]["id"], "endpoint-openai-chat");
    assert_eq!(items[0]["provider_name"], "openai");
    assert_eq!(items[0]["api_format"], "openai:chat");
    assert_eq!(items[0]["total_keys"], 2);
    assert_eq!(items[0]["active_keys"], 2);
    assert_eq!(items[1]["id"], "endpoint-openai-embed");
    assert_eq!(items[1]["total_keys"], 0);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_returns_service_unavailable_for_admin_provider_endpoint_create_when_catalog_writer_unavailable(
) {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/endpoints/providers/provider-openai/endpoints",
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
        vec![],
        vec![],
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
            "{gateway_url}/api/admin/endpoints/providers/provider-openai/endpoints"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-openai",
            "api_format": "openai:chat",
            "base_url": "https://api.openai.example/"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["detail"], ADMIN_ENDPOINTS_DATA_UNAVAILABLE_DETAIL);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_creates_admin_provider_endpoint_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/endpoints/providers/provider-openai/endpoints",
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
        vec![],
        vec![],
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
            "{gateway_url}/api/admin/endpoints/providers/provider-openai/endpoints"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-openai",
            "api_format": "openai:chat",
            "base_url": "https://api.openai.example/",
            "custom_path": "/v1/chat/completions",
            "max_retries": 5,
            "config": {"foo": "bar"},
            "proxy": {"url": "http://proxy.internal", "password": "secret"}
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["provider_id"], "provider-openai");
    assert_eq!(payload["provider_name"], "openai");
    assert_eq!(payload["api_format"], "openai:chat");
    assert_eq!(payload["base_url"], "https://api.openai.example");
    assert_eq!(payload["custom_path"], "/v1/chat/completions");
    assert_eq!(payload["max_retries"], 5);
    assert_eq!(payload["total_keys"], 0);
    assert_eq!(payload["active_keys"], 0);
    assert_eq!(payload["proxy"]["url"], "http://proxy.internal");
    assert_eq!(payload["proxy"]["password"], "***");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    let endpoints = provider_catalog_repository
        .list_endpoints_by_provider_ids(&["provider-openai".to_string()])
        .await
        .expect("endpoints should read");
    assert_eq!(endpoints.len(), 1);
    assert_eq!(endpoints[0].provider_id, "provider-openai");
    assert_eq!(endpoints[0].api_format, "openai:chat");
    assert_eq!(endpoints[0].base_url, "https://api.openai.example");
    assert_eq!(endpoints[0].max_retries, Some(5));

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_updates_admin_provider_endpoint_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/endpoints/endpoint-openai-chat",
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
            "https://api.openai.example",
        )
        .with_transport_fields(
            "https://api.openai.example".to_string(),
            None,
            None,
            Some(2),
            Some("/v1/chat/completions".to_string()),
            Some(json!({"foo":"old"})),
            None,
            Some(json!({"url":"http://proxy.internal","password":"secret"})),
        )
        .expect("endpoint transport should build")],
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
        .put(format!(
            "{gateway_url}/api/admin/endpoints/endpoint-openai-chat"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "base_url": "https://updated.openai.example/",
            "custom_path": "/v1/responses",
            "max_retries": 5,
            "is_active": false,
            "config": {"foo": "new"},
            "proxy": {"url": "http://proxy-2.internal"}
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["id"], "endpoint-openai-chat");
    assert_eq!(payload["provider_id"], "provider-openai");
    assert_eq!(payload["api_format"], "openai:chat");
    assert_eq!(payload["base_url"], "https://updated.openai.example");
    assert_eq!(payload["custom_path"], "/v1/responses");
    assert_eq!(payload["max_retries"], 5);
    assert_eq!(payload["is_active"], false);
    assert_eq!(payload["total_keys"], 1);
    assert_eq!(payload["active_keys"], 1);
    assert_eq!(payload["proxy"]["url"], "http://proxy-2.internal");
    assert_eq!(payload["proxy"]["password"], "***");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    let endpoints = provider_catalog_repository
        .list_endpoints_by_ids(&["endpoint-openai-chat".to_string()])
        .await
        .expect("endpoints should read");
    assert_eq!(endpoints.len(), 1);
    assert_eq!(endpoints[0].base_url, "https://updated.openai.example");
    assert_eq!(endpoints[0].custom_path.as_deref(), Some("/v1/responses"));
    assert_eq!(endpoints[0].max_retries, Some(5));
    assert!(!endpoints[0].is_active);
    assert_eq!(endpoints[0].config, Some(json!({"foo":"new"})));
    assert_eq!(
        endpoints[0].proxy,
        Some(json!({"url":"http://proxy-2.internal","password":"secret"}))
    );

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_deletes_admin_provider_endpoint_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/endpoints/endpoint-openai-chat",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let mut key_a = sample_key(
        "key-openai-a",
        "provider-openai",
        "openai:chat",
        "sk-test-a",
    );
    key_a.api_formats = Some(json!(["openai:chat", "openai:embedding"]));

    let mut key_b = sample_key(
        "key-openai-b",
        "provider-openai",
        "openai:chat",
        "sk-test-b",
    );
    key_b.api_formats = Some(json!(["openai:chat"]));

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-openai", "openai", 10)],
        vec![sample_endpoint(
            "endpoint-openai-chat",
            "provider-openai",
            "openai:chat",
            "https://api.openai.example",
        )],
        vec![key_a, key_b],
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
        .delete(format!(
            "{gateway_url}/api/admin/endpoints/endpoint-openai-chat"
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
    assert_eq!(payload["message"], "Endpoint endpoint-openai-chat 已删除");
    assert_eq!(payload["affected_keys_count"], 2);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    let endpoints = provider_catalog_repository
        .list_endpoints_by_ids(&["endpoint-openai-chat".to_string()])
        .await
        .expect("endpoints should read");
    assert!(endpoints.is_empty());

    let keys = provider_catalog_repository
        .list_keys_by_provider_ids(&["provider-openai".to_string()])
        .await
        .expect("keys should read");
    let key_a = keys
        .iter()
        .find(|key| key.id == "key-openai-a")
        .expect("key a should exist");
    let key_b = keys
        .iter()
        .find(|key| key.id == "key-openai-b")
        .expect("key b should exist");
    assert_eq!(key_a.api_formats, Some(json!(["openai:embedding"])));
    assert_eq!(key_b.api_formats, Some(json!([])));

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_get_endpoint_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/endpoints/endpoint-openai-chat",
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
            "https://api.openai.example",
        )
        .with_timestamps(Some(1_711_000_000), Some(1_711_000_100))],
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
            .with_data_state_for_tests(GatewayDataState::with_provider_catalog_reader_for_tests(
                provider_catalog_repository,
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/endpoints/endpoint-openai-chat"
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
    assert_eq!(payload["id"], "endpoint-openai-chat");
    assert_eq!(payload["provider_id"], "provider-openai");
    assert_eq!(payload["provider_name"], "openai");
    assert_eq!(payload["api_format"], "openai:chat");
    assert_eq!(payload["total_keys"], 1);
    assert_eq!(payload["active_keys"], 1);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_default_body_rules_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/endpoints/defaults/openai:responses/body-rules",
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
            "{gateway_url}/api/admin/endpoints/defaults/openai:responses/body-rules?provider_type=codex"
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
    assert_eq!(payload["api_format"], "openai:responses");
    let rules = payload["body_rules"]
        .as_array()
        .expect("body_rules should be an array");
    assert!(rules.is_empty());
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}
