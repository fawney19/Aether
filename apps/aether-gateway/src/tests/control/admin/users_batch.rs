use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use aether_data::repository::management_tokens::InMemoryManagementTokenRepository;
use aether_data::repository::users::{InMemoryUserReadRepository, StoredUserAuthRecord};
use aether_data::repository::wallet::StoredWalletSnapshot;
use axum::http::StatusCode;
use chrono::Utc;
use reqwest::{Client, RequestBuilder, Response};
use serde_json::{json, Value};

use super::super::{
    build_router_with_state, hash_management_token, sample_management_token, start_server, AppState,
};
use crate::data::GatewayDataState;

fn admin_headers(request: RequestBuilder) -> RequestBuilder {
    request
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(crate::constants::TRUSTED_ADMIN_USER_ID_HEADER, "admin-user")
        .header(crate::constants::TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(
            crate::constants::TRUSTED_ADMIN_SESSION_ID_HEADER,
            "session-admin",
        )
}

fn sample_user(user_id: &str) -> StoredUserAuthRecord {
    sample_user_with_role(user_id, "user")
}

fn sample_user_with_role(user_id: &str, role: &str) -> StoredUserAuthRecord {
    StoredUserAuthRecord::new(
        user_id.to_string(),
        Some(format!("{user_id}@example.com")),
        true,
        user_id.to_string(),
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
    .expect("test user should build")
}

fn sample_wallet(user_id: &str, balance: f64, gift_balance: f64) -> StoredWalletSnapshot {
    StoredWalletSnapshot::new(
        format!("wallet-{user_id}"),
        Some(user_id.to_string()),
        None,
        balance,
        gift_balance,
        "finite".to_string(),
        "USD".to_string(),
        "active".to_string(),
        balance.max(0.0),
        0.0,
        0.0,
        0.0,
        1_710_000_000,
    )
    .expect("test wallet should build")
}

async fn post_batch_action(client: &Client, gateway_url: &str, payload: Value) -> Response {
    static NEXT_TEST_IDEMPOTENCY_KEY: AtomicU64 = AtomicU64::new(1);
    let mut payload = payload;
    if payload.get("action").and_then(Value::as_str) == Some("adjust_wallet_balance")
        && payload.get("idempotency_key").is_none()
    {
        let sequence = NEXT_TEST_IDEMPOTENCY_KEY.fetch_add(1, Ordering::Relaxed);
        payload["idempotency_key"] = json!(format!("test-wallet-batch-{sequence}"));
    }
    admin_headers(client.post(format!("{gateway_url}/api/admin/users/batch-action")))
        .json(&payload)
        .send()
        .await
        .expect("batch request should complete")
}

async fn wallet_detail(client: &Client, gateway_url: &str, user_id: &str) -> Value {
    admin_headers(client.get(format!("{gateway_url}/api/admin/wallets/wallet-{user_id}")))
        .send()
        .await
        .expect("wallet lookup should complete")
        .json()
        .await
        .expect("wallet response should parse")
}

#[tokio::test]
async fn gateway_batches_wallet_addition_deduction_and_clamped_deduction_per_user() {
    let state = AppState::new()
        .expect("gateway should build")
        .with_auth_users_for_tests([sample_user("user-1"), sample_user("user-2")])
        .with_auth_wallets_for_tests([
            sample_wallet("user-1", 10.0, 3.0),
            sample_wallet("user-2", 2.0, 1.0),
        ]);
    let (gateway_url, gateway_handle) = start_server(build_router_with_state(state)).await;
    let client = Client::new();
    let selection = json!({ "user_ids": ["user-1", "user-2"] });

    let add_response = post_batch_action(
        &client,
        &gateway_url,
        json!({
            "selection": selection.clone(),
            "action": "adjust_wallet_balance",
            "payload": { "operation": "add", "amount": 5.0 }
        }),
    )
    .await;
    assert_eq!(add_response.status(), StatusCode::OK);
    let add_result: Value = add_response.json().await.expect("response should parse");
    assert_eq!(add_result["success"], 2);
    assert_eq!(add_result["failed"], 0);
    assert_eq!(add_result["modified_fields"], json!(["wallet_balance"]));
    assert_eq!(
        wallet_detail(&client, &gateway_url, "user-1").await["balance"],
        18.0
    );
    assert_eq!(
        wallet_detail(&client, &gateway_url, "user-2").await["balance"],
        8.0
    );

    let deduct_response = post_batch_action(
        &client,
        &gateway_url,
        json!({
            "selection": selection.clone(),
            "action": "adjust_wallet_balance",
            "payload": { "operation": "deduct", "amount": 4.0 }
        }),
    )
    .await;
    assert_eq!(deduct_response.status(), StatusCode::OK);
    let deduct_result: Value = deduct_response.json().await.expect("response should parse");
    assert_eq!(deduct_result["success"], 2);
    assert_eq!(
        wallet_detail(&client, &gateway_url, "user-1").await["balance"],
        14.0
    );
    assert_eq!(
        wallet_detail(&client, &gateway_url, "user-2").await["balance"],
        4.0
    );

    let over_deduct_response = post_batch_action(
        &client,
        &gateway_url,
        json!({
            "selection": selection,
            "action": "adjust_wallet_balance",
            "payload": { "operation": "deduct", "amount": 100.0 }
        }),
    )
    .await;
    assert_eq!(over_deduct_response.status(), StatusCode::OK);
    let over_deduct_result: Value = over_deduct_response
        .json()
        .await
        .expect("response should parse");
    assert_eq!(over_deduct_result["success"], 2);
    assert_eq!(
        wallet_detail(&client, &gateway_url, "user-1").await["balance"],
        0.0
    );
    assert_eq!(
        wallet_detail(&client, &gateway_url, "user-2").await["balance"],
        0.0
    );

    gateway_handle.abort();
}

#[tokio::test]
async fn gateway_replays_wallet_batch_idempotently_and_rejects_key_reuse() {
    let state = AppState::new()
        .expect("gateway should build")
        .with_auth_users_for_tests([sample_user("user-1")])
        .with_auth_wallets_for_tests([sample_wallet("user-1", 10.0, 0.0)]);
    let (gateway_url, gateway_handle) = start_server(build_router_with_state(state)).await;
    let client = Client::new();
    let request = json!({
        "selection": { "user_ids": ["user-1"] },
        "action": "adjust_wallet_balance",
        "payload": { "operation": "add", "amount": 5.0 },
        "idempotency_key": "same-wallet-batch"
    });

    let first = post_batch_action(&client, &gateway_url, request.clone()).await;
    assert_eq!(first.status(), StatusCode::OK);
    let first_result: Value = first.json().await.expect("response should parse");
    assert_eq!(first_result["success"], 1);
    assert_eq!(
        wallet_detail(&client, &gateway_url, "user-1").await["balance"],
        15.0
    );

    let replay = post_batch_action(&client, &gateway_url, request.clone()).await;
    assert_eq!(replay.status(), StatusCode::OK);
    let replay_result: Value = replay.json().await.expect("response should parse");
    assert_eq!(replay_result["success"], 1);
    assert_eq!(
        wallet_detail(&client, &gateway_url, "user-1").await["balance"],
        15.0
    );

    let changed_request = json!({
        "selection": { "user_ids": ["user-1"] },
        "action": "adjust_wallet_balance",
        "payload": { "operation": "add", "amount": 50.0 },
        "idempotency_key": "same-wallet-batch"
    });
    let conflict = post_batch_action(&client, &gateway_url, changed_request).await;
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    assert_eq!(
        wallet_detail(&client, &gateway_url, "user-1").await["balance"],
        15.0
    );

    gateway_handle.abort();
}

#[tokio::test]
async fn gateway_returns_partial_results_when_failure_recording_fails() {
    let state = AppState::new()
        .expect("gateway should build")
        .with_auth_users_for_tests([sample_user("user-1"), sample_user("user-2")])
        .with_auth_wallets_for_tests([sample_wallet("user-1", 10.0, 0.0)])
        .fail_auth_wallet_batch_failure_record_for_tests("user-2");
    let (gateway_url, gateway_handle) = start_server(build_router_with_state(state)).await;
    let client = Client::new();
    let request = json!({
        "selection": { "user_ids": ["user-1", "user-2"] },
        "action": "adjust_wallet_balance",
        "payload": { "operation": "add", "amount": 5.0 },
        "idempotency_key": "failure-record-wallet-batch"
    });

    let response = post_batch_action(&client, &gateway_url, request).await;
    assert_eq!(response.status(), StatusCode::OK);
    let result: Value = response.json().await.expect("response should parse");
    assert_eq!(result["success"], 1);
    assert_eq!(result["failed"], 1);
    assert_eq!(result["interrupted"], true);
    assert_eq!(result["completed_user_ids"], json!(["user-1"]));
    assert_eq!(result["uncertain_user_ids"], json!([]));
    assert_eq!(result["unprocessed_user_ids"], json!(["user-2"]));
    assert_eq!(
        wallet_detail(&client, &gateway_url, "user-1").await["balance"],
        15.0
    );

    gateway_handle.abort();
}

#[tokio::test]
async fn gateway_records_zero_delta_batch_for_later_replay() {
    let state = AppState::new()
        .expect("gateway should build")
        .with_auth_users_for_tests([sample_user("user-1")])
        .with_auth_wallets_for_tests([sample_wallet("user-1", 0.0, 0.0)]);
    let (gateway_url, gateway_handle) = start_server(build_router_with_state(state)).await;
    let client = Client::new();
    let deduction = json!({
        "selection": { "user_ids": ["user-1"] },
        "action": "adjust_wallet_balance",
        "payload": { "operation": "deduct", "amount": 5.0 },
        "idempotency_key": "zero-delta-wallet-batch"
    });
    let first = post_batch_action(&client, &gateway_url, deduction.clone()).await;
    assert_eq!(first.status(), StatusCode::OK);

    let top_up = post_batch_action(
        &client,
        &gateway_url,
        json!({
            "selection": { "user_ids": ["user-1"] },
            "action": "adjust_wallet_balance",
            "payload": { "operation": "add", "amount": 10.0 },
            "idempotency_key": "wallet-top-up-batch"
        }),
    )
    .await;
    assert_eq!(top_up.status(), StatusCode::OK);
    assert_eq!(
        wallet_detail(&client, &gateway_url, "user-1").await["balance"],
        10.0
    );

    let replay = post_batch_action(&client, &gateway_url, deduction).await;
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(
        wallet_detail(&client, &gateway_url, "user-1").await["balance"],
        10.0
    );

    gateway_handle.abort();
}

#[tokio::test]
async fn gateway_requires_wallet_write_permission_for_batch_balance_adjustments() {
    let users_write_token = "ae-batch-users-write-only";
    let wallet_write_token = "ae-batch-users-wallet-write";
    let wallet_admin_token = "ae-batch-users-wallet-admin";
    let token_owner = sample_user_with_role("token-owner", "admin");
    let target_user = sample_user("user-1");
    let mut users_only = sample_management_token(
        "token-users-write-only",
        &token_owner.id,
        &token_owner.username,
        true,
    );
    users_only.token.allowed_ips = None;
    users_only.token.permissions = Some(json!(["admin:users:write"]));
    let mut users_and_wallets = sample_management_token(
        "token-users-and-wallet-write",
        &token_owner.id,
        &token_owner.username,
        true,
    );
    users_and_wallets.token.allowed_ips = None;
    users_and_wallets.token.permissions = Some(json!(["admin:users:write", "admin:wallets:write"]));
    let mut wallets_admin = sample_management_token(
        "token-users-wallet-admin",
        &token_owner.id,
        &token_owner.username,
        true,
    );
    wallets_admin.token.allowed_ips = None;
    wallets_admin.token.permissions = Some(json!(["admin:users:write", "admin:wallets:admin"]));
    let token_repository = Arc::new(InMemoryManagementTokenRepository::seed_with_hashes(
        vec![users_only, users_and_wallets, wallets_admin],
        vec![
            (
                hash_management_token(users_write_token),
                "token-users-write-only".to_string(),
            ),
            (
                hash_management_token(wallet_write_token),
                "token-users-and-wallet-write".to_string(),
            ),
            (
                hash_management_token(wallet_admin_token),
                "token-users-wallet-admin".to_string(),
            ),
        ],
    ));
    let user_repository = Arc::new(InMemoryUserReadRepository::seed_auth_users(vec![
        token_owner.clone(),
        target_user.clone(),
    ]));
    let data = GatewayDataState::with_management_token_repository_for_tests(token_repository)
        .with_user_reader(user_repository);
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(data)
        .with_auth_users_for_tests([token_owner, target_user])
        .with_auth_wallets_for_tests([sample_wallet("user-1", 10.0, 0.0)]);
    let (gateway_url, gateway_handle) = start_server(build_router_with_state(state)).await;
    let client = Client::new();
    let payload = json!({
        "selection": { "user_ids": ["user-1"] },
        "action": "adjust_wallet_balance",
        "payload": { "operation": "add", "amount": 5.0 },
        "idempotency_key": "wallet-write-batch"
    });

    let denied = client
        .post(format!("{gateway_url}/api/admin/users/batch-action"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .bearer_auth(users_write_token)
        .json(&payload)
        .send()
        .await
        .expect("users-only management token request should complete");
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    let denied_payload: Value = denied.json().await.expect("response should parse");
    assert_eq!(
        denied_payload["required_permissions"],
        json!(["admin:wallets:write", "admin:wallets:admin"])
    );
    assert_eq!(denied_payload["permission_mode"], "any_of");
    assert_eq!(
        wallet_detail(&client, &gateway_url, "user-1").await["balance"],
        10.0
    );

    let allowed = client
        .post(format!("{gateway_url}/api/admin/users/batch-action"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .bearer_auth(wallet_write_token)
        .json(&payload)
        .send()
        .await
        .expect("wallet-write management token request should complete");
    assert_eq!(allowed.status(), StatusCode::OK);
    let result: Value = allowed.json().await.expect("response should parse");
    assert_eq!(result["success"], 1);
    assert_eq!(
        wallet_detail(&client, &gateway_url, "user-1").await["balance"],
        15.0
    );

    let allowed_with_wallet_admin = client
        .post(format!("{gateway_url}/api/admin/users/batch-action"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .bearer_auth(wallet_admin_token)
        .json(&json!({
            "selection": payload["selection"],
            "action": payload["action"],
            "payload": payload["payload"],
            "idempotency_key": "wallet-admin-batch"
        }))
        .send()
        .await
        .expect("wallet-admin management token request should complete");
    assert_eq!(allowed_with_wallet_admin.status(), StatusCode::OK);
    let result: Value = allowed_with_wallet_admin
        .json()
        .await
        .expect("response should parse");
    assert_eq!(result["success"], 1);
    assert_eq!(
        wallet_detail(&client, &gateway_url, "user-1").await["balance"],
        20.0
    );

    gateway_handle.abort();
}

#[tokio::test]
async fn gateway_reports_completed_uncertain_and_unprocessed_users_after_adjustment_error() {
    let state = AppState::new()
        .expect("gateway should build")
        .with_auth_users_for_tests([
            sample_user("user-1"),
            sample_user("user-2"),
            sample_user("user-3"),
        ])
        .with_auth_wallets_for_tests([
            sample_wallet("user-1", 10.0, 0.0),
            sample_wallet("user-2", 20.0, 0.0),
            sample_wallet("user-3", 30.0, 0.0),
        ])
        .fail_auth_wallet_adjustment_for_tests("wallet-user-2");
    let (gateway_url, gateway_handle) = start_server(build_router_with_state(state)).await;
    let client = Client::new();

    let response = post_batch_action(
        &client,
        &gateway_url,
        json!({
            "selection": { "user_ids": ["user-1", "user-2", "user-3"] },
            "action": "adjust_wallet_balance",
            "payload": { "operation": "add", "amount": 5.0 }
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let result: Value = response.json().await.expect("response should parse");
    assert_eq!(result["interrupted"], true);
    assert_eq!(result["success"], 1);
    assert_eq!(result["failed"], 2);
    assert_eq!(result["completed_user_ids"], json!(["user-1"]));
    assert_eq!(result["uncertain_user_ids"], json!(["user-2"]));
    assert_eq!(result["unprocessed_user_ids"], json!(["user-3"]));
    assert_eq!(result["failures"][0]["user_id"], "user-2");
    assert_eq!(result["failures"][1]["user_id"], "user-3");
    assert_eq!(
        wallet_detail(&client, &gateway_url, "user-1").await["balance"],
        15.0
    );
    assert_eq!(
        wallet_detail(&client, &gateway_url, "user-2").await["balance"],
        20.0
    );
    assert_eq!(
        wallet_detail(&client, &gateway_url, "user-3").await["balance"],
        30.0
    );

    gateway_handle.abort();
}

#[tokio::test]
async fn gateway_reports_wallet_lookup_failure_as_unprocessed() {
    let state = AppState::new()
        .expect("gateway should build")
        .with_auth_users_for_tests([
            sample_user("user-1"),
            sample_user("user-2"),
            sample_user("user-3"),
        ])
        .with_auth_wallets_for_tests([
            sample_wallet("user-1", 10.0, 0.0),
            sample_wallet("user-2", 20.0, 0.0),
            sample_wallet("user-3", 30.0, 0.0),
        ])
        .fail_auth_wallet_lookup_for_tests("user-2");
    let (gateway_url, gateway_handle) = start_server(build_router_with_state(state)).await;
    let client = Client::new();

    let response = post_batch_action(
        &client,
        &gateway_url,
        json!({
            "selection": { "user_ids": ["user-1", "user-2", "user-3"] },
            "action": "adjust_wallet_balance",
            "payload": { "operation": "add", "amount": 5.0 }
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let result: Value = response.json().await.expect("response should parse");
    assert_eq!(result["interrupted"], true);
    assert_eq!(result["completed_user_ids"], json!(["user-1"]));
    assert_eq!(result["uncertain_user_ids"], json!([]));
    assert_eq!(result["unprocessed_user_ids"], json!(["user-2", "user-3"]));
    assert_eq!(
        wallet_detail(&client, &gateway_url, "user-1").await["balance"],
        15.0
    );
    assert_eq!(
        wallet_detail(&client, &gateway_url, "user-2").await["balance"],
        20.0
    );
    assert_eq!(
        wallet_detail(&client, &gateway_url, "user-3").await["balance"],
        30.0
    );

    gateway_handle.abort();
}

#[tokio::test]
async fn gateway_reports_wallet_limit_lookup_failure_as_unprocessed() {
    let state = AppState::new()
        .expect("gateway should build")
        .with_auth_users_for_tests([
            sample_user("user-1"),
            sample_user("user-2"),
            sample_user("user-3"),
        ])
        .with_auth_wallets_for_tests([
            sample_wallet("user-1", 10.0, 0.0),
            sample_wallet("user-2", 20.0, 0.0),
            sample_wallet("user-3", 30.0, 0.0),
        ])
        .fail_auth_wallet_lookup_for_tests("user-2");
    let (gateway_url, gateway_handle) = start_server(build_router_with_state(state)).await;
    let client = Client::new();

    let response = post_batch_action(
        &client,
        &gateway_url,
        json!({
            "selection": { "user_ids": ["user-1", "user-2", "user-3"] },
            "action": "update_access_control",
            "payload": { "unlimited": true }
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let result: Value = response.json().await.expect("response should parse");
    assert_eq!(result["interrupted"], true);
    assert_eq!(result["completed_user_ids"], json!(["user-1"]));
    assert_eq!(result["uncertain_user_ids"], json!([]));
    assert_eq!(result["unprocessed_user_ids"], json!(["user-2", "user-3"]));

    gateway_handle.abort();
}

#[tokio::test]
async fn gateway_reports_missing_wallet_and_skips_zero_delta_for_non_positive_balance() {
    let state = AppState::new()
        .expect("gateway should build")
        .with_auth_users_for_tests([sample_user("user-negative"), sample_user("user-no-wallet")])
        .with_auth_wallets_for_tests([sample_wallet("user-negative", -2.0, 1.0)]);
    let (gateway_url, gateway_handle) = start_server(build_router_with_state(state)).await;
    let client = Client::new();

    let response = post_batch_action(
        &client,
        &gateway_url,
        json!({
            "selection": { "user_ids": ["user-negative", "user-no-wallet"] },
            "action": "adjust_wallet_balance",
            "payload": { "operation": "deduct", "amount": 10.0 }
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let result: Value = response.json().await.expect("response should parse");
    assert_eq!(result["success"], 1);
    assert_eq!(result["failed"], 1);
    assert_eq!(result["failures"][0]["user_id"], "user-no-wallet");
    assert_eq!(result["failures"][0]["reason"], "用户钱包不可用");

    let wallet = wallet_detail(&client, &gateway_url, "user-negative").await;
    assert_eq!(wallet["balance"], -1.0);
    assert_eq!(wallet["total_adjusted"], 0.0);

    gateway_handle.abort();
}

#[tokio::test]
async fn gateway_rejects_zero_and_non_finite_batch_wallet_adjustments() {
    let state = AppState::new()
        .expect("gateway should build")
        .with_auth_users_for_tests([sample_user("user-1")])
        .with_auth_wallets_for_tests([sample_wallet("user-1", 10.0, 0.0)]);
    let (gateway_url, gateway_handle) = start_server(build_router_with_state(state)).await;
    let client = Client::new();

    let zero_response = post_batch_action(
        &client,
        &gateway_url,
        json!({
            "selection": { "user_ids": ["user-1"] },
            "action": "adjust_wallet_balance",
            "payload": { "operation": "add", "amount": 0.0 }
        }),
    )
    .await;
    assert_eq!(zero_response.status(), StatusCode::BAD_REQUEST);

    let non_finite_response = admin_headers(
        client.post(format!("{gateway_url}/api/admin/users/batch-action")),
    )
    .header(reqwest::header::CONTENT_TYPE, "application/json")
    .body(
        r#"{"selection":{"user_ids":["user-1"]},"action":"adjust_wallet_balance","payload":{"operation":"add","amount":1e999}}"#,
    )
    .send()
    .await
    .expect("non-finite amount request should complete");
    assert_eq!(non_finite_response.status(), StatusCode::BAD_REQUEST);

    gateway_handle.abort();
}
