use axum::{body::Body, http, response::Response};
use hmac::{Hmac, Mac};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};

use super::super::{build_auth_json_response, wallet_normalize_optional_string_field};
use crate::handlers::shared::normalize_payment_currency;

pub(super) const PAYMENT_CALLBACK_TOKEN_HEADER: &str = "x-payment-callback-token";
pub(super) const PAYMENT_CALLBACK_SIGNATURE_HEADER: &str = "x-payment-callback-signature";
const PAYMENT_CALLBACK_SECRET_MIN_BYTES: usize = 32;
const PAYMENT_CALLBACK_SECRET_MIN_UNIQUE_BYTES: usize = 8;
const PAYMENT_CALLBACK_KEY_MAX_CHARS: usize = 128;

#[derive(Deserialize)]
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

impl std::fmt::Debug for PaymentCallbackRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PaymentCallbackRequest")
            .field("callback_key", &"[REDACTED]")
            .field("order_no", &self.order_no)
            .field("gateway_order_id", &self.gateway_order_id)
            .field("amount_usd", &self.amount_usd)
            .field("pay_amount", &self.pay_amount)
            .field("pay_currency", &self.pay_currency)
            .field("exchange_rate", &self.exchange_rate)
            .field("payload", &self.payload.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

#[derive(Clone)]
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

impl std::fmt::Debug for NormalizedPaymentCallbackRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NormalizedPaymentCallbackRequest")
            .field("callback_key", &"[REDACTED]")
            .field("order_no", &self.order_no)
            .field("gateway_order_id", &self.gateway_order_id)
            .field("amount_usd", &self.amount_usd)
            .field("pay_amount", &self.pay_amount)
            .field("pay_currency", &self.pay_currency)
            .field("exchange_rate", &self.exchange_rate)
            .field("payload", &"[REDACTED]")
            .finish()
    }
}

pub(super) fn payment_callback_secret() -> Option<String> {
    std::env::var("PAYMENT_CALLBACK_SECRET")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| payment_callback_secret_is_strong(value))
}

fn payment_callback_secret_is_strong(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() < PAYMENT_CALLBACK_SECRET_MIN_BYTES
        || bytes.iter().any(|byte| byte.is_ascii_control())
    {
        return false;
    }
    let mut seen = [false; 256];
    let mut unique = 0usize;
    for byte in bytes {
        let index = usize::from(*byte);
        if !seen[index] {
            seen[index] = true;
            unique += 1;
        }
    }
    unique >= PAYMENT_CALLBACK_SECRET_MIN_UNIQUE_BYTES
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

pub(super) fn generic_payment_callback_method_allowed(payment_method: &str) -> bool {
    !matches!(
        payment_method.trim().to_ascii_lowercase().as_str(),
        "alipay" | "wxpay" | "stripe" | "epay"
    )
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
    let pay_currency = normalize_payment_callback_optional_string(payload.pay_currency, 3)?
        .map(|value| normalize_payment_currency(&value, "pay_currency"))
        .transpose()
        .map_err(|_| "输入验证失败")?;

    // The signature covers the complete settlement envelope. Signing only the
    // provider-specific payload would leave the order and amount fields mutable.
    let payload_value = json!({
        "callback_key": callback_key,
        "order_no": order_no,
        "gateway_order_id": gateway_order_id,
        "amount_usd": payload.amount_usd,
        "pay_amount": payload.pay_amount,
        "pay_currency": pay_currency,
        "exchange_rate": payload.exchange_rate,
        "payload": payload.payload.map(serde_json::Value::Object),
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

fn decode_payment_callback_signature(value: &str) -> Option<Vec<u8>> {
    let value = value.trim().strip_prefix("sha256=").unwrap_or(value.trim());
    if value.len() != 64 || !value.is_ascii() {
        return None;
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|chunk| {
            std::str::from_utf8(chunk)
                .ok()
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        })
        .collect()
}

pub(super) fn payment_callback_secret_matches(provided: &str, expected: &str) -> bool {
    let mut expected_mac = Hmac::<Sha256>::new_from_slice(b"aether-payment-callback-token")
        .expect("static payment callback comparison key should be valid");
    expected_mac.update(expected.as_bytes());
    let expected_tag = expected_mac.finalize().into_bytes();

    let mut provided_mac = Hmac::<Sha256>::new_from_slice(b"aether-payment-callback-token")
        .expect("static payment callback comparison key should be valid");
    provided_mac.update(provided.trim().as_bytes());
    provided_mac.verify_slice(&expected_tag).is_ok()
}

pub(super) fn payment_callback_signature_matches(
    payload: &serde_json::Value,
    provided_signature: &str,
    secret: &str,
) -> Result<bool, String> {
    let Some(provided) = decode_payment_callback_signature(provided_signature) else {
        return Ok(false);
    };
    let canonical = serde_json::to_string(payload)
        .map_err(|_| "payment callback canonicalization failed".to_string())?;
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
        .map_err(|_| "payment callback hmac init failed".to_string())?;
    mac.update(canonical.as_bytes());
    Ok(mac.verify_slice(&provided).is_ok())
}

pub(super) fn payment_callback_payload_hash(payload: &serde_json::Value) -> Result<String, String> {
    let encoded = serde_json::to_vec(payload)
        .map_err(|_| "payment callback payload encode failed".to_string())?;
    let digest = Sha256::digest(&encoded);
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub(super) fn payment_callback_namespaced_key(payment_method: &str, callback_key: &str) -> String {
    let payment_method = payment_method.trim().to_ascii_lowercase();
    let prefix = format!("{payment_method}:");
    if prefix.chars().count() + callback_key.chars().count() <= PAYMENT_CALLBACK_KEY_MAX_CHARS {
        return format!("{prefix}{callback_key}");
    }

    let digest = Sha256::digest(callback_key.as_bytes());
    let digest = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("{prefix}{digest}")
}

pub(super) fn payment_callback_persistence_projection(
    payment_method: &str,
    payload: &NormalizedPaymentCallbackRequest,
    signature_valid: bool,
) -> serde_json::Value {
    json!({
        "gateway": payment_method,
        "order_no": payload.order_no,
        "gateway_order_id": payload.gateway_order_id,
        "amount_usd": payload.amount_usd,
        "pay_amount": payload.pay_amount,
        "pay_currency": payload.pay_currency,
        "exchange_rate": payload.exchange_rate,
        "signature_valid": signature_valid,
    })
}

pub(super) fn payment_callback_success_response() -> Response<Body> {
    build_auth_json_response(http::StatusCode::OK, json!({ "ok": true }), None)
}

pub(super) fn payment_callback_mark_failed_response() -> Response<Body> {
    build_auth_json_response(
        http::StatusCode::OK,
        json!({
            "ok": false,
            "error": "payment callback rejected",
        }),
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::{
        payment_callback_mark_failed_response, payment_callback_namespaced_key,
        payment_callback_persistence_projection, payment_callback_secret_is_strong,
        payment_callback_success_response, NormalizedPaymentCallbackRequest,
    };
    use axum::body::to_bytes;
    use serde_json::json;

    #[test]
    fn payment_callback_secret_rejects_short_or_obviously_low_entropy_values() {
        assert!(!payment_callback_secret_is_strong("callback-secret-test"));
        assert!(!payment_callback_secret_is_strong(&"a".repeat(64)));
        assert!(!payment_callback_secret_is_strong(
            "0123456789abcdef\n0123456789abcdef"
        ));
        assert!(payment_callback_secret_is_strong(
            "test-callback-secret-0123456789abcdef"
        ));
    }

    #[test]
    fn payment_callback_key_namespace_is_canonical_and_bounded() {
        assert_eq!(
            payment_callback_namespaced_key(" MANUAL ", "event-1"),
            "manual:event-1"
        );

        let first = payment_callback_namespaced_key("MANUAL", &"a".repeat(128));
        let second = payment_callback_namespaced_key("manual", &"b".repeat(128));
        assert!(first.starts_with("manual:"));
        assert!(first.chars().count() <= 128);
        assert_ne!(first, second);
    }

    #[tokio::test]
    async fn payment_callback_responses_are_fixed_and_do_not_expose_processing_state() {
        let response = payment_callback_mark_failed_response();
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should read");
        let payload: serde_json::Value =
            serde_json::from_slice(&body).expect("body should be JSON");
        assert_eq!(
            payload,
            json!({ "ok": false, "error": "payment callback rejected" })
        );

        let response = payment_callback_success_response();
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should read");
        let payload: serde_json::Value =
            serde_json::from_slice(&body).expect("body should be JSON");
        assert_eq!(payload, json!({ "ok": true }));
    }

    #[test]
    fn payment_callback_persistence_projection_excludes_arbitrary_signed_payload() {
        let payload = NormalizedPaymentCallbackRequest {
            callback_key: "event-1".to_string(),
            order_no: Some("po_1".to_string()),
            gateway_order_id: Some("gateway-1".to_string()),
            amount_usd: 10.0,
            pay_amount: Some(72.0),
            pay_currency: Some("CNY".to_string()),
            exchange_rate: Some(7.2),
            payload: json!({
                "payload": {
                    "client_secret": "pi_1_secret_replayable",
                    "customer_email": "payer@example.com",
                    "authorization": "Bearer upstream-secret"
                }
            }),
        };

        let debug = format!("{payload:?}");
        assert!(debug.contains("[REDACTED]"));
        for secret in [
            "event-1",
            "pi_1_secret_replayable",
            "payer@example.com",
            "upstream-secret",
        ] {
            assert!(!debug.contains(secret), "debug output leaked {secret}");
        }

        let projected = payment_callback_persistence_projection("manual", &payload, true);
        assert_eq!(projected["gateway"], "manual");
        assert_eq!(projected["order_no"], "po_1");
        assert_eq!(projected["gateway_order_id"], "gateway-1");
        assert_eq!(projected["pay_currency"], "CNY");
        assert!(projected.get("status").is_none());
        let encoded = projected.to_string();
        for forbidden in [
            "client_secret",
            "replayable",
            "customer_email",
            "payer@example.com",
            "authorization",
            "upstream-secret",
        ] {
            assert!(!encoded.contains(forbidden), "persisted {forbidden}");
        }

        let rejected = payment_callback_persistence_projection("manual", &payload, false);
        assert_eq!(rejected["signature_valid"], false);
        assert!(rejected.get("status").is_none());
    }
}
