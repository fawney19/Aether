use super::super::support_wallet::wallet_test_recharge_store;
use axum::{body::Body, response::Response};
use chrono::Utc;
use serde_json::json;

use super::super::sanitize_wallet_gateway_response;
use super::payment_shared::{
    payment_callback_mark_failed_response, payment_callback_success_response,
    NormalizedPaymentCallbackRequest,
};
use super::GatewayPublicRequestContext;

#[derive(Debug, Clone)]
struct PaymentTestCallbackRecord {
    callback_key: String,
    status: String,
}

fn payment_test_callback_store() -> &'static std::sync::Mutex<Vec<PaymentTestCallbackRecord>> {
    static STORE: std::sync::OnceLock<std::sync::Mutex<Vec<PaymentTestCallbackRecord>>> =
        std::sync::OnceLock::new();
    STORE.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

pub(super) async fn handle_payment_callback_with_test_store(
    payment_method: &str,
    _request_context: &GatewayPublicRequestContext,
    payload: &NormalizedPaymentCallbackRequest,
    signature_valid: bool,
) -> Response<Body> {
    let mut callback_store = payment_test_callback_store()
        .lock()
        .expect("payment test callback store should lock");
    if callback_store
        .iter()
        .any(|entry| entry.callback_key == payload.callback_key && entry.status == "processed")
    {
        return payment_callback_success_response();
    }
    if !signature_valid {
        callback_store.push(PaymentTestCallbackRecord {
            callback_key: payload.callback_key.clone(),
            status: "failed".to_string(),
        });
        return payment_callback_mark_failed_response();
    }

    let mut recharge_store = wallet_test_recharge_store()
        .lock()
        .expect("wallet test recharge store should lock");
    let order = recharge_store.iter_mut().find(|entry| {
        entry.payload["order_no"].as_str() == payload.order_no.as_deref()
            || entry.payload["gateway_order_id"].as_str() == payload.gateway_order_id.as_deref()
    });
    let Some(order) = order else {
        callback_store.push(PaymentTestCallbackRecord {
            callback_key: payload.callback_key.clone(),
            status: "failed".to_string(),
        });
        return payment_callback_mark_failed_response();
    };
    let order_payment_method = order.payload["payment_method"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    if !order_payment_method.eq_ignore_ascii_case(payment_method) {
        callback_store.push(PaymentTestCallbackRecord {
            callback_key: payload.callback_key.clone(),
            status: "failed".to_string(),
        });
        return payment_callback_mark_failed_response();
    }

    let order_amount = order.payload["amount_usd"].as_f64().unwrap_or_default();
    if (payload.amount_usd - order_amount).abs() > f64::EPSILON {
        callback_store.push(PaymentTestCallbackRecord {
            callback_key: payload.callback_key.clone(),
            status: "failed".to_string(),
        });
        return payment_callback_mark_failed_response();
    }

    let current_status = order.payload["status"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    if current_status == "credited" {
        callback_store.push(PaymentTestCallbackRecord {
            callback_key: payload.callback_key.clone(),
            status: "processed".to_string(),
        });
        return payment_callback_success_response();
    }

    let now = Utc::now().to_rfc3339();
    order.payload["status"] = json!("credited");
    order.payload["gateway_response"] =
        sanitize_wallet_gateway_response(Some(payload.payload.clone()));
    order.payload["pay_amount"] = match payload.pay_amount {
        Some(value) => json!(value),
        None => order.payload["pay_amount"].clone(),
    };
    order.payload["pay_currency"] = match payload.pay_currency.as_deref() {
        Some(value) => json!(value),
        None => order.payload["pay_currency"].clone(),
    };
    order.payload["exchange_rate"] = match payload.exchange_rate {
        Some(value) => json!(value),
        None => order.payload["exchange_rate"].clone(),
    };
    if let Some(gateway_order_id) = payload.gateway_order_id.as_deref() {
        order.payload["gateway_order_id"] = json!(gateway_order_id);
    }
    order.payload["refundable_amount_usd"] = json!(order_amount);
    order.payload["paid_at"] = json!(now.clone());
    order.payload["credited_at"] = json!(now);
    callback_store.push(PaymentTestCallbackRecord {
        callback_key: payload.callback_key.clone(),
        status: "processed".to_string(),
    });
    payment_callback_success_response()
}
