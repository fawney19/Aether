use crate::handlers::admin::request::AdminRequestContext;
use crate::handlers::admin::shared::{query_param_value, unix_secs_to_rfc3339};
use crate::handlers::shared::normalize_payment_currency;
use crate::GatewayAdminPaymentCallbackView;
use aether_data::repository::wallet::stored_timestamp_unix_secs;
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};

const ADMIN_PAYMENTS_DATA_UNAVAILABLE_DETAIL: &str = "Admin payments data unavailable";

#[derive(Default, serde::Deserialize)]
pub(super) struct AdminPaymentOrderCreditRequest {
    #[serde(default)]
    pub(super) gateway_order_id: Option<String>,
    #[serde(default)]
    pub(super) pay_amount: Option<f64>,
    #[serde(default)]
    pub(super) pay_currency: Option<String>,
    #[serde(default)]
    pub(super) exchange_rate: Option<f64>,
    #[serde(default)]
    pub(super) gateway_response: Option<serde_json::Value>,
}

impl std::fmt::Debug for AdminPaymentOrderCreditRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdminPaymentOrderCreditRequest")
            .field("gateway_order_id", &self.gateway_order_id)
            .field("pay_amount", &self.pay_amount)
            .field("pay_currency", &self.pay_currency)
            .field("exchange_rate", &self.exchange_rate)
            .field(
                "gateway_response",
                &self.gateway_response.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
}

pub(super) fn build_admin_payments_data_unavailable_response() -> Response<Body> {
    (
        http::StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "detail": ADMIN_PAYMENTS_DATA_UNAVAILABLE_DETAIL })),
    )
        .into_response()
}

pub(super) fn build_admin_payments_bad_request_response(
    detail: impl Into<String>,
) -> Response<Body> {
    (
        http::StatusCode::BAD_REQUEST,
        Json(json!({ "detail": detail.into() })),
    )
        .into_response()
}

pub(super) fn build_admin_payments_backend_unavailable_response(
    detail: impl Into<String>,
) -> Response<Body> {
    (
        http::StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "detail": detail.into() })),
    )
        .into_response()
}

pub(super) fn build_admin_payment_order_not_found_response() -> Response<Body> {
    (
        http::StatusCode::NOT_FOUND,
        Json(json!({ "detail": "Payment order not found" })),
    )
        .into_response()
}

pub(super) fn build_admin_payment_orders_page_response(
    items: Vec<serde_json::Value>,
    total: u64,
    limit: usize,
    offset: usize,
) -> Response<Body> {
    Json(json!({
        "items": items,
        "total": total,
        "limit": limit,
        "offset": offset,
    }))
    .into_response()
}

pub(super) fn parse_admin_payments_limit(query: Option<&str>) -> Result<usize, String> {
    match query_param_value(query, "limit") {
        Some(value) => {
            let parsed = value
                .parse::<usize>()
                .map_err(|_| "limit must be an integer between 1 and 200".to_string())?;
            if (1..=200).contains(&parsed) {
                Ok(parsed)
            } else {
                Err("limit must be an integer between 1 and 200".to_string())
            }
        }
        None => Ok(50),
    }
}

pub(super) fn parse_admin_payments_offset(query: Option<&str>) -> Result<usize, String> {
    match query_param_value(query, "offset") {
        Some(value) => value
            .parse::<usize>()
            .map_err(|_| "offset must be a non-negative integer".to_string()),
        None => Ok(0),
    }
}

pub(super) fn admin_payment_order_id_from_detail_path(request_path: &str) -> Option<String> {
    request_path
        .strip_prefix("/api/admin/payments/orders/")?
        .trim()
        .trim_matches('/')
        .split('/')
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .filter(|value| !value.contains('/'))
        .map(ToOwned::to_owned)
}

pub(super) fn admin_payment_order_id_from_suffix_path(
    request_path: &str,
    suffix: &str,
) -> Option<String> {
    request_path
        .trim()
        .trim_end_matches('/')
        .strip_prefix("/api/admin/payments/orders/")?
        .strip_suffix(suffix)
        .map(|value| value.trim().trim_matches('/').to_string())
        .filter(|value| !value.is_empty() && !value.contains('/'))
}

pub(super) fn normalize_admin_payment_optional_string(
    value: Option<String>,
    field_name: &str,
    max_len: usize,
) -> Result<Option<String>, String> {
    match value {
        None => Ok(None),
        Some(value) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                return Ok(None);
            }
            if trimmed.chars().count() > max_len {
                return Err(format!("{field_name} 长度不能超过 {max_len}"));
            }
            Ok(Some(trimmed.to_string()))
        }
    }
}

pub(super) fn normalize_admin_payment_currency(
    value: Option<String>,
) -> Result<Option<String>, String> {
    let Some(value) = normalize_admin_payment_optional_string(value, "pay_currency", 3)? else {
        return Ok(None);
    };
    normalize_payment_currency(&value, "pay_currency")
        .map(Some)
        .map_err(|_| "pay_currency 必须是 3 位 ASCII 货币代码".to_string())
}

pub(super) fn normalize_admin_payment_positive_number(
    value: Option<f64>,
    field_name: &str,
) -> Result<Option<f64>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    if !value.is_finite() || value <= 0.0 {
        return Err(format!("{field_name} 必须为大于 0 的有限数字"));
    }
    Ok(Some(value))
}

pub(super) fn admin_payment_operator_id(
    request_context: &AdminRequestContext<'_>,
) -> Option<String> {
    request_context
        .decision()
        .and_then(|decision| decision.admin_principal.as_ref())
        .map(|principal| principal.user_id.clone())
}

pub(super) fn admin_payment_effective_status(
    status: &str,
    expires_at_unix_secs: Option<u64>,
) -> String {
    let now_unix_secs = chrono::Utc::now().timestamp().max(0) as u64;
    if status == "pending" && expires_at_unix_secs.is_some_and(|value| value <= now_unix_secs) {
        "expired".to_string()
    } else {
        status.to_string()
    }
}

fn admin_payment_bounded_string(value: &Value, max_chars: usize) -> Option<Value> {
    let value = value.as_str()?.trim();
    (!value.is_empty() && value.chars().count() <= max_chars)
        .then(|| Value::String(value.to_string()))
}

fn admin_payment_identifier(value: &Value, max_chars: usize) -> Option<Value> {
    let value = value.as_str()?.trim();
    (!value.is_empty()
        && value.chars().count() <= max_chars
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-')))
    .then(|| Value::String(value.to_string()))
}

fn admin_payment_gateway_response_field(key: &str, value: &Value) -> Option<Value> {
    match key {
        "gateway" | "submit_method" | "payment_channel" => admin_payment_identifier(value, 64),
        "pay_currency" => admin_payment_identifier(value, 16),
        "display_name" | "provider_label" => admin_payment_bounded_string(value, 128),
        "gateway_order_id" | "intent_id" => admin_payment_bounded_string(value, 256),
        "expires_at" => admin_payment_bounded_string(value, 64),
        "pay_amount" | "base_pay_amount" | "fee_rate" | "fee_amount" => {
            value.is_number().then(|| value.clone())
        }
        "manual_credit" => value.as_bool().map(Value::Bool),
        "payment_method_types" => {
            let values = value.as_array()?;
            if values.len() > 16 {
                return None;
            }
            values
                .iter()
                .map(|value| admin_payment_identifier(value, 64))
                .collect::<Option<Vec<_>>>()
                .map(Value::Array)
        }
        _ => None,
    }
}

pub(in crate::handlers::admin) fn admin_payment_gateway_response_projection(
    value: Option<&Value>,
) -> Value {
    let Some(object) = value.and_then(Value::as_object) else {
        return Value::Null;
    };
    Value::Object(
        object
            .iter()
            .filter_map(|(key, value)| {
                admin_payment_gateway_response_field(key, value).map(|value| (key.clone(), value))
            })
            .collect(),
    )
}

pub(super) fn prepare_admin_payment_gateway_response_for_storage(
    value: Option<Value>,
) -> Option<Value> {
    value.map(|value| admin_payment_gateway_response_projection(Some(&value)))
}

#[derive(Default)]
struct AdminPaymentJsonShape {
    objects: u64,
    arrays: u64,
    strings: u64,
    numbers: u64,
    booleans: u64,
    nulls: u64,
    object_fields: u64,
    array_items: u64,
    max_depth: u64,
}

impl AdminPaymentJsonShape {
    fn observe(&mut self, value: &Value, depth: u64) {
        self.max_depth = self.max_depth.max(depth);
        match value {
            Value::Object(object) => {
                self.objects = self.objects.saturating_add(1);
                self.object_fields = self
                    .object_fields
                    .saturating_add(u64::try_from(object.len()).unwrap_or(u64::MAX));
                for value in object.values() {
                    self.observe(value, depth.saturating_add(1));
                }
            }
            Value::Array(values) => {
                self.arrays = self.arrays.saturating_add(1);
                self.array_items = self
                    .array_items
                    .saturating_add(u64::try_from(values.len()).unwrap_or(u64::MAX));
                for value in values {
                    self.observe(value, depth.saturating_add(1));
                }
            }
            Value::String(_) => self.strings = self.strings.saturating_add(1),
            Value::Number(_) => self.numbers = self.numbers.saturating_add(1),
            Value::Bool(_) => self.booleans = self.booleans.saturating_add(1),
            Value::Null => self.nulls = self.nulls.saturating_add(1),
        }
    }
}

fn admin_payment_json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn admin_payment_payload_summary(value: Option<&Value>) -> Value {
    let Some(value) = value else {
        return Value::Null;
    };
    let mut shape = AdminPaymentJsonShape::default();
    shape.observe(value, 1);
    json!({
        "kind": admin_payment_json_kind(value),
        "serialized_bytes": serde_json::to_vec(value).map_or(0, |encoded| encoded.len()),
        "objects": shape.objects,
        "arrays": shape.arrays,
        "strings": shape.strings,
        "numbers": shape.numbers,
        "booleans": shape.booleans,
        "nulls": shape.nulls,
        "object_fields": shape.object_fields,
        "array_items": shape.array_items,
        "max_depth": shape.max_depth,
    })
}

fn admin_payment_callback_error_projection(value: Option<&str>) -> Option<String> {
    const SAFE_ERRORS: &[&str] = &[
        "callback amount mismatch",
        "callback key reused with different payment payload",
        "invalid callback signature",
        "invalid payment callback numeric or identity fields",
        "payment channel mismatch",
        "payment currency mismatch",
        "payment gateway order belongs to another payment order",
        "payment gateway order identifier mismatch",
        "payment gateway order mismatch",
        "payment method mismatch",
        "payment order expired",
        "payment order not found",
        "payment order number mismatch",
        "payment order user missing",
        "payment provider mismatch",
        "plan purchase limit reached",
        "wallet is not active",
        "wallet not found",
    ];

    let value = value?.trim();
    if SAFE_ERRORS.contains(&value) {
        return Some(value.to_string());
    }
    if value.starts_with("payment order is not creditable:") {
        return Some("payment order is not creditable".to_string());
    }
    Some("payment callback processing failed".to_string())
}

pub(super) fn build_admin_payment_order_payload(
    record: &crate::AdminWalletPaymentOrderRecord,
) -> serde_json::Value {
    json!({
        "id": record.id,
        "order_no": record.order_no,
        "wallet_id": record.wallet_id,
        "user_id": record.user_id,
        "amount_usd": record.amount_usd,
        "pay_amount": record.pay_amount,
        "pay_currency": record.pay_currency,
        "exchange_rate": record.exchange_rate,
        "refunded_amount_usd": record.refunded_amount_usd,
        "refundable_amount_usd": record.refundable_amount_usd,
        "payment_method": record.payment_method,
        "gateway_order_id": record.gateway_order_id,
        "gateway_response": admin_payment_gateway_response_projection(record.gateway_response.as_ref()),
        "has_gateway_response": record.gateway_response.is_some(),
        "status": admin_payment_effective_status(&record.status, record.expires_at_unix_secs),
        "created_at": unix_secs_to_rfc3339(stored_timestamp_unix_secs(record.created_at_unix_ms)),
        "paid_at": record.paid_at_unix_secs.and_then(unix_secs_to_rfc3339),
        "credited_at": record.credited_at_unix_secs.and_then(unix_secs_to_rfc3339),
        "expires_at": record.expires_at_unix_secs.and_then(unix_secs_to_rfc3339),
    })
}

pub(super) fn build_admin_payment_callback_payload_from_record(
    record: &GatewayAdminPaymentCallbackView,
) -> serde_json::Value {
    json!({
        "id": record.id,
        "payment_order_id": record.payment_order_id,
        "payment_method": record.payment_method,
        "callback_key": record.callback_key,
        "order_no": record.order_no,
        "gateway_order_id": record.gateway_order_id,
        "payload_hash": record.payload_hash,
        "signature_valid": record.signature_valid,
        "status": record.status,
        "payload": Value::Null,
        "has_payload": record.payload.is_some(),
        "payload_summary": admin_payment_payload_summary(record.payload.as_ref()),
        "error_message": admin_payment_callback_error_projection(record.error_message.as_deref()),
        "has_error_message": record.error_message.is_some(),
        "created_at": unix_secs_to_rfc3339(stored_timestamp_unix_secs(record.created_at_unix_ms)),
        "processed_at": record.processed_at_unix_secs.and_then(unix_secs_to_rfc3339),
    })
}

#[cfg(test)]
mod tests {
    use super::{
        build_admin_payment_callback_payload_from_record, build_admin_payment_order_payload,
        prepare_admin_payment_gateway_response_for_storage,
    };
    use crate::{AdminWalletPaymentOrderRecord, GatewayAdminPaymentCallbackView};
    use serde_json::json;

    #[test]
    fn admin_payment_order_projection_excludes_replayable_gateway_fields() {
        let record = AdminWalletPaymentOrderRecord {
            id: "order-1".to_string(),
            order_no: "merchant-order-1".to_string(),
            wallet_id: "wallet-1".to_string(),
            user_id: Some("user-1".to_string()),
            amount_usd: 10.0,
            pay_amount: Some(72.0),
            pay_currency: Some("CNY".to_string()),
            exchange_rate: Some(7.2),
            refunded_amount_usd: 0.0,
            refundable_amount_usd: 0.0,
            payment_method: "stripe".to_string(),
            gateway_order_id: Some("pi_1".to_string()),
            status: "pending".to_string(),
            gateway_response: Some(json!({
                "gateway": "stripe",
                "intent_id": "pi_1",
                "client_secret": "pi_1_secret_replayable",
                "payment_url": "https://pay.example/checkout?token=secret",
                "payment_params": {"sign": "signed-secret"},
                "customer_email": "payer@example.com"
            })),
            created_at_unix_ms: 1,
            paid_at_unix_secs: None,
            credited_at_unix_secs: None,
            expires_at_unix_secs: None,
        };

        let payload = build_admin_payment_order_payload(&record);
        assert_eq!(payload["has_gateway_response"], true);
        assert_eq!(
            payload.pointer("/gateway_response/gateway"),
            Some(&json!("stripe"))
        );
        assert_eq!(
            payload.pointer("/gateway_response/intent_id"),
            Some(&json!("pi_1"))
        );
        for key in [
            "client_secret",
            "payment_url",
            "payment_params",
            "customer_email",
        ] {
            assert!(payload
                .pointer(&format!("/gateway_response/{key}"))
                .is_none());
        }
    }

    #[test]
    fn admin_payment_order_projection_rejects_nested_or_mistyped_safe_fields() {
        let mut record = AdminWalletPaymentOrderRecord {
            id: "order-1".to_string(),
            order_no: "merchant-order-1".to_string(),
            wallet_id: "wallet-1".to_string(),
            user_id: Some("user-1".to_string()),
            amount_usd: 10.0,
            pay_amount: Some(72.0),
            pay_currency: Some("CNY".to_string()),
            exchange_rate: Some(7.2),
            refunded_amount_usd: 0.0,
            refundable_amount_usd: 0.0,
            payment_method: "stripe".to_string(),
            gateway_order_id: Some("pi_1".to_string()),
            status: "pending".to_string(),
            gateway_response: None,
            created_at_unix_ms: 1,
            paid_at_unix_secs: None,
            credited_at_unix_secs: None,
            expires_at_unix_secs: None,
        };
        record.gateway_response = Some(json!({
            "gateway": {"client_secret": "secret-in-nested-object"},
            "intent_id": ["pi_1", "secret-in-array"],
            "payment_method_types": ["card", {"secret": "nested"}],
            "manual_credit": "secret-in-string",
        }));

        let encoded = build_admin_payment_order_payload(&record).to_string();
        assert!(!encoded.contains("secret-in-nested-object"));
        assert!(!encoded.contains("secret-in-array"));
        assert!(!encoded.contains("nested"));
        assert!(!encoded.contains("secret-in-string"));
    }

    #[test]
    fn admin_payment_gateway_response_is_projected_before_storage() {
        let projected = prepare_admin_payment_gateway_response_for_storage(Some(json!({
            "gateway": "stripe",
            "intent_id": "pi_1",
            "client_secret": "pi_1_secret_replayable",
            "customer": {"email": "payer@example.com"},
            "payment_params": {"authorization": "Bearer secret"},
        })))
        .expect("provided gateway response should remain present");

        assert_eq!(projected, json!({"gateway": "stripe", "intent_id": "pi_1"}));
        let encoded = projected.to_string();
        for forbidden in [
            "client_secret",
            "replayable",
            "customer",
            "payer@example.com",
            "authorization",
            "Bearer secret",
        ] {
            assert!(!encoded.contains(forbidden), "persisted {forbidden}");
        }
    }

    #[test]
    fn admin_payment_callback_projection_does_not_return_raw_payload() {
        let record = GatewayAdminPaymentCallbackView {
            id: "callback-1".to_string(),
            payment_order_id: Some("order-1".to_string()),
            payment_method: "stripe".to_string(),
            callback_key: "stripe:event-1".to_string(),
            order_no: Some("merchant-order-1".to_string()),
            gateway_order_id: Some("pi_1".to_string()),
            payload_hash: Some("hash-1".to_string()),
            signature_valid: true,
            status: "processed".to_string(),
            payload: Some(json!({
                "data": {"object": {"client_secret": "secret", "customer_email": "payer@example.com"}}
            })),
            error_message: None,
            created_at_unix_ms: 1,
            processed_at_unix_secs: Some(1),
        };

        let payload = build_admin_payment_callback_payload_from_record(&record);
        assert_eq!(payload["has_payload"], true);
        assert!(payload["payload"].is_null());
        assert_eq!(payload["payload_summary"]["kind"], "object");
        assert_eq!(payload["payload_summary"]["objects"], 3);
        assert_eq!(payload["payload_summary"]["strings"], 2);
        assert_eq!(payload["payload_summary"]["max_depth"], 4);
        let encoded = payload.to_string();
        assert!(!encoded.contains("customer_email"));
        assert!(!encoded.contains("payer@example.com"));
        assert!(!encoded.contains("client_secret"));
        assert!(!encoded.contains("secret"));
    }

    #[test]
    fn admin_payment_callback_projection_does_not_return_unknown_historical_errors() {
        let record = GatewayAdminPaymentCallbackView {
            id: "callback-1".to_string(),
            payment_order_id: None,
            payment_method: "stripe".to_string(),
            callback_key: "stripe:event-1".to_string(),
            order_no: None,
            gateway_order_id: None,
            payload_hash: None,
            signature_valid: false,
            status: "failed".to_string(),
            payload: None,
            error_message: Some("upstream rejected sk_live_secret_value".to_string()),
            created_at_unix_ms: 1,
            processed_at_unix_secs: Some(1),
        };

        let payload = build_admin_payment_callback_payload_from_record(&record);
        assert_eq!(payload["has_error_message"], true);
        assert_eq!(
            payload["error_message"],
            "payment callback processing failed"
        );
        assert!(!payload.to_string().contains("sk_live_secret_value"));
    }
}
