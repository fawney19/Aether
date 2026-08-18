use std::sync::{Arc, Mutex};

use aether_crypto::{encrypt_python_fernet_plaintext, DEVELOPMENT_ENCRYPTION_KEY};
use aether_data::repository::auth::{
    AuthApiKeyLookupKey, AuthApiKeyReadRepository, AuthApiKeyWriteRepository,
    InMemoryAuthApiKeySnapshotRepository, StoredAuthApiKeyExportRecord, StoredAuthApiKeySnapshot,
};
use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
use aether_data::repository::usage::InMemoryUsageReadRepository;
use aether_data::repository::users::{
    InMemoryUserReadRepository, StoredUserExportRow, UpsertUserGroupRecord, UserReadRepository,
};
use aether_data::repository::wallet::{InMemoryWalletRepository, StoredWalletSnapshot};
use aether_data_contracts::repository::usage::StoredRequestUsageAudit;
use axum::body::Body;
use axum::routing::any;
use axum::{extract::Request, Router};
use http::StatusCode;
use serde_json::json;

use super::super::{build_router_with_state, start_server, AppState};
use crate::constants::{
    GATEWAY_HEADER, TRUSTED_ADMIN_SESSION_ID_HEADER, TRUSTED_ADMIN_USER_ID_HEADER,
    TRUSTED_ADMIN_USER_ROLE_HEADER,
};
use crate::data::GatewayDataState;

fn admin_request(builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    builder
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
}

async fn start_api_keys_upstream(
    path: &'static str,
) -> (String, Arc<Mutex<usize>>, tokio::task::JoinHandle<()>) {
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
    (upstream_url, upstream_hits, upstream_handle)
}

fn sample_standalone_api_key_snapshot(
    api_key_id: &str,
    user_id: &str,
    is_active: bool,
) -> StoredAuthApiKeySnapshot {
    StoredAuthApiKeySnapshot::new(
        user_id.to_string(),
        "standalone-owner".to_string(),
        Some("owner@example.com".to_string()),
        "admin".to_string(),
        "local".to_string(),
        true,
        false,
        None,
        None,
        None,
        api_key_id.to_string(),
        Some(format!("key-{api_key_id}")),
        is_active,
        false,
        true,
        Some(120),
        Some(5),
        Some(4_102_444_800),
        Some(json!(["openai"])),
        Some(json!(["openai:chat"])),
        Some(json!(["gpt-4.1"])),
    )
    .expect("snapshot should build")
}

fn sample_standalone_export_record(
    api_key_id: &str,
    user_id: &str,
    plaintext_key: &str,
    is_active: bool,
) -> StoredAuthApiKeyExportRecord {
    let mut record = StoredAuthApiKeyExportRecord::new(
        user_id.to_string(),
        api_key_id.to_string(),
        format!("hash-{api_key_id}"),
        Some(
            encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, plaintext_key)
                .expect("key should encrypt"),
        ),
        Some(format!("key-{api_key_id}")),
        Some(json!(["openai"])),
        Some(json!(["openai:chat"])),
        Some(json!(["gpt-4.1"])),
        Some(120),
        Some(5),
        None,
        is_active,
        Some(4_102_444_800),
        false,
        7,
        0,
        1.25,
        true,
    )
    .expect("export record should build");
    record.created_at_unix_secs = Some(1_711_000_100);
    record.updated_at_unix_secs = Some(1_711_000_101);
    record.last_used_at_unix_secs = Some(1_711_000_102);
    record
}

fn sample_standalone_wallet(api_key_id: &str) -> StoredWalletSnapshot {
    StoredWalletSnapshot::new(
        format!("wallet-{api_key_id}"),
        None,
        Some(api_key_id.to_string()),
        18.5,
        1.5,
        "unlimited".to_string(),
        "USD".to_string(),
        "active".to_string(),
        30.0,
        2.0,
        0.0,
        0.0,
        1_710_000_000,
    )
    .expect("wallet should build")
}

fn sample_usage_row(
    id: &str,
    request_id: &str,
    api_key_id: &str,
    total_tokens: i32,
) -> StoredRequestUsageAudit {
    StoredRequestUsageAudit::new(
        id.to_string(),
        request_id.to_string(),
        Some("user-1".to_string()),
        Some(api_key_id.to_string()),
        Some("user-user-1".to_string()),
        Some(format!("key-{api_key_id}")),
        "openai".to_string(),
        "gpt-4.1".to_string(),
        Some("gpt-4.1".to_string()),
        Some("provider-1".to_string()),
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
        total_tokens - (total_tokens / 2),
        total_tokens,
        0.42,
        0.42,
        Some(200),
        None,
        None,
        Some(240),
        Some(80),
        "completed".to_string(),
        "settled".to_string(),
        1_711_000_000,
        1_711_000_001,
        Some(1_711_000_002),
    )
    .expect("usage row should build")
}

#[tokio::test]
async fn gateway_handles_admin_api_keys_list_locally_with_trusted_admin_principal() {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_api_keys_upstream("/api/admin/api-keys").await;
    let auth_repository = Arc::new(
        InMemoryAuthApiKeySnapshotRepository::seed(vec![
            (
                None,
                sample_standalone_api_key_snapshot("key-1", "user-1", true),
            ),
            (
                None,
                sample_standalone_api_key_snapshot("key-2", "user-2", false),
            ),
        ])
        .with_export_records([
            sample_standalone_export_record("key-1", "user-1", "sk-key-1-plaintext", true),
            sample_standalone_export_record("key-2", "user-2", "sk-key-2-plaintext", false),
        ]),
    );
    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![
        sample_usage_row("usage-1", "req-1", "key-1", 90),
        sample_usage_row("usage-2", "req-2", "key-2", 120),
    ]));
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_auth_wallet_and_usage_for_tests(
                    auth_repository,
                    Arc::new(InMemoryWalletRepository::seed(vec![
                        sample_standalone_wallet("key-1"),
                    ])),
                    usage_repository,
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(reqwest::Client::new().get(format!(
        "{gateway_url}/api/admin/api-keys?is_active=true&limit=10&skip=0"
    )))
    .send()
    .await
    .expect("request should succeed");

    let status = response.status();
    let body = response
        .text()
        .await
        .expect("response body should be readable");
    assert_eq!(status, StatusCode::OK, "unexpected response body: {body}");
    let payload: serde_json::Value = serde_json::from_str(&body).expect("json body should parse");
    assert_eq!(payload["total"], json!(1));
    assert_eq!(payload["limit"], json!(10));
    assert_eq!(payload["skip"], json!(0));
    assert_eq!(payload["api_keys"][0]["id"], json!("key-1"));
    assert_eq!(payload["api_keys"][0]["is_standalone"], json!(true));
    assert_eq!(
        payload["api_keys"][0]["key_display"],
        json!("sk-key-1-p...text")
    );
    assert_eq!(payload["api_keys"][0]["total_requests"], json!(7));
    assert_eq!(payload["api_keys"][0]["total_tokens"], json!(0));
    assert_eq!(
        payload["api_keys"][0]["created_at"],
        json!("2024-03-21T05:48:20+00:00")
    );
    assert_eq!(
        payload["api_keys"][0]["last_used_at"],
        json!("2024-03-21T05:48:22+00:00")
    );
    assert_eq!(
        payload["api_keys"][0]["wallet"]["id"],
        json!("wallet-key-1")
    );
    assert_eq!(payload["api_keys"][0]["wallet"]["balance"], json!(20.0));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_api_keys_detail_locally_with_trusted_admin_principal() {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_api_keys_upstream("/api/admin/api-keys/key-1").await;
    let mut export = sample_standalone_export_record("key-1", "user-1", "sk-key-1-plaintext", true);
    export.total_tokens = 77;
    let auth_repository = Arc::new(
        InMemoryAuthApiKeySnapshotRepository::seed(vec![(
            None,
            sample_standalone_api_key_snapshot("key-1", "user-1", true),
        )])
        .with_export_records([export]),
    );
    let wallet_repository = Arc::new(InMemoryWalletRepository::seed(vec![
        sample_standalone_wallet("key-1"),
    ]));
    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![sample_usage_row(
        "usage-1", "req-1", "key-1", 150,
    )]));
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_auth_wallet_and_usage_for_tests(
                    auth_repository,
                    wallet_repository,
                    usage_repository,
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(
        reqwest::Client::new().get(format!("{gateway_url}/api/admin/api-keys/key-1")),
    )
    .send()
    .await
    .expect("request should succeed");

    let status = response.status();
    let body = response
        .text()
        .await
        .expect("response body should be readable");
    assert_eq!(status, StatusCode::OK, "unexpected response body: {body}");
    let payload: serde_json::Value = serde_json::from_str(&body).expect("json body should parse");
    assert_eq!(payload["id"], json!("key-1"));
    assert_eq!(payload["user_id"], json!("user-1"));
    assert_eq!(payload["wallet"]["id"], json!("wallet-key-1"));
    assert_eq!(payload["wallet"]["unlimited"], json!(true));
    assert_eq!(payload["wallet"]["balance"], json!(20.0));
    assert_eq!(payload["key_display"], json!("sk-key-1-p...text"));
    assert_eq!(payload["total_tokens"], json!(77));
    assert_eq!(payload["created_at"], json!("2024-03-21T05:48:20+00:00"));
    assert_eq!(payload["last_used_at"], json!("2024-03-21T05:48:22+00:00"));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_includes_usage_summary_when_admin_api_keys_list_requests_it() {
    let (_upstream_url, _upstream_hits, upstream_handle) =
        start_api_keys_upstream("/api/admin/api-keys").await;
    let mut export = sample_standalone_export_record("key-1", "user-1", "sk-key-1-plaintext", true);
    export.total_tokens = 42;
    let auth_repository = Arc::new(
        InMemoryAuthApiKeySnapshotRepository::seed(vec![(
            None,
            sample_standalone_api_key_snapshot("key-1", "user-1", true),
        )])
        .with_export_records([export]),
    );
    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![sample_usage_row(
        "usage-1", "req-1", "key-1", 90,
    )]));
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_auth_wallet_and_usage_for_tests(
                    auth_repository,
                    Arc::new(InMemoryWalletRepository::seed(
                        Vec::<StoredWalletSnapshot>::new(),
                    )),
                    usage_repository,
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(reqwest::Client::new().get(format!(
        "{gateway_url}/api/admin/api-keys?include_usage_summary=true"
    )))
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["api_keys"][0]["total_tokens"], json!(42));

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_api_keys_full_key_locally_with_trusted_admin_principal() {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_api_keys_upstream("/api/admin/api-keys/key-1").await;
    let auth_repository = Arc::new(
        InMemoryAuthApiKeySnapshotRepository::seed(vec![(
            None,
            sample_standalone_api_key_snapshot("key-1", "user-1", true),
        )])
        .with_export_records([sample_standalone_export_record(
            "key-1",
            "user-1",
            "sk-key-1-plaintext",
            true,
        )]),
    );
    let wallet_repository = Arc::new(InMemoryWalletRepository::seed(vec![
        sample_standalone_wallet("key-1"),
    ]));
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_auth_and_wallet_for_tests(
                    auth_repository,
                    wallet_repository,
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(reqwest::Client::new().get(format!(
        "{gateway_url}/api/admin/api-keys/key-1?include_key=true"
    )))
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload, json!({ "key": "sk-key-1-plaintext" }));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_api_key_install_session_locally_with_trusted_admin_principal() {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_api_keys_upstream("/api/admin/api-keys/key-1/install-sessions").await;
    let auth_repository = Arc::new(
        InMemoryAuthApiKeySnapshotRepository::seed(vec![(
            None,
            sample_standalone_api_key_snapshot("key-1", "user-1", true),
        )])
        .with_export_records([sample_standalone_export_record(
            "key-1",
            "user-1",
            "sk-key-1-plaintext",
            true,
        )]),
    );
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_auth_api_key_repository_for_tests(auth_repository)
                    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(reqwest::Client::new().post(format!(
        "{gateway_url}/api/admin/api-keys/key-1/install-sessions/"
    )))
    .header("x-forwarded-host", "aether.example")
    .header("x-forwarded-proto", "https")
    .json(&json!({
        "target_cli": "codex_cli",
        "target_system": "linux",
    }))
    .send()
    .await
    .expect("request should succeed");

    let status = response.status();
    let body = response
        .text()
        .await
        .expect("response body should be readable");
    assert_eq!(status, StatusCode::OK, "unexpected response body: {body}");
    let payload: serde_json::Value = serde_json::from_str(&body).expect("json body should parse");
    let install_code = payload["install_code"]
        .as_str()
        .expect("install code should be returned");
    assert_eq!(install_code.len(), 24);
    assert_eq!(payload["expires_in_seconds"], json!(15 * 60));
    assert_eq!(payload["target_cli"], json!("codex_cli"));
    assert_eq!(payload["target_system"], json!("linux"));
    assert!(payload["expires_at_unix_secs"].is_number());
    assert_eq!(
        payload["unix_command"],
        json!(format!(
            "curl -fsSL https://aether.example/install/{install_code} | sh"
        ))
    );
    assert_eq!(
        payload["powershell_command"],
        json!(format!(
            "irm https://aether.example/install/{install_code}.ps1 | iex"
        ))
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_api_keys_create_locally_with_trusted_admin_principal() {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_api_keys_upstream("/api/admin/api-keys").await;
    let repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(Vec::<(
        Option<String>,
        StoredAuthApiKeySnapshot,
    )>::new()));
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_auth_api_key_repository_for_tests(Arc::clone(
                    &repository,
                ))
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response =
        admin_request(reqwest::Client::new().post(format!("{gateway_url}/api/admin/api-keys")))
            .json(&json!({
                "name": "standalone-key",
                "rate_limit": null,
                "allowed_providers": ["openai"],
                "allowed_api_formats": ["openai:chat"],
                "allowed_models": ["gpt-4.1"],
                "initial_balance_usd": 12.5,
                "expires_at": "2030-01-02",
                "auto_delete_on_expiry": true,
            }))
            .send()
            .await
            .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["name"], json!("standalone-key"));
    assert_eq!(payload["is_standalone"], json!(true));
    assert_eq!(payload["rate_limit"], serde_json::Value::Null);
    assert_eq!(payload["concurrent_limit"], serde_json::Value::Null);
    assert_eq!(payload["allowed_providers"], json!(["openai"]));
    assert_eq!(payload["allowed_api_formats"], json!(["openai:chat"]));
    assert_eq!(payload["allowed_models"], json!(["gpt-4.1"]));
    assert_eq!(payload["auto_delete_on_expiry"], json!(true));
    assert_eq!(payload["wallet"]["balance"], json!(12.5));
    assert_eq!(payload["wallet"]["limit_mode"], json!("finite"));
    assert_eq!(payload["wallet"]["unlimited"], json!(false));
    assert!(payload["expires_at"]
        .as_str()
        .expect("expires_at should exist")
        .starts_with("2030-01-02"));
    let plaintext = payload["key"]
        .as_str()
        .expect("plaintext key should exist")
        .to_string();
    assert!(plaintext.starts_with("sk-"));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    let list_response =
        admin_request(reqwest::Client::new().get(format!("{gateway_url}/api/admin/api-keys")))
            .send()
            .await
            .expect("list request should succeed");
    assert_eq!(list_response.status(), StatusCode::OK);
    let list_payload: serde_json::Value =
        list_response.json().await.expect("list json should parse");
    assert_eq!(list_payload["total"], json!(1));
    assert_eq!(list_payload["api_keys"][0]["name"], json!("standalone-key"));

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_api_keys_update_locally_with_trusted_admin_principal() {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_api_keys_upstream("/api/admin/api-keys/key-123").await;
    let repository = Arc::new(
        InMemoryAuthApiKeySnapshotRepository::seed(vec![(
            Some("hash-key-123".to_string()),
            sample_standalone_api_key_snapshot("key-123", "admin-user-123", true),
        )])
        .with_export_records([sample_standalone_export_record(
            "key-123",
            "admin-user-123",
            "sk-key-123-plaintext",
            true,
        )]),
    );
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_auth_wallets_for_tests([sample_standalone_wallet("key-123")])
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_auth_api_key_repository_for_tests(Arc::clone(
                    &repository,
                ))
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(
        reqwest::Client::new().put(format!("{gateway_url}/api/admin/api-keys/key-123")),
    )
    .json(&json!({
        "name": "renamed-key",
        "rate_limit": null,
        "concurrent_limit": 12,
        "allowed_providers": ["gemini"],
        "allowed_api_formats": ["gemini:generate_content"],
        "allowed_models": ["gemini-2.5-pro"],
        "expires_at": "2030-03-04",
        "auto_delete_on_expiry": true,
        "unlimited_balance": true,
    }))
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["id"], json!("key-123"));
    assert_eq!(payload["name"], json!("renamed-key"));
    assert_eq!(payload["rate_limit"], serde_json::Value::Null);
    assert_eq!(payload["concurrent_limit"], json!(12));
    assert_eq!(payload["allowed_providers"], json!(["gemini"]));
    assert_eq!(
        payload["allowed_api_formats"],
        json!(["gemini:generate_content"])
    );
    assert_eq!(payload["allowed_models"], json!(["gemini-2.5-pro"]));
    assert_eq!(payload["auto_delete_on_expiry"], json!(true));
    assert_eq!(payload["wallet"]["limit_mode"], json!("unlimited"));
    assert_eq!(payload["wallet"]["unlimited"], json!(true));
    assert!(payload["expires_at"]
        .as_str()
        .expect("expires_at should exist")
        .starts_with("2030-03-04"));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_api_keys_toggle_locally_with_trusted_admin_principal() {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_api_keys_upstream("/api/admin/api-keys/key-123").await;
    let repository = Arc::new(
        InMemoryAuthApiKeySnapshotRepository::seed(vec![(
            Some("hash-key-123".to_string()),
            sample_standalone_api_key_snapshot("key-123", "admin-user-123", true),
        )])
        .with_export_records([sample_standalone_export_record(
            "key-123",
            "admin-user-123",
            "sk-key-123-plaintext",
            true,
        )]),
    );
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_auth_api_key_repository_for_tests(Arc::clone(
                    &repository,
                ))
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(
        reqwest::Client::new().patch(format!("{gateway_url}/api/admin/api-keys/key-123")),
    )
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["id"], json!("key-123"));
    assert_eq!(payload["is_active"], json!(false));
    assert_eq!(payload["message"], json!("API密钥已禁用"));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_api_keys_delete_locally_with_trusted_admin_principal() {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_api_keys_upstream("/api/admin/api-keys/key-123").await;
    let repository = Arc::new(
        InMemoryAuthApiKeySnapshotRepository::seed(vec![(
            Some("hash-key-123".to_string()),
            sample_standalone_api_key_snapshot("key-123", "admin-user-123", true),
        )])
        .with_export_records([sample_standalone_export_record(
            "key-123",
            "admin-user-123",
            "sk-key-123-plaintext",
            true,
        )]),
    );
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_auth_api_key_repository_for_tests(Arc::clone(
                    &repository,
                ))
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(
        reqwest::Client::new().delete(format!("{gateway_url}/api/admin/api-keys/key-123")),
    )
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["message"], json!("API密钥已删除"));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    let detail_response = admin_request(
        reqwest::Client::new().get(format!("{gateway_url}/api/admin/api-keys/key-123")),
    )
    .send()
    .await
    .expect("detail request should succeed");
    assert_eq!(detail_response.status(), StatusCode::NOT_FOUND);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_admin_standalone_api_key_provider_key_scope_round_trip() {
    let (upstream_url, _upstream_hits, upstream_handle) =
        start_api_keys_upstream("/api/admin/api-keys").await;
    let repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(Vec::<(
        Option<String>,
        StoredAuthApiKeySnapshot,
    )>::new()));
    let provider = super::super::helpers::sample_provider("provider-1", "Provider One", 10);
    let other_provider = super::super::helpers::sample_provider("provider-2", "Provider Two", 20);
    let catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider, other_provider],
        Vec::new(),
        vec![
            super::super::helpers::sample_key("key-a", "provider-1", "openai:chat", "secret-a"),
            super::super::helpers::sample_key("key-b", "provider-1", "openai:chat", "secret-b"),
            super::super::helpers::sample_key("key-c", "provider-2", "openai:chat", "secret-c"),
        ],
    ));
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_auth_api_key_repository_for_tests(Arc::clone(
                    &repository,
                ))
                .with_provider_catalog_reader(catalog)
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    // Create with a provider-scoped key allowlist.
    let response =
        admin_request(reqwest::Client::new().post(format!("{gateway_url}/api/admin/api-keys")))
            .json(&json!({
                "name": "scoped-key",
                "initial_balance_usd": 5.0,
                "allowed_providers": ["provider-1"],
                "allowed_provider_key_ids": { "provider-1": ["key-a"] },
                "allowed_api_formats": ["openai:chat"],
            }))
            .send()
            .await
            .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["allowed_providers"], json!(["provider-1"]));
    assert_eq!(
        payload["allowed_provider_key_ids"],
        json!({ "provider-1": ["key-a"] })
    );

    // The list endpoint echoes the scope back.
    let list_response =
        admin_request(reqwest::Client::new().get(format!("{gateway_url}/api/admin/api-keys")))
            .send()
            .await
            .expect("list request should succeed");
    assert_eq!(list_response.status(), StatusCode::OK);
    let list_payload: serde_json::Value =
        list_response.json().await.expect("list json should parse");
    assert_eq!(
        list_payload["api_keys"][0]["allowed_provider_key_ids"],
        json!({ "provider-1": ["key-a"] })
    );

    // A key belonging to a different provider is rejected.
    let bad_provider_response =
        admin_request(reqwest::Client::new().post(format!("{gateway_url}/api/admin/api-keys")))
            .json(&json!({
                "name": "bad-scope",
                "initial_balance_usd": 5.0,
                "allowed_providers": ["provider-1"],
                "allowed_provider_key_ids": { "provider-1": ["key-c"] },
            }))
            .send()
            .await
            .expect("request should succeed");
    assert_eq!(bad_provider_response.status(), StatusCode::BAD_REQUEST);

    // A scope without a provider allowlist is rejected.
    let bad_provider_response =
        admin_request(reqwest::Client::new().post(format!("{gateway_url}/api/admin/api-keys")))
            .json(&json!({
                "name": "bad-scope",
                "initial_balance_usd": 5.0,
                "allowed_provider_key_ids": { "provider-1": ["key-a"] },
            }))
            .send()
            .await
            .expect("request should succeed");
    assert_eq!(bad_provider_response.status(), StatusCode::BAD_REQUEST);

    // A scope referencing a missing key is rejected.
    let bad_key_response =
        admin_request(reqwest::Client::new().post(format!("{gateway_url}/api/admin/api-keys")))
            .json(&json!({
                "name": "bad-scope",
                "initial_balance_usd": 5.0,
                "allowed_providers": ["provider-1"],
                "allowed_provider_key_ids": { "provider-1": ["missing-key"] },
            }))
            .send()
            .await
            .expect("request should succeed");
    assert_eq!(bad_key_response.status(), StatusCode::BAD_REQUEST);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_admin_standalone_api_key_direct_null_allowlist_clears_scope() {
    let (_upstream_url, _upstream_hits, upstream_handle) =
        start_api_keys_upstream("/api/admin/api-keys/key-123").await;
    let mut snapshot = sample_standalone_api_key_snapshot("key-123", "admin-user-123", true);
    snapshot.api_key_allowed_providers = Some(vec!["provider-1".to_string()]);
    snapshot.api_key_allowed_provider_key_ids =
        Some([("provider-1".to_string(), ["key-a".to_string()].into())].into());
    let mut export =
        sample_standalone_export_record("key-123", "admin-user-123", "sk-key-123-plaintext", true);
    export.allowed_providers = Some(vec!["provider-1".to_string()]);
    export.allowed_provider_key_ids =
        Some([("provider-1".to_string(), ["key-a".to_string()].into())].into());
    let repository = Arc::new(
        InMemoryAuthApiKeySnapshotRepository::seed(vec![(
            Some("hash-key-123".to_string()),
            snapshot,
        )])
        .with_export_records([export]),
    );
    let provider = super::super::helpers::sample_provider("provider-1", "Provider One", 10);
    let catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        vec![super::super::helpers::sample_key(
            "key-a",
            "provider-1",
            "openai:chat",
            "secret-a",
        )],
    ));
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_auth_wallets_for_tests([sample_standalone_wallet("key-123")])
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_auth_api_key_repository_for_tests(Arc::clone(
                    &repository,
                ))
                .with_provider_catalog_reader(catalog)
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    // The first and only update explicitly clears the provider allowlist.
    let response = admin_request(
        reqwest::Client::new().put(format!("{gateway_url}/api/admin/api-keys/key-123")),
    )
    .json(&json!({ "allowed_providers": null }))
    .send()
    .await
    .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["allowed_providers"], serde_json::Value::Null);
    assert_eq!(payload["allowed_provider_key_ids"], serde_json::Value::Null);

    let stored_snapshot = repository
        .find_api_key_snapshot(AuthApiKeyLookupKey::ApiKeyId("key-123"))
        .await
        .expect("snapshot lookup should succeed")
        .expect("snapshot should exist");
    assert_eq!(stored_snapshot.api_key_allowed_provider_key_ids, None);
    let stored_export = repository
        .find_export_standalone_api_key_by_id("key-123")
        .await
        .expect("export lookup should succeed")
        .expect("export record should exist");
    assert_eq!(stored_export.allowed_provider_key_ids, None);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_admin_standalone_api_key_patch_canonicalizes_scope_against_new_allowlist() {
    let (upstream_url, _upstream_hits, upstream_handle) =
        start_api_keys_upstream("/api/admin/api-keys/key-123").await;
    let mut snapshot = sample_standalone_api_key_snapshot("key-123", "admin-user-123", true);
    snapshot.api_key_allowed_providers = Some(vec!["provider-1".to_string()]);
    snapshot.api_key_allowed_provider_key_ids =
        Some([("provider-1".to_string(), ["key-a".to_string()].into())].into());
    let mut export =
        sample_standalone_export_record("key-123", "admin-user-123", "sk-key-123-plaintext", true);
    export.allowed_providers = Some(vec!["provider-1".to_string()]);
    export.allowed_provider_key_ids =
        Some([("provider-1".to_string(), ["key-a".to_string()].into())].into());
    let repository = Arc::new(
        InMemoryAuthApiKeySnapshotRepository::seed(vec![(
            Some("hash-key-123".to_string()),
            snapshot,
        )])
        .with_export_records([export]),
    );
    let provider = super::super::helpers::sample_provider("provider-1", "Provider One", 10);
    let other_provider = super::super::helpers::sample_provider("provider-2", "Provider Two", 20);
    let catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider, other_provider],
        Vec::new(),
        vec![
            super::super::helpers::sample_key("key-a", "provider-1", "openai:chat", "secret-a"),
            super::super::helpers::sample_key("key-b", "provider-2", "openai:chat", "secret-b"),
        ],
    ));
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_auth_wallets_for_tests([sample_standalone_wallet("key-123")])
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_auth_api_key_repository_for_tests(Arc::clone(
                    &repository,
                ))
                .with_provider_catalog_reader(catalog)
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    // PATCH that changes the allowlist to a provider whose keys are not in the
    // stored scope: the stored scope must be canonicalized away (no stale
    // P1 -> K1 entry survives), even though the scope field is absent.
    let response = admin_request(
        reqwest::Client::new().put(format!("{gateway_url}/api/admin/api-keys/key-123")),
    )
    .json(&json!({
        "allowed_providers": ["provider-2"],
    }))
    .send()
    .await
    .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["allowed_providers"], json!(["provider-2"]));
    assert_eq!(payload["allowed_provider_key_ids"], serde_json::Value::Null);

    // PATCH that switches to unrestricted providers clears the scope too.
    let response = admin_request(
        reqwest::Client::new().put(format!("{gateway_url}/api/admin/api-keys/key-123")),
    )
    .json(&json!({
        "allowed_providers": null,
    }))
    .send()
    .await
    .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["allowed_provider_key_ids"], serde_json::Value::Null);

    // PATCH that keeps the scope consistent with the allowlist preserves it.
    let response = admin_request(
        reqwest::Client::new().put(format!("{gateway_url}/api/admin/api-keys/key-123")),
    )
    .json(&json!({
        "allowed_providers": ["provider-2"],
        "allowed_provider_key_ids": { "provider-2": ["key-b"] },
    }))
    .send()
    .await
    .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(
        payload["allowed_provider_key_ids"],
        json!({ "provider-2": ["key-b"] })
    );

    // A later name-only PATCH keeps the scope intact.
    let response = admin_request(
        reqwest::Client::new().put(format!("{gateway_url}/api/admin/api-keys/key-123")),
    )
    .json(&json!({
        "name": "renamed",
    }))
    .send()
    .await
    .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(
        payload["allowed_provider_key_ids"],
        json!({ "provider-2": ["key-b"] })
    );

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_disabling_provider_key_preserves_scopes_and_allows_reopen_edit_save() {
    let (upstream_url, _upstream_hits, upstream_handle) =
        start_api_keys_upstream("/api/admin/api-keys/key-123").await;
    let mut snapshot = sample_standalone_api_key_snapshot("key-123", "admin-user-123", true);
    snapshot.api_key_allowed_providers = Some(vec!["provider-1".to_string()]);
    snapshot.api_key_allowed_provider_key_ids =
        Some([("provider-1".to_string(), ["key-a".to_string()].into())].into());
    let mut export =
        sample_standalone_export_record("key-123", "admin-user-123", "sk-key-123-plaintext", true);
    export.allowed_providers = Some(vec!["provider-1".to_string()]);
    export.allowed_provider_key_ids =
        Some([("provider-1".to_string(), ["key-a".to_string()].into())].into());
    let auth_repository = Arc::new(
        InMemoryAuthApiKeySnapshotRepository::seed(vec![(
            Some("hash-key-123".to_string()),
            snapshot,
        )])
        .with_export_records([export]),
    );

    let user_repository = Arc::new(
        InMemoryUserReadRepository::seed_auth_users(vec![])
            .with_export_users(Vec::<StoredUserExportRow>::new()),
    );
    let group = user_repository
        .create_user_group(UpsertUserGroupRecord {
            name: "Scoped Group".to_string(),
            description: None,
            priority: 0,
            allowed_providers: Some(vec!["provider-1".to_string()]),
            allowed_provider_key_ids: Some(
                [("provider-1".to_string(), ["key-a".to_string()].into())].into(),
            ),
            allowed_providers_mode: "specific".to_string(),
            allowed_api_formats: None,
            allowed_api_formats_mode: "inherit".to_string(),
            allowed_models: None,
            allowed_models_mode: "inherit".to_string(),
            rate_limit: None,
            rate_limit_mode: "inherit".to_string(),
        })
        .await
        .expect("group should create")
        .expect("group should exist");
    let group_id = group.id.clone();

    let provider = super::super::helpers::sample_provider("provider-1", "Provider One", 10);
    let catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        vec![
            super::super::helpers::sample_key("key-a", "provider-1", "openai:chat", "secret-a"),
            super::super::helpers::sample_key("key-b", "provider-1", "openai:chat", "secret-b"),
        ],
    ));
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_auth_wallets_for_tests([sample_standalone_wallet("key-123")])
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_auth_api_key_repository_for_tests(Arc::clone(
                    &auth_repository,
                ))
                .with_user_reader(user_repository.clone())
                .attach_provider_catalog_repository_for_tests(catalog)
                .with_system_config_values_for_tests(Vec::<(String, serde_json::Value)>::new())
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    // Disable provider key key-a through the admin API.
    let response = admin_request(
        reqwest::Client::new().put(format!("{gateway_url}/api/admin/endpoints/keys/key-a")),
    )
    .json(&json!({ "is_active": false }))
    .send()
    .await
    .expect("request should succeed");
    if response.status() != StatusCode::OK {
        let body: serde_json::Value = response.json().await.expect("json body should parse");
        panic!("disable failed: {body}");
    }

    // Disabling a provider key must not erase references in either policy table.
    let stored = auth_repository
        .find_export_standalone_api_key_by_id("key-123")
        .await
        .expect("export should load")
        .expect("export should exist");
    assert_eq!(
        stored.allowed_provider_key_ids,
        Some([("provider-1".to_string(), ["key-a".to_string()].into())].into())
    );
    let stored_group = user_repository
        .find_user_group_by_id(&group_id)
        .await
        .expect("group should load")
        .expect("group should exist");
    assert_eq!(
        stored_group.allowed_provider_key_ids,
        Some([("provider-1".to_string(), ["key-a".to_string()].into())].into())
    );

    // Reopen (list) shows the disabled key scope instead of losing it.
    let list_response =
        admin_request(reqwest::Client::new().get(format!("{gateway_url}/api/admin/api-keys")))
            .send()
            .await
            .expect("list request should succeed");
    assert_eq!(list_response.status(), StatusCode::OK);
    let list_payload: serde_json::Value =
        list_response.json().await.expect("list json should parse");
    assert_eq!(
        list_payload["api_keys"][0]["allowed_provider_key_ids"],
        json!({ "provider-1": ["key-a"] })
    );

    // Explicit saves still accept a disabled reference.
    let update_response = admin_request(
        reqwest::Client::new().put(format!("{gateway_url}/api/admin/api-keys/key-123")),
    )
    .json(&json!({
        "allowed_provider_key_ids": { "provider-1": ["key-a"] },
    }))
    .send()
    .await
    .expect("request should succeed");
    assert_eq!(update_response.status(), StatusCode::OK);

    let group_response = admin_request(
        reqwest::Client::new().put(format!("{gateway_url}/api/admin/user-groups/{group_id}")),
    )
    .json(&json!({
        "name": "Scoped Group",
        "allowed_providers": ["provider-1"],
        "allowed_providers_mode": "specific",
        "allowed_provider_key_ids": { "provider-1": ["key-a"] },
        "allowed_api_formats_mode": "inherit",
        "allowed_models_mode": "inherit",
        "rate_limit_mode": "inherit"
    }))
    .send()
    .await
    .expect("request should succeed");
    assert_eq!(group_response.status(), StatusCode::OK);

    // Re-enabling the key keeps the original scope in both policy tables.
    let response = admin_request(
        reqwest::Client::new().put(format!("{gateway_url}/api/admin/endpoints/keys/key-a")),
    )
    .json(&json!({ "is_active": true }))
    .send()
    .await
    .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::OK);

    let stored = auth_repository
        .find_export_standalone_api_key_by_id("key-123")
        .await
        .expect("export should load")
        .expect("export should exist");
    assert_eq!(
        stored.allowed_provider_key_ids,
        Some([("provider-1".to_string(), ["key-a".to_string()].into())].into())
    );
    let stored_group = user_repository
        .find_user_group_by_id(&group_id)
        .await
        .expect("group should load")
        .expect("group should exist");
    assert_eq!(
        stored_group.allowed_provider_key_ids,
        Some([("provider-1".to_string(), ["key-a".to_string()].into())].into())
    );

    gateway_handle.abort();
    upstream_handle.abort();
}
