use crate::handlers::admin::request::AdminRequestContext;
use crate::handlers::admin::shared::{query_param_value, unix_secs_to_rfc3339};
use crate::{GatewayAdminPaymentCallbackView, GatewayError};
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use sqlx::Row;

const ADMIN_PAYMENTS_DATA_UNAVAILABLE_DETAIL: &str = "Admin payments data unavailable";

#[derive(Debug, Default, serde::Deserialize)]
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
    let normalized = value.to_ascii_uppercase();
    if normalized.len() != 3 {
        return Err("pay_currency 必须是 3 位货币代码".to_string());
    }
    Ok(Some(normalized))
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
    if status == "pending" && expires_at_unix_secs.is_some_and(|value| value < now_unix_secs) {
        "expired".to_string()
    } else {
        status.to_string()
    }
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
        "gateway_response": record.gateway_response,
        "status": admin_payment_effective_status(&record.status, record.expires_at_unix_secs),
        "created_at": unix_secs_to_rfc3339(record.created_at_unix_secs),
        "paid_at": record.paid_at_unix_secs.and_then(unix_secs_to_rfc3339),
        "credited_at": record.credited_at_unix_secs.and_then(unix_secs_to_rfc3339),
        "expires_at": record.expires_at_unix_secs.and_then(unix_secs_to_rfc3339),
    })
}

pub(super) fn build_admin_payment_callback_payload(
    row: &sqlx::postgres::PgRow,
) -> Result<serde_json::Value, GatewayError> {
    Ok(json!({
        "id": row.try_get::<String, _>("id").map_err(|err| GatewayError::Internal(err.to_string()))?,
        "payment_order_id": row.try_get::<Option<String>, _>("payment_order_id").map_err(|err| GatewayError::Internal(err.to_string()))?,
        "payment_method": row.try_get::<String, _>("payment_method").map_err(|err| GatewayError::Internal(err.to_string()))?,
        "callback_key": row.try_get::<String, _>("callback_key").map_err(|err| GatewayError::Internal(err.to_string()))?,
        "order_no": row.try_get::<Option<String>, _>("order_no").map_err(|err| GatewayError::Internal(err.to_string()))?,
        "gateway_order_id": row.try_get::<Option<String>, _>("gateway_order_id").map_err(|err| GatewayError::Internal(err.to_string()))?,
        "payload_hash": row.try_get::<Option<String>, _>("payload_hash").map_err(|err| GatewayError::Internal(err.to_string()))?,
        "signature_valid": row.try_get::<bool, _>("signature_valid").map_err(|err| GatewayError::Internal(err.to_string()))?,
        "status": row.try_get::<String, _>("status").map_err(|err| GatewayError::Internal(err.to_string()))?,
        "payload": row.try_get::<Option<serde_json::Value>, _>("payload").map_err(|err| GatewayError::Internal(err.to_string()))?,
        "error_message": row.try_get::<Option<String>, _>("error_message").map_err(|err| GatewayError::Internal(err.to_string()))?,
        "created_at": row
            .try_get::<Option<i64>, _>("created_at_unix_secs")
            .map_err(|err| GatewayError::Internal(err.to_string()))?
            .and_then(|value| u64::try_from(value).ok())
            .and_then(unix_secs_to_rfc3339),
        "processed_at": row
            .try_get::<Option<i64>, _>("processed_at_unix_secs")
            .map_err(|err| GatewayError::Internal(err.to_string()))?
            .and_then(|value| u64::try_from(value).ok())
            .and_then(unix_secs_to_rfc3339),
    }))
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
        "payload": record.payload,
        "error_message": record.error_message,
        "created_at": unix_secs_to_rfc3339(record.created_at_unix_secs),
        "processed_at": record.processed_at_unix_secs.and_then(unix_secs_to_rfc3339),
    })
}
