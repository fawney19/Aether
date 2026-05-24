use super::support_payment::payment_epay::{
    build_epay_checkout_url, epay_callback_base_url, load_epay_config, resolve_epay_channel,
    EpayCheckoutInput,
};
use super::{
    build_auth_error_response, build_auth_json_response, resolve_authenticated_local_user,
    sanitize_wallet_gateway_response, unix_secs_to_rfc3339, AppState, GatewayPublicRequestContext,
};
use crate::handlers::shared::{
    create_alipay_direct_checkout, create_stripe_direct_checkout, create_wxpay_direct_checkout,
    direct_payment_client_ip, DirectPaymentCheckoutInput,
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
use uuid::Uuid;

const BILLING_STORAGE_UNAVAILABLE_DETAIL: &str = "套餐后端暂不可用";

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

fn normalize_optional_checkout_string(value: Option<String>, max_len: usize) -> Option<String> {
    value
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty() && value.chars().count() <= max_len)
}

fn normalize_checkout_request(
    payload: BillingPlanCheckoutRequest,
) -> Result<NormalizedBillingPlanCheckoutRequest, &'static str> {
    let payment_provider = normalize_optional_checkout_string(payload.payment_provider, 30)
        .or_else(|| normalize_optional_checkout_string(payload.payment_method.clone(), 30))
        .unwrap_or_else(|| "epay".to_string());
    if !matches!(
        payment_provider.as_str(),
        "epay" | "alipay" | "wxpay" | "stripe"
    ) {
        return Err("unsupported payment_provider");
    }
    let payment_method = normalize_optional_checkout_string(payload.payment_method, 30)
        .unwrap_or_else(|| payment_provider.clone());
    let payment_channel = normalize_optional_checkout_string(payload.payment_channel, 30)
        .or_else(|| (payment_method != "epay").then_some(payment_method.clone()));
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

fn billing_order_no(now: chrono::DateTime<chrono::Utc>) -> String {
    format!(
        "pp_{}_{}",
        now.format("%Y%m%d%H%M%S%6f"),
        &Uuid::new_v4().simple().to_string()[..12]
    )
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
                Some("daily_quota" | "membership_group")
            )
        })
    })
}

fn payment_order_payload(
    record: &aether_data::repository::wallet::StoredAdminPaymentOrder,
    plan: &aether_data_contracts::repository::billing::BillingPlanRecord,
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
        "payment_method": record.payment_method,
        "gateway_order_id": record.gateway_order_id,
        "gateway_response": sanitize_wallet_gateway_response(record.gateway_response.clone()),
        "status": record.status,
        "order_kind": "plan_purchase",
        "product_id": plan.id,
        "product": billing_plan_payload(plan),
        "created_at": unix_secs_to_rfc3339(record.created_at_unix_ms),
        "paid_at": record.paid_at_unix_secs.and_then(unix_secs_to_rfc3339),
        "credited_at": record.credited_at_unix_secs.and_then(unix_secs_to_rfc3339),
        "expires_at": record.expires_at_unix_secs.and_then(unix_secs_to_rfc3339),
    })
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
    if !plan.price_amount.is_finite() || plan.price_amount <= 0.0 || usd_exchange_rate <= 0.0 {
        return Err("套餐价格配置无效");
    }
    if plan.price_currency.eq_ignore_ascii_case(pay_currency) {
        let amount_usd =
            (plan.price_amount / usd_exchange_rate * 100_000_000.0).round() / 100_000_000.0;
        let pay_amount = (plan.price_amount * 100.0).round() / 100.0;
        return Ok((amount_usd, pay_amount));
    }
    if plan.price_currency.eq_ignore_ascii_case("USD") {
        let amount_usd = (plan.price_amount * 100_000_000.0).round() / 100_000_000.0;
        let pay_amount = (plan.price_amount * usd_exchange_rate * 100.0).round() / 100.0;
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
    let now = Utc::now();
    let order_no = billing_order_no(now);
    let expires_at = now + chrono::Duration::minutes(30);
    let requested_provider = checkout_request.payment_provider.as_str();
    let payment_method = checkout_request.payment_method.clone();
    let payment_channel =
        checkout_request
            .payment_channel
            .clone()
            .or_else(|| match requested_provider {
                "alipay" => Some("alipay".to_string()),
                "wxpay" => Some("native".to_string()),
                "stripe" => Some("card".to_string()),
                _ => None,
            });
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
        let Some(callback_base_url) = epay_callback_base_url(
            config.callback_base_url.as_deref(),
            headers,
            request_context,
        ) else {
            return build_auth_error_response(
                http::StatusCode::BAD_REQUEST,
                "epay callback_base_url is required",
                false,
            );
        };
        let checkout = build_epay_checkout_url(
            &config,
            &EpayCheckoutInput {
                order_no: order_no.clone(),
                channel: payment_channel_id.clone(),
                subject: plan.title.clone(),
                pay_amount,
                notify_url: format!("{callback_base_url}/api/payment/epay/notify"),
                return_url: format!("{callback_base_url}/api/payment/epay/return"),
            },
        );
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
                    payment_channel: Some(payment_channel_id),
                    gateway_order_id: order_no.clone(),
                    gateway_response: checkout.clone(),
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
                return build_auth_error_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("billing checkout create failed: {err:?}"),
                    false,
                )
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
        build_auth_json_response(
            http::StatusCode::OK,
            json!({
                "order": order,
                "payment_instructions": sanitize_wallet_gateway_response(Some(checkout)),
            }),
            None,
        )
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
            let payment_channel =
                payment_channel
                    .clone()
                    .unwrap_or_else(|| match requested_provider {
                        "alipay" => "alipay".to_string(),
                        "wxpay" => "native".to_string(),
                        "stripe" => "card".to_string(),
                        _ => "alipay".to_string(),
                    });
            let display_name = match requested_provider {
                "alipay" => "支付宝官方".to_string(),
                "wxpay" => match payment_channel.as_str() {
                    "h5" => "微信 H5".to_string(),
                    "jsapi" => "微信 JSAPI".to_string(),
                    _ => "微信 Native".to_string(),
                },
                "stripe" => match payment_channel.as_str() {
                    "alipay" => "Stripe Alipay".to_string(),
                    "wechat_pay" => "Stripe WeChat Pay".to_string(),
                    "link" => "Stripe Link".to_string(),
                    _ => "Stripe Card".to_string(),
                },
                _ => "支付".to_string(),
            };
            (
                payment_channel,
                display_name,
                record.pay_currency,
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
        let Some(callback_base_url) =
            epay_callback_base_url(callback_base_url.as_deref(), headers, request_context)
        else {
            return build_auth_error_response(
                http::StatusCode::BAD_REQUEST,
                "支付网关 callback_base_url is required",
                false,
            );
        };
        let direct_input = DirectPaymentCheckoutInput {
            payment_channel: payment_channel.clone(),
            display_name,
            order_no: order_no.clone(),
            subject: plan.title.clone(),
            pay_amount,
            pay_currency: pay_currency.clone(),
            notify_url: format!("{callback_base_url}/api/payment/{requested_provider}/notify"),
            return_url: Some(format!("{callback_base_url}/dashboard/billing")),
            client_ip: direct_payment_client_ip(headers),
            expires_at,
        };
        let checkout = match requested_provider {
            "alipay" => match create_alipay_direct_checkout(state, &direct_input).await {
                Ok(value) => value,
                Err(detail) => {
                    return build_auth_error_response(http::StatusCode::BAD_GATEWAY, detail, false)
                }
            },
            "wxpay" => match create_wxpay_direct_checkout(state, &direct_input).await {
                Ok(value) => value,
                Err(detail) => {
                    return build_auth_error_response(http::StatusCode::BAD_GATEWAY, detail, false)
                }
            },
            "stripe" => match create_stripe_direct_checkout(state, &direct_input).await {
                Ok(value) => value,
                Err(detail) => {
                    return build_auth_error_response(http::StatusCode::BAD_GATEWAY, detail, false)
                }
            },
            _ => {
                return build_auth_error_response(
                    http::StatusCode::BAD_REQUEST,
                    "unsupported payment provider",
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
                    pay_currency: pay_currency.clone(),
                    exchange_rate: usd_exchange_rate,
                    payment_method,
                    payment_provider: Some(requested_provider.to_string()),
                    payment_channel: Some(payment_channel.clone()),
                    gateway_order_id: checkout
                        .get("gateway_order_id")
                        .and_then(Value::as_str)
                        .unwrap_or(&order_no)
                        .to_string(),
                    gateway_response: checkout.clone(),
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
                return build_auth_error_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("billing checkout create failed: {err:?}"),
                    false,
                )
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
        build_auth_json_response(
            http::StatusCode::OK,
            json!({
                "order": order,
                "payment_instructions": sanitize_wallet_gateway_response(Some(checkout)),
            }),
            None,
        )
    }
}

pub(super) async fn maybe_build_local_billing_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
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
        Some("plan_checkout") => {
            Some(handle_billing_plan_checkout(state, request_context, headers, request_body).await)
        }
        Some("entitlements") if request_context.request_path == "/api/billing/entitlements" => {
            Some(handle_billing_entitlements(state, request_context, headers).await)
        }
        _ => None,
    }
}
