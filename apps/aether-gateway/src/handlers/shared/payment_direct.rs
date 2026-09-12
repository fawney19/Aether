use crate::AppState;
use aes_gcm::{
    aead::{Aead, Payload},
    Aes256Gcm, KeyInit, Nonce,
};
use aether_crypto::{rsa_pkcs1_sha256_sign, rsa_pkcs1_sha256_verify, RsaPkcs1Sha256Error};
use axum::http;
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use chrono::Utc;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};

const ALIPAY_DEFAULT_GATEWAY_URL: &str = "https://openapi.alipay.com/gateway.do";
const WXPAY_DEFAULT_BASE_URL: &str = "https://api.mch.weixin.qq.com";
const MAX_PAYMENT_GATEWAY_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_PAYMENT_ORDER_NO_BYTES: usize = 64;
const MAX_PAYMENT_GATEWAY_ID_BYTES: usize = 128;
const MAX_PAYMENT_CALLBACK_KEY_BYTES: usize = 128;
const MAX_PAYMENT_SIGNATURE_BYTES: usize = 16 * 1024;
const MAX_PAYMENT_CALLBACK_CIPHERTEXT_BYTES: usize = 1024 * 1024;
const WXPAY_NOTIFY_SUCCESS: &str = "TRANSACTION.SUCCESS";
const WXPAY_TRADE_SUCCESS: &str = "SUCCESS";
const WXPAY_CURRENCY: &str = "CNY";
const WXPAY_SIGNATURE_TOLERANCE_SECONDS: i64 = 5 * 60;
const STRIPE_CHECKOUT_UNCERTAIN_DETAIL: &str = "Stripe 支付服务暂时不可用";
const STRIPE_CHECKOUT_FAILED_DETAIL: &str = "Stripe 支付请求被拒绝";

/// Errors returned by direct checkout creators are typed so callers can
/// distinguish a provider request whose outcome was not observed from a
/// deterministic validation/business failure. A cancelled Stripe intent must
/// never be replayed as a live checkout under the same merchant order number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DirectPaymentCheckoutError {
    Canceled,
    Uncertain(String),
    Failed(String),
}

impl From<String> for DirectPaymentCheckoutError {
    fn from(value: String) -> Self {
        Self::Failed(value)
    }
}

impl DirectPaymentCheckoutError {
    pub(crate) fn into_detail(self) -> String {
        match self {
            Self::Canceled => "支付订单已取消".to_string(),
            Self::Uncertain(detail) | Self::Failed(detail) => detail,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DirectPaymentRequestError {
    Uncertain(String),
    Failed(String),
}

impl From<String> for DirectPaymentRequestError {
    fn from(value: String) -> Self {
        Self::Failed(value)
    }
}

impl DirectPaymentRequestError {
    fn into_detail(self) -> String {
        match self {
            Self::Uncertain(detail) | Self::Failed(detail) => detail,
        }
    }
}

impl From<DirectPaymentRequestError> for DirectPaymentCheckoutError {
    fn from(error: DirectPaymentRequestError) -> Self {
        match error {
            DirectPaymentRequestError::Uncertain(detail) => Self::Uncertain(detail),
            DirectPaymentRequestError::Failed(detail) => Self::Failed(detail),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct DirectPaymentCheckoutInput {
    pub(crate) payment_channel: String,
    pub(crate) display_name: String,
    pub(crate) order_no: String,
    pub(crate) subject: String,
    pub(crate) pay_amount: f64,
    pub(crate) pay_currency: String,
    pub(crate) notify_url: String,
    pub(crate) return_url: Option<String>,
    pub(crate) client_ip: Option<String>,
    pub(crate) expires_at: chrono::DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub(crate) struct DirectGatewayRefundResult {
    pub(crate) gateway_refund_id: String,
    pub(crate) status: String,
    pub(crate) proof: Value,
}

impl DirectGatewayRefundResult {
    pub(crate) fn is_succeeded(&self) -> bool {
        self.status.eq_ignore_ascii_case("success")
    }

    pub(crate) fn is_pending(&self) -> bool {
        matches!(
            self.status.to_ascii_lowercase().as_str(),
            "pending" | "processing"
        )
    }
}

#[derive(Clone)]
struct DirectGatewayConfig {
    record: aether_data_contracts::repository::billing::PaymentGatewayConfigRecord,
    config: serde_json::Map<String, Value>,
    secrets: serde_json::Map<String, Value>,
}

impl std::fmt::Debug for DirectGatewayConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DirectGatewayConfig")
            .field("record", &self.record)
            .field("config", &"[REDACTED]")
            .field("secrets", &"[REDACTED]")
            .finish()
    }
}

pub(crate) async fn find_payment_callback_order(
    state: &AppState,
    order_no: &str,
) -> Result<Option<aether_data::repository::wallet::StoredAdminPaymentOrder>, String> {
    state
        .find_payment_order_by_order_no(order_no)
        .await
        .map_err(|_| "支付订单查询失败".to_string())
}

fn valid_exchange_rate(value: Option<f64>) -> Option<f64> {
    value.filter(|value| value.is_finite() && *value > 0.0)
}

/// Resolve callback accounting values without allowing mutable gateway
/// configuration to rewrite an order that was already created. The provider
/// amount is still passed separately and is checked against the stored order
/// by the repository; this helper only supplies a finite amount for the
/// callback contract and a non-sensitive rate fallback.
pub(crate) fn payment_callback_settlement_values(
    order: Option<&aether_data::repository::wallet::StoredAdminPaymentOrder>,
    pay_amount: f64,
    fallback_exchange_rate: Option<f64>,
) -> Result<(f64, Option<f64>), String> {
    let exchange_rate = order
        .and_then(|order| valid_exchange_rate(order.exchange_rate))
        .or_else(|| valid_exchange_rate(fallback_exchange_rate));
    let amount_usd = order
        .map(|order| order.amount_usd)
        .filter(|value| value.is_finite() && *value > 0.0)
        .or_else(|| {
            exchange_rate
                .map(|rate| pay_amount / rate)
                .filter(|value| value.is_finite() && *value > 0.0)
        })
        .unwrap_or(pay_amount);
    if !amount_usd.is_finite() || amount_usd <= 0.0 {
        return Err("支付回调金额换算无效".to_string());
    }
    Ok((amount_usd, exchange_rate))
}

fn payment_payload_hash(payload: &Value) -> Result<String, String> {
    let encoded = serde_json::to_vec(payload)
        .map_err(|_| "payment callback payload encode failed".to_string())?;
    let digest = Sha256::digest(&encoded);
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn payment_bytes_hash(payload: &[u8]) -> String {
    let digest = Sha256::digest(payload);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn validated_payment_identifier(
    value: &str,
    field: &str,
    max_bytes: usize,
) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > max_bytes
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(format!("{field} 格式无效"));
    }
    Ok(value.to_string())
}

fn validated_optional_payment_identifier(
    value: Option<&str>,
    field: &str,
    max_bytes: usize,
) -> Result<Option<String>, String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| validated_payment_identifier(value, field, max_bytes))
        .transpose()
}

fn payment_callback_key(
    gateway: &str,
    candidate: Option<&str>,
    payload_hash: &str,
) -> Result<String, String> {
    let candidate = validated_optional_payment_identifier(
        candidate,
        "支付通知事件 ID",
        MAX_PAYMENT_GATEWAY_ID_BYTES,
    )?;
    let prefix = format!("{gateway}:");
    if let Some(candidate) = candidate {
        if prefix.len() + candidate.len() <= MAX_PAYMENT_CALLBACK_KEY_BYTES {
            return Ok(format!("{prefix}{candidate}"));
        }
    }
    Ok(format!("{prefix}{payload_hash}"))
}

fn payment_callback_projection(
    gateway: &str,
    order_no: &str,
    gateway_order_id: Option<&str>,
    amount: f64,
    currency: &str,
    status: &str,
) -> Value {
    json!({
        "gateway": gateway,
        "order_no": order_no,
        "gateway_order_id": gateway_order_id,
        "amount": amount,
        "currency": currency,
        "status": status,
        "signature_valid": true,
        "processed_at": Utc::now().to_rfc3339(),
    })
}

fn gateway_refund_proof(
    gateway: &str,
    gateway_refund_id: &str,
    status: &str,
    order_no: &str,
    refund_no: &str,
    amount: f64,
    currency: &str,
) -> Value {
    json!({
        "gateway": gateway,
        "id": gateway_refund_id,
        "status": status,
        "order_no": order_no,
        "refund_no": refund_no,
        "amount": amount,
        "currency": currency,
        "processed_at": Utc::now().to_rfc3339(),
    })
}

fn wxpay_refund_status(value: Option<&str>) -> &'static str {
    match value.map(str::trim).map(str::to_ascii_uppercase).as_deref() {
        Some("SUCCESS") => "success",
        Some("PROCESSING") => "processing",
        Some("CLOSED" | "ABNORMAL") | None => "failed",
        Some(_) => "failed",
    }
}

fn config_string(config: &serde_json::Map<String, Value>, key: &str) -> Option<String> {
    config
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn secret_string(secrets: &serde_json::Map<String, Value>, key: &str) -> Option<String> {
    secrets
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn stripe_checkout_response_is_canceled(value: &Value) -> bool {
    value
        .get("status")
        .and_then(Value::as_str)
        .is_some_and(|status| status.trim().eq_ignore_ascii_case("canceled"))
}

fn record_config_map(
    record: &aether_data_contracts::repository::billing::PaymentGatewayConfigRecord,
) -> serde_json::Map<String, Value> {
    super::payment_gateway_config_json(&record.channels_json)
        .as_object()
        .cloned()
        .unwrap_or_default()
}

async fn load_direct_gateway_config(
    state: &AppState,
    provider: &str,
) -> Result<DirectGatewayConfig, String> {
    let mut record = state
        .find_payment_gateway_config(provider)
        .await
        .map_err(|_| format!("{provider} 配置读取失败"))?
        .ok_or_else(|| format!("{provider} 未配置"))?;
    if !record.enabled {
        return Err(format!("{provider} 未启用"));
    }
    record.pay_currency = super::normalize_payment_currency(&record.pay_currency, "pay_currency")?;
    record.usd_exchange_rate =
        super::effective_payment_exchange_rate(&record.pay_currency, record.usd_exchange_rate)
            .map_err(|_| format!("{provider} 汇率配置无效"))?;
    if !record.endpoint_url.trim().is_empty() {
        record.endpoint_url =
            super::normalize_payment_https_url(&record.endpoint_url, "endpoint_url")?;
    }
    record.callback_base_url = record
        .callback_base_url
        .as_deref()
        .map(super::normalize_payment_callback_base_url)
        .transpose()?;
    let Some(encrypted) = record.merchant_key_encrypted.as_deref() else {
        return Err(format!("{provider} 密钥未配置"));
    };
    let binding = super::PaymentGatewaySecretBinding::from_record(&record)
        .map_err(|_| format!("{provider} 密钥绑定无效"))?;
    let plaintext = super::open_payment_gateway_secret(state, &binding, encrypted)
        .map_err(|_| format!("{provider} 密钥解密失败"))?
        .plaintext;
    let secrets = serde_json::from_str::<Value>(&plaintext)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .ok_or_else(|| format!("{provider} 密钥格式无效"))?;
    let config = record_config_map(&record);
    Ok(DirectGatewayConfig {
        record,
        config,
        secrets,
    })
}

/// Load only the credentials and provider identifiers required to authenticate
/// a callback. Gateway enablement, checkout endpoint, and current pricing
/// settings are mutable operational controls; changing them must not strand a
/// payment that was already accepted by the provider.
async fn load_direct_gateway_callback_config(
    state: &AppState,
    provider: &str,
) -> Result<DirectGatewayConfig, String> {
    let record = state
        .find_payment_gateway_config(provider)
        .await
        .map_err(|_| format!("{provider} 配置读取失败"))?
        .ok_or_else(|| format!("{provider} 未配置"))?;
    let Some(encrypted) = record.merchant_key_encrypted.as_deref() else {
        return Err(format!("{provider} 密钥未配置"));
    };
    let binding = super::PaymentGatewaySecretBinding::from_record(&record)
        .map_err(|_| format!("{provider} 密钥绑定无效"))?;
    let plaintext = super::open_payment_gateway_secret(state, &binding, encrypted)
        .map_err(|_| format!("{provider} 密钥解密失败"))?
        .plaintext;
    let secrets = serde_json::from_str::<Value>(&plaintext)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .ok_or_else(|| format!("{provider} 密钥格式无效"))?;
    let config = record_config_map(&record);
    Ok(DirectGatewayConfig {
        record,
        config,
        secrets,
    })
}

fn rsa_sha256_sign_base64(private_key: &str, message: &str) -> Result<String, String> {
    let signature =
        rsa_pkcs1_sha256_sign(private_key.as_bytes(), message.as_bytes()).map_err(|error| {
            match error {
                RsaPkcs1Sha256Error::InvalidPrivateKey => "RSA 私钥解析失败".to_string(),
                _ => "RSA 签名失败".to_string(),
            }
        })?;
    Ok(BASE64_STANDARD.encode(signature))
}

fn rsa_sha256_verify_base64(
    public_key: &str,
    message: &str,
    signature_base64: &str,
) -> Result<bool, String> {
    let signature_bytes = decode_payment_base64_with_limit(
        signature_base64.trim(),
        MAX_PAYMENT_SIGNATURE_BYTES,
        "签名 base64 解码失败",
    )?;
    rsa_pkcs1_sha256_verify(public_key.as_bytes(), message.as_bytes(), &signature_bytes).map_err(
        |error| match error {
            RsaPkcs1Sha256Error::InvalidPublicKey => "RSA 公钥解析失败".to_string(),
            _ => "签名格式无效".to_string(),
        },
    )
}

fn decode_payment_base64_with_limit(
    value: &str,
    limit_bytes: usize,
    invalid_message: &'static str,
) -> Result<Vec<u8>, String> {
    crate::execution_runtime::transport::decode_base64_body_with_limit(value, limit_bytes)
        .map_err(|_| invalid_message.to_string())
}

fn alipay_timestamp() -> String {
    (Utc::now() + chrono::Duration::hours(8))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}

fn alipay_gateway_url(config: &DirectGatewayConfig) -> String {
    let endpoint_url = config.record.endpoint_url.trim();
    if endpoint_url.is_empty() {
        ALIPAY_DEFAULT_GATEWAY_URL.to_string()
    } else {
        endpoint_url.to_string()
    }
}

fn alipay_app_id(config: &DirectGatewayConfig) -> Result<String, String> {
    config_string(&config.config, "app_id").ok_or_else(|| "支付宝 app_id 未配置".to_string())
}

fn alipay_private_key(config: &DirectGatewayConfig) -> Result<String, String> {
    secret_string(&config.secrets, "private_key").ok_or_else(|| "支付宝应用私钥未配置".to_string())
}

fn alipay_public_key(config: &DirectGatewayConfig) -> Result<String, String> {
    secret_string(&config.secrets, "alipay_public_key")
        .or_else(|| secret_string(&config.secrets, "public_key"))
        .ok_or_else(|| "支付宝公钥未配置".to_string())
}

fn alipay_request_sign_content(params: &BTreeMap<String, String>) -> String {
    params
        .iter()
        .filter(|(key, value)| key.as_str() != "sign" && !value.trim().is_empty())
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("&")
}

fn alipay_notify_sign_content(params: &BTreeMap<String, String>) -> String {
    params
        .iter()
        .filter(|(key, value)| {
            key.as_str() != "sign" && key.as_str() != "sign_type" && !value.trim().is_empty()
        })
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("&")
}

fn alipay_signed_params(
    config: &DirectGatewayConfig,
    method: &str,
    biz_content: Value,
    notify_url: Option<&str>,
    return_url: Option<&str>,
) -> Result<BTreeMap<String, String>, String> {
    let mut params = BTreeMap::new();
    params.insert("app_id".to_string(), alipay_app_id(config)?);
    params.insert("method".to_string(), method.to_string());
    params.insert("format".to_string(), "JSON".to_string());
    params.insert("charset".to_string(), "utf-8".to_string());
    params.insert("sign_type".to_string(), "RSA2".to_string());
    params.insert("timestamp".to_string(), alipay_timestamp());
    params.insert("version".to_string(), "1.0".to_string());
    params.insert(
        "biz_content".to_string(),
        serde_json::to_string(&biz_content)
            .map_err(|_| "支付宝 biz_content 编码失败".to_string())?,
    );
    if let Some(notify_url) = notify_url.map(str::trim).filter(|value| !value.is_empty()) {
        params.insert("notify_url".to_string(), notify_url.to_string());
    }
    if let Some(return_url) = return_url.map(str::trim).filter(|value| !value.is_empty()) {
        params.insert("return_url".to_string(), return_url.to_string());
    }
    let sign_content = alipay_request_sign_content(&params);
    let sign = rsa_sha256_sign_base64(&alipay_private_key(config)?, &sign_content)?;
    params.insert("sign".to_string(), sign);
    Ok(params)
}

fn url_with_query(base: &str, params: &BTreeMap<String, String>) -> Result<String, String> {
    let mut url = url::Url::parse(base).map_err(|_| "支付网关地址无效".to_string())?;
    {
        let mut query = url.query_pairs_mut();
        for (key, value) in params {
            query.append_pair(key, value);
        }
    }
    Ok(url.to_string())
}

async fn alipay_post_typed(
    state: &AppState,
    config: &DirectGatewayConfig,
    params: &BTreeMap<String, String>,
) -> Result<Value, DirectPaymentRequestError> {
    let gateway_url = url::Url::parse(&alipay_gateway_url(config))
        .map_err(|_| DirectPaymentRequestError::Failed("支付宝网关 URL 无效".to_string()))?;
    let client = public_payment_http_client(&gateway_url)
        .await
        .map_err(DirectPaymentRequestError::Failed)?;
    let response = client
        .post(gateway_url)
        .form(params)
        .send()
        .await
        .map_err(|_| DirectPaymentRequestError::Uncertain("支付宝请求失败".to_string()))?;
    let status = response.status();
    let body =
        aether_http::read_response_bytes_with_limit(response, MAX_PAYMENT_GATEWAY_RESPONSE_BYTES)
            .await
            .map_err(|_| DirectPaymentRequestError::Uncertain("支付宝响应读取失败".to_string()))?;
    let value = serde_json::from_slice::<Value>(&body)
        .map_err(|_| DirectPaymentRequestError::Uncertain("支付宝响应格式无效".to_string()))?;
    if !status.is_success() {
        let detail = format!("支付宝 HTTP 状态异常: {status}");
        return Err(
            if status.is_server_error() || status == http::StatusCode::TOO_MANY_REQUESTS {
                DirectPaymentRequestError::Uncertain(detail)
            } else {
                DirectPaymentRequestError::Failed(detail)
            },
        );
    }
    Ok(value)
}

async fn alipay_post(
    state: &AppState,
    config: &DirectGatewayConfig,
    params: &BTreeMap<String, String>,
) -> Result<Value, String> {
    alipay_post_typed(state, config, params)
        .await
        .map_err(DirectPaymentRequestError::into_detail)
}

/// A parsed 4xx Alipay response is an explicit request/business rejection and
/// can safely use the page-pay fallback. System/transient sub-codes are
/// excluded even when Alipay wraps them in the generic 40004 business code:
/// those responses do not establish that the precreate request was rejected
/// before reaching the provider.
fn alipay_precreate_business_refusal(response: &Value) -> bool {
    let code = response.get("code").and_then(Value::as_str).unwrap_or("");
    if !code.starts_with('4') {
        return false;
    }
    let sub_code = response
        .get("sub_code")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_ascii_uppercase();
    ![
        "SYSTEM_ERROR",
        "TIMEOUT",
        "REQUEST_TIMEOUT",
        "NETWORK_ERROR",
        "PROCESSING",
        "UNKNOWN_ERROR",
    ]
    .iter()
    .any(|marker| sub_code.contains(marker))
}

fn alipay_response_success<'a>(value: &'a Value, key: &str) -> Result<&'a Value, String> {
    let response = value
        .get(key)
        .ok_or_else(|| "支付宝响应缺少业务结果".to_string())?;
    if response.get("code").and_then(Value::as_str) == Some("10000") {
        return Ok(response);
    }
    Err("支付宝业务请求失败".to_string())
}

pub(crate) async fn create_alipay_direct_checkout(
    state: &AppState,
    input: &DirectPaymentCheckoutInput,
) -> Result<Value, DirectPaymentCheckoutError> {
    let order_no =
        validated_payment_identifier(&input.order_no, "支付订单号", MAX_PAYMENT_ORDER_NO_BYTES)?;
    let config = load_direct_gateway_config(state, "alipay").await?;
    if !input.pay_currency.eq_ignore_ascii_case("CNY") {
        return Err(DirectPaymentCheckoutError::Failed(
            "支付宝官方直连当前仅支持 CNY".to_string(),
        ));
    }
    let mode = config_string(&config.config, "payment_mode")
        .unwrap_or_else(|| "precreate".to_string())
        .to_ascii_lowercase();
    let total_amount = format!("{:.2}", input.pay_amount);
    let gateway_url = alipay_gateway_url(&config);
    let return_url = input.return_url.as_deref();
    if matches!(mode.as_str(), "page" | "redirect") {
        let params = alipay_signed_params(
            &config,
            "alipay.trade.page.pay",
            json!({
                "out_trade_no": order_no,
                "total_amount": total_amount,
                "subject": input.subject,
                "product_code": "FAST_INSTANT_TRADE_PAY",
            }),
            Some(&input.notify_url),
            return_url,
        )?;
        return Ok(json!({
            "gateway": "alipay",
            "display_name": input.display_name,
            "gateway_order_id": order_no,
            "payment_url": url_with_query(&gateway_url, &params)?,
            "submit_method": "GET",
            "qr_code": Value::Null,
            "pay_amount": input.pay_amount,
            "pay_currency": input.pay_currency,
            "payment_channel": input.payment_channel,
            "callback_url": input.notify_url,
            "return_url": return_url,
            "expires_at": input.expires_at.to_rfc3339(),
        }));
    }
    if matches!(mode.as_str(), "wap" | "h5") {
        let params = alipay_signed_params(
            &config,
            "alipay.trade.wap.pay",
            json!({
                "out_trade_no": order_no,
                "total_amount": total_amount,
                "subject": input.subject,
                "product_code": "QUICK_WAP_WAY",
            }),
            Some(&input.notify_url),
            return_url,
        )?;
        return Ok(json!({
            "gateway": "alipay",
            "display_name": input.display_name,
            "gateway_order_id": order_no,
            "payment_url": url_with_query(&gateway_url, &params)?,
            "submit_method": "GET",
            "qr_code": Value::Null,
            "pay_amount": input.pay_amount,
            "pay_currency": input.pay_currency,
            "payment_channel": input.payment_channel,
            "callback_url": input.notify_url,
            "return_url": return_url,
            "expires_at": input.expires_at.to_rfc3339(),
        }));
    }

    let params = alipay_signed_params(
        &config,
        "alipay.trade.precreate",
        json!({
            "out_trade_no": order_no,
            "total_amount": total_amount,
            "subject": input.subject,
            "product_code": "FACE_TO_FACE_PAYMENT",
        }),
        Some(&input.notify_url),
        None,
    )?;
    let value = alipay_post_typed(state, &config, &params)
        .await
        .map_err(DirectPaymentCheckoutError::from)?;
    let Some(response) = value.get("alipay_trade_precreate_response") else {
        return Err(DirectPaymentCheckoutError::Uncertain(
            "支付宝预下单响应缺少业务结果".to_string(),
        ));
    };
    let response_code = response.get("code").and_then(Value::as_str);
    if response_code != Some("10000") {
        if !alipay_precreate_business_refusal(response) {
            return Err(DirectPaymentCheckoutError::Uncertain(
                "支付宝预下单结果不确定".to_string(),
            ));
        }
        {
            let params = alipay_signed_params(
                &config,
                "alipay.trade.page.pay",
                json!({
                    "out_trade_no": order_no,
                    "total_amount": total_amount,
                    "subject": input.subject,
                    "product_code": "FAST_INSTANT_TRADE_PAY",
                }),
                Some(&input.notify_url),
                return_url,
            )?;
            Ok(json!({
                "gateway": "alipay",
                "display_name": input.display_name,
                "gateway_order_id": order_no,
                "payment_url": url_with_query(&gateway_url, &params)?,
                "submit_method": "GET",
                "qr_code": Value::Null,
                "pay_amount": input.pay_amount,
                "pay_currency": input.pay_currency,
                "payment_channel": input.payment_channel,
                "callback_url": input.notify_url,
                "return_url": return_url,
                "expires_at": input.expires_at.to_rfc3339(),
                "integration_status": "precreate_fallback",
            }))
        }
    } else {
        let Some(qr_code) = response
            .get("qr_code")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return Err(DirectPaymentCheckoutError::Uncertain(
                "支付宝预下单响应缺少 qr_code".to_string(),
            ));
        };
        Ok(json!({
            "gateway": "alipay",
            "display_name": input.display_name,
            "gateway_order_id": order_no,
            "payment_url": Value::Null,
            "submit_method": "qrcode",
            "qr_code": qr_code,
            "pay_amount": input.pay_amount,
            "pay_currency": input.pay_currency,
            "payment_channel": input.payment_channel,
            "callback_url": input.notify_url,
            "return_url": return_url,
            "expires_at": input.expires_at.to_rfc3339(),
        }))
    }
}

pub(crate) async fn verify_alipay_notify_callback(
    state: &AppState,
    body: &[u8],
) -> Result<aether_data::repository::wallet::ProcessPaymentCallbackInput, String> {
    let config = load_direct_gateway_callback_config(state, "alipay").await?;
    let raw = std::str::from_utf8(body).map_err(|_| "支付宝通知请求体不是 UTF-8".to_string())?;
    let params = url::form_urlencoded::parse(raw.as_bytes())
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect::<BTreeMap<_, _>>();
    let signature = params
        .get("sign")
        .map(String::as_str)
        .ok_or_else(|| "支付宝通知缺少 sign".to_string())?;
    let sign_content = alipay_notify_sign_content(&params);
    if !rsa_sha256_verify_base64(&alipay_public_key(&config)?, &sign_content, signature)? {
        return Err("支付宝通知签名无效".to_string());
    }
    let app_id = params
        .get("app_id")
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "支付宝通知缺少 app_id".to_string())?;
    if app_id != alipay_app_id(&config)? {
        return Err("支付宝通知 app_id 不匹配".to_string());
    }
    if !matches!(
        params.get("trade_status").map(String::as_str),
        Some("TRADE_SUCCESS" | "TRADE_FINISHED")
    ) {
        return Err("支付宝通知不是成功支付状态".to_string());
    }
    let order_no = params
        .get("out_trade_no")
        .map(|value| {
            validated_payment_identifier(
                value,
                "支付宝通知 out_trade_no",
                MAX_PAYMENT_ORDER_NO_BYTES,
            )
        })
        .transpose()?
        .ok_or_else(|| "支付宝通知缺少 out_trade_no".to_string())?;
    let gateway_order_id = validated_optional_payment_identifier(
        params.get("trade_no").map(String::as_str),
        "支付宝通知 trade_no",
        MAX_PAYMENT_GATEWAY_ID_BYTES,
    )?
    .ok_or_else(|| "支付宝通知缺少 trade_no".to_string())?;
    let pay_amount = params
        .get("total_amount")
        .or_else(|| params.get("receipt_amount"))
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value > 0.0)
        .ok_or_else(|| "支付宝通知金额无效".to_string())?;
    let raw_payload = serde_json::to_value(&params).unwrap_or_else(|_| json!({}));
    let payload_hash = payment_payload_hash(&raw_payload)?;
    let callback_key = payment_callback_key(
        "alipay",
        params
            .get("notify_id")
            .map(String::as_str)
            .or(Some(gateway_order_id.as_str())),
        &payload_hash,
    )?;
    let order = find_payment_callback_order(state, &order_no).await?;
    let (amount_usd, exchange_rate) = payment_callback_settlement_values(
        order.as_ref(),
        pay_amount,
        Some(config.record.usd_exchange_rate),
    )?;
    let payload = payment_callback_projection(
        "alipay",
        &order_no,
        Some(&gateway_order_id),
        pay_amount,
        "CNY",
        "success",
    );
    Ok(
        aether_data::repository::wallet::ProcessPaymentCallbackInput {
            payment_method: "alipay".to_string(),
            payment_provider: Some("alipay".to_string()),
            payment_channel: Some("alipay".to_string()),
            callback_key,
            order_no: Some(order_no),
            gateway_order_id: Some(gateway_order_id),
            amount_usd,
            pay_amount: Some(pay_amount),
            pay_currency: Some("CNY".to_string()),
            exchange_rate,
            payload_hash,
            payload,
            signature_valid: true,
        },
    )
}

fn wxpay_base_url(config: &DirectGatewayConfig) -> String {
    let base = config.record.endpoint_url.trim();
    if base.is_empty() {
        WXPAY_DEFAULT_BASE_URL.to_string()
    } else {
        base.trim_end_matches('/').to_string()
    }
}

pub(crate) async fn public_payment_http_client(url: &url::Url) -> Result<reqwest::Client, String> {
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.host_str().is_none()
        || url.fragment().is_some()
    {
        return Err("支付网关必须是无凭据的 HTTPS URL".to_string());
    }
    let host = url
        .host_str()
        .ok_or_else(|| "支付网关 URL 缺少主机名".to_string())?;
    let port = url
        .port_or_known_default()
        .ok_or_else(|| "支付网关 URL 缺少端口".to_string())?;
    let addrs = if let Ok(ip) = host.parse::<IpAddr>() {
        vec![SocketAddr::new(ip, port)]
    } else {
        aether_http::lookup_host_with_limits(host, port, aether_http::DEFAULT_DNS_LOOKUP_TIMEOUT)
            .await
            .map_err(|_| "支付网关 DNS 解析失败".to_string())?
    };
    validate_public_payment_resolved_addrs(url, &addrs)?;

    let mut builder = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none());
    if host.parse::<IpAddr>().is_err() {
        builder = builder.resolve_to_addrs(host, &addrs);
    }
    builder
        .build()
        .map_err(|_| "支付网关 HTTP 客户端初始化失败".to_string())
}

fn validate_public_payment_resolved_addrs(
    url: &url::Url,
    addrs: &[SocketAddr],
) -> Result<(), String> {
    if addrs.is_empty()
        || addrs.iter().any(|addr| {
            aether_http::is_private_or_reserved_ip(addr.ip())
                && !(is_fixed_stripe_api_origin(url)
                    && aether_http::is_ipv4_benchmarking_fake_ip(addr.ip()))
        })
    {
        return Err("支付网关解析到私有或保留地址".to_string());
    }
    Ok(())
}

fn is_fixed_stripe_api_origin(url: &url::Url) -> bool {
    url.scheme() == "https"
        && url.host_str().is_some_and(|host| {
            host.trim_end_matches('.')
                .eq_ignore_ascii_case("api.stripe.com")
        })
        && url.port_or_known_default() == Some(443)
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
}

fn wxpay_config_string(config: &DirectGatewayConfig, key: &str) -> Result<String, String> {
    config_string(&config.config, key).ok_or_else(|| format!("微信支付 {key} 未配置"))
}

fn wxpay_secret_string(config: &DirectGatewayConfig, key: &str) -> Result<String, String> {
    secret_string(&config.secrets, key).ok_or_else(|| format!("微信支付 {key} 未配置"))
}

fn wxpay_money_to_fen(amount: f64) -> Result<i64, String> {
    let value = (amount * 100.0).round();
    if !value.is_finite() || value <= 0.0 {
        return Err("微信支付金额无效".to_string());
    }
    Ok(value as i64)
}

fn wxpay_authorization(
    config: &DirectGatewayConfig,
    method: &str,
    canonical_url: &str,
    body: &str,
) -> Result<String, String> {
    let mch_id = wxpay_config_string(config, "mch_id")?;
    let serial_no = wxpay_config_string(config, "cert_serial")?;
    let private_key = wxpay_secret_string(config, "private_key")?;
    let timestamp = Utc::now().timestamp();
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let message = format!("{method}\n{canonical_url}\n{timestamp}\n{nonce}\n{body}\n");
    let signature = rsa_sha256_sign_base64(&private_key, &message)?;
    Ok(format!(
        "WECHATPAY2-SHA256-RSA2048 mchid=\"{mch_id}\",nonce_str=\"{nonce}\",signature=\"{signature}\",timestamp=\"{timestamp}\",serial_no=\"{serial_no}\""
    ))
}

async fn wxpay_post_json(
    state: &AppState,
    config: &DirectGatewayConfig,
    canonical_url: &str,
    body: Value,
) -> Result<Value, DirectPaymentRequestError> {
    let body = serde_json::to_string(&body)
        .map_err(|_| DirectPaymentRequestError::Failed("微信支付请求体编码失败".to_string()))?;
    let auth = wxpay_authorization(config, "POST", canonical_url, &body)
        .map_err(DirectPaymentRequestError::Failed)?;
    let url = format!("{}{}", wxpay_base_url(config), canonical_url);
    let url = url::Url::parse(&url)
        .map_err(|_| DirectPaymentRequestError::Failed("微信支付网关 URL 无效".to_string()))?;
    let client = public_payment_http_client(&url)
        .await
        .map_err(DirectPaymentRequestError::Failed)?;
    let response = client
        .post(url)
        .header(http::header::AUTHORIZATION, auth)
        .header(http::header::ACCEPT, "application/json")
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(body)
        .send()
        .await
        .map_err(|_| DirectPaymentRequestError::Uncertain("微信支付请求失败".to_string()))?;
    let status = response.status();
    let body =
        aether_http::read_response_bytes_with_limit(response, MAX_PAYMENT_GATEWAY_RESPONSE_BYTES)
            .await
            .map_err(|_| {
                if status.is_server_error() || status == http::StatusCode::TOO_MANY_REQUESTS {
                    DirectPaymentRequestError::Uncertain("微信支付响应读取失败".to_string())
                } else {
                    DirectPaymentRequestError::Failed("微信支付响应读取失败".to_string())
                }
            })?;
    let text = String::from_utf8_lossy(&body);
    if !status.is_success() {
        let detail = "微信支付业务请求失败".to_string();
        return Err(
            if status.is_server_error() || status == http::StatusCode::TOO_MANY_REQUESTS {
                DirectPaymentRequestError::Uncertain(detail)
            } else {
                DirectPaymentRequestError::Failed(detail)
            },
        );
    }
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str::<Value>(&text)
        .map_err(|_| DirectPaymentRequestError::Uncertain("微信支付响应格式无效".to_string()))
}

async fn wxpay_post_empty_success(
    state: &AppState,
    config: &DirectGatewayConfig,
    canonical_url: &str,
    body: Value,
) -> Result<Value, DirectPaymentRequestError> {
    wxpay_post_json(state, config, canonical_url, body).await
}

pub(crate) async fn create_wxpay_direct_checkout(
    state: &AppState,
    input: &DirectPaymentCheckoutInput,
) -> Result<Value, DirectPaymentCheckoutError> {
    let order_no =
        validated_payment_identifier(&input.order_no, "支付订单号", MAX_PAYMENT_ORDER_NO_BYTES)?;
    let config = load_direct_gateway_config(state, "wxpay").await?;
    if !input.pay_currency.eq_ignore_ascii_case(WXPAY_CURRENCY) {
        return Err(DirectPaymentCheckoutError::Failed(
            "微信支付官方直连当前仅支持 CNY".to_string(),
        ));
    }
    let total_fen = wxpay_money_to_fen(input.pay_amount)?;
    let app_id = wxpay_config_string(&config, "app_id")?;
    let mch_id = wxpay_config_string(&config, "mch_id")?;
    let common = json!({
        "appid": app_id,
        "mchid": mch_id,
        "description": input.subject,
        "out_trade_no": order_no,
        "notify_url": input.notify_url,
        "amount": {
            "total": total_fen,
            "currency": WXPAY_CURRENCY,
        },
    });
    match input.payment_channel.as_str() {
        "native" => {
            let value =
                wxpay_post_json(state, &config, "/v3/pay/transactions/native", common).await?;
            let code_url = value
                .get("code_url")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    DirectPaymentCheckoutError::Uncertain(
                        "微信 Native 响应缺少 code_url".to_string(),
                    )
                })?;
            Ok(json!({
                "gateway": "wxpay",
                "display_name": input.display_name,
                "gateway_order_id": order_no,
                "payment_url": Value::Null,
                "submit_method": "qrcode",
                "qr_code": code_url,
                "code_url": code_url,
                "pay_amount": input.pay_amount,
                "pay_currency": input.pay_currency,
                "payment_channel": input.payment_channel,
                "callback_url": input.notify_url,
                "return_url": input.return_url,
                "expires_at": input.expires_at.to_rfc3339(),
            }))
        }
        "h5" => {
            let client_ip = input
                .client_ip
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "微信 H5 支付需要客户端 IP".to_string())?;
            let mut body = common;
            body["scene_info"] = json!({
                "payer_client_ip": client_ip,
                "h5_info": { "type": "Wap" },
            });
            let value = wxpay_post_json(state, &config, "/v3/pay/transactions/h5", body).await?;
            let mut h5_url = value
                .get("h5_url")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    DirectPaymentCheckoutError::Uncertain("微信 H5 响应缺少 h5_url".to_string())
                })?
                .to_string();
            if let Some(return_url) = input.return_url.as_deref() {
                let sep = if h5_url.contains('?') { "&" } else { "?" };
                h5_url = format!(
                    "{h5_url}{sep}redirect_url={}",
                    url::form_urlencoded::byte_serialize(return_url.as_bytes()).collect::<String>()
                );
            }
            Ok(json!({
                "gateway": "wxpay",
                "display_name": input.display_name,
                "gateway_order_id": order_no,
                "payment_url": h5_url,
                "h5_url": h5_url,
                "submit_method": "GET",
                "qr_code": Value::Null,
                "pay_amount": input.pay_amount,
                "pay_currency": input.pay_currency,
                "payment_channel": input.payment_channel,
                "callback_url": input.notify_url,
                "return_url": input.return_url,
                "expires_at": input.expires_at.to_rfc3339(),
            }))
        }
        "jsapi" => Err(DirectPaymentCheckoutError::Failed(
            "微信 JSAPI 需要前端提供 OpenID，当前充值入口尚未接入".to_string(),
        )),
        _ => Err(DirectPaymentCheckoutError::Failed(
            "微信支付通道不可用".to_string(),
        )),
    }
}

pub(crate) async fn create_stripe_direct_checkout(
    state: &AppState,
    input: &DirectPaymentCheckoutInput,
) -> Result<Value, DirectPaymentCheckoutError> {
    let order_no =
        validated_payment_identifier(&input.order_no, "支付订单号", MAX_PAYMENT_ORDER_NO_BYTES)?;
    let config = load_direct_gateway_config(state, "stripe").await?;
    let Some(secret_key) = secret_string(&config.secrets, "secret_key") else {
        return Err("Stripe secret_key 未配置".to_string().into());
    };
    let Some(publishable_key) = config_string(&config.config, "publishable_key") else {
        return Err("Stripe publishable_key 未配置".to_string().into());
    };
    let amount = super::stripe_amount_to_minor(input.pay_amount, &config.record.pay_currency)
        .ok_or_else(|| "Stripe 支付金额无效".to_string())?;
    let currency = config.record.pay_currency.trim().to_ascii_lowercase();
    let mut form = vec![
        ("amount".to_string(), amount.to_string()),
        ("currency".to_string(), currency.clone()),
        ("description".to_string(), input.subject.clone()),
        ("metadata[order_no]".to_string(), order_no),
        (
            "metadata[payment_provider]".to_string(),
            "stripe".to_string(),
        ),
        (
            "metadata[payment_channel]".to_string(),
            input.payment_channel.clone(),
        ),
        (
            "payment_method_types[]".to_string(),
            input.payment_channel.clone(),
        ),
    ];
    if input.payment_channel == "wechat_pay" {
        form.push((
            "payment_method_options[wechat_pay][client]".to_string(),
            "web".to_string(),
        ));
    }
    let stripe_endpoint =
        url::Url::parse("https://api.stripe.com/v1/payment_intents").map_err(|_| {
            DirectPaymentCheckoutError::Uncertain(STRIPE_CHECKOUT_UNCERTAIN_DETAIL.to_string())
        })?;
    let stripe_client = public_payment_http_client(&stripe_endpoint)
        .await
        .map_err(|_| {
            DirectPaymentCheckoutError::Uncertain(STRIPE_CHECKOUT_UNCERTAIN_DETAIL.to_string())
        })?;
    let response = match stripe_client
        .post(stripe_endpoint)
        .header(
            "Idempotency-Key",
            format!("aether-payment-intent-{}", input.order_no),
        )
        .basic_auth(secret_key, Some(""))
        .form(&form)
        .send()
        .await
    {
        Ok(response) => response,
        Err(_) => {
            // Do not surface reqwest's error string: it can contain provider
            // response details, proxy information, or other deployment data.
            tracing::warn!(
                event_name = "stripe_payment_intent_request_failed",
                "Stripe PaymentIntent request failed"
            );
            return Err(DirectPaymentCheckoutError::Uncertain(
                STRIPE_CHECKOUT_UNCERTAIN_DETAIL.to_string(),
            ));
        }
    };
    let status = response.status();
    let body =
        aether_http::read_response_bytes_with_limit(response, MAX_PAYMENT_GATEWAY_RESPONSE_BYTES)
            .await
            .map_err(|_| {
                tracing::warn!(
                    event_name = "stripe_payment_intent_response_read_failed",
                    upstream_status = %status,
                    "Stripe PaymentIntent response could not be read"
                );
                if status.is_server_error() || status == http::StatusCode::TOO_MANY_REQUESTS {
                    DirectPaymentCheckoutError::Uncertain(
                        STRIPE_CHECKOUT_UNCERTAIN_DETAIL.to_string(),
                    )
                } else {
                    DirectPaymentCheckoutError::Failed(STRIPE_CHECKOUT_FAILED_DETAIL.to_string())
                }
            })?;
    if !status.is_success() {
        // Stripe error bodies may contain account identifiers or provider
        // internals.  Classify by status only and keep the body private.
        tracing::warn!(
            event_name = "stripe_payment_intent_upstream_rejected",
            upstream_status = %status,
            "Stripe PaymentIntent request was rejected"
        );
        return Err(
            if status.is_server_error() || status == http::StatusCode::TOO_MANY_REQUESTS {
                DirectPaymentCheckoutError::Uncertain(STRIPE_CHECKOUT_UNCERTAIN_DETAIL.to_string())
            } else {
                DirectPaymentCheckoutError::Failed(STRIPE_CHECKOUT_FAILED_DETAIL.to_string())
            },
        );
    }
    let value = serde_json::from_slice::<Value>(&body)
        .map_err(|_| DirectPaymentCheckoutError::Uncertain("Stripe 响应格式无效".to_string()))?;
    // Stripe retains the response for an idempotency key even after the
    // PaymentIntent is cancelled. Reusing that key would otherwise hand the
    // caller a client secret that can never be confirmed.
    if stripe_checkout_response_is_canceled(&value) {
        return Err(DirectPaymentCheckoutError::Canceled);
    }
    let Some(intent_id) = value.get("id").and_then(Value::as_str) else {
        return Err(DirectPaymentCheckoutError::Uncertain(
            "Stripe 响应缺少 PaymentIntent ID".to_string(),
        ));
    };
    let intent_id = validated_payment_identifier(
        intent_id,
        "Stripe PaymentIntent ID",
        MAX_PAYMENT_GATEWAY_ID_BYTES,
    )
    .map_err(DirectPaymentCheckoutError::Uncertain)?;
    let Some(client_secret) = value.get("client_secret").and_then(Value::as_str) else {
        return Err(DirectPaymentCheckoutError::Uncertain(
            "Stripe 响应缺少 client_secret".to_string(),
        ));
    };
    Ok(json!({
        "gateway": "stripe",
        "display_name": input.display_name,
        "gateway_order_id": intent_id,
        "intent_id": intent_id,
        "client_secret": client_secret,
        "publishable_key": publishable_key,
        "expires_at": input.expires_at.to_rfc3339(),
        "pay_amount": input.pay_amount,
        "pay_currency": config.record.pay_currency,
        "payment_channel": input.payment_channel,
        "payment_method_types": [input.payment_channel],
        "submit_method": "stripe_payment_intent"
    }))
}

fn wxpay_header(headers: &http::HeaderMap, name: &str) -> Result<String, String> {
    crate::headers::header_value_str(headers, name)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("微信支付通知缺少 {name}"))
}

fn wxpay_verify_notify_headers(
    config: &DirectGatewayConfig,
    headers: &http::HeaderMap,
    body: &[u8],
) -> Result<(), String> {
    let signature = wxpay_header(headers, "wechatpay-signature")?;
    let timestamp = wxpay_header(headers, "wechatpay-timestamp")?;
    let timestamp_unix = timestamp
        .parse::<i64>()
        .map_err(|_| "微信支付通知时间戳无效".to_string())?;
    if Utc::now().timestamp().abs_diff(timestamp_unix) > WXPAY_SIGNATURE_TOLERANCE_SECONDS as u64 {
        return Err("微信支付通知已过期".to_string());
    }
    let nonce = wxpay_header(headers, "wechatpay-nonce")?;
    let serial = wxpay_header(headers, "wechatpay-serial")?;
    if let Some(expected) = config_string(&config.config, "public_key_id") {
        if serial != expected {
            return Err("微信支付通知公钥 ID 不匹配".to_string());
        }
    }
    let body = std::str::from_utf8(body).map_err(|_| "微信支付通知体不是 UTF-8".to_string())?;
    let message = format!("{timestamp}\n{nonce}\n{body}\n");
    let public_key = wxpay_secret_string(config, "public_key")?;
    if rsa_sha256_verify_base64(&public_key, &message, &signature)? {
        Ok(())
    } else {
        Err("微信支付通知签名无效".to_string())
    }
}

fn wxpay_decrypt_resource(config: &DirectGatewayConfig, resource: &Value) -> Result<Value, String> {
    let algorithm = resource
        .get("algorithm")
        .and_then(Value::as_str)
        .unwrap_or("");
    if algorithm != "AEAD_AES_256_GCM" {
        return Err("微信支付通知加密算法不支持".to_string());
    }
    let api_v3_key = wxpay_secret_string(config, "api_v3_key")?;
    if api_v3_key.as_bytes().len() != 32 {
        return Err("微信支付 api_v3_key 必须为 32 字节".to_string());
    }
    let nonce = resource
        .get("nonce")
        .and_then(Value::as_str)
        .ok_or_else(|| "微信支付通知 resource.nonce 缺失".to_string())?;
    if nonce.as_bytes().len() != 12 {
        return Err("微信支付通知 resource.nonce 长度无效".to_string());
    }
    let associated_data = resource
        .get("associated_data")
        .and_then(Value::as_str)
        .unwrap_or("");
    let ciphertext = resource
        .get("ciphertext")
        .and_then(Value::as_str)
        .ok_or_else(|| "微信支付通知 resource.ciphertext 缺失".to_string())?;
    let ciphertext = decode_payment_base64_with_limit(
        ciphertext,
        MAX_PAYMENT_CALLBACK_CIPHERTEXT_BYTES,
        "微信支付通知密文解码失败",
    )?;
    let cipher = Aes256Gcm::new_from_slice(api_v3_key.as_bytes())
        .map_err(|_| "微信支付 api_v3_key 无效".to_string())?;
    let plaintext = cipher
        .decrypt(
            Nonce::from_slice(nonce.as_bytes()),
            Payload {
                msg: &ciphertext,
                aad: associated_data.as_bytes(),
            },
        )
        .map_err(|_| "微信支付通知解密失败".to_string())?;
    serde_json::from_slice::<Value>(&plaintext).map_err(|_| "微信支付通知明文格式无效".to_string())
}

fn wxpay_notify_payment_channel(tx: &Value) -> Result<String, String> {
    let trade_type = tx
        .get("trade_type")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "微信支付通知缺少 trade_type".to_string())?;
    match trade_type.to_ascii_uppercase().as_str() {
        "NATIVE" => Ok("native".to_string()),
        "MWEB" => Ok("h5".to_string()),
        "JSAPI" => Ok("jsapi".to_string()),
        _ => Err("微信支付通知 trade_type 不受支持".to_string()),
    }
}

pub(crate) async fn verify_wxpay_notify_callback(
    state: &AppState,
    headers: &http::HeaderMap,
    body: &[u8],
) -> Result<aether_data::repository::wallet::ProcessPaymentCallbackInput, String> {
    let config = load_direct_gateway_callback_config(state, "wxpay").await?;
    wxpay_verify_notify_headers(&config, headers, body)?;
    let payload =
        serde_json::from_slice::<Value>(body).map_err(|_| "微信支付通知请求体无效".to_string())?;
    if payload.get("event_type").and_then(Value::as_str) != Some(WXPAY_NOTIFY_SUCCESS) {
        return Err("微信支付通知不是成功支付事件".to_string());
    }
    let tx = wxpay_decrypt_resource(
        &config,
        payload
            .get("resource")
            .ok_or_else(|| "微信支付通知缺少 resource".to_string())?,
    )?;
    if tx.get("trade_state").and_then(Value::as_str) != Some(WXPAY_TRADE_SUCCESS) {
        return Err("微信支付交易不是成功状态".to_string());
    }
    let expected_app_id = wxpay_config_string(&config, "app_id")?;
    let expected_mch_id = wxpay_config_string(&config, "mch_id")?;
    if tx.get("appid").and_then(Value::as_str) != Some(expected_app_id.as_str()) {
        return Err("微信支付通知 appid 不匹配".to_string());
    }
    if tx.get("mchid").and_then(Value::as_str) != Some(expected_mch_id.as_str()) {
        return Err("微信支付通知 mchid 不匹配".to_string());
    }
    let payment_channel = wxpay_notify_payment_channel(&tx)?;
    let order_no = tx
        .get("out_trade_no")
        .and_then(Value::as_str)
        .map(|value| {
            validated_payment_identifier(
                value,
                "微信支付通知 out_trade_no",
                MAX_PAYMENT_ORDER_NO_BYTES,
            )
        })
        .transpose()?
        .ok_or_else(|| "微信支付通知缺少 out_trade_no".to_string())?;
    let transaction_id = validated_optional_payment_identifier(
        tx.get("transaction_id").and_then(Value::as_str),
        "微信支付通知 transaction_id",
        MAX_PAYMENT_GATEWAY_ID_BYTES,
    )?
    .ok_or_else(|| "微信支付通知缺少 transaction_id".to_string())?;
    let amount_fen = tx
        .get("amount")
        .and_then(|value| value.get("total"))
        .and_then(Value::as_i64)
        .filter(|value| *value > 0)
        .ok_or_else(|| "微信支付通知金额无效".to_string())?;
    let currency = tx
        .get("amount")
        .and_then(|value| value.get("currency"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "微信支付通知缺少币种".to_string())?;
    if !currency.eq_ignore_ascii_case(WXPAY_CURRENCY) {
        return Err("微信支付通知币种不匹配".to_string());
    }
    let pay_amount = amount_fen as f64 / 100.0;
    let order = find_payment_callback_order(state, &order_no).await?;
    let (amount_usd, exchange_rate) = payment_callback_settlement_values(
        order.as_ref(),
        pay_amount,
        Some(config.record.usd_exchange_rate),
    )?;
    let payload_hash = payment_bytes_hash(body);
    let callback_key = payment_callback_key(
        "wxpay",
        payload
            .get("id")
            .and_then(Value::as_str)
            .or(Some(transaction_id.as_str())),
        &payload_hash,
    )?;
    let callback_payload = payment_callback_projection(
        "wxpay",
        &order_no,
        Some(&transaction_id),
        pay_amount,
        WXPAY_CURRENCY,
        "success",
    );
    Ok(
        aether_data::repository::wallet::ProcessPaymentCallbackInput {
            payment_method: "wxpay".to_string(),
            payment_provider: Some("wxpay".to_string()),
            payment_channel: Some(payment_channel),
            callback_key,
            order_no: Some(order_no),
            gateway_order_id: Some(transaction_id),
            amount_usd,
            pay_amount: Some(pay_amount),
            pay_currency: Some(WXPAY_CURRENCY.to_string()),
            exchange_rate,
            payload_hash,
            payload: callback_payload,
            signature_valid: true,
        },
    )
}

pub(crate) async fn close_direct_gateway_checkout(
    state: &AppState,
    payment_provider: &str,
    order_no: &str,
    gateway_order_id: Option<&str>,
) -> Result<Option<Value>, String> {
    match payment_provider.trim().to_ascii_lowercase().as_str() {
        "alipay" => {
            let order_no =
                validated_payment_identifier(order_no, "支付订单号", MAX_PAYMENT_ORDER_NO_BYTES)?;
            let config = load_direct_gateway_config(state, "alipay").await?;
            let params = alipay_signed_params(
                &config,
                "alipay.trade.close",
                json!({ "out_trade_no": order_no }),
                None,
                None,
            )?;
            let value = alipay_post(state, &config, &params).await?;
            let response = alipay_response_success(&value, "alipay_trade_close_response")?;
            let gateway_order_id = validated_optional_payment_identifier(
                response.get("trade_no").and_then(Value::as_str),
                "支付宝 trade_no",
                MAX_PAYMENT_GATEWAY_ID_BYTES,
            )?;
            Ok(Some(json!({
                "gateway": "alipay",
                "closed": true,
                "gateway_order_id": gateway_order_id,
            })))
        }
        "wxpay" => {
            let order_no =
                validated_payment_identifier(order_no, "支付订单号", MAX_PAYMENT_ORDER_NO_BYTES)?;
            let config = load_direct_gateway_config(state, "wxpay").await?;
            let canonical_url = format!("/v3/pay/transactions/out-trade-no/{order_no}/close");
            wxpay_post_empty_success(
                state,
                &config,
                &canonical_url,
                json!({ "mchid": wxpay_config_string(&config, "mch_id")? }),
            )
            .await
            .map_err(DirectPaymentRequestError::into_detail)?;
            let gateway_order_id = validated_optional_payment_identifier(
                gateway_order_id,
                "微信支付 transaction_id",
                MAX_PAYMENT_GATEWAY_ID_BYTES,
            )?;
            Ok(Some(json!({
                "gateway": "wxpay",
                "closed": true,
                "gateway_order_id": gateway_order_id,
            })))
        }
        "stripe" => {
            let intent_id = validated_optional_payment_identifier(
                gateway_order_id,
                "Stripe PaymentIntent ID",
                MAX_PAYMENT_GATEWAY_ID_BYTES,
            )?
            .ok_or_else(|| "Stripe PaymentIntent ID 缺失，无法取消支付".to_string())?;
            let config = load_direct_gateway_config(state, "stripe").await?;
            let secret_key = secret_string(&config.secrets, "secret_key")
                .ok_or_else(|| "Stripe secret_key 未配置".to_string())?;
            let stripe_endpoint = url::Url::parse(&format!(
                "https://api.stripe.com/v1/payment_intents/{intent_id}/cancel"
            ))
            .map_err(|_| "Stripe PaymentIntent 取消 URL 无效".to_string())?;
            let stripe_client = public_payment_http_client(&stripe_endpoint).await?;
            let response = stripe_client
                .post(stripe_endpoint)
                .basic_auth(secret_key, Some(""))
                .send()
                .await
                .map_err(|_| "Stripe PaymentIntent 取消失败".to_string())?;
            let status = response.status();
            let body = aether_http::read_response_bytes_with_limit(
                response,
                MAX_PAYMENT_GATEWAY_RESPONSE_BYTES,
            )
            .await
            .map_err(|_| "Stripe 取消响应读取失败".to_string())?;
            let value = serde_json::from_slice::<Value>(&body)
                .map_err(|_| "Stripe 取消响应格式无效".to_string())?;
            if !status.is_success() {
                return Err("Stripe PaymentIntent 取消失败".to_string());
            }
            if value.get("id").and_then(Value::as_str) != Some(intent_id.as_str())
                || value.get("status").and_then(Value::as_str) != Some("canceled")
            {
                return Err("Stripe PaymentIntent 取消结果无效".to_string());
            }
            Ok(Some(json!({
                "gateway": "stripe",
                "closed": true,
                "gateway_order_id": intent_id,
            })))
        }
        _ => Ok(None),
    }
}

pub(crate) async fn close_direct_gateway_order(
    state: &AppState,
    order: &crate::AdminWalletPaymentOrderRecord,
) -> Result<Option<Value>, String> {
    close_direct_gateway_checkout(
        state,
        &order.payment_method,
        &order.order_no,
        order.gateway_order_id.as_deref(),
    )
    .await
}

fn refund_pay_amount(
    order: &crate::AdminWalletPaymentOrderRecord,
    amount_usd: f64,
) -> Result<f64, String> {
    if amount_usd <= 0.0 || !amount_usd.is_finite() {
        return Err("退款金额无效".to_string());
    }
    let total_pay_amount = order.pay_amount.unwrap_or_else(|| {
        let exchange_rate = order.exchange_rate.unwrap_or(1.0);
        order.amount_usd * exchange_rate
    });
    if !order.amount_usd.is_finite()
        || order.amount_usd <= 0.0
        || !total_pay_amount.is_finite()
        || total_pay_amount <= 0.0
    {
        return Err("原支付订单金额无效".to_string());
    }
    Ok((amount_usd * total_pay_amount / order.amount_usd * 100.0).round() / 100.0)
}

pub(crate) async fn refund_direct_gateway_order(
    state: &AppState,
    order: &crate::AdminWalletPaymentOrderRecord,
    refund_no: &str,
    amount_usd: f64,
    reason: Option<&str>,
) -> Result<Option<DirectGatewayRefundResult>, String> {
    match order.payment_method.as_str() {
        "alipay" => {
            let order_no = validated_payment_identifier(
                &order.order_no,
                "支付订单号",
                MAX_PAYMENT_ORDER_NO_BYTES,
            )?;
            let refund_no =
                validated_payment_identifier(refund_no, "退款单号", MAX_PAYMENT_ORDER_NO_BYTES)?;
            let config = load_direct_gateway_config(state, "alipay").await?;
            if !config.record.pay_currency.eq_ignore_ascii_case("CNY") {
                return Err("支付宝退款币种配置无效".to_string());
            }
            let refund_amount = refund_pay_amount(order, amount_usd)?;
            let params = alipay_signed_params(
                &config,
                "alipay.trade.refund",
                json!({
                    "out_trade_no": order_no,
                    "refund_amount": format!("{refund_amount:.2}"),
                    "refund_reason": reason.unwrap_or("wallet refund"),
                    "out_request_no": refund_no,
                }),
                None,
                None,
            )?;
            let value = alipay_post(state, &config, &params).await?;
            let response = alipay_response_success(&value, "alipay_trade_refund_response")?;
            // Alipay's `trade_no` identifies the original payment transaction,
            // not this refund. `out_request_no` is the merchant-scoped,
            // idempotent refund identifier and is what we persist.
            let gateway_refund_id = validated_payment_identifier(
                &refund_no,
                "支付宝退款 ID",
                MAX_PAYMENT_GATEWAY_ID_BYTES,
            )?;
            let status = "success".to_string();
            let proof = gateway_refund_proof(
                "alipay",
                &gateway_refund_id,
                &status,
                &order_no,
                &refund_no,
                refund_amount,
                "CNY",
            );
            Ok(Some(DirectGatewayRefundResult {
                gateway_refund_id,
                status,
                proof,
            }))
        }
        "wxpay" => {
            let order_no = validated_payment_identifier(
                &order.order_no,
                "支付订单号",
                MAX_PAYMENT_ORDER_NO_BYTES,
            )?;
            let refund_no =
                validated_payment_identifier(refund_no, "退款单号", MAX_PAYMENT_ORDER_NO_BYTES)?;
            let config = load_direct_gateway_config(state, "wxpay").await?;
            if !config
                .record
                .pay_currency
                .eq_ignore_ascii_case(WXPAY_CURRENCY)
            {
                return Err("微信支付退款币种配置无效".to_string());
            }
            let total_pay_amount = order.pay_amount.ok_or_else(|| {
                "微信支付退款需要原订单 pay_amount，请确认订单已通过官方直连创建".to_string()
            })?;
            let refund_amount = refund_pay_amount(order, amount_usd)?;
            let value = wxpay_post_json(
                state,
                &config,
                "/v3/refund/domestic/refunds",
                json!({
                    "out_trade_no": order_no,
                    "out_refund_no": refund_no,
                    "reason": reason.unwrap_or("wallet refund"),
                    "amount": {
                        "refund": wxpay_money_to_fen(refund_amount)?,
                        "total": wxpay_money_to_fen(total_pay_amount)?,
                        "currency": WXPAY_CURRENCY,
                    },
                }),
            )
            .await
            .map_err(DirectPaymentRequestError::into_detail)?;
            let gateway_refund_id = value
                .get("refund_id")
                .and_then(Value::as_str)
                .unwrap_or(&refund_no);
            let gateway_refund_id = validated_payment_identifier(
                gateway_refund_id,
                "微信支付退款 ID",
                MAX_PAYMENT_GATEWAY_ID_BYTES,
            )?;
            let status =
                wxpay_refund_status(value.get("status").and_then(Value::as_str)).to_string();
            let proof = gateway_refund_proof(
                "wxpay",
                &gateway_refund_id,
                &status,
                &order_no,
                &refund_no,
                refund_amount,
                WXPAY_CURRENCY,
            );
            Ok(Some(DirectGatewayRefundResult {
                gateway_refund_id,
                status,
                proof,
            }))
        }
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        alipay_precreate_business_refusal, decode_payment_base64_with_limit, gateway_refund_proof,
        payment_callback_key, payment_callback_projection, payment_payload_hash,
        public_payment_http_client, rsa_sha256_sign_base64, rsa_sha256_verify_base64,
        validate_public_payment_resolved_addrs, validated_payment_identifier,
        wxpay_notify_payment_channel, wxpay_refund_status, DirectGatewayConfig,
        DirectGatewayRefundResult, MAX_PAYMENT_GATEWAY_ID_BYTES,
    };
    use aws_lc_rs::encoding::{AsDer, Pkcs8V1Der, PublicKeyX509Der};
    use aws_lc_rs::rsa::{KeyPair as AwsRsaKeyPair, KeySize};
    use aws_lc_rs::signature::KeyPair as _;
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine as _;
    use serde_json::json;
    use std::net::SocketAddr;

    #[test]
    fn direct_gateway_config_debug_output_redacts_decrypted_secrets() {
        let config = DirectGatewayConfig {
            record: aether_data_contracts::repository::billing::PaymentGatewayConfigRecord {
                provider: "stripe".to_string(),
                enabled: true,
                endpoint_url: "https://example.test".to_string(),
                callback_base_url: None,
                merchant_id: "merchant".to_string(),
                merchant_key_encrypted: Some("gateway-ciphertext-canary".to_string()),
                pay_currency: "USD".to_string(),
                usd_exchange_rate: 1.0,
                min_recharge_usd: 1.0,
                channels_json: json!({}),
                created_at_unix_secs: 1,
                updated_at_unix_secs: 1,
            },
            config: serde_json::Map::from_iter([(
                "private_config".to_string(),
                json!("gateway-config-canary"),
            )]),
            secrets: serde_json::Map::from_iter([(
                "private_key".to_string(),
                json!("gateway-plaintext-secret-canary"),
            )]),
        };

        let debug = format!("{config:?}");
        assert!(debug.contains("[REDACTED]"));
        for secret in [
            "gateway-ciphertext-canary",
            "gateway-config-canary",
            "gateway-plaintext-secret-canary",
        ] {
            assert!(
                !debug.contains(secret),
                "debug output leaked {secret}: {debug}"
            );
        }
    }

    #[test]
    fn payment_rsa_sha256_keeps_pem_and_base64_compatibility() {
        let key_pair = AwsRsaKeyPair::generate(KeySize::Rsa2048)
            .expect("2048-bit test RSA private key should generate");
        let pkcs8 = AsDer::<Pkcs8V1Der<'static>>::as_der(&key_pair)
            .expect("test RSA private key should encode as PKCS#8");
        let spki = AsDer::<PublicKeyX509Der<'static>>::as_der(key_pair.public_key())
            .expect("test RSA public key should encode as SPKI");
        let private_key = format!(
            "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----",
            STANDARD.encode(pkcs8.as_ref())
        );
        let public_key = STANDARD.encode(spki.as_ref());
        let message = "Aether payment signature compatibility";
        let signature =
            rsa_sha256_sign_base64(&private_key, message).expect("PKCS#8 PEM key should sign");

        assert!(rsa_sha256_verify_base64(&public_key, message, &signature)
            .expect("bare-base64 SPKI key should verify"));
        assert!(
            !rsa_sha256_verify_base64(&public_key, "tampered", &signature)
                .expect("valid but mismatched signature should return false")
        );
        assert_eq!(
            rsa_sha256_verify_base64(&public_key, message, "not-base64"),
            Err("签名 base64 解码失败".to_string())
        );
    }

    #[test]
    fn payment_base64_decode_enforces_its_allocation_limit() {
        assert_eq!(
            decode_payment_base64_with_limit("YWI=", 2, "invalid").expect("two decoded bytes"),
            b"ab"
        );
        assert_eq!(
            decode_payment_base64_with_limit("AAAA", 2, "invalid"),
            Err("invalid".to_string())
        );
        assert_eq!(
            decode_payment_base64_with_limit("AAAAA", 2, "invalid"),
            Err("invalid".to_string())
        );
    }

    #[test]
    fn wxpay_notify_trade_type_maps_to_the_stored_checkout_channel() {
        for (trade_type, expected) in [
            ("NATIVE", "native"),
            ("MWEB", "h5"),
            ("JSAPI", "jsapi"),
            (" native ", "native"),
        ] {
            assert_eq!(
                wxpay_notify_payment_channel(&json!({ "trade_type": trade_type }))
                    .expect("supported trade type should map"),
                expected
            );
        }
    }

    #[test]
    fn wxpay_notify_trade_type_is_required_and_rejects_uncreated_channels() {
        assert!(wxpay_notify_payment_channel(&json!({})).is_err());
        assert!(wxpay_notify_payment_channel(&json!({ "trade_type": "APP" })).is_err());
    }

    #[test]
    fn direct_refund_requires_an_explicit_success_terminal_state() {
        for status in ["PROCESSING", "pending"] {
            let result = DirectGatewayRefundResult {
                gateway_refund_id: "refund-1".to_string(),
                status: status.to_string(),
                proof: json!({}),
            };
            assert!(result.is_pending());
            assert!(!result.is_succeeded());
        }

        for status in ["SUCCESS", "success"] {
            let result = DirectGatewayRefundResult {
                gateway_refund_id: "refund-1".to_string(),
                status: status.to_string(),
                proof: json!({}),
            };
            assert!(result.is_succeeded());
            assert!(!result.is_pending());
        }

        for status in ["CLOSED", "ABNORMAL", "unknown"] {
            let result = DirectGatewayRefundResult {
                gateway_refund_id: "refund-1".to_string(),
                status: status.to_string(),
                proof: json!({}),
            };
            assert!(!result.is_succeeded());
            assert!(!result.is_pending());
        }
    }

    #[test]
    fn payment_identifiers_reject_secret_bearing_or_oversized_values() {
        for value in [
            "Authorization: Bearer top-secret",
            "https://internal.example/refund?token=top-secret",
            "payer/openid",
            "refund id with spaces",
        ] {
            assert!(validated_payment_identifier(value, "test", 128).is_err());
        }
        assert!(validated_payment_identifier(
            &"a".repeat(MAX_PAYMENT_GATEWAY_ID_BYTES + 1),
            "test",
            MAX_PAYMENT_GATEWAY_ID_BYTES,
        )
        .is_err());
        assert_eq!(
            validated_payment_identifier(" rf_123-ABC ", "test", 128)
                .expect("safe identifier should pass"),
            "rf_123-ABC"
        );
    }

    #[test]
    fn callback_projection_contains_only_the_persistence_allowlist() {
        let raw = json!({
            "authorization": "Bearer payment-secret",
            "url": "https://internal.example/callback?token=secret",
            "payer": {"openid": "openid-secret"},
            "credential": "gateway-credential"
        });
        let raw_hash = payment_payload_hash(&raw).expect("raw callback should be hashable");
        assert_eq!(raw_hash.len(), 64);

        let projection = payment_callback_projection(
            "wxpay",
            "order-1",
            Some("transaction-1"),
            12.34,
            "CNY",
            "success",
        );
        let object = projection
            .as_object()
            .expect("callback projection should be an object");
        assert_eq!(object.len(), 8);
        for key in [
            "gateway",
            "order_no",
            "gateway_order_id",
            "amount",
            "currency",
            "status",
            "signature_valid",
            "processed_at",
        ] {
            assert!(object.contains_key(key), "missing safe field: {key}");
        }
        let encoded = projection.to_string();
        for sensitive in [
            "payment-secret",
            "?token=secret",
            "openid-secret",
            "gateway-credential",
            "payer",
            "openid",
            "credential",
            "Bearer",
        ] {
            assert!(!encoded.contains(sensitive));
        }
    }

    #[test]
    fn gateway_refund_proof_contains_only_fixed_fields() {
        let proof = gateway_refund_proof(
            "alipay",
            "gateway-refund-1",
            "success",
            "order-1",
            "refund-1",
            8.5,
            "CNY",
        );
        let object = proof
            .as_object()
            .expect("gateway refund proof should be an object");
        assert_eq!(object.len(), 8);
        assert!(object
            .get("processed_at")
            .and_then(|v| v.as_str())
            .is_some());
        for forbidden in ["payload", "payer", "openid", "credential", "message"] {
            assert!(!proof.to_string().contains(forbidden));
        }
    }

    #[test]
    fn callback_keys_and_wxpay_refund_statuses_are_bounded_allowlists() {
        let hash = "a".repeat(64);
        let long_event_id = "b".repeat(MAX_PAYMENT_GATEWAY_ID_BYTES);
        assert_eq!(
            payment_callback_key("wxpay", Some(&long_event_id), &hash)
                .expect("valid long event IDs should use a bounded hash key"),
            format!("wxpay:{hash}")
        );
        assert_eq!(wxpay_refund_status(Some("SUCCESS")), "success");
        assert_eq!(wxpay_refund_status(Some("PROCESSING")), "processing");
        for status in [
            Some("CLOSED"),
            Some("ABNORMAL"),
            Some("credential=secret"),
            None,
        ] {
            assert_eq!(wxpay_refund_status(status), "failed");
        }
    }

    #[test]
    fn stripe_checkout_does_not_replay_cancelled_idempotent_intent() {
        assert!(super::stripe_checkout_response_is_canceled(&json!({
            "id": "pi_cancelled",
            "status": "canceled",
            "client_secret": "pi_cancelled_secret"
        })));
        assert!(super::stripe_checkout_response_is_canceled(&json!({
            "status": " CANCELED "
        })));
        assert!(!super::stripe_checkout_response_is_canceled(&json!({
            "status": "requires_payment_method"
        })));
        assert!(!super::stripe_checkout_response_is_canceled(&json!({
            "id": "pi_missing_status"
        })));
    }

    #[test]
    fn alipay_precreate_fallback_requires_a_parsed_business_refusal() {
        assert!(alipay_precreate_business_refusal(&json!({
            "code": "40004",
            "msg": "Business Failed",
            "sub_code": "ACQ.INVALID_PARAMETER"
        })));
        // Alipay sometimes omits sub_code for a regular 4xx rejection. The
        // HTTP response is still an explicit business refusal in that case.
        assert!(alipay_precreate_business_refusal(&json!({
            "code": "40001"
        })));

        for response in [
            json!({"code": "20000", "sub_code": "ACQ.SYSTEM_ERROR"}),
            json!({"code": "40004", "sub_code": "ACQ.SYSTEM_ERROR"}),
            json!({"code": "40004", "sub_code": "REQUEST_TIMEOUT"}),
            json!({"code": "10000", "qr_code": "https://qr.example"}),
            json!({"msg": "missing code"}),
        ] {
            assert!(
                !alipay_precreate_business_refusal(&response),
                "response must not trigger a page-pay fallback: {response}"
            );
        }
    }

    #[tokio::test]
    async fn payment_http_client_rejects_private_targets_before_connecting() {
        for target in [
            "https://127.0.0.1/payment",
            "https://169.254.169.254/latest/meta-data",
            "https://[::1]/payment",
        ] {
            let url = url::Url::parse(target).expect("test URL should parse");
            assert!(
                public_payment_http_client(&url).await.is_err(),
                "private payment target should fail: {target}"
            );
        }
    }

    #[test]
    fn stripe_api_origin_allows_only_benchmarking_addresses() {
        let fake = SocketAddr::from(([198, 18, 75, 234], 443));
        for raw_url in [
            "https://api.stripe.com/v1/payment_intents",
            "https://API.STRIPE.COM:443/v1/payment_intents",
        ] {
            let url = url::Url::parse(raw_url).expect("Stripe URL should parse");
            assert!(validate_public_payment_resolved_addrs(&url, &[fake]).is_ok());
        }
        assert!(validate_public_payment_resolved_addrs(
            &url::Url::parse("https://api.stripe.com/v1/payment_intents").unwrap(),
            &[fake, SocketAddr::from(([127, 0, 0, 1], 443))],
        )
        .is_err());
    }

    #[test]
    fn custom_or_non_default_payment_origins_reject_benchmarking_addresses() {
        let fake = SocketAddr::from(([198, 18, 75, 234], 443));
        for raw_url in [
            "https://payments.example.test/v1/payment_intents",
            "https://api.stripe.com:8443/v1/payment_intents",
            "http://api.stripe.com/v1/payment_intents",
            "https://api.stripe.com.evil.test/v1/payment_intents",
            "https://api.stripe.com/v1/payment_intents?redirect=internal",
            "https://api.stripe.com/v1/payment_intents#fragment",
        ] {
            let url = url::Url::parse(raw_url).expect("test URL should parse");
            assert!(validate_public_payment_resolved_addrs(&url, &[fake]).is_err());
        }
    }
}
