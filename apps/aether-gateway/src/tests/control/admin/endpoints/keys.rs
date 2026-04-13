use std::sync::{Arc, Mutex};

use aether_contracts::ExecutionPlan;
use aether_crypto::{
    decrypt_python_fernet_ciphertext, encrypt_python_fernet_plaintext, DEVELOPMENT_ENCRYPTION_KEY,
};
use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
use aether_data_contracts::repository::provider_catalog::ProviderCatalogReadRepository;
use axum::body::Body;
use axum::routing::any;
use axum::{extract::Request, Json, Router};
use http::StatusCode;
use serde_json::json;

use super::super::super::{
    build_router_with_state, build_state_with_execution_runtime_override, sample_endpoint,
    sample_key, sample_provider, start_server, AppState,
};
use crate::constants::{
    GATEWAY_HEADER, TRUSTED_ADMIN_SESSION_ID_HEADER, TRUSTED_ADMIN_USER_ID_HEADER,
    TRUSTED_ADMIN_USER_ROLE_HEADER,
};
use crate::data::GatewayDataState;

#[tokio::test]
async fn gateway_handles_admin_provider_keys_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/endpoints/providers/provider-openai/keys",
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
    key_a.internal_priority = 10;
    key_a.request_count = Some(12);
    key_a.success_count = Some(9);
    key_a.error_count = Some(3);
    key_a.total_response_time_ms = Some(1800);
    key_a.created_at_unix_ms = Some(1_711_000_000);
    key_a.updated_at_unix_secs = Some(1_711_000_100);
    key_a.last_used_at_unix_secs = Some(1_711_000_120);
    key_a.note = Some("primary key".to_string());
    key_a.status_snapshot = Some(json!({
        "oauth": {"code": "none", "requires_reauth": false, "expiring_soon": false},
        "account": {"code": "ok", "blocked": false, "recoverable": false},
        "quota": {"code": "unknown", "exhausted": false}
    }));

    let mut key_b = sample_key(
        "key-openai-b",
        "provider-openai",
        "openai:chat",
        "sk-test-b",
    );
    key_b.internal_priority = 20;
    key_b.request_count = Some(4);
    key_b.success_count = Some(4);
    key_b.error_count = Some(0);
    key_b.total_response_time_ms = Some(400);
    key_b.created_at_unix_ms = Some(1_711_100_000);
    key_b.updated_at_unix_secs = Some(1_711_100_100);

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-openai", "openai", 10)],
        vec![],
        vec![key_a, key_b],
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
            "{gateway_url}/api/admin/endpoints/providers/provider-openai/keys?skip=0&limit=50"
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
    assert_eq!(items[0]["id"], "key-openai-a");
    assert_eq!(items[0]["internal_priority"], 10);
    assert_eq!(items[0]["request_count"], 12);
    assert_eq!(items[0]["success_count"], 9);
    assert_eq!(items[0]["error_count"], 3);
    assert_eq!(items[0]["note"], "primary key");
    assert_eq!(items[0]["api_key_masked"], "sk-test-a***");
    assert_eq!(items[1]["id"], "key-openai-b");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_admin_provider_keys_prefers_upstream_plan_type_over_auth_config() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/endpoints/providers/provider-codex/keys",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let mut provider = sample_provider("provider-codex", "codex", 10);
    provider.provider_type = "codex".to_string();
    let mut key = sample_key(
        "key-codex-oauth",
        "provider-codex",
        "openai:cli",
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
        vec![],
        vec![key],
    ));

    let (_upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_reader_for_tests(
                    provider_catalog_repository,
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/endpoints/providers/provider-codex/keys?skip=0&limit=50"
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
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["oauth_plan_type"], "plus");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_creates_admin_provider_key_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/endpoints/providers/provider-openai/keys",
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
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/endpoints/providers/provider-openai/keys"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "api_formats": ["openai:chat"],
            "api_key": "sk-created-openai",
            "name": "created key",
            "internal_priority": 15,
            "capabilities": {"cache_1h": true},
            "note": "created from rust"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["provider_id"], "provider-openai");
    assert_eq!(payload["name"], "created key");
    assert_eq!(payload["internal_priority"], 15);
    assert_eq!(payload["api_formats"], json!(["openai:chat"]));
    assert_eq!(payload["api_key_masked"], "sk-creat***enai");
    assert_eq!(payload["request_count"], 0);
    assert_eq!(payload["success_count"], 0);
    assert_eq!(payload["error_count"], 0);
    assert_eq!(payload["note"], "created from rust");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    let keys = provider_catalog_repository
        .list_keys_by_provider_ids(&["provider-openai".to_string()])
        .await
        .expect("keys should read");
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].name, "created key");
    assert_eq!(keys[0].auth_type, "api_key");
    assert_eq!(keys[0].internal_priority, 15);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_reveals_admin_provider_key_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/endpoints/keys/key-openai-a/reveal",
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
                    provider_catalog_repository,
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/endpoints/keys/key-openai-a/reveal"
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
    assert_eq!(payload["auth_type"], "api_key");
    assert_eq!(payload["api_key"], "sk-test-a");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_exports_admin_provider_key_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/endpoints/keys/key-kiro-a/export",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let mut key = sample_key(
        "key-kiro-a",
        "provider-kiro",
        "claude:cli",
        "oauth-access-token",
    );
    key.auth_type = "oauth".to_string();
    key.encrypted_auth_config = Some(
        encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            r#"{"provider_type":"kiro","auth_method":"idc","refresh_token":"rt-kiro-123"}"#,
        )
        .expect("auth config ciphertext should build"),
    );
    key.upstream_metadata = Some(json!({"kiro": {"email": "alice@example.com"}}));

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-kiro", "kiro", 10)],
        vec![],
        vec![key],
    ));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_reader_for_tests(
                    provider_catalog_repository,
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/endpoints/keys/key-kiro-a/export"
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
    assert_eq!(payload["provider_type"], "kiro");
    assert_eq!(payload["auth_method"], "idc");
    assert_eq!(payload["refresh_token"], "rt-kiro-123");
    assert_eq!(payload["email"], "alice@example.com");
    assert_eq!(payload["name"], "default");
    assert!(payload.get("exported_at").is_some());
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_clears_admin_provider_key_oauth_invalid_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/endpoints/keys/key-openai-a/clear-oauth-invalid",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let mut key = sample_key(
        "key-openai-a",
        "provider-openai",
        "openai:chat",
        "sk-test-a",
    );
    key.oauth_invalid_at_unix_secs = Some(1_710_000_000);
    key.oauth_invalid_reason = Some("token expired".to_string());

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-openai", "openai", 10)],
        vec![],
        vec![key],
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
            "{gateway_url}/api/admin/endpoints/keys/key-openai-a/clear-oauth-invalid"
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
    assert_eq!(payload["message"], "已清除 OAuth 失效标记");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    let reloaded = provider_catalog_repository
        .list_keys_by_ids(&["key-openai-a".to_string()])
        .await
        .expect("keys should read");
    assert_eq!(reloaded.len(), 1);
    assert_eq!(reloaded[0].oauth_invalid_at_unix_secs, None);
    assert_eq!(reloaded[0].oauth_invalid_reason, None);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_noops_admin_provider_key_oauth_invalid_clear_when_marker_absent() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/endpoints/keys/key-openai-a/clear-oauth-invalid",
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
                    provider_catalog_repository,
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/endpoints/keys/key-openai-a/clear-oauth-invalid"
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
    assert_eq!(payload["message"], "该 Key 当前无失效标记，无需清除");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_updates_admin_provider_key_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/endpoints/keys/key-openai-a",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let mut key = sample_key(
        "key-openai-a",
        "provider-openai",
        "openai:chat",
        "sk-test-a",
    );
    key.learned_rpm_limit = Some(88);
    key.allowed_models = Some(json!(["gpt-4.1"]));
    key.fingerprint = Some(json!({"user_agent": "old-ua"}));

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-openai", "openai", 10)],
        vec![],
        vec![key],
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
        .put(format!(
            "{gateway_url}/api/admin/endpoints/keys/key-openai-a"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "api_key": "sk-updated-openai",
            "name": "updated key",
            "internal_priority": 15,
            "rpm_limit": null,
            "allowed_models": [],
            "note": "updated from rust",
            "is_active": false,
            "fingerprint": {"user_agent": "new-ua"}
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["name"], "updated key");
    assert_eq!(payload["internal_priority"], 15);
    assert_eq!(payload["rpm_limit"], serde_json::Value::Null);
    assert_eq!(payload["learned_rpm_limit"], serde_json::Value::Null);
    assert_eq!(payload["allowed_models"], json!([]));
    assert_eq!(payload["note"], "updated from rust");
    assert_eq!(payload["is_active"], false);
    assert_eq!(payload["fingerprint"]["user_agent"], "new-ua");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    let reloaded = provider_catalog_repository
        .list_keys_by_ids(&["key-openai-a".to_string()])
        .await
        .expect("keys should read");
    assert_eq!(reloaded.len(), 1);
    assert_eq!(reloaded[0].name, "updated key");
    assert_eq!(reloaded[0].internal_priority, 15);
    assert_eq!(reloaded[0].rpm_limit, None);
    assert_eq!(reloaded[0].learned_rpm_limit, None);
    assert_eq!(reloaded[0].allowed_models, None);
    assert_eq!(reloaded[0].note.as_deref(), Some("updated from rust"));
    assert!(!reloaded[0].is_active);
    let decrypted = decrypt_python_fernet_ciphertext(
        DEVELOPMENT_ENCRYPTION_KEY,
        &reloaded[0].encrypted_api_key,
    )
    .expect("ciphertext should decrypt");
    assert_eq!(decrypted, "sk-updated-openai");

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_clears_allowed_models_when_disabling_auto_fetch_on_provider_key_update() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/endpoints/keys/key-openai-a",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let mut key = sample_key(
        "key-openai-a",
        "provider-openai",
        "openai:chat",
        "sk-test-a",
    );
    key.auto_fetch_models = true;
    key.allowed_models = Some(json!(["gpt-5", "gpt-4.1-mini"]));

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-openai", "openai", 10)],
        vec![],
        vec![key],
    ));

    let (_upstream_url, upstream_handle) = start_server(upstream).await;
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
        .put(format!(
            "{gateway_url}/api/admin/endpoints/keys/key-openai-a"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "auto_fetch_models": false
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["auto_fetch_models"], false);
    assert_eq!(payload["allowed_models"], json!([]));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    let reloaded = provider_catalog_repository
        .list_keys_by_ids(&["key-openai-a".to_string()])
        .await
        .expect("keys should read");
    assert_eq!(reloaded.len(), 1);
    assert!(!reloaded[0].auto_fetch_models);
    assert_eq!(reloaded[0].allowed_models, None);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_overwrites_allowed_models_immediately_when_enabling_auto_fetch() {
    let execution_runtime_hits = Arc::new(Mutex::new(0usize));
    let execution_runtime_hits_clone = Arc::clone(&execution_runtime_hits);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| {
            let execution_runtime_hits_inner = Arc::clone(&execution_runtime_hits_clone);
            async move {
                *execution_runtime_hits_inner
                    .lock()
                    .expect("mutex should lock") += 1;
                assert_eq!(plan.url, "https://api.openai.example/v1/models");
                assert_eq!(
                    plan.headers.get("authorization").map(String::as_str),
                    Some("Bearer sk-test-a")
                );
                Json(json!({
                    "request_id": "req-update-key-auto-fetch",
                    "status_code": 200,
                    "headers": {
                        "content-type": "application/json"
                    },
                    "body": {
                        "json_body": {
                            "data": [
                                {"id": "gpt-5"},
                                {"id": "gpt-4.1"},
                                {"id": "gpt-o1"}
                            ]
                        }
                    }
                }))
            }
        }),
    );
    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;

    let mut key = sample_key(
        "key-openai-a",
        "provider-openai",
        "openai:chat",
        "sk-test-a",
    );
    key.auto_fetch_models = false;
    key.allowed_models = Some(json!(["manual-a", "manual-b"]));

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-openai", "openai", 10)],
        vec![sample_endpoint(
            "endpoint-openai-chat",
            "provider-openai",
            "openai:chat",
            "https://api.openai.example",
        )],
        vec![key],
    ));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository.clone(),
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .put(format!(
            "{gateway_url}/api/admin/endpoints/keys/key-openai-a"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "auto_fetch_models": true
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["auto_fetch_models"], true);
    assert_eq!(
        payload["allowed_models"],
        json!(["gpt-4.1", "gpt-5", "gpt-o1"])
    );
    assert_eq!(payload["last_models_fetch_error"], serde_json::Value::Null);
    assert_eq!(
        *execution_runtime_hits.lock().expect("mutex should lock"),
        1
    );

    let reloaded = provider_catalog_repository
        .list_keys_by_ids(&["key-openai-a".to_string()])
        .await
        .expect("keys should read");
    assert_eq!(reloaded.len(), 1);
    assert!(reloaded[0].auto_fetch_models);
    assert_eq!(
        reloaded[0].allowed_models,
        Some(json!(["gpt-4.1", "gpt-5", "gpt-o1"]))
    );
    assert_eq!(reloaded[0].locked_models, None);

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[tokio::test]
async fn gateway_fetches_allowed_models_immediately_when_enabling_auto_fetch_from_empty_state() {
    let execution_runtime_hits = Arc::new(Mutex::new(0usize));
    let execution_runtime_hits_clone = Arc::clone(&execution_runtime_hits);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| {
            let execution_runtime_hits_inner = Arc::clone(&execution_runtime_hits_clone);
            async move {
                *execution_runtime_hits_inner
                    .lock()
                    .expect("mutex should lock") += 1;
                assert_eq!(plan.url, "https://api.openai.example/v1/models");
                Json(json!({
                    "request_id": "req-update-key-auto-fetch-empty",
                    "status_code": 200,
                    "headers": {
                        "content-type": "application/json"
                    },
                    "body": {
                        "json_body": {
                            "data": [
                                {"id": "gpt-5-mini"},
                                {"id": "gpt-4.1-nano"}
                            ]
                        }
                    }
                }))
            }
        }),
    );
    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;

    let mut key = sample_key(
        "key-openai-a",
        "provider-openai",
        "openai:chat",
        "sk-test-a",
    );
    key.auto_fetch_models = false;
    key.allowed_models = None;

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-openai", "openai", 10)],
        vec![sample_endpoint(
            "endpoint-openai-chat",
            "provider-openai",
            "openai:chat",
            "https://api.openai.example",
        )],
        vec![key],
    ));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository.clone(),
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .put(format!(
            "{gateway_url}/api/admin/endpoints/keys/key-openai-a"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "auto_fetch_models": true
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["auto_fetch_models"], true);
    assert_eq!(
        payload["allowed_models"],
        json!(["gpt-4.1-nano", "gpt-5-mini"])
    );
    assert_eq!(
        *execution_runtime_hits.lock().expect("mutex should lock"),
        1
    );

    let reloaded = provider_catalog_repository
        .list_keys_by_ids(&["key-openai-a".to_string()])
        .await
        .expect("keys should read");
    assert_eq!(reloaded.len(), 1);
    assert!(reloaded[0].auto_fetch_models);
    assert_eq!(
        reloaded[0].allowed_models,
        Some(json!(["gpt-4.1-nano", "gpt-5-mini"]))
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[tokio::test]
async fn gateway_rejects_admin_provider_key_update_when_api_key_duplicates_existing_key() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/endpoints/keys/key-openai-a",
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
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository,
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .put(format!(
            "{gateway_url}/api/admin/endpoints/keys/key-openai-a"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "api_key": "sk-test-b"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert!(payload["detail"]
        .as_str()
        .expect("detail should be string")
        .contains("该 API Key 已存在于当前 Provider 中"));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_deletes_admin_provider_key_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/endpoints/keys/key-openai-a",
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
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .delete(format!(
            "{gateway_url}/api/admin/endpoints/keys/key-openai-a"
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
    assert_eq!(payload["message"], "Key key-openai-a 已删除");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    let reloaded = provider_catalog_repository
        .list_keys_by_ids(&["key-openai-a".to_string()])
        .await
        .expect("keys should read");
    assert!(reloaded.is_empty());

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_batch_deletes_admin_provider_keys_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/endpoints/keys/batch-delete",
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
            "{gateway_url}/api/admin/endpoints/keys/batch-delete"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "ids": ["key-openai-a", "key-missing"]
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success_count"], 1);
    assert_eq!(payload["failed_count"], 1);
    assert_eq!(payload["failed"][0]["id"], "key-missing");
    assert_eq!(payload["failed"][0]["error"], "not found");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    let reloaded = provider_catalog_repository
        .list_keys_by_provider_ids(&["provider-openai".to_string()])
        .await
        .expect("keys should read");
    assert_eq!(reloaded.len(), 1);
    assert_eq!(reloaded[0].id, "key-openai-b");

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_keys_grouped_by_format_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/endpoints/keys/grouped-by-format",
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
    key_a.internal_priority = 10;
    key_a.request_count = Some(12);
    key_a.success_count = Some(9);
    key_a.created_at_unix_ms = Some(1_711_000_000);
    key_a.updated_at_unix_secs = Some(1_711_000_100);
    key_a.capabilities = Some(json!({"cache_1h": true, "gemini_files": false}));
    key_a.global_priority_by_format = Some(json!({"openai:chat": 3}));
    key_a.health_by_format = Some(json!({"openai:chat": {"health_score": 0.8}}));
    key_a.circuit_breaker_by_format = Some(json!({"openai:chat": {"open": false}}));

    let mut key_b = sample_key("key-claude-a", "provider-claude", "claude:chat", "sk-ant-a");
    key_b.internal_priority = 20;
    key_b.request_count = Some(2);
    key_b.success_count = Some(1);
    key_b.created_at_unix_ms = Some(1_711_100_000);
    key_b.updated_at_unix_secs = Some(1_711_100_100);

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![
            sample_provider("provider-openai", "openai", 10),
            sample_provider("provider-claude", "claude", 20)
                .with_transport_fields(false, false, true, None, None, None, None, None, None),
        ],
        vec![
            sample_endpoint(
                "endpoint-openai-chat",
                "provider-openai",
                "openai:chat",
                "https://api.openai.example",
            ),
            sample_endpoint(
                "endpoint-claude-chat",
                "provider-claude",
                "claude:chat",
                "https://api.claude.example",
            ),
        ],
        vec![key_a, key_b],
    ));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_reader_for_tests(
                    provider_catalog_repository,
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/endpoints/keys/grouped-by-format"
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
    assert_eq!(payload["openai:chat"][0]["id"], "key-openai-a");
    assert_eq!(payload["openai:chat"][0]["provider_name"], "openai");
    assert_eq!(
        payload["openai:chat"][0]["endpoint_base_url"],
        "https://api.openai.example"
    );
    assert_eq!(payload["openai:chat"][0]["capabilities"], json!(["1h缓存"]));
    assert_eq!(payload["claude:chat"][0]["provider_active"], false);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}
