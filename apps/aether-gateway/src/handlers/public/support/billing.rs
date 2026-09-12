use super::support_payment::payment_epay::{
    build_epay_checkout_url, epay_callback_base_url, load_epay_config, resolve_epay_channel,
    EpayCheckoutInput,
};
use super::{
    build_auth_error_response, build_auth_json_response, mark_sensitive_response_no_store,
    prepare_billing_gateway_response_for_storage, resolve_authenticated_local_user,
    resolve_direct_gateway_channel, sanitize_wallet_gateway_response, unix_secs_to_rfc3339,
    wallet_payment_instructions_from_checkout, wallet_payment_instructions_from_stored, AppState,
    GatewayPublicRequestContext,
};
use crate::handlers::shared::normalize_payment_currency;
use crate::handlers::shared::{
    close_direct_gateway_checkout, create_alipay_direct_checkout, create_stripe_direct_checkout,
    create_wxpay_direct_checkout, DirectPaymentCheckoutError, DirectPaymentCheckoutInput,
};
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tracing::warn;
use uuid::Uuid;

use aether_data::repository::wallet::stored_timestamp_unix_secs;

const BILLING_STORAGE_UNAVAILABLE_DETAIL: &str = "套餐后端暂不可用";
const BILLING_ORDER_IDENTITY_BUCKET_SECS: u64 = 30 * 60;
const PLAN_PAYMENT_AMOUNT_EPSILON: f64 = 0.000_001;

#[derive(Debug, Deserialize, Default)]
struct BillingPlanCheckoutRequest {
    #[serde(default)]
    payment_method: Option<String>,
    #[serde(default)]
    payment_provider: Option<String>,
    #[serde(default)]
    payment_channel: Option<String>,
}

#[derive(Debug, Clone)]
struct NormalizedBillingPlanCheckoutRequest {
    payment_method: String,
    payment_provider: String,
    payment_channel: Option<String>,
}

fn billing_storage_unavailable_response() -> Response<Body> {
    build_auth_error_response(
        http::StatusCode::SERVICE_UNAVAILABLE,
        BILLING_STORAGE_UNAVAILABLE_DETAIL,
        false,
    )
}

fn normalize_optional_checkout_string(
    value: Option<String>,
    max_len: usize,
) -> Result<Option<String>, &'static str> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim().to_ascii_lowercase();
    if value.is_empty() {
        return Ok(None);
    }
    if value.chars().count() > max_len {
        return Err("输入验证失败");
    }
    Ok(Some(value))
}

fn normalize_checkout_request(
    payload: BillingPlanCheckoutRequest,
) -> Result<NormalizedBillingPlanCheckoutRequest, &'static str> {
    let requested_provider = normalize_optional_checkout_string(payload.payment_provider, 30)?;
    let requested_method = normalize_optional_checkout_string(payload.payment_method, 30)?;
    let payment_provider = requested_provider
        .clone()
        .or(requested_method.clone())
        .unwrap_or_else(|| "epay".to_string());
    if !matches!(
        payment_provider.as_str(),
        "epay" | "alipay" | "wxpay" | "stripe"
    ) {
        return Err("unsupported payment_provider");
    }
    let payment_method = requested_method.unwrap_or_else(|| payment_provider.clone());
    let payment_channel = normalize_optional_checkout_string(payload.payment_channel, 30)?;
    if payment_provider == "epay" {
        if !matches!(payment_method.as_str(), "epay" | "alipay" | "wxpay") {
            return Err("payment_method 与 payment_provider 不匹配");
        }
        if payment_method != "epay"
            && payment_channel
                .as_deref()
                .is_some_and(|channel| channel != payment_method)
        {
            return Err("payment_method 与 payment_channel 不匹配");
        }
    } else if payment_method != payment_provider {
        return Err("payment_method 与 payment_provider 不匹配");
    }
    // Only the legacy EPay shorthand uses `payment_method` as a channel.
    // Direct providers have their own channel allowlists (for example
    // `native`/`h5` for WxPay and `card`/`link` for Stripe); passing the
    // provider name through as a channel would make an otherwise valid
    // request fail resolution before the configured default can be selected.
    let payment_channel = if payment_provider == "epay" && payment_method != "epay" {
        payment_channel.or_else(|| Some(payment_method.clone()))
    } else {
        payment_channel
    };
    Ok(NormalizedBillingPlanCheckoutRequest {
        payment_method: payment_provider.clone(),
        payment_provider,
        payment_channel,
    })
}

fn plan_id_from_checkout_path(path: &str) -> Option<String> {
    let trimmed = path.trim_end_matches('/');
    let rest = trimmed.strip_prefix("/api/billing/plans/")?;
    let plan_id = rest.strip_suffix("/checkout")?.trim_matches('/');
    if plan_id.is_empty() || plan_id.contains('/') {
        None
    } else {
        Some(plan_id.to_string())
    }
}

/// Derive a stable merchant order number for one checkout identity.  The
/// short time bucket lets a later attempt create a fresh order after the
/// original 30-minute pending window expires, while retries in the same
/// window reuse the provider-side idempotency key (Stripe) and merchant order
/// number (EPay/Alipay/WxPay).
fn billing_order_no(
    user_id: &str,
    plan_id: &str,
    payment_method: &str,
    payment_provider: &str,
    payment_channel: &str,
    amount_usd: f64,
    pay_amount: f64,
    pay_currency: &str,
    exchange_rate: f64,
    now: chrono::DateTime<chrono::Utc>,
) -> String {
    billing_order_no_with_salt(
        user_id,
        plan_id,
        payment_method,
        payment_provider,
        payment_channel,
        amount_usd,
        pay_amount,
        pay_currency,
        exchange_rate,
        None,
        now,
    )
}

fn billing_order_no_with_salt(
    user_id: &str,
    plan_id: &str,
    payment_method: &str,
    payment_provider: &str,
    payment_channel: &str,
    amount_usd: f64,
    pay_amount: f64,
    pay_currency: &str,
    exchange_rate: f64,
    salt: Option<&str>,
    now: chrono::DateTime<chrono::Utc>,
) -> String {
    let bucket = now.timestamp().max(0) as u64 / BILLING_ORDER_IDENTITY_BUCKET_SECS;
    let identity = json!({
        "version": 2,
        "bucket": bucket,
        "user_id": user_id,
        "plan_id": plan_id,
        "payment_method": payment_method.trim().to_ascii_lowercase(),
        "payment_provider": payment_provider.trim().to_ascii_lowercase(),
        "payment_channel": payment_channel.trim().to_ascii_lowercase(),
        "amount_usd": format!("{amount_usd:.8}"),
        "pay_amount": format!("{pay_amount:.8}"),
        "pay_currency": pay_currency.trim().to_ascii_lowercase(),
        "exchange_rate": format!("{exchange_rate:.8}"),
        "salt": salt,
    })
    .to_string();
    let digest = format!("{:x}", Sha256::digest(identity.as_bytes()));
    // 3-byte prefix + 56 hex characters stays below every adapter's 64-byte
    // order number limit while retaining ample collision resistance.
    format!("pp_{}", &digest[..56])
}

fn billing_order_no_with_nonce(
    user_id: &str,
    plan_id: &str,
    payment_method: &str,
    payment_provider: &str,
    payment_channel: &str,
    amount_usd: f64,
    pay_amount: f64,
    pay_currency: &str,
    exchange_rate: f64,
    nonce: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> String {
    billing_order_no_with_salt(
        user_id,
        plan_id,
        payment_method,
        payment_provider,
        payment_channel,
        amount_usd,
        pay_amount,
        pay_currency,
        exchange_rate,
        Some(nonce),
        now,
    )
}

fn payment_amounts_match(expected: f64, stored: f64) -> bool {
    expected.is_finite()
        && stored.is_finite()
        && (expected - stored).abs() <= PLAN_PAYMENT_AMOUNT_EPSILON
}

fn plan_order_metadata_string<'a>(
    order: &'a aether_data::repository::wallet::StoredAdminPaymentOrder,
    key: &str,
) -> Option<&'a str> {
    order
        .gateway_response
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|object| object.get(key))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

/// Check every payment identity component before replaying a pending plan
/// order.  The repository query deliberately stays broad for compatibility
/// with older rows; this boundary check prevents a payment-method, channel,
/// currency, rate, or amount change from receiving stale instructions.
fn plan_purchase_order_matches_checkout(
    order: &aether_data::repository::wallet::StoredAdminPaymentOrder,
    plan_id: &str,
    payment_method: &str,
    payment_provider: &str,
    payment_channel: &str,
    amount_usd: f64,
    pay_amount: f64,
    pay_currency: &str,
    exchange_rate: f64,
) -> bool {
    if !order.status.eq_ignore_ascii_case("pending")
        || !order
            .expires_at_unix_secs
            .is_some_and(|expires_at| expires_at > Utc::now().timestamp().max(0) as u64)
        || !payment_amounts_match(amount_usd, order.amount_usd)
        || !payment_amounts_match(pay_amount, order.pay_amount.unwrap_or(f64::NAN))
        || !order
            .pay_currency
            .as_deref()
            .is_some_and(|stored| stored.eq_ignore_ascii_case(pay_currency.trim()))
        || !payment_amounts_match(exchange_rate, order.exchange_rate.unwrap_or(f64::NAN))
    {
        return false;
    }

    let stored_provider = plan_order_metadata_string(order, "gateway")
        .or_else(|| plan_order_metadata_string(order, "payment_provider"));
    let provider_matches = stored_provider
        .map(|stored| stored.eq_ignore_ascii_case(payment_provider.trim()))
        .unwrap_or_else(|| {
            order
                .payment_method
                .eq_ignore_ascii_case(payment_provider.trim())
        });

    let stored_channel = plan_order_metadata_string(order, "payment_channel");
    let channel_matches = stored_channel
        .map(|stored| stored.eq_ignore_ascii_case(payment_channel.trim()))
        // Pre-provider plan rows may have stored the EPay channel as the
        // payment method. Accept that legacy shape only when the requested
        // provider is EPay and the channel still agrees exactly.
        .unwrap_or_else(|| {
            payment_provider.eq_ignore_ascii_case("epay")
                && order
                    .payment_method
                    .eq_ignore_ascii_case(payment_channel.trim())
        });

    let method_matches = order
        .payment_method
        .eq_ignore_ascii_case(payment_method.trim())
        || (payment_provider.eq_ignore_ascii_case("epay")
            && payment_method.eq_ignore_ascii_case("epay")
            && order
                .payment_method
                .eq_ignore_ascii_case(payment_channel.trim()));

    let product_matches = plan_order_metadata_string(order, "product_id")
        .map(|stored| stored == plan_id)
        .unwrap_or(true);

    provider_matches && channel_matches && method_matches && product_matches
}

async fn find_matching_pending_plan_purchase_order(
    state: &AppState,
    user_id: &str,
    plan_id: &str,
    payment_method: &str,
    payment_provider: &str,
    payment_channel: &str,
    amount_usd: f64,
    pay_amount: f64,
    pay_currency: &str,
    exchange_rate: f64,
) -> Result<Option<aether_data::repository::wallet::StoredAdminPaymentOrder>, String> {
    let order = state
        .find_pending_plan_purchase_order_by_user_id(user_id, plan_id)
        .await
        .map_err(|err| format!("pending billing checkout lookup failed: {err:?}"))?;
    Ok(order.filter(|order| {
        plan_purchase_order_matches_checkout(
            order,
            plan_id,
            payment_method,
            payment_provider,
            payment_channel,
            amount_usd,
            pay_amount,
            pay_currency,
            exchange_rate,
        )
    }))
}

enum PlanOrderNoChoice {
    Reuse(aether_data::repository::wallet::StoredAdminPaymentOrder),
    Fresh(String),
}

/// Reserve a merchant order number identity before contacting a provider.
/// Deterministic retries keep their original number, while a terminal (or
/// unrelated) row occupying that number receives a fresh high-entropy number.
async fn choose_plan_order_no(
    state: &AppState,
    user_id: &str,
    plan_id: &str,
    payment_method: &str,
    payment_provider: &str,
    payment_channel: &str,
    amount_usd: f64,
    pay_amount: f64,
    pay_currency: &str,
    exchange_rate: f64,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<PlanOrderNoChoice, String> {
    let base = billing_order_no(
        user_id,
        plan_id,
        payment_method,
        payment_provider,
        payment_channel,
        amount_usd,
        pay_amount,
        pay_currency,
        exchange_rate,
        now,
    );
    let existing = state
        .find_payment_order_by_order_no(&base)
        .await
        .map_err(|err| format!("billing order number lookup failed: {err:?}"))?;
    if let Some(existing) = existing {
        if existing.user_id.as_deref() == Some(user_id)
            && plan_purchase_order_matches_checkout(
                &existing,
                plan_id,
                payment_method,
                payment_provider,
                payment_channel,
                amount_usd,
                pay_amount,
                pay_currency,
                exchange_rate,
            )
        {
            return Ok(PlanOrderNoChoice::Reuse(existing));
        }

        // A deterministic identity may already belong to a completed order.
        // Keep trying bounded random identities; the database uniqueness check
        // remains the final authority for a concurrent writer.
        for _ in 0..4 {
            let nonce = Uuid::new_v4().simple().to_string();
            let candidate = billing_order_no_with_nonce(
                user_id,
                plan_id,
                payment_method,
                payment_provider,
                payment_channel,
                amount_usd,
                pay_amount,
                pay_currency,
                exchange_rate,
                &nonce,
                now,
            );
            if state
                .find_payment_order_by_order_no(&candidate)
                .await
                .map_err(|err| format!("billing order number lookup failed: {err:?}"))?
                .is_none()
            {
                return Ok(PlanOrderNoChoice::Fresh(candidate));
            }
        }
        return Err("无法生成唯一支付订单号，请稍后重试".to_string());
    }
    Ok(PlanOrderNoChoice::Fresh(base))
}

enum PlanCreateFailureResolution {
    Replay(Response<Body>),
    Missing,
    Occupied,
}

async fn fresh_plan_order_no(
    state: &AppState,
    user_id: &str,
    plan_id: &str,
    payment_method: &str,
    payment_provider: &str,
    payment_channel: &str,
    amount_usd: f64,
    pay_amount: f64,
    pay_currency: &str,
    exchange_rate: f64,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<String, String> {
    for _ in 0..4 {
        let nonce = Uuid::new_v4().simple().to_string();
        let candidate = billing_order_no_with_nonce(
            user_id,
            plan_id,
            payment_method,
            payment_provider,
            payment_channel,
            amount_usd,
            pay_amount,
            pay_currency,
            exchange_rate,
            &nonce,
            now,
        );
        if state
            .find_payment_order_by_order_no(&candidate)
            .await
            .map_err(|err| format!("billing order number lookup failed: {err:?}"))?
            .is_none()
        {
            return Ok(candidate);
        }
    }
    Err("无法生成唯一支付订单号，请稍后重试".to_string())
}

async fn resolve_plan_order_after_create_failure(
    state: &AppState,
    user_id: &str,
    plan: &aether_data_contracts::repository::billing::BillingPlanRecord,
    provider: &str,
    payment_method: &str,
    payment_channel: &str,
    amount_usd: f64,
    pay_amount: f64,
    pay_currency: &str,
    exchange_rate: f64,
    order_no: &str,
) -> Result<PlanCreateFailureResolution, String> {
    let Some(order) = state
        .find_payment_order_by_order_no(order_no)
        .await
        .map_err(|err| format!("billing order reconciliation failed: {err:?}"))?
    else {
        return Ok(PlanCreateFailureResolution::Missing);
    };
    if order.user_id.as_deref() != Some(user_id)
        || !plan_purchase_order_matches_checkout(
            &order,
            &plan.id,
            payment_method,
            provider,
            payment_channel,
            amount_usd,
            pay_amount,
            pay_currency,
            exchange_rate,
        )
    {
        return Ok(PlanCreateFailureResolution::Occupied);
    }
    Ok(PlanCreateFailureResolution::Replay(
        plan_checkout_replay_response(state, &order, plan).await,
    ))
}

async fn close_abandoned_direct_plan_checkout(
    state: &AppState,
    provider: &str,
    order_no: &str,
    gateway_order_id: Option<&str>,
    failure_stage: &'static str,
) {
    match close_direct_gateway_checkout(state, provider, order_no, gateway_order_id).await {
        Ok(Some(_)) => warn!(
            payment_provider = provider,
            order_no,
            failure_stage,
            "closed direct gateway checkout after local plan checkout failure"
        ),
        Ok(None) => warn!(
            payment_provider = provider,
            order_no, failure_stage, "direct gateway does not support checkout compensation"
        ),
        Err(error) => warn!(
            payment_provider = provider,
            order_no,
            failure_stage,
            error,
            "failed to close direct gateway checkout after local plan checkout failure"
        ),
    }
}

fn billing_plan_payload(
    record: &aether_data_contracts::repository::billing::BillingPlanRecord,
) -> serde_json::Value {
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

fn billing_plan_snapshot(
    record: &aether_data_contracts::repository::billing::BillingPlanRecord,
) -> serde_json::Value {
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

fn plan_has_package_rights(
    record: &aether_data_contracts::repository::billing::BillingPlanRecord,
) -> bool {
    record.entitlements_json.as_array().is_some_and(|items| {
        items.iter().any(|item| {
            matches!(
                item.get("type").and_then(|value| value.as_str()),
                Some("daily_quota" | "membership_group" | "usage_policy")
            )
        })
    })
}

fn payment_order_payload(
    record: &aether_data::repository::wallet::StoredAdminPaymentOrder,
    plan: &aether_data_contracts::repository::billing::BillingPlanRecord,
) -> serde_json::Value {
    // A gateway response is a live checkout capability, not durable order
    // history. Once the order is paid, terminal, or expired, suppress URLs,
    // form parameters, and provider metadata from the public payload.
    let gateway_response = if record.status.eq_ignore_ascii_case("pending")
        && record
            .expires_at_unix_secs
            .is_some_and(|expires_at| expires_at > Utc::now().timestamp().max(0) as u64)
    {
        record.gateway_response.clone()
    } else {
        None
    };
    json!({
        "id": record.id,
        "order_no": record.order_no,
        "wallet_id": record.wallet_id,
        "user_id": record.user_id,
        "amount_usd": record.amount_usd,
        "pay_amount": record.pay_amount,
        "pay_currency": record.pay_currency,
        "exchange_rate": record.exchange_rate,
        "payment_method": record.payment_method,
        "gateway_order_id": record.gateway_order_id,
        "gateway_response": sanitize_wallet_gateway_response(gateway_response),
        "status": record.status,
        "order_kind": "plan_purchase",
        "product_id": plan.id,
        "product": billing_plan_payload(plan),
        "created_at": unix_secs_to_rfc3339(stored_timestamp_unix_secs(record.created_at_unix_ms)),
        "paid_at": record.paid_at_unix_secs.and_then(unix_secs_to_rfc3339),
        "credited_at": record.credited_at_unix_secs.and_then(unix_secs_to_rfc3339),
        "expires_at": record.expires_at_unix_secs.and_then(unix_secs_to_rfc3339),
    })
}

async fn plan_checkout_replay_response(
    state: &AppState,
    order: &aether_data::repository::wallet::StoredAdminPaymentOrder,
    plan: &aether_data_contracts::repository::billing::BillingPlanRecord,
) -> Response<Body> {
    let payment_instructions = wallet_payment_instructions_from_stored(state, order).await;
    mark_sensitive_response_no_store(build_auth_json_response(
        http::StatusCode::OK,
        json!({
            "order": payment_order_payload(order, plan),
            "payment_instructions": payment_instructions,
            "reused_pending_order": true,
        }),
        None,
    ))
}

fn entitlement_payload(
    record: &aether_data_contracts::repository::billing::UserPlanEntitlementRecord,
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
        "created_at": unix_secs_to_rfc3339(record.created_at_unix_secs),
        "updated_at": unix_secs_to_rfc3339(record.updated_at_unix_secs),
    })
}

fn compute_plan_payment_amounts(
    plan: &aether_data_contracts::repository::billing::BillingPlanRecord,
    pay_currency: &str,
    usd_exchange_rate: f64,
) -> Result<(f64, f64), &'static str> {
    if !plan.price_amount.is_finite()
        || plan.price_amount <= 0.0
        || !usd_exchange_rate.is_finite()
        || usd_exchange_rate <= 0.0
    {
        return Err("套餐价格配置无效");
    }
    let plan_currency = normalize_payment_currency(&plan.price_currency, "price_currency")
        .map_err(|_| "套餐币种配置无效")?;
    let pay_currency = normalize_payment_currency(pay_currency, "pay_currency")
        .map_err(|_| "支付网关币种配置无效")?;
    // USD is the canonical settlement currency.  A USD-priced plan paid in
    // USD must not be divided by the configured non-USD conversion rate (the
    // default is 7.2), even though the two normalized currencies are equal.
    if plan_currency == "USD" && pay_currency == "USD" {
        let amount_usd = (plan.price_amount * 100_000_000.0).round() / 100_000_000.0;
        let pay_amount = (plan.price_amount * 100.0).round() / 100.0;
        if !amount_usd.is_finite()
            || amount_usd <= 0.0
            || !pay_amount.is_finite()
            || pay_amount <= 0.0
        {
            return Err("套餐价格配置无效");
        }
        return Ok((amount_usd, pay_amount));
    }
    if plan_currency == pay_currency {
        let amount_usd =
            (plan.price_amount / usd_exchange_rate * 100_000_000.0).round() / 100_000_000.0;
        let pay_amount = (plan.price_amount * 100.0).round() / 100.0;
        if !amount_usd.is_finite()
            || amount_usd <= 0.0
            || !pay_amount.is_finite()
            || pay_amount <= 0.0
        {
            return Err("套餐价格配置无效");
        }
        return Ok((amount_usd, pay_amount));
    }
    if plan_currency == "USD" {
        let amount_usd = (plan.price_amount * 100_000_000.0).round() / 100_000_000.0;
        let pay_amount = (plan.price_amount * usd_exchange_rate * 100.0).round() / 100.0;
        if !amount_usd.is_finite()
            || amount_usd <= 0.0
            || !pay_amount.is_finite()
            || pay_amount <= 0.0
        {
            return Err("套餐价格配置无效");
        }
        return Ok((amount_usd, pay_amount));
    }
    Err("套餐币种与支付网关币种不匹配")
}

pub(super) async fn handle_billing_plans_list(state: &AppState) -> Response<Body> {
    let plans = match state.list_billing_plans(false).await {
        Ok(Some(value)) => value,
        Ok(None) => return billing_storage_unavailable_response(),
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("billing plan lookup failed: {err:?}"),
                false,
            )
        }
    };
    let items = plans
        .iter()
        .filter(|plan| plan_has_package_rights(plan))
        .map(billing_plan_payload)
        .collect::<Vec<_>>();
    Json(json!({"items": items, "total": items.len()})).into_response()
}

pub(super) async fn handle_billing_entitlements(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let entitlements = match state.list_user_plan_entitlements(&auth.user.id).await {
        Ok(Some(value)) => value,
        Ok(None) => return billing_storage_unavailable_response(),
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("billing entitlement lookup failed: {err:?}"),
                false,
            )
        }
    };
    let now = Utc::now().timestamp().max(0) as u64;
    let items = entitlements
        .iter()
        .map(|record| {
            let mut payload = entitlement_payload(record);
            payload["active"] = json!(
                record.status == "active"
                    && record.starts_at_unix_secs <= now
                    && record.expires_at_unix_secs > now
            );
            payload
        })
        .collect::<Vec<_>>();
    Json(json!({"items": items, "total": items.len()})).into_response()
}

pub(super) async fn handle_billing_plan_checkout(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
    client_ip: std::net::IpAddr,
    request_body: Option<&Bytes>,
) -> Response<Body> {
    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(plan_id) = plan_id_from_checkout_path(&request_context.request_path) else {
        return build_auth_error_response(http::StatusCode::BAD_REQUEST, "缺少套餐ID", false);
    };
    let payload = match request_body {
        Some(body) if !body.is_empty() => {
            match serde_json::from_slice::<BillingPlanCheckoutRequest>(body) {
                Ok(value) => value,
                Err(_) => {
                    return build_auth_error_response(
                        http::StatusCode::BAD_REQUEST,
                        "输入验证失败",
                        false,
                    )
                }
            }
        }
        _ => BillingPlanCheckoutRequest::default(),
    };
    let checkout_request = match normalize_checkout_request(payload) {
        Ok(value) => value,
        Err(detail) => {
            return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false)
        }
    };

    let plan = match state.find_billing_plan(&plan_id).await {
        Ok(Some(value)) if value.enabled => value,
        Ok(Some(_)) => {
            return build_auth_error_response(http::StatusCode::BAD_REQUEST, "套餐已下架", false)
        }
        Ok(None) => {
            return build_auth_error_response(http::StatusCode::NOT_FOUND, "套餐不存在", false)
        }
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("billing plan lookup failed: {err:?}"),
                false,
            )
        }
    };
    if !plan_has_package_rights(&plan) {
        return build_auth_error_response(
            http::StatusCode::BAD_REQUEST,
            "余额包已移除，请使用钱包充值功能",
            false,
        );
    }
    let requested_provider = checkout_request.payment_provider.as_str();
    let payment_method = checkout_request.payment_method.clone();
    if requested_provider == "epay" {
        let config = match load_epay_config(state).await {
            Ok(value) => value,
            Err(detail) => {
                return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false)
            }
        };
        let payment_channel =
            match resolve_epay_channel(&config, checkout_request.payment_channel.as_deref()) {
                Ok(value) => value,
                Err(detail) => {
                    return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false);
                }
            };
        let payment_channel_id = payment_channel.channel.clone();
        let (amount_usd, pay_amount) = match compute_plan_payment_amounts(
            &plan,
            &config.pay_currency,
            config.usd_exchange_rate,
        ) {
            Ok(value) => value,
            Err(detail) => {
                return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false)
            }
        };
        let pending_order = match find_matching_pending_plan_purchase_order(
            state,
            &auth.user.id,
            &plan.id,
            &payment_method,
            requested_provider,
            &payment_channel_id,
            amount_usd,
            pay_amount,
            &config.pay_currency,
            config.usd_exchange_rate,
        )
        .await
        {
            Ok(value) => value,
            Err(detail) => {
                return build_auth_error_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    detail,
                    false,
                )
            }
        };
        if let Some(order) = pending_order {
            return plan_checkout_replay_response(state, &order, &plan).await;
        }
        let now = Utc::now();
        let expires_at = now + chrono::Duration::minutes(30);
        let order_no = match choose_plan_order_no(
            state,
            &auth.user.id,
            &plan.id,
            &payment_method,
            requested_provider,
            &payment_channel_id,
            amount_usd,
            pay_amount,
            &config.pay_currency,
            config.usd_exchange_rate,
            now,
        )
        .await
        {
            Ok(PlanOrderNoChoice::Fresh(order_no)) => order_no,
            Ok(PlanOrderNoChoice::Reuse(order)) => {
                return plan_checkout_replay_response(state, &order, &plan).await
            }
            Err(detail) => {
                return build_auth_error_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    detail,
                    false,
                )
            }
        };
        let Some(callback_base_url) = epay_callback_base_url(config.callback_base_url.as_deref())
        else {
            return build_auth_error_response(
                http::StatusCode::BAD_REQUEST,
                "epay callback_base_url is required",
                false,
            );
        };
        let checkout = match build_epay_checkout_url(
            &config,
            &EpayCheckoutInput {
                order_no: order_no.clone(),
                channel: payment_channel_id.clone(),
                subject: plan.title.clone(),
                pay_amount,
                notify_url: format!("{callback_base_url}/api/payment/epay/notify"),
                return_url: format!("{callback_base_url}/api/payment/epay/return"),
            },
        ) {
            Ok(value) => value,
            Err(detail) => {
                return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false)
            }
        };
        let stored_gateway_response = match prepare_billing_gateway_response_for_storage(
            state,
            "epay",
            &order_no,
            &auth.user.id,
            &checkout,
        ) {
            Ok(value) => value,
            Err(detail) => {
                return build_auth_error_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    detail,
                    false,
                )
            }
        };
        let outcome = match state
            .create_plan_purchase_order(
                aether_data::repository::wallet::CreatePlanPurchaseOrderInput {
                    preferred_wallet_id: None,
                    user_id: auth.user.id.clone(),
                    amount_usd,
                    pay_amount,
                    pay_currency: config.pay_currency.clone(),
                    exchange_rate: config.usd_exchange_rate,
                    payment_method: payment_method.clone(),
                    payment_provider: Some(checkout_request.payment_provider.clone()),
                    payment_channel: Some(payment_channel_id.clone()),
                    gateway_order_id: order_no.clone(),
                    gateway_response: stored_gateway_response,
                    order_no: order_no.clone(),
                    product_id: plan.id.clone(),
                    product_snapshot: billing_plan_snapshot(&plan),
                    expires_at_unix_secs: expires_at.timestamp().max(0) as u64,
                },
            )
            .await
        {
            Ok(Some(value)) => value,
            Ok(None) => return billing_storage_unavailable_response(),
            Err(err) => {
                match resolve_plan_order_after_create_failure(
                    state,
                    &auth.user.id,
                    &plan,
                    requested_provider,
                    &payment_method,
                    &payment_channel_id,
                    amount_usd,
                    pay_amount,
                    &config.pay_currency,
                    config.usd_exchange_rate,
                    &order_no,
                )
                .await
                {
                    Ok(PlanCreateFailureResolution::Replay(response)) => return response,
                    Ok(
                        PlanCreateFailureResolution::Missing
                        | PlanCreateFailureResolution::Occupied,
                    ) => {}
                    Err(reconcile_error) => warn!(
                        order_no,
                        error = reconcile_error,
                        "failed to reconcile EPay plan order after create error"
                    ),
                }
                return build_auth_error_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("billing checkout create failed: {err:?}"),
                    false,
                );
            }
        };
        let order = match outcome {
            aether_data::repository::wallet::CreatePlanPurchaseOrderOutcome::Created(order) => {
                payment_order_payload(&order, &plan)
            }
            aether_data::repository::wallet::CreatePlanPurchaseOrderOutcome::WalletInactive => {
                return build_auth_error_response(
                    http::StatusCode::BAD_REQUEST,
                    "wallet is not active",
                    false,
                )
            }
            aether_data::repository::wallet::CreatePlanPurchaseOrderOutcome::ActivePlanLimitReached => {
                return build_auth_error_response(
                    http::StatusCode::CONFLICT,
                    "套餐购买限制已达到上限",
                    false,
                )
            }
        };
        mark_sensitive_response_no_store(build_auth_json_response(
            http::StatusCode::OK,
            json!({
                "order": order,
                "payment_instructions": sanitize_wallet_gateway_response(Some(checkout)),
            }),
            None,
        ))
    } else {
        let (payment_channel, display_name, pay_currency, usd_exchange_rate, callback_base_url) = {
            let record = match state.find_payment_gateway_config(requested_provider).await {
                Ok(Some(value)) if value.enabled && value.merchant_key_encrypted.is_some() => value,
                Ok(Some(_)) | Ok(None) => {
                    return build_auth_error_response(
                        http::StatusCode::BAD_REQUEST,
                        "支付网关未启用或密钥未配置",
                        false,
                    )
                }
                Err(err) => {
                    return build_auth_error_response(
                        http::StatusCode::INTERNAL_SERVER_ERROR,
                        format!("payment gateway lookup failed: {err:?}"),
                        false,
                    )
                }
            };
            let resolved_channel = match resolve_direct_gateway_channel(
                requested_provider,
                &record,
                checkout_request.payment_channel.as_deref(),
            ) {
                Ok(value) => value,
                Err(detail) => {
                    return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false)
                }
            };
            let payment_channel = resolved_channel.channel;
            let display_name = resolved_channel.display_name;
            let pay_currency =
                match normalize_payment_currency(&record.pay_currency, "pay_currency") {
                    Ok(value) => value,
                    Err(_) => {
                        return build_auth_error_response(
                            http::StatusCode::BAD_REQUEST,
                            "支付网关币种配置无效",
                            false,
                        )
                    }
                };
            (
                payment_channel,
                display_name,
                pay_currency,
                record.usd_exchange_rate,
                record.callback_base_url,
            )
        };
        let (amount_usd, pay_amount) =
            match compute_plan_payment_amounts(&plan, &pay_currency, usd_exchange_rate) {
                Ok(value) => value,
                Err(detail) => {
                    return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false)
                }
            };
        let pending_order = match find_matching_pending_plan_purchase_order(
            state,
            &auth.user.id,
            &plan.id,
            &payment_method,
            requested_provider,
            &payment_channel,
            amount_usd,
            pay_amount,
            &pay_currency,
            usd_exchange_rate,
        )
        .await
        {
            Ok(value) => value,
            Err(detail) => {
                return build_auth_error_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    detail,
                    false,
                )
            }
        };
        if let Some(order) = pending_order {
            return plan_checkout_replay_response(state, &order, &plan).await;
        }
        let now = Utc::now();
        let expires_at = now + chrono::Duration::minutes(30);
        let mut order_no = match choose_plan_order_no(
            state,
            &auth.user.id,
            &plan.id,
            &payment_method,
            requested_provider,
            &payment_channel,
            amount_usd,
            pay_amount,
            &pay_currency,
            usd_exchange_rate,
            now,
        )
        .await
        {
            Ok(PlanOrderNoChoice::Fresh(order_no)) => order_no,
            Ok(PlanOrderNoChoice::Reuse(order)) => {
                return plan_checkout_replay_response(state, &order, &plan).await
            }
            Err(detail) => {
                return build_auth_error_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    detail,
                    false,
                )
            }
        };
        let Some(callback_base_url) = epay_callback_base_url(callback_base_url.as_deref()) else {
            return build_auth_error_response(
                http::StatusCode::BAD_REQUEST,
                "支付网关 callback_base_url is required",
                false,
            );
        };
        let mut checkout_attempt = 0_u8;
        let checkout = loop {
            let direct_input = DirectPaymentCheckoutInput {
                payment_channel: payment_channel.clone(),
                display_name: display_name.clone(),
                order_no: order_no.clone(),
                subject: plan.title.clone(),
                pay_amount,
                pay_currency: pay_currency.clone(),
                notify_url: format!("{callback_base_url}/api/payment/{requested_provider}/notify"),
                return_url: Some(format!("{callback_base_url}/dashboard/billing")),
                client_ip: Some(client_ip.to_string()),
                expires_at,
            };
            let checkout_result: Result<Value, DirectPaymentCheckoutError> =
                match requested_provider {
                    "alipay" => create_alipay_direct_checkout(state, &direct_input).await,
                    "wxpay" => create_wxpay_direct_checkout(state, &direct_input).await,
                    "stripe" => create_stripe_direct_checkout(state, &direct_input).await,
                    _ => Err(DirectPaymentCheckoutError::Failed(
                        "unsupported payment provider".to_string(),
                    )),
                };
            match checkout_result {
                Ok(value) => break value,
                Err(DirectPaymentCheckoutError::Canceled)
                    if requested_provider == "stripe" && checkout_attempt == 0 =>
                {
                    // A prior failed local write may have cancelled the
                    // PaymentIntent retained by Stripe for this idempotency
                    // key.  Move to a fresh merchant identity before retrying;
                    // otherwise Stripe would return the same unusable intent.
                    checkout_attempt = 1;
                    order_no = match fresh_plan_order_no(
                        state,
                        &auth.user.id,
                        &plan.id,
                        &payment_method,
                        requested_provider,
                        &payment_channel,
                        amount_usd,
                        pay_amount,
                        &pay_currency,
                        usd_exchange_rate,
                        now,
                    )
                    .await
                    {
                        Ok(value) => value,
                        Err(detail) => {
                            return build_auth_error_response(
                                http::StatusCode::INTERNAL_SERVER_ERROR,
                                detail,
                                false,
                            )
                        }
                    };
                }
                Err(DirectPaymentCheckoutError::Canceled) => {
                    return build_auth_error_response(
                        http::StatusCode::BAD_GATEWAY,
                        "Stripe PaymentIntent 已取消，请稍后重试",
                        false,
                    )
                }
                Err(DirectPaymentCheckoutError::Uncertain(detail)) => {
                    // A provider may accept the merchant order before the
                    // client loses or rejects its response. Alipay and WxPay
                    // can still be closed by merchant order number; Stripe
                    // remains retry-recoverable through its deterministic
                    // idempotency key when no intent id was received.
                    close_abandoned_direct_plan_checkout(
                        state,
                        requested_provider,
                        &order_no,
                        None,
                        "provider_checkout_failed",
                    )
                    .await;
                    return build_auth_error_response(http::StatusCode::BAD_GATEWAY, detail, false);
                }
                Err(DirectPaymentCheckoutError::Failed(detail)) => {
                    return build_auth_error_response(http::StatusCode::BAD_GATEWAY, detail, false);
                }
            }
        };
        let checkout_gateway_order_id = checkout
            .get("gateway_order_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
        let payment_instructions =
            wallet_payment_instructions_from_checkout(requested_provider, &checkout);
        let stored_gateway_response = match prepare_billing_gateway_response_for_storage(
            state,
            requested_provider,
            &order_no,
            &auth.user.id,
            &checkout,
        ) {
            Ok(value) => value,
            Err(detail) => {
                close_abandoned_direct_plan_checkout(
                    state,
                    requested_provider,
                    &order_no,
                    checkout_gateway_order_id.as_deref(),
                    "gateway_response_projection_failed",
                )
                .await;
                return build_auth_error_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    detail,
                    false,
                );
            }
        };
        let outcome = match state
            .create_plan_purchase_order(
                aether_data::repository::wallet::CreatePlanPurchaseOrderInput {
                    preferred_wallet_id: None,
                    user_id: auth.user.id.clone(),
                    amount_usd,
                    pay_amount,
                    pay_currency: pay_currency.clone(),
                    exchange_rate: usd_exchange_rate,
                    payment_method: payment_method.clone(),
                    payment_provider: Some(requested_provider.to_string()),
                    payment_channel: Some(payment_channel.clone()),
                    gateway_order_id: checkout_gateway_order_id
                        .clone()
                        .unwrap_or_else(|| order_no.clone()),
                    gateway_response: stored_gateway_response,
                    order_no: order_no.clone(),
                    product_id: plan.id.clone(),
                    product_snapshot: billing_plan_snapshot(&plan),
                    expires_at_unix_secs: expires_at.timestamp().max(0) as u64,
                },
            )
            .await
        {
            Ok(Some(value)) => value,
            Ok(None) => {
                close_abandoned_direct_plan_checkout(
                    state,
                    requested_provider,
                    &order_no,
                    checkout_gateway_order_id.as_deref(),
                    "billing_storage_unavailable",
                )
                .await;
                return billing_storage_unavailable_response();
            }
            Err(err) => {
                let create_error = format!("billing checkout create failed: {err:?}");
                match resolve_plan_order_after_create_failure(
                    state,
                    &auth.user.id,
                    &plan,
                    requested_provider,
                    &payment_method,
                    &payment_channel,
                    amount_usd,
                    pay_amount,
                    &pay_currency,
                    usd_exchange_rate,
                    &order_no,
                )
                .await
                {
                    Ok(PlanCreateFailureResolution::Replay(response)) => return response,
                    Ok(PlanCreateFailureResolution::Missing) => {
                        close_abandoned_direct_plan_checkout(
                            state,
                            requested_provider,
                            &order_no,
                            checkout_gateway_order_id.as_deref(),
                            "billing_order_create_failed_no_local_order",
                        )
                        .await;
                    }
                    Ok(PlanCreateFailureResolution::Occupied) => {
                        warn!(
                            order_no,
                            "direct plan checkout create conflicted with an existing order; external checkout was left untouched"
                        );
                    }
                    Err(reconcile_error) => warn!(
                        order_no,
                        error = reconcile_error,
                        "could not determine whether direct plan checkout was persisted"
                    ),
                }
                return build_auth_error_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    create_error,
                    false,
                );
            }
        };
        let order = match outcome {
            aether_data::repository::wallet::CreatePlanPurchaseOrderOutcome::Created(order) => {
                payment_order_payload(&order, &plan)
            }
            aether_data::repository::wallet::CreatePlanPurchaseOrderOutcome::WalletInactive => {
                close_abandoned_direct_plan_checkout(
                    state,
                    requested_provider,
                    &order_no,
                    checkout_gateway_order_id.as_deref(),
                    "wallet_inactive",
                )
                .await;
                return build_auth_error_response(
                    http::StatusCode::BAD_REQUEST,
                    "wallet is not active",
                    false,
                )
            }
            aether_data::repository::wallet::CreatePlanPurchaseOrderOutcome::ActivePlanLimitReached => {
                close_abandoned_direct_plan_checkout(
                    state,
                    requested_provider,
                    &order_no,
                    checkout_gateway_order_id.as_deref(),
                    "active_plan_limit_reached",
                )
                .await;
                return build_auth_error_response(
                    http::StatusCode::CONFLICT,
                    "套餐购买限制已达到上限",
                    false,
                )
            }
        };
        mark_sensitive_response_no_store(build_auth_json_response(
            http::StatusCode::OK,
            json!({
                "order": order,
                "payment_instructions": payment_instructions,
            }),
            None,
        ))
    }
}

pub(super) async fn maybe_build_local_billing_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
    client_ip: std::net::IpAddr,
    request_body: Option<&Bytes>,
) -> Option<Response<Body>> {
    let decision = request_context.control_decision.as_ref()?;
    if decision.route_family.as_deref() != Some("billing") {
        return None;
    }
    match decision.route_kind.as_deref() {
        Some("plans") if request_context.request_path == "/api/billing/plans" => {
            Some(handle_billing_plans_list(state).await)
        }
        Some("plan_checkout") => Some(
            handle_billing_plan_checkout(state, request_context, headers, client_ip, request_body)
                .await,
        ),
        Some("entitlements") if request_context.request_path == "/api/billing/entitlements" => {
            Some(handle_billing_entitlements(state, request_context, headers).await)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::super::prepare_wallet_gateway_response_for_storage;
    use super::{
        billing_order_no, billing_order_no_with_nonce, compute_plan_payment_amounts,
        normalize_checkout_request, plan_purchase_order_matches_checkout,
        prepare_billing_gateway_response_for_storage, wallet_payment_instructions_from_stored,
        AppState, BillingPlanCheckoutRequest,
    };
    use aether_data_contracts::repository::{
        billing::BillingPlanRecord, wallet::StoredAdminPaymentOrder,
    };
    use chrono::Utc;
    use serde_json::json;

    fn request(
        payment_method: Option<&str>,
        payment_provider: Option<&str>,
        payment_channel: Option<&str>,
    ) -> BillingPlanCheckoutRequest {
        BillingPlanCheckoutRequest {
            payment_method: payment_method.map(str::to_string),
            payment_provider: payment_provider.map(str::to_string),
            payment_channel: payment_channel.map(str::to_string),
        }
    }

    #[test]
    fn checkout_normalization_preserves_canonical_and_legacy_epay_requests() {
        let canonical =
            normalize_checkout_request(request(Some("EPAY"), Some("epay"), Some("Alipay")))
                .expect("canonical EPay request should normalize");
        assert_eq!(canonical.payment_method, "epay");
        assert_eq!(canonical.payment_provider, "epay");
        assert_eq!(canonical.payment_channel.as_deref(), Some("alipay"));

        let legacy = normalize_checkout_request(request(Some("alipay"), Some("epay"), None))
            .expect("legacy EPay shorthand should normalize");
        // EPay is the durable payment namespace; the legacy method selects
        // only the aggregator channel.
        assert_eq!(legacy.payment_method, "epay");
        assert_eq!(legacy.payment_provider, "epay");
        assert_eq!(legacy.payment_channel.as_deref(), Some("alipay"));

        let defaulted = normalize_checkout_request(request(None, None, None))
            .expect("empty checkout request should use the EPay default");
        assert_eq!(defaulted.payment_method, "epay");
        assert_eq!(defaulted.payment_provider, "epay");
        assert!(defaulted.payment_channel.is_none());
    }

    #[test]
    fn checkout_normalization_rejects_provider_method_and_legacy_channel_conflicts() {
        assert!(normalize_checkout_request(request(Some("alipay"), Some("stripe"), None)).is_err());
        assert!(normalize_checkout_request(request(Some("stripe"), Some("epay"), None)).is_err());
        assert!(
            normalize_checkout_request(request(Some("alipay"), Some("epay"), Some("wxpay")))
                .is_err()
        );
        assert!(normalize_checkout_request(request(Some("unknown"), Some("epay"), None)).is_err());
    }

    #[test]
    fn checkout_normalization_leaves_direct_provider_channel_for_config_resolution() {
        let wxpay = normalize_checkout_request(request(Some("wxpay"), Some("wxpay"), None))
            .expect("wxpay checkout should normalize without a channel");
        assert_eq!(wxpay.payment_provider, "wxpay");
        assert_eq!(wxpay.payment_method, "wxpay");
        assert!(wxpay.payment_channel.is_none());

        let stripe = normalize_checkout_request(request(Some("stripe"), Some("stripe"), None))
            .expect("stripe checkout should normalize without a channel");
        assert_eq!(stripe.payment_provider, "stripe");
        assert_eq!(stripe.payment_method, "stripe");
        assert!(stripe.payment_channel.is_none());

        let explicit =
            normalize_checkout_request(request(Some("wxpay"), Some("wxpay"), Some("h5")))
                .expect("an explicit direct channel should be preserved");
        assert_eq!(explicit.payment_channel.as_deref(), Some("h5"));
    }

    #[test]
    fn checkout_normalization_rejects_overlong_fields_instead_of_falling_back() {
        let too_long = "x".repeat(31);
        assert!(normalize_checkout_request(request(None, Some(&too_long), None)).is_err());
        assert!(normalize_checkout_request(request(Some(&too_long), None, None)).is_err());
        assert!(normalize_checkout_request(request(None, None, Some(&too_long))).is_err());
    }

    fn test_plan(price_amount: f64, price_currency: &str) -> BillingPlanRecord {
        BillingPlanRecord {
            id: "plan-test".to_string(),
            title: "Test".to_string(),
            description: None,
            price_amount,
            price_currency: price_currency.to_string(),
            duration_unit: "month".to_string(),
            duration_value: 1,
            enabled: true,
            sort_order: 0,
            max_active_per_user: 0,
            purchase_limit_scope: "none".to_string(),
            entitlements_json: json!({}),
            created_at_unix_secs: 0,
            updated_at_unix_secs: 0,
        }
    }

    #[test]
    fn plan_payment_amounts_reject_non_finite_exchange_rate_and_results() {
        assert!(compute_plan_payment_amounts(&test_plan(10.0, "USD"), "CNY", f64::NAN).is_err());
        assert!(
            compute_plan_payment_amounts(&test_plan(f64::MAX, "USD"), "CNY", f64::MAX).is_err()
        );
        assert!(
            compute_plan_payment_amounts(&test_plan(10.0, "USD"), "CNY", 7.25)
                .expect("valid plan amounts")
                .0
                .is_finite()
        );
    }

    #[test]
    fn plan_payment_amounts_normalize_currency_at_the_public_boundary() {
        let amounts = compute_plan_payment_amounts(&test_plan(10.0, " USD "), " cny ", 7.25)
            .expect("trimmed ASCII currencies should be accepted");
        assert_eq!(amounts.0, 10.0);
        assert_eq!(amounts.1, 72.5);
        assert!(compute_plan_payment_amounts(&test_plan(10.0, "US$"), "CNY", 7.25).is_err());
        assert!(compute_plan_payment_amounts(&test_plan(10.0, "美元"), "CNY", 7.25).is_err());
        assert!(compute_plan_payment_amounts(&test_plan(10.0, "USD"), "CN", 7.25).is_err());
    }

    #[test]
    fn usd_plan_paid_in_usd_ignores_non_usd_exchange_rate() {
        let amounts = compute_plan_payment_amounts(&test_plan(10.0, "USD"), "USD", 7.2)
            .expect("USD/USD plan checkout should be valid");
        assert_eq!(amounts, (10.0, 10.0));
    }

    fn pending_order(
        payment_method: &str,
        gateway_response: serde_json::Value,
    ) -> StoredAdminPaymentOrder {
        let now = Utc::now().timestamp().max(0) as u64;
        StoredAdminPaymentOrder {
            id: "order-test".to_string(),
            order_no: "pp-test".to_string(),
            wallet_id: "wallet-test".to_string(),
            user_id: Some("user-test".to_string()),
            amount_usd: 10.0,
            pay_amount: Some(72.5),
            pay_currency: Some("CNY".to_string()),
            exchange_rate: Some(7.25),
            refunded_amount_usd: 0.0,
            refundable_amount_usd: 0.0,
            payment_method: payment_method.to_string(),
            payment_provider: Some(
                gateway_response
                    .get("gateway")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(payment_method)
                    .to_string(),
            ),
            order_kind: "plan_purchase".to_string(),
            gateway_order_id: Some("gateway-test".to_string()),
            gateway_response: Some(gateway_response),
            status: "pending".to_string(),
            created_at_unix_ms: now.saturating_mul(1000),
            paid_at_unix_secs: None,
            credited_at_unix_secs: None,
            expires_at_unix_secs: Some(now.saturating_add(600)),
        }
    }

    #[tokio::test]
    async fn plan_stripe_client_secret_cannot_be_copied_to_another_order_or_wallet_kind() {
        let state = AppState::new().expect("test app state");
        let client_secret = "pi_plan_source_secret_capability";
        let stored = prepare_billing_gateway_response_for_storage(
            &state,
            "stripe",
            "pp-source",
            "user-source",
            &json!({
                "gateway": "stripe",
                "client_secret": client_secret,
                "publishable_key": "pk_test_public",
            }),
        )
        .expect("source plan checkout should encrypt");
        let mut source = pending_order("stripe", stored);
        source.id = "plan-order-source".to_string();
        source.order_no = "pp-source".to_string();
        source.user_id = Some("user-source".to_string());
        assert_eq!(
            wallet_payment_instructions_from_stored(&state, &source).await["client_secret"],
            client_secret
        );

        let mut foreign = source.clone();
        foreign.id = "plan-order-foreign".to_string();
        foreign.order_no = "pp-foreign".to_string();
        assert!(wallet_payment_instructions_from_stored(&state, &foreign)
            .await
            .get("client_secret")
            .is_none());

        let wallet_ciphertext = prepare_wallet_gateway_response_for_storage(
            &state,
            "stripe",
            "po-wallet-source",
            "user-source",
            &json!({
                "gateway": "stripe",
                "client_secret": "pi_wallet_source_secret_capability",
                "publishable_key": "pk_test_public",
            }),
        )
        .expect("wallet checkout should encrypt");
        let mut plan_with_wallet_ciphertext = pending_order("stripe", wallet_ciphertext);
        plan_with_wallet_ciphertext.id = "plan-with-wallet-ciphertext".to_string();
        plan_with_wallet_ciphertext.order_no = "po-wallet-source".to_string();
        plan_with_wallet_ciphertext.user_id = Some("user-source".to_string());
        assert!(
            wallet_payment_instructions_from_stored(&state, &plan_with_wallet_ciphertext)
                .await
                .get("client_secret")
                .is_none()
        );
    }

    #[test]
    fn pending_plan_reuse_requires_the_complete_payment_identity() {
        let order = pending_order(
            "stripe",
            json!({
                "gateway": "stripe",
                "payment_channel": "card",
                "product_id": "plan-test"
            }),
        );
        let matches = |order: &StoredAdminPaymentOrder| {
            plan_purchase_order_matches_checkout(
                order,
                "plan-test",
                "stripe",
                "stripe",
                "card",
                10.0,
                72.5,
                "CNY",
                7.25,
            )
        };
        assert!(matches(&order));

        let mut changed = order.clone();
        changed.payment_method = "alipay".to_string();
        assert!(!matches(&changed));
        let mut changed = order.clone();
        changed.gateway_response = Some(json!({
            "gateway": "wxpay",
            "payment_channel": "native",
            "product_id": "plan-test"
        }));
        assert!(!matches(&changed));
        let mut changed = order.clone();
        changed.gateway_response = Some(json!({
            "gateway": "stripe",
            "payment_channel": "link",
            "product_id": "plan-test"
        }));
        assert!(!matches(&changed));
        let mut changed = order.clone();
        changed.pay_amount = Some(72.0);
        assert!(!matches(&changed));
        let mut changed = order.clone();
        changed.pay_currency = Some("USD".to_string());
        assert!(!matches(&changed));
        let mut changed = order;
        changed.exchange_rate = Some(7.0);
        assert!(!matches(&changed));
    }

    #[test]
    fn pending_plan_reuse_accepts_legacy_epay_method_only_for_same_channel() {
        let order = pending_order(
            "alipay",
            json!({
                "gateway": "epay",
                "payment_channel": "alipay",
                "product_id": "plan-test"
            }),
        );
        assert!(plan_purchase_order_matches_checkout(
            &order,
            "plan-test",
            "epay",
            "epay",
            "alipay",
            10.0,
            72.5,
            "CNY",
            7.25,
        ));
        assert!(!plan_purchase_order_matches_checkout(
            &order,
            "plan-test",
            "epay",
            "epay",
            "wxpay",
            10.0,
            72.5,
            "CNY",
            7.25,
        ));
    }

    #[test]
    fn billing_order_number_is_stable_for_retries_and_changes_with_payment_identity() {
        let now = Utc::now();
        let first = billing_order_no(
            "user-test",
            "plan-test",
            "stripe",
            "stripe",
            "card",
            10.0,
            72.5,
            "CNY",
            7.25,
            now,
        );
        let retry = billing_order_no(
            "user-test",
            "plan-test",
            "stripe",
            "stripe",
            "card",
            10.0,
            72.5,
            "CNY",
            7.25,
            now,
        );
        assert_eq!(first, retry);
        assert!(first.starts_with("pp_"));
        assert!(first.len() <= 64);
        let different_channel = billing_order_no(
            "user-test",
            "plan-test",
            "stripe",
            "stripe",
            "link",
            10.0,
            72.5,
            "CNY",
            7.25,
            now,
        );
        assert_ne!(first, different_channel);
    }

    #[test]
    fn cancelled_stripe_retry_uses_a_new_merchant_order_identity() {
        let now = Utc::now();
        let original = billing_order_no(
            "user-test",
            "plan-test",
            "stripe",
            "stripe",
            "card",
            10.0,
            72.5,
            "CNY",
            7.25,
            now,
        );
        let recovered = billing_order_no_with_nonce(
            "user-test",
            "plan-test",
            "stripe",
            "stripe",
            "card",
            10.0,
            72.5,
            "CNY",
            7.25,
            "stripe-cancelled-recovery",
            now,
        );
        assert_ne!(original, recovered);
        assert!(recovered.starts_with("pp_"));
        assert!(recovered.len() <= 64);
    }

    #[test]
    fn direct_channel_resolution_uses_the_configured_allowlist() {
        let record = aether_data_contracts::repository::billing::PaymentGatewayConfigRecord {
            provider: "wxpay".to_string(),
            enabled: true,
            endpoint_url: "https://pay.example.test".to_string(),
            callback_base_url: Some("https://app.example.test".to_string()),
            merchant_id: "merchant".to_string(),
            merchant_key_encrypted: Some("encrypted".to_string()),
            pay_currency: "CNY".to_string(),
            usd_exchange_rate: 7.0,
            min_recharge_usd: 1.0,
            channels_json: json!([
                {"channel": "native", "display_name": "Native", "fee_rate": 0.0}
            ]),
            created_at_unix_secs: 0,
            updated_at_unix_secs: 0,
        };
        let selected = super::resolve_direct_gateway_channel("wxpay", &record, None)
            .expect("configured native channel should be selected");
        assert_eq!(selected.channel, "native");
        assert!(super::resolve_direct_gateway_channel("wxpay", &record, Some("h5")).is_err());
    }
}
