use std::collections::BTreeMap;

use axum::{body::Body, http, response::Response};
use hmac::{Hmac, Mac};
use md5::{Digest, Md5};
use serde_json::json;
use sha2::Sha256;
use tracing::warn;

use super::{payment_shared::payment_callback_payload_hash, AppState, GatewayPublicRequestContext};

const MAX_EPAY_ORDER_NO_BYTES: usize = 64;
const MAX_EPAY_GATEWAY_ORDER_ID_BYTES: usize = 123;
const MAX_EPAY_CHANNEL_BYTES: usize = 64;

#[derive(Clone)]
pub(crate) struct EpayMerchantConfig {
    pub(crate) endpoint_url: String,
    pub(crate) callback_base_url: Option<String>,
    pub(crate) merchant_id: String,
    pub(crate) merchant_key: String,
    pub(crate) pay_currency: String,
    pub(crate) usd_exchange_rate: f64,
    pub(crate) min_recharge_usd: f64,
    pub(crate) channels: serde_json::Value,
}

impl std::fmt::Debug for EpayMerchantConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EpayMerchantConfig")
            .field("endpoint_url", &"[REDACTED]")
            .field(
                "callback_base_url",
                &self.callback_base_url.as_ref().map(|_| "[REDACTED]"),
            )
            .field("merchant_id", &self.merchant_id)
            .field("merchant_key", &"[REDACTED]")
            .field("pay_currency", &self.pay_currency)
            .field("usd_exchange_rate", &self.usd_exchange_rate)
            .field("min_recharge_usd", &self.min_recharge_usd)
            .field("channels", &"[REDACTED]")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct EpayChannelConfig {
    pub(crate) channel: String,
    pub(crate) display_name: String,
    pub(crate) fee_rate: f64,
}

#[derive(Debug, Clone)]
pub(crate) struct EpayCheckoutInput {
    pub(crate) order_no: String,
    pub(crate) channel: String,
    pub(crate) subject: String,
    pub(crate) pay_amount: f64,
    pub(crate) notify_url: String,
    pub(crate) return_url: String,
}

fn epay_channel_fee_rate(value: Option<&serde_json::Value>) -> f64 {
    let fee_rate = match value {
        Some(serde_json::Value::Number(number)) => number.as_f64().unwrap_or(0.0),
        Some(serde_json::Value::String(value)) => value.trim().parse::<f64>().unwrap_or(0.0),
        _ => 0.0,
    };
    if fee_rate.is_finite() && fee_rate >= 0.0 {
        fee_rate
    } else {
        0.0
    }
}

pub(crate) fn configured_epay_channels(config: &EpayMerchantConfig) -> Vec<EpayChannelConfig> {
    let channels_value = crate::handlers::shared::payment_gateway_channels_json(&config.channels);
    let Some(channels) = channels_value.as_array() else {
        return Vec::new();
    };
    channels
        .iter()
        .filter_map(|channel| {
            let channel_id = channel
                .get("channel")
                .or_else(|| channel.get("type"))
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())?;
            let display_name = channel
                .get("display_name")
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(channel_id)
                .to_string();
            Some(EpayChannelConfig {
                channel: channel_id.to_string(),
                display_name,
                fee_rate: epay_channel_fee_rate(channel.get("fee_rate")),
            })
        })
        .collect()
}

pub(crate) fn resolve_epay_channel(
    config: &EpayMerchantConfig,
    requested_channel: Option<&str>,
) -> Result<EpayChannelConfig, &'static str> {
    let channels = configured_epay_channels(config);
    if channels.is_empty() {
        return Err("支付网关未配置可用通道");
    }
    let requested_channel = requested_channel
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty());
    if let Some(requested_channel) = requested_channel {
        if let Some(channel) = channels
            .iter()
            .find(|channel| channel.channel.eq_ignore_ascii_case(&requested_channel))
        {
            return Ok(channel.clone());
        }
        return Err("支付通道未配置或已停用");
    }
    Ok(channels[0].clone())
}

pub(crate) fn epay_sign(params: &BTreeMap<String, String>, merchant_key: &str) -> String {
    let canonical = params
        .iter()
        .filter(|(key, value)| {
            key.as_str() != "sign" && key.as_str() != "sign_type" && !value.trim().is_empty()
        })
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("&");
    let mut hasher = Md5::new();
    hasher.update(canonical.as_bytes());
    hasher.update(merchant_key.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub(crate) fn epay_signature_valid(params: &BTreeMap<String, String>, merchant_key: &str) -> bool {
    let Some(sign) = params.get("sign") else {
        return false;
    };
    let expected = epay_sign(params, merchant_key);
    let provided = sign.trim().to_ascii_lowercase();
    let mut expected_mac = Hmac::<Sha256>::new_from_slice(b"aether-epay-signature-compare")
        .expect("static epay comparison key should be valid");
    expected_mac.update(expected.as_bytes());
    let expected_tag = expected_mac.finalize().into_bytes();
    let mut provided_mac = Hmac::<Sha256>::new_from_slice(b"aether-epay-signature-compare")
        .expect("static epay comparison key should be valid");
    provided_mac.update(provided.as_bytes());
    provided_mac.verify_slice(&expected_tag).is_ok()
}

fn epay_submit_url(endpoint_url: &str) -> Result<String, String> {
    let normalized =
        crate::handlers::shared::normalize_payment_https_url(endpoint_url, "endpoint_url")?;
    let mut url = url::Url::parse(&normalized)
        .map_err(|_| "endpoint_url must be an absolute HTTPS URL".to_string())?;
    let path = url.path();
    if path.is_empty() || path == "/" {
        url.set_path("submit.php");
    }
    Ok(url.to_string())
}

pub(crate) fn epay_callback_base_url(configured: Option<&str>) -> Option<String> {
    if let Some(configured) = configured {
        return crate::handlers::shared::normalize_payment_callback_base_url(configured).ok();
    }

    std::env::var("AETHER_PUBLIC_BASE_URL")
        .ok()
        .or_else(|| std::env::var("PUBLIC_BASE_URL").ok())
        .and_then(|value| crate::handlers::shared::normalize_payment_callback_base_url(&value).ok())
}

pub(crate) fn build_epay_checkout_url(
    config: &EpayMerchantConfig,
    input: &EpayCheckoutInput,
) -> Result<serde_json::Value, String> {
    let money = format!("{:.2}", input.pay_amount);
    let mut params = BTreeMap::new();
    params.insert("pid".to_string(), config.merchant_id.clone());
    params.insert("type".to_string(), input.channel.clone());
    params.insert("out_trade_no".to_string(), input.order_no.clone());
    params.insert("notify_url".to_string(), input.notify_url.clone());
    params.insert("return_url".to_string(), input.return_url.clone());
    params.insert("name".to_string(), input.subject.clone());
    params.insert("money".to_string(), money.clone());
    params.insert("sign_type".to_string(), "MD5".to_string());
    let sign = epay_sign(&params, &config.merchant_key);
    params.insert("sign".to_string(), sign);

    let payment_url = epay_submit_url(&config.endpoint_url)?;
    let payment_params = params
        .iter()
        .map(|(key, value)| (key.clone(), serde_json::Value::String(value.clone())))
        .collect::<serde_json::Map<_, _>>();
    Ok(json!({
        "gateway": "epay",
        "display_name": "易支付",
        "gateway_order_id": input.order_no,
        "payment_url": payment_url,
        "submit_method": "POST",
        "payment_params": serde_json::Value::Object(payment_params),
        "qr_code": serde_json::Value::Null,
        "pay_amount": input.pay_amount,
        "pay_currency": config.pay_currency,
        "payment_channel": input.channel,
    }))
}

pub(crate) fn parse_epay_params(
    query: Option<&str>,
    body: Option<&axum::body::Bytes>,
) -> BTreeMap<String, String> {
    let raw = body
        .filter(|bytes| !bytes.is_empty())
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .or(query)
        .unwrap_or("");
    url::form_urlencoded::parse(raw.as_bytes())
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect()
}

fn epay_callback_projection(
    _params: &BTreeMap<String, String>,
    order_no: &str,
    gateway_order_id: Option<&str>,
    pay_amount: f64,
    pay_currency: &str,
    payment_channel: Option<&str>,
) -> serde_json::Value {
    json!({
        "gateway": "epay",
        "event_id": gateway_order_id,
        "gateway_order_id": gateway_order_id,
        "order_no": order_no,
        "amount": pay_amount,
        "currency": pay_currency,
        "payment_channel": payment_channel,
        "status": "success",
        "signature_valid": true,
    })
}

fn bounded_epay_callback_value(value: Option<&String>, max_bytes: usize) -> Option<String> {
    let value = value?.trim();
    (!value.is_empty()
        && value.len() <= max_bytes
        && !value.bytes().any(|byte| byte.is_ascii_control()))
    .then(|| value.to_string())
}

pub(crate) async fn load_epay_config(state: &AppState) -> Result<EpayMerchantConfig, String> {
    let Some(record) = state
        .find_payment_gateway_config("epay")
        .await
        .map_err(|_| "epay config lookup failed".to_string())?
    else {
        return Err("epay is not configured".to_string());
    };
    if !record.enabled {
        return Err("epay is disabled".to_string());
    }
    let pay_currency = crate::handlers::shared::normalize_payment_currency(
        &record.pay_currency,
        "epay pay_currency",
    )
    .map_err(|_| "epay pay_currency is invalid".to_string())?;
    let usd_exchange_rate = crate::handlers::shared::effective_payment_exchange_rate(
        &pay_currency,
        record.usd_exchange_rate,
    )
    .map_err(|_| "epay usd_exchange_rate is invalid".to_string())?;
    if !record.min_recharge_usd.is_finite() || record.min_recharge_usd < 0.0 {
        return Err("epay min_recharge_usd is invalid".to_string());
    }
    let endpoint_url =
        crate::handlers::shared::normalize_payment_https_url(&record.endpoint_url, "endpoint_url")?;
    let callback_base_url = record
        .callback_base_url
        .as_deref()
        .map(crate::handlers::shared::normalize_payment_callback_base_url)
        .transpose()?;
    let Some(encrypted_key) = record.merchant_key_encrypted.as_deref() else {
        return Err("epay merchant key is missing".to_string());
    };
    let binding = crate::handlers::shared::PaymentGatewaySecretBinding::from_record(&record)
        .map_err(|_| "epay merchant key binding is invalid".to_string())?;
    let merchant_key =
        crate::handlers::shared::open_payment_gateway_secret(state, &binding, encrypted_key)
            .map_err(|_| "epay merchant key decrypt failed".to_string())?
            .plaintext;
    Ok(EpayMerchantConfig {
        endpoint_url,
        callback_base_url,
        merchant_id: record.merchant_id,
        merchant_key,
        pay_currency,
        usd_exchange_rate,
        min_recharge_usd: record.min_recharge_usd,
        channels: record.channels_json,
    })
}

/// Callback authentication must continue to work for an order created before
/// an administrator disabled EPay or changed checkout pricing. Only the
/// merchant identity and signing secret are needed to authenticate a notify;
/// settlement values are resolved from the stored payment order below.
pub(crate) async fn load_epay_callback_config(
    state: &AppState,
) -> Result<EpayMerchantConfig, String> {
    let Some(record) = state
        .find_payment_gateway_config("epay")
        .await
        .map_err(|_| "epay config lookup failed".to_string())?
    else {
        return Err("epay is not configured".to_string());
    };
    let Some(encrypted_key) = record.merchant_key_encrypted.as_deref() else {
        return Err("epay merchant key is missing".to_string());
    };
    let binding = crate::handlers::shared::PaymentGatewaySecretBinding::from_record(&record)
        .map_err(|_| "epay merchant key binding is invalid".to_string())?;
    let merchant_key =
        crate::handlers::shared::open_payment_gateway_secret(state, &binding, encrypted_key)
            .map_err(|_| "epay merchant key decrypt failed".to_string())?
            .plaintext;
    // EPay notifications do not carry a currency or exchange-rate field. Use
    // conservative finite fallbacks only when the order lookup below cannot
    // provide the values; an unknown order is rejected by the repository.
    let pay_currency = crate::handlers::shared::normalize_payment_currency(
        &record.pay_currency,
        "epay pay_currency",
    )
    .unwrap_or_else(|_| "CNY".to_string());
    let usd_exchange_rate = crate::handlers::shared::effective_payment_exchange_rate(
        &pay_currency,
        record.usd_exchange_rate,
    )
    .unwrap_or(1.0);
    Ok(EpayMerchantConfig {
        endpoint_url: record.endpoint_url,
        callback_base_url: record.callback_base_url,
        merchant_id: record.merchant_id,
        merchant_key,
        pay_currency,
        usd_exchange_rate,
        min_recharge_usd: 0.0,
        channels: record.channels_json,
    })
}

fn epay_plain(status: http::StatusCode, body: &'static str) -> Response<Body> {
    Response::builder()
        .status(status)
        .header(http::header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Body::from(body))
        .expect("epay plain response should build")
}

fn epay_redirect(location: String) -> Response<Body> {
    Response::builder()
        .status(http::StatusCode::FOUND)
        .header(http::header::LOCATION, location)
        .body(Body::empty())
        .expect("epay redirect response should build")
}

fn epay_return_location(params: &BTreeMap<String, String>, signature_valid: bool) -> String {
    let order_no = params.get("out_trade_no").map(String::as_str).unwrap_or("");
    let base = if order_no.starts_with("pp_") {
        "/dashboard/billing"
    } else {
        "/dashboard/wallet"
    };
    let payment_status = if signature_valid
        && params.get("trade_status").map(String::as_str) == Some("TRADE_SUCCESS")
    {
        "success"
    } else {
        "pending"
    };
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    serializer.append_pair("payment_provider", "epay");
    serializer.append_pair("payment_status", payment_status);
    if !order_no.is_empty() {
        serializer.append_pair("order_no", order_no);
    }
    if let Some(trade_no) = params
        .get("trade_no")
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty())
    {
        serializer.append_pair("trade_no", trade_no);
    }
    format!("{base}?{}", serializer.finish())
}

pub(super) async fn handle_epay_notify(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    request_body: Option<&axum::body::Bytes>,
) -> Response<Body> {
    let config = match load_epay_callback_config(state).await {
        Ok(value) => value,
        Err(_) => return epay_plain(http::StatusCode::OK, "fail"),
    };
    let params = parse_epay_params(
        request_context.request_query_string.as_deref(),
        request_body,
    );
    if !epay_signature_valid(&params, &config.merchant_key) {
        return epay_plain(http::StatusCode::OK, "fail");
    }
    if params.get("pid").map(String::as_str) != Some(config.merchant_id.as_str()) {
        return epay_plain(http::StatusCode::OK, "fail");
    }
    if params.get("trade_status").map(String::as_str) != Some("TRADE_SUCCESS") {
        return epay_plain(http::StatusCode::OK, "fail");
    }
    let Some(order_no) =
        bounded_epay_callback_value(params.get("out_trade_no"), MAX_EPAY_ORDER_NO_BYTES)
    else {
        return epay_plain(http::StatusCode::OK, "fail");
    };
    let Some(pay_amount) = params
        .get("money")
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value > 0.0)
    else {
        return epay_plain(http::StatusCode::OK, "fail");
    };
    let Some(channel) = bounded_epay_callback_value(params.get("type"), MAX_EPAY_CHANNEL_BYTES)
        .map(|value| value.to_ascii_lowercase())
    else {
        return epay_plain(http::StatusCode::OK, "fail");
    };
    // Do not require the channel to remain enabled after checkout creation.
    // The repository binds this signed value to the channel stored on the
    // order, so removing a channel only blocks new checkouts and cannot strand
    // an already-paid order.
    let Some(gateway_order_id) =
        bounded_epay_callback_value(params.get("trade_no"), MAX_EPAY_GATEWAY_ORDER_ID_BYTES)
    else {
        return epay_plain(http::StatusCode::OK, "fail");
    };
    let raw_payload = serde_json::to_value(&params).unwrap_or_else(|_| json!({}));
    let payload_hash = match payment_callback_payload_hash(&raw_payload) {
        Ok(value) => value,
        Err(_) => return epay_plain(http::StatusCode::OK, "fail"),
    };
    let callback_key = format!("epay:{gateway_order_id}");
    let order = match crate::handlers::shared::find_payment_callback_order(state, &order_no).await {
        Ok(value) => value,
        Err(_) => return epay_plain(http::StatusCode::OK, "fail"),
    };
    let (amount_usd, exchange_rate) =
        match crate::handlers::shared::payment_callback_settlement_values(
            order.as_ref(),
            pay_amount,
            Some(config.usd_exchange_rate),
        ) {
            Ok(value) => value,
            Err(_) => return epay_plain(http::StatusCode::OK, "fail"),
        };
    let pay_currency = order
        .as_ref()
        .and_then(|order| order.pay_currency.clone())
        .unwrap_or_else(|| config.pay_currency.clone());
    let payload = epay_callback_projection(
        &params,
        &order_no,
        Some(&gateway_order_id),
        pay_amount,
        &pay_currency,
        Some(&channel),
    );

    let outcome = state
        .process_payment_callback(
            aether_data::repository::wallet::ProcessPaymentCallbackInput {
                payment_method: "epay".to_string(),
                payment_provider: Some("epay".to_string()),
                payment_channel: Some(channel),
                callback_key,
                order_no: Some(order_no),
                gateway_order_id: Some(gateway_order_id),
                amount_usd,
                pay_amount: Some(pay_amount),
                pay_currency: Some(pay_currency),
                exchange_rate,
                payload_hash,
                payload,
                signature_valid: true,
            },
        )
        .await;

    if let Ok(Some(callback_outcome)) = &outcome {
        if !super::reconcile_payment_callback_referral_rewards(state, callback_outcome, "epay")
            .await
        {
            return epay_plain(http::StatusCode::OK, "fail");
        }
    }

    match outcome {
        Ok(Some(aether_data::repository::wallet::ProcessPaymentCallbackOutcome::Applied {
            ..
        })) => epay_plain(http::StatusCode::OK, "success"),
        Ok(Some(
            aether_data::repository::wallet::ProcessPaymentCallbackOutcome::AlreadyCredited {
                ..
            },
        ))
        | Ok(Some(
            aether_data::repository::wallet::ProcessPaymentCallbackOutcome::DuplicateProcessed {
                ..
            },
        )) => epay_plain(http::StatusCode::OK, "success"),
        _ => epay_plain(http::StatusCode::OK, "fail"),
    }
}

pub(super) async fn handle_epay_return(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    request_body: Option<&axum::body::Bytes>,
) -> Response<Body> {
    let params = parse_epay_params(
        request_context.request_query_string.as_deref(),
        request_body,
    );
    let signature_valid = load_epay_callback_config(state)
        .await
        .ok()
        .is_some_and(|config| epay_signature_valid(&params, &config.merchant_key));
    epay_redirect(epay_return_location(&params, signature_valid))
}

#[cfg(test)]
mod tests {
    use super::{
        build_epay_checkout_url, configured_epay_channels, epay_callback_base_url,
        epay_callback_projection, epay_sign, epay_signature_valid, resolve_epay_channel,
        EpayCheckoutInput, EpayMerchantConfig,
    };
    use chrono::Utc;
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    fn epay_sign_excludes_sign_type_sign_and_empty_values() {
        let mut params = BTreeMap::new();
        params.insert("pid".to_string(), "1001".to_string());
        params.insert("out_trade_no".to_string(), "po_1".to_string());
        params.insert("money".to_string(), "10.00".to_string());
        params.insert("empty".to_string(), "".to_string());
        params.insert("sign_type".to_string(), "MD5".to_string());
        let sign = epay_sign(&params, "secret");
        params.insert("sign".to_string(), sign.clone());
        assert!(epay_signature_valid(&params, "secret"));
        assert!(!epay_signature_valid(&params, "wrong"));
    }

    #[test]
    fn epay_config_debug_output_redacts_merchant_credentials() {
        let mut config = test_epay_config(json!({"private": "epay-channel-canary"}));
        config.endpoint_url = "https://pay.example/?token=epay-endpoint-canary".to_string();
        config.callback_base_url =
            Some("https://callback.example/epay-callback-canary".to_string());
        config.merchant_key = "epay-merchant-key-canary".to_string();

        let debug = format!("{config:?}");
        assert!(debug.contains("[REDACTED]"));
        for secret in [
            "epay-channel-canary",
            "epay-endpoint-canary",
            "epay-callback-canary",
            "epay-merchant-key-canary",
        ] {
            assert!(!debug.contains(secret), "debug output leaked {secret}");
        }
    }

    #[test]
    fn configured_epay_channels_do_not_invent_defaults() {
        let mut config = test_epay_config(json!([
            {"channel": " Alipay ", "display_name": "支付宝", "fee_rate": 2.5},
            {"type": "wxpay", "display_name": "", "fee_rate": "1.2"},
            {"display_name": "缺少通道值"}
        ]));

        let channels = configured_epay_channels(&config);
        assert_eq!(channels.len(), 2);
        assert_eq!(channels[0].channel, "Alipay");
        assert_eq!(channels[0].display_name, "支付宝");
        assert_eq!(channels[0].fee_rate, 2.5);
        assert_eq!(channels[1].channel, "wxpay");
        assert_eq!(channels[1].display_name, "wxpay");
        assert_eq!(channels[1].fee_rate, 1.2);
        assert_eq!(
            resolve_epay_channel(&config, None).map(|channel| channel.channel),
            Ok("Alipay".to_string())
        );
        assert_eq!(
            resolve_epay_channel(&config, Some("WXPAY")).map(|channel| channel.channel),
            Ok("wxpay".to_string())
        );
        assert_eq!(
            resolve_epay_channel(&config, Some("manual")),
            Err("支付通道未配置或已停用")
        );

        config.channels = json!([]);
        assert!(configured_epay_channels(&config).is_empty());
        assert_eq!(
            resolve_epay_channel(&config, None),
            Err("支付网关未配置可用通道")
        );
    }

    #[test]
    fn epay_checkout_uses_post_form_payload_and_submit_endpoint() {
        let mut config = test_epay_config(json!([]));
        config.endpoint_url = "https://pay.example.com/".to_string();

        let checkout = build_epay_checkout_url(
            &config,
            &EpayCheckoutInput {
                order_no: "po_test".to_string(),
                channel: "alipay".to_string(),
                subject: "钱包充值".to_string(),
                pay_amount: 10.0,
                notify_url: "https://aether.example.com/api/payment/epay/notify".to_string(),
                return_url: "https://aether.example.com/api/payment/epay/return".to_string(),
            },
        )
        .expect("valid HTTPS endpoint should build checkout");

        assert_eq!(
            checkout["payment_url"],
            "https://pay.example.com/submit.php"
        );
        assert_eq!(checkout["submit_method"], "POST");
        assert_eq!(checkout["payment_params"]["pid"], "1000");
        assert_eq!(checkout["payment_params"]["type"], "alipay");
        assert_eq!(checkout["payment_params"]["out_trade_no"], "po_test");
        assert_eq!(checkout["payment_params"]["money"], "10.00");
        assert_eq!(checkout["payment_params"]["sign_type"], "MD5");
        assert!(checkout["payment_params"]["sign"]
            .as_str()
            .is_some_and(|value| !value.is_empty()));

        config.endpoint_url = "https://pay.example.com/submit.php".to_string();
        let checkout = build_epay_checkout_url(
            &config,
            &EpayCheckoutInput {
                order_no: format!("po_{}", Utc::now().timestamp()),
                channel: "wxpay".to_string(),
                subject: "钱包充值".to_string(),
                pay_amount: 1.0,
                notify_url: "https://aether.example.com/api/payment/epay/notify".to_string(),
                return_url: "https://aether.example.com/api/payment/epay/return".to_string(),
            },
        )
        .expect("valid HTTPS endpoint should build checkout");
        assert_eq!(
            checkout["payment_url"],
            "https://pay.example.com/submit.php"
        );
    }

    #[test]
    fn epay_checkout_rejects_executable_or_insecure_endpoint_urls() {
        for endpoint_url in [
            "javascript:alert(document.domain)",
            "data:text/html,attack",
            "http://pay.example.com/submit.php",
            "/submit.php",
        ] {
            let mut config = test_epay_config(json!([]));
            config.endpoint_url = endpoint_url.to_string();
            let result = build_epay_checkout_url(
                &config,
                &EpayCheckoutInput {
                    order_no: "po_unsafe".to_string(),
                    channel: "alipay".to_string(),
                    subject: "wallet recharge".to_string(),
                    pay_amount: 1.0,
                    notify_url: "https://aether.example/api/payment/epay/notify".to_string(),
                    return_url: "https://aether.example/api/payment/epay/return".to_string(),
                },
            );
            assert!(
                result.is_err(),
                "unsafe endpoint should fail: {endpoint_url}"
            );
        }
    }

    #[test]
    fn epay_callback_base_requires_explicit_https_configuration() {
        assert_eq!(
            epay_callback_base_url(Some("https://aether.example/")),
            Some("https://aether.example".to_string())
        );
        assert_eq!(epay_callback_base_url(Some("http://aether.example")), None);
        assert_eq!(
            epay_callback_base_url(Some("https://user:secret@aether.example")),
            None
        );
    }

    #[test]
    fn epay_callback_projection_does_not_persist_signature_or_payer_fields() {
        let params = BTreeMap::from([
            ("trade_no".to_string(), "gateway-1".to_string()),
            ("sign".to_string(), "replayable-signature".to_string()),
            ("buyer_email".to_string(), "payer@example.com".to_string()),
            ("name".to_string(), "private subject".to_string()),
        ]);

        let projection = epay_callback_projection(
            &params,
            "po_1",
            Some("gateway-1"),
            72.0,
            "CNY",
            Some("alipay"),
        );
        assert_eq!(projection["event_id"], "gateway-1");
        assert_eq!(projection["gateway_order_id"], "gateway-1");
        assert_eq!(projection["order_no"], "po_1");
        assert_eq!(projection["payment_channel"], "alipay");
        assert!(projection.get("sign").is_none());

        let encoded = projection.to_string();
        for forbidden in [
            "replayable-signature",
            "buyer_email",
            "payer@example.com",
            "private subject",
        ] {
            assert!(!encoded.contains(forbidden), "persisted {forbidden}");
        }
    }

    fn test_epay_config(channels: serde_json::Value) -> EpayMerchantConfig {
        EpayMerchantConfig {
            endpoint_url: "https://pay.example.com/submit.php".to_string(),
            callback_base_url: Some("https://aether.example.com".to_string()),
            merchant_id: "1000".to_string(),
            merchant_key: "secret".to_string(),
            pay_currency: "CNY".to_string(),
            usd_exchange_rate: 7.2,
            min_recharge_usd: 1.0,
            channels,
        }
    }
}
