use super::{
    admin_payment_operator_id, build_admin_payment_order_not_found_response,
    build_admin_payments_backend_unavailable_response, build_admin_payments_bad_request_response,
    parse_admin_payments_limit, parse_admin_payments_offset,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::{
    attach_admin_audit_response, query_param_value, unix_secs_to_rfc3339,
};
use crate::GatewayError;
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Deserialize)]
pub(super) struct AdminRedeemCodeBatchCreateRequest {
    pub(super) name: String,
    pub(super) amount_usd: f64,
    pub(super) total_count: usize,
    #[serde(default)]
    pub(super) expires_at: Option<String>,
    #[serde(default)]
    pub(super) description: Option<String>,
}

fn normalize_required_text(
    value: &str,
    field_name: &str,
    max_len: usize,
) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{field_name} 不能为空"));
    }
    if trimmed.chars().count() > max_len {
        return Err(format!("{field_name} 长度不能超过 {max_len}"));
    }
    Ok(trimmed.to_string())
}

fn normalize_optional_text(
    value: Option<String>,
    field_name: &str,
    max_len: usize,
) -> Result<Option<String>, String> {
    match value {
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
        None => Ok(None),
    }
}

fn parse_batch_id_from_detail_path(path: &str) -> Option<String> {
    path.trim_end_matches('/')
        .strip_prefix("/api/admin/payments/redeem-codes/batches/")?
        .split('/')
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty() && !value.contains('/'))
        .map(ToOwned::to_owned)
}

fn parse_batch_id_from_suffix_path(path: &str, suffix: &str) -> Option<String> {
    path.trim_end_matches('/')
        .strip_prefix("/api/admin/payments/redeem-codes/batches/")?
        .strip_suffix(suffix)
        .map(|value| value.trim().trim_matches('/').to_string())
        .filter(|value| !value.is_empty() && !value.contains('/'))
}

fn parse_code_id_from_suffix_path(path: &str, suffix: &str) -> Option<String> {
    path.trim_end_matches('/')
        .strip_prefix("/api/admin/payments/redeem-codes/codes/")?
        .strip_suffix(suffix)
        .map(|value| value.trim().trim_matches('/').to_string())
        .filter(|value| !value.is_empty() && !value.contains('/'))
}

fn parse_batch_expires_at(value: Option<String>) -> Result<Option<u64>, String> {
    let Some(value) = normalize_optional_text(value, "expires_at", 64)? else {
        return Ok(None);
    };
    let parsed = chrono::DateTime::parse_from_rfc3339(&value)
        .map_err(|_| "expires_at 必须为 ISO8601 时间".to_string())?;
    Ok(Some(parsed.timestamp().max(0) as u64))
}

fn build_redeem_code_not_found_response(detail: &str) -> Response<Body> {
    (
        http::StatusCode::NOT_FOUND,
        Json(json!({ "detail": detail })),
    )
        .into_response()
}

fn build_batch_payload(
    batch: &aether_data::repository::wallet::StoredAdminRedeemCodeBatch,
) -> serde_json::Value {
    json!({
        "id": batch.id,
        "name": batch.name,
        "amount_usd": batch.amount_usd,
        "currency": batch.currency,
        "balance_bucket": batch.balance_bucket,
        "total_count": batch.total_count,
        "redeemed_count": batch.redeemed_count,
        "active_count": batch.active_count,
        "status": batch.status,
        "description": batch.description,
        "created_by": batch.created_by,
        "expires_at": batch.expires_at_unix_secs.and_then(unix_secs_to_rfc3339),
        "created_at": unix_secs_to_rfc3339(batch.created_at_unix_ms),
        "updated_at": unix_secs_to_rfc3339(batch.updated_at_unix_secs),
    })
}

fn build_code_payload(
    code: &aether_data::repository::wallet::StoredAdminRedeemCode,
) -> serde_json::Value {
    json!({
        "id": code.id,
        "batch_id": code.batch_id,
        "batch_name": code.batch_name,
        "code_prefix": code.code_prefix,
        "code_suffix": code.code_suffix,
        "masked_code": code.masked_code,
        "status": code.status,
        "redeemed_by_user_id": code.redeemed_by_user_id,
        "redeemed_by_user_name": code.redeemed_by_user_name,
        "redeemed_wallet_id": code.redeemed_wallet_id,
        "redeemed_payment_order_id": code.redeemed_payment_order_id,
        "redeemed_order_no": code.redeemed_order_no,
        "redeemed_at": code.redeemed_at_unix_secs.and_then(unix_secs_to_rfc3339),
        "disabled_by": code.disabled_by,
        "expires_at": code.expires_at_unix_secs.and_then(unix_secs_to_rfc3339),
        "created_at": unix_secs_to_rfc3339(code.created_at_unix_ms),
        "updated_at": unix_secs_to_rfc3339(code.updated_at_unix_secs),
    })
}

pub(super) async fn maybe_build_local_admin_redeem_codes_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&axum::body::Bytes>,
    route_kind: Option<&str>,
) -> Result<Option<Response<Body>>, GatewayError> {
    match route_kind {
        Some("list_redeem_code_batches") => Ok(Some(
            build_admin_redeem_code_batches_response(state, request_context).await?,
        )),
        Some("create_redeem_code_batch") => Ok(Some(
            build_admin_create_redeem_code_batch_response(state, request_context, request_body)
                .await?,
        )),
        Some("get_redeem_code_batch") => Ok(Some(
            build_admin_redeem_code_batch_detail_response(state, request_context).await?,
        )),
        Some("list_redeem_codes") => Ok(Some(
            build_admin_redeem_codes_response(state, request_context).await?,
        )),
        Some("disable_redeem_code_batch") => Ok(Some(
            build_admin_disable_redeem_code_batch_response(state, request_context).await?,
        )),
        Some("delete_redeem_code_batch") => Ok(Some(
            build_admin_delete_redeem_code_batch_response(state, request_context).await?,
        )),
        Some("disable_redeem_code") => Ok(Some(
            build_admin_disable_redeem_code_response(state, request_context).await?,
        )),
        _ => Ok(None),
    }
}

async fn build_admin_redeem_code_batches_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let query = request_context.query_string();
    let limit = match parse_admin_payments_limit(query) {
        Ok(value) => value,
        Err(detail) => return Ok(build_admin_payments_bad_request_response(detail)),
    };
    let offset = match parse_admin_payments_offset(query) {
        Ok(value) => value,
        Err(detail) => return Ok(build_admin_payments_bad_request_response(detail)),
    };
    let status = query_param_value(query, "status");
    let (items, total) = state
        .list_admin_redeem_code_batches(status.as_deref(), limit, offset)
        .await?;
    Ok(Json(json!({
        "items": items.iter().map(build_batch_payload).collect::<Vec<_>>(),
        "total": total,
        "limit": limit,
        "offset": offset,
    }))
    .into_response())
}

async fn build_admin_create_redeem_code_batch_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&axum::body::Bytes>,
) -> Result<Response<Body>, GatewayError> {
    let Some(body) = request_body else {
        return Ok(build_admin_payments_bad_request_response("请求体不能为空"));
    };
    let payload = match serde_json::from_slice::<AdminRedeemCodeBatchCreateRequest>(body) {
        Ok(value) => value,
        Err(_) => {
            return Ok(build_admin_payments_bad_request_response(
                "请求数据验证失败",
            ))
        }
    };

    let name = match normalize_required_text(&payload.name, "name", 120) {
        Ok(value) => value,
        Err(detail) => return Ok(build_admin_payments_bad_request_response(detail)),
    };
    if !payload.amount_usd.is_finite() || payload.amount_usd <= 0.0 {
        return Ok(build_admin_payments_bad_request_response(
            "amount_usd 必须大于 0",
        ));
    }
    if payload.total_count == 0 || payload.total_count > 5000 {
        return Ok(build_admin_payments_bad_request_response(
            "total_count 必须在 1 到 5000 之间",
        ));
    }
    let description = match normalize_optional_text(payload.description, "description", 500) {
        Ok(value) => value,
        Err(detail) => return Ok(build_admin_payments_bad_request_response(detail)),
    };
    let expires_at_unix_secs = match parse_batch_expires_at(payload.expires_at) {
        Ok(value) => value,
        Err(detail) => return Ok(build_admin_payments_bad_request_response(detail)),
    };

    let Some(result) = state
        .admin_create_redeem_code_batch(
            aether_data::repository::wallet::CreateAdminRedeemCodeBatchInput {
                name,
                amount_usd: payload.amount_usd,
                currency: "USD".to_string(),
                balance_bucket: "gift".to_string(),
                total_count: payload.total_count,
                expires_at_unix_secs,
                description,
                created_by: admin_payment_operator_id(request_context),
            },
        )
        .await?
    else {
        return Ok(build_admin_payments_backend_unavailable_response(
            "Redeem code batch backend unavailable",
        ));
    };

    Ok(attach_admin_audit_response(
        Json(json!({
            "batch": build_batch_payload(&result.batch),
            "codes": result
                .codes
                .iter()
                .map(|code| json!({
                    "id": code.code_id,
                    "code": code.code,
                    "masked_code": code.masked_code,
                }))
                .collect::<Vec<_>>(),
        }))
        .into_response(),
        "admin_redeem_code_batch_created",
        "create_redeem_code_batch",
        "redeem_code_batch",
        &result.batch.id,
    ))
}

async fn build_admin_redeem_code_batch_detail_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let Some(batch_id) = parse_batch_id_from_detail_path(request_context.path()) else {
        return Ok(build_admin_payment_order_not_found_response());
    };
    match state.read_admin_redeem_code_batch(&batch_id).await? {
        crate::AdminWalletMutationOutcome::Applied(batch) => {
            Ok(Json(json!({ "batch": build_batch_payload(&batch) })).into_response())
        }
        crate::AdminWalletMutationOutcome::NotFound => Ok(build_redeem_code_not_found_response(
            "Redeem code batch not found",
        )),
        crate::AdminWalletMutationOutcome::Invalid(detail) => {
            Ok(build_admin_payments_bad_request_response(detail))
        }
        crate::AdminWalletMutationOutcome::Unavailable => {
            Ok(build_admin_payments_backend_unavailable_response(
                "Redeem code batch backend unavailable",
            ))
        }
    }
}

async fn build_admin_redeem_codes_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let Some(batch_id) = parse_batch_id_from_suffix_path(request_context.path(), "/codes") else {
        return Ok(build_admin_payment_order_not_found_response());
    };
    let query = request_context.query_string();
    let limit = match parse_admin_payments_limit(query) {
        Ok(value) => value,
        Err(detail) => return Ok(build_admin_payments_bad_request_response(detail)),
    };
    let offset = match parse_admin_payments_offset(query) {
        Ok(value) => value,
        Err(detail) => return Ok(build_admin_payments_bad_request_response(detail)),
    };
    let status = query_param_value(query, "status");
    match state.read_admin_redeem_code_batch(&batch_id).await? {
        crate::AdminWalletMutationOutcome::Applied(batch) => {
            let page = state
                .list_admin_redeem_codes(&batch_id, status.as_deref(), limit, offset)
                .await?;
            Ok(Json(json!({
                "batch": build_batch_payload(&batch),
                "items": page.items.iter().map(build_code_payload).collect::<Vec<_>>(),
                "total": page.total,
                "limit": limit,
                "offset": offset,
            }))
            .into_response())
        }
        crate::AdminWalletMutationOutcome::NotFound => Ok(build_redeem_code_not_found_response(
            "Redeem code batch not found",
        )),
        crate::AdminWalletMutationOutcome::Invalid(detail) => {
            Ok(build_admin_payments_bad_request_response(detail))
        }
        crate::AdminWalletMutationOutcome::Unavailable => {
            Ok(build_admin_payments_backend_unavailable_response(
                "Redeem code batch backend unavailable",
            ))
        }
    }
}

async fn build_admin_disable_redeem_code_batch_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let Some(batch_id) = parse_batch_id_from_suffix_path(request_context.path(), "/disable") else {
        return Ok(build_admin_payment_order_not_found_response());
    };
    match state
        .admin_disable_redeem_code_batch(
            &batch_id,
            admin_payment_operator_id(request_context).as_deref(),
        )
        .await?
    {
        crate::AdminWalletMutationOutcome::Applied(batch) => Ok(attach_admin_audit_response(
            Json(json!({ "batch": build_batch_payload(&batch) })).into_response(),
            "admin_redeem_code_batch_disabled",
            "disable_redeem_code_batch",
            "redeem_code_batch",
            &batch_id,
        )),
        crate::AdminWalletMutationOutcome::NotFound => Ok(build_redeem_code_not_found_response(
            "Redeem code batch not found",
        )),
        crate::AdminWalletMutationOutcome::Invalid(detail) => {
            Ok(build_admin_payments_bad_request_response(detail))
        }
        crate::AdminWalletMutationOutcome::Unavailable => {
            Ok(build_admin_payments_backend_unavailable_response(
                "Redeem code batch backend unavailable",
            ))
        }
    }
}

async fn build_admin_delete_redeem_code_batch_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let Some(batch_id) = parse_batch_id_from_suffix_path(request_context.path(), "/delete") else {
        return Ok(build_admin_payment_order_not_found_response());
    };
    match state
        .admin_delete_redeem_code_batch(
            &batch_id,
            admin_payment_operator_id(request_context).as_deref(),
        )
        .await?
    {
        crate::AdminWalletMutationOutcome::Applied(batch) => Ok(attach_admin_audit_response(
            Json(json!({ "batch": build_batch_payload(&batch) })).into_response(),
            "admin_redeem_code_batch_deleted",
            "delete_redeem_code_batch",
            "redeem_code_batch",
            &batch_id,
        )),
        crate::AdminWalletMutationOutcome::NotFound => Ok(build_redeem_code_not_found_response(
            "Redeem code batch not found",
        )),
        crate::AdminWalletMutationOutcome::Invalid(detail) => {
            Ok(build_admin_payments_bad_request_response(detail))
        }
        crate::AdminWalletMutationOutcome::Unavailable => {
            Ok(build_admin_payments_backend_unavailable_response(
                "Redeem code batch backend unavailable",
            ))
        }
    }
}

async fn build_admin_disable_redeem_code_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let Some(code_id) = parse_code_id_from_suffix_path(request_context.path(), "/disable") else {
        return Ok(build_admin_payment_order_not_found_response());
    };
    match state
        .admin_disable_redeem_code(
            &code_id,
            admin_payment_operator_id(request_context).as_deref(),
        )
        .await?
    {
        crate::AdminWalletMutationOutcome::Applied(code) => Ok(attach_admin_audit_response(
            Json(json!({ "code": build_code_payload(&code) })).into_response(),
            "admin_redeem_code_disabled",
            "disable_redeem_code",
            "redeem_code",
            &code_id,
        )),
        crate::AdminWalletMutationOutcome::NotFound => Ok(build_redeem_code_not_found_response(
            "Redeem code not found",
        )),
        crate::AdminWalletMutationOutcome::Invalid(detail) => {
            Ok(build_admin_payments_bad_request_response(detail))
        }
        crate::AdminWalletMutationOutcome::Unavailable => Ok(
            build_admin_payments_backend_unavailable_response("Redeem code backend unavailable"),
        ),
    }
}
