use std::sync::{Arc, Mutex};

use aether_crypto::{
    decrypt_python_fernet_ciphertext, encrypt_python_fernet_plaintext, DEVELOPMENT_ENCRYPTION_KEY,
};
use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
use aether_data_contracts::repository::provider_catalog::ProviderCatalogReadRepository;
use axum::body::to_bytes;
use axum::body::Body;
use axum::routing::{any, get, post};
use axum::{extract::Request, Json, Router};
use http::StatusCode;
use serde_json::json;

use super::super::{
    build_router_with_state, issue_test_admin_access_token, sample_endpoint, sample_provider,
    start_server, AppState,
};
use crate::constants::{
    GATEWAY_HEADER, TRUSTED_ADMIN_SESSION_ID_HEADER, TRUSTED_ADMIN_USER_ID_HEADER,
    TRUSTED_ADMIN_USER_ROLE_HEADER,
};
use crate::data::GatewayDataState;

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
    assert_eq!(payload["success"], true);
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
    assert_eq!(
        credentials.get("_cached_access_token"),
        Some(&json!("cached-token"))
    );
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
    assert_eq!(payload["data"]["display_name"], "Alice");
    assert_eq!(payload["data"]["quota"], 7.5);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
    verify_handle.abort();
}

#[tokio::test]
async fn gateway_verifies_admin_provider_ops_locally_for_anyrouter_proxy_mode() {
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
            "base_url": "https://ops.example",
            "connector": {
                "auth_type": "cookie",
                "config": {
                    "proxy_enabled": true,
                },
                "credentials": {
                    "session_cookie": "session=live-session-cookie",
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
    let message = payload
        .get("message")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    assert!(
        message.starts_with("连接失败:") || message == "连接超时",
        "message should report local network verification failure, got: {message}"
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
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
                        "base_url": "https://anyrouter.example",
                        "connector": {
                            "auth_type": "cookie",
                            "config": {},
                            "credentials": {
                                "session_cookie": encrypt_python_fernet_plaintext(
                                    DEVELOPMENT_ENCRYPTION_KEY,
                                    "session=live-session-cookie",
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
    assert_eq!(payload["provider-anyrouter"]["status"], "not_supported");
    assert_eq!(
        payload["provider-anyrouter"]["message"],
        "Provider 操作仅支持 Rust execution runtime"
    );
    assert_eq!(payload["provider-missing"]["status"], "not_configured");
    assert_eq!(payload["provider-missing"]["message"], "未配置操作设置");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
    ops_handle.abort();
}
