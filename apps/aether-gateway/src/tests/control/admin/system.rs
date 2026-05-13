use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use aether_crypto::{encrypt_python_fernet_plaintext, DEVELOPMENT_ENCRYPTION_KEY};
use aether_data::repository::auth::{
    InMemoryAuthApiKeySnapshotRepository, StoredAuthApiKeyExportRecord,
};
use aether_data::repository::auth_modules::{
    InMemoryAuthModuleReadRepository, StoredOAuthProviderModuleConfig,
};
use aether_data::repository::candidates::InMemoryRequestCandidateRepository;
use aether_data::repository::global_models::InMemoryGlobalModelReadRepository;
use aether_data::repository::oauth_providers::InMemoryOAuthProviderRepository;
use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
use aether_data::repository::proxy_nodes::InMemoryProxyNodeRepository;
use aether_data::repository::users::{
    InMemoryUserReadRepository, StoredUserAuthRecord, UpsertUserGroupRecord, UserReadRepository,
};
use aether_data::repository::wallet::{InMemoryWalletRepository, StoredWalletSnapshot};
use aether_data_contracts::repository::global_models::StoredPublicGlobalModel;
use axum::body::Body;
use axum::routing::{any, delete, get, post, put};
use axum::{extract::Request, Router};
use http::StatusCode;
use serde_json::json;

use super::super::{
    build_router_with_state, issue_test_admin_access_token, sample_admin_global_model,
    sample_admin_provider_model, sample_endpoint, sample_key, sample_ldap_module_config,
    sample_oauth_provider_config, sample_provider, sample_proxy_node,
    sample_recent_key_rpm_candidate, start_server, AppState,
};
use crate::constants::{
    GATEWAY_HEADER, TRUSTED_ADMIN_SESSION_ID_HEADER, TRUSTED_ADMIN_USER_ID_HEADER,
    TRUSTED_ADMIN_USER_ROLE_HEADER,
};
use crate::data::GatewayDataState;

#[tokio::test]
async fn gateway_handles_admin_system_version_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/system/version",
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
        .get(format!("{gateway_url}/api/admin/system/version"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert!(payload["version"]
        .as_str()
        .is_some_and(|value| !value.is_empty()));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_system_check_update_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/system/check-update",
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
        .get(format!("{gateway_url}/api/admin/system/check-update"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert!(payload["current_version"]
        .as_str()
        .is_some_and(|value| !value.is_empty()));
    assert_eq!(payload["latest_version"], serde_json::Value::Null);
    assert_eq!(payload["has_update"], json!(false));
    assert_eq!(payload["error"], "检查更新需要 Rust 管理后端");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_system_check_update_locally_with_bearer_admin_session() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/system/check-update",
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
    let access_token = issue_test_admin_access_token(&state, "device-admin-system").await;
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/api/admin/system/check-update"))
        .header("authorization", format!("Bearer {access_token}"))
        .header("x-client-device-id", "device-admin-system")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["has_update"], json!(false));
    assert_eq!(payload["error"], "检查更新需要 Rust 管理后端");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_system_aws_regions_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/system/aws-regions",
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
        .get(format!("{gateway_url}/api/admin/system/aws-regions"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    let regions = payload["regions"]
        .as_array()
        .expect("regions should be array");
    assert!(regions.len() > 10);
    assert!(regions.iter().any(|value| value == "us-east-1"));
    assert!(regions.iter().any(|value| value == "eu-west-1"));
    assert!(regions.iter().any(|value| value == "ap-southeast-1"));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_system_aws_regions_locally_with_bearer_admin_session() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/system/aws-regions",
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
    let access_token = issue_test_admin_access_token(&state, "device-admin-aws-regions").await;
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/api/admin/system/aws-regions"))
        .header("authorization", format!("Bearer {access_token}"))
        .header("x-client-device-id", "device-admin-aws-regions")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    let regions = payload["regions"]
        .as_array()
        .expect("regions should be array");
    assert!(regions.iter().any(|value| value == "us-east-1"));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_system_stats_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/system/stats",
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
            sample_provider("provider-anthropic", "anthropic", 20)
                .with_transport_fields(false, false, false, None, None, None, None, None, None),
        ],
        vec![],
        vec![],
    ));
    let data_state =
        GatewayDataState::with_provider_catalog_reader_for_tests(provider_catalog_repository);

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/api/admin/system/stats"))
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
    assert_eq!(payload["api_keys"], json!(0));
    assert_eq!(payload["requests"], json!(0));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_system_settings_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/system/settings",
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
    let data_state =
        GatewayDataState::with_provider_catalog_reader_for_tests(provider_catalog_repository)
            .with_system_config_values_for_tests(vec![(
                "default_model".to_string(),
                json!("gpt-4o-mini"),
            )]);

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/api/admin/system/settings"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["default_provider"], "openai");
    assert_eq!(payload["default_model"], "gpt-4o-mini");
    assert_eq!(payload["enable_usage_tracking"], json!(true));
    assert_eq!(payload["password_policy_level"], "weak");

    let response = reqwest::Client::new()
        .put(format!("{gateway_url}/api/admin/system/settings"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "default_provider": "openai",
            "default_model": "gpt-5",
            "enable_usage_tracking": false,
            "password_policy_level": "strong",
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["message"], "系统设置更新成功");

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/api/admin/system/settings"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["default_provider"], "openai");
    assert_eq!(payload["default_model"], "gpt-5");
    assert_eq!(payload["enable_usage_tracking"], json!(false));
    assert_eq!(payload["password_policy_level"], "strong");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_system_config_export_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/system/config/export",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let provider_id = "provider-openai".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![
            sample_provider(&provider_id, "openai", 10).with_transport_fields(
                true,
                false,
                true,
                Some(8),
                Some(3),
                Some(json!({"node_id": "node-1"})),
                Some(30.0),
                Some(12.5),
                Some(json!({
                    "provider_ops": {
                        "connector": {
                            "credentials": {
                                "refresh_token": encrypt_python_fernet_plaintext(
                                    DEVELOPMENT_ENCRYPTION_KEY,
                                    "provider-refresh-token",
                                )
                                .expect("provider credential should encrypt")
                            }
                        }
                    }
                })),
            ),
        ],
        vec![
            sample_endpoint(
                "endpoint-chat",
                &provider_id,
                "openai",
                "https://api.openai.example",
            ),
            sample_endpoint(
                "endpoint-cli",
                &provider_id,
                "openai:responses",
                "https://api.openai.example",
            ),
        ],
        vec![{
            let mut key = sample_key("key-openai", &provider_id, "openai:chat", "live-api-key");
            key.name = "primary".to_string();
            key.allowed_models = Some(json!(["gpt-5"]));
            key.encrypted_auth_config = Some(
                encrypt_python_fernet_plaintext(
                    DEVELOPMENT_ENCRYPTION_KEY,
                    r#"{"refresh_token":"oauth-refresh"}"#,
                )
                .expect("auth config should encrypt"),
            );
            key
        }],
    ));
    let global_model_repository = Arc::new(
        InMemoryGlobalModelReadRepository::seed(Vec::<StoredPublicGlobalModel>::new())
            .with_admin_global_models(vec![sample_admin_global_model(
                "global-gpt-5",
                "gpt-5",
                "GPT 5",
            )])
            .with_admin_provider_models(vec![sample_admin_provider_model(
                "model-gpt-5",
                &provider_id,
                "global-gpt-5",
                "gpt-5",
            )]),
    );
    let auth_module_repository = Arc::new(InMemoryAuthModuleReadRepository::seed(
        Vec::<StoredOAuthProviderModuleConfig>::new(),
        Some(sample_ldap_module_config()),
    ));
    let oauth_provider_repository = Arc::new(InMemoryOAuthProviderRepository::seed(vec![
        sample_oauth_provider_config("linuxdo"),
    ]));
    let proxy_node_repository =
        Arc::new(InMemoryProxyNodeRepository::seed(vec![sample_proxy_node(
            "node-1",
        )
        .with_manual_proxy_fields(
            Some("http://proxy.local:8080".to_string()),
            Some("proxy-user".to_string()),
            Some("proxy-pass".to_string()),
        )]));

    let data_state = GatewayDataState::disabled()
        .attach_provider_catalog_repository_for_tests(provider_catalog_repository)
        .with_global_model_repository_for_tests(global_model_repository)
        .attach_auth_module_reader_for_tests(auth_module_repository)
        .attach_oauth_provider_repository_for_tests(oauth_provider_repository)
        .attach_proxy_node_repository_for_tests(proxy_node_repository)
        .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY)
        .with_system_config_values_for_tests(vec![
            (
                "smtp_password".to_string(),
                json!(
                    encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "smtp-secret",)
                        .expect("smtp secret should encrypt")
                ),
            ),
            ("site_name".to_string(), json!("Aether Test")),
        ]);

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/api/admin/system/config/export"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["version"], "2.2");
    assert!(payload["exported_at"].as_str().is_some());
    assert_eq!(payload["global_models"][0]["name"], "gpt-5");
    assert_eq!(payload["providers"][0]["name"], "openai");
    assert_eq!(
        payload["providers"][0]["config"]["provider_ops"]["connector"]["credentials"]
            ["refresh_token"],
        "provider-refresh-token"
    );
    assert_eq!(
        payload["providers"][0]["api_keys"][0]["api_key"],
        "live-api-key"
    );
    assert_eq!(
        payload["providers"][0]["api_keys"][0]["auth_config"],
        r#"{"refresh_token":"oauth-refresh"}"#
    );
    assert_eq!(
        payload["providers"][0]["api_keys"][0]["supported_endpoints"],
        json!(["openai:chat"])
    );
    assert_eq!(
        payload["providers"][0]["models"][0]["global_model_name"],
        "gpt-5"
    );
    assert_eq!(payload["ldap_config"]["bind_password"], "");
    assert_eq!(
        payload["oauth_providers"][0]["client_secret"],
        "secret-value"
    );
    assert_eq!(
        payload["proxy_nodes"][0]["proxy_url"],
        "http://proxy.local:8080"
    );
    let smtp_password = payload["system_configs"]
        .as_array()
        .expect("system configs should be array")
        .iter()
        .find(|entry| entry["key"] == "smtp_password")
        .cloned()
        .expect("smtp_password should exist");
    assert_eq!(smtp_password["value"], "smtp-secret");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_system_users_export_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/system/users/export",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let user_repository = Arc::new(InMemoryUserReadRepository::seed_auth_users(vec![
        StoredUserAuthRecord::new(
            "user-1".to_string(),
            Some("alice@example.com".to_string()),
            true,
            "alice".to_string(),
            Some("argon2-hash".to_string()),
            "user".to_string(),
            "local".to_string(),
            Some(json!(["openai"])),
            Some(json!(["openai:chat"])),
            Some(json!(["gpt-5"])),
            true,
            false,
            Some(chrono::Utc::now()),
            None,
        )
        .expect("user export row should build")
        .with_policy_modes(
            "specific".to_string(),
            "specific".to_string(),
            "specific".to_string(),
        )
        .expect("user policy modes should build"),
    ]));
    let user_group = user_repository
        .create_user_group(UpsertUserGroupRecord {
            name: "Restricted GPT".to_string(),
            description: Some("GPT-only users".to_string()),
            priority: 10,
            allowed_providers: Some(vec!["openai".to_string()]),
            allowed_providers_mode: "specific".to_string(),
            allowed_api_formats: Some(vec!["openai:chat".to_string()]),
            allowed_api_formats_mode: "specific".to_string(),
            allowed_models: Some(vec!["gpt-5".to_string()]),
            allowed_models_mode: "specific".to_string(),
            rate_limit: Some(60),
            rate_limit_mode: "custom".to_string(),
        })
        .await
        .expect("user group should create")
        .expect("user group should exist");
    user_repository
        .replace_user_groups_for_user("user-1", std::slice::from_ref(&user_group.id))
        .await
        .expect("user group membership should create");
    let auth_repository = Arc::new(
        InMemoryAuthApiKeySnapshotRepository::default().with_export_records(vec![
            StoredAuthApiKeyExportRecord::new(
                "user-1".to_string(),
                "key-user-1".to_string(),
                "hash-user-1".to_string(),
                Some(
                    encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "ak-user-live-1")
                        .expect("user api key should encrypt"),
                ),
                Some("User Key".to_string()),
                Some(json!(["openai"])),
                Some(json!(["openai:chat"])),
                Some(json!(["gpt-5"])),
                Some(60),
                Some(2),
                Some(json!({"cache": true})),
                true,
                Some(1_900_000_000),
                false,
                42,
                420,
                12.34,
                false,
            )
            .expect("user api key export record should build"),
            StoredAuthApiKeyExportRecord::new(
                "admin-owner".to_string(),
                "key-standalone-1".to_string(),
                "hash-standalone-1".to_string(),
                Some(
                    encrypt_python_fernet_plaintext(
                        DEVELOPMENT_ENCRYPTION_KEY,
                        "ak-standalone-live-1",
                    )
                    .expect("standalone api key should encrypt"),
                ),
                Some("Standalone Key".to_string()),
                Some(json!(["openai"])),
                Some(json!(["openai:chat"])),
                Some(json!(["gpt-5"])),
                None,
                Some(5),
                None,
                true,
                None,
                false,
                7,
                84,
                3.21,
                true,
            )
            .expect("standalone api key export record should build"),
        ]),
    );
    let wallet_repository = Arc::new(InMemoryWalletRepository::seed(vec![
        StoredWalletSnapshot::new(
            "wallet-user-1".to_string(),
            Some("user-1".to_string()),
            None,
            10.0,
            2.5,
            "finite".to_string(),
            "USD".to_string(),
            "active".to_string(),
            100.0,
            20.0,
            5.0,
            1.0,
            1_800_000_000,
        )
        .expect("user wallet should build"),
        StoredWalletSnapshot::new(
            "wallet-standalone-1".to_string(),
            None,
            Some("key-standalone-1".to_string()),
            30.0,
            0.0,
            "unlimited".to_string(),
            "USD".to_string(),
            "active".to_string(),
            80.0,
            10.0,
            0.0,
            0.0,
            1_800_000_100,
        )
        .expect("standalone wallet should build"),
    ]));
    let data_state =
        GatewayDataState::with_auth_and_wallet_for_tests(auth_repository, wallet_repository)
            .with_user_reader(user_repository)
            .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY);

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/api/admin/system/users/export"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["version"], "1.4");
    assert!(payload["exported_at"].as_str().is_some());
    assert_eq!(payload["user_groups"][0]["name"], "Restricted GPT");
    assert!(payload["user_groups"][0].get("priority").is_none());
    assert_eq!(
        payload["user_groups"][0]["allowed_models"],
        json!(["gpt-5"])
    );
    assert_eq!(payload["users"][0]["email"], "alice@example.com");
    assert_eq!(
        payload["users"][0]["allowed_models_mode"],
        json!("specific")
    );
    assert_eq!(payload["users"][0]["rate_limit_mode"], json!("system"));
    assert_eq!(
        payload["users"][0]["group_names"],
        json!(["Restricted GPT"])
    );
    assert_eq!(payload["users"][0]["wallet"]["balance"], json!(12.5));
    assert_eq!(
        payload["users"][0]["wallet"]["recharge_balance"],
        json!(10.0)
    );
    assert_eq!(payload["users"][0]["wallet"]["gift_balance"], json!(2.5));
    assert_eq!(
        payload["users"][0]["wallet"]["refundable_balance"],
        json!(10.0)
    );
    assert_eq!(payload["users"][0]["unlimited"], json!(false));
    assert_eq!(payload["users"][0]["api_keys"][0]["key"], "ak-user-live-1");
    assert_eq!(
        payload["users"][0]["api_keys"][0]["key_hash"],
        "hash-user-1"
    );
    assert_eq!(
        payload["users"][0]["api_keys"][0]["is_standalone"],
        json!(false)
    );
    assert_eq!(
        payload["users"][0]["api_keys"][0]["total_tokens"],
        json!(420)
    );
    assert_eq!(
        payload["standalone_keys"][0]["key"],
        json!("ak-standalone-live-1")
    );
    assert_eq!(payload["standalone_keys"][0]["total_tokens"], json!(84));
    assert_eq!(
        payload["standalone_keys"][0]["wallet"]["unlimited"],
        json!(true)
    );
    assert_eq!(payload["standalone_keys"][0].get("is_standalone"), None,);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_system_unavailable_write_routes_locally_with_trusted_admin_principal(
) {
    const DETAIL: &str = "Admin system data unavailable";

    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().fallback(any(move |_request: Request| {
        let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
        async move {
            *upstream_hits_inner.lock().expect("mutex should lock") += 1;
            (StatusCode::OK, Body::from("unexpected upstream hit"))
        }
    }));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(AppState::new().expect("gateway should build"));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let client = reqwest::Client::new();
    let unavailable_paths = [
        "/api/admin/system/config/import",
        "/api/admin/system/users/import",
    ];
    let local_paths = [
        "/api/admin/system/cleanup",
        "/api/admin/system/purge/config",
        "/api/admin/system/purge/users",
        "/api/admin/system/purge/usage",
        "/api/admin/system/purge/audit-logs",
        "/api/admin/system/purge/request-bodies",
        "/api/admin/system/purge/stats",
    ];

    for path in unavailable_paths {
        let response = client
            .post(format!("{gateway_url}{path}"))
            .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
            .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
            .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
            .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
            .json(&json!({}))
            .send()
            .await
            .expect("request should succeed");

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let payload: serde_json::Value = response.json().await.expect("json body should parse");
        assert_eq!(payload["detail"], DETAIL, "path={path}");
    }

    for path in local_paths {
        let response = client
            .post(format!("{gateway_url}{path}"))
            .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
            .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
            .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
            .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
            .json(&json!({}))
            .send()
            .await
            .expect("request should succeed");

        assert_eq!(response.status(), StatusCode::OK, "path={path}");
        let payload: serde_json::Value = response.json().await.expect("json body should parse");
        assert!(
            payload["message"]
                .as_str()
                .is_some_and(|value| !value.is_empty()),
            "path={path}"
        );
    }

    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_system_smtp_test_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().fallback(any(move |_request: Request| {
        let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
        async move {
            *upstream_hits_inner.lock().expect("mutex should lock") += 1;
            (StatusCode::OK, Body::from("unexpected upstream hit"))
        }
    }));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(AppState::new().expect("gateway should build"));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let client = reqwest::Client::new();
    let response = client
        .post(format!("{gateway_url}/api/admin/system/smtp/test"))
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
    assert_eq!(payload["success"], json!(false));
    assert_eq!(
        payload["message"],
        json!("SMTP 配置不完整，请检查 smtp_host, smtp_user, smtp_password, smtp_from_email")
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_system_email_templates_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new()
        .route(
            "/api/admin/system/email/templates",
            any(move |_request: Request| {
                let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
                async move {
                    *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                    (StatusCode::OK, Body::from("unexpected upstream hit"))
                }
            }),
        )
        .route(
            "/api/admin/system/email/templates/verification",
            any(|| async { (StatusCode::OK, Body::from("unexpected upstream hit")) }),
        )
        .route(
            "/api/admin/system/email/templates/verification/preview",
            any(|| async { (StatusCode::OK, Body::from("unexpected upstream hit")) }),
        )
        .route(
            "/api/admin/system/email/templates/verification/reset",
            any(|| async { (StatusCode::OK, Body::from("unexpected upstream hit")) }),
        );

    let data_state = GatewayDataState::disabled().with_system_config_values_for_tests(vec![
        (
            "email_template_verification_subject".to_string(),
            json!("自定义验证码"),
        ),
        (
            "email_template_verification_html".to_string(),
            json!("<div>{{ app_name }} - {{ code }}</div>"),
        ),
        ("smtp_from_name".to_string(), json!("Aether Mail")),
    ]);
    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let client = reqwest::Client::new();

    let list_response = client
        .get(format!("{gateway_url}/api/admin/system/email/templates"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(list_response.status(), StatusCode::OK);
    let list_payload: serde_json::Value = list_response.json().await.expect("json should parse");
    let templates = list_payload["templates"]
        .as_array()
        .expect("templates should be an array");
    let verification = templates
        .iter()
        .find(|item| item["type"] == "verification")
        .expect("verification template should exist");
    assert_eq!(verification["subject"], "自定义验证码");
    assert_eq!(verification["is_custom"], json!(true));

    let detail_response = client
        .get(format!(
            "{gateway_url}/api/admin/system/email/templates/verification"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(detail_response.status(), StatusCode::OK);
    let detail_payload: serde_json::Value =
        detail_response.json().await.expect("json should parse");
    assert_eq!(detail_payload["default_subject"], "验证码");
    assert_eq!(detail_payload["is_custom"], json!(true));

    let update_response = client
        .put(format!(
            "{gateway_url}/api/admin/system/email/templates/verification"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "subject": "新验证码",
            "html": "<div>{{ app_name }}: {{ code }}</div>",
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(update_response.status(), StatusCode::OK);
    let update_payload: serde_json::Value =
        update_response.json().await.expect("json should parse");
    assert_eq!(update_payload["message"], "模板保存成功");

    let preview_response = client
        .post(format!(
            "{gateway_url}/api/admin/system/email/templates/verification/preview"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "code": "654321",
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(preview_response.status(), StatusCode::OK);
    let preview_payload: serde_json::Value =
        preview_response.json().await.expect("json should parse");
    assert_eq!(preview_payload["variables"]["code"], "654321");
    assert!(preview_payload["html"]
        .as_str()
        .is_some_and(|html| html.contains("Aether Mail: 654321")));

    let reset_response = client
        .post(format!(
            "{gateway_url}/api/admin/system/email/templates/verification/reset"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(reset_response.status(), StatusCode::OK);
    let reset_payload: serde_json::Value = reset_response.json().await.expect("json should parse");
    assert_eq!(reset_payload["message"], "模板已重置为默认值");
    assert_eq!(reset_payload["template"]["type"], "verification");

    let final_detail_response = client
        .get(format!(
            "{gateway_url}/api/admin/system/email/templates/verification"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(final_detail_response.status(), StatusCode::OK);
    let final_detail_payload: serde_json::Value = final_detail_response
        .json()
        .await
        .expect("json should parse");
    assert_eq!(final_detail_payload["subject"], "验证码");
    assert_eq!(final_detail_payload["is_custom"], json!(false));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_system_api_formats_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/system/api-formats",
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
        .get(format!("{gateway_url}/api/admin/system/api-formats"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    let formats = payload["formats"]
        .as_array()
        .expect("formats should be an array");
    assert_eq!(formats[0]["value"], "openai:chat");
    assert_eq!(formats[0]["default_path"], "/v1/chat/completions");
    assert!(formats
        .iter()
        .any(|item| item["value"] == "openai:embedding"));
    assert!(formats.iter().any(|item| item["value"] == "openai:rerank"));
    assert!(formats.iter().any(|item| item["value"] == "jina:embedding"));
    assert!(formats.iter().any(|item| item["value"] == "jina:rerank"));
    assert!(formats.iter().any(|item| item["value"] == "gemini:video"));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_system_configs_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/system/configs",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let data_state = GatewayDataState::disabled().with_system_config_values_for_tests(vec![
        ("request_log_level".to_string(), json!("headers")),
        ("smtp_password".to_string(), json!("encrypted-secret")),
        ("site_name".to_string(), json!("Aether Test")),
    ]);
    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/api/admin/system/configs"))
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
    assert!(items
        .iter()
        .any(|item| item["key"] == "request_record_level"));
    assert!(!items.iter().any(|item| item["key"] == "request_log_level"));
    let smtp_password = items
        .iter()
        .find(|item| item["key"] == "smtp_password")
        .expect("smtp_password should exist");
    assert_eq!(smtp_password["value"], serde_json::Value::Null);
    assert_eq!(smtp_password["is_set"], json!(true));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_system_config_detail_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/system/configs/password_policy_level",
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
            "{gateway_url}/api/admin/system/configs/password_policy_level"
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
    assert_eq!(payload["key"], "password_policy_level");
    assert_eq!(payload["value"], "weak");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_system_format_conversion_default_as_disabled() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/system/configs/enable_format_conversion",
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
            "{gateway_url}/api/admin/system/configs/enable_format_conversion"
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
    assert_eq!(payload["key"], "enable_format_conversion");
    assert_eq!(payload["value"], json!(false));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_system_model_directives_default_as_disabled() {
    let gateway = build_router_with_state(AppState::new().expect("gateway should build"));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/system/configs/enable_model_directives"
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
    assert_eq!(payload["key"], "enable_model_directives");
    assert_eq!(payload["value"], json!(false));

    gateway_handle.abort();
}

#[tokio::test]
async fn gateway_validates_chat_pii_redaction_system_config_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/system/configs/module.chat_pii_redaction.cache_ttl_seconds",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let data_state =
        GatewayDataState::disabled()
            .with_system_config_values_for_tests(Vec::<(String, serde_json::Value)>::new());
    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();

    let get_config = |key: &'static str| {
        let client = client.clone();
        let gateway_url = gateway_url.clone();
        async move {
            let response = client
                .get(format!("{gateway_url}/api/admin/system/configs/{key}"))
                .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
                .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
                .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
                .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
                .send()
                .await
                .expect("request should succeed");
            assert_eq!(response.status(), StatusCode::OK, "key={key}");
            response
                .json::<serde_json::Value>()
                .await
                .expect("json body should parse")
        }
    };
    let put_config = |key: &'static str, value: serde_json::Value| {
        let client = client.clone();
        let gateway_url = gateway_url.clone();
        async move {
            client
                .put(format!("{gateway_url}/api/admin/system/configs/{key}"))
                .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
                .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
                .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
                .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
                .json(&json!({ "value": value }))
                .send()
                .await
                .expect("request should succeed")
        }
    };

    assert_eq!(
        get_config("module.chat_pii_redaction.enabled").await["value"],
        json!(false)
    );
    assert_eq!(
        get_config("module.chat_pii_redaction.provider_scope").await["value"],
        json!("selected_providers")
    );
    assert_eq!(
        get_config("module.chat_pii_redaction.inject_model_instruction").await["value"],
        json!(true)
    );
    assert_eq!(
        get_config("module.chat_pii_redaction.cache_ttl_seconds").await["value"],
        json!(300)
    );
    assert_eq!(
        get_config("module.chat_pii_redaction.entities").await["value"],
        json!([
            "email",
            "cn_phone",
            "global_phone",
            "cn_id",
            "payment_card",
            "ipv4",
            "ipv6",
            "api_key",
            "access_token",
            "secret_key",
            "bearer_token",
            "jwt"
        ])
    );

    let enabled_response = put_config("module.chat_pii_redaction.enabled", json!(true)).await;
    assert_eq!(enabled_response.status(), StatusCode::OK);
    let enabled_payload: serde_json::Value = enabled_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(enabled_payload["value"], json!(true));

    let scope_response = put_config(
        "module.chat_pii_redaction.provider_scope",
        json!("all_providers"),
    )
    .await;
    assert_eq!(scope_response.status(), StatusCode::OK);
    let scope_payload: serde_json::Value =
        scope_response.json().await.expect("json body should parse");
    assert_eq!(scope_payload["value"], json!("all_providers"));

    let selected_entities_response = put_config(
        "module.chat_pii_redaction.entities",
        json!(["email", "jwt", "cn_phone"]),
    )
    .await;
    assert_eq!(selected_entities_response.status(), StatusCode::OK);
    let selected_entities_payload: serde_json::Value = selected_entities_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(
        selected_entities_payload["value"],
        json!(["email", "cn_phone", "jwt"])
    );

    let ttl_response = put_config("module.chat_pii_redaction.cache_ttl_seconds", json!(3600)).await;
    assert_eq!(ttl_response.status(), StatusCode::OK);
    let ttl_payload: serde_json::Value = ttl_response.json().await.expect("json body should parse");
    assert_eq!(ttl_payload["value"], json!(3600));

    let instruction_response = put_config(
        "module.chat_pii_redaction.inject_model_instruction",
        json!(false),
    )
    .await;
    assert_eq!(instruction_response.status(), StatusCode::OK);
    let instruction_payload: serde_json::Value = instruction_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(instruction_payload["value"], json!(false));

    let invalid_scope_response = put_config(
        "module.chat_pii_redaction.provider_scope",
        json!("enabled_providers"),
    )
    .await;
    assert_eq!(invalid_scope_response.status(), StatusCode::BAD_REQUEST);

    let invalid_entities_response = put_config(
        "module.chat_pii_redaction.entities",
        json!(["email", "name"]),
    )
    .await;
    assert_eq!(invalid_entities_response.status(), StatusCode::BAD_REQUEST);

    let invalid_ttl_response =
        put_config("module.chat_pii_redaction.cache_ttl_seconds", json!(600)).await;
    assert_eq!(invalid_ttl_response.status(), StatusCode::BAD_REQUEST);

    let invalid_instruction_response = put_config(
        "module.chat_pii_redaction.inject_model_instruction",
        json!("yes"),
    )
    .await;
    assert_eq!(
        invalid_instruction_response.status(),
        StatusCode::BAD_REQUEST
    );

    let enabled_default_response =
        put_config("module.chat_pii_redaction.enabled", serde_json::Value::Null).await;
    assert_eq!(enabled_default_response.status(), StatusCode::OK);
    let enabled_default_payload: serde_json::Value = enabled_default_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(enabled_default_payload["value"], json!(false));

    let scope_default_response = put_config(
        "module.chat_pii_redaction.provider_scope",
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(scope_default_response.status(), StatusCode::OK);
    let scope_default_payload: serde_json::Value = scope_default_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(scope_default_payload["value"], json!("selected_providers"));

    let entities_default_response = put_config(
        "module.chat_pii_redaction.entities",
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(entities_default_response.status(), StatusCode::OK);
    let entities_default_payload: serde_json::Value = entities_default_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(
        entities_default_payload["value"],
        json!([
            "email",
            "cn_phone",
            "global_phone",
            "cn_id",
            "payment_card",
            "ipv4",
            "ipv6",
            "api_key",
            "access_token",
            "secret_key",
            "bearer_token",
            "jwt"
        ])
    );

    let ttl_default_response = put_config(
        "module.chat_pii_redaction.cache_ttl_seconds",
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(ttl_default_response.status(), StatusCode::OK);
    let ttl_default_payload: serde_json::Value = ttl_default_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(ttl_default_payload["value"], json!(300));

    let instruction_default_response = put_config(
        "module.chat_pii_redaction.inject_model_instruction",
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(instruction_default_response.status(), StatusCode::OK);
    let instruction_default_payload: serde_json::Value = instruction_default_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(instruction_default_payload["value"], json!(true));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_system_provider_priority_mode_locally_with_bearer_admin_session() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/system/configs/provider_priority_mode",
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
    let access_token = issue_test_admin_access_token(&state, "device-admin-config").await;
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/system/configs/provider_priority_mode"
        ))
        .header("authorization", format!("Bearer {access_token}"))
        .header("x-client-device-id", "device-admin-config")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["key"], "provider_priority_mode");
    assert_eq!(payload["value"], "provider");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_sets_admin_system_config_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/system/configs/smtp_password",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let data_state =
        GatewayDataState::disabled().with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY);
    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let client = reqwest::Client::new();
    let put_response = client
        .put(format!(
            "{gateway_url}/api/admin/system/configs/smtp_password"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "value": "smtp-secret-123",
            "description": "SMTP password",
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(put_response.status(), StatusCode::OK);
    let put_payload: serde_json::Value = put_response.json().await.expect("json body should parse");
    assert_eq!(put_payload["key"], "smtp_password");
    assert_eq!(put_payload["value"], "********");
    assert!(put_payload["updated_at"].as_str().is_some());

    let get_response = client
        .get(format!(
            "{gateway_url}/api/admin/system/configs/smtp_password"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(get_response.status(), StatusCode::OK);
    let get_payload: serde_json::Value = get_response.json().await.expect("json body should parse");
    assert_eq!(get_payload["value"], serde_json::Value::Null);
    assert_eq!(get_payload["is_set"], json!(true));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_deletes_admin_system_config_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/system/configs/custom_flag",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let data_state = GatewayDataState::disabled()
        .with_system_config_values_for_tests(vec![("custom_flag".to_string(), json!(true))]);
    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let client = reqwest::Client::new();
    let delete_response = client
        .delete(format!(
            "{gateway_url}/api/admin/system/configs/custom_flag"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(delete_response.status(), StatusCode::OK);
    let delete_payload: serde_json::Value = delete_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(delete_payload["message"], "配置项 'custom_flag' 已删除");

    let get_response = client
        .get(format!(
            "{gateway_url}/api/admin/system/configs/custom_flag"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(get_response.status(), StatusCode::NOT_FOUND);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_key_rpm_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/endpoints/rpm/key/key-openai",
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
            "endpoint-openai",
            "provider-openai",
            "openai:chat",
            "https://api.openai.example",
        )],
        vec![
            sample_key("key-openai", "provider-openai", "openai:chat", "sk-test")
                .with_rate_limit_fields(Some(60), None, None, None, None, None, None, None, None),
        ],
    ));
    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("current time should be after epoch")
        .as_secs() as i64;
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::seed(vec![
        sample_recent_key_rpm_candidate(
            "cand-openai-rpm-1",
            "req-openai-rpm-1",
            "endpoint-openai",
            "key-openai",
            now_unix_secs,
            2,
        ),
        sample_recent_key_rpm_candidate(
            "cand-openai-rpm-2",
            "req-openai-rpm-2",
            "endpoint-openai",
            "key-openai",
            now_unix_secs,
            1,
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
            "{gateway_url}/api/admin/endpoints/rpm/key/key-openai"
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
    assert_eq!(payload["key_id"], "key-openai");
    assert_eq!(payload["current_rpm"], 2);
    assert_eq!(payload["rpm_limit"], 60);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_resets_admin_key_rpm_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/endpoints/rpm/key/key-openai",
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
            "endpoint-openai",
            "provider-openai",
            "openai:chat",
            "https://api.openai.example",
        )],
        vec![
            sample_key("key-openai", "provider-openai", "openai:chat", "sk-test")
                .with_rate_limit_fields(Some(60), None, None, None, None, None, None, None, None),
        ],
    ));
    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("current time should be after epoch")
        .as_secs() as i64;
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::seed(vec![
        sample_recent_key_rpm_candidate(
            "cand-openai-rpm-1",
            "req-openai-rpm-1",
            "endpoint-openai",
            "key-openai",
            now_unix_secs,
            2,
        ),
        sample_recent_key_rpm_candidate(
            "cand-openai-rpm-2",
            "req-openai-rpm-2",
            "endpoint-openai",
            "key-openai",
            now_unix_secs,
            1,
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

    let client = reqwest::Client::new();
    let reset_response = client
        .delete(format!(
            "{gateway_url}/api/admin/endpoints/rpm/key/key-openai"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(reset_response.status(), StatusCode::OK);
    let reset_payload: serde_json::Value =
        reset_response.json().await.expect("json body should parse");
    assert_eq!(reset_payload["message"], "RPM 计数已重置");

    let rpm_response = client
        .get(format!(
            "{gateway_url}/api/admin/endpoints/rpm/key/key-openai"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(rpm_response.status(), StatusCode::OK);
    let rpm_payload: serde_json::Value = rpm_response.json().await.expect("json body should parse");
    assert_eq!(rpm_payload["key_id"], "key-openai");
    assert_eq!(rpm_payload["current_rpm"], 0);
    assert_eq!(rpm_payload["rpm_limit"], 60);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}
