use std::sync::{Arc, Mutex};

use aether_data::repository::auth::{
    InMemoryAuthApiKeySnapshotRepository, StoredAuthApiKeySnapshot,
};
use aether_data::repository::users::{InMemoryUserReadRepository, StoredUserAuthRecord};
use aether_data::repository::wallet::InMemoryWalletRepository;
use aether_data::repository::wallet::StoredWalletSnapshot;
use axum::body::Body;
use axum::routing::{any, get, post};
use axum::{extract::Request, Router};
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

const ADMIN_WALLETS_DATA_UNAVAILABLE_DETAIL: &str = "Admin wallets data unavailable";
const ADMIN_WALLETS_API_KEY_REFUND_DETAIL: &str = "独立密钥钱包不支持退款审批";

async fn assert_admin_wallets_route_returns_local_503(path: &str) {
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
    let gateway = build_router_with_state(AppState::new().expect("gateway should build"));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}{path}"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["detail"], ADMIN_WALLETS_DATA_UNAVAILABLE_DETAIL);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

async fn assert_admin_wallets_post_route_returns_local_503(path: &str) {
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
    let gateway = build_router_with_state(AppState::new().expect("gateway should build"));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
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
    assert_eq!(payload["detail"], ADMIN_WALLETS_DATA_UNAVAILABLE_DETAIL);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

fn sample_wallet_snapshot(
    wallet_id: &str,
    user_id: Option<&str>,
    api_key_id: Option<&str>,
    limit_mode: &str,
) -> StoredWalletSnapshot {
    StoredWalletSnapshot::new(
        wallet_id.to_string(),
        user_id.map(ToOwned::to_owned),
        api_key_id.map(ToOwned::to_owned),
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

fn sample_payment_order_record(
    order_id: &str,
    wallet_id: &str,
    user_id: Option<&str>,
    amount_usd: f64,
    refunded_amount_usd: f64,
    refundable_amount_usd: f64,
) -> crate::AdminWalletPaymentOrderRecord {
    crate::AdminWalletPaymentOrderRecord {
        id: order_id.to_string(),
        order_no: format!("po-{order_id}"),
        wallet_id: wallet_id.to_string(),
        user_id: user_id.map(ToOwned::to_owned),
        amount_usd,
        pay_amount: None,
        pay_currency: None,
        exchange_rate: None,
        refunded_amount_usd,
        refundable_amount_usd,
        payment_method: "admin_manual".to_string(),
        gateway_order_id: None,
        status: "credited".to_string(),
        gateway_response: None,
        created_at_unix_ms: 1_710_000_000,
        paid_at_unix_secs: Some(1_710_000_060),
        credited_at_unix_secs: Some(1_710_000_120),
        expires_at_unix_secs: None,
    }
}

fn sample_transaction_record(
    transaction_id: &str,
    wallet_id: &str,
    operator_id: Option<&str>,
) -> crate::AdminWalletTransactionRecord {
    crate::AdminWalletTransactionRecord {
        id: transaction_id.to_string(),
        wallet_id: wallet_id.to_string(),
        category: "recharge".to_string(),
        reason_code: "manual_credit".to_string(),
        amount: 8.0,
        balance_before: 15.0,
        balance_after: 23.0,
        recharge_balance_before: 12.5,
        recharge_balance_after: 20.5,
        gift_balance_before: 2.5,
        gift_balance_after: 2.5,
        link_type: Some("payment_order".to_string()),
        link_id: Some("order-123".to_string()),
        operator_id: operator_id.map(ToOwned::to_owned),
        description: Some("Admin manual credit".to_string()),
        created_at_unix_ms: 1_710_000_200,
    }
}

#[allow(clippy::too_many_arguments)]
fn sample_refund_record(
    refund_id: &str,
    wallet_id: &str,
    user_id: Option<&str>,
    payment_order_id: Option<&str>,
    amount_usd: f64,
    status: &str,
    approved_by: Option<&str>,
    processed_by: Option<&str>,
    processed_at_unix_secs: Option<u64>,
) -> crate::AdminWalletRefundRecord {
    crate::AdminWalletRefundRecord {
        id: refund_id.to_string(),
        refund_no: format!("rf-{refund_id}"),
        wallet_id: wallet_id.to_string(),
        user_id: user_id.map(ToOwned::to_owned),
        payment_order_id: payment_order_id.map(ToOwned::to_owned),
        source_type: "payment_order".to_string(),
        source_id: payment_order_id.map(ToOwned::to_owned),
        refund_mode: "original_channel".to_string(),
        amount_usd,
        status: status.to_string(),
        reason: Some("用户申请退款".to_string()),
        failure_reason: None,
        gateway_refund_id: None,
        payout_method: Some("manual".to_string()),
        payout_reference: None,
        payout_proof: None,
        requested_by: user_id.map(ToOwned::to_owned),
        approved_by: approved_by.map(ToOwned::to_owned),
        processed_by: processed_by.map(ToOwned::to_owned),
        created_at_unix_ms: 1_710_000_000,
        updated_at_unix_secs: processed_at_unix_secs.unwrap_or(1_710_000_000),
        processed_at_unix_secs,
        completed_at_unix_secs: None,
    }
}

#[tokio::test]
async fn gateway_handles_admin_wallets_list_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/wallets",
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
            "{gateway_url}/api/admin/wallets?status=active&limit=20&offset=1"
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
    assert_eq!(payload["items"], json!([]));
    assert_eq!(payload["total"], json!(0));
    assert_eq!(payload["limit"], json!(20));
    assert_eq!(payload["offset"], json!(1));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_wallets_list_locally_with_bearer_admin_session() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/wallets",
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
    let access_token = issue_test_admin_access_token(&state, "device-admin-wallets").await;
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/wallets?limit=200&offset=0"
        ))
        .header("authorization", format!("Bearer {access_token}"))
        .header("x-client-device-id", "device-admin-wallets")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["items"], json!([]));
    assert_eq!(payload["total"], json!(0));
    assert_eq!(payload["limit"], json!(200));
    assert_eq!(payload["offset"], json!(0));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_wallets_ledger_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/wallets/ledger",
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
            "{gateway_url}/api/admin/wallets/ledger?owner_type=user&category=adjust&limit=5&offset=2"
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
    assert_eq!(payload["items"], json!([]));
    assert_eq!(payload["total"], json!(0));
    assert_eq!(payload["limit"], json!(5));
    assert_eq!(payload["offset"], json!(2));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_wallets_refund_requests_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/wallets/refund-requests",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let user_alice = StoredUserAuthRecord::new(
        "user-123".to_string(),
        Some("alice@example.com".to_string()),
        true,
        "alice".to_string(),
        Some("hash".to_string()),
        "user".to_string(),
        "local".to_string(),
        None,
        None,
        None,
        true,
        false,
        None,
        None,
    )
    .expect("user should build");
    let user_bob = StoredUserAuthRecord::new(
        "user-456".to_string(),
        Some("bob@example.com".to_string()),
        true,
        "bob".to_string(),
        Some("hash".to_string()),
        "user".to_string(),
        "local".to_string(),
        None,
        None,
        None,
        true,
        false,
        None,
        None,
    )
    .expect("user should build");
    let wallet_repository = Arc::new(InMemoryWalletRepository::seed(vec![
        sample_wallet_snapshot("wallet-user-123", Some("user-123"), None, "finite"),
        sample_wallet_snapshot("wallet-user-456", Some("user-456"), None, "finite"),
        sample_wallet_snapshot("wallet-key-123", None, Some("key-123"), "unlimited"),
    ]));
    let user_repository = Arc::new(InMemoryUserReadRepository::seed_auth_users(vec![
        user_alice, user_bob,
    ]));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_user_and_wallet_for_tests(
                    user_repository,
                    wallet_repository,
                ),
            )
            .with_auth_wallets_for_tests([
                sample_wallet_snapshot("wallet-user-123", Some("user-123"), None, "finite"),
                sample_wallet_snapshot("wallet-user-456", Some("user-456"), None, "finite"),
                sample_wallet_snapshot("wallet-key-123", None, Some("key-123"), "unlimited"),
            ])
            .with_admin_wallet_refunds_for_tests([
                {
                    let mut refund = sample_refund_record(
                        "refund-user-newest",
                        "wallet-user-123",
                        Some("user-123"),
                        Some("po-user-1"),
                        4.0,
                        "pending_approval",
                        None,
                        None,
                        None,
                    );
                    refund.created_at_unix_ms = 1_710_000_300;
                    refund
                },
                {
                    let mut refund = sample_refund_record(
                        "refund-key-ignored",
                        "wallet-key-123",
                        None,
                        Some("po-key-1"),
                        6.0,
                        "pending_approval",
                        None,
                        None,
                        None,
                    );
                    refund.created_at_unix_ms = 1_710_000_200;
                    refund
                },
                sample_refund_record(
                    "refund-user-older",
                    "wallet-user-456",
                    Some("user-456"),
                    Some("po-user-2"),
                    2.0,
                    "pending_approval",
                    None,
                    None,
                    None,
                ),
            ]),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/wallets/refund-requests?status=pending_approval&limit=8&offset=1"
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
    assert_eq!(payload["items"].as_array().map(Vec::len), Some(1));
    assert_eq!(payload["items"][0]["id"], json!("refund-user-older"));
    assert_eq!(payload["items"][0]["wallet_id"], json!("wallet-user-456"));
    assert_eq!(payload["items"][0]["owner_type"], json!("user"));
    assert_eq!(payload["items"][0]["owner_name"], json!("bob"));
    assert_eq!(payload["items"][0]["status"], json!("pending_approval"));
    assert_eq!(payload["items"][0]["payment_order_id"], json!("po-user-2"));
    assert_eq!(payload["total"], json!(2));
    assert_eq!(payload["limit"], json!(8));
    assert_eq!(payload["offset"], json!(1));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_rejects_admin_wallets_refund_requests_for_api_key_owner_type_locally() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/wallets/refund-requests",
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
            "{gateway_url}/api/admin/wallets/refund-requests?owner_type=api_key"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(
        payload["detail"],
        json!(ADMIN_WALLETS_API_KEY_REFUND_DETAIL)
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_wallets_detail_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/wallets/wallet-user-123",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let user = StoredUserAuthRecord::new(
        "user-123".to_string(),
        Some("alice@example.com".to_string()),
        true,
        "alice".to_string(),
        Some("hash".to_string()),
        "user".to_string(),
        "local".to_string(),
        None,
        None,
        None,
        true,
        false,
        None,
        None,
    )
    .expect("user should build");
    let wallet_repository = Arc::new(InMemoryWalletRepository::seed(vec![
        sample_wallet_snapshot("wallet-user-123", Some("user-123"), None, "finite"),
    ]));
    let user_repository = Arc::new(InMemoryUserReadRepository::seed_auth_users(vec![
        user.clone()
    ]));
    let operator = StoredUserAuthRecord::new(
        "admin-user-operator".to_string(),
        Some("admin@example.com".to_string()),
        true,
        "admin-operator".to_string(),
        Some("hash".to_string()),
        "admin".to_string(),
        "local".to_string(),
        None,
        None,
        None,
        true,
        false,
        None,
        None,
    )
    .expect("operator should build");

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_user_and_wallet_for_tests(
                    user_repository,
                    wallet_repository,
                ),
            )
            .with_auth_users_for_tests([operator])
            .with_admin_wallet_transactions_for_tests([
                {
                    let mut transaction = sample_transaction_record(
                        "tx-456",
                        "wallet-user-123",
                        Some("admin-user-operator"),
                    );
                    transaction.created_at_unix_ms = 1_710_000_300;
                    transaction.description = Some("Most recent credit".to_string());
                    transaction
                },
                sample_transaction_record("tx-123", "wallet-user-123", Some("admin-user-operator")),
            ]),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/api/admin/wallets/wallet-user-123"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["id"], json!("wallet-user-123"));
    assert_eq!(payload["user_id"], json!("user-123"));
    assert_eq!(payload["owner_type"], json!("user"));
    assert_eq!(payload["owner_name"], json!("alice"));
    assert_eq!(payload["balance"], json!(15.0));
    assert_eq!(payload["recharge_balance"], json!(12.5));
    assert_eq!(payload["gift_balance"], json!(2.5));
    assert_eq!(payload["refundable_balance"], json!(12.5));
    assert_eq!(payload["unlimited"], json!(false));
    assert_eq!(payload["pending_refund_count"], serde_json::Value::Null);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_api_key_wallet_detail_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/wallets/wallet-key-123",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        None,
        StoredAuthApiKeySnapshot::new(
            "user-standalone".to_string(),
            "standalone-owner".to_string(),
            Some("owner@example.com".to_string()),
            "user".to_string(),
            "local".to_string(),
            true,
            false,
            None,
            None,
            None,
            "key-123".to_string(),
            Some("Standalone Key".to_string()),
            true,
            false,
            true,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("auth snapshot should build"),
    )]));
    let wallet_repository = Arc::new(InMemoryWalletRepository::seed(vec![
        sample_wallet_snapshot("wallet-key-123", None, Some("key-123"), "unlimited"),
    ]));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_auth_and_wallet_for_tests(
                    auth_repository,
                    wallet_repository,
                ),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/api/admin/wallets/wallet-key-123"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["id"], json!("wallet-key-123"));
    assert_eq!(payload["api_key_id"], json!("key-123"));
    assert_eq!(payload["owner_type"], json!("api_key"));
    assert_eq!(payload["owner_name"], json!("Standalone Key"));
    assert_eq!(payload["unlimited"], json!(true));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_wallets_transactions_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/wallets/wallet-user-123/transactions",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let user = StoredUserAuthRecord::new(
        "user-123".to_string(),
        Some("alice@example.com".to_string()),
        true,
        "alice".to_string(),
        Some("hash".to_string()),
        "user".to_string(),
        "local".to_string(),
        None,
        None,
        None,
        true,
        false,
        None,
        None,
    )
    .expect("user should build");
    let wallet_repository = Arc::new(InMemoryWalletRepository::seed(vec![
        sample_wallet_snapshot("wallet-user-123", Some("user-123"), None, "finite"),
    ]));
    let user_repository = Arc::new(InMemoryUserReadRepository::seed_auth_users(vec![user]));
    let operator = StoredUserAuthRecord::new(
        "admin-user-operator".to_string(),
        Some("admin@example.com".to_string()),
        true,
        "admin-operator".to_string(),
        Some("hash".to_string()),
        "admin".to_string(),
        "local".to_string(),
        None,
        None,
        None,
        true,
        false,
        None,
        None,
    )
    .expect("operator should build");

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_user_and_wallet_for_tests(
                    user_repository,
                    wallet_repository,
                ),
            )
            .with_auth_users_for_tests([operator])
            .with_admin_wallet_transactions_for_tests([
                {
                    let mut transaction = sample_transaction_record(
                        "tx-456",
                        "wallet-user-123",
                        Some("admin-user-operator"),
                    );
                    transaction.created_at_unix_ms = 1_710_000_300;
                    transaction.description = Some("Most recent credit".to_string());
                    transaction
                },
                sample_transaction_record("tx-123", "wallet-user-123", Some("admin-user-operator")),
            ]),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/wallets/wallet-user-123/transactions?limit=20&offset=1"
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
    assert_eq!(payload["wallet"]["id"], json!("wallet-user-123"));
    assert_eq!(payload["wallet"]["owner_name"], json!("alice"));
    assert_eq!(payload["items"].as_array().map(Vec::len), Some(1));
    assert_eq!(payload["items"][0]["id"], json!("tx-123"));
    assert_eq!(payload["items"][0]["wallet_id"], json!("wallet-user-123"));
    assert_eq!(payload["items"][0]["owner_type"], json!("user"));
    assert_eq!(payload["items"][0]["owner_name"], json!("alice"));
    assert_eq!(payload["items"][0]["category"], json!("recharge"));
    assert_eq!(payload["items"][0]["reason_code"], json!("manual_credit"));
    assert_eq!(
        payload["items"][0]["operator_id"],
        json!("admin-user-operator")
    );
    assert_eq!(
        payload["items"][0]["operator_name"],
        json!("admin-operator")
    );
    assert_eq!(
        payload["items"][0]["operator_email"],
        json!("admin@example.com")
    );
    assert_eq!(
        payload["items"][0]["description"],
        json!("Admin manual credit")
    );
    assert_eq!(payload["total"], json!(2));
    assert_eq!(payload["limit"], json!(20));
    assert_eq!(payload["offset"], json!(1));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_returns_bad_request_for_admin_wallet_transactions_with_empty_wallet_id() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/wallets//transactions",
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
        .get(format!("{gateway_url}/api/admin/wallets//transactions"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["detail"], "wallet_id 无效");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_wallets_refunds_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/wallets/wallet-user-123/refunds",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let user = StoredUserAuthRecord::new(
        "user-123".to_string(),
        Some("alice@example.com".to_string()),
        true,
        "alice".to_string(),
        Some("hash".to_string()),
        "user".to_string(),
        "local".to_string(),
        None,
        None,
        None,
        true,
        false,
        None,
        None,
    )
    .expect("user should build");
    let wallet_repository = Arc::new(InMemoryWalletRepository::seed(vec![
        sample_wallet_snapshot("wallet-user-123", Some("user-123"), None, "finite"),
    ]));
    let user_repository = Arc::new(InMemoryUserReadRepository::seed_auth_users(vec![user]));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_user_and_wallet_for_tests(
                    user_repository,
                    wallet_repository,
                ),
            )
            .with_admin_wallet_refunds_for_tests([
                {
                    let mut refund = sample_refund_record(
                        "refund-3",
                        "wallet-user-123",
                        Some("user-123"),
                        Some("po-3"),
                        3.0,
                        "pending_approval",
                        None,
                        None,
                        None,
                    );
                    refund.created_at_unix_ms = 1_710_000_300;
                    refund
                },
                {
                    let mut refund = sample_refund_record(
                        "refund-2",
                        "wallet-user-123",
                        Some("user-123"),
                        Some("po-2"),
                        2.0,
                        "processing",
                        Some("admin-user-123"),
                        Some("admin-user-123"),
                        Some(1_710_000_250),
                    );
                    refund.created_at_unix_ms = 1_710_000_200;
                    refund
                },
                sample_refund_record(
                    "refund-1",
                    "wallet-user-123",
                    Some("user-123"),
                    Some("po-1"),
                    1.0,
                    "succeeded",
                    Some("admin-user-123"),
                    Some("admin-user-123"),
                    Some(1_710_000_150),
                ),
            ]),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/wallets/wallet-user-123/refunds?limit=10&offset=2"
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
    assert_eq!(payload["wallet"]["id"], json!("wallet-user-123"));
    assert_eq!(payload["wallet"]["owner_name"], json!("alice"));
    assert_eq!(payload["items"].as_array().map(Vec::len), Some(1));
    assert_eq!(payload["items"][0]["id"], json!("refund-1"));
    assert_eq!(payload["items"][0]["wallet_id"], json!("wallet-user-123"));
    assert_eq!(payload["items"][0]["owner_type"], json!("user"));
    assert_eq!(payload["items"][0]["owner_name"], json!("alice"));
    assert_eq!(payload["items"][0]["status"], json!("succeeded"));
    assert_eq!(payload["items"][0]["payment_order_id"], json!("po-1"));
    assert_eq!(payload["total"], json!(3));
    assert_eq!(payload["limit"], json!(10));
    assert_eq!(payload["offset"], json!(2));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_rejects_admin_api_key_wallet_refunds_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/wallets/wallet-key-123/refunds",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        None,
        StoredAuthApiKeySnapshot::new(
            "user-standalone".to_string(),
            "standalone-owner".to_string(),
            Some("owner@example.com".to_string()),
            "user".to_string(),
            "local".to_string(),
            true,
            false,
            None,
            None,
            None,
            "key-123".to_string(),
            Some("Standalone Key".to_string()),
            true,
            false,
            true,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("auth snapshot should build"),
    )]));
    let wallet_repository = Arc::new(InMemoryWalletRepository::seed(vec![
        sample_wallet_snapshot("wallet-key-123", None, Some("key-123"), "unlimited"),
    ]));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_auth_and_wallet_for_tests(
                    auth_repository,
                    wallet_repository,
                ),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/wallets/wallet-key-123/refunds"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(
        payload["detail"],
        json!(ADMIN_WALLETS_API_KEY_REFUND_DETAIL)
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_wallets_adjust_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/wallets/wallet-adjust-123/adjust",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_auth_wallets_for_tests([sample_wallet_snapshot(
                "wallet-adjust-123",
                Some("user-1"),
                None,
                "finite",
            )]),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/wallets/wallet-adjust-123/adjust"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "amount_usd": 4.0,
            "balance_type": "recharge",
            "description": "管理员调账"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["wallet"]["id"], "wallet-adjust-123");
    assert_eq!(payload["wallet"]["balance"], json!(19.0));
    assert_eq!(payload["wallet"]["recharge_balance"], json!(16.5));
    assert_eq!(payload["wallet"]["total_adjusted"], json!(5.5));
    assert_eq!(payload["transaction"]["category"], "adjust");
    assert_eq!(payload["transaction"]["reason_code"], "adjust_admin");
    assert_eq!(payload["transaction"]["amount"], json!(4.0));
    assert_eq!(payload["transaction"]["balance_before"], json!(15.0));
    assert_eq!(payload["transaction"]["balance_after"], json!(19.0));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_wallets_recharge_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/wallets/wallet-recharge-123/recharge",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_auth_wallets_for_tests([sample_wallet_snapshot(
                "wallet-recharge-123",
                Some("user-1"),
                None,
                "finite",
            )]),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/wallets/wallet-recharge-123/recharge"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "amount_usd": 8.0,
            "payment_method": "admin_manual",
            "description": "管理员充值"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["wallet"]["id"], "wallet-recharge-123");
    assert_eq!(payload["wallet"]["balance"], json!(23.0));
    assert_eq!(payload["wallet"]["recharge_balance"], json!(20.5));
    assert_eq!(payload["wallet"]["total_recharged"], json!(38.0));
    assert_eq!(payload["payment_order"]["amount_usd"], json!(8.0));
    assert_eq!(payload["payment_order"]["payment_method"], "admin_manual");
    assert_eq!(payload["payment_order"]["status"], "credited");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_wallets_process_refund_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/wallets/wallet-123/refunds/refund-1/process",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_auth_wallets_for_tests([sample_wallet_snapshot(
                "wallet-123",
                Some("user-1"),
                None,
                "finite",
            )])
            .with_admin_wallet_payment_orders_for_tests([sample_payment_order_record(
                "po-1",
                "wallet-123",
                Some("user-1"),
                10.0,
                1.0,
                9.0,
            )])
            .with_admin_wallet_refunds_for_tests([sample_refund_record(
                "refund-1",
                "wallet-123",
                Some("user-1"),
                Some("po-1"),
                4.0,
                "pending_approval",
                None,
                None,
                None,
            )]),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/wallets/wallet-123/refunds/refund-1/process"
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
    assert_eq!(payload["wallet"]["id"], json!("wallet-123"));
    assert_eq!(payload["wallet"]["balance"], json!(11.0));
    assert_eq!(payload["wallet"]["recharge_balance"], json!(8.5));
    assert_eq!(payload["wallet"]["total_refunded"], json!(7.0));
    assert_eq!(payload["refund"]["status"], json!("processing"));
    assert_eq!(payload["refund"]["approved_by"], json!("admin-user-123"));
    assert_eq!(payload["refund"]["processed_by"], json!("admin-user-123"));
    assert_eq!(payload["transaction"]["category"], json!("refund"));
    assert_eq!(payload["transaction"]["reason_code"], json!("refund_out"));
    assert_eq!(payload["transaction"]["amount"], json!(-4.0));
    assert_eq!(payload["transaction"]["description"], json!("退款占款"));

    let ledger_response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/wallets/wallet-123/transactions?limit=20&offset=0"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("transaction list request should succeed");
    assert_eq!(ledger_response.status(), StatusCode::OK);
    let ledger_payload: serde_json::Value = ledger_response
        .json()
        .await
        .expect("transaction list response should be json");
    assert_eq!(ledger_payload["total"], json!(1));
    assert_eq!(
        ledger_payload["items"][0]["reason_code"],
        json!("refund_out")
    );
    assert_eq!(ledger_payload["items"][0]["amount"], json!(-4.0));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_returns_bad_request_for_admin_wallet_process_refund_with_empty_refund_id() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/wallets/wallet-123/refunds//process",
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
            "{gateway_url}/api/admin/wallets/wallet-123/refunds//process"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["detail"], "wallet_id 或 refund_id 无效");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_wallets_complete_refund_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/wallets/wallet-123/refunds/refund-1/complete",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_auth_wallets_for_tests([sample_wallet_snapshot(
                "wallet-123",
                Some("user-1"),
                None,
                "finite",
            )])
            .with_admin_wallet_refunds_for_tests([sample_refund_record(
                "refund-1",
                "wallet-123",
                Some("user-1"),
                Some("po-1"),
                4.0,
                "processing",
                Some("admin-user-123"),
                Some("admin-user-123"),
                Some(1_710_000_500),
            )]),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/wallets/wallet-123/refunds/refund-1/complete"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "gateway_refund_id": "gw-refund-123",
            "payout_reference": "bank-slip-1",
            "payout_proof": {
                "channel": "manual",
                "operator": "finance"
            }
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["refund"]["status"], json!("succeeded"));
    assert_eq!(
        payload["refund"]["gateway_refund_id"],
        json!("gw-refund-123")
    );
    assert_eq!(payload["refund"]["payout_reference"], json!("bank-slip-1"));
    assert_eq!(
        payload["refund"]["payout_proof"],
        json!({"channel": "manual", "operator": "finance"})
    );
    assert!(payload["refund"]["completed_at"].as_str().is_some());
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_rejects_admin_wallets_fail_processing_refund_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/wallets/wallet-123/refunds/refund-1/fail",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let mut wallet = sample_wallet_snapshot("wallet-123", Some("user-1"), None, "finite");
    wallet.balance = 8.5;
    wallet.total_refunded = 7.0;

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_auth_wallets_for_tests([wallet])
            .with_admin_wallet_payment_orders_for_tests([sample_payment_order_record(
                "po-1",
                "wallet-123",
                Some("user-1"),
                10.0,
                5.0,
                5.0,
            )])
            .with_admin_wallet_refunds_for_tests([sample_refund_record(
                "refund-1",
                "wallet-123",
                Some("user-1"),
                Some("po-1"),
                4.0,
                "processing",
                Some("admin-user-123"),
                Some("admin-user-123"),
                Some(1_710_000_500),
            )]),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/wallets/wallet-123/refunds/refund-1/fail"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "reason": "原路退款失败"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(
        payload["detail"],
        json!("cannot fail refund while gateway settlement is processing")
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_releases_offline_processing_refund_without_gateway_evidence() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/wallets/wallet-123/refunds/refund-1/fail",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let mut wallet = sample_wallet_snapshot("wallet-123", Some("user-1"), None, "finite");
    wallet.balance = 8.5;
    wallet.total_refunded = 7.0;
    let mut refund = sample_refund_record(
        "refund-1",
        "wallet-123",
        Some("user-1"),
        Some("po-1"),
        4.0,
        "processing",
        Some("admin-user-123"),
        Some("admin-user-123"),
        Some(1_710_000_500),
    );
    refund.refund_mode = "offline_payout".to_string();

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_auth_wallets_for_tests([wallet])
            .with_admin_wallet_payment_orders_for_tests([sample_payment_order_record(
                "po-1",
                "wallet-123",
                Some("user-1"),
                10.0,
                5.0,
                5.0,
            )])
            .with_admin_wallet_refunds_for_tests([refund]),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/wallets/wallet-123/refunds/refund-1/fail"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "reason": "线下退款未打款"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["wallet"]["balance"], json!(15.0));
    assert_eq!(payload["wallet"]["recharge_balance"], json!(12.5));
    assert_eq!(payload["wallet"]["total_refunded"], json!(3.0));
    assert_eq!(payload["refund"]["status"], json!("failed"));
    assert_eq!(
        payload["transaction"]["reason_code"],
        json!("refund_revert")
    );
    assert_eq!(payload["transaction"]["amount"], json!(4.0));

    let ledger_response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/wallets/wallet-123/transactions?limit=20&offset=0"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("transaction list request should succeed");
    assert_eq!(ledger_response.status(), StatusCode::OK);
    let ledger_payload: serde_json::Value = ledger_response
        .json()
        .await
        .expect("transaction list response should be json");
    assert_eq!(ledger_payload["total"], json!(1));
    assert_eq!(
        ledger_payload["items"][0]["reason_code"],
        json!("refund_revert")
    );
    assert_eq!(ledger_payload["items"][0]["amount"], json!(4.0));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_rejects_admin_api_key_wallet_process_refund_locally_with_trusted_admin_principal()
{
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/wallets/wallet-key-123/refunds/refund-1/process",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        None,
        StoredAuthApiKeySnapshot::new(
            "user-standalone".to_string(),
            "standalone-owner".to_string(),
            Some("owner@example.com".to_string()),
            "user".to_string(),
            "local".to_string(),
            true,
            false,
            None,
            None,
            None,
            "key-123".to_string(),
            Some("Standalone Key".to_string()),
            true,
            false,
            true,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("auth snapshot should build"),
    )]));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_auth_and_wallet_for_tests(
                    auth_repository,
                    Arc::new(InMemoryWalletRepository::seed(vec![
                        sample_wallet_snapshot(
                            "wallet-key-123",
                            None,
                            Some("key-123"),
                            "unlimited",
                        ),
                    ])),
                ),
            )
            .with_auth_wallets_for_tests([sample_wallet_snapshot(
                "wallet-key-123",
                None,
                Some("key-123"),
                "unlimited",
            )]),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/wallets/wallet-key-123/refunds/refund-1/process"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(
        payload["detail"],
        json!(ADMIN_WALLETS_API_KEY_REFUND_DETAIL)
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}
