use std::sync::{Arc, Mutex};

use aether_contracts::{
    ExecutionPlan, EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER, EXECUTION_REQUEST_HTTP1_ONLY_HEADER,
};
use aether_crypto::{
    decrypt_python_fernet_ciphertext, encrypt_python_fernet_plaintext, DEVELOPMENT_ENCRYPTION_KEY,
};
use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
use aether_data::repository::proxy_nodes::InMemoryProxyNodeRepository;
use aether_data_contracts::repository::provider_catalog::ProviderCatalogReadRepository;
use aether_testkit::ManagedRedisServer;
use axum::body::to_bytes;
use axum::body::Body;
use axum::routing::{any, get, post};
use axum::{extract::Request, Json, Router};
use base64::Engine as _;
use flate2::{write::GzEncoder, Compression};
use http::StatusCode;
use serde_json::json;
use std::io::Write;

use super::super::{
    build_router_with_state, build_state_with_execution_runtime_override,
    issue_test_admin_access_token, sample_endpoint, sample_provider, sample_proxy_node,
    start_server, wait_until, AppState,
};
use crate::constants::{
    GATEWAY_HEADER, TRUSTED_ADMIN_SESSION_ID_HEADER, TRUSTED_ADMIN_USER_ID_HEADER,
    TRUSTED_ADMIN_USER_ROLE_HEADER,
};
use crate::data::{GatewayDataConfig, GatewayDataState};

async fn start_managed_redis_or_skip() -> Option<ManagedRedisServer> {
    match ManagedRedisServer::start().await {
        Ok(server) => Some(server),
        Err(err) if err.to_string().contains("No such file or directory") => {
            eprintln!("skipping redis-backed provider ops test: {err}");
            None
        }
        Err(err) => panic!("redis server should start: {err}"),
    }
}

#[tokio::test]
async fn gateway_handles_admin_provider_ops_architectures_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/provider-ops/architectures",
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
            "{gateway_url}/api/admin/provider-ops/architectures"
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
    let items = payload.as_array().expect("items should be array");
    assert_eq!(items.len(), 6);
    assert!(items
        .iter()
        .all(|item| item["architecture_id"] != "generic_api"));
    assert_eq!(items[0]["architecture_id"], "anyrouter");
    assert_eq!(items[0]["default_connector"], "cookie");
    assert_eq!(items[3]["architecture_id"], "new_api");
    assert_eq!(items[3]["supported_auth_types"][0]["type"], "api_key");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_provider_ops_architectures_locally_with_bearer_admin_session() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/provider-ops/architectures",
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
    let access_token = issue_test_admin_access_token(&state, "device-admin-provider-ops").await;
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/provider-ops/architectures"
        ))
        .header("authorization", format!("Bearer {access_token}"))
        .header("x-client-device-id", "device-admin-provider-ops")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    let items = payload.as_array().expect("items should be array");
    assert_eq!(items.len(), 6);
    assert!(items
        .iter()
        .all(|item| item["architecture_id"] != "generic_api"));
    assert_eq!(items[0]["architecture_id"], "anyrouter");
    assert_eq!(items[0]["default_connector"], "cookie");
    assert_eq!(items[3]["architecture_id"], "new_api");
    assert_eq!(items[3]["supported_auth_types"][0]["type"], "api_key");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_provider_ops_architecture_detail_locally_with_trusted_admin_principal(
) {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/provider-ops/architectures/generic_api",
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
            "{gateway_url}/api/admin/provider-ops/architectures/generic_api"
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
    assert_eq!(payload["architecture_id"], "generic_api");
    assert_eq!(payload["display_name"], "通用 API");
    assert_eq!(payload["default_connector"], "api_key");
    assert_eq!(payload["supported_actions"][0]["type"], "query_balance");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_provider_ops_status_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/provider-ops/providers/provider-openai/status",
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
            sample_provider("provider-openai", "openai", 10).with_transport_fields(
                true,
                false,
                true,
                None,
                None,
                None,
                None,
                None,
                Some(json!({
                    "provider_ops": {
                        "architecture_id": "anyrouter",
                        "connector": {
                            "auth_type": "cookie",
                            "config": {"cookie_name": "session"},
                        },
                        "actions": {
                            "query_balance": {"enabled": true},
                            "checkin": {"enabled": false},
                            "get_models": {},
                        }
                    }
                })),
            ),
        ],
        vec![],
        vec![],
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
        .get(format!(
            "{gateway_url}/api/admin/provider-ops/providers/provider-openai/status"
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
    assert_eq!(payload["is_configured"], true);
    assert_eq!(payload["architecture_id"], "anyrouter");
    assert_eq!(payload["connection_status"]["status"], "disconnected");
    assert_eq!(payload["connection_status"]["auth_type"], "cookie");
    assert_eq!(
        payload["enabled_actions"],
        json!(["get_models", "query_balance"])
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_provider_ops_config_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/provider-ops/providers/provider-openai/config",
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
            sample_provider("provider-openai", "openai", 10).with_transport_fields(
                true,
                false,
                true,
                None,
                None,
                None,
                None,
                None,
                Some(json!({
                    "provider_ops": {
                        "architecture_id": "cubence",
                        "connector": {
                            "auth_type": "session_login",
                            "config": {"username": "alice"},
                            "credentials": {
                                "refresh_token": encrypt_python_fernet_plaintext(
                                    DEVELOPMENT_ENCRYPTION_KEY,
                                    "refresh-secret-1234",
                                ).expect("refresh token should encrypt"),
                                "password": encrypt_python_fernet_plaintext(
                                    DEVELOPMENT_ENCRYPTION_KEY,
                                    "top-secret-password",
                                ).expect("password should encrypt"),
                            }
                        }
                    }
                })),
            ),
        ],
        vec![sample_endpoint(
            "endpoint-openai-chat",
            "provider-openai",
            "openai:chat",
            "https://api.openai.example",
        )],
        vec![],
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
        .get(format!(
            "{gateway_url}/api/admin/provider-ops/providers/provider-openai/config"
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
    assert_eq!(payload["is_configured"], true);
    assert_eq!(payload["architecture_id"], "cubence");
    assert_eq!(payload["base_url"], "https://api.openai.example");
    assert_eq!(payload["connector"]["auth_type"], "session_login");
    assert_eq!(payload["connector"]["config"]["username"], "alice");
    assert_eq!(
        payload["connector"]["credentials"]["refresh_token"],
        "refr****1234"
    );
    assert_eq!(payload["connector"]["credentials"]["password"], "********");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_saves_admin_provider_ops_config_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/provider-ops/providers/provider-openai/config",
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
            sample_provider("provider-openai", "openai", 10).with_transport_fields(
                true,
                false,
                true,
                None,
                None,
                None,
                None,
                None,
                Some(json!({
                    "feature_flag": true,
                    "provider_ops": {
                        "architecture_id": "cubence",
                        "connector": {
                            "auth_type": "session_login",
                            "config": {"username": "alice"},
                            "credentials": {
                                "refresh_token": encrypt_python_fernet_plaintext(
                                    DEVELOPMENT_ENCRYPTION_KEY,
                                    "refresh-secret-1234",
                                ).expect("refresh token should encrypt"),
                                "_cached_access_token": "cached-token",
                            }
                        }
                    }
                })),
            ),
        ],
        vec![],
        vec![],
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
        .put(format!(
            "{gateway_url}/api/admin/provider-ops/providers/provider-openai/config"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "architecture_id": "anyrouter",
            "base_url": "https://ops.example",
            "connector": {
                "auth_type": "api_key",
                "config": {
                    "tenant": "acme"
                },
                "credentials": {
                    "refresh_token": "************",
                    "api_key": "live-secret-api-key",
                }
            },
            "actions": {
                "query_balance": {
                    "enabled": true,
                    "config": {
                        "currency": "USD"
                    }
                }
            },
            "schedule": {
                "query_balance": "0 0 * * *"
            }
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], true, "payload={payload}");
    assert_eq!(payload["message"], "配置保存成功");

    let stored_provider = provider_catalog_repository
        .list_providers_by_ids(&["provider-openai".to_string()])
        .await
        .expect("providers should list")
        .into_iter()
        .next()
        .expect("stored provider should exist");
    let provider_config = stored_provider
        .config
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .expect("provider config should be object");
    assert_eq!(provider_config.get("feature_flag"), Some(&json!(true)));
    let provider_ops = provider_config
        .get("provider_ops")
        .and_then(serde_json::Value::as_object)
        .expect("provider ops config should exist");
    assert_eq!(
        provider_ops.get("architecture_id"),
        Some(&json!("anyrouter"))
    );
    assert_eq!(
        provider_ops.get("base_url"),
        Some(&json!("https://ops.example"))
    );
    let connector = provider_ops
        .get("connector")
        .and_then(serde_json::Value::as_object)
        .expect("connector should be object");
    assert_eq!(connector.get("auth_type"), Some(&json!("api_key")));
    assert_eq!(connector.get("config"), Some(&json!({"tenant": "acme"})));
    let credentials = connector
        .get("credentials")
        .and_then(serde_json::Value::as_object)
        .expect("credentials should be object");
    assert!(!credentials.contains_key("_cached_access_token"));
    let stored_refresh = credentials
        .get("refresh_token")
        .and_then(serde_json::Value::as_str)
        .expect("refresh token should be string");
    assert_ne!(stored_refresh, "refresh-secret-1234");
    assert_eq!(
        decrypt_python_fernet_ciphertext(DEVELOPMENT_ENCRYPTION_KEY, stored_refresh)
            .expect("refresh token should decrypt"),
        "refresh-secret-1234"
    );
    let stored_api_key = credentials
        .get("api_key")
        .and_then(serde_json::Value::as_str)
        .expect("api key should be string");
    assert_ne!(stored_api_key, "live-secret-api-key");
    assert_eq!(
        decrypt_python_fernet_ciphertext(DEVELOPMENT_ENCRYPTION_KEY, stored_api_key)
            .expect("api key should decrypt"),
        "live-secret-api-key"
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_deletes_admin_provider_ops_config_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/provider-ops/providers/provider-openai/config",
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
            sample_provider("provider-openai", "openai", 10).with_transport_fields(
                true,
                false,
                true,
                None,
                None,
                None,
                None,
                None,
                Some(json!({
                    "feature_flag": true,
                    "provider_ops": {
                        "architecture_id": "anyrouter",
                        "connector": {
                            "auth_type": "api_key",
                            "credentials": {
                                "api_key": encrypt_python_fernet_plaintext(
                                    DEVELOPMENT_ENCRYPTION_KEY,
                                    "live-secret-api-key",
                                ).expect("api key should encrypt"),
                            }
                        }
                    }
                })),
            ),
        ],
        vec![],
        vec![],
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
        .delete(format!(
            "{gateway_url}/api/admin/provider-ops/providers/provider-openai/config"
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
    assert_eq!(payload["success"], true);
    assert_eq!(payload["message"], "配置已删除");

    let stored_provider = provider_catalog_repository
        .list_providers_by_ids(&["provider-openai".to_string()])
        .await
        .expect("providers should list")
        .into_iter()
        .next()
        .expect("stored provider should exist");
    let provider_config = stored_provider
        .config
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .expect("provider config should be object");
    assert_eq!(provider_config.get("feature_flag"), Some(&json!(true)));
    assert!(!provider_config.contains_key("provider_ops"));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_disconnects_admin_provider_ops_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/provider-ops/providers/provider-openai/disconnect",
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
        .post(format!(
            "{gateway_url}/api/admin/provider-ops/providers/provider-openai/disconnect"
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
    assert_eq!(payload["success"], true);
    assert_eq!(payload["message"], "已断开连接");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_verifies_admin_provider_ops_locally_for_generic_api_with_trusted_admin_principal()
{
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/provider-ops/providers/provider-openai/verify",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (
                    StatusCode::OK,
                    Json(json!({ "success": false, "message": "unexpected upstream hit" })),
                )
            }
        }),
    );

    let verify_target = Router::new().route(
        "/api/user/self",
        any(|headers: axum::http::HeaderMap| async move {
            assert_eq!(
                headers
                    .get(axum::http::header::AUTHORIZATION)
                    .and_then(|value| value.to_str().ok()),
                Some("Bearer live-secret-api-key")
            );
            (
                StatusCode::OK,
                Json(json!({
                    "success": true,
                    "data": {
                        "username": "alice",
                        "display_name": "Alice",
                        "quota": 42.5
                    }
                })),
            )
        }),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-openai", "openai", 10)],
        vec![],
        vec![],
    ));

    let (verify_url, verify_handle) = start_server(verify_target).await;
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
        .post(format!(
            "{gateway_url}/api/admin/provider-ops/providers/provider-openai/verify"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "architecture_id": "generic_api",
            "base_url": verify_url,
            "connector": {
                "auth_type": "api_key",
                "config": {
                    "auth_method": "bearer",
                },
                "credentials": {
                    "api_key": "live-secret-api-key",
                }
            },
            "actions": {},
            "schedule": {},
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], true);
    assert_eq!(payload["data"]["username"], "alice");
    assert_eq!(payload["data"]["display_name"], "Alice");
    assert_eq!(payload["data"]["quota"], 42.5);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
    verify_handle.abort();
}

#[tokio::test]
async fn gateway_verifies_admin_provider_ops_locally_for_anyrouter_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/provider-ops/providers/provider-openai/verify",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (
                    StatusCode::OK,
                    Json(json!({
                        "success": false,
                        "message": "fallback probe",
                    })),
                )
            }
        }),
    );

    let verify_target = Router::new()
        .route(
            "/",
            get(|| async move {
                (
                    StatusCode::OK,
                    [(
                        axum::http::header::SET_COOKIE,
                        axum::http::HeaderValue::from_static(
                            "acw_tc=test-acw-tc; Path=/; HttpOnly, cdn_sec_tc=test-cdn-sec; Path=/; HttpOnly",
                        ),
                    )],
                    Body::from(
                        "<html><script>var arg1 = '0123456789abcdef0123456789abcdef01234567';</script></html>",
                    ),
                )
            }),
        )
        .route(
            "/api/user/self",
            get(|headers: axum::http::HeaderMap| async move {
                let cookie = headers
                    .get(axum::http::header::COOKIE)
                    .and_then(|value| value.to_str().ok())
                    .expect("cookie header should exist");
                assert!(!cookie.contains("acw_tc="));
                assert!(!cookie.contains("cdn_sec_tc="));
                assert!(cookie.contains("acw_sc__v2="));
                assert!(cookie.contains(
                    "session=MTIzfGVIaDRlQUpwWkFOcGJuU3F1d0RfVkhsNWVYa0lkWE5sY201aGJXVUdjM1J5YVc1bkRCQUFCV0ZzYVdObHxzaWc"
                ));
                assert_eq!(
                    headers
                        .get("New-Api-User")
                        .and_then(|value| value.to_str().ok()),
                    Some("42")
                );
                (
                    StatusCode::OK,
                    Json(json!({
                        "id": 42,
                        "username": "alice",
                        "display_name": "Alice",
                        "quota": 7.5,
                        "used_quota": 1.25,
                        "request_count": 8
                    })),
                )
            }),
        );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-openai", "openai", 10)],
        vec![],
        vec![],
    ));

    let (verify_url, verify_handle) = start_server(verify_target).await;
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
        .post(format!(
            "{gateway_url}/api/admin/provider-ops/providers/provider-openai/verify"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "architecture_id": "anyrouter",
            "base_url": verify_url,
            "connector": {
                "auth_type": "cookie",
                "config": {},
                "credentials": {
                    "session_cookie": "session=MTIzfGVIaDRlQUpwWkFOcGJuU3F1d0RfVkhsNWVYa0lkWE5sY201aGJXVUdjM1J5YVc1bkRCQUFCV0ZzYVdObHxzaWc",
                }
            },
            "actions": {},
            "schedule": {},
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], true);
    assert_eq!(payload["data"]["username"], "alice");
    assert_eq!(payload["data"]["display_name"], "Alice");
    assert_eq!(payload["data"]["quota"], 7.5);
    assert_eq!(payload["data"]["used_quota"], 1.25);
    assert_eq!(payload["data"]["request_count"], 8);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
    verify_handle.abort();
}

#[tokio::test]
async fn gateway_verifies_admin_provider_ops_locally_for_anyrouter_with_cookie_auth_failure_message(
) {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/provider-ops/providers/provider-openai/verify",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (
                    StatusCode::OK,
                    Json(json!({
                        "success": false,
                        "message": "fallback probe",
                    })),
                )
            }
        }),
    );

    let verify_target = Router::new()
        .route(
            "/",
            get(|| async move {
                (
                    StatusCode::OK,
                    Body::from(
                        "<html><script>var arg1 = '0123456789abcdef0123456789abcdef01234567';</script></html>",
                    ),
                )
            }),
        )
        .route("/api/user/self", get(|| async move { StatusCode::UNAUTHORIZED }));

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-openai", "openai", 10)],
        vec![],
        vec![],
    ));

    let (verify_url, verify_handle) = start_server(verify_target).await;
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
        .post(format!(
            "{gateway_url}/api/admin/provider-ops/providers/provider-openai/verify"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "architecture_id": "anyrouter",
            "base_url": verify_url,
            "connector": {
                "auth_type": "cookie",
                "config": {},
                "credentials": {
                    "session_cookie": "session=MTIzfGVIaDRlQUpwWkFOcGJuU3F1d0RfVkhsNWVYa0lkWE5sY201aGJXVUdjM1J5YVc1bkRCQUFCV0ZzYVdObHxzaWc",
                }
            },
            "actions": {},
            "schedule": {},
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], false);
    assert_eq!(payload["message"], "Cookie 已失效，请重新配置");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
    verify_handle.abort();
}

#[tokio::test]
async fn gateway_verifies_admin_provider_ops_locally_for_anyrouter_when_acw_redirect_body_contains_arg1(
) {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/provider-ops/providers/provider-openai/verify",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (
                    StatusCode::OK,
                    Json(json!({
                        "success": false,
                        "message": "fallback probe",
                    })),
                )
            }
        }),
    );

    let verify_target = Router::new()
        .route(
            "/",
            get(|| async move {
                (
                    StatusCode::FOUND,
                    [(
                        axum::http::header::LOCATION,
                        axum::http::HeaderValue::from_static("/landing"),
                    ),
                    (
                        axum::http::header::SET_COOKIE,
                        axum::http::HeaderValue::from_static(
                            "acw_tc=redirect-acw-tc; Path=/; HttpOnly, cdn_sec_tc=redirect-cdn-sec; Path=/; HttpOnly",
                        ),
                    )],
                    Body::from(
                        "<html><script>var arg1 = '0123456789abcdef0123456789abcdef01234567';</script></html>",
                    ),
                )
            }),
        )
        .route("/landing", get(|| async move { (StatusCode::OK, Body::from("landing")) }))
        .route(
            "/api/user/self",
            get(|headers: axum::http::HeaderMap| async move {
                let cookie = headers
                    .get(axum::http::header::COOKIE)
                    .and_then(|value| value.to_str().ok())
                    .expect("cookie header should exist");
                assert!(!cookie.contains("acw_tc="));
                assert!(!cookie.contains("cdn_sec_tc="));
                assert!(cookie.contains("acw_sc__v2="));
                assert!(cookie.contains(
                    "session=MTIzfGVIaDRlQUpwWkFOcGJuU3F1d0RfVkhsNWVYa0lkWE5sY201aGJXVUdjM1J5YVc1bkRCQUFCV0ZzYVdObHxzaWc"
                ));
                (
                    StatusCode::OK,
                    Json(json!({
                        "id": 42,
                        "username": "alice",
                        "display_name": "Alice",
                        "quota": 7.5
                    })),
                )
            }),
        );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-openai", "openai", 10)],
        vec![],
        vec![],
    ));

    let (verify_url, verify_handle) = start_server(verify_target).await;
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
        .post(format!(
            "{gateway_url}/api/admin/provider-ops/providers/provider-openai/verify"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "architecture_id": "anyrouter",
            "base_url": verify_url,
            "connector": {
                "auth_type": "cookie",
                "config": {},
                "credentials": {
                    "session_cookie": "session=MTIzfGVIaDRlQUpwWkFOcGJuU3F1d0RfVkhsNWVYa0lkWE5sY201aGJXVUdjM1J5YVc1bkRCQUFCV0ZzYVdObHxzaWc",
                }
            },
            "actions": {},
            "schedule": {},
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], true);
    assert_eq!(payload["data"]["username"], "alice");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
    verify_handle.abort();
}

#[tokio::test]
async fn gateway_verifies_admin_provider_ops_locally_for_anyrouter_proxy_mode() {
    let execution_plans = Arc::new(Mutex::new(Vec::<ExecutionPlan>::new()));
    let execution_plans_clone = Arc::clone(&execution_plans);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| {
            let execution_plans_inner = Arc::clone(&execution_plans_clone);
            async move {
                execution_plans_inner
                    .lock()
                    .expect("mutex should lock")
                    .push(plan.clone());
                let proxy = plan.proxy.as_ref().expect("proxy snapshot should exist");
                let proxy_url = proxy
                    .url
                    .as_deref()
                    .expect("manual proxy url should exist");
                let parsed_proxy = url::Url::parse(proxy_url).expect("proxy url should parse");
                assert_eq!(parsed_proxy.scheme(), "http");
                assert_eq!(parsed_proxy.host_str(), Some("proxy.example"));
                assert_eq!(parsed_proxy.port_or_known_default(), Some(8080));
                assert_eq!(parsed_proxy.username(), "alice");
                assert_eq!(parsed_proxy.password(), Some("supersecret"));

                if plan.url.ends_with("/api/user/self") {
                    assert_eq!(
                        plan.headers
                            .get(EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER)
                            .map(String::as_str),
                        None
                    );
                    Json(json!({
                        "request_id": plan.request_id,
                        "status_code": 200,
                        "headers": {
                            "content-type": "application/json"
                        },
                        "body": {
                            "json_body": {
                                "id": 42,
                                "username": "alice",
                                "display_name": "Alice",
                                "quota": 7.5,
                                "used_quota": 1.25,
                                "request_count": 8
                            }
                        }
                    }))
                } else {
                    assert_eq!(plan.request_id, "provider-ops-acw:anyrouter");
                    assert_eq!(plan.url, "https://ops.example");
                    assert_eq!(
                        plan.headers
                            .get(EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER)
                            .map(String::as_str),
                        Some("false")
                    );
                    Json(json!({
                        "request_id": plan.request_id,
                        "status_code": 200,
                        "headers": {
                            "content-type": "text/html"
                        },
                        "body": {
                            "body_bytes_b64": base64::engine::general_purpose::STANDARD.encode(
                                "<html><script>var arg1 = '0123456789abcdef0123456789abcdef01234567';</script></html>"
                            )
                        }
                    }))
                }
            }
        }),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-openai", "openai", 10)],
        vec![],
        vec![],
    ));
    let mut manual_node = sample_proxy_node("proxy-node-manual");
    manual_node.name = "alpha-manual".to_string();
    manual_node.status = "online".to_string();
    manual_node.is_manual = true;
    manual_node.tunnel_mode = false;
    manual_node.tunnel_connected = false;
    manual_node.proxy_url = Some("http://proxy.example:8080".to_string());
    manual_node.proxy_username = Some("alice".to_string());
    manual_node.proxy_password = Some("supersecret".to_string());
    let proxy_node_repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![manual_node]));

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository,
                )
                .attach_proxy_node_repository_for_tests(proxy_node_repository)
                .with_system_config_values_for_tests(vec![(
                    "system_proxy_node_id".to_string(),
                    json!("proxy-node-manual"),
                )]),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-ops/providers/provider-openai/verify"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "architecture_id": "anyrouter",
            "base_url": "https://ops.example",
            "connector": {
                "auth_type": "cookie",
                "config": {},
                "credentials": {
                    "session_cookie": "session=MTIzfGVIaDRlQUpwWkFOcGJuU3F1d0RfVkhsNWVYa0lkWE5sY201aGJXVUdjM1J5YVc1bkRCQUFCV0ZzYVdObHxzaWc",
                }
            },
            "actions": {},
            "schedule": {},
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], true);
    assert_eq!(payload["data"]["username"], "alice");
    assert_eq!(payload["data"]["used_quota"], 1.25);
    assert_eq!(payload["data"]["request_count"], 8);

    let plans = execution_plans.lock().expect("mutex should lock");
    assert_eq!(plans.len(), 2);
    assert_eq!(plans[0].request_id, "provider-ops-acw:anyrouter");
    assert_eq!(plans[0].url, "https://ops.example");
    assert_eq!(plans[1].request_id, "provider-ops-verify:anyrouter");
    assert_eq!(plans[1].url, "https://ops.example/api/user/self");

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[tokio::test]
async fn gateway_verifies_admin_provider_ops_sub2api_proxy_mode_via_execution_runtime_http1_only() {
    let execution_plans = Arc::new(Mutex::new(Vec::<ExecutionPlan>::new()));
    let execution_plans_clone = Arc::clone(&execution_plans);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| {
            let execution_plans_inner = Arc::clone(&execution_plans_clone);
            async move {
                execution_plans_inner
                    .lock()
                    .expect("mutex should lock")
                    .push(plan.clone());
                let proxy = plan.proxy.as_ref().expect("proxy snapshot should exist");
                assert_eq!(proxy.node_id.as_deref(), Some("proxy-node-sub2api"));
                assert_eq!(
                    plan.headers
                        .get(EXECUTION_REQUEST_HTTP1_ONLY_HEADER)
                        .map(String::as_str),
                    Some("true")
                );

                if plan.url.ends_with("/api/v1/auth/refresh") {
                    Json(json!({
                        "request_id": plan.request_id,
                        "status_code": 200,
                        "headers": {
                            "content-type": "application/json"
                        },
                        "body": {
                            "json_body": {
                                "code": 0,
                                "data": {
                                    "access_token": "sub2api-access-token",
                                    "refresh_token": "sub2api-refresh-token-new"
                                }
                            }
                        }
                    }))
                } else {
                    assert_eq!(
                        plan.headers.get("authorization").map(String::as_str),
                        Some("Bearer sub2api-access-token")
                    );
                    Json(json!({
                        "request_id": plan.request_id,
                        "status_code": 200,
                        "headers": {
                            "content-type": "application/json"
                        },
                        "body": {
                            "json_body": {
                                "code": 0,
                                "data": {
                                    "username": "sub2api-user",
                                    "email": "sub2api@example.com",
                                    "balance": 8.5,
                                    "points": 1.5,
                                    "status": "active",
                                    "concurrency": 3
                                }
                            }
                        }
                    }))
                }
            }
        }),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-openai", "openai", 10)],
        vec![],
        vec![],
    ));
    let mut manual_node = sample_proxy_node("proxy-node-sub2api");
    manual_node.name = "sub2api-manual".to_string();
    manual_node.status = "online".to_string();
    manual_node.is_manual = true;
    manual_node.tunnel_mode = false;
    manual_node.tunnel_connected = false;
    manual_node.proxy_url = Some("http://proxy.example:8080".to_string());
    let proxy_node_repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![manual_node]));

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository,
                )
                .attach_proxy_node_repository_for_tests(proxy_node_repository)
                .with_system_config_values_for_tests(vec![(
                    "system_proxy_node_id".to_string(),
                    json!("proxy-node-sub2api"),
                )]),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-ops/providers/provider-openai/verify"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "architecture_id": "sub2api",
            "base_url": "https://sub2api.example",
            "connector": {
                "auth_type": "session_login",
                "config": {},
                "credentials": {
                    "refresh_token": "refresh-token-old",
                }
            },
            "actions": {},
            "schedule": {},
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], true);
    assert_eq!(payload["data"]["username"], "sub2api-user");
    assert_eq!(
        payload["updated_credentials"]["refresh_token"],
        "sub2api-refresh-token-new"
    );

    let plans = execution_plans.lock().expect("mutex should lock");
    assert_eq!(plans.len(), 2);
    assert_eq!(plans[0].url, "https://sub2api.example/api/v1/auth/refresh");
    assert!(
        plans[1]
            .url
            .starts_with("https://sub2api.example/api/v1/auth/me"),
        "url={}",
        plans[1].url
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[tokio::test]
async fn gateway_verifies_admin_provider_ops_locally_for_new_api_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/provider-ops/providers/provider-openai/verify",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (
                    StatusCode::OK,
                    Json(json!({ "success": false, "message": "unexpected upstream hit" })),
                )
            }
        }),
    );

    let verify_target = Router::new().route(
        "/api/user/self",
        any(|headers: axum::http::HeaderMap| async move {
            assert_eq!(
                headers
                    .get(axum::http::header::AUTHORIZATION)
                    .and_then(|value| value.to_str().ok()),
                Some("Bearer live-secret-api-key")
            );
            assert_eq!(
                headers
                    .get("New-Api-User")
                    .and_then(|value| value.to_str().ok()),
                Some("42")
            );
            assert_eq!(
                headers
                    .get(axum::http::header::COOKIE)
                    .and_then(|value| value.to_str().ok()),
                Some("session=foo")
            );
            assert!(headers.contains_key("sec-ch-ua"));
            let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
            encoder
                .write_all(
                    json!({
                        "success": true,
                        "data": {
                            "username": "alice",
                            "display_name": "Alice",
                            "quota": 42.5,
                            "used_quota": 12.5,
                            "request_count": 9,
                            "email": "",
                            "group": "default"
                        }
                    })
                    .to_string()
                    .as_bytes(),
                )
                .expect("gzip body should write");
            let compressed = encoder.finish().expect("gzip body should finish");
            (
                StatusCode::OK,
                [
                    (
                        axum::http::header::CONTENT_TYPE,
                        axum::http::HeaderValue::from_static("application/json"),
                    ),
                    (
                        axum::http::header::CONTENT_ENCODING,
                        axum::http::HeaderValue::from_static("gzip"),
                    ),
                ],
                Body::from(compressed),
            )
        }),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-openai", "openai", 10)],
        vec![],
        vec![],
    ));

    let (verify_url, verify_handle) = start_server(verify_target).await;
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
        .post(format!(
            "{gateway_url}/api/admin/provider-ops/providers/provider-openai/verify"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "architecture_id": "new_api",
            "base_url": verify_url,
            "connector": {
                "auth_type": "api_key",
                "config": {},
                "credentials": {
                    "api_key": "live-secret-api-key",
                    "user_id": "42",
                    "cookie": "session=foo"
                }
            },
            "actions": {},
            "schedule": {},
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], true);
    assert_eq!(payload["message"], serde_json::Value::Null);
    assert_eq!(payload["data"]["username"], "alice");
    assert_eq!(payload["data"]["quota"], 42.5);
    assert_eq!(payload["data"]["used_quota"], 12.5);
    assert_eq!(payload["data"]["request_count"], 9);
    assert_eq!(payload["data"]["email"], "");
    assert_eq!(payload["data"]["extra"]["group"], "default");
    assert_eq!(payload["updated_credentials"], serde_json::Value::Null);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
    verify_handle.abort();
}

#[tokio::test]
async fn gateway_verifies_admin_provider_ops_locally_for_new_api_proxy_node_mode() {
    let execution_plans = Arc::new(Mutex::new(Vec::<ExecutionPlan>::new()));
    let execution_plans_clone = Arc::clone(&execution_plans);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| {
            let execution_plans_inner = Arc::clone(&execution_plans_clone);
            async move {
                execution_plans_inner
                    .lock()
                    .expect("mutex should lock")
                    .push(plan.clone());
                assert_eq!(plan.request_id, "provider-ops-verify:new_api");
                assert_eq!(plan.url, "https://ops.example/api/user/self");
                assert_eq!(
                    plan.headers.get("authorization").map(String::as_str),
                    Some("Bearer live-secret-api-key")
                );
                assert_eq!(
                    plan.headers.get("new-api-user").map(String::as_str),
                    Some("42")
                );
                assert!(!plan.headers.contains_key("accept-encoding"));
                let proxy = plan.proxy.as_ref().expect("proxy snapshot should exist");
                assert_eq!(proxy.node_id.as_deref(), Some("proxy-node-manual"));
                let proxy_url = proxy.url.as_deref().expect("manual proxy url should exist");
                let parsed_proxy = url::Url::parse(proxy_url).expect("proxy url should parse");
                assert_eq!(parsed_proxy.host_str(), Some("proxy.example"));
                assert_eq!(parsed_proxy.username(), "alice");
                assert_eq!(parsed_proxy.password(), Some("supersecret"));
                Json(json!({
                    "request_id": plan.request_id,
                    "status_code": 200,
                    "headers": {
                        "content-type": "application/json"
                    },
                    "body": {
                        "json_body": {
                            "success": true,
                            "data": {
                                "username": "alice",
                                "display_name": "Alice",
                                "quota": 42.5,
                                "used_quota": 12.5,
                                "request_count": 9
                            }
                        }
                    }
                }))
            }
        }),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-openai", "openai", 10)],
        vec![],
        vec![],
    ));
    let mut manual_node = sample_proxy_node("proxy-node-manual");
    manual_node.name = "alpha-manual".to_string();
    manual_node.status = "online".to_string();
    manual_node.is_manual = true;
    manual_node.tunnel_mode = false;
    manual_node.tunnel_connected = false;
    manual_node.proxy_url = Some("http://proxy.example:8080".to_string());
    manual_node.proxy_username = Some("alice".to_string());
    manual_node.proxy_password = Some("supersecret".to_string());
    let proxy_node_repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![manual_node]));

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository,
                )
                .attach_proxy_node_repository_for_tests(proxy_node_repository),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-ops/providers/provider-openai/verify"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "architecture_id": "new_api",
            "base_url": "https://ops.example",
            "connector": {
                "auth_type": "api_key",
                "config": {
                    "proxy_node_id": "proxy-node-manual"
                },
                "credentials": {
                    "api_key": "live-secret-api-key",
                    "user_id": "42",
                    "cookie": "session=foo"
                }
            },
            "actions": {},
            "schedule": {},
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], true);
    assert_eq!(payload["message"], serde_json::Value::Null);
    assert_eq!(payload["data"]["used_quota"], 12.5);
    assert_eq!(payload["data"]["request_count"], 9);
    assert_eq!(payload["data"]["extra"], json!({}));
    assert_eq!(payload["updated_credentials"], serde_json::Value::Null);
    assert_eq!(execution_plans.lock().expect("mutex should lock").len(), 1);

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[tokio::test]
async fn gateway_verifies_admin_provider_ops_locally_for_new_api_without_proxy_via_execution_runtime(
) {
    let execution_plans = Arc::new(Mutex::new(Vec::<ExecutionPlan>::new()));
    let execution_plans_clone = Arc::clone(&execution_plans);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| {
            let execution_plans_inner = Arc::clone(&execution_plans_clone);
            async move {
                execution_plans_inner
                    .lock()
                    .expect("mutex should lock")
                    .push(plan.clone());
                assert_eq!(plan.request_id, "provider-ops-verify:new_api");
                assert_eq!(plan.url, "https://ops.example/api/user/self");
                assert_eq!(
                    plan.headers.get("authorization").map(String::as_str),
                    Some("Bearer live-secret-api-key")
                );
                assert_eq!(
                    plan.headers.get("new-api-user").map(String::as_str),
                    Some("42")
                );
                assert!(plan.proxy.is_none());
                Json(json!({
                    "request_id": plan.request_id,
                    "status_code": 200,
                    "headers": {
                        "content-type": "application/json"
                    },
                    "body": {
                        "json_body": {
                            "success": true,
                            "data": {
                                "username": "alice",
                                "display_name": "Alice",
                                "quota": 42.5,
                                "used_quota": 12.5,
                                "request_count": 9
                            }
                        }
                    }
                }))
            }
        }),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-openai", "openai", 10)],
        vec![],
        vec![],
    ));
    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository,
                ),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-ops/providers/provider-openai/verify"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "architecture_id": "new_api",
            "base_url": "https://ops.example",
            "connector": {
                "auth_type": "api_key",
                "config": {},
                "credentials": {
                    "api_key": "live-secret-api-key",
                    "user_id": "42",
                    "cookie": "session=foo"
                }
            },
            "actions": {},
            "schedule": {},
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], true);
    assert_eq!(payload["data"]["username"], "alice");
    assert_eq!(payload["data"]["used_quota"], 12.5);
    assert_eq!(payload["data"]["request_count"], 9);
    assert_eq!(execution_plans.lock().expect("mutex should lock").len(), 1);

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[tokio::test]
async fn gateway_verifies_admin_provider_ops_locally_for_sub2api_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/provider-ops/providers/provider-openai/verify",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (
                    StatusCode::OK,
                    Json(json!({ "success": false, "message": "unexpected upstream hit" })),
                )
            }
        }),
    );

    let verify_target = Router::new()
        .route(
            "/api/v1/auth/refresh",
            any(|request: Request| async move {
                let body = to_bytes(request.into_body(), usize::MAX)
                    .await
                    .expect("body should read");
                let payload: serde_json::Value =
                    serde_json::from_slice(&body).expect("json body should parse");
                assert_eq!(payload["refresh_token"], "refresh-token-old");
                (
                    StatusCode::OK,
                    Json(json!({
                        "code": 0,
                        "data": {
                            "access_token": "access-token-new",
                            "refresh_token": "refresh-token-new",
                        }
                    })),
                )
            }),
        )
        .route(
            "/api/v1/auth/me",
            get(|headers: axum::http::HeaderMap| async move {
                assert_eq!(
                    headers
                        .get(axum::http::header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok()),
                    Some("Bearer access-token-new")
                );
                (
                    StatusCode::OK,
                    Json(json!({
                        "code": 0,
                        "data": {
                            "username": "sub2api-user",
                            "email": "sub2api@example.com",
                            "balance": 8.5,
                            "points": 1.5,
                            "status": "active",
                            "concurrency": 3,
                        }
                    })),
                )
            }),
        );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-openai", "openai", 10)],
        vec![],
        vec![],
    ));

    let (verify_url, verify_handle) = start_server(verify_target).await;
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
        .post(format!(
            "{gateway_url}/api/admin/provider-ops/providers/provider-openai/verify"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "architecture_id": "sub2api",
            "base_url": verify_url,
            "connector": {
                "auth_type": "session_login",
                "config": {},
                "credentials": {
                    "refresh_token": "refresh-token-old",
                }
            },
            "actions": {},
            "schedule": {},
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], true);
    assert_eq!(payload["data"]["username"], "sub2api-user");
    assert_eq!(payload["data"]["display_name"], "sub2api-user");
    assert_eq!(payload["data"]["email"], "sub2api@example.com");
    assert_eq!(payload["data"]["quota"], 10.0);
    assert_eq!(payload["data"]["extra"]["balance"], 8.5);
    assert_eq!(payload["data"]["extra"]["points"], 1.5);
    assert_eq!(
        payload["updated_credentials"]["refresh_token"],
        "refresh-token-new"
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
    verify_handle.abort();
}

#[tokio::test]
async fn gateway_verifies_admin_provider_ops_sub2api_against_site_root_when_base_url_has_path() {
    let nested_refresh_hits = Arc::new(Mutex::new(0usize));
    let nested_refresh_hits_clone = Arc::clone(&nested_refresh_hits);
    let root_refresh_hits = Arc::new(Mutex::new(0usize));
    let root_refresh_hits_clone = Arc::clone(&root_refresh_hits);
    let verify_target = Router::new()
        .route(
            "/nested/api/v1/auth/refresh",
            post(move |_request: Request| {
                let nested_refresh_hits_inner = Arc::clone(&nested_refresh_hits_clone);
                async move {
                    *nested_refresh_hits_inner.lock().expect("mutex should lock") += 1;
                    (
                        StatusCode::OK,
                        Json(json!({
                            "code": 1,
                            "message": "invalid refresh token",
                        })),
                    )
                }
            }),
        )
        .route(
            "/api/v1/auth/refresh",
            post(move |request: Request| {
                let root_refresh_hits_inner = Arc::clone(&root_refresh_hits_clone);
                async move {
                    *root_refresh_hits_inner.lock().expect("mutex should lock") += 1;
                    let body = to_bytes(request.into_body(), usize::MAX)
                        .await
                        .expect("body should read");
                    let payload: serde_json::Value =
                        serde_json::from_slice(&body).expect("json body should parse");
                    assert_eq!(payload["refresh_token"], "refresh-token-old");
                    (
                        StatusCode::OK,
                        Json(json!({
                            "code": 0,
                            "data": {
                                "access_token": "access-token-new",
                                "refresh_token": "refresh-token-new",
                            }
                        })),
                    )
                }
            }),
        )
        .route(
            "/api/v1/auth/me",
            get(|headers: axum::http::HeaderMap| async move {
                assert_eq!(
                    headers
                        .get(axum::http::header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok()),
                    Some("Bearer access-token-new")
                );
                (
                    StatusCode::OK,
                    Json(json!({
                        "code": 0,
                        "data": {
                            "username": "sub2api-user",
                            "email": "sub2api@example.com",
                            "balance": 8.5,
                            "points": 1.5,
                        }
                    })),
                )
            }),
        );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-openai", "openai", 10)],
        vec![],
        vec![],
    ));

    let (verify_url, verify_handle) = start_server(verify_target).await;
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
        .post(format!(
            "{gateway_url}/api/admin/provider-ops/providers/provider-openai/verify"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "architecture_id": "sub2api",
            "base_url": format!("{verify_url}/nested"),
            "connector": {
                "auth_type": "api_key",
                "config": {},
                "credentials": {
                    "refresh_token": "refresh-token-old",
                }
            },
            "actions": {},
            "schedule": {},
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], true);
    assert_eq!(payload["data"]["username"], "sub2api-user");
    assert_eq!(
        payload["updated_credentials"]["refresh_token"],
        "refresh-token-new"
    );
    assert_eq!(*root_refresh_hits.lock().expect("mutex should lock"), 1);
    assert_eq!(*nested_refresh_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    verify_handle.abort();
}

#[tokio::test]
async fn gateway_verifies_admin_provider_ops_sub2api_with_cached_access_token_without_refresh() {
    let refresh_hits = Arc::new(Mutex::new(0usize));
    let refresh_hits_clone = Arc::clone(&refresh_hits);
    let ops = Router::new()
        .route(
            "/api/v1/auth/refresh",
            post(move |_request: Request| {
                let refresh_hits_inner = Arc::clone(&refresh_hits_clone);
                async move {
                    *refresh_hits_inner.lock().expect("mutex should lock") += 1;
                    (
                        StatusCode::OK,
                        Json(json!({
                            "code": 1,
                            "message": "invalid refresh token",
                        })),
                    )
                }
            }),
        )
        .route(
            "/api/v1/auth/me",
            get(|headers: axum::http::HeaderMap| async move {
                assert_eq!(
                    headers
                        .get(axum::http::header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok()),
                    Some("Bearer cached-access-token")
                );
                (
                    StatusCode::OK,
                    Json(json!({
                        "code": 0,
                        "data": {
                            "username": "sub2api-user",
                            "email": "sub2api@example.com",
                            "balance": 8.5,
                            "points": 1.5,
                        }
                    })),
                )
            }),
        );

    let (ops_url, ops_handle) = start_server(ops).await;
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![
            sample_provider("provider-openai", "openai", 10).with_transport_fields(
                true,
                false,
                true,
                None,
                None,
                None,
                None,
                None,
                Some(json!({
                    "provider_ops": {
                        "architecture_id": "sub2api",
                        "base_url": ops_url,
                        "connector": {
                            "auth_type": "api_key",
                            "config": {},
                            "credentials": {
                                "refresh_token": encrypt_python_fernet_plaintext(
                                    DEVELOPMENT_ENCRYPTION_KEY,
                                    "refresh-token-old",
                                ).expect("refresh token should encrypt"),
                                "_cached_access_token": encrypt_python_fernet_plaintext(
                                    DEVELOPMENT_ENCRYPTION_KEY,
                                    "cached-access-token",
                                ).expect("cached access token should encrypt"),
                                "_cached_token_expires_at": 4102444800.0,
                            }
                        }
                    }
                })),
            ),
        ],
        vec![],
        vec![],
    ));
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
        .post(format!(
            "{gateway_url}/api/admin/provider-ops/providers/provider-openai/verify"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "architecture_id": "sub2api",
            "base_url": ops_url,
            "connector": {
                "auth_type": "api_key",
                "config": {},
                "credentials": {}
            },
            "actions": {},
            "schedule": {},
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], true);
    assert_eq!(payload["data"]["username"], "sub2api-user");
    assert_eq!(payload["updated_credentials"], serde_json::Value::Null);
    assert_eq!(*refresh_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    ops_handle.abort();
}

#[tokio::test]
async fn gateway_verifies_admin_provider_ops_sub2api_persists_rotated_runtime_credentials() {
    let ops = Router::new()
        .route(
            "/api/v1/auth/refresh",
            post(|request: Request| async move {
                let body = to_bytes(request.into_body(), usize::MAX)
                    .await
                    .expect("body should read");
                let payload: serde_json::Value =
                    serde_json::from_slice(&body).expect("json body should parse");
                assert_eq!(payload["refresh_token"], "refresh-token-old");
                (
                    StatusCode::OK,
                    Json(json!({
                        "code": 0,
                        "data": {
                            "access_token": "access-token-new",
                            "refresh_token": "refresh-token-new",
                            "expires_in": 900
                        }
                    })),
                )
            }),
        )
        .route(
            "/api/v1/auth/me",
            get(|headers: axum::http::HeaderMap| async move {
                assert_eq!(
                    headers
                        .get(axum::http::header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok()),
                    Some("Bearer access-token-new")
                );
                (
                    StatusCode::OK,
                    Json(json!({
                        "code": 0,
                        "data": {
                            "username": "sub2api-user",
                            "email": "sub2api@example.com",
                            "balance": 8.5,
                            "points": 1.5,
                        }
                    })),
                )
            }),
        );

    let (ops_url, ops_handle) = start_server(ops).await;
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![
            sample_provider("provider-openai", "openai", 10).with_transport_fields(
                true,
                false,
                true,
                None,
                None,
                None,
                None,
                None,
                Some(json!({
                    "provider_ops": {
                        "architecture_id": "sub2api",
                        "base_url": ops_url,
                        "connector": {
                            "auth_type": "api_key",
                            "config": {},
                            "credentials": {
                                "refresh_token": encrypt_python_fernet_plaintext(
                                    DEVELOPMENT_ENCRYPTION_KEY,
                                    "refresh-token-old",
                                ).expect("refresh token should encrypt"),
                            }
                        }
                    }
                })),
            ),
        ],
        vec![],
        vec![],
    ));
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(Arc::clone(
                    &provider_catalog_repository,
                ))
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-ops/providers/provider-openai/verify"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "architecture_id": "sub2api",
            "base_url": ops_url,
            "connector": {
                "auth_type": "api_key",
                "config": {},
                "credentials": {}
            },
            "actions": {},
            "schedule": {},
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], true);
    assert_eq!(
        payload["updated_credentials"]["refresh_token"],
        "refresh-token-new"
    );

    let stored_provider = provider_catalog_repository
        .list_providers_by_ids(&["provider-openai".to_string()])
        .await
        .expect("providers should list")
        .into_iter()
        .next()
        .expect("stored provider should exist");
    let credentials = stored_provider
        .config
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .and_then(|config| config.get("provider_ops"))
        .and_then(serde_json::Value::as_object)
        .and_then(|config| config.get("connector"))
        .and_then(serde_json::Value::as_object)
        .and_then(|connector| connector.get("credentials"))
        .and_then(serde_json::Value::as_object)
        .expect("credentials should be object");
    let stored_refresh_token = credentials
        .get("refresh_token")
        .and_then(serde_json::Value::as_str)
        .expect("refresh token should be string");
    assert_eq!(
        decrypt_python_fernet_ciphertext(DEVELOPMENT_ENCRYPTION_KEY, stored_refresh_token)
            .expect("refresh token should decrypt"),
        "refresh-token-new"
    );
    let stored_cached_access_token = credentials
        .get("_cached_access_token")
        .and_then(serde_json::Value::as_str)
        .expect("cached access token should be string");
    assert_eq!(
        decrypt_python_fernet_ciphertext(DEVELOPMENT_ENCRYPTION_KEY, stored_cached_access_token,)
            .expect("cached access token should decrypt"),
        "access-token-new"
    );
    assert!(
        credentials
            .get("_cached_token_expires_at")
            .and_then(serde_json::Value::as_f64)
            .expect("cached token expires at should be number")
            > std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time should move forward")
                .as_secs_f64()
    );

    gateway_handle.abort();
    ops_handle.abort();
}

#[tokio::test]
async fn gateway_rejects_admin_provider_ops_connect_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/provider-ops/providers/provider-openai/connect",
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
            sample_provider("provider-openai", "openai", 10).with_transport_fields(
                true,
                false,
                true,
                None,
                None,
                None,
                None,
                None,
                Some(json!({
                    "provider_ops": {
                        "architecture_id": "anyrouter",
                        "base_url": "https://ops.example",
                        "connector": {
                            "auth_type": "api_key",
                            "credentials": {
                                "api_key": encrypt_python_fernet_plaintext(
                                    DEVELOPMENT_ENCRYPTION_KEY,
                                    "live-secret-api-key",
                                ).expect("api key should encrypt"),
                            }
                        }
                    }
                })),
            ),
        ],
        vec![],
        vec![],
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
        .post(format!(
            "{gateway_url}/api/admin/provider-ops/providers/provider-openai/connect"
        ))
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
    assert_eq!(
        payload["detail"],
        "Provider 连接仅支持 Rust execution runtime"
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_provider_ops_balance_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/provider-ops/providers/provider-openai/balance",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let ops = Router::new()
        .route(
            "/api/user/checkin",
            post(|headers: axum::http::HeaderMap| async move {
                assert_eq!(
                    headers
                        .get(axum::http::header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok()),
                    Some("Bearer live-secret-api-key")
                );
                (
                    StatusCode::OK,
                    Json(json!({
                        "success": true,
                        "message": "签到成功",
                    })),
                )
            }),
        )
        .route(
            "/api/user/balance",
            get(|headers: axum::http::HeaderMap| async move {
                assert_eq!(
                    headers
                        .get(axum::http::header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok()),
                    Some("Bearer live-secret-api-key")
                );
                (
                    StatusCode::OK,
                    Json(json!({
                        "success": true,
                        "data": {
                            "quota": 5000000,
                            "used_quota": 1000000
                        }
                    })),
                )
            }),
        );

    let (ops_url, ops_handle) = start_server(ops).await;
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![
            sample_provider("provider-openai", "openai", 10).with_transport_fields(
                true,
                false,
                true,
                None,
                None,
                None,
                None,
                None,
                Some(json!({
                    "provider_ops": {
                        "architecture_id": "generic_api",
                        "base_url": ops_url,
                        "connector": {
                            "auth_type": "api_key",
                            "config": {
                                "auth_method": "bearer"
                            },
                            "credentials": {
                                "api_key": encrypt_python_fernet_plaintext(
                                    DEVELOPMENT_ENCRYPTION_KEY,
                                    "live-secret-api-key",
                                ).expect("api key should encrypt"),
                            }
                        }
                    }
                })),
            ),
        ],
        vec![],
        vec![],
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
        .get(format!(
            "{gateway_url}/api/admin/provider-ops/providers/provider-openai/balance?refresh=false"
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
    assert_eq!(payload["status"], "success");
    assert_eq!(payload["action_type"], "query_balance");
    assert_eq!(payload["data"]["total_available"], 10.0);
    assert_eq!(payload["data"]["total_used"], 2.0);
    assert_eq!(payload["data"]["extra"]["checkin_success"], true);
    assert_eq!(payload["data"]["extra"]["checkin_message"], "签到成功");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
    ops_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_provider_ops_balance_locally_for_generic_api_proxy_node_mode() {
    let execution_plans = Arc::new(Mutex::new(Vec::<ExecutionPlan>::new()));
    let execution_plans_clone = Arc::clone(&execution_plans);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| {
            let execution_plans_inner = Arc::clone(&execution_plans_clone);
            async move {
                execution_plans_inner
                    .lock()
                    .expect("mutex should lock")
                    .push(plan.clone());
                let proxy = plan.proxy.as_ref().expect("proxy snapshot should exist");
                assert_eq!(proxy.node_id.as_deref(), Some("proxy-node-manual"));
                assert_eq!(
                    plan.headers.get("authorization").map(String::as_str),
                    Some("Bearer live-secret-api-key")
                );

                if plan.url.ends_with("/api/user/checkin") {
                    Json(json!({
                        "request_id": plan.request_id,
                        "status_code": 200,
                        "headers": {
                            "content-type": "application/json"
                        },
                        "body": {
                            "json_body": {
                                "success": true,
                                "message": "代理签到成功"
                            }
                        }
                    }))
                } else {
                    assert!(plan.url.ends_with("/api/user/balance"));
                    Json(json!({
                        "request_id": plan.request_id,
                        "status_code": 200,
                        "headers": {
                            "content-type": "application/json"
                        },
                        "body": {
                            "json_body": {
                                "success": true,
                                "data": {
                                    "quota": 2500000,
                                    "used_quota": 500000
                                }
                            }
                        }
                    }))
                }
            }
        }),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![
            sample_provider("provider-openai", "openai", 10).with_transport_fields(
                true,
                false,
                true,
                None,
                None,
                None,
                None,
                None,
                Some(json!({
                    "provider_ops": {
                        "architecture_id": "generic_api",
                        "base_url": "https://ops.example",
                        "connector": {
                            "auth_type": "api_key",
                            "config": {
                                "auth_method": "bearer",
                                "proxy_node_id": "proxy-node-manual"
                            },
                            "credentials": {
                                "api_key": encrypt_python_fernet_plaintext(
                                    DEVELOPMENT_ENCRYPTION_KEY,
                                    "live-secret-api-key",
                                ).expect("api key should encrypt"),
                            }
                        }
                    }
                })),
            ),
        ],
        vec![],
        vec![],
    ));
    let mut manual_node = sample_proxy_node("proxy-node-manual");
    manual_node.name = "alpha-manual".to_string();
    manual_node.status = "online".to_string();
    manual_node.is_manual = true;
    manual_node.tunnel_mode = false;
    manual_node.tunnel_connected = false;
    manual_node.proxy_url = Some("http://proxy.example:8080".to_string());
    manual_node.proxy_username = Some("alice".to_string());
    manual_node.proxy_password = Some("supersecret".to_string());
    let proxy_node_repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![manual_node]));

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository,
                )
                .attach_proxy_node_repository_for_tests(proxy_node_repository),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/provider-ops/providers/provider-openai/balance?refresh=false"
        ))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["status"], "success");
    assert_eq!(payload["data"]["total_available"], 5.0);
    assert_eq!(payload["data"]["total_used"], 1.0);
    assert_eq!(payload["data"]["extra"]["checkin_success"], true);
    assert_eq!(payload["data"]["extra"]["checkin_message"], "代理签到成功");

    let plans = execution_plans.lock().expect("mutex should lock");
    assert_eq!(plans.len(), 2);
    assert!(plans.iter().any(|plan| {
        plan.request_id == "provider-ops-action:probe_checkin"
            && plan.url == "https://ops.example/api/user/checkin"
    }));
    assert!(plans.iter().any(|plan| {
        plan.request_id == "provider-ops-action:generic_api:query_balance:provider-openai"
            && plan.url == "https://ops.example/api/user/balance"
    }));

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_provider_ops_balance_locally_without_proxy_via_execution_runtime() {
    let execution_plans = Arc::new(Mutex::new(Vec::<ExecutionPlan>::new()));
    let execution_plans_clone = Arc::clone(&execution_plans);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| {
            let execution_plans_inner = Arc::clone(&execution_plans_clone);
            async move {
                execution_plans_inner
                    .lock()
                    .expect("mutex should lock")
                    .push(plan.clone());
                assert!(plan.proxy.is_none());
                assert_eq!(
                    plan.headers.get("authorization").map(String::as_str),
                    Some("Bearer live-secret-api-key")
                );

                if plan.url.ends_with("/api/user/checkin") {
                    Json(json!({
                        "request_id": plan.request_id,
                        "status_code": 200,
                        "headers": {
                            "content-type": "application/json"
                        },
                        "body": {
                            "json_body": {
                                "success": true,
                                "message": "执行层签到成功"
                            }
                        }
                    }))
                } else {
                    assert!(plan.url.ends_with("/api/user/balance"));
                    Json(json!({
                        "request_id": plan.request_id,
                        "status_code": 200,
                        "headers": {
                            "content-type": "application/json"
                        },
                        "body": {
                            "json_body": {
                                "success": true,
                                "data": {
                                    "quota": 2500000,
                                    "used_quota": 500000
                                }
                            }
                        }
                    }))
                }
            }
        }),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![
            sample_provider("provider-openai", "openai", 10).with_transport_fields(
                true,
                false,
                true,
                None,
                None,
                None,
                None,
                None,
                Some(json!({
                    "provider_ops": {
                        "architecture_id": "generic_api",
                        "base_url": "https://ops.example",
                        "connector": {
                            "auth_type": "api_key",
                            "config": {
                                "auth_method": "bearer"
                            },
                            "credentials": {
                                "api_key": encrypt_python_fernet_plaintext(
                                    DEVELOPMENT_ENCRYPTION_KEY,
                                    "live-secret-api-key",
                                ).expect("api key should encrypt"),
                            }
                        }
                    }
                })),
            ),
        ],
        vec![],
        vec![],
    ));

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository,
                ),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/provider-ops/providers/provider-openai/balance?refresh=false"
        ))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["status"], "success");
    assert_eq!(payload["data"]["total_available"], 5.0);
    assert_eq!(payload["data"]["total_used"], 1.0);
    assert_eq!(payload["data"]["extra"]["checkin_success"], true);
    assert_eq!(
        payload["data"]["extra"]["checkin_message"],
        "执行层签到成功"
    );

    let plans = execution_plans.lock().expect("mutex should lock");
    assert_eq!(plans.len(), 2);
    assert!(plans.iter().all(|plan| plan.proxy.is_none()));
    assert!(plans
        .iter()
        .any(|plan| plan.request_id == "provider-ops-action:probe_checkin"));
    assert!(plans.iter().any(|plan| {
        plan.request_id == "provider-ops-action:generic_api:query_balance:provider-openai"
    }));

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_provider_ops_checkin_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/provider-ops/providers/provider-openai/checkin",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let ops = Router::new().route(
        "/api/user/checkin",
        post(|headers: axum::http::HeaderMap| async move {
            assert_eq!(
                headers
                    .get(axum::http::header::AUTHORIZATION)
                    .and_then(|value| value.to_str().ok()),
                Some("Bearer live-secret-api-key")
            );
            (
                StatusCode::OK,
                Json(json!({
                    "success": true,
                    "message": "今日签到完成",
                    "data": {
                        "reward": 1.5,
                        "streak_days": 3,
                    }
                })),
            )
        }),
    );

    let (ops_url, ops_handle) = start_server(ops).await;
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![
            sample_provider("provider-openai", "openai", 10).with_transport_fields(
                true,
                false,
                true,
                None,
                None,
                None,
                None,
                None,
                Some(json!({
                    "provider_ops": {
                        "architecture_id": "generic_api",
                        "base_url": ops_url,
                        "connector": {
                            "auth_type": "api_key",
                            "config": {
                                "auth_method": "bearer"
                            },
                            "credentials": {
                                "api_key": encrypt_python_fernet_plaintext(
                                    DEVELOPMENT_ENCRYPTION_KEY,
                                    "live-secret-api-key",
                                ).expect("api key should encrypt"),
                            }
                        }
                    }
                })),
            ),
        ],
        vec![],
        vec![],
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
        .post(format!(
            "{gateway_url}/api/admin/provider-ops/providers/provider-openai/checkin"
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
    assert_eq!(payload["status"], "success");
    assert_eq!(payload["action_type"], "checkin");
    assert_eq!(payload["data"]["reward"], 1.5);
    assert_eq!(payload["data"]["streak_days"], 3);
    assert_eq!(payload["data"]["message"], "今日签到完成");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
    ops_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_provider_ops_checkin_locally_for_generic_api_proxy_node_mode() {
    let execution_plans = Arc::new(Mutex::new(Vec::<ExecutionPlan>::new()));
    let execution_plans_clone = Arc::clone(&execution_plans);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| {
            let execution_plans_inner = Arc::clone(&execution_plans_clone);
            async move {
                execution_plans_inner
                    .lock()
                    .expect("mutex should lock")
                    .push(plan.clone());
                let proxy = plan.proxy.as_ref().expect("proxy snapshot should exist");
                assert_eq!(proxy.node_id.as_deref(), Some("proxy-node-manual"));
                assert_eq!(plan.request_id, "provider-ops-action:generic_api:checkin");
                assert_eq!(plan.url, "https://ops.example/api/user/checkin");
                assert_eq!(
                    plan.headers.get("authorization").map(String::as_str),
                    Some("Bearer live-secret-api-key")
                );
                Json(json!({
                    "request_id": plan.request_id,
                    "status_code": 200,
                    "headers": {
                        "content-type": "application/json"
                    },
                    "body": {
                        "json_body": {
                            "success": true,
                            "message": "今日签到完成",
                            "data": {
                                "reward": 1.5,
                                "streak_days": 3
                            }
                        }
                    }
                }))
            }
        }),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![
            sample_provider("provider-openai", "openai", 10).with_transport_fields(
                true,
                false,
                true,
                None,
                None,
                None,
                None,
                None,
                Some(json!({
                    "provider_ops": {
                        "architecture_id": "generic_api",
                        "base_url": "https://ops.example",
                        "connector": {
                            "auth_type": "api_key",
                            "config": {
                                "auth_method": "bearer",
                                "proxy_node_id": "proxy-node-manual"
                            },
                            "credentials": {
                                "api_key": encrypt_python_fernet_plaintext(
                                    DEVELOPMENT_ENCRYPTION_KEY,
                                    "live-secret-api-key",
                                ).expect("api key should encrypt"),
                            }
                        }
                    }
                })),
            ),
        ],
        vec![],
        vec![],
    ));
    let mut manual_node = sample_proxy_node("proxy-node-manual");
    manual_node.name = "alpha-manual".to_string();
    manual_node.status = "online".to_string();
    manual_node.is_manual = true;
    manual_node.tunnel_mode = false;
    manual_node.tunnel_connected = false;
    manual_node.proxy_url = Some("http://proxy.example:8080".to_string());
    manual_node.proxy_username = Some("alice".to_string());
    manual_node.proxy_password = Some("supersecret".to_string());
    let proxy_node_repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![manual_node]));

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository,
                )
                .attach_proxy_node_repository_for_tests(proxy_node_repository),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-ops/providers/provider-openai/checkin"
        ))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["status"], "success");
    assert_eq!(payload["action_type"], "checkin");
    assert_eq!(payload["data"]["reward"], 1.5);
    assert_eq!(payload["data"]["streak_days"], 3);
    assert_eq!(payload["data"]["message"], "今日签到完成");
    assert_eq!(execution_plans.lock().expect("mutex should lock").len(), 1);

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_provider_ops_batch_balance_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/provider-ops/batch/balance",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let ops = Router::new()
        .route(
            "/",
            get(|| async move {
                (
                    StatusCode::OK,
                    [(
                        axum::http::header::SET_COOKIE,
                        axum::http::HeaderValue::from_static(
                            "acw_tc=batch-acw-tc; Path=/; HttpOnly, cdn_sec_tc=batch-cdn-sec; Path=/; HttpOnly",
                        ),
                    )],
                    Body::from(
                        "<html><script>var arg1 = '0123456789abcdef0123456789abcdef01234567';</script></html>",
                    ),
                )
            }),
        )
        .route(
            "/api/user/checkin",
            post(|| async move {
                (
                    StatusCode::OK,
                    Json(json!({
                        "success": true,
                        "message": "签到成功",
                    })),
                )
            }),
        )
        .route(
            "/api/user/sign_in",
            post(|headers: axum::http::HeaderMap| async move {
                let cookie = headers
                    .get(axum::http::header::COOKIE)
                    .and_then(|value| value.to_str().ok())
                    .expect("cookie header should exist");
                assert!(!cookie.contains("acw_tc="));
                assert!(!cookie.contains("cdn_sec_tc="));
                assert!(cookie.contains("acw_sc__v2="));
                assert!(cookie.contains(
                    "session=MTIzfGVIaDRlQUpwWkFOcGJuU3F1d0RfVkhsNWVYa0lkWE5sY201aGJXVUdjM1J5YVc1bkRCQUFCV0ZzYVdObHxzaWc"
                ));
                assert_eq!(
                    headers
                        .get("New-Api-User")
                        .and_then(|value| value.to_str().ok()),
                    Some("42")
                );
                (
                    StatusCode::OK,
                    Json(json!({
                        "success": true,
                        "message": "Anyrouter 签到成功",
                    })),
                )
            }),
        )
        .route(
            "/api/user/balance",
            get(|| async move {
                (
                    StatusCode::OK,
                    Json(json!({
                        "success": true,
                        "data": {
                            "quota": 2500000,
                            "used_quota": 500000
                        }
                    })),
                )
            }),
        );
    let ops = ops.route(
        "/api/user/self",
        get(|headers: axum::http::HeaderMap| async move {
            let cookie = headers
                .get(axum::http::header::COOKIE)
                .and_then(|value| value.to_str().ok())
                .expect("cookie header should exist");
            assert!(!cookie.contains("acw_tc="));
            assert!(!cookie.contains("cdn_sec_tc="));
            assert!(cookie.contains("acw_sc__v2="));
            assert!(cookie.contains(
                "session=MTIzfGVIaDRlQUpwWkFOcGJuU3F1d0RfVkhsNWVYa0lkWE5sY201aGJXVUdjM1J5YVc1bkRCQUFCV0ZzYVdObHxzaWc"
            ));
            assert_eq!(
                headers
                    .get("New-Api-User")
                    .and_then(|value| value.to_str().ok()),
                Some("42")
            );
            (
                StatusCode::OK,
                Json(json!({
                    "success": true,
                    "data": {
                        "quota": 3500000,
                        "used_quota": 500000
                    }
                })),
            )
        }),
    );

    let (ops_url, ops_handle) = start_server(ops).await;
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![
            sample_provider("provider-openai", "openai", 10).with_transport_fields(
                true,
                false,
                true,
                None,
                None,
                None,
                None,
                None,
                Some(json!({
                    "provider_ops": {
                        "architecture_id": "generic_api",
                        "base_url": ops_url,
                        "connector": {
                            "auth_type": "api_key",
                            "config": {
                                "auth_method": "bearer"
                            },
                            "credentials": {
                                "api_key": encrypt_python_fernet_plaintext(
                                    DEVELOPMENT_ENCRYPTION_KEY,
                                    "live-secret-api-key",
                                ).expect("api key should encrypt"),
                            }
                        }
                    }
                })),
            ),
            sample_provider("provider-anyrouter", "openai", 20).with_transport_fields(
                true,
                false,
                true,
                None,
                None,
                None,
                None,
                None,
                Some(json!({
                    "provider_ops": {
                        "architecture_id": "anyrouter",
                        "base_url": ops_url,
                        "connector": {
                            "auth_type": "cookie",
                            "config": {},
                            "credentials": {
                                "session_cookie": encrypt_python_fernet_plaintext(
                                    DEVELOPMENT_ENCRYPTION_KEY,
                                    "session=MTIzfGVIaDRlQUpwWkFOcGJuU3F1d0RfVkhsNWVYa0lkWE5sY201aGJXVUdjM1J5YVc1bkRCQUFCV0ZzYVdObHxzaWc",
                                ).expect("session cookie should encrypt"),
                            }
                        }
                    }
                })),
            ),
        ],
        vec![],
        vec![],
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
        .post(format!(
            "{gateway_url}/api/admin/provider-ops/batch/balance"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!([
            "provider-openai",
            "provider-anyrouter",
            "provider-missing"
        ]))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["provider-openai"]["status"], "success");
    assert_eq!(payload["provider-openai"]["data"]["total_available"], 5.0);
    assert_eq!(payload["provider-anyrouter"]["status"], "success");
    assert_eq!(
        payload["provider-anyrouter"]["data"]["total_available"],
        7.0
    );
    assert_eq!(
        payload["provider-anyrouter"]["data"]["extra"]["checkin_success"],
        true
    );
    assert_eq!(
        payload["provider-anyrouter"]["data"]["extra"]["checkin_message"],
        "Anyrouter 签到成功"
    );
    assert_eq!(payload["provider-missing"]["status"], "not_configured");
    assert_eq!(payload["provider-missing"]["message"], "未配置操作设置");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
    ops_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_provider_ops_anyrouter_balance_with_auth_expired_cookie() {
    let ops = Router::new()
        .route(
            "/",
            get(|| async move {
                (
                    StatusCode::OK,
                    [(
                        axum::http::header::SET_COOKIE,
                        axum::http::HeaderValue::from_static(
                            "acw_tc=expired-acw-tc; Path=/; HttpOnly, cdn_sec_tc=expired-cdn-sec; Path=/; HttpOnly",
                        ),
                    )],
                    Body::from(
                        "<html><script>var arg1 = '0123456789abcdef0123456789abcdef01234567';</script></html>",
                    ),
                )
            }),
        )
        .route(
            "/api/user/sign_in",
            post(|| async move {
                (
                    StatusCode::UNAUTHORIZED,
                    Json(json!({
                        "success": false,
                        "message": "Cookie 已失效",
                    })),
                )
            }),
        )
        .route(
            "/api/user/self",
            get(|headers: axum::http::HeaderMap| async move {
                let cookie = headers
                    .get(axum::http::header::COOKIE)
                    .and_then(|value| value.to_str().ok())
                    .expect("cookie header should exist");
                assert!(!cookie.contains("acw_tc="));
                assert!(!cookie.contains("cdn_sec_tc="));
                assert!(cookie.contains("acw_sc__v2="));
                assert!(cookie.contains(
                    "session=MTIzfGVIaDRlQUpwWkFOcGJuU3F1d0RfVkhsNWVYa0lkWE5sY201aGJXVUdjM1J5YVc1bkRCQUFCV0ZzYVdObHxzaWc"
                ));
                assert_eq!(
                    headers
                        .get("New-Api-User")
                        .and_then(|value| value.to_str().ok()),
                    Some("42")
                );
                (
                    StatusCode::OK,
                    Json(json!({
                        "quota": 3500000,
                        "used_quota": 500000
                    })),
                )
            }),
        );

    let (ops_url, ops_handle) = start_server(ops).await;
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![
            sample_provider("provider-anyrouter", "openai", 20).with_transport_fields(
                true,
                false,
                true,
                None,
                None,
                None,
                None,
                None,
                Some(json!({
                    "provider_ops": {
                        "architecture_id": "anyrouter",
                        "base_url": ops_url,
                        "connector": {
                            "auth_type": "cookie",
                            "config": {},
                            "credentials": {
                                "session_cookie": encrypt_python_fernet_plaintext(
                                    DEVELOPMENT_ENCRYPTION_KEY,
                                    "session=MTIzfGVIaDRlQUpwWkFOcGJuU3F1d0RfVkhsNWVYa0lkWE5sY201aGJXVUdjM1J5YVc1bkRCQUFCV0ZzYVdObHxzaWc",
                                ).expect("session cookie should encrypt"),
                            }
                        }
                    }
                })),
            ),
        ],
        vec![],
        vec![],
    ));
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
        .post(format!(
            "{gateway_url}/api/admin/provider-ops/providers/provider-anyrouter/balance"
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
    assert_eq!(payload["status"], "auth_expired");
    assert_eq!(payload["data"]["total_available"], 7.0);
    assert_eq!(payload["data"]["total_used"], 1.0);
    assert_eq!(payload["data"]["extra"]["cookie_expired"], true);
    assert_eq!(
        payload["data"]["extra"]["cookie_expired_message"],
        "Cookie 已失效"
    );

    gateway_handle.abort();
    ops_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_provider_ops_balance_cache_refresh_modes_with_redis() {
    let Some(redis) = start_managed_redis_or_skip().await else {
        return;
    };
    let balance_hits = Arc::new(Mutex::new(0usize));
    let balance_hits_clone = Arc::clone(&balance_hits);
    let ops = Router::new().route(
        "/api/user/balance",
        get(move |headers: axum::http::HeaderMap| {
            let balance_hits_inner = Arc::clone(&balance_hits_clone);
            async move {
                *balance_hits_inner.lock().expect("mutex should lock") += 1;
                assert_eq!(
                    headers
                        .get(axum::http::header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok()),
                    Some("Bearer live-secret-api-key")
                );
                (
                    StatusCode::OK,
                    Json(json!({
                        "success": true,
                        "data": {
                            "quota": 2000000,
                            "used_quota": 500000
                        }
                    })),
                )
            }
        }),
    );

    let (ops_url, ops_handle) = start_server(ops).await;
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![
            sample_provider("provider-openai", "openai", 10).with_transport_fields(
                true,
                false,
                true,
                None,
                None,
                None,
                None,
                None,
                Some(json!({
                    "provider_ops": {
                        "architecture_id": "generic_api",
                        "base_url": ops_url,
                        "connector": {
                            "auth_type": "api_key",
                            "config": {
                                "auth_method": "bearer"
                            },
                            "credentials": {
                                "api_key": encrypt_python_fernet_plaintext(
                                    DEVELOPMENT_ENCRYPTION_KEY,
                                    "live-secret-api-key",
                                ).expect("api key should encrypt"),
                            }
                        }
                    }
                })),
            ),
        ],
        vec![],
        vec![],
    ));
    let data_state = GatewayDataState::from_config(
        GatewayDataConfig::disabled()
            .with_redis_url(redis.redis_url(), Some("provider_ops_balance_cache"))
            .with_encryption_key(DEVELOPMENT_ENCRYPTION_KEY),
    )
    .expect("data state should build")
    .attach_provider_catalog_repository_for_tests(Arc::clone(&provider_catalog_repository));
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();

    let pending_response = client
        .get(format!(
            "{gateway_url}/api/admin/provider-ops/providers/provider-openai/balance?refresh=true"
        ))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(pending_response.status(), StatusCode::OK);
    let pending_payload: serde_json::Value = pending_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(pending_payload["status"], "pending");

    wait_until(5000, || {
        *balance_hits.lock().expect("mutex should lock") == 1
    })
    .await;

    let cached_response = client
        .get(format!(
            "{gateway_url}/api/admin/provider-ops/providers/provider-openai/balance?refresh=false"
        ))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(cached_response.status(), StatusCode::OK);
    let cached_payload: serde_json::Value = cached_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(cached_payload["status"], "success");
    assert_eq!(cached_payload["data"]["total_available"], 4.0);
    assert_eq!(cached_payload["data"]["total_used"], 1.0);
    assert_eq!(*balance_hits.lock().expect("mutex should lock"), 1);

    let refresh_response = client
        .get(format!(
            "{gateway_url}/api/admin/provider-ops/providers/provider-openai/balance?refresh=true"
        ))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(refresh_response.status(), StatusCode::OK);
    let refresh_payload: serde_json::Value = refresh_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(refresh_payload["status"], "success");
    wait_until(5000, || {
        *balance_hits.lock().expect("mutex should lock") == 2
    })
    .await;

    gateway_handle.abort();
    ops_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_provider_ops_balance_cache_miss_without_refresh_returns_live_payload_once_with_redis(
) {
    let Some(redis) = start_managed_redis_or_skip().await else {
        return;
    };
    let balance_hits = Arc::new(Mutex::new(0usize));
    let balance_hits_clone = Arc::clone(&balance_hits);
    let ops = Router::new().route(
        "/api/user/balance",
        get(move |headers: axum::http::HeaderMap| {
            let balance_hits_inner = Arc::clone(&balance_hits_clone);
            async move {
                *balance_hits_inner.lock().expect("mutex should lock") += 1;
                assert_eq!(
                    headers
                        .get(axum::http::header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok()),
                    Some("Bearer live-secret-api-key")
                );
                (
                    StatusCode::OK,
                    Json(json!({
                        "success": true,
                        "data": {
                            "quota": 2000000,
                            "used_quota": 500000
                        }
                    })),
                )
            }
        }),
    );

    let (ops_url, ops_handle) = start_server(ops).await;
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![
            sample_provider("provider-openai", "openai", 10).with_transport_fields(
                true,
                false,
                true,
                None,
                None,
                None,
                None,
                None,
                Some(json!({
                    "provider_ops": {
                        "architecture_id": "generic_api",
                        "base_url": ops_url,
                        "connector": {
                            "auth_type": "api_key",
                            "config": {
                                "auth_method": "bearer"
                            },
                            "credentials": {
                                "api_key": encrypt_python_fernet_plaintext(
                                    DEVELOPMENT_ENCRYPTION_KEY,
                                    "live-secret-api-key",
                                ).expect("api key should encrypt"),
                            }
                        }
                    }
                })),
            ),
        ],
        vec![],
        vec![],
    ));
    let data_state = GatewayDataState::from_config(
        GatewayDataConfig::disabled()
            .with_redis_url(
                redis.redis_url(),
                Some("provider_ops_balance_cache_sync_miss"),
            )
            .with_encryption_key(DEVELOPMENT_ENCRYPTION_KEY),
    )
    .expect("data state should build")
    .attach_provider_catalog_repository_for_tests(Arc::clone(&provider_catalog_repository));
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();

    let live_response = client
        .get(format!(
            "{gateway_url}/api/admin/provider-ops/providers/provider-openai/balance?refresh=false"
        ))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(live_response.status(), StatusCode::OK);
    let live_payload: serde_json::Value =
        live_response.json().await.expect("json body should parse");
    assert_eq!(live_payload["status"], "success");
    assert_eq!(live_payload["data"]["total_available"], 4.0);
    assert_eq!(live_payload["data"]["total_used"], 1.0);
    assert_eq!(*balance_hits.lock().expect("mutex should lock"), 1);

    let cached_response = client
        .get(format!(
            "{gateway_url}/api/admin/provider-ops/providers/provider-openai/balance?refresh=false"
        ))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(cached_response.status(), StatusCode::OK);
    let cached_payload: serde_json::Value = cached_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(cached_payload["status"], "success");
    assert_eq!(cached_payload["data"]["total_available"], 4.0);
    assert_eq!(cached_payload["data"]["total_used"], 1.0);
    assert_eq!(*balance_hits.lock().expect("mutex should lock"), 1);

    gateway_handle.abort();
    ops_handle.abort();
}

#[tokio::test]
async fn gateway_clears_admin_provider_ops_balance_cache_after_config_save_with_redis() {
    let Some(redis) = start_managed_redis_or_skip().await else {
        return;
    };
    let balance_hits_v1 = Arc::new(Mutex::new(0usize));
    let balance_hits_v1_clone = Arc::clone(&balance_hits_v1);
    let ops_v1 = Router::new().route(
        "/api/user/balance",
        get(move |headers: axum::http::HeaderMap| {
            let balance_hits_inner = Arc::clone(&balance_hits_v1_clone);
            async move {
                *balance_hits_inner.lock().expect("mutex should lock") += 1;
                assert_eq!(
                    headers
                        .get(axum::http::header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok()),
                    Some("Bearer live-secret-api-key")
                );
                (
                    StatusCode::OK,
                    Json(json!({
                        "success": true,
                        "data": {
                            "quota": 2000000,
                            "used_quota": 500000
                        }
                    })),
                )
            }
        }),
    );
    let balance_hits_v2 = Arc::new(Mutex::new(0usize));
    let balance_hits_v2_clone = Arc::clone(&balance_hits_v2);
    let ops_v2 = Router::new().route(
        "/api/user/balance",
        get(move |headers: axum::http::HeaderMap| {
            let balance_hits_inner = Arc::clone(&balance_hits_v2_clone);
            async move {
                *balance_hits_inner.lock().expect("mutex should lock") += 1;
                assert_eq!(
                    headers
                        .get(axum::http::header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok()),
                    Some("Bearer next-secret-api-key")
                );
                (
                    StatusCode::OK,
                    Json(json!({
                        "success": true,
                        "data": {
                            "quota": 3500000,
                            "used_quota": 1000000
                        }
                    })),
                )
            }
        }),
    );

    let (ops_v1_url, ops_v1_handle) = start_server(ops_v1).await;
    let (ops_v2_url, ops_v2_handle) = start_server(ops_v2).await;
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![
            sample_provider("provider-openai", "openai", 10).with_transport_fields(
                true,
                false,
                true,
                None,
                None,
                None,
                None,
                None,
                Some(json!({
                    "provider_ops": {
                        "architecture_id": "generic_api",
                        "base_url": ops_v1_url,
                        "connector": {
                            "auth_type": "api_key",
                            "config": {
                                "auth_method": "bearer"
                            },
                            "credentials": {
                                "api_key": encrypt_python_fernet_plaintext(
                                    DEVELOPMENT_ENCRYPTION_KEY,
                                    "live-secret-api-key",
                                ).expect("api key should encrypt"),
                            }
                        }
                    }
                })),
            ),
        ],
        vec![],
        vec![],
    ));
    let data_state = GatewayDataState::from_config(
        GatewayDataConfig::disabled()
            .with_redis_url(
                redis.redis_url(),
                Some("provider_ops_balance_cache_config_save"),
            )
            .with_encryption_key(DEVELOPMENT_ENCRYPTION_KEY),
    )
    .expect("data state should build")
    .attach_provider_catalog_repository_for_tests(Arc::clone(&provider_catalog_repository));
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();

    let initial_response = client
        .get(format!(
            "{gateway_url}/api/admin/provider-ops/providers/provider-openai/balance?refresh=false"
        ))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(initial_response.status(), StatusCode::OK);
    let initial_payload: serde_json::Value = initial_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(initial_payload["status"], "success");
    assert_eq!(initial_payload["data"]["total_available"], 4.0);
    assert_eq!(*balance_hits_v1.lock().expect("mutex should lock"), 1);
    assert_eq!(*balance_hits_v2.lock().expect("mutex should lock"), 0);

    let save_response = client
        .put(format!(
            "{gateway_url}/api/admin/provider-ops/providers/provider-openai/config"
        ))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "architecture_id": "generic_api",
            "base_url": ops_v2_url,
            "connector": {
                "auth_type": "api_key",
                "config": {
                    "auth_method": "bearer"
                },
                "credentials": {
                    "api_key": "next-secret-api-key"
                }
            },
            "actions": {},
            "schedule": {}
        }))
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(save_response.status(), StatusCode::OK);

    let updated_response = client
        .get(format!(
            "{gateway_url}/api/admin/provider-ops/providers/provider-openai/balance?refresh=false"
        ))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(updated_response.status(), StatusCode::OK);
    let updated_payload: serde_json::Value = updated_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(updated_payload["status"], "success");
    assert_eq!(updated_payload["data"]["total_available"], 7.0);
    assert_eq!(updated_payload["data"]["total_used"], 2.0);
    assert_eq!(*balance_hits_v1.lock().expect("mutex should lock"), 1);
    assert_eq!(*balance_hits_v2.lock().expect("mutex should lock"), 1);

    gateway_handle.abort();
    ops_v1_handle.abort();
    ops_v2_handle.abort();
}

#[tokio::test]
async fn gateway_verify_does_not_pollute_balance_cache_and_balance_uses_saved_action_config() {
    let Some(redis) = start_managed_redis_or_skip().await else {
        return;
    };
    let saved_balance_hits = Arc::new(Mutex::new(0usize));
    let saved_balance_hits_clone = Arc::clone(&saved_balance_hits);
    let verify_hits = Arc::new(Mutex::new(0usize));
    let verify_hits_clone = Arc::clone(&verify_hits);
    let saved_ops = Router::new()
        .route(
            "/api/user/self",
            any(|| async move {
                (
                    StatusCode::OK,
                    Json(json!({
                        "success": true,
                        "data": {
                            "username": "saved-user",
                            "quota": 9999999
                        }
                    })),
                )
            }),
        )
        .route(
            "/api/custom-balance",
            get(move |headers: axum::http::HeaderMap| {
                let saved_balance_hits_inner = Arc::clone(&saved_balance_hits_clone);
                async move {
                    *saved_balance_hits_inner.lock().expect("mutex should lock") += 1;
                    assert_eq!(
                        headers
                            .get(axum::http::header::AUTHORIZATION)
                            .and_then(|value| value.to_str().ok()),
                        Some("Bearer live-secret-api-key")
                    );
                    (
                        StatusCode::OK,
                        Json(json!({
                            "success": true,
                            "data": {
                                "quota": 8000000,
                                "used_quota": 2000000
                            }
                        })),
                    )
                }
            }),
        );
    let verify_ops = Router::new().route(
        "/api/user/self",
        any(move |headers: axum::http::HeaderMap| {
            let verify_hits_inner = Arc::clone(&verify_hits_clone);
            async move {
                *verify_hits_inner.lock().expect("mutex should lock") += 1;
                assert_eq!(
                    headers
                        .get(axum::http::header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok()),
                    Some("Bearer verify-secret-api-key")
                );
                (
                    StatusCode::OK,
                    Json(json!({
                        "success": true,
                        "data": {
                            "username": "verify-user",
                            "quota": 12000000,
                            "used_quota": 4000000
                        }
                    })),
                )
            }
        }),
    );

    let (saved_ops_url, saved_ops_handle) = start_server(saved_ops).await;
    let (verify_ops_url, verify_ops_handle) = start_server(verify_ops).await;
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![
            sample_provider("provider-openai", "openai", 10).with_transport_fields(
                true,
                false,
                true,
                None,
                None,
                None,
                None,
                None,
                Some(json!({
                    "provider_ops": {
                        "architecture_id": "generic_api",
                        "base_url": saved_ops_url,
                        "connector": {
                            "auth_type": "api_key",
                            "config": {
                                "auth_method": "bearer"
                            },
                            "credentials": {
                                "api_key": encrypt_python_fernet_plaintext(
                                    DEVELOPMENT_ENCRYPTION_KEY,
                                    "live-secret-api-key",
                                ).expect("api key should encrypt"),
                            }
                        },
                        "actions": {
                            "query_balance": {
                                "enabled": true,
                                "config": {
                                    "endpoint": "/api/custom-balance",
                                    "quota_divisor": 2000000,
                                    "currency": "CNY"
                                }
                            }
                        }
                    }
                })),
            ),
        ],
        vec![],
        vec![],
    ));
    let data_state = GatewayDataState::from_config(
        GatewayDataConfig::disabled()
            .with_redis_url(
                redis.redis_url(),
                Some("provider_ops_verify_cache_isolation"),
            )
            .with_encryption_key(DEVELOPMENT_ENCRYPTION_KEY),
    )
    .expect("data state should build")
    .attach_provider_catalog_repository_for_tests(Arc::clone(&provider_catalog_repository));
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();

    let verify_response = client
        .post(format!(
            "{gateway_url}/api/admin/provider-ops/providers/provider-openai/verify"
        ))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "architecture_id": "generic_api",
            "base_url": verify_ops_url,
            "connector": {
                "auth_type": "api_key",
                "config": {
                    "auth_method": "bearer"
                },
                "credentials": {
                    "api_key": "verify-secret-api-key"
                }
            },
            "actions": {
                "query_balance": {
                    "enabled": true,
                    "config": {
                        "endpoint": "/api/user/self",
                        "quota_divisor": 1,
                        "currency": "USD"
                    }
                }
            },
            "schedule": {}
        }))
        .send()
        .await
        .expect("verify request should succeed");
    assert_eq!(verify_response.status(), StatusCode::OK);
    let verify_payload: serde_json::Value = verify_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(verify_payload["success"], true);
    assert_eq!(verify_payload["data"]["username"], "verify-user");
    assert_eq!(*verify_hits.lock().expect("mutex should lock"), 1);
    assert_eq!(*saved_balance_hits.lock().expect("mutex should lock"), 0);

    let first_balance_response = client
        .get(format!(
            "{gateway_url}/api/admin/provider-ops/providers/provider-openai/balance?refresh=false"
        ))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("balance request should succeed");
    assert_eq!(first_balance_response.status(), StatusCode::OK);
    let first_balance_payload: serde_json::Value = first_balance_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(first_balance_payload["status"], "success");
    assert_eq!(first_balance_payload["data"]["total_available"], 4.0);
    assert_eq!(first_balance_payload["data"]["total_used"], 1.0);
    assert_eq!(first_balance_payload["data"]["currency"], "CNY");
    assert_eq!(*saved_balance_hits.lock().expect("mutex should lock"), 1);

    let cached_balance_response = client
        .get(format!(
            "{gateway_url}/api/admin/provider-ops/providers/provider-openai/balance?refresh=false"
        ))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("cached balance request should succeed");
    assert_eq!(cached_balance_response.status(), StatusCode::OK);
    let cached_balance_payload: serde_json::Value = cached_balance_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(cached_balance_payload["status"], "success");
    assert_eq!(cached_balance_payload["data"]["total_available"], 4.0);
    assert_eq!(cached_balance_payload["data"]["total_used"], 1.0);
    assert_eq!(cached_balance_payload["data"]["currency"], "CNY");
    assert_eq!(*saved_balance_hits.lock().expect("mutex should lock"), 1);

    gateway_handle.abort();
    saved_ops_handle.abort();
    verify_ops_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_provider_ops_batch_balance_with_pending_cache_hits_and_redis() {
    let Some(redis) = start_managed_redis_or_skip().await else {
        return;
    };
    let balance_hits = Arc::new(Mutex::new(0usize));
    let balance_hits_clone = Arc::clone(&balance_hits);
    let ops = Router::new().route(
        "/api/user/balance",
        get(move || {
            let balance_hits_inner = Arc::clone(&balance_hits_clone);
            async move {
                *balance_hits_inner.lock().expect("mutex should lock") += 1;
                (
                    StatusCode::OK,
                    Json(json!({
                        "success": true,
                        "data": {
                            "quota": 1500000,
                            "used_quota": 500000
                        }
                    })),
                )
            }
        }),
    );

    let (ops_url, ops_handle) = start_server(ops).await;
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![
            sample_provider("provider-openai", "openai", 10).with_transport_fields(
                true,
                false,
                true,
                None,
                None,
                None,
                None,
                None,
                Some(json!({
                    "provider_ops": {
                        "architecture_id": "generic_api",
                        "base_url": ops_url,
                        "connector": {
                            "auth_type": "api_key",
                            "config": {
                                "auth_method": "bearer"
                            },
                            "credentials": {
                                "api_key": encrypt_python_fernet_plaintext(
                                    DEVELOPMENT_ENCRYPTION_KEY,
                                    "live-secret-api-key",
                                ).expect("api key should encrypt"),
                            }
                        }
                    }
                })),
            ),
        ],
        vec![],
        vec![],
    ));
    let data_state = GatewayDataState::from_config(
        GatewayDataConfig::disabled()
            .with_redis_url(redis.redis_url(), Some("provider_ops_batch_balance"))
            .with_encryption_key(DEVELOPMENT_ENCRYPTION_KEY),
    )
    .expect("data state should build")
    .attach_provider_catalog_repository_for_tests(Arc::clone(&provider_catalog_repository));
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();

    let pending_response = client
        .post(format!(
            "{gateway_url}/api/admin/provider-ops/batch/balance"
        ))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!(["provider-openai"]))
        .send()
        .await
        .expect("request should succeed");
    let pending_payload: serde_json::Value = pending_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(pending_payload["provider-openai"]["status"], "pending");

    wait_until(5000, || {
        *balance_hits.lock().expect("mutex should lock") == 1
    })
    .await;

    let cached_response = client
        .post(format!(
            "{gateway_url}/api/admin/provider-ops/batch/balance"
        ))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!(["provider-openai"]))
        .send()
        .await
        .expect("request should succeed");
    let cached_payload: serde_json::Value = cached_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(cached_payload["provider-openai"]["status"], "success");
    assert_eq!(
        cached_payload["provider-openai"]["data"]["total_available"],
        3.0
    );
    wait_until(5000, || {
        *balance_hits.lock().expect("mutex should lock") == 2
    })
    .await;

    gateway_handle.abort();
    ops_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_provider_ops_sub2api_balance_with_refresh_token_rotation() {
    let ops = Router::new()
        .route(
            "/api/v1/auth/refresh",
            post(|request: Request| async move {
                let body = to_bytes(request.into_body(), usize::MAX)
                    .await
                    .expect("body should read");
                let payload: serde_json::Value =
                    serde_json::from_slice(&body).expect("json body should parse");
                assert_eq!(payload["refresh_token"], "refresh-token-old");
                (
                    StatusCode::OK,
                    Json(json!({
                        "code": 0,
                        "data": {
                            "access_token": "access-token-new",
                            "refresh_token": "refresh-token-new",
                            "expires_in": 900
                        }
                    })),
                )
            }),
        )
        .route(
            "/api/v1/auth/me",
            get(|headers: axum::http::HeaderMap| async move {
                assert_eq!(
                    headers
                        .get(axum::http::header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok()),
                    Some("Bearer access-token-new")
                );
                (
                    StatusCode::OK,
                    Json(json!({
                        "code": 0,
                        "data": {
                            "username": "sub2api-user",
                            "email": "sub2api@example.com",
                            "balance": 8.5,
                            "points": 1.5
                        }
                    })),
                )
            }),
        )
        .route(
            "/api/v1/subscriptions/summary",
            get(|headers: axum::http::HeaderMap| async move {
                assert_eq!(
                    headers
                        .get(axum::http::header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok()),
                    Some("Bearer access-token-new")
                );
                (
                    StatusCode::OK,
                    Json(json!({
                        "code": 0,
                        "data": {
                            "active_count": 2,
                            "total_used_usd": 3.5,
                            "subscriptions": [
                                {
                                    "group_name": "Pro",
                                    "status": "active",
                                    "monthly_used_usd": 1.2,
                                    "monthly_limit_usd": 20,
                                    "expires_at": "2099-12-31T00:00:00Z"
                                }
                            ]
                        }
                    })),
                )
            }),
        );

    let (ops_url, ops_handle) = start_server(ops).await;
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![
            sample_provider("provider-openai", "openai", 10).with_transport_fields(
                true,
                false,
                true,
                None,
                None,
                None,
                None,
                None,
                Some(json!({
                    "provider_ops": {
                        "architecture_id": "sub2api",
                        "base_url": ops_url,
                        "connector": {
                            "auth_type": "api_key",
                            "config": {},
                            "credentials": {
                                "refresh_token": encrypt_python_fernet_plaintext(
                                    DEVELOPMENT_ENCRYPTION_KEY,
                                    "refresh-token-old",
                                ).expect("refresh token should encrypt"),
                                "_cached_access_token": encrypt_python_fernet_plaintext(
                                    DEVELOPMENT_ENCRYPTION_KEY,
                                    "stale-access-token",
                                ).expect("cached access token should encrypt"),
                                "_cached_token_expires_at": 0.0,
                            }
                        }
                    }
                })),
            ),
        ],
        vec![],
        vec![],
    ));
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(Arc::clone(
                    &provider_catalog_repository,
                ))
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-ops/providers/provider-openai/balance"
        ))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["status"], "success");
    assert_eq!(payload["data"]["total_available"], 10.0);
    assert_eq!(payload["data"]["extra"]["balance"], 8.5);
    assert_eq!(payload["data"]["extra"]["points"], 1.5);
    assert_eq!(payload["data"]["extra"]["active_subscriptions"], 2);
    assert_eq!(payload["data"]["extra"]["total_used_usd"], 3.5);
    assert_eq!(
        payload["data"]["extra"]["subscriptions"][0]["group_name"],
        "Pro"
    );

    let stored_provider = provider_catalog_repository
        .list_providers_by_ids(&["provider-openai".to_string()])
        .await
        .expect("providers should list")
        .into_iter()
        .next()
        .expect("stored provider should exist");
    let credentials = stored_provider
        .config
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .and_then(|config| config.get("provider_ops"))
        .and_then(serde_json::Value::as_object)
        .and_then(|config| config.get("connector"))
        .and_then(serde_json::Value::as_object)
        .and_then(|connector| connector.get("credentials"))
        .and_then(serde_json::Value::as_object)
        .expect("credentials should be object");
    let stored_refresh_token = credentials
        .get("refresh_token")
        .and_then(serde_json::Value::as_str)
        .expect("refresh token should be string");
    assert_eq!(
        decrypt_python_fernet_ciphertext(DEVELOPMENT_ENCRYPTION_KEY, stored_refresh_token)
            .expect("refresh token should decrypt"),
        "refresh-token-new"
    );
    let stored_cached_access_token = credentials
        .get("_cached_access_token")
        .and_then(serde_json::Value::as_str)
        .expect("cached access token should be string");
    assert_eq!(
        decrypt_python_fernet_ciphertext(DEVELOPMENT_ENCRYPTION_KEY, stored_cached_access_token,)
            .expect("cached access token should decrypt"),
        "access-token-new"
    );
    assert!(
        credentials
            .get("_cached_token_expires_at")
            .and_then(serde_json::Value::as_f64)
            .expect("cached token expires at should be number")
            > std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time should move forward")
                .as_secs_f64()
    );

    gateway_handle.abort();
    ops_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_provider_ops_sub2api_balance_against_site_root_when_base_url_has_path(
) {
    let nested_refresh_hits = Arc::new(Mutex::new(0usize));
    let nested_refresh_hits_clone = Arc::clone(&nested_refresh_hits);
    let nested_me_hits = Arc::new(Mutex::new(0usize));
    let nested_me_hits_clone = Arc::clone(&nested_me_hits);
    let nested_subscription_hits = Arc::new(Mutex::new(0usize));
    let nested_subscription_hits_clone = Arc::clone(&nested_subscription_hits);
    let ops = Router::new()
        .route(
            "/nested/api/v1/auth/refresh",
            post(move |_request: Request| {
                let nested_refresh_hits_inner = Arc::clone(&nested_refresh_hits_clone);
                async move {
                    *nested_refresh_hits_inner.lock().expect("mutex should lock") += 1;
                    (
                        StatusCode::OK,
                        Json(json!({
                            "code": 1,
                            "message": "invalid refresh token",
                        })),
                    )
                }
            }),
        )
        .route(
            "/nested/api/v1/auth/me",
            get(move || {
                let nested_me_hits_inner = Arc::clone(&nested_me_hits_clone);
                async move {
                    *nested_me_hits_inner.lock().expect("mutex should lock") += 1;
                    (
                        StatusCode::OK,
                        Json(json!({
                            "code": 1,
                            "message": "unexpected nested me request",
                        })),
                    )
                }
            }),
        )
        .route(
            "/nested/api/v1/subscriptions/summary",
            get(move || {
                let nested_subscription_hits_inner = Arc::clone(&nested_subscription_hits_clone);
                async move {
                    *nested_subscription_hits_inner
                        .lock()
                        .expect("mutex should lock") += 1;
                    (
                        StatusCode::OK,
                        Json(json!({
                            "code": 1,
                            "message": "unexpected nested subscriptions request",
                        })),
                    )
                }
            }),
        )
        .route(
            "/api/v1/auth/refresh",
            post(|request: Request| async move {
                let body = to_bytes(request.into_body(), usize::MAX)
                    .await
                    .expect("body should read");
                let payload: serde_json::Value =
                    serde_json::from_slice(&body).expect("json body should parse");
                assert_eq!(payload["refresh_token"], "refresh-token-old");
                (
                    StatusCode::OK,
                    Json(json!({
                        "code": 0,
                        "data": {
                            "access_token": "access-token-new",
                            "refresh_token": "refresh-token-new",
                            "expires_in": 900
                        }
                    })),
                )
            }),
        )
        .route(
            "/api/v1/auth/me",
            get(|headers: axum::http::HeaderMap| async move {
                assert_eq!(
                    headers
                        .get(axum::http::header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok()),
                    Some("Bearer access-token-new")
                );
                (
                    StatusCode::OK,
                    Json(json!({
                        "code": 0,
                        "data": {
                            "username": "sub2api-user",
                            "email": "sub2api@example.com",
                            "balance": 8.5,
                            "points": 1.5
                        }
                    })),
                )
            }),
        )
        .route(
            "/api/v1/subscriptions/summary",
            get(|headers: axum::http::HeaderMap| async move {
                assert_eq!(
                    headers
                        .get(axum::http::header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok()),
                    Some("Bearer access-token-new")
                );
                (
                    StatusCode::OK,
                    Json(json!({
                        "code": 0,
                        "data": {
                            "active_count": 2,
                            "total_used_usd": 3.5,
                            "subscriptions": [
                                {
                                    "group_name": "Pro",
                                    "status": "active",
                                    "monthly_used_usd": 1.2,
                                    "monthly_limit_usd": 20,
                                    "expires_at": "2099-12-31T00:00:00Z"
                                }
                            ]
                        }
                    })),
                )
            }),
        );

    let (ops_url, ops_handle) = start_server(ops).await;
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![
            sample_provider("provider-openai", "openai", 10).with_transport_fields(
                true,
                false,
                true,
                None,
                None,
                None,
                None,
                None,
                Some(json!({
                    "provider_ops": {
                        "architecture_id": "sub2api",
                        "base_url": format!("{ops_url}/nested"),
                        "connector": {
                            "auth_type": "api_key",
                            "config": {},
                            "credentials": {
                                "refresh_token": encrypt_python_fernet_plaintext(
                                    DEVELOPMENT_ENCRYPTION_KEY,
                                    "refresh-token-old",
                                ).expect("refresh token should encrypt"),
                            }
                        }
                    }
                })),
            ),
        ],
        vec![],
        vec![],
    ));
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(Arc::clone(
                    &provider_catalog_repository,
                ))
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-ops/providers/provider-openai/balance"
        ))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["status"], "success");
    assert_eq!(payload["data"]["total_available"], 10.0);
    assert_eq!(payload["data"]["extra"]["active_subscriptions"], 2);
    assert_eq!(*nested_refresh_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*nested_me_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(
        *nested_subscription_hits.lock().expect("mutex should lock"),
        0
    );

    gateway_handle.abort();
    ops_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_provider_ops_sub2api_balance_with_session_login() {
    let ops = Router::new()
        .route(
            "/api/v1/auth/login",
            post(|request: Request| async move {
                let body = to_bytes(request.into_body(), usize::MAX)
                    .await
                    .expect("body should read");
                let payload: serde_json::Value =
                    serde_json::from_slice(&body).expect("json body should parse");
                assert_eq!(payload["email"], "sub2api@example.com");
                assert_eq!(payload["password"], "top-secret-password");
                (
                    StatusCode::OK,
                    Json(json!({
                        "code": 0,
                        "data": {
                            "access_token": "login-access-token",
                            "refresh_token": "login-refresh-token",
                            "expires_in": 900
                        }
                    })),
                )
            }),
        )
        .route(
            "/api/v1/auth/me",
            get(|headers: axum::http::HeaderMap| async move {
                assert_eq!(
                    headers
                        .get(axum::http::header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok()),
                    Some("Bearer login-access-token")
                );
                (
                    StatusCode::OK,
                    Json(json!({
                        "code": 0,
                        "data": {
                            "username": "sub2api-user",
                            "balance": 6,
                            "points": 2
                        }
                    })),
                )
            }),
        )
        .route(
            "/api/v1/subscriptions/summary",
            get(|| async move {
                (
                    StatusCode::OK,
                    Json(json!({
                        "code": 0,
                        "data": {
                            "active_count": 1,
                            "subscriptions": []
                        }
                    })),
                )
            }),
        );

    let (ops_url, ops_handle) = start_server(ops).await;
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![
            sample_provider("provider-openai", "openai", 10).with_transport_fields(
                true,
                false,
                true,
                None,
                None,
                None,
                None,
                None,
                Some(json!({
                    "provider_ops": {
                        "architecture_id": "sub2api",
                        "base_url": ops_url,
                        "connector": {
                            "auth_type": "session_login",
                            "config": {},
                            "credentials": {
                                "email": "sub2api@example.com",
                                "password": encrypt_python_fernet_plaintext(
                                    DEVELOPMENT_ENCRYPTION_KEY,
                                    "top-secret-password",
                                ).expect("password should encrypt"),
                            }
                        }
                    }
                })),
            ),
        ],
        vec![],
        vec![],
    ));
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(Arc::clone(
                    &provider_catalog_repository,
                ))
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-ops/providers/provider-openai/balance"
        ))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["status"], "success");
    assert_eq!(payload["data"]["total_available"], 8.0);
    assert_eq!(payload["data"]["extra"]["active_subscriptions"], 1);

    let stored_provider = provider_catalog_repository
        .list_providers_by_ids(&["provider-openai".to_string()])
        .await
        .expect("providers should list")
        .into_iter()
        .next()
        .expect("stored provider should exist");
    let credentials = stored_provider
        .config
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .and_then(|config| config.get("provider_ops"))
        .and_then(serde_json::Value::as_object)
        .and_then(|config| config.get("connector"))
        .and_then(serde_json::Value::as_object)
        .and_then(|connector| connector.get("credentials"))
        .and_then(serde_json::Value::as_object)
        .expect("credentials should be object");
    let stored_refresh_token = credentials
        .get("refresh_token")
        .and_then(serde_json::Value::as_str)
        .expect("refresh token should be string");
    assert_eq!(
        decrypt_python_fernet_ciphertext(DEVELOPMENT_ENCRYPTION_KEY, stored_refresh_token)
            .expect("refresh token should decrypt"),
        "login-refresh-token"
    );
    let stored_cached_access_token = credentials
        .get("_cached_access_token")
        .and_then(serde_json::Value::as_str)
        .expect("cached access token should be string");
    assert_eq!(
        decrypt_python_fernet_ciphertext(DEVELOPMENT_ENCRYPTION_KEY, stored_cached_access_token,)
            .expect("cached access token should decrypt"),
        "login-access-token"
    );
    assert!(
        credentials
            .get("_cached_token_expires_at")
            .and_then(serde_json::Value::as_f64)
            .expect("cached token expires at should be number")
            > std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time should move forward")
                .as_secs_f64()
    );

    gateway_handle.abort();
    ops_handle.abort();
}
