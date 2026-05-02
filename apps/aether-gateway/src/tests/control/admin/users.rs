use std::sync::{Arc, Mutex};

use aether_crypto::{encrypt_python_fernet_plaintext, DEVELOPMENT_ENCRYPTION_KEY};
use aether_data::repository::auth::{
    InMemoryAuthApiKeySnapshotRepository, StoredAuthApiKeyExportRecord, StoredAuthApiKeySnapshot,
};
use aether_data::repository::usage::InMemoryUsageReadRepository;
use aether_data::repository::users::{
    InMemoryUserReadRepository, StoredUserAuthRecord, StoredUserExportRow,
};
use aether_data::repository::wallet::InMemoryWalletRepository;
use aether_data::repository::wallet::StoredWalletSnapshot;
use aether_data_contracts::repository::usage::StoredRequestUsageAudit;
use axum::body::Body;
use axum::routing::{any, delete, get, patch, post, put};
use axum::{extract::Request, Router};
use chrono::Utc;
use http::StatusCode;
use serde_json::json;

use super::super::{
    build_router_with_state, issue_test_admin_access_token, start_server, AppState,
};
use crate::constants::{
    GATEWAY_HEADER, TRUSTED_ADMIN_SESSION_ID_HEADER, TRUSTED_ADMIN_USER_ID_HEADER,
    TRUSTED_ADMIN_USER_ROLE_HEADER,
};
use crate::data::GatewayDataState;

fn sample_admin_user(user_id: &str) -> StoredUserAuthRecord {
    sample_admin_user_with_role(user_id, "user", "alice@example.com", "alice")
}

fn sample_admin_user_with_role(
    user_id: &str,
    role: &str,
    email: &str,
    username: &str,
) -> StoredUserAuthRecord {
    StoredUserAuthRecord::new(
        user_id.to_string(),
        Some(email.to_string()),
        true,
        username.to_string(),
        Some("hash".to_string()),
        role.to_string(),
        "local".to_string(),
        Some(json!(["openai"])),
        Some(json!(["openai:chat"])),
        Some(json!(["gpt-4.1"])),
        true,
        false,
        Some(Utc::now()),
        Some(Utc::now()),
    )
    .expect("user should build")
}

fn sample_admin_export_user(user_id: &str) -> StoredUserExportRow {
    sample_admin_export_user_with("user", true, user_id, "alice@example.com", "alice")
}

fn sample_admin_export_user_with(
    role: &str,
    is_active: bool,
    user_id: &str,
    email: &str,
    username: &str,
) -> StoredUserExportRow {
    StoredUserExportRow::new(
        user_id.to_string(),
        Some(email.to_string()),
        true,
        username.to_string(),
        Some("hash".to_string()),
        role.to_string(),
        "local".to_string(),
        Some(json!(["openai"])),
        Some(json!(["openai:chat"])),
        Some(json!(["gpt-4.1"])),
        Some(60),
        None,
        is_active,
    )
    .expect("user export row should build")
}

fn sample_admin_session(
    user_id: &str,
    session_id: &str,
) -> crate::data::state::StoredUserSessionRecord {
    let now = Utc::now();
    crate::data::state::StoredUserSessionRecord::new(
        session_id.to_string(),
        user_id.to_string(),
        format!("device-{session_id}"),
        None,
        format!("refresh-{session_id}"),
        None,
        None,
        Some(now),
        Some(now + chrono::Duration::days(7)),
        None,
        None,
        Some("127.0.0.1".to_string()),
        Some("admin-test".to_string()),
        Some(now),
        Some(now),
    )
    .expect("session should build")
}

fn sample_admin_wallet(user_id: &str, limit_mode: &str) -> StoredWalletSnapshot {
    StoredWalletSnapshot::new(
        format!("wallet-{user_id}"),
        Some(user_id.to_string()),
        None,
        12.5,
        2.5,
        limit_mode.to_string(),
        "USD".to_string(),
        "active".to_string(),
        30.0,
        10.0,
        3.0,
        1.5,
        1_710_000_000,
    )
    .expect("wallet should build")
}

fn sample_usage_row(
    id: &str,
    request_id: &str,
    user_id: Option<&str>,
    provider_name: &str,
    status: &str,
    total_tokens: i32,
) -> StoredRequestUsageAudit {
    StoredRequestUsageAudit::new(
        id.to_string(),
        request_id.to_string(),
        user_id.map(str::to_string),
        Some(format!("key-{id}")),
        user_id.map(|value| format!("user-{value}")),
        Some(format!("key-name-{id}")),
        provider_name.to_string(),
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
        status == "streaming",
        total_tokens,
        0,
        total_tokens,
        0.1,
        0.1,
        Some(if status == "failed" { 500 } else { 200 }),
        (status == "failed").then(|| "request failed".to_string()),
        None,
        Some(320),
        Some(120),
        status.to_string(),
        "settled".to_string(),
        1_711_000_000_000,
        1_711_000_001,
        Some(1_711_000_002),
    )
    .expect("usage row should build")
}

fn sample_admin_api_key_snapshot(user_id: &str, api_key_id: &str) -> StoredAuthApiKeySnapshot {
    StoredAuthApiKeySnapshot::new(
        user_id.to_string(),
        "alice".to_string(),
        Some("alice@example.com".to_string()),
        "user".to_string(),
        "local".to_string(),
        true,
        false,
        Some(json!(["openai"])),
        Some(json!(["openai:chat"])),
        Some(json!(["gpt-4.1"])),
        api_key_id.to_string(),
        Some("default".to_string()),
        true,
        false,
        false,
        Some(60),
        Some(5),
        Some(200),
        Some(json!(["openai"])),
        Some(json!(["openai:chat"])),
        Some(json!(["gpt-4.1"])),
    )
    .expect("api key snapshot should build")
}

#[tokio::test]
async fn gateway_handles_admin_users_root_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/users",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let user_repository = Arc::new(
        InMemoryUserReadRepository::seed_auth_users(vec![
            sample_admin_user("user-1"),
            sample_admin_user_with_role("user-2", "admin", "root@example.com", "root"),
            sample_admin_user_with_role("user-3", "user", "carol@example.com", "carol"),
        ])
        .with_export_users(vec![
            sample_admin_export_user("user-1"),
            sample_admin_export_user_with("admin", true, "user-2", "root@example.com", "root"),
            sample_admin_export_user_with("user", false, "user-3", "carol@example.com", "carol"),
        ]),
    );
    let wallet_repository = Arc::new(InMemoryWalletRepository::seed(vec![
        sample_admin_wallet("user-1", "unlimited"),
        sample_admin_wallet("user-2", "monthly"),
        sample_admin_wallet("user-3", "monthly"),
    ]));
    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![
        sample_usage_row(
            "usage-1",
            "req-1",
            Some("user-1"),
            "OpenAI",
            "completed",
            80,
        ),
        sample_usage_row("usage-2", "req-2", Some("user-1"), "OpenAI", "failed", 20),
        sample_usage_row(
            "usage-3",
            "req-3",
            Some("user-1"),
            "OpenAI",
            "streaming",
            999,
        ),
        sample_usage_row(
            "usage-4",
            "req-4",
            Some("user-1"),
            "pending",
            "completed",
            999,
        ),
        sample_usage_row(
            "usage-5",
            "req-5",
            Some("user-3"),
            "OpenAI",
            "completed",
            777,
        ),
    ]));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(GatewayDataState::with_user_wallet_and_usage_for_tests(
                user_repository,
                wallet_repository,
                usage_repository,
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/users?skip=0&limit=20&role=user&is_active=true"
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
    let items = payload.as_array().expect("list payload should be array");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], "user-1");
    assert_eq!(items[0]["email"], "alice@example.com");
    assert_eq!(items[0]["username"], "alice");
    assert_eq!(items[0]["role"], "user");
    assert_eq!(items[0]["allowed_providers"], json!(["openai"]));
    assert_eq!(items[0]["allowed_api_formats"], json!(["openai:chat"]));
    assert_eq!(items[0]["allowed_models"], json!(["gpt-4.1"]));
    assert_eq!(items[0]["rate_limit"], 60);
    assert_eq!(items[0]["unlimited"], true);
    assert_eq!(items[0]["is_active"], true);
    assert_eq!(items[0]["request_count"], 2);
    assert_eq!(items[0]["total_tokens"], 100);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();

    let create_gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_auth_users_for_tests(Vec::<StoredUserAuthRecord>::new())
            .with_auth_wallets_for_tests(Vec::<StoredWalletSnapshot>::new()),
    );
    let (create_gateway_url, create_gateway_handle) = start_server(create_gateway).await;

    let create_response = reqwest::Client::new()
        .post(format!("{create_gateway_url}/api/admin/users"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "email": "new-user@example.com",
            "username": "new_user",
            "password": "NewUser123!",
            "role": "user",
            "unlimited": false,
            "initial_gift_usd": 6.5,
            "allowed_providers": ["openai"],
            "allowed_api_formats": ["openai:chat"],
            "allowed_models": ["gpt-4.1"],
            "rate_limit": 80
        }))
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(create_response.status(), StatusCode::OK);
    let create_payload: serde_json::Value = create_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(create_payload["email"], "new-user@example.com");
    assert_eq!(create_payload["username"], "new_user");
    assert_eq!(create_payload["role"], "user");
    assert_eq!(create_payload["rate_limit"], 80);
    assert_eq!(create_payload["unlimited"], false);

    create_gateway_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_users_root_locally_with_bearer_admin_session() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/users",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let user_repository = Arc::new(
        InMemoryUserReadRepository::seed_auth_users(vec![sample_admin_user("user-1")])
            .with_export_users(vec![sample_admin_export_user("user-1")]),
    );
    let wallet_repository = Arc::new(InMemoryWalletRepository::seed(vec![sample_admin_wallet(
        "user-1",
        "unlimited",
    )]));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(GatewayDataState::with_user_and_wallet_for_tests(
            user_repository,
            wallet_repository,
        ));
    let access_token = issue_test_admin_access_token(&state, "device-admin-users").await;
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/api/admin/users"))
        .header("authorization", format!("Bearer {access_token}"))
        .header("x-client-device-id", "device-admin-users")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    let items = payload.as_array().expect("list payload should be array");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], "user-1");
    assert_eq!(items[0]["email"], "alice@example.com");
    assert_eq!(items[0]["username"], "alice");
    assert_eq!(items[0]["role"], "user");
    assert_eq!(items[0]["allowed_providers"], json!(["openai"]));
    assert_eq!(items[0]["allowed_api_formats"], json!(["openai:chat"]));
    assert_eq!(items[0]["allowed_models"], json!(["gpt-4.1"]));
    assert_eq!(items[0]["rate_limit"], 60);
    assert_eq!(items[0]["unlimited"], true);
    assert_eq!(items[0]["is_active"], true);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_user_detail_routes_locally_with_trusted_admin_principal() {
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
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_auth_users_for_tests([
                sample_admin_user("user-1"),
                sample_admin_user_with_role("admin-1", "admin", "admin1@example.com", "admin_one"),
                sample_admin_user_with_role("admin-2", "admin", "admin2@example.com", "admin_two"),
            ])
            .with_auth_wallets_for_tests([sample_admin_wallet("user-1", "finite")]),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let client = reqwest::Client::new();

    let update_response = client
        .put(format!("{gateway_url}/api/admin/users/user-1"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "email": "alice-updated@example.com",
            "username": "alice_updated",
            "role": "admin",
            "allowed_providers": ["openai", "anthropic"],
            "allowed_api_formats": ["openai:chat", "gemini:generate_content"],
            "allowed_models": ["gpt-4.1", "gemini-2.5-pro"],
            "rate_limit": 120,
            "is_active": false,
            "unlimited": true
        }))
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(update_response.status(), StatusCode::OK);
    let update_payload: serde_json::Value = update_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(update_payload["email"], "alice-updated@example.com");
    assert_eq!(update_payload["username"], "alice_updated");
    assert_eq!(update_payload["role"], "admin");
    assert_eq!(update_payload["rate_limit"], 120);
    assert_eq!(update_payload["unlimited"], true);
    assert_eq!(update_payload["is_active"], false);

    let delete_response = client
        .delete(format!("{gateway_url}/api/admin/users/user-1"))
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
    assert_eq!(delete_payload["message"], "用户删除成功");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_user_detail_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/users/user-1",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let user_repository = Arc::new(
        InMemoryUserReadRepository::seed_auth_users(vec![sample_admin_user("user-1")])
            .with_export_users(vec![sample_admin_export_user("user-1")]),
    );
    let wallet_repository = Arc::new(InMemoryWalletRepository::seed(vec![sample_admin_wallet(
        "user-1",
        "unlimited",
    )]));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(GatewayDataState::with_user_and_wallet_for_tests(
                user_repository,
                wallet_repository,
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/api/admin/users/user-1"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["id"], "user-1");
    assert_eq!(payload["email"], "alice@example.com");
    assert_eq!(payload["username"], "alice");
    assert_eq!(payload["role"], "user");
    assert_eq!(payload["allowed_providers"], json!(["openai"]));
    assert_eq!(payload["allowed_api_formats"], json!(["openai:chat"]));
    assert_eq!(payload["allowed_models"], json!(["gpt-4.1"]));
    assert_eq!(payload["rate_limit"], 60);
    assert_eq!(payload["unlimited"], true);
    assert_eq!(payload["is_active"], true);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_user_session_routes_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/users/user-1/sessions",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let user_repository = Arc::new(InMemoryUserReadRepository::seed_auth_users(vec![
        sample_admin_user("user-1"),
    ]));
    let mut older = sample_admin_session("user-1", "session-1");
    older.created_at =
        Some(chrono::Utc::now() - chrono::Duration::days(2) - chrono::Duration::hours(1));
    older.last_seen_at = Some(chrono::Utc::now() - chrono::Duration::hours(6));
    older.ip_address = Some("10.0.0.1".to_string());
    let mut newer = sample_admin_session("user-1", "session-2");
    newer.created_at = Some(chrono::Utc::now() - chrono::Duration::days(1));
    newer.last_seen_at = Some(chrono::Utc::now() - chrono::Duration::hours(1));
    newer.ip_address = Some("10.0.0.2".to_string());
    let mut revoked = sample_admin_session("user-1", "session-3");
    revoked.revoked_at = Some(chrono::Utc::now());
    let mut expired = sample_admin_session("user-1", "session-4");
    expired.expires_at = Some(chrono::Utc::now() - chrono::Duration::minutes(1));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(crate::data::GatewayDataState::with_user_reader_for_tests(
                user_repository,
            ))
            .with_auth_sessions_for_tests([older.clone(), newer.clone(), revoked, expired]),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/api/admin/users/user-1/sessions"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    let sessions = payload
        .as_array()
        .expect("sessions payload should be array");
    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0]["id"], newer.id);
    assert_eq!(sessions[0]["device_label"], "未知设备");
    assert_eq!(sessions[0]["device_type"], "unknown");
    assert_eq!(sessions[0]["ip_address"], "10.0.0.2");
    assert_eq!(sessions[0]["is_current"], false);
    assert_eq!(sessions[1]["id"], older.id);
    assert_eq!(sessions[1]["ip_address"], "10.0.0.1");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_user_api_key_routes_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().fallback(any(move |_request: Request| {
        let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
        async move {
            *upstream_hits_inner.lock().expect("mutex should lock") += 1;
            (StatusCode::OK, Body::from("unexpected upstream hit"))
        }
    }));

    let encrypted = encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "sk-user-1")
        .expect("ciphertext should build");
    let auth_repository = Arc::new(
        InMemoryAuthApiKeySnapshotRepository::seed(vec![(
            Some("hash-key-1".to_string()),
            sample_admin_api_key_snapshot("user-1", "key-1"),
        )])
        .with_export_records(vec![StoredAuthApiKeyExportRecord::new(
            "user-1".to_string(),
            "key-1".to_string(),
            "hash-key-1".to_string(),
            Some(encrypted),
            Some("default".to_string()),
            Some(json!(["openai"])),
            Some(json!(["openai:chat"])),
            Some(json!(["gpt-4.1"])),
            Some(60),
            Some(5),
            None,
            true,
            Some(200),
            false,
            9,
            0,
            1.5,
            false,
        )
        .expect("export record should build")
        .with_activity_timestamps(
            Some(1_711_000_102),
            Some(1_711_000_100),
            Some(1_711_000_101),
        )
        .expect("export activity timestamps should build")]),
    );
    let user_repository = Arc::new(InMemoryUserReadRepository::seed_auth_users(vec![
        sample_admin_user("user-1"),
    ]));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_auth_api_key_repository_for_tests(
                    auth_repository,
                )
                .with_user_reader(user_repository),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let client = reqwest::Client::new();

    let create_response = client
        .post(format!("{gateway_url}/api/admin/users/user-1/api-keys"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "name": "new-key",
            "rate_limit": 90,
        }))
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(create_response.status(), StatusCode::OK);
    let create_payload: serde_json::Value = create_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(create_payload["name"], "new-key");
    assert_eq!(create_payload["rate_limit"], 90);
    assert_eq!(create_payload["concurrent_limit"], serde_json::Value::Null);
    assert_eq!(
        create_payload["message"],
        "API Key创建成功，请妥善保存完整密钥"
    );
    let created_at = create_payload["created_at"]
        .as_str()
        .expect("created_at should be string");
    assert!(chrono::DateTime::parse_from_rfc3339(created_at).is_ok());
    assert!(!created_at.starts_with("1970-01-01"));
    assert!(create_payload["id"]
        .as_str()
        .is_some_and(|value| !value.is_empty()));
    assert!(create_payload["key"]
        .as_str()
        .is_some_and(|value| value.starts_with("sk-")));

    let update_response = client
        .put(format!(
            "{gateway_url}/api/admin/users/user-1/api-keys/key-1"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "name": "renamed",
            "rate_limit": 120,
            "concurrent_limit": 9,
        }))
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(update_response.status(), StatusCode::OK);
    let update_payload: serde_json::Value = update_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(update_payload["id"], "key-1");
    assert_eq!(update_payload["name"], "renamed");
    assert_eq!(update_payload["is_locked"], false);
    assert_eq!(update_payload["rate_limit"], 120);
    assert_eq!(update_payload["concurrent_limit"], 9);
    assert_eq!(update_payload["created_at"], "2024-03-21T05:48:20+00:00");
    assert_eq!(update_payload["message"], "API Key更新成功");

    let lock_response = client
        .patch(format!(
            "{gateway_url}/api/admin/users/user-1/api-keys/key-1/lock"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({ "locked": true }))
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(lock_response.status(), StatusCode::OK);
    let lock_payload: serde_json::Value =
        lock_response.json().await.expect("json body should parse");
    assert_eq!(lock_payload["id"], "key-1");
    assert_eq!(lock_payload["is_locked"], true);
    assert_eq!(lock_payload["message"], "API密钥已锁定");

    let delete_response = client
        .delete(format!(
            "{gateway_url}/api/admin/users/user-1/api-keys/key-1"
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
    assert_eq!(delete_payload["message"], "API Key已删除");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_returns_conflict_for_admin_create_user_api_key_when_writer_unavailable() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().fallback(any(move |_request: Request| {
        let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
        async move {
            *upstream_hits_inner.lock().expect("mutex should lock") += 1;
            (StatusCode::OK, Body::from("unexpected upstream hit"))
        }
    }));

    let encrypted = encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "sk-user-1")
        .expect("ciphertext should build");
    let auth_repository = Arc::new(
        InMemoryAuthApiKeySnapshotRepository::seed(vec![(
            Some("hash-key-1".to_string()),
            sample_admin_api_key_snapshot("user-1", "key-1"),
        )])
        .with_export_records(vec![StoredAuthApiKeyExportRecord::new(
            "user-1".to_string(),
            "key-1".to_string(),
            "hash-key-1".to_string(),
            Some(encrypted),
            Some("default".to_string()),
            Some(json!(["openai"])),
            Some(json!(["openai:chat"])),
            Some(json!(["gpt-4.1"])),
            Some(60),
            Some(5),
            None,
            true,
            Some(200),
            false,
            9,
            0,
            1.5,
            false,
        )
        .expect("export record should build")]),
    );
    let user_repository = Arc::new(InMemoryUserReadRepository::seed_auth_users(vec![
        sample_admin_user("user-1"),
    ]));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_auth_api_key_reader_for_tests(auth_repository)
                    .with_user_reader(user_repository),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/users/user-1/api-keys"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "name": "new-key",
            "rate_limit": 90,
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["error_code"], "read_only_mode");
    assert_eq!(payload["detail"], "当前为只读模式，无法创建用户 API Key");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_returns_conflict_for_admin_create_user_when_writer_unavailable() {
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
    let mut state = AppState::new().expect("gateway should build");
    state =
        state.with_data_state_for_tests(crate::data::GatewayDataState::with_user_reader_for_tests(
            Arc::new(InMemoryUserReadRepository::seed_auth_users(Vec::new())),
        ));
    state.auth_user_store = None;
    state.auth_wallet_store = None;
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/users"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "email": "new-user@example.com",
            "username": "new_user",
            "password": "NewUser123!",
            "role": "user"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["error_code"], "read_only_mode");
    assert_eq!(payload["detail"], "当前为只读模式，无法创建用户");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_returns_conflict_for_admin_lock_user_api_key_when_writer_unavailable() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().fallback(any(move |_request: Request| {
        let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
        async move {
            *upstream_hits_inner.lock().expect("mutex should lock") += 1;
            (StatusCode::OK, Body::from("unexpected upstream hit"))
        }
    }));

    let encrypted = encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "sk-user-1")
        .expect("ciphertext should build");
    let auth_repository = Arc::new(
        InMemoryAuthApiKeySnapshotRepository::seed(vec![(
            Some("hash-key-1".to_string()),
            sample_admin_api_key_snapshot("user-1", "key-1"),
        )])
        .with_export_records(vec![StoredAuthApiKeyExportRecord::new(
            "user-1".to_string(),
            "key-1".to_string(),
            "hash-key-1".to_string(),
            Some(encrypted),
            Some("default".to_string()),
            Some(json!(["openai"])),
            Some(json!(["openai:chat"])),
            Some(json!(["gpt-4.1"])),
            Some(60),
            Some(5),
            None,
            true,
            Some(200),
            false,
            9,
            0,
            1.5,
            false,
        )
        .expect("export record should build")]),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_auth_api_key_reader_for_tests(auth_repository),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .patch(format!(
            "{gateway_url}/api/admin/users/user-1/api-keys/key-1/lock"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({ "locked": true }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["error_code"], "read_only_mode");
    assert_eq!(
        payload["detail"],
        "当前为只读模式，无法锁定或解锁用户 API Key"
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_returns_conflict_for_admin_update_user_when_writer_unavailable() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().fallback(any(move |_request: Request| {
        let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
        async move {
            *upstream_hits_inner.lock().expect("mutex should lock") += 1;
            (StatusCode::OK, Body::from("unexpected upstream hit"))
        }
    }));

    let user_repository = Arc::new(InMemoryUserReadRepository::seed_auth_users(vec![
        sample_admin_user("user-1"),
    ]));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let mut state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(crate::data::GatewayDataState::with_user_reader_for_tests(
            user_repository,
        ));
    state.auth_user_store = None;
    state.auth_wallet_store = None;
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .put(format!("{gateway_url}/api/admin/users/user-1"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({ "email": "alice-updated@example.com" }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["error_code"], "read_only_mode");
    assert_eq!(payload["detail"], "当前为只读模式，无法更新用户");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_returns_conflict_for_admin_update_user_api_key_when_writer_unavailable() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().fallback(any(move |_request: Request| {
        let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
        async move {
            *upstream_hits_inner.lock().expect("mutex should lock") += 1;
            (StatusCode::OK, Body::from("unexpected upstream hit"))
        }
    }));

    let encrypted = encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "sk-user-1")
        .expect("ciphertext should build");
    let auth_repository = Arc::new(
        InMemoryAuthApiKeySnapshotRepository::seed(vec![(
            Some("hash-key-1".to_string()),
            sample_admin_api_key_snapshot("user-1", "key-1"),
        )])
        .with_export_records(vec![StoredAuthApiKeyExportRecord::new(
            "user-1".to_string(),
            "key-1".to_string(),
            "hash-key-1".to_string(),
            Some(encrypted),
            Some("default".to_string()),
            Some(json!(["openai"])),
            Some(json!(["openai:chat"])),
            Some(json!(["gpt-4.1"])),
            Some(60),
            Some(5),
            None,
            true,
            Some(200),
            false,
            9,
            0,
            1.5,
            false,
        )
        .expect("export record should build")]),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_auth_api_key_reader_for_tests(auth_repository),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .put(format!(
            "{gateway_url}/api/admin/users/user-1/api-keys/key-1"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({ "name": "renamed", "rate_limit": 88 }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["error_code"], "read_only_mode");
    assert_eq!(payload["detail"], "当前为只读模式，无法更新用户 API Key");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_returns_conflict_for_admin_delete_user_api_key_when_writer_unavailable() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().fallback(any(move |_request: Request| {
        let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
        async move {
            *upstream_hits_inner.lock().expect("mutex should lock") += 1;
            (StatusCode::OK, Body::from("unexpected upstream hit"))
        }
    }));

    let encrypted = encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "sk-user-1")
        .expect("ciphertext should build");
    let auth_repository = Arc::new(
        InMemoryAuthApiKeySnapshotRepository::seed(vec![(
            Some("hash-key-1".to_string()),
            sample_admin_api_key_snapshot("user-1", "key-1"),
        )])
        .with_export_records(vec![StoredAuthApiKeyExportRecord::new(
            "user-1".to_string(),
            "key-1".to_string(),
            "hash-key-1".to_string(),
            Some(encrypted),
            Some("default".to_string()),
            Some(json!(["openai"])),
            Some(json!(["openai:chat"])),
            Some(json!(["gpt-4.1"])),
            Some(60),
            Some(5),
            None,
            true,
            Some(200),
            false,
            9,
            0,
            1.5,
            false,
        )
        .expect("export record should build")]),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_auth_api_key_reader_for_tests(auth_repository),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .delete(format!(
            "{gateway_url}/api/admin/users/user-1/api-keys/key-1"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["error_code"], "read_only_mode");
    assert_eq!(payload["detail"], "当前为只读模式，无法删除用户 API Key");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_lists_admin_user_api_keys_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/users/user-1/api-keys",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let encrypted = encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "sk-user-1")
        .expect("ciphertext should build");
    let auth_repository = Arc::new(
        InMemoryAuthApiKeySnapshotRepository::seed(vec![(
            Some("hash-key-1".to_string()),
            sample_admin_api_key_snapshot("user-1", "key-1"),
        )])
        .with_export_records(vec![StoredAuthApiKeyExportRecord::new(
            "user-1".to_string(),
            "key-1".to_string(),
            "hash-key-1".to_string(),
            Some(encrypted),
            Some("default".to_string()),
            Some(json!(["openai"])),
            Some(json!(["openai:chat"])),
            Some(json!(["gpt-4.1"])),
            Some(60),
            Some(5),
            None,
            true,
            Some(200),
            false,
            9,
            0,
            1.5,
            false,
        )
        .expect("export record should build")
        .with_activity_timestamps(
            Some(1_711_000_102),
            Some(1_711_000_100),
            Some(1_711_000_101),
        )
        .expect("export activity timestamps should build")]),
    );
    let user_repository = Arc::new(InMemoryUserReadRepository::seed_auth_users(vec![
        sample_admin_user("user-1"),
    ]));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_user_reader_for_tests(user_repository)
                    .with_auth_api_key_reader(auth_repository),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/api/admin/users/user-1/api-keys"))
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
    assert_eq!(payload["user_email"], "alice@example.com");
    assert_eq!(payload["username"], "alice");
    assert_eq!(payload["api_keys"][0]["id"], "key-1");
    assert_eq!(payload["api_keys"][0]["name"], "default");
    assert_eq!(payload["api_keys"][0]["key_display"], "sk-user-1...er-1");
    assert_eq!(payload["api_keys"][0]["is_active"], true);
    assert_eq!(payload["api_keys"][0]["is_locked"], false);
    assert_eq!(payload["api_keys"][0]["total_requests"], 9);
    assert_eq!(payload["api_keys"][0]["total_cost_usd"], 1.5);
    assert_eq!(payload["api_keys"][0]["rate_limit"], 60);
    assert_eq!(
        payload["api_keys"][0]["created_at"],
        "2024-03-21T05:48:20+00:00"
    );
    assert_eq!(
        payload["api_keys"][0]["last_used_at"],
        "2024-03-21T05:48:22+00:00"
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_revokes_admin_user_session_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/users/user-1/sessions/session-1",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let user_repository = Arc::new(InMemoryUserReadRepository::seed_auth_users(vec![
        sample_admin_user("user-1"),
    ]));
    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(crate::data::GatewayDataState::with_user_reader_for_tests(
                user_repository,
            ))
            .with_auth_session_for_tests(sample_admin_session("user-1", "session-1")),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .delete(format!(
            "{gateway_url}/api/admin/users/user-1/sessions/session-1"
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
    assert_eq!(payload["message"], "用户设备已强制下线");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_revokes_all_admin_user_sessions_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/users/user-1/sessions",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let user_repository = Arc::new(InMemoryUserReadRepository::seed_auth_users(vec![
        sample_admin_user("user-1"),
    ]));
    let sessions = vec![
        sample_admin_session("user-1", "session-1"),
        sample_admin_session("user-1", "session-2"),
    ];
    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(crate::data::GatewayDataState::with_user_reader_for_tests(
                user_repository,
            ))
            .with_auth_sessions_for_tests(sessions),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .delete(format!("{gateway_url}/api/admin/users/user-1/sessions"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["message"], "已强制下线该用户所有设备");
    assert_eq!(payload["revoked_count"], 2);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_reveals_admin_user_full_key_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/users/user-1/api-keys/key-1/full-key",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let encrypted = encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "sk-user-1")
        .expect("ciphertext should build");
    let auth_repository = Arc::new(
        InMemoryAuthApiKeySnapshotRepository::seed(vec![(
            Some("hash-key-1".to_string()),
            sample_admin_api_key_snapshot("user-1", "key-1"),
        )])
        .with_export_records(vec![StoredAuthApiKeyExportRecord::new(
            "user-1".to_string(),
            "key-1".to_string(),
            "hash-key-1".to_string(),
            Some(encrypted),
            Some("default".to_string()),
            Some(json!(["openai"])),
            Some(json!(["openai:chat"])),
            Some(json!(["gpt-4.1"])),
            Some(60),
            Some(5),
            None,
            true,
            Some(200),
            false,
            9,
            0,
            1.5,
            false,
        )
        .expect("export record should build")]),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_auth_api_key_reader_for_tests(auth_repository),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/users/user-1/api-keys/key-1/full-key"
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
    assert_eq!(payload["key"], "sk-user-1");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}
