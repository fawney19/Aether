use axum::{body::Body, http, response::Response};
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;

use super::{
    build_auth_error_response, build_auth_json_response,
    handle_payment_callback_input_with_wallet_repository,
    payment_shared::payment_callback_payload_hash, AppState, GatewayPublicRequestContext,
};

const STRIPE_SIGNATURE_HEADER: &str = "stripe-signature";
const STRIPE_SIGNATURE_TOLERANCE_SECONDS: i64 = 300;

type HmacSha256 = Hmac<Sha256>;

fn decrypt_gateway_secrets(
    state: &AppState,
    record: &aether_data_contracts::repository::billing::PaymentGatewayConfigRecord,
) -> Result<serde_json::Map<String, Value>, String> {
    let Some(encrypted) = record.merchant_key_encrypted.as_deref() else {
        return Err("Stripe webhook_secret 未配置".to_string());
    };
    let Some(plaintext) = crate::handlers::shared::decrypt_catalog_secret_with_fallbacks(
        state.encryption_key(),
        encrypted,
    ) else {
        return Err("Stripe 密钥解密失败".to_string());
    };
    serde_json::from_str::<Value>(&plaintext)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .ok_or_else(|| "Stripe 密钥格式无效".to_string())
}

fn gateway_secret_string(secrets: &serde_json::Map<String, Value>, key: &str) -> Option<String> {
    secrets
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

async fn stripe_webhook_secret(state: &AppState) -> Result<String, String> {
    let Some(record) = state
        .find_payment_gateway_config("stripe")
        .await
        .map_err(|err| format!("Stripe 配置读取失败: {err:?}"))?
    else {
        return Err("Stripe 未配置".to_string());
    };
    if !record.enabled {
        return Err("Stripe 未启用".to_string());
    }
    let secrets = decrypt_gateway_secrets(state, &record)?;
    gateway_secret_string(&secrets, "webhook_secret")
        .ok_or_else(|| "Stripe webhook_secret 未配置".to_string())
}

fn parse_stripe_signature_header(value: &str) -> Option<(i64, Vec<&str>)> {
    let mut timestamp = None;
    let mut signatures = Vec::new();
    for part in value.split(',') {
        let Some((key, value)) = part.trim().split_once('=') else {
            continue;
        };
        match key.trim() {
            "t" => timestamp = value.trim().parse::<i64>().ok(),
            "v1" => {
                let signature = value.trim();
                if !signature.is_empty() {
                    signatures.push(signature);
                }
            }
            _ => {}
        }
    }
    timestamp.map(|value| (value, signatures))
}

fn hex_to_bytes(value: &str) -> Option<Vec<u8>> {
    let value = value.trim();
    if !value.len().is_multiple_of(2) {
        return None;
    }
    let mut bytes = Vec::with_capacity(value.len() / 2);
    let mut chars = value.as_bytes().chunks_exact(2);
    for chunk in &mut chars {
        let hex = std::str::from_utf8(chunk).ok()?;
        bytes.push(u8::from_str_radix(hex, 16).ok()?);
    }
    Some(bytes)
}

fn stripe_signature_matches_at(
    secret: &str,
    signature_header: &str,
    body: &[u8],
    now_unix_secs: i64,
) -> Result<bool, String> {
    let Some((timestamp, signatures)) = parse_stripe_signature_header(signature_header) else {
        return Ok(false);
    };
    if signatures.is_empty() {
        return Ok(false);
    }
    if (now_unix_secs - timestamp).abs() > STRIPE_SIGNATURE_TOLERANCE_SECONDS {
        return Ok(false);
    }

    for signature in signatures {
        let Some(signature_bytes) = hex_to_bytes(signature) else {
            continue;
        };
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .map_err(|err| format!("Stripe webhook HMAC 初始化失败: {err}"))?;
        mac.update(timestamp.to_string().as_bytes());
        mac.update(b".");
        mac.update(body);
        if mac.verify_slice(&signature_bytes).is_ok() {
            return Ok(true);
        }
    }
    Ok(false)
}

fn stripe_amount_multiplier(currency: &str) -> f64 {
    match currency.trim().to_ascii_lowercase().as_str() {
        "bif" | "clp" | "djf" | "gnf" | "jpy" | "kmf" | "krw" | "mga" | "pyg" | "rwf" | "ugx"
        | "vnd" | "vuv" | "xaf" | "xof" | "xpf" => 1.0,
        _ => 100.0,
    }
}

fn stripe_amount_to_major(amount_minor: i64, currency: &str) -> f64 {
    amount_minor as f64 / stripe_amount_multiplier(currency)
}

fn stripe_string_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn stripe_payment_intent_channel(intent: &Value) -> Option<String> {
    intent
        .get("payment_method_types")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::trim)
        .find(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

async fn build_stripe_callback_input(
    state: &AppState,
    event: Value,
) -> Result<Option<aether_data::repository::wallet::ProcessPaymentCallbackInput>, String> {
    let event_type = stripe_string_field(&event, "type").unwrap_or("");
    if event_type != "payment_intent.succeeded" {
        return Ok(None);
    }

    let event_id = stripe_string_field(&event, "id")
        .ok_or_else(|| "Stripe 事件缺少 id".to_string())?
        .to_string();
    let intent = event
        .get("data")
        .and_then(|value| value.get("object"))
        .ok_or_else(|| "Stripe 事件缺少 PaymentIntent".to_string())?;
    let intent_id = stripe_string_field(intent, "id")
        .ok_or_else(|| "Stripe PaymentIntent 缺少 id".to_string())?
        .to_string();
    let order_no = intent
        .get("metadata")
        .and_then(Value::as_object)
        .and_then(|metadata| metadata.get("order_no"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let Some(order_no) = order_no else {
        return Err("Stripe PaymentIntent 缺少 metadata.order_no".to_string());
    };
    let currency = stripe_string_field(intent, "currency")
        .unwrap_or("usd")
        .to_ascii_uppercase();
    let amount_minor = intent
        .get("amount_received")
        .or_else(|| intent.get("amount"))
        .and_then(Value::as_i64)
        .filter(|value| *value > 0)
        .ok_or_else(|| "Stripe PaymentIntent 金额无效".to_string())?;
    let pay_amount = stripe_amount_to_major(amount_minor, &currency);

    let record = state
        .find_payment_gateway_config("stripe")
        .await
        .map_err(|err| format!("Stripe 配置读取失败: {err:?}"))?
        .ok_or_else(|| "Stripe 未配置".to_string())?;
    let exchange_rate = record.usd_exchange_rate;
    let amount_usd = if exchange_rate > 0.0 {
        pay_amount / exchange_rate
    } else {
        pay_amount
    };
    let payload_hash = payment_callback_payload_hash(&event)?;
    Ok(Some(
        aether_data::repository::wallet::ProcessPaymentCallbackInput {
            payment_method: "stripe".to_string(),
            payment_provider: Some("stripe".to_string()),
            payment_channel: stripe_payment_intent_channel(intent),
            callback_key: event_id,
            order_no: Some(order_no),
            gateway_order_id: Some(intent_id),
            amount_usd,
            pay_amount: Some(pay_amount),
            pay_currency: Some(currency),
            exchange_rate: Some(exchange_rate),
            payload_hash,
            payload: event,
            signature_valid: true,
        },
    ))
}

pub(super) async fn handle_stripe_webhook(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
    request_body: Option<&axum::body::Bytes>,
) -> Response<Body> {
    let Some(request_body) = request_body else {
        return build_auth_error_response(http::StatusCode::BAD_REQUEST, "缺少请求体", false);
    };
    let Some(signature_header) = crate::headers::header_value_str(headers, STRIPE_SIGNATURE_HEADER)
    else {
        return build_auth_error_response(
            http::StatusCode::UNAUTHORIZED,
            "缺少 Stripe-Signature",
            false,
        );
    };
    let webhook_secret = match stripe_webhook_secret(state).await {
        Ok(value) => value,
        Err(detail) => {
            return build_auth_error_response(http::StatusCode::SERVICE_UNAVAILABLE, detail, false)
        }
    };
    let now = chrono::Utc::now().timestamp();
    match stripe_signature_matches_at(&webhook_secret, &signature_header, request_body, now) {
        Ok(true) => {}
        Ok(false) => {
            return build_auth_error_response(
                http::StatusCode::UNAUTHORIZED,
                "Stripe webhook 签名无效",
                false,
            )
        }
        Err(detail) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                detail,
                false,
            )
        }
    }

    let event = match serde_json::from_slice::<Value>(request_body) {
        Ok(value) => value,
        Err(_) => {
            return build_auth_error_response(
                http::StatusCode::BAD_REQUEST,
                "Stripe webhook 请求体无效",
                false,
            )
        }
    };
    let input = match build_stripe_callback_input(state, event).await {
        Ok(Some(value)) => value,
        Ok(None) => {
            return build_auth_json_response(
                http::StatusCode::OK,
                json!({ "ok": true, "ignored": true, "payment_method": "stripe" }),
                None,
            )
        }
        Err(detail) => {
            return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false)
        }
    };

    handle_payment_callback_input_with_wallet_repository(state, request_context, input).await
}

#[cfg(test)]
mod tests {
    use super::{stripe_amount_to_major, stripe_signature_matches_at};
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    #[test]
    fn stripe_signature_matches_raw_body() {
        let body = br#"{"id":"evt_1","type":"payment_intent.succeeded"}"#;
        let secret = "whsec_test";
        let timestamp = 1_800_000_000_i64;
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac");
        mac.update(timestamp.to_string().as_bytes());
        mac.update(b".");
        mac.update(body);
        let signature = mac
            .finalize()
            .into_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let header = format!("t={timestamp},v1={signature}");

        assert!(
            stripe_signature_matches_at(secret, &header, body, timestamp)
                .expect("signature check should run")
        );
        assert!(
            !stripe_signature_matches_at("wrong", &header, body, timestamp)
                .expect("signature check should run")
        );
    }

    #[test]
    fn stripe_amount_handles_zero_decimal_currencies() {
        assert_eq!(stripe_amount_to_major(1234, "usd"), 12.34);
        assert_eq!(stripe_amount_to_major(1234, "jpy"), 1234.0);
    }
}
