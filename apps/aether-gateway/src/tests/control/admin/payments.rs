use std::sync::{Arc, Mutex};

use aether_data::repository::users::InMemoryUserReadRepository;
use aether_data::repository::wallet::StoredWalletSnapshot;
use aether_data::repository::wallet::{InMemoryWalletRepository, WalletWriteRepository};
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

fn admin_request(builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    builder
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
}

async fn start_payments_upstream(
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

fn sample_wallet(wallet_id: &str, user_id: &str) -> StoredWalletSnapshot {
    StoredWalletSnapshot::new(
        wallet_id.to_string(),
        Some(user_id.to_string()),
        None,
        12.5,
        2.5,
        "finite".to_string(),
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

#[allow(clippy::too_many_arguments)]
fn sample_payment_order(
    order_id: &str,
    wallet_id: &str,
    user_id: &str,
    amount_usd: f64,
    payment_method: &str,
    status: &str,
    created_at_unix_ms: u64,
    expires_at_unix_secs: Option<u64>,
) -> crate::AdminWalletPaymentOrderRecord {
    crate::AdminWalletPaymentOrderRecord {
        id: order_id.to_string(),
        order_no: format!("po-{order_id}"),
        wallet_id: wallet_id.to_string(),
        user_id: Some(user_id.to_string()),
        amount_usd,
        pay_amount: None,
        pay_currency: None,
        exchange_rate: None,
        refunded_amount_usd: 0.0,
        refundable_amount_usd: amount_usd,
        payment_method: payment_method.to_string(),
        gateway_order_id: None,
        status: status.to_string(),
        gateway_response: None,
        created_at_unix_ms,
        paid_at_unix_secs: None,
        credited_at_unix_secs: None,
        expires_at_unix_secs,
    }
}

fn sample_payment_callback(
    callback_id: &str,
    payment_order_id: Option<&str>,
    payment_method: &str,
    callback_key: &str,
    status: &str,
    created_at_unix_ms: u64,
) -> crate::state::AdminPaymentCallbackRecord {
    crate::state::AdminPaymentCallbackRecord {
        id: callback_id.to_string(),
        payment_order_id: payment_order_id.map(str::to_string),
        payment_method: payment_method.to_string(),
        callback_key: callback_key.to_string(),
        order_no: payment_order_id.map(|value| format!("po-{value}")),
        gateway_order_id: Some(format!("gw-{callback_id}")),
        payload_hash: Some(format!("hash-{callback_id}")),
        signature_valid: status != "failed",
        status: status.to_string(),
        payload: Some(json!({ "source": callback_id })),
        error_message: (status == "failed").then(|| "signature mismatch".to_string()),
        created_at_unix_ms,
        processed_at_unix_secs: Some(created_at_unix_ms.saturating_add(60)),
    }
}

#[tokio::test]
async fn gateway_handles_admin_payments_list_orders_locally_with_trusted_admin_principal() {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_payments_upstream("/api/admin/payments/orders").await;

    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_admin_wallet_payment_orders_for_tests([
                sample_payment_order(
                    "order-1",
                    "wallet-1",
                    "user-1",
                    12.5,
                    "alipay",
                    "pending",
                    1_710_000_000,
                    Some(4_102_444_800),
                ),
                sample_payment_order(
                    "order-2",
                    "wallet-2",
                    "user-2",
                    8.0,
                    "manual",
                    "failed",
                    1_709_000_000,
                    None,
                ),
            ]),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(reqwest::Client::new().get(format!(
        "{gateway_url}/api/admin/payments/orders?status=pending&payment_method=alipay&limit=20&offset=0"
    )))
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    let items = payload["items"].as_array().expect("items should be array");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], "order-1");
    assert_eq!(items[0]["wallet_id"], "wallet-1");
    assert_eq!(items[0]["payment_method"], "alipay");
    assert_eq!(items[0]["status"], "pending");
    assert_eq!(payload["total"], 1);
    assert_eq!(payload["limit"], 20);
    assert_eq!(payload["offset"], 0);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_payments_get_order_locally_with_trusted_admin_principal() {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_payments_upstream("/api/admin/payments/orders/order-1").await;

    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_admin_wallet_payment_orders_for_tests([sample_payment_order(
                "order-1",
                "wallet-1",
                "user-1",
                12.5,
                "wechat",
                "pending",
                1_710_000_000,
                Some(4_102_444_800),
            )]),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(
        reqwest::Client::new().get(format!("{gateway_url}/api/admin/payments/orders/order-1")),
    )
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["order"]["id"], "order-1");
    assert_eq!(payload["order"]["payment_method"], "wechat");
    assert_eq!(payload["order"]["amount_usd"], 12.5);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_payments_expire_order_locally_with_trusted_admin_principal() {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_payments_upstream("/api/admin/payments/orders/order-1/expire").await;

    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_admin_wallet_payment_orders_for_tests([sample_payment_order(
                "order-1",
                "wallet-1",
                "user-1",
                12.5,
                "wechat",
                "pending",
                1_710_000_000,
                Some(4_102_444_800),
            )]),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(reqwest::Client::new().post(format!(
        "{gateway_url}/api/admin/payments/orders/order-1/expire"
    )))
    .json(&json!({}))
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["expired"], true);
    assert_eq!(payload["order"]["status"], "expired");
    assert_eq!(
        payload["order"]["gateway_response"]["expire_reason"],
        "admin_mark_expired"
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_payments_credit_order_locally_with_trusted_admin_principal() {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_payments_upstream("/api/admin/payments/orders/order-1/credit").await;

    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_auth_wallets_for_tests([sample_wallet("wallet-1", "user-1")])
            .with_admin_wallet_payment_orders_for_tests([sample_payment_order(
                "order-1",
                "wallet-1",
                "user-1",
                12.5,
                "wechat",
                "pending",
                1_710_000_000,
                Some(4_102_444_800),
            )]),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(reqwest::Client::new().post(format!(
        "{gateway_url}/api/admin/payments/orders/order-1/credit"
    )))
    .json(&json!({
        "gateway_order_id": "gateway-order-1",
        "pay_amount": 91.25,
        "pay_currency": "cny",
        "exchange_rate": 7.3,
        "gateway_response": {
            "channel": "manual-review"
        }
    }))
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["credited"], true);
    assert_eq!(payload["order"]["status"], "credited");
    assert_eq!(payload["order"]["gateway_order_id"], "gateway-order-1");
    assert_eq!(payload["order"]["pay_amount"], 91.25);
    assert_eq!(payload["order"]["pay_currency"], "CNY");
    assert_eq!(payload["order"]["exchange_rate"], 7.3);
    assert_eq!(
        payload["order"]["gateway_response"]["channel"],
        "manual-review"
    );
    assert_eq!(payload["order"]["gateway_response"]["manual_credit"], true);
    assert_eq!(
        payload["order"]["gateway_response"]["credited_by"],
        "admin-user-123"
    );
    assert!(payload["order"]["paid_at"].as_str().is_some());
    assert!(payload["order"]["credited_at"].as_str().is_some());
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_payments_fail_order_locally_with_trusted_admin_principal() {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_payments_upstream("/api/admin/payments/orders/order-1/fail").await;

    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_admin_wallet_payment_orders_for_tests([sample_payment_order(
                "order-1",
                "wallet-1",
                "user-1",
                12.5,
                "wechat",
                "pending",
                1_710_000_000,
                Some(4_102_444_800),
            )]),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(reqwest::Client::new().post(format!(
        "{gateway_url}/api/admin/payments/orders/order-1/fail"
    )))
    .json(&json!({}))
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["order"]["status"], "failed");
    assert_eq!(
        payload["order"]["gateway_response"]["failure_reason"],
        "admin_mark_failed"
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_payments_callbacks_locally_with_trusted_admin_principal() {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_payments_upstream("/api/admin/payments/callbacks").await;

    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_admin_payment_callbacks_for_tests([
                sample_payment_callback(
                    "callback-1",
                    Some("order-1"),
                    "alipay",
                    "callback-key-1",
                    "processed",
                    1_710_100_000,
                ),
                sample_payment_callback(
                    "callback-2",
                    Some("order-2"),
                    "wechat",
                    "callback-key-2",
                    "failed",
                    1_710_000_000,
                ),
            ]),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(reqwest::Client::new().get(format!(
        "{gateway_url}/api/admin/payments/callbacks?payment_method=alipay&limit=10&offset=0"
    )))
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    let items = payload["items"].as_array().expect("items should be array");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], "callback-1");
    assert_eq!(items[0]["payment_order_id"], "order-1");
    assert_eq!(items[0]["payment_method"], "alipay");
    assert_eq!(items[0]["callback_key"], "callback-key-1");
    assert_eq!(items[0]["signature_valid"], true);
    assert_eq!(items[0]["status"], "processed");
    assert_eq!(items[0]["payload"]["source"], "callback-1");
    assert!(items[0]["processed_at"].as_str().is_some());
    assert_eq!(payload["total"], 1);
    assert_eq!(payload["limit"], 10);
    assert_eq!(payload["offset"], 0);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_payments_trailing_slash_routes_locally_with_trusted_admin_principal()
{
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new()
        .route(
            "/api/admin/payments/orders/order-1",
            any({
                let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
                move |_request: Request| {
                    let upstream_hits_inner = Arc::clone(&upstream_hits_inner);
                    async move {
                        *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                        (StatusCode::OK, Body::from("unexpected upstream hit"))
                    }
                }
            }),
        )
        .route(
            "/api/admin/payments/orders/order-1/expire",
            any({
                let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
                move |_request: Request| {
                    let upstream_hits_inner = Arc::clone(&upstream_hits_inner);
                    async move {
                        *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                        (StatusCode::OK, Body::from("unexpected upstream hit"))
                    }
                }
            }),
        )
        .route(
            "/api/admin/payments/orders/order-1/credit",
            any({
                let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
                move |_request: Request| {
                    let upstream_hits_inner = Arc::clone(&upstream_hits_inner);
                    async move {
                        *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                        (StatusCode::OK, Body::from("unexpected upstream hit"))
                    }
                }
            }),
        )
        .route(
            "/api/admin/payments/orders/order-1/fail",
            any({
                let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
                move |_request: Request| {
                    let upstream_hits_inner = Arc::clone(&upstream_hits_inner);
                    async move {
                        *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                        (StatusCode::OK, Body::from("unexpected upstream hit"))
                    }
                }
            }),
        );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_auth_wallets_for_tests([sample_wallet("wallet-1", "user-1")])
            .with_admin_wallet_payment_orders_for_tests([sample_payment_order(
                "order-1",
                "wallet-1",
                "user-1",
                12.5,
                "wechat",
                "pending",
                1_710_000_000,
                Some(4_102_444_800),
            )]),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let detail = admin_request(
        reqwest::Client::new().get(format!("{gateway_url}/api/admin/payments/orders/order-1/")),
    )
    .send()
    .await
    .expect("detail request should succeed");
    assert_eq!(detail.status(), StatusCode::OK);
    let detail_payload: serde_json::Value = detail.json().await.expect("json body should parse");
    assert_eq!(detail_payload["order"]["id"], "order-1");

    let expire = admin_request(reqwest::Client::new().post(format!(
        "{gateway_url}/api/admin/payments/orders/order-1/expire/"
    )))
    .json(&json!({}))
    .send()
    .await
    .expect("expire request should succeed");
    assert_eq!(expire.status(), StatusCode::OK);
    let expire_payload: serde_json::Value = expire.json().await.expect("json body should parse");
    assert_eq!(expire_payload["order"]["status"], "expired");

    let mut state = AppState::new().expect("gateway should build");
    state = state
        .with_auth_wallets_for_tests([sample_wallet("wallet-1", "user-1")])
        .with_admin_wallet_payment_orders_for_tests([sample_payment_order(
            "order-1",
            "wallet-1",
            "user-1",
            12.5,
            "wechat",
            "pending",
            1_710_000_000,
            Some(4_102_444_800),
        )]);
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle_credit) = start_server(gateway).await;

    let credit = admin_request(reqwest::Client::new().post(format!(
        "{gateway_url}/api/admin/payments/orders/order-1/credit/"
    )))
    .json(&json!({
        "gateway_order_id": "gateway-order-1",
        "pay_amount": 91.25,
        "pay_currency": "cny",
        "exchange_rate": 7.3
    }))
    .send()
    .await
    .expect("credit request should succeed");
    assert_eq!(credit.status(), StatusCode::OK);
    let credit_payload: serde_json::Value = credit.json().await.expect("json body should parse");
    assert_eq!(credit_payload["order"]["status"], "credited");

    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_admin_wallet_payment_orders_for_tests([sample_payment_order(
                "order-1",
                "wallet-1",
                "user-1",
                12.5,
                "wechat",
                "pending",
                1_710_000_000,
                Some(4_102_444_800),
            )]),
    );
    let (gateway_url, gateway_handle_fail) = start_server(gateway).await;

    let fail = admin_request(reqwest::Client::new().post(format!(
        "{gateway_url}/api/admin/payments/orders/order-1/fail/"
    )))
    .json(&json!({}))
    .send()
    .await
    .expect("fail request should succeed");
    assert_eq!(fail.status(), StatusCode::OK);
    let fail_payload: serde_json::Value = fail.json().await.expect("json body should parse");
    assert_eq!(fail_payload["order"]["status"], "failed");

    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    gateway_handle_credit.abort();
    gateway_handle_fail.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_payments_list_orders_locally_without_payment_backend() {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_payments_upstream("/api/admin/payments/orders").await;

    let mut state = AppState::new().expect("gateway should build");
    state.admin_wallet_payment_order_store = None;
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(reqwest::Client::new().get(format!(
        "{gateway_url}/api/admin/payments/orders?limit=10&offset=0"
    )))
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["items"], json!([]));
    assert_eq!(payload["total"], 0);
    assert_eq!(payload["limit"], 10);
    assert_eq!(payload["offset"], 0);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_payments_get_order_locally_without_payment_backend() {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_payments_upstream("/api/admin/payments/orders/order-missing").await;

    let mut state = AppState::new().expect("gateway should build");
    state.admin_wallet_payment_order_store = None;
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(reqwest::Client::new().get(format!(
        "{gateway_url}/api/admin/payments/orders/order-missing"
    )))
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["detail"], "Payment order read backend unavailable");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_payments_expire_order_with_backend_unavailable_detail() {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_payments_upstream("/api/admin/payments/orders/order-missing/expire").await;

    let mut state = AppState::new().expect("gateway should build");
    state.admin_wallet_payment_order_store = None;
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(reqwest::Client::new().post(format!(
        "{gateway_url}/api/admin/payments/orders/order-missing/expire"
    )))
    .json(&json!({}))
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["detail"], "Payment order write backend unavailable");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_payments_callbacks_locally_without_payment_backend() {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_payments_upstream("/api/admin/payments/callbacks").await;

    let mut state = AppState::new().expect("gateway should build");
    state.admin_payment_callback_store = None;
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(reqwest::Client::new().get(format!(
        "{gateway_url}/api/admin/payments/callbacks?payment_method=alipay&limit=10&offset=5"
    )))
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["items"], json!([]));
    assert_eq!(payload["total"], 0);
    assert_eq!(payload["limit"], 10);
    assert_eq!(payload["offset"], 5);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_rejects_admin_payments_empty_order_identifier_locally_with_trusted_admin_principal(
) {
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

    let expire = admin_request(
        reqwest::Client::new().post(format!("{gateway_url}/api/admin/payments/orders//expire")),
    )
    .json(&json!({}))
    .send()
    .await
    .expect("expire request should succeed");
    assert_eq!(expire.status(), StatusCode::NOT_FOUND);
    let expire_payload: serde_json::Value = expire.json().await.expect("json body should parse");
    assert_eq!(expire_payload["detail"], "Payment order not found");

    let credit = admin_request(
        reqwest::Client::new().post(format!("{gateway_url}/api/admin/payments/orders//credit")),
    )
    .json(&json!({}))
    .send()
    .await
    .expect("credit request should succeed");
    assert_eq!(credit.status(), StatusCode::NOT_FOUND);
    let credit_payload: serde_json::Value = credit.json().await.expect("json body should parse");
    assert_eq!(credit_payload["detail"], "Payment order not found");

    let fail = admin_request(
        reqwest::Client::new().post(format!("{gateway_url}/api/admin/payments/orders//fail")),
    )
    .json(&json!({}))
    .send()
    .await
    .expect("fail request should succeed");
    assert_eq!(fail.status(), StatusCode::NOT_FOUND);
    let fail_payload: serde_json::Value = fail.json().await.expect("json body should parse");
    assert_eq!(fail_payload["detail"], "Payment order not found");

    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_redeem_code_batch_lifecycle_locally() {
    let user_repository = Arc::new(InMemoryUserReadRepository::seed_auth_users(vec![]));
    let wallet_repository = Arc::new(InMemoryWalletRepository::seed(
        Vec::<StoredWalletSnapshot>::new(),
    ));
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_user_and_wallet_for_tests(
                    user_repository,
                    wallet_repository.clone(),
                ),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();

    let create_response = admin_request(client.post(format!(
        "{gateway_url}/api/admin/payments/redeem-codes/batches"
    )))
    .json(&json!({
        "name": "五月渠道卡",
        "amount_usd": 9.9,
        "total_count": 2,
        "description": "offline promo",
    }))
    .send()
    .await
    .expect("request should succeed");
    assert_eq!(create_response.status(), StatusCode::OK);
    let create_payload: serde_json::Value = create_response
        .json()
        .await
        .expect("json body should parse");
    let batch_id = create_payload["batch"]["id"]
        .as_str()
        .expect("batch id should exist")
        .to_string();
    assert_eq!(create_payload["batch"]["name"], "五月渠道卡");
    assert_eq!(create_payload["batch"]["balance_bucket"], "gift");
    assert_eq!(create_payload["codes"].as_array().map(Vec::len), Some(2));

    let list_response = admin_request(client.get(format!(
        "{gateway_url}/api/admin/payments/redeem-codes/batches?limit=20&offset=0"
    )))
    .send()
    .await
    .expect("request should succeed");
    assert_eq!(list_response.status(), StatusCode::OK);
    let list_payload: serde_json::Value =
        list_response.json().await.expect("json body should parse");
    assert_eq!(list_payload["items"][0]["id"], batch_id);
    assert_eq!(list_payload["items"][0]["balance_bucket"], "gift");

    let codes_response = admin_request(client.get(format!(
        "{gateway_url}/api/admin/payments/redeem-codes/batches/{batch_id}/codes?limit=20&offset=0"
    )))
    .send()
    .await
    .expect("request should succeed");
    assert_eq!(codes_response.status(), StatusCode::OK);
    let codes_payload: serde_json::Value =
        codes_response.json().await.expect("json body should parse");
    assert_eq!(codes_payload["batch"]["id"], batch_id);
    assert_eq!(codes_payload["batch"]["balance_bucket"], "gift");
    assert_eq!(codes_payload["items"].as_array().map(Vec::len), Some(2));

    let disable_response = admin_request(client.post(format!(
        "{gateway_url}/api/admin/payments/redeem-codes/batches/{batch_id}/disable"
    )))
    .json(&json!({}))
    .send()
    .await
    .expect("request should succeed");
    assert_eq!(disable_response.status(), StatusCode::OK);
    let disable_payload: serde_json::Value = disable_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(disable_payload["batch"]["status"], "disabled");

    let delete_response = admin_request(client.post(format!(
        "{gateway_url}/api/admin/payments/redeem-codes/batches/{batch_id}/delete"
    )))
    .json(&json!({}))
    .send()
    .await
    .expect("request should succeed");
    assert_eq!(delete_response.status(), StatusCode::OK);
    let delete_payload: serde_json::Value = delete_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(delete_payload["batch"]["id"], batch_id);

    gateway_handle.abort();
}
