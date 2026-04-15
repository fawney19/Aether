use axum::{body::Body, http, response::Response};
use hmac::{Hmac, Mac};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};

use super::super::{build_auth_json_response, wallet_normalize_optional_string_field};

pub(super) const PAYMENT_CALLBACK_TOKEN_HEADER: &str = "x-payment-callback-token";
pub(super) const PAYMENT_CALLBACK_SIGNATURE_HEADER: &str = "x-payment-callback-signature";

#[derive(Debug, Deserialize)]
pub(crate) struct PaymentCallbackRequest {
    pub(crate) callback_key: String,
    #[serde(default)]
    pub(crate) order_no: Option<String>,
    #[serde(default)]
    pub(crate) gateway_order_id: Option<String>,
    pub(crate) amount_usd: f64,
    #[serde(default)]
    pub(crate) pay_amount: Option<f64>,
    #[serde(default)]
    pub(crate) pay_currency: Option<String>,
    #[serde(default)]
    pub(crate) exchange_rate: Option<f64>,
    #[serde(default)]
    pub(crate) payload: Option<serde_json::Map<String, serde_json::Value>>,
}

#[derive(Debug, Clone)]
pub(crate) struct NormalizedPaymentCallbackRequest {
    pub(crate) callback_key: String,
    pub(crate) order_no: Option<String>,
    pub(crate) gateway_order_id: Option<String>,
    pub(crate) amount_usd: f64,
    pub(crate) pay_amount: Option<f64>,
    pub(crate) pay_currency: Option<String>,
    pub(crate) exchange_rate: Option<f64>,
    pub(crate) payload: serde_json::Value,
}

pub(super) fn payment_callback_secret() -> Option<String> {
    std::env::var("PAYMENT_CALLBACK_SECRET")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(super) fn payment_callback_payment_method_from_path(path: &str) -> Option<String> {
    let normalized = path.trim_end_matches('/');
    let prefix = "/api/payment/callback/";
    normalized
        .strip_prefix(prefix)
        .map(str::trim)
        .filter(|value| !value.is_empty() && !value.contains('/'))
        .map(ToOwned::to_owned)
}

fn normalize_payment_callback_optional_string(
    value: Option<String>,
    max_chars: usize,
) -> Result<Option<String>, &'static str> {
    wallet_normalize_optional_string_field(value, max_chars)
}

pub(super) fn normalize_payment_callback_request(
    payload: PaymentCallbackRequest,
) -> Result<NormalizedPaymentCallbackRequest, &'static str> {
    let callback_key = payload.callback_key.trim();
    if callback_key.is_empty() || callback_key.chars().count() > 128 {
        return Err("输入验证失败");
    }
    if !payload.amount_usd.is_finite() || payload.amount_usd <= 0.0 {
        return Err("输入验证失败");
    }
    if matches!(payload.pay_amount, Some(value) if !value.is_finite() || value <= 0.0) {
        return Err("输入验证失败");
    }
    if matches!(payload.exchange_rate, Some(value) if !value.is_finite() || value <= 0.0) {
        return Err("输入验证失败");
    }
    let order_no = normalize_payment_callback_optional_string(payload.order_no, 64)?;
    let gateway_order_id =
        normalize_payment_callback_optional_string(payload.gateway_order_id, 128)?;
    let pay_currency = normalize_payment_callback_optional_string(payload.pay_currency, 3)?;
    if matches!(pay_currency.as_deref(), Some(value) if value.chars().count() != 3) {
        return Err("输入验证失败");
    }

    let payload_value = payload
        .payload
        .map(serde_json::Value::Object)
        .unwrap_or_else(|| {
            json!({
                "callback_key": callback_key,
                "order_no": order_no,
                "gateway_order_id": gateway_order_id,
                "amount_usd": payload.amount_usd,
                "pay_amount": payload.pay_amount,
                "pay_currency": pay_currency,
                "exchange_rate": payload.exchange_rate,
                "payload": serde_json::Value::Null,
            })
        });

    Ok(NormalizedPaymentCallbackRequest {
        callback_key: callback_key.to_string(),
        order_no,
        gateway_order_id,
        amount_usd: payload.amount_usd,
        pay_amount: payload.pay_amount,
        pay_currency,
        exchange_rate: payload.exchange_rate,
        payload: payment_callback_canonicalize_json(&payload_value),
    })
}

fn payment_callback_canonicalize_json(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut items = map.iter().collect::<Vec<_>>();
            items.sort_by(|left, right| left.0.cmp(right.0));
            let mut object = serde_json::Map::new();
            for (key, value) in items {
                object.insert(key.clone(), payment_callback_canonicalize_json(value));
            }
            serde_json::Value::Object(object)
        }
        serde_json::Value::Array(items) => serde_json::Value::Array(
            items
                .iter()
                .map(payment_callback_canonicalize_json)
                .collect(),
        ),
        _ => value.clone(),
    }
}

fn payment_callback_signature_hex(
    payload: &serde_json::Value,
    secret: &str,
) -> Result<String, String> {
    let canonical = serde_json::to_string(payload)
        .map_err(|err| format!("payment callback canonicalization failed: {err}"))?;
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
        .map_err(|err| format!("payment callback hmac init failed: {err}"))?;
    mac.update(canonical.as_bytes());
    let bytes = mac.finalize().into_bytes();
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub(super) fn payment_callback_signature_matches(
    payload: &serde_json::Value,
    provided_signature: &str,
    secret: &str,
) -> Result<bool, String> {
    let expected = payment_callback_signature_hex(payload, secret)?;
    let provided = provided_signature
        .trim()
        .strip_prefix("sha256=")
        .unwrap_or(provided_signature.trim())
        .to_ascii_lowercase();
    Ok(provided == expected)
}

pub(super) fn payment_callback_payload_hash(payload: &serde_json::Value) -> Result<String, String> {
    let encoded = serde_json::to_vec(payload)
        .map_err(|err| format!("payment callback payload encode failed: {err}"))?;
    let digest = Sha256::digest(&encoded);
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub(super) fn payment_callback_mark_failed_response(
    duplicate: bool,
    error: &str,
    payment_method: &str,
    request_path: &str,
) -> Response<Body> {
    build_auth_json_response(
        http::StatusCode::OK,
        json!({
            "ok": false,
            "duplicate": duplicate,
            "error": error,
            "payment_method": payment_method,
            "request_path": request_path,
        }),
        None,
    )
}
