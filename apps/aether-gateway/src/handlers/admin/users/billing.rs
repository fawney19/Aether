use super::{build_admin_users_bad_request_response, build_admin_users_data_unavailable_response};
use crate::handlers::admin::billing::admin_payment_gateway_response_projection;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::{attach_admin_audit_response, unix_secs_to_rfc3339};
use crate::handlers::shared::unix_ms_to_rfc3339;
use crate::GatewayError;
use aether_data_contracts::repository::billing::{BillingPlanRecord, UserPlanEntitlementRecord};
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use chrono::Utc;
use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Debug, Deserialize)]
struct AdminGrantUserPlanRequest {
    plan_id: String,
    #[serde(default)]
    reason: Option<String>,
}

fn admin_user_id_from_billing_path(request_path: &str, suffix: &str) -> Option<String> {
    let trimmed = request_path.trim_end_matches('/');
    let rest = trimmed.strip_prefix("/api/admin/users/")?;
    let user_id = rest.strip_suffix(suffix)?.trim_end_matches('/');
    if user_id.is_empty() || user_id.contains('/') {
        None
    } else {
        Some(user_id.to_string())
    }
}

fn admin_user_entitlement_ids_from_path(request_path: &str) -> Option<(String, String)> {
    let rest = request_path
        .trim_end_matches('/')
        .strip_prefix("/api/admin/users/")?;
    let mut parts = rest.split('/');
    let user_id = parts.next()?.trim();
    if parts.next()? != "billing" || parts.next()? != "entitlements" {
        return None;
    }
    let entitlement_id = parts.next()?.trim();
    if user_id.is_empty() || entitlement_id.is_empty() || parts.next().is_some() {
        return None;
    }
    Some((user_id.to_string(), entitlement_id.to_string()))
}

fn admin_user_billing_operator_id(request_context: &AdminRequestContext<'_>) -> Option<String> {
    request_context
        .decision()
        .and_then(|decision| decision.admin_principal.as_ref())
        .map(|principal| principal.user_id.clone())
}

fn normalize_admin_grant_reason(value: Option<String>) -> Result<Option<String>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value.chars().count() > 512 {
        return Err("reason exceeds maximum length 512".to_string());
    }
    Ok(Some(value.to_string()))
}

fn admin_plan_grant_order_no(now: chrono::DateTime<Utc>) -> String {
    format!(
        "pg_{}_{}",
        now.format("%Y%m%d%H%M%S%6f"),
        &Uuid::new_v4().simple().to_string()[..12]
    )
}

fn billing_plan_payload(record: &BillingPlanRecord) -> serde_json::Value {
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

fn billing_plan_snapshot(record: &BillingPlanRecord) -> serde_json::Value {
    json!({
        "id": record.id,
        "title": record.title,
        "description": record.description,
        "price_amount": record.price_amount,
        "price_currency": record.price_currency,
        "duration_unit": record.duration_unit,
        "duration_value": record.duration_value,
        "max_active_per_user": record.max_active_per_user,
        "purchase_limit_scope": record.purchase_limit_scope,
        "entitlements": record.entitlements_json,
    })
}

fn plan_has_package_rights(record: &BillingPlanRecord) -> bool {
    record.entitlements_json.as_array().is_some_and(|items| {
        items.iter().any(|item| {
            matches!(
                item.get("type").and_then(|value| value.as_str()),
                Some("daily_quota" | "membership_group" | "usage_policy")
            )
        })
    })
}

fn admin_payment_order_payload(record: &crate::AdminWalletPaymentOrderRecord) -> serde_json::Value {
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
        "status": record.status,
        "order_kind": "plan_purchase",
        "created_at": unix_ms_to_rfc3339(record.created_at_unix_ms),
        "paid_at": record.paid_at_unix_secs.and_then(unix_secs_to_rfc3339),
        "credited_at": record.credited_at_unix_secs.and_then(unix_secs_to_rfc3339),
        "expires_at": record.expires_at_unix_secs.and_then(unix_secs_to_rfc3339),
    })
}

fn entitlement_payload(
    record: &UserPlanEntitlementRecord,
    plan: Option<&BillingPlanRecord>,
    now_unix_secs: u64,
) -> serde_json::Value {
    json!({
        "id": record.id,
        "user_id": record.user_id,
        "plan_id": record.plan_id,
        "payment_order_id": record.payment_order_id,
        "status": record.status,
        "starts_at": unix_secs_to_rfc3339(record.starts_at_unix_secs),
        "expires_at": unix_secs_to_rfc3339(record.expires_at_unix_secs),
        "entitlements": record.entitlements_snapshot,
        "active": record.status == "active"
            && record.starts_at_unix_secs <= now_unix_secs
            && record.expires_at_unix_secs > now_unix_secs,
        "plan_title": plan.map(|plan| plan.title.clone()),
        "plan": plan.map(billing_plan_payload),
        "created_at": unix_secs_to_rfc3339(record.created_at_unix_secs),
        "updated_at": unix_secs_to_rfc3339(record.updated_at_unix_secs),
    })
}

async fn load_admin_user_entitlements_payload(
    state: &AdminAppState<'_>,
    user_id: &str,
) -> Result<Option<serde_json::Value>, GatewayError> {
    let entitlements = match state.app().list_user_plan_entitlements(user_id).await? {
        Some(value) => value,
        None => return Ok(None),
    };
    let plans = state
        .app()
        .list_billing_plans(true)
        .await?
        .unwrap_or_default()
        .into_iter()
        .map(|plan| (plan.id.clone(), plan))
        .collect::<BTreeMap<_, _>>();
    let now = Utc::now().timestamp().max(0) as u64;
    let items = entitlements
        .iter()
        .map(|record| entitlement_payload(record, plans.get(&record.plan_id), now))
        .collect::<Vec<_>>();
    Ok(Some(json!({"items": items, "total": items.len()})))
}

pub(in super::super) async fn build_admin_list_user_billing_entitlements_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let Some(user_id) =
        admin_user_id_from_billing_path(request_context.path(), "/billing/entitlements")
    else {
        return Ok(build_admin_users_bad_request_response("缺少 user_id"));
    };
    if state.find_user_auth_by_id(&user_id).await?.is_none() {
        return Ok((
            http::StatusCode::NOT_FOUND,
            Json(json!({ "detail": "用户不存在" })),
        )
            .into_response());
    }
    match load_admin_user_entitlements_payload(state, &user_id).await? {
        Some(payload) => Ok(Json(payload).into_response()),
        None => Ok(build_admin_users_data_unavailable_response()),
    }
}

pub(in super::super) async fn build_admin_revoke_user_billing_entitlement_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let Some((user_id, entitlement_id)) =
        admin_user_entitlement_ids_from_path(request_context.path())
    else {
        return Ok(build_admin_users_bad_request_response("缺少套餐权益 ID"));
    };
    if state.find_user_auth_by_id(&user_id).await?.is_none() {
        return Ok((
            http::StatusCode::NOT_FOUND,
            Json(json!({ "detail": "用户不存在" })),
        )
            .into_response());
    }
    match state
        .app()
        .revoke_user_plan_entitlement(&user_id, &entitlement_id)
        .await?
    {
        crate::LocalMutationOutcome::Applied(()) => {}
        crate::LocalMutationOutcome::NotFound => {
            return Ok((
                http::StatusCode::NOT_FOUND,
                Json(json!({ "detail": "套餐权益不存在或已失效" })),
            )
                .into_response());
        }
        crate::LocalMutationOutcome::Invalid(detail) => {
            return Ok(build_admin_users_bad_request_response(detail));
        }
        crate::LocalMutationOutcome::Unavailable => {
            return Ok(build_admin_users_data_unavailable_response());
        }
    }
    let entitlements = match load_admin_user_entitlements_payload(state, &user_id).await? {
        Some(value) => value,
        None => return Ok(build_admin_users_data_unavailable_response()),
    };
    Ok(attach_admin_audit_response(
        Json(json!({
            "items": entitlements["items"].clone(),
            "entitlements": entitlements["items"].clone(),
            "total": entitlements["total"].clone(),
        }))
        .into_response(),
        "admin_user_plan_revoked",
        "revoke_user_billing_entitlement",
        "user_plan_entitlement",
        &entitlement_id,
    ))
}

pub(in super::super) async fn build_admin_grant_user_billing_plan_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Response<Body>, GatewayError> {
    let Some(user_id) =
        admin_user_id_from_billing_path(request_context.path(), "/billing/grant-plan")
    else {
        return Ok(build_admin_users_bad_request_response("缺少 user_id"));
    };
    if state.find_user_auth_by_id(&user_id).await?.is_none() {
        return Ok((
            http::StatusCode::NOT_FOUND,
            Json(json!({ "detail": "用户不存在" })),
        )
            .into_response());
    }
    let Some(body) = request_body else {
        return Ok(build_admin_users_bad_request_response("缺少请求体"));
    };
    let payload = match serde_json::from_slice::<AdminGrantUserPlanRequest>(body) {
        Ok(value) => value,
        Err(_) => return Ok(build_admin_users_bad_request_response("输入验证失败")),
    };
    let plan_id = payload.plan_id.trim();
    if plan_id.is_empty() {
        return Ok(build_admin_users_bad_request_response("plan_id 不能为空"));
    }
    let reason = match normalize_admin_grant_reason(payload.reason) {
        Ok(value) => value,
        Err(detail) => return Ok(build_admin_users_bad_request_response(detail)),
    };
    let Some(plan) = state.app().find_billing_plan(plan_id).await? else {
        return Ok((
            http::StatusCode::NOT_FOUND,
            Json(json!({ "detail": "套餐不存在" })),
        )
            .into_response());
    };
    if !plan_has_package_rights(&plan) {
        return Ok(build_admin_users_bad_request_response(
            "余额包已移除，请使用钱包充值功能",
        ));
    }

    let now = Utc::now();
    let order_no = admin_plan_grant_order_no(now);
    let operator_id = admin_user_billing_operator_id(request_context);
    let gateway_response = json!({
        "source": "admin_grant",
        "operator_id": operator_id.as_deref(),
        "reason": reason,
        "granted_at": now.to_rfc3339(),
    });
    let outcome = match state
        .app()
        .create_plan_purchase_order(
            aether_data::repository::wallet::CreatePlanPurchaseOrderInput {
                preferred_wallet_id: None,
                user_id: user_id.clone(),
                amount_usd: 0.0,
                pay_amount: 0.0,
                // Manual grants have no provider settlement. Keep the order
                // currency stable and independent of the plan's display
                // currency (the amount is always zero).
                pay_currency: "USD".to_string(),
                exchange_rate: 1.0,
                payment_method: "admin_grant".to_string(),
                payment_provider: Some("admin".to_string()),
                payment_channel: Some("manual".to_string()),
                gateway_order_id: order_no.clone(),
                gateway_response,
                order_no: order_no.clone(),
                product_id: plan.id.clone(),
                product_snapshot: billing_plan_snapshot(&plan),
                expires_at_unix_secs: (now + chrono::Duration::minutes(30)).timestamp().max(0)
                    as u64,
            },
        )
        .await?
    {
        Some(value) => value,
        None => return Ok(build_admin_users_data_unavailable_response()),
    };
    let order = match outcome {
        aether_data::repository::wallet::CreatePlanPurchaseOrderOutcome::Created(order) => order,
        aether_data::repository::wallet::CreatePlanPurchaseOrderOutcome::WalletInactive => {
            return Ok(build_admin_users_bad_request_response(
                "wallet is not active",
            ));
        }
        aether_data::repository::wallet::CreatePlanPurchaseOrderOutcome::ActivePlanLimitReached => {
            return Ok((
                http::StatusCode::CONFLICT,
                Json(json!({ "detail": "套餐购买限制已达到上限" })),
            )
                .into_response());
        }
    };

    let credit_result = state
        .admin_credit_payment_order(
            &order.id,
            Some(&order_no),
            Some(0.0),
            Some("USD"),
            Some(1.0),
            Some(json!({ "admin_grant": true })),
            operator_id.as_deref(),
        )
        .await?;
    let (credited_order, credited) = match credit_result {
        crate::AdminWalletMutationOutcome::Applied(value) => value,
        crate::AdminWalletMutationOutcome::NotFound => {
            return Ok(build_admin_users_data_unavailable_response());
        }
        crate::AdminWalletMutationOutcome::Invalid(detail) => {
            return Ok((
                http::StatusCode::CONFLICT,
                Json(json!({ "detail": detail })),
            )
                .into_response());
        }
        crate::AdminWalletMutationOutcome::Unavailable => {
            return Ok(build_admin_users_data_unavailable_response());
        }
    };
    let entitlements = match load_admin_user_entitlements_payload(state, &user_id).await? {
        Some(value) => value,
        None => return Ok(build_admin_users_data_unavailable_response()),
    };
    Ok(attach_admin_audit_response(
        Json(json!({
            "order": admin_payment_order_payload(&credited_order),
            "credited": credited,
            "items": entitlements["items"].clone(),
            "entitlements": entitlements["items"].clone(),
            "total": entitlements["total"].clone(),
        }))
        .into_response(),
        "admin_user_plan_granted",
        "grant_user_billing_plan",
        "user",
        &user_id,
    ))
}
