use super::super::support_payment::payment_epay::{
    build_epay_checkout_url, configured_epay_channels, epay_callback_base_url, load_epay_config,
    resolve_epay_channel, EpayCheckoutInput,
};
use super::super::support_payment::payment_gateway::{
    CreateCheckoutSessionInput, PaymentGatewayRegistry,
};
use super::{
    build_auth_error_response, build_auth_json_response, build_wallet_payload,
    build_wallet_recharge_storage_unavailable_response, http, parse_wallet_limit,
    parse_wallet_offset, resolve_authenticated_local_user, unix_secs_to_rfc3339,
    wallet_normalize_optional_string_field, AppState, Body, GatewayPublicRequestContext, Response,
    WALLET_SAFE_GATEWAY_RESPONSE_KEYS,
};
#[cfg(test)]
use super::{
    record_wallet_test_recharge, wallet_test_recharge_order_by_id,
    wallet_test_recharge_orders_for_user,
};
use crate::handlers::shared::{
    create_alipay_direct_checkout, create_wxpay_direct_checkout, direct_payment_client_ip,
    DirectPaymentCheckoutInput,
};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

#[derive(Debug, Deserialize)]
struct WalletCreateRechargeRequest {
    amount_usd: f64,
    payment_method: String,
    #[serde(default)]
    payment_provider: Option<String>,
    #[serde(default)]
    payment_channel: Option<String>,
    #[serde(default)]
    pay_amount: Option<f64>,
    #[serde(default)]
    pay_currency: Option<String>,
    #[serde(default)]
    exchange_rate: Option<f64>,
}

#[derive(Debug, Clone)]
struct NormalizedWalletCreateRechargeRequest {
    amount_usd: f64,
    payment_method: String,
    payment_provider: Option<String>,
    payment_channel: Option<String>,
    pay_amount: Option<f64>,
    pay_currency: Option<String>,
    exchange_rate: Option<f64>,
}

fn normalize_wallet_create_recharge_request(
    payload: WalletCreateRechargeRequest,
) -> Result<NormalizedWalletCreateRechargeRequest, &'static str> {
    if !payload.amount_usd.is_finite() || payload.amount_usd <= 0.0 {
        return Err("输入验证失败");
    }
    let payment_method = payload.payment_method.trim().to_ascii_lowercase();
    if payment_method.is_empty() || payment_method.chars().count() > 30 {
        return Err("输入验证失败");
    }
    let payment_provider = wallet_normalize_optional_string_field(payload.payment_provider, 30)?
        .map(|value| value.to_ascii_lowercase());
    let payment_channel = wallet_normalize_optional_string_field(payload.payment_channel, 30)?
        .map(|value| value.to_ascii_lowercase());
    if matches!(payload.pay_amount, Some(value) if !value.is_finite() || value <= 0.0) {
        return Err("输入验证失败");
    }
    if matches!(payload.exchange_rate, Some(value) if !value.is_finite() || value <= 0.0) {
        return Err("输入验证失败");
    }
    let pay_currency = wallet_normalize_optional_string_field(payload.pay_currency, 3)?;
    if matches!(pay_currency.as_deref(), Some(value) if value.chars().count() != 3) {
        return Err("输入验证失败");
    }

    Ok(NormalizedWalletCreateRechargeRequest {
        amount_usd: payload.amount_usd,
        payment_method,
        payment_provider,
        payment_channel,
        pay_amount: payload.pay_amount,
        pay_currency,
        exchange_rate: payload.exchange_rate,
    })
}

fn wallet_build_order_no(now: chrono::DateTime<chrono::Utc>) -> String {
    format!(
        "po_{}_{}",
        now.format("%Y%m%d%H%M%S%6f"),
        &Uuid::new_v4().simple().to_string()[..12]
    )
}

fn wallet_payment_return_url(callback_base_url: &str, provider: &str, order_no: &str) -> String {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    serializer.append_pair("payment_provider", provider);
    serializer.append_pair("payment_status", "pending");
    serializer.append_pair("order_no", order_no);
    format!(
        "{callback_base_url}/dashboard/wallet?{}",
        serializer.finish()
    )
}

fn wallet_order_id_from_path(request_path: &str) -> Option<String> {
    let trimmed = request_path.trim_end_matches('/');
    let order_id = trimmed.strip_prefix("/api/wallet/recharge/")?.trim();
    if order_id.is_empty() || order_id.contains('/') {
        None
    } else {
        Some(order_id.to_string())
    }
}

pub(super) fn wallet_recharge_detail_path_matches(request_path: &str) -> bool {
    wallet_order_id_from_path(request_path).is_some()
}

pub(crate) fn sanitize_wallet_gateway_response(
    value: Option<serde_json::Value>,
) -> serde_json::Value {
    let Some(value) = value else {
        return json!({});
    };
    let Some(object) = value.as_object() else {
        return json!({});
    };
    let mut sanitized = serde_json::Map::new();
    for key in WALLET_SAFE_GATEWAY_RESPONSE_KEYS {
        if let Some(item) = object.get(*key) {
            sanitized.insert((*key).to_string(), item.clone());
        }
    }
    serde_json::Value::Object(sanitized)
}

fn build_wallet_payment_order_payload(
    id: String,
    order_no: String,
    wallet_id: String,
    user_id: Option<String>,
    amount_usd: f64,
    pay_amount: Option<f64>,
    pay_currency: Option<String>,
    exchange_rate: Option<f64>,
    refunded_amount_usd: f64,
    refundable_amount_usd: f64,
    payment_method: String,
    gateway_order_id: Option<String>,
    gateway_response: Option<serde_json::Value>,
    status: String,
    created_at: Option<String>,
    paid_at: Option<String>,
    credited_at: Option<String>,
    expires_at: Option<String>,
) -> serde_json::Value {
    json!({
        "id": id,
        "order_no": order_no,
        "wallet_id": wallet_id,
        "user_id": user_id,
        "amount_usd": amount_usd,
        "pay_amount": pay_amount,
        "pay_currency": pay_currency,
        "exchange_rate": exchange_rate,
        "refunded_amount_usd": refunded_amount_usd,
        "refundable_amount_usd": refundable_amount_usd,
        "payment_method": payment_method,
        "gateway_order_id": gateway_order_id,
        "gateway_response": sanitize_wallet_gateway_response(gateway_response),
        "status": status,
        "created_at": created_at,
        "paid_at": paid_at,
        "credited_at": credited_at,
        "expires_at": expires_at,
    })
}

fn wallet_payment_order_payload_from_record(
    record: &aether_data::repository::wallet::StoredAdminPaymentOrder,
) -> serde_json::Value {
    build_wallet_payment_order_payload(
        record.id.clone(),
        record.order_no.clone(),
        record.wallet_id.clone(),
        record.user_id.clone(),
        record.amount_usd,
        record.pay_amount,
        record.pay_currency.clone(),
        record.exchange_rate,
        record.refunded_amount_usd,
        record.refundable_amount_usd,
        record.payment_method.clone(),
        record.gateway_order_id.clone(),
        record.gateway_response.clone(),
        record.status.clone(),
        Some(unix_secs_to_rfc3339(record.created_at_unix_ms)).flatten(),
        record.paid_at_unix_secs.and_then(unix_secs_to_rfc3339),
        record.credited_at_unix_secs.and_then(unix_secs_to_rfc3339),
        record.expires_at_unix_secs.and_then(unix_secs_to_rfc3339),
    )
}

#[derive(Debug, Clone)]
pub(crate) struct DirectGatewayChannelConfig {
    pub(crate) channel: String,
    pub(crate) display_name: String,
    pub(crate) fee_rate: f64,
}

fn configured_channel_fee_rate(value: Option<&Value>) -> f64 {
    let fee_rate = match value {
        Some(Value::Number(number)) => number.as_f64().unwrap_or(0.0),
        Some(Value::String(value)) => value.trim().parse::<f64>().unwrap_or(0.0),
        _ => 0.0,
    };
    if fee_rate.is_finite() && fee_rate >= 0.0 {
        fee_rate
    } else {
        0.0
    }
}

fn round_payment_amount(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn wallet_recharge_payment_breakdown(
    amount_usd: f64,
    usd_exchange_rate: f64,
    fee_rate: f64,
) -> (f64, f64, f64) {
    let safe_fee_rate = if fee_rate.is_finite() && fee_rate > 0.0 {
        fee_rate
    } else {
        0.0
    };
    let base_pay_amount = round_payment_amount(amount_usd * usd_exchange_rate);
    let fee_amount = round_payment_amount(base_pay_amount * safe_fee_rate / 100.0);
    let pay_amount = round_payment_amount(base_pay_amount + fee_amount);
    (base_pay_amount, fee_amount, pay_amount)
}

fn add_wallet_recharge_fee_metadata(
    mut checkout: Value,
    base_pay_amount: f64,
    fee_rate: f64,
    fee_amount: f64,
) -> Value {
    if let Some(object) = checkout.as_object_mut() {
        object.insert("base_pay_amount".to_string(), json!(base_pay_amount));
        object.insert("fee_rate".to_string(), json!(fee_rate));
        object.insert("fee_amount".to_string(), json!(fee_amount));
    }
    checkout
}

pub(crate) fn direct_gateway_channels(
    provider: &str,
    record: &aether_data_contracts::repository::billing::PaymentGatewayConfigRecord,
) -> Vec<DirectGatewayChannelConfig> {
    let channels_value =
        crate::handlers::shared::payment_gateway_channels_json(&record.channels_json);
    let channels = channels_value.as_array().into_iter().flatten();
    channels
        .filter_map(|channel| {
            let channel_id = channel
                .get("channel")
                .or_else(|| channel.get("type"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())?;
            let display_name = channel
                .get("display_name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(channel_id);
            Some(DirectGatewayChannelConfig {
                channel: channel_id.to_string(),
                display_name: display_name.to_string(),
                fee_rate: configured_channel_fee_rate(channel.get("fee_rate")),
            })
        })
        .filter(|channel| match provider {
            "alipay" => channel.channel == "alipay",
            "wxpay" => matches!(channel.channel.as_str(), "native" | "h5" | "jsapi"),
            "stripe" => matches!(
                channel.channel.as_str(),
                "card" | "alipay" | "wechat_pay" | "link"
            ),
            _ => false,
        })
        .collect()
}

fn resolve_direct_gateway_channel(
    provider: &str,
    record: &aether_data_contracts::repository::billing::PaymentGatewayConfigRecord,
    requested: Option<&str>,
) -> Result<DirectGatewayChannelConfig, String> {
    let channels = direct_gateway_channels(provider, record);
    if channels.is_empty() {
        return Err("支付网关没有可用通道".to_string());
    }
    let Some(requested) = requested.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(channels[0].clone());
    };
    channels
        .into_iter()
        .find(|channel| channel.channel.eq_ignore_ascii_case(requested))
        .ok_or_else(|| "支付通道不可用".to_string())
}

fn direct_gateway_public_config_string(
    record: &aether_data_contracts::repository::billing::PaymentGatewayConfigRecord,
    key: &str,
) -> Option<String> {
    crate::handlers::shared::payment_gateway_config_json(&record.channels_json)
        .as_object()?
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn decrypt_direct_gateway_secrets(
    state: &AppState,
    record: &aether_data_contracts::repository::billing::PaymentGatewayConfigRecord,
) -> Result<serde_json::Map<String, Value>, String> {
    let Some(encrypted) = record.merchant_key_encrypted.as_deref() else {
        return Err("支付网关密钥未配置".to_string());
    };
    let Some(plaintext) = crate::handlers::shared::decrypt_catalog_secret_with_fallbacks(
        state.encryption_key(),
        encrypted,
    ) else {
        return Err("支付网关密钥解密失败".to_string());
    };
    serde_json::from_str::<Value>(&plaintext)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .ok_or_else(|| "支付网关密钥格式无效".to_string())
}

fn direct_gateway_secret_string(
    secrets: &serde_json::Map<String, Value>,
    key: &str,
) -> Option<String> {
    secrets
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn stripe_minor_unit_amount(pay_amount: f64, pay_currency: &str) -> Result<i64, String> {
    let currency = pay_currency.trim().to_ascii_lowercase();
    let multiplier = match currency.as_str() {
        "bif" | "clp" | "djf" | "gnf" | "jpy" | "kmf" | "krw" | "mga" | "pyg" | "rwf" | "ugx"
        | "vnd" | "vuv" | "xaf" | "xof" | "xpf" => 1.0,
        _ => 100.0,
    };
    let amount = (pay_amount * multiplier).round();
    if !amount.is_finite() || amount <= 0.0 {
        return Err("Stripe 支付金额无效".to_string());
    }
    Ok(amount as i64)
}

async fn create_stripe_wallet_recharge_checkout(
    state: &AppState,
    record: &aether_data_contracts::repository::billing::PaymentGatewayConfigRecord,
    payment_channel: &str,
    display_name: &str,
    order_no: &str,
    pay_amount: f64,
    expires_at: chrono::DateTime<chrono::Utc>,
) -> Result<Value, String> {
    let secrets = decrypt_direct_gateway_secrets(state, record)?;
    let Some(secret_key) = direct_gateway_secret_string(&secrets, "secret_key") else {
        return Err("Stripe secret_key 未配置".to_string());
    };
    let Some(publishable_key) = direct_gateway_public_config_string(record, "publishable_key")
    else {
        return Err("Stripe publishable_key 未配置".to_string());
    };
    let amount = stripe_minor_unit_amount(pay_amount, &record.pay_currency)?;
    let currency = record.pay_currency.trim().to_ascii_lowercase();
    let mut form = vec![
        ("amount".to_string(), amount.to_string()),
        ("currency".to_string(), currency.clone()),
        ("description".to_string(), "钱包充值".to_string()),
        ("metadata[order_no]".to_string(), order_no.to_string()),
        (
            "metadata[payment_provider]".to_string(),
            "stripe".to_string(),
        ),
        (
            "metadata[payment_channel]".to_string(),
            payment_channel.to_string(),
        ),
        (
            "payment_method_types[]".to_string(),
            payment_channel.to_string(),
        ),
    ];
    if payment_channel == "wechat_pay" {
        form.push((
            "payment_method_options[wechat_pay][client]".to_string(),
            "web".to_string(),
        ));
    }
    let response = state
        .client
        .post("https://api.stripe.com/v1/payment_intents")
        .basic_auth(secret_key, Some(""))
        .form(&form)
        .send()
        .await
        .map_err(|err| format!("Stripe PaymentIntent 创建失败: {err}"))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|err| format!("Stripe 响应读取失败: {err}"))?;
    let value =
        serde_json::from_str::<Value>(&body).map_err(|_| "Stripe 响应格式无效".to_string())?;
    if !status.is_success() {
        let message = value
            .get("error")
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("Stripe PaymentIntent 创建失败");
        return Err(message.to_string());
    }
    let Some(intent_id) = value.get("id").and_then(Value::as_str) else {
        return Err("Stripe 响应缺少 PaymentIntent ID".to_string());
    };
    let Some(client_secret) = value.get("client_secret").and_then(Value::as_str) else {
        return Err("Stripe 响应缺少 client_secret".to_string());
    };
    Ok(json!({
        "gateway": "stripe",
        "display_name": display_name,
        "gateway_order_id": intent_id,
        "intent_id": intent_id,
        "client_secret": client_secret,
        "publishable_key": publishable_key,
        "expires_at": expires_at.to_rfc3339(),
        "pay_amount": pay_amount,
        "pay_currency": record.pay_currency,
        "payment_channel": payment_channel,
        "payment_method_types": [payment_channel],
        "submit_method": "stripe_payment_intent"
    }))
}

pub(super) async fn handle_wallet_create_recharge(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
    request_body: Option<&axum::body::Bytes>,
) -> Response<Body> {
    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(request_body) = request_body else {
        return build_auth_error_response(http::StatusCode::BAD_REQUEST, "缺少请求体", false);
    };
    let payload = match serde_json::from_slice::<WalletCreateRechargeRequest>(request_body) {
        Ok(value) => value,
        Err(_) => {
            return build_auth_error_response(http::StatusCode::BAD_REQUEST, "输入验证失败", false)
        }
    };
    let payload = match normalize_wallet_create_recharge_request(payload) {
        Ok(value) => value,
        Err(detail) => {
            return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false)
        }
    };
    if payload.payment_method == "admin_manual" {
        return build_auth_error_response(
            http::StatusCode::BAD_REQUEST,
            "admin_manual is reserved for admin recharge",
            false,
        );
    }

    let wallet = match state
        .find_wallet(aether_data::repository::wallet::WalletLookupKey::UserId(
            &auth.user.id,
        ))
        .await
    {
        Ok(value) => value,
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("wallet lookup failed: {err:?}"),
                false,
            )
        }
    };

    if !state.has_database_wallet_data_writer() {
        #[cfg(test)]
        {
            let Some(wallet) = wallet else {
                return build_auth_error_response(
                    http::StatusCode::BAD_REQUEST,
                    "wallet not available",
                    false,
                );
            };
            if wallet.status != "active" {
                return build_auth_error_response(
                    http::StatusCode::BAD_REQUEST,
                    "wallet is not active",
                    false,
                );
            }
            let now = Utc::now();
            let order_id = Uuid::new_v4().to_string();
            let order_no = wallet_build_order_no(now);
            let expires_at = now + chrono::Duration::minutes(30);
            let Some(adapter) = PaymentGatewayRegistry::get(&payload.payment_method) else {
                return build_auth_error_response(
                    http::StatusCode::BAD_REQUEST,
                    format!("unsupported payment_method: {}", payload.payment_method),
                    false,
                );
            };
            let checkout = match adapter.create_checkout_session(&CreateCheckoutSessionInput {
                order_no: order_no.clone(),
                amount_usd: payload.amount_usd,
                expires_at,
            }) {
                Ok(value) => value,
                Err(detail) => {
                    return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false);
                }
            };
            let order_payload = build_wallet_payment_order_payload(
                order_id,
                order_no,
                wallet.id.clone(),
                Some(auth.user.id.clone()),
                payload.amount_usd,
                payload.pay_amount,
                payload.pay_currency.clone(),
                payload.exchange_rate,
                0.0,
                0.0,
                payload.payment_method,
                Some(checkout.gateway_order_id.clone()),
                Some(checkout.gateway_response.clone()),
                "pending".to_string(),
                Some(now.to_rfc3339()),
                None,
                None,
                Some(expires_at.to_rfc3339()),
            );
            record_wallet_test_recharge(auth.user.id, order_payload.clone());
            return build_auth_json_response(
                http::StatusCode::OK,
                json!({
                    "order": order_payload,
                    "payment_instructions": sanitize_wallet_gateway_response(Some(checkout.gateway_response)),
                }),
                None,
            );
        }
        #[cfg(not(test))]
        return build_wallet_recharge_storage_unavailable_response();
    }

    let now = Utc::now();
    let order_no = wallet_build_order_no(now);
    let expires_at = now + chrono::Duration::minutes(30);
    let uses_epay =
        payload.payment_provider.as_deref() == Some("epay") || payload.payment_method == "epay";
    if uses_epay {
        let config = match load_epay_config(state).await {
            Ok(value) => value,
            Err(detail) => {
                return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false);
            }
        };
        if payload.amount_usd < config.min_recharge_usd {
            return build_auth_error_response(
                http::StatusCode::BAD_REQUEST,
                "充值金额低于支付网关最小金额",
                false,
            );
        }
        let requested_channel = payload.payment_channel.as_deref().or_else(|| {
            (payload.payment_method != "epay").then_some(payload.payment_method.as_str())
        });
        let payment_channel = match resolve_epay_channel(&config, requested_channel) {
            Ok(value) => value,
            Err(detail) => {
                return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false);
            }
        };
        let (base_pay_amount, fee_amount, pay_amount) = wallet_recharge_payment_breakdown(
            payload.amount_usd,
            config.usd_exchange_rate,
            payment_channel.fee_rate,
        );
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
                channel: payment_channel.channel.clone(),
                subject: "钱包充值".to_string(),
                pay_amount,
                notify_url: format!("{callback_base_url}/api/payment/epay/notify"),
                return_url: format!("{callback_base_url}/api/payment/epay/return"),
            },
        );
        let checkout = add_wallet_recharge_fee_metadata(
            checkout,
            base_pay_amount,
            payment_channel.fee_rate,
            fee_amount,
        );
        let outcome = match state
            .create_wallet_recharge_order(
                aether_data::repository::wallet::CreateWalletRechargeOrderInput {
                    preferred_wallet_id: wallet.as_ref().map(|value| value.id.clone()),
                    user_id: auth.user.id.clone(),
                    amount_usd: payload.amount_usd,
                    pay_amount: Some(pay_amount),
                    pay_currency: Some(config.pay_currency.clone()),
                    exchange_rate: Some(config.usd_exchange_rate),
                    payment_method: "epay".to_string(),
                    payment_provider: Some("epay".to_string()),
                    payment_channel: Some(payment_channel.channel),
                    gateway_order_id: order_no.clone(),
                    gateway_response: checkout.clone(),
                    order_no,
                    expires_at_unix_secs: expires_at.timestamp().max(0) as u64,
                },
            )
            .await
        {
            Ok(Some(value)) => value,
            Ok(None) => return build_wallet_recharge_storage_unavailable_response(),
            Err(err) => {
                return build_auth_error_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("wallet recharge create failed: {err:?}"),
                    false,
                )
            }
        };
        let order_payload = match outcome {
            aether_data::repository::wallet::CreateWalletRechargeOrderOutcome::Created(order) => {
                wallet_payment_order_payload_from_record(&order)
            }
            aether_data::repository::wallet::CreateWalletRechargeOrderOutcome::WalletInactive => {
                return build_auth_error_response(
                    http::StatusCode::BAD_REQUEST,
                    "wallet is not active",
                    false,
                )
            }
        };
        return build_auth_json_response(
            http::StatusCode::OK,
            json!({
                "order": order_payload,
                "payment_instructions": sanitize_wallet_gateway_response(Some(checkout)),
            }),
            None,
        );
    }
    let requested_provider = payload
        .payment_provider
        .as_deref()
        .unwrap_or(payload.payment_method.as_str());
    if matches!(requested_provider, "alipay" | "wxpay" | "stripe") {
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
        if payload.amount_usd < record.min_recharge_usd {
            return build_auth_error_response(
                http::StatusCode::BAD_REQUEST,
                "充值金额低于支付网关最小金额",
                false,
            );
        }
        let payment_channel = match resolve_direct_gateway_channel(
            requested_provider,
            &record,
            payload.payment_channel.as_deref(),
        ) {
            Ok(value) => value,
            Err(detail) => {
                return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false)
            }
        };
        let (base_pay_amount, fee_amount, pay_amount) = wallet_recharge_payment_breakdown(
            payload.amount_usd,
            record.usd_exchange_rate,
            payment_channel.fee_rate,
        );
        let checkout = if requested_provider == "stripe" {
            match create_stripe_wallet_recharge_checkout(
                state,
                &record,
                &payment_channel.channel,
                &payment_channel.display_name,
                &order_no,
                pay_amount,
                expires_at,
            )
            .await
            {
                Ok(value) => value,
                Err(detail) => {
                    return build_auth_error_response(http::StatusCode::BAD_GATEWAY, detail, false)
                }
            }
        } else {
            let Some(callback_base_url) = epay_callback_base_url(
                record.callback_base_url.as_deref(),
                headers,
                request_context,
            ) else {
                return build_auth_error_response(
                    http::StatusCode::BAD_REQUEST,
                    "支付网关 callback_base_url is required",
                    false,
                );
            };
            let direct_input = DirectPaymentCheckoutInput {
                payment_channel: payment_channel.channel.clone(),
                display_name: payment_channel.display_name.clone(),
                order_no: order_no.clone(),
                subject: "钱包充值".to_string(),
                pay_amount,
                pay_currency: record.pay_currency.clone(),
                notify_url: format!("{callback_base_url}/api/payment/{requested_provider}/notify"),
                return_url: Some(wallet_payment_return_url(
                    &callback_base_url,
                    requested_provider,
                    &order_no,
                )),
                client_ip: direct_payment_client_ip(headers),
                expires_at,
            };
            let result = match requested_provider {
                "alipay" => create_alipay_direct_checkout(state, &direct_input).await,
                "wxpay" => create_wxpay_direct_checkout(state, &direct_input).await,
                _ => Err("支付网关不支持".to_string()),
            };
            match result {
                Ok(value) => value,
                Err(detail) => {
                    return build_auth_error_response(http::StatusCode::BAD_GATEWAY, detail, false)
                }
            }
        };
        let checkout = add_wallet_recharge_fee_metadata(
            checkout,
            base_pay_amount,
            payment_channel.fee_rate,
            fee_amount,
        );
        let gateway_order_id = checkout
            .get("gateway_order_id")
            .and_then(Value::as_str)
            .unwrap_or(&order_no)
            .to_string();
        let outcome = match state
            .create_wallet_recharge_order(
                aether_data::repository::wallet::CreateWalletRechargeOrderInput {
                    preferred_wallet_id: wallet.as_ref().map(|value| value.id.clone()),
                    user_id: auth.user.id.clone(),
                    amount_usd: payload.amount_usd,
                    pay_amount: Some(pay_amount),
                    pay_currency: Some(record.pay_currency.clone()),
                    exchange_rate: Some(record.usd_exchange_rate),
                    payment_method: requested_provider.to_string(),
                    payment_provider: Some(requested_provider.to_string()),
                    payment_channel: Some(payment_channel.channel),
                    gateway_order_id,
                    gateway_response: checkout.clone(),
                    order_no,
                    expires_at_unix_secs: expires_at.timestamp().max(0) as u64,
                },
            )
            .await
        {
            Ok(Some(value)) => value,
            Ok(None) => return build_wallet_recharge_storage_unavailable_response(),
            Err(err) => {
                return build_auth_error_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("wallet recharge create failed: {err:?}"),
                    false,
                )
            }
        };
        let order_payload = match outcome {
            aether_data::repository::wallet::CreateWalletRechargeOrderOutcome::Created(order) => {
                wallet_payment_order_payload_from_record(&order)
            }
            aether_data::repository::wallet::CreateWalletRechargeOrderOutcome::WalletInactive => {
                return build_auth_error_response(
                    http::StatusCode::BAD_REQUEST,
                    "wallet is not active",
                    false,
                )
            }
        };
        return build_auth_json_response(
            http::StatusCode::OK,
            json!({
                "order": order_payload,
                "payment_instructions": sanitize_wallet_gateway_response(Some(checkout)),
            }),
            None,
        );
    }
    build_auth_error_response(
        http::StatusCode::BAD_REQUEST,
        format!("unsupported payment_method: {}", payload.payment_method),
        false,
    )
}

pub(super) async fn handle_wallet_recharge_options(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    if let Err(response) = resolve_authenticated_local_user(state, request_context, headers).await {
        return response;
    }
    let mut methods = Vec::new();
    if let Ok(config) = load_epay_config(state).await {
        for channel in configured_epay_channels(&config) {
            let payment_channel = channel.channel.clone();
            let display_name = channel.display_name.clone();
            let fee_rate = channel.fee_rate;
            methods.push(json!({
                "payment_method": "epay",
                "payment_provider": "epay",
                "payment_channel": payment_channel,
                "display_name": display_name,
                "pay_currency": config.pay_currency,
                "usd_exchange_rate": config.usd_exchange_rate,
                "min_recharge_usd": config.min_recharge_usd,
                "fee_rate": fee_rate,
            }));
        }
    }
    for provider in ["alipay", "wxpay", "stripe"] {
        let Ok(Some(record)) = state.find_payment_gateway_config(provider).await else {
            continue;
        };
        if !record.enabled || record.merchant_key_encrypted.is_none() {
            continue;
        }
        for DirectGatewayChannelConfig {
            channel: payment_channel,
            display_name,
            fee_rate,
        } in direct_gateway_channels(provider, &record)
        {
            methods.push(json!({
                "payment_method": provider,
                "payment_provider": provider,
                "payment_channel": payment_channel,
                "display_name": display_name,
                "pay_currency": record.pay_currency,
                "usd_exchange_rate": record.usd_exchange_rate,
                "min_recharge_usd": record.min_recharge_usd,
                "fee_rate": fee_rate,
            }));
        }
    }
    build_auth_json_response(http::StatusCode::OK, json!({ "items": methods }), None)
}

pub(super) async fn handle_wallet_recharge_list(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let query = request_context.request_query_string.as_deref();
    let limit = match parse_wallet_limit(query) {
        Ok(value) => value,
        Err(detail) => {
            return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false)
        }
    };
    let offset = match parse_wallet_offset(query) {
        Ok(value) => value,
        Err(detail) => {
            return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false)
        }
    };
    let wallet = match state
        .find_wallet(aether_data::repository::wallet::WalletLookupKey::UserId(
            &auth.user.id,
        ))
        .await
    {
        Ok(value) => value,
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("wallet lookup failed: {err:?}"),
                false,
            )
        }
    };

    let (items, total) = match state
        .list_wallet_payment_orders_by_user_id(&auth.user.id, limit, offset)
        .await
    {
        Ok(page) => (
            page.items
                .iter()
                .map(wallet_payment_order_payload_from_record)
                .collect::<Vec<_>>(),
            page.total,
        ),
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("wallet recharge lookup failed: {err:?}"),
                false,
            )
        }
    };
    #[cfg(test)]
    let (items, total) =
        if !state.has_database_wallet_data_writer() && items.is_empty() && total == 0 {
            wallet_test_recharge_orders_for_user(&auth.user.id, limit, offset)
        } else {
            (items, total)
        };

    let mut payload = json!({
        "items": items,
        "total": total,
        "limit": limit,
        "offset": offset,
    });
    if let Some(object) = payload.as_object_mut() {
        if let Some(wallet_payload) = build_wallet_payload(wallet.as_ref()).as_object() {
            object.extend(wallet_payload.clone());
        }
    }
    build_auth_json_response(http::StatusCode::OK, payload, None)
}

pub(super) async fn handle_wallet_recharge_detail(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(order_id) = wallet_order_id_from_path(&request_context.request_path) else {
        return build_auth_error_response(
            http::StatusCode::NOT_FOUND,
            "Payment order not found",
            false,
        );
    };
    match state
        .find_wallet_payment_order_by_user_id(&auth.user.id, &order_id)
        .await
    {
        Ok(Some(order)) => build_auth_json_response(
            http::StatusCode::OK,
            json!({ "order": wallet_payment_order_payload_from_record(&order) }),
            None,
        ),
        Ok(None) => {
            #[cfg(test)]
            if let Some(order) = wallet_test_recharge_order_by_id(&auth.user.id, &order_id) {
                return build_auth_json_response(
                    http::StatusCode::OK,
                    json!({ "order": order }),
                    None,
                );
            }
            build_auth_error_response(
                http::StatusCode::NOT_FOUND,
                "Payment order not found",
                false,
            )
        }
        Err(err) => build_auth_error_response(
            http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("wallet recharge detail lookup failed: {err:?}"),
            false,
        ),
    }
}
