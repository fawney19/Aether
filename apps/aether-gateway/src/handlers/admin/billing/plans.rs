use super::{
    build_admin_billing_bad_request_response, build_admin_billing_conflict_response,
    build_admin_billing_data_unavailable_response, build_admin_billing_not_found_response,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::{GatewayError, LocalMutationOutcome};
use aether_data_contracts::repository::billing::{BillingPlanRecord, BillingPlanWriteInput};
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Deserialize)]
struct BillingPlanRequest {
    title: String,
    #[serde(default)]
    description: Option<String>,
    price_amount: f64,
    #[serde(default = "default_price_currency")]
    price_currency: String,
    duration_unit: String,
    duration_value: i64,
    #[serde(default = "default_enabled")]
    enabled: bool,
    #[serde(default)]
    sort_order: i64,
    #[serde(default = "default_max_active_per_user")]
    max_active_per_user: i64,
    #[serde(default = "default_purchase_limit_scope")]
    purchase_limit_scope: String,
    entitlements: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct BillingPlanStatusRequest {
    enabled: bool,
}

fn default_price_currency() -> String {
    "CNY".to_string()
}

fn default_enabled() -> bool {
    true
}

fn default_max_active_per_user() -> i64 {
    1
}

fn default_purchase_limit_scope() -> String {
    "active_period".to_string()
}

fn normalize_text(value: impl Into<String>, field: &str, max_len: usize) -> Result<String, String> {
    let value = value.into();
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{field} must not be empty"));
    }
    if trimmed.chars().count() > max_len {
        return Err(format!("{field} exceeds maximum length {max_len}"));
    }
    Ok(trimmed.to_string())
}

fn normalize_optional_text(
    value: Option<String>,
    max_len: usize,
) -> Result<Option<String>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.chars().count() > max_len {
        return Err(format!("field exceeds maximum length {max_len}"));
    }
    Ok(Some(trimmed.to_string()))
}

fn validate_entitlements(value: &serde_json::Value) -> Result<(), String> {
    let items = value
        .as_array()
        .ok_or_else(|| "entitlements must be an array".to_string())?;
    if items.is_empty() {
        return Err("entitlements must not be empty".to_string());
    }
    for item in items {
        let kind = item
            .get("type")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "entitlement.type is required".to_string())?;
        match kind {
            "wallet_credit" => {
                let amount = item
                    .get("amount_usd")
                    .and_then(|value| value.as_f64())
                    .ok_or_else(|| "wallet_credit.amount_usd is required".to_string())?;
                if !amount.is_finite() || amount <= 0.0 {
                    return Err("wallet_credit.amount_usd must be positive".to_string());
                }
                if let Some(bucket) = item.get("balance_bucket") {
                    let bucket = bucket.as_str().ok_or_else(|| {
                        "wallet_credit.balance_bucket must be a string".to_string()
                    })?;
                    if !matches!(bucket, "recharge" | "gift") {
                        return Err(
                            "wallet_credit.balance_bucket must be recharge/gift".to_string()
                        );
                    }
                }
            }
            "daily_quota" => {
                let amount = item
                    .get("daily_quota_usd")
                    .and_then(|value| value.as_f64())
                    .ok_or_else(|| "daily_quota.daily_quota_usd is required".to_string())?;
                if !amount.is_finite() || amount <= 0.0 {
                    return Err("daily_quota.daily_quota_usd must be positive".to_string());
                }
                if let Some(reset_timezone) = item.get("reset_timezone") {
                    let reset_timezone = reset_timezone
                        .as_str()
                        .ok_or_else(|| "daily_quota.reset_timezone must be a string".to_string())?
                        .trim();
                    if reset_timezone.is_empty() {
                        return Err("daily_quota.reset_timezone must not be empty".to_string());
                    }
                    reset_timezone.parse::<chrono_tz::Tz>().map_err(|_| {
                        "daily_quota.reset_timezone must be a valid timezone".to_string()
                    })?;
                }
                if let Some(carry_over) = item.get("carry_over") {
                    carry_over
                        .as_bool()
                        .ok_or_else(|| "daily_quota.carry_over must be a boolean".to_string())?;
                }
                let carry_over_days = if let Some(carry_over_days) = item.get("carry_over_days") {
                    let carry_over_days = carry_over_days.as_u64().ok_or_else(|| {
                        "daily_quota.carry_over_days must be an integer".to_string()
                    })?;
                    if !(1..=30).contains(&carry_over_days) {
                        return Err(
                            "daily_quota.carry_over_days must be between 1 and 30".to_string()
                        );
                    }
                    carry_over_days
                } else {
                    1
                };
                if let Some(limit_multiplier) = item.get("carry_over_limit_multiplier") {
                    let limit_multiplier = limit_multiplier.as_f64().ok_or_else(|| {
                        "daily_quota.carry_over_limit_multiplier must be a number".to_string()
                    })?;
                    let max_multiplier = carry_over_days as f64 + 1.0;
                    if !limit_multiplier.is_finite()
                        || limit_multiplier < 1.0
                        || limit_multiplier > max_multiplier
                    {
                        return Err(
                            "daily_quota.carry_over_limit_multiplier must be between 1 and carry_over_days+1"
                                .to_string(),
                        );
                    }
                }
                if item
                    .get("allow_wallet_overage")
                    .is_some_and(|value| !value.is_boolean())
                {
                    return Err("daily_quota.allow_wallet_overage must be a boolean".to_string());
                }
                if item
                    .get("self_service_daily_quota_reset")
                    .is_some_and(|value| !value.is_boolean())
                {
                    return Err(
                        "daily_quota.self_service_daily_quota_reset must be a boolean".to_string(),
                    );
                }
            }
            "membership_group" => {
                let groups = item
                    .get("grant_user_groups")
                    .and_then(|value| value.as_array())
                    .ok_or_else(|| "membership_group.grant_user_groups is required".to_string())?;
                if groups.is_empty() {
                    return Err("membership_group.grant_user_groups must not be empty".to_string());
                }
                for group in groups {
                    let group = group.as_str().ok_or_else(|| {
                        "membership_group.grant_user_groups must contain strings".to_string()
                    })?;
                    if group.trim().is_empty() {
                        return Err(
                            "membership_group.grant_user_groups must not contain empty values"
                                .to_string(),
                        );
                    }
                }
            }
            _ => return Err(format!("unsupported entitlement type: {kind}")),
        }
    }
    if !entitlements_include_package_rights(items) {
        return Err("套餐至少需要包含每日额度或会员分组；钱包充值请使用充值功能".to_string());
    }
    Ok(())
}

fn entitlements_include_package_rights(items: &[serde_json::Value]) -> bool {
    items.iter().any(|item| {
        matches!(
            item.get("type").and_then(|value| value.as_str()),
            Some("daily_quota" | "membership_group")
        )
    })
}

fn normalize_plan_input(payload: BillingPlanRequest) -> Result<BillingPlanWriteInput, String> {
    if !payload.price_amount.is_finite() || payload.price_amount <= 0.0 {
        return Err("price_amount must be positive".to_string());
    }
    if payload.duration_value <= 0 {
        return Err("duration_value must be positive".to_string());
    }
    if payload.max_active_per_user <= 0 {
        return Err("max_active_per_user must be positive".to_string());
    }
    let duration_unit = normalize_text(payload.duration_unit, "duration_unit", 32)?;
    if !matches!(duration_unit.as_str(), "day" | "month" | "year" | "custom") {
        return Err("duration_unit must be day/month/year/custom".to_string());
    }
    let purchase_limit_scope =
        normalize_text(payload.purchase_limit_scope, "purchase_limit_scope", 32)?;
    if !matches!(
        purchase_limit_scope.as_str(),
        "active_period" | "lifetime" | "unlimited"
    ) {
        return Err("purchase_limit_scope must be active_period/lifetime/unlimited".to_string());
    }
    validate_entitlements(&payload.entitlements)?;
    Ok(BillingPlanWriteInput {
        title: normalize_text(payload.title, "title", 128)?,
        description: normalize_optional_text(payload.description, 2048)?,
        price_amount: payload.price_amount,
        price_currency: normalize_text(payload.price_currency, "price_currency", 16)?,
        duration_unit,
        duration_value: payload.duration_value,
        enabled: payload.enabled,
        sort_order: payload.sort_order,
        max_active_per_user: payload.max_active_per_user,
        purchase_limit_scope,
        entitlements_json: payload.entitlements,
    })
}

pub(crate) fn billing_plan_payload(record: &BillingPlanRecord) -> serde_json::Value {
    json!({
        "id": record.id,
        "title": record.title,
        "description": record.description,
        "price_amount": record.price_amount,
        "price_currency": record.price_currency,
        "duration_unit": record.duration_unit,
        "duration_value": record.duration_value,
        "enabled": record.enabled,
        "sort_order": record.sort_order,
        "max_active_per_user": record.max_active_per_user,
        "purchase_limit_scope": record.purchase_limit_scope,
        "entitlements": record.entitlements_json,
        "created_at": record.created_at_unix_secs,
        "updated_at": record.updated_at_unix_secs,
    })
}

fn plan_id_from_path(path: &str, suffix: Option<&str>) -> Option<String> {
    let trimmed = path.trim_end_matches('/');
    let rest = trimmed.strip_prefix("/api/admin/billing/plans/")?;
    let id = if let Some(suffix) = suffix {
        rest.strip_suffix(suffix)?.trim_end_matches('/')
    } else {
        rest
    };
    if id.is_empty() || id.contains('/') {
        None
    } else {
        Some(id.to_string())
    }
}

pub(super) async fn maybe_build_local_admin_billing_plans_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let path = request_context.path().trim_end_matches('/');
    match (request_context.method(), path) {
        (&http::Method::GET, "/api/admin/billing/plans") => {
            let items = state
                .app()
                .list_billing_plans(true)
                .await?
                .unwrap_or_default()
                .iter()
                .map(billing_plan_payload)
                .collect::<Vec<_>>();
            Ok(Some(
                Json(json!({"items": items, "total": items.len()})).into_response(),
            ))
        }
        (&http::Method::POST, "/api/admin/billing/plans") => {
            let Some(body) = request_body else {
                return Ok(Some(build_admin_billing_bad_request_response("缺少请求体")));
            };
            let payload = match serde_json::from_slice::<BillingPlanRequest>(body) {
                Ok(value) => value,
                Err(_) => {
                    return Ok(Some(build_admin_billing_bad_request_response(
                        "输入验证失败",
                    )))
                }
            };
            let input = match normalize_plan_input(payload) {
                Ok(value) => value,
                Err(detail) => return Ok(Some(build_admin_billing_bad_request_response(detail))),
            };
            match state.app().create_billing_plan(&input).await? {
                LocalMutationOutcome::Applied(record) => {
                    Ok(Some(Json(billing_plan_payload(&record)).into_response()))
                }
                _ => Ok(Some(build_admin_billing_data_unavailable_response())),
            }
        }
        _ if request_context.method() == http::Method::PUT
            && path.starts_with("/api/admin/billing/plans/") =>
        {
            let Some(plan_id) = plan_id_from_path(path, None) else {
                return Ok(None);
            };
            let Some(body) = request_body else {
                return Ok(Some(build_admin_billing_bad_request_response("缺少请求体")));
            };
            let payload = match serde_json::from_slice::<BillingPlanRequest>(body) {
                Ok(value) => value,
                Err(_) => {
                    return Ok(Some(build_admin_billing_bad_request_response(
                        "输入验证失败",
                    )))
                }
            };
            let input = match normalize_plan_input(payload) {
                Ok(value) => value,
                Err(detail) => return Ok(Some(build_admin_billing_bad_request_response(detail))),
            };
            match state.app().update_billing_plan(&plan_id, &input).await? {
                LocalMutationOutcome::Applied(record) => {
                    Ok(Some(Json(billing_plan_payload(&record)).into_response()))
                }
                LocalMutationOutcome::NotFound => Ok(Some(build_admin_billing_not_found_response(
                    "Billing plan not found",
                ))),
                _ => Ok(Some(build_admin_billing_data_unavailable_response())),
            }
        }
        _ if request_context.method() == http::Method::PATCH
            && path.ends_with("/status")
            && path.starts_with("/api/admin/billing/plans/") =>
        {
            let Some(plan_id) = plan_id_from_path(path, Some("/status")) else {
                return Ok(None);
            };
            let Some(body) = request_body else {
                return Ok(Some(build_admin_billing_bad_request_response("缺少请求体")));
            };
            let payload = match serde_json::from_slice::<BillingPlanStatusRequest>(body) {
                Ok(value) => value,
                Err(_) => {
                    return Ok(Some(build_admin_billing_bad_request_response(
                        "输入验证失败",
                    )))
                }
            };
            match state
                .app()
                .set_billing_plan_enabled(&plan_id, payload.enabled)
                .await?
            {
                LocalMutationOutcome::Applied(record) => {
                    Ok(Some(Json(billing_plan_payload(&record)).into_response()))
                }
                LocalMutationOutcome::NotFound => Ok(Some(build_admin_billing_not_found_response(
                    "Billing plan not found",
                ))),
                _ => Ok(Some(build_admin_billing_data_unavailable_response())),
            }
        }
        _ if request_context.method() == http::Method::DELETE
            && path.starts_with("/api/admin/billing/plans/") =>
        {
            let Some(plan_id) = plan_id_from_path(path, None) else {
                return Ok(None);
            };
            match state.app().delete_billing_plan(&plan_id).await? {
                LocalMutationOutcome::Applied(()) => {
                    Ok(Some(http::StatusCode::NO_CONTENT.into_response()))
                }
                LocalMutationOutcome::NotFound => Ok(Some(build_admin_billing_not_found_response(
                    "Billing plan not found",
                ))),
                LocalMutationOutcome::Invalid(detail) => {
                    Ok(Some(build_admin_billing_conflict_response(detail)))
                }
                LocalMutationOutcome::Unavailable => {
                    Ok(Some(build_admin_billing_data_unavailable_response()))
                }
            }
        }
        _ => Ok(None),
    }
}
