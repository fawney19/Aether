use std::sync::{Arc, Mutex};

use aether_crypto::{encrypt_python_fernet_plaintext, DEVELOPMENT_ENCRYPTION_KEY};
use aether_data::repository::auth::{
    InMemoryAuthApiKeySnapshotRepository, StoredAuthApiKeyExportRecord, StoredAuthApiKeySnapshot,
};
use aether_data::repository::usage::InMemoryUsageReadRepository;
use aether_data::repository::users::{
    InMemoryUserReadRepository, StoredUserAuthRecord, StoredUserExportRow, UpsertUserGroupRecord,
    UserReadRepository,
};
use aether_data::repository::wallet::InMemoryWalletRepository;
use aether_data::repository::wallet::StoredWalletSnapshot;
use aether_data_contracts::repository::usage::StoredRequestUsageAudit;
use axum::body::Body;
use axum::routing::{any, delete, get, patch, post, put};
use axum::{extract::Request, Router};
use chrono::{TimeZone, Utc};
use http::StatusCode;
use serde_json::json;

use super::super::{
    build_router_with_state, hash_api_key, issue_test_admin_access_token, start_server, AppState,
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
    sample_admin_user_with_role_and_created_at(user_id, role, email, username, Utc::now())
}

fn sample_admin_user_with_role_and_created_at(
    user_id: &str,
    role: &str,
    email: &str,
    username: &str,
    created_at: chrono::DateTime<Utc>,
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
        Some(created_at),
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
async fn gateway_sorts_admin_users_by_created_at() {
    let oldest = Utc
        .with_ymd_and_hms(2026, 1, 10, 0, 0, 0)
        .single()
        .expect("valid timestamp");
    let middle = Utc
        .with_ymd_and_hms(2026, 2, 10, 0, 0, 0)
        .single()
        .expect("valid timestamp");
    let newest = Utc
        .with_ymd_and_hms(2026, 3, 10, 0, 0, 0)
        .single()
        .expect("valid timestamp");

    let user_repository = Arc::new(
        InMemoryUserReadRepository::seed_auth_users(vec![
            sample_admin_user_with_role_and_created_at(
                "user-old",
                "user",
                "old@example.com",
                "old",
                oldest,
            ),
            sample_admin_user_with_role_and_created_at(
                "user-middle",
                "user",
                "middle@example.com",
                "middle",
                middle,
            ),
            sample_admin_user_with_role_and_created_at(
                "user-new",
                "user",
                "new@example.com",
                "new",
                newest,
            ),
        ])
        .with_export_users(vec![
            sample_admin_export_user_with("user", true, "user-old", "old@example.com", "old"),
            sample_admin_export_user_with(
                "user",
                true,
                "user-middle",
                "middle@example.com",
                "middle",
            ),
            sample_admin_export_user_with("user", true, "user-new", "new@example.com", "new"),
        ]),
    );

    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(GatewayDataState::with_user_reader_for_tests(
                user_repository,
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();

    let desc_response = client
        .get(format!(
            "{gateway_url}/api/admin/users?skip=0&limit=10&sort_by=created_at&sort_order=desc"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(desc_response.status(), StatusCode::OK);
    let desc_payload: serde_json::Value = desc_response.json().await.expect("json should parse");
    let desc_ids = desc_payload["items"]
        .as_array()
        .expect("items should be array")
        .iter()
        .map(|item| item["id"].as_str().expect("id should be string"))
        .collect::<Vec<_>>();
    assert_eq!(desc_ids, vec!["user-new", "user-middle", "user-old"]);

    let asc_response = client
        .get(format!(
            "{gateway_url}/api/admin/users?skip=0&limit=10&sort_by=created_at&sort_order=asc"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(asc_response.status(), StatusCode::OK);
    let asc_payload: serde_json::Value = asc_response.json().await.expect("json should parse");
    let asc_ids = asc_payload["items"]
        .as_array()
        .expect("items should be array")
        .iter()
        .map(|item| item["id"].as_str().expect("id should be string"))
        .collect::<Vec<_>>();
    assert_eq!(asc_ids, vec!["user-old", "user-middle", "user-new"]);

    gateway_handle.abort();
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
    assert_eq!(payload["total"], 1);
    assert_eq!(payload["skip"], 0);
    assert_eq!(payload["limit"], 20);
    assert_eq!(payload["has_more"], false);
    let items = payload["items"].as_array().expect("items should be array");
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

    let search_response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/users?skip=0&limit=20&search=carol"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("search request should succeed");

    assert_eq!(search_response.status(), StatusCode::OK);
    let search_payload: serde_json::Value = search_response
        .json()
        .await
        .expect("search json body should parse");
    assert_eq!(search_payload["total"], 1);
    let search_items = search_payload["items"]
        .as_array()
        .expect("search items should be array");
    assert_eq!(search_items.len(), 1);
    assert_eq!(search_items[0]["id"], "user-3");
    assert_eq!(search_items[0]["email"], "carol@example.com");

    let id_search_response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/users?skip=0&limit=20&search=user-3"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("id search request should succeed");
    assert_eq!(id_search_response.status(), StatusCode::OK);
    let id_search_payload: serde_json::Value = id_search_response
        .json()
        .await
        .expect("id search json body should parse");
    assert_eq!(id_search_payload["total"], 1);
    let id_search_items = id_search_payload["items"]
        .as_array()
        .expect("id search items should be array");
    assert_eq!(id_search_items.len(), 1);
    assert_eq!(id_search_items[0]["id"], "user-3");

    let limited_search_response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/users?skip=0&limit=2&search=example.com"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("limited search request should succeed");
    assert_eq!(limited_search_response.status(), StatusCode::OK);
    let limited_search_payload: serde_json::Value = limited_search_response
        .json()
        .await
        .expect("limited search json body should parse");
    assert_eq!(limited_search_payload["total"], 3);
    assert_eq!(limited_search_payload["has_more"], true);
    let limited_search_items = limited_search_payload["items"]
        .as_array()
        .expect("limited search items should be array");
    assert_eq!(limited_search_items.len(), 2);

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
            "initial_gift_usd": 6.5
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
    assert_eq!(create_payload["rate_limit"], serde_json::Value::Null);
    assert_eq!(create_payload["unlimited"], false);

    let hidden_policy_response = reqwest::Client::new()
        .post(format!("{create_gateway_url}/api/admin/users"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "email": "hidden-policy@example.com",
            "username": "hidden_policy",
            "password": "Hidden123!",
            "allowed_models": ["gpt-4.1"]
        }))
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(hidden_policy_response.status(), StatusCode::BAD_REQUEST);
    let hidden_policy_payload: serde_json::Value = hidden_policy_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(
        hidden_policy_payload["detail"],
        "allowed_models 已停用，请通过用户分组管理访问权限"
    );

    create_gateway_handle.abort();
}

#[tokio::test]
async fn gateway_allows_default_user_group_access_policy_updates() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().fallback(any(move |_request: Request| {
        let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
        async move {
            *upstream_hits_inner.lock().expect("mutex should lock") += 1;
            (StatusCode::OK, Body::from("unexpected upstream hit"))
        }
    }));

    let user_repository = Arc::new(
        InMemoryUserReadRepository::seed_auth_users(vec![
            sample_admin_user_with_role("admin-1", "admin", "admin@example.com", "admin"),
            sample_admin_user_with_role("user-2", "user", "bob@example.com", "bob"),
        ])
        .with_export_users(vec![
            sample_admin_export_user_with("admin", true, "admin-1", "admin@example.com", "admin"),
            sample_admin_export_user_with("user", true, "user-2", "bob@example.com", "bob"),
        ]),
    );
    let group = user_repository
        .create_user_group(UpsertUserGroupRecord {
            name: "Baseline".to_string(),
            description: None,
            priority: 0,
            allowed_providers: Some(vec!["openai".to_string()]),
            allowed_providers_mode: "specific".to_string(),
            allowed_api_formats: Some(vec!["openai:chat".to_string()]),
            allowed_api_formats_mode: "specific".to_string(),
            allowed_models: Some(vec!["gpt-4.1".to_string()]),
            allowed_models_mode: "specific".to_string(),
            rate_limit: Some(60),
            rate_limit_mode: "custom".to_string(),
        })
        .await
        .expect("user group should create")
        .expect("user group should exist");
    let group_id = group.id.clone();

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_user_reader_for_tests(user_repository.clone())
                    .with_system_config_values_for_tests(Vec::<(String, serde_json::Value)>::new()),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();

    let default_response = client
        .put(format!("{gateway_url}/api/admin/user-groups/default"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({ "group_id": group_id }))
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(default_response.status(), StatusCode::OK);
    let default_payload: serde_json::Value = default_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(default_payload["default_group_id"], group_id);

    let members = user_repository
        .list_user_group_members(&group_id)
        .await
        .expect("default members should list");
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].user_id, "user-2");

    let update_response = client
        .put(format!("{gateway_url}/api/admin/user-groups/{group_id}"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "name": "Baseline Limited",
            "allowed_providers": ["openai", "anthropic"],
            "allowed_providers_mode": "specific",
            "allowed_api_formats": ["openai:chat"],
            "allowed_api_formats_mode": "specific",
            "allowed_models": ["gpt-4.1", "claude-sonnet-4-5"],
            "allowed_models_mode": "specific",
            "rate_limit": 25,
            "rate_limit_mode": "custom"
        }))
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(update_response.status(), StatusCode::OK);
    let update_payload: serde_json::Value = update_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(update_payload["is_default"], true);
    assert_eq!(update_payload["name"], "Baseline Limited");
    assert_eq!(
        update_payload["allowed_models"],
        json!(["gpt-4.1", "claude-sonnet-4-5"])
    );
    assert_eq!(update_payload["rate_limit"], 25);

    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_allows_removing_default_group_members_when_other_group_remains() {
    let upstream = Router::new().fallback(any(|_request: Request| async {
        (StatusCode::OK, Body::from("unexpected upstream hit"))
    }));

    let user_repository = Arc::new(
        InMemoryUserReadRepository::seed_auth_users(vec![
            sample_admin_user_with_role("admin-1", "admin", "admin@example.com", "admin"),
            sample_admin_user_with_role("user-2", "user", "bob@example.com", "bob"),
            sample_admin_user_with_role("user-3", "user", "carol@example.com", "carol"),
        ])
        .with_export_users(vec![
            sample_admin_export_user_with("admin", true, "admin-1", "admin@example.com", "admin"),
            sample_admin_export_user_with("user", true, "user-2", "bob@example.com", "bob"),
            sample_admin_export_user_with("user", true, "user-3", "carol@example.com", "carol"),
        ]),
    );
    let default_group = user_repository
        .create_user_group(UpsertUserGroupRecord {
            name: "Default".to_string(),
            description: None,
            priority: 0,
            allowed_providers: None,
            allowed_providers_mode: "unrestricted".to_string(),
            allowed_api_formats: None,
            allowed_api_formats_mode: "unrestricted".to_string(),
            allowed_models: None,
            allowed_models_mode: "unrestricted".to_string(),
            rate_limit: None,
            rate_limit_mode: "system".to_string(),
        })
        .await
        .expect("default group should create")
        .expect("default group should exist");
    let team_group = user_repository
        .create_user_group(UpsertUserGroupRecord {
            name: "Team".to_string(),
            description: None,
            priority: 0,
            allowed_providers: None,
            allowed_providers_mode: "unrestricted".to_string(),
            allowed_api_formats: None,
            allowed_api_formats_mode: "unrestricted".to_string(),
            allowed_models: None,
            allowed_models_mode: "unrestricted".to_string(),
            rate_limit: None,
            rate_limit_mode: "system".to_string(),
        })
        .await
        .expect("team group should create")
        .expect("team group should exist");
    user_repository
        .add_user_to_group(&team_group.id, "user-2")
        .await
        .expect("team membership should create");

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_user_reader_for_tests(user_repository.clone())
                    .with_system_config_values_for_tests(vec![(
                        crate::constants::DEFAULT_USER_GROUP_CONFIG_KEY.to_string(),
                        json!(default_group.id),
                    )]),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();

    user_repository
        .add_user_to_group(&default_group.id, "user-2")
        .await
        .expect("default membership should create");
    user_repository
        .add_user_to_group(&default_group.id, "user-3")
        .await
        .expect("default membership should create");

    let remove_user_with_other_group = client
        .put(format!(
            "{gateway_url}/api/admin/user-groups/{}/members",
            default_group.id
        ))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({ "user_ids": ["user-3"] }))
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(remove_user_with_other_group.status(), StatusCode::OK);

    let reject_groupless_user = client
        .put(format!(
            "{gateway_url}/api/admin/user-groups/{}/members",
            default_group.id
        ))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({ "user_ids": [] }))
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(reject_groupless_user.status(), StatusCode::BAD_REQUEST);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_resolves_admin_user_batch_selection_locally() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().fallback(any(move |_request: Request| {
        let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
        async move {
            *upstream_hits_inner.lock().expect("mutex should lock") += 1;
            (StatusCode::OK, Body::from("unexpected upstream hit"))
        }
    }));

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

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(GatewayDataState::with_user_reader_for_tests(
                user_repository,
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/users/resolve-selection"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "filters": {
                "search": "ali",
                "role": "user",
                "is_active": true
            }
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["total"], 1);
    let items = payload["items"].as_array().expect("items should be array");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["user_id"], "user-1");
    assert_eq!(items[0]["username"], "alice");
    assert_eq!(items[0]["email"], "alice@example.com");
    assert_eq!(items[0]["role"], "user");
    assert_eq!(items[0]["is_active"], true);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    let all_filtered_response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/users/resolve-selection"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({ "filters": {} }))
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(all_filtered_response.status(), StatusCode::OK);
    let all_filtered_payload: serde_json::Value = all_filtered_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(all_filtered_payload["total"], 3);

    let empty_selection_response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/users/resolve-selection"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({}))
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(empty_selection_response.status(), StatusCode::BAD_REQUEST);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_user_batch_actions_locally() {
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
            .with_auth_users_for_tests([sample_admin_user("user-1")])
            .with_auth_wallets_for_tests([sample_admin_wallet("user-1", "unlimited")]),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();

    let disable_response = client
        .post(format!("{gateway_url}/api/admin/users/batch-action"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "selection": {
                "user_ids": ["user-1", "user-1", "missing-user"]
            },
            "action": "disable"
        }))
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(disable_response.status(), StatusCode::OK);
    let disable_payload: serde_json::Value = disable_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(disable_payload["total"], 2);
    assert_eq!(disable_payload["success"], 1);
    assert_eq!(disable_payload["failed"], 1);
    assert_eq!(disable_payload["failures"][0]["user_id"], "missing-user");

    let empty_selection_response = client
        .post(format!("{gateway_url}/api/admin/users/batch-action"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "selection": {},
            "action": "disable"
        }))
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(empty_selection_response.status(), StatusCode::BAD_REQUEST);

    let access_response = client
        .post(format!("{gateway_url}/api/admin/users/batch-action"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "selection": {
                "user_ids": ["user-1"]
            },
            "action": "update_access_control",
            "payload": {
                "unlimited": false
            }
        }))
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(access_response.status(), StatusCode::OK);
    let access_payload: serde_json::Value = access_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(access_payload["total"], 1);
    assert_eq!(access_payload["success"], 1);
    assert_eq!(access_payload["failed"], 0);
    assert_eq!(access_payload["modified_fields"], json!(["unlimited"]));

    let hidden_access_response = client
        .post(format!("{gateway_url}/api/admin/users/batch-action"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "selection": {
                "user_ids": ["user-1"]
            },
            "action": "update_access_control",
            "payload": {
                "rate_limit": 0
            }
        }))
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(hidden_access_response.status(), StatusCode::BAD_REQUEST);
    let hidden_access_payload: serde_json::Value = hidden_access_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(
        hidden_access_payload["detail"],
        "rate_limit 已停用，请通过用户分组管理访问权限"
    );

    let role_response = client
        .post(format!("{gateway_url}/api/admin/users/batch-action"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "selection": {
                "user_ids": ["user-1"]
            },
            "action": "update_role",
            "payload": {
                "role": "admin"
            }
        }))
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(role_response.status(), StatusCode::OK);
    let role_payload: serde_json::Value =
        role_response.json().await.expect("json body should parse");
    assert_eq!(role_payload["total"], 1);
    assert_eq!(role_payload["success"], 1);
    assert_eq!(role_payload["modified_fields"], json!(["role"]));

    let blank_role_response = client
        .post(format!("{gateway_url}/api/admin/users/batch-action"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "selection": {
                "user_ids": ["user-1"]
            },
            "action": "update_role",
            "payload": {
                "role": ""
            }
        }))
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(blank_role_response.status(), StatusCode::BAD_REQUEST);

    let enable_admin_response = client
        .post(format!("{gateway_url}/api/admin/users/batch-action"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "selection": {
                "user_ids": ["user-1"]
            },
            "action": "enable"
        }))
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(enable_admin_response.status(), StatusCode::OK);

    let last_admin_demotion_response = client
        .post(format!("{gateway_url}/api/admin/users/batch-action"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "selection": {
                "user_ids": ["user-1"]
            },
            "action": "update_role",
            "payload": {
                "role": "user"
            }
        }))
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(last_admin_demotion_response.status(), StatusCode::OK);
    let last_admin_demotion_payload: serde_json::Value = last_admin_demotion_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(last_admin_demotion_payload["success"], 0);
    assert_eq!(last_admin_demotion_payload["failed"], 1);
    assert_eq!(
        last_admin_demotion_payload["failures"][0]["reason"],
        "不能降级最后一个管理员账户"
    );

    let last_admin_audit_demotion_response = client
        .post(format!("{gateway_url}/api/admin/users/batch-action"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "selection": {
                "user_ids": ["user-1"]
            },
            "action": "update_role",
            "payload": {
                "role": "audit_admin"
            }
        }))
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(last_admin_audit_demotion_response.status(), StatusCode::OK);
    let last_admin_audit_demotion_payload: serde_json::Value = last_admin_audit_demotion_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(last_admin_audit_demotion_payload["success"], 0);
    assert_eq!(last_admin_audit_demotion_payload["failed"], 1);
    assert_eq!(
        last_admin_audit_demotion_payload["failures"][0]["reason"],
        "不能降级最后一个管理员账户"
    );

    let detail_response = client
        .get(format!("{gateway_url}/api/admin/users/user-1"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(detail_response.status(), StatusCode::OK);
    let detail_payload: serde_json::Value = detail_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(detail_payload["role"], "admin");
    assert_eq!(detail_payload["is_active"], true);
    assert_eq!(detail_payload["unlimited"], false);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
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
    assert_eq!(payload["total"], 1);
    let items = payload["items"].as_array().expect("items should be array");
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
    assert_eq!(update_payload["rate_limit"], serde_json::Value::Null);
    assert_eq!(update_payload["unlimited"], true);
    assert_eq!(update_payload["is_active"], false);

    let hidden_update_response = client
        .put(format!("{gateway_url}/api/admin/users/user-1"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "allowed_providers": ["openai"]
        }))
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(hidden_update_response.status(), StatusCode::BAD_REQUEST);
    let hidden_update_payload: serde_json::Value = hidden_update_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(
        hidden_update_payload["detail"],
        "allowed_providers 已停用，请通过用户分组管理访问权限"
    );

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
async fn gateway_rejects_demoting_the_last_active_admin() {
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_auth_users_for_tests([sample_admin_user_with_role(
                "admin-1",
                "admin",
                "admin@example.com",
                "admin",
            )]),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();

    for role in ["user", "audit_admin"] {
        let response = client
            .put(format!("{gateway_url}/api/admin/users/admin-1"))
            .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
            .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
            .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
            .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
            .json(&json!({ "role": role }))
            .send()
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let payload: serde_json::Value = response.json().await.expect("json body should parse");
        assert_eq!(payload["detail"], "不能降级最后一个管理员账户");
    }

    gateway_handle.abort();
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
async fn admin_created_user_keys_inherit_owner_group_policy() {
    let mut admin_snapshot = sample_admin_api_key_snapshot("admin-user", "admin-seed-key");
    admin_snapshot.user_role = "admin".to_string();
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![
        (Some(hash_api_key("sk-admin-seed")), admin_snapshot),
        (
            Some(hash_api_key("sk-target-seed")),
            sample_admin_api_key_snapshot("target-user", "target-seed-key"),
        ),
    ]));
    let user_repository = Arc::new(InMemoryUserReadRepository::seed_auth_users(vec![
        sample_admin_user_with_role("admin-user", "admin", "admin@example.com", "admin"),
        sample_admin_user_with_role("target-user", "user", "target@example.com", "target"),
    ]));

    let admin_group = user_repository
        .create_user_group(UpsertUserGroupRecord {
            name: "Admin OpenAI".to_string(),
            description: None,
            priority: 10,
            allowed_providers: Some(vec!["openai".to_string()]),
            allowed_providers_mode: "specific".to_string(),
            allowed_api_formats: Some(vec!["openai:responses".to_string()]),
            allowed_api_formats_mode: "specific".to_string(),
            allowed_models: Some(vec!["gpt-5.4".to_string()]),
            allowed_models_mode: "specific".to_string(),
            rate_limit: Some(100),
            rate_limit_mode: "custom".to_string(),
        })
        .await
        .expect("admin group should create")
        .expect("admin group should exist");
    user_repository
        .add_user_to_group(&admin_group.id, "admin-user")
        .await
        .expect("admin group membership should create");

    let target_group = user_repository
        .create_user_group(UpsertUserGroupRecord {
            name: "Target Claude".to_string(),
            description: None,
            priority: 10,
            allowed_providers: Some(vec!["anthropic".to_string()]),
            allowed_providers_mode: "specific".to_string(),
            allowed_api_formats: Some(vec!["claude:messages".to_string()]),
            allowed_api_formats_mode: "specific".to_string(),
            allowed_models: Some(vec!["claude-sonnet-4-5".to_string()]),
            allowed_models_mode: "specific".to_string(),
            rate_limit: Some(30),
            rate_limit_mode: "custom".to_string(),
        })
        .await
        .expect("target group should create")
        .expect("target group should exist");
    user_repository
        .add_user_to_group(&target_group.id, "target-user")
        .await
        .expect("target group membership should create");
    user_repository
        .update_user_feature_settings(
            "admin-user",
            Some(json!({"chat_pii_redaction": {"enabled": false}})),
        )
        .await
        .expect("admin feature settings should update");
    user_repository
        .update_user_feature_settings(
            "target-user",
            Some(json!({"chat_pii_redaction": {"enabled": true}})),
        )
        .await
        .expect("target feature settings should update");

    let data_state = GatewayDataState::with_auth_api_key_repository_for_tests(auth_repository)
        .with_user_reader(user_repository);
    let app_state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(data_state);
    let inspection_state = app_state.clone();
    let gateway = build_router_with_state(app_state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/users/target-user/api-keys"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "admin-session")
        .json(&json!({"name": "target-key"}))
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("response should parse");
    let plaintext_key = payload["key"].as_str().expect("plaintext key should exist");
    let api_key_id = payload["id"].as_str().expect("key id should exist");

    let resolved = inspection_state
        .read_cached_auth_api_key_snapshot_by_key_hash(
            &hash_api_key(plaintext_key),
            chrono::Utc::now().timestamp().max(0) as u64,
        )
        .await
        .expect("created key snapshot should resolve")
        .expect("created key snapshot should exist");
    assert_eq!(resolved.user_id, "target-user");
    assert_eq!(
        resolved.effective_allowed_providers(),
        Some(&["anthropic".to_string()][..])
    );
    assert_eq!(
        resolved.effective_allowed_api_formats(),
        Some(&["claude:messages".to_string()][..])
    );
    assert_eq!(
        resolved.effective_allowed_models(),
        Some(&["claude-sonnet-4-5".to_string()][..])
    );
    assert_eq!(resolved.user_rate_limit, Some(30));

    inspection_state
        .update_user_group(
            &target_group.id,
            UpsertUserGroupRecord {
                name: "Target Gemini".to_string(),
                description: None,
                priority: 10,
                allowed_providers: Some(vec!["google".to_string()]),
                allowed_providers_mode: "specific".to_string(),
                allowed_api_formats: Some(vec!["gemini:generate-content".to_string()]),
                allowed_api_formats_mode: "specific".to_string(),
                allowed_models: Some(vec!["gemini-2.5-pro".to_string()]),
                allowed_models_mode: "specific".to_string(),
                rate_limit: Some(15),
                rate_limit_mode: "custom".to_string(),
            },
        )
        .await
        .expect("target group should update")
        .expect("target group should still exist");
    let updated = inspection_state
        .read_cached_auth_api_key_snapshot_by_key_hash(
            &hash_api_key(plaintext_key),
            chrono::Utc::now().timestamp().max(0) as u64,
        )
        .await
        .expect("created key snapshot should refresh")
        .expect("created key snapshot should still exist");
    assert_eq!(updated.user_id, "target-user");
    assert_eq!(
        updated.effective_allowed_providers(),
        Some(&["google".to_string()][..])
    );
    assert_eq!(
        updated.effective_allowed_api_formats(),
        Some(&["gemini:generate-content".to_string()][..])
    );
    assert_eq!(
        updated.effective_allowed_models(),
        Some(&["gemini-2.5-pro".to_string()][..])
    );
    assert_eq!(updated.user_rate_limit, Some(15));

    let target_features = inspection_state
        .read_user_feature_settings("target-user")
        .await
        .expect("target feature settings should resolve");
    let key_features = inspection_state
        .read_auth_api_key_feature_settings("target-user", api_key_id, false)
        .await
        .expect("key feature settings should resolve");
    assert_eq!(
        target_features,
        Some(json!({"chat_pii_redaction": {"enabled": true}}))
    );
    assert_eq!(
        key_features, None,
        "an omitted key override must inherit target settings"
    );

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/users/admin-user/api-keys"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "admin-session")
        .json(&json!({"name": "admin-user-key"}))
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("response should parse");
    let plaintext_key = payload["key"].as_str().expect("plaintext key should exist");

    let resolved = inspection_state
        .read_cached_auth_api_key_snapshot_by_key_hash(
            &hash_api_key(plaintext_key),
            chrono::Utc::now().timestamp().max(0) as u64,
        )
        .await
        .expect("admin-owned user key snapshot should resolve")
        .expect("admin-owned user key snapshot should exist");
    assert_eq!(resolved.user_id, "admin-user");
    assert!(!resolved.api_key_is_standalone);
    assert_eq!(
        resolved.effective_allowed_providers(),
        Some(&["openai".to_string()][..])
    );
    assert_eq!(
        resolved.effective_allowed_api_formats(),
        Some(&["openai:responses".to_string()][..])
    );
    assert_eq!(
        resolved.effective_allowed_models(),
        Some(&["gpt-5.4".to_string()][..])
    );
    assert_eq!(resolved.user_rate_limit, Some(100));

    gateway_handle.abort();
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
            Arc::new(InMemoryUserReadRepository::seed_auth_users(Vec::new()).read_only()),
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
async fn gateway_allows_admin_update_user_to_clear_explicit_groups() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().fallback(any(move |_request: Request| {
        let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
        async move {
            *upstream_hits_inner.lock().expect("mutex should lock") += 1;
            (StatusCode::OK, Body::from("unexpected upstream hit"))
        }
    }));

    let user_repository = Arc::new(
        InMemoryUserReadRepository::seed_auth_users(vec![sample_admin_user("user-1")])
            .with_export_users(vec![sample_admin_export_user("user-1")]),
    );
    let default_group = user_repository
        .create_user_group(UpsertUserGroupRecord {
            name: "GPT Adapt".to_string(),
            description: None,
            priority: 0,
            allowed_providers: None,
            allowed_providers_mode: "unrestricted".to_string(),
            allowed_api_formats: None,
            allowed_api_formats_mode: "unrestricted".to_string(),
            allowed_models: None,
            allowed_models_mode: "unrestricted".to_string(),
            rate_limit: None,
            rate_limit_mode: "system".to_string(),
        })
        .await
        .expect("default group should create")
        .expect("default group should exist");
    user_repository
        .add_user_to_group(&default_group.id, "user-1")
        .await
        .expect("default membership should create");

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_user_reader_for_tests(user_repository.clone())
                    .with_system_config_values_for_tests(vec![(
                        crate::constants::DEFAULT_USER_GROUP_CONFIG_KEY.to_string(),
                        json!(default_group.id),
                    )]),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .put(format!("{gateway_url}/api/admin/users/user-1"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({ "group_ids": [] }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["groups"], json!([]));
    assert!(user_repository
        .list_user_groups_for_user("user-1")
        .await
        .expect("memberships should load")
        .is_empty());
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

    let user_repository = Arc::new(
        InMemoryUserReadRepository::seed_auth_users(vec![sample_admin_user("user-1")]).read_only(),
    );

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
