use crate::AppState;
use aes_gcm::{
    aead::{Aead, Payload},
    Aes256Gcm, KeyInit, Nonce,
};
use axum::http;
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use chrono::Utc;
use rsa::pkcs1::{DecodeRsaPrivateKey, DecodeRsaPublicKey};
use rsa::pkcs1v15::{
    Signature as RsaPkcs1v15Signature, SigningKey as RsaPkcs1v15SigningKey,
    VerifyingKey as RsaPkcs1v15VerifyingKey,
};
use rsa::pkcs8::{DecodePrivateKey, DecodePublicKey};
use rsa::signature::{SignatureEncoding, Signer, Verifier};
use rsa::{RsaPrivateKey, RsaPublicKey};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const ALIPAY_DEFAULT_GATEWAY_URL: &str = "https://openapi.alipay.com/gateway.do";
const WXPAY_DEFAULT_BASE_URL: &str = "https://api.mch.weixin.qq.com";
const WXPAY_NOTIFY_SUCCESS: &str = "TRANSACTION.SUCCESS";
const WXPAY_TRADE_SUCCESS: &str = "SUCCESS";
const WXPAY_CURRENCY: &str = "CNY";

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
    pub(crate) payload: Value,
}

#[derive(Debug, Clone)]
struct DirectGatewayConfig {
    record: aether_data_contracts::repository::billing::PaymentGatewayConfigRecord,
    config: serde_json::Map<String, Value>,
    secrets: serde_json::Map<String, Value>,
}

fn payment_payload_hash(payload: &Value) -> Result<String, String> {
    let encoded = serde_json::to_vec(payload)
        .map_err(|err| format!("payment callback payload encode failed: {err}"))?;
    let digest = Sha256::digest(&encoded);
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
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
    let record = state
        .find_payment_gateway_config(provider)
        .await
        .map_err(|err| format!("{provider} 配置读取失败: {err:?}"))?
        .ok_or_else(|| format!("{provider} 未配置"))?;
    if !record.enabled {
        return Err(format!("{provider} 未启用"));
    }
    let Some(encrypted) = record.merchant_key_encrypted.as_deref() else {
        return Err(format!("{provider} 密钥未配置"));
    };
    let Some(plaintext) =
        super::decrypt_catalog_secret_with_fallbacks(state.encryption_key(), encrypted)
    else {
        return Err(format!("{provider} 密钥解密失败"));
    };
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

fn pem_candidates(raw: &str, labels: &[&str]) -> Vec<String> {
    let trimmed = raw.trim();
    if trimmed.starts_with("-----BEGIN") {
        return vec![trimmed.to_string()];
    }
    labels
        .iter()
        .map(|label| format!("-----BEGIN {label}-----\n{trimmed}\n-----END {label}-----"))
        .collect()
}

fn decode_rsa_private_key(raw: &str) -> Result<RsaPrivateKey, String> {
    let mut errors = Vec::new();
    for candidate in pem_candidates(raw, &["PRIVATE KEY", "RSA PRIVATE KEY"]) {
        match RsaPrivateKey::from_pkcs8_pem(&candidate) {
            Ok(key) => return Ok(key),
            Err(err) => errors.push(format!("pkcs8: {err}")),
        }
        match RsaPrivateKey::from_pkcs1_pem(&candidate) {
            Ok(key) => return Ok(key),
            Err(err) => errors.push(format!("pkcs1: {err}")),
        }
    }
    Err(format!("RSA 私钥解析失败: {}", errors.join("; ")))
}

fn decode_rsa_public_key(raw: &str) -> Result<RsaPublicKey, String> {
    let mut errors = Vec::new();
    for candidate in pem_candidates(raw, &["PUBLIC KEY", "RSA PUBLIC KEY"]) {
        match RsaPublicKey::from_public_key_pem(&candidate) {
            Ok(key) => return Ok(key),
            Err(err) => errors.push(format!("spki: {err}")),
        }
        match RsaPublicKey::from_pkcs1_pem(&candidate) {
            Ok(key) => return Ok(key),
            Err(err) => errors.push(format!("pkcs1: {err}")),
        }
    }
    Err(format!("RSA 公钥解析失败: {}", errors.join("; ")))
}

fn rsa_sha256_sign_base64(private_key: &str, message: &str) -> Result<String, String> {
    let private_key = decode_rsa_private_key(private_key)?;
    let signing_key = RsaPkcs1v15SigningKey::<Sha256>::new(private_key);
    let signature = signing_key.sign(message.as_bytes());
    Ok(BASE64_STANDARD.encode(signature.to_bytes()))
}

fn rsa_sha256_verify_base64(
    public_key: &str,
    message: &str,
    signature_base64: &str,
) -> Result<bool, String> {
    let public_key = decode_rsa_public_key(public_key)?;
    let signature_bytes = BASE64_STANDARD
        .decode(signature_base64.trim())
        .map_err(|err| format!("签名 base64 解码失败: {err}"))?;
    let signature = RsaPkcs1v15Signature::try_from(signature_bytes.as_slice())
        .map_err(|err| format!("签名格式无效: {err}"))?;
    let verifying_key = RsaPkcs1v15VerifyingKey::<Sha256>::new(public_key);
    Ok(verifying_key.verify(message.as_bytes(), &signature).is_ok())
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
            .map_err(|err| format!("支付宝 biz_content 编码失败: {err}"))?,
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
    let mut url = url::Url::parse(base).map_err(|err| format!("支付网关地址无效: {err}"))?;
    {
        let mut query = url.query_pairs_mut();
        for (key, value) in params {
            query.append_pair(key, value);
        }
    }
    Ok(url.to_string())
}

async fn alipay_post(
    state: &AppState,
    config: &DirectGatewayConfig,
    params: &BTreeMap<String, String>,
) -> Result<Value, String> {
    let response = state
        .client
        .post(alipay_gateway_url(config))
        .form(params)
        .send()
        .await
        .map_err(|err| format!("支付宝请求失败: {err}"))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|err| format!("支付宝响应读取失败: {err}"))?;
    let value =
        serde_json::from_str::<Value>(&body).map_err(|_| "支付宝响应格式无效".to_string())?;
    if !status.is_success() {
        return Err(format!("支付宝 HTTP 状态异常: {status}"));
    }
    Ok(value)
}

fn alipay_response_success<'a>(value: &'a Value, key: &str) -> Result<&'a Value, String> {
    let response = value
        .get(key)
        .ok_or_else(|| "支付宝响应缺少业务结果".to_string())?;
    if response.get("code").and_then(Value::as_str) == Some("10000") {
        return Ok(response);
    }
    let message = response
        .get("sub_msg")
        .or_else(|| response.get("msg"))
        .and_then(Value::as_str)
        .unwrap_or("支付宝业务请求失败");
    Err(message.to_string())
}

pub(crate) async fn create_alipay_direct_checkout(
    state: &AppState,
    input: &DirectPaymentCheckoutInput,
) -> Result<Value, String> {
    let config = load_direct_gateway_config(state, "alipay").await?;
    if !input.pay_currency.eq_ignore_ascii_case("CNY") {
        return Err("支付宝官方直连当前仅支持 CNY".to_string());
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
                "out_trade_no": input.order_no,
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
            "gateway_order_id": input.order_no,
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
                "out_trade_no": input.order_no,
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
            "gateway_order_id": input.order_no,
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
            "out_trade_no": input.order_no,
            "total_amount": total_amount,
            "subject": input.subject,
            "product_code": "FACE_TO_FACE_PAYMENT",
        }),
        Some(&input.notify_url),
        None,
    )?;
    match alipay_post(state, &config, &params)
        .await
        .and_then(|value| {
            let response = alipay_response_success(&value, "alipay_trade_precreate_response")?;
            response
                .get("qr_code")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|qr_code| {
                    json!({
                        "gateway": "alipay",
                        "display_name": input.display_name,
                        "gateway_order_id": input.order_no,
                        "payment_url": Value::Null,
                        "submit_method": "qrcode",
                        "qr_code": qr_code,
                        "pay_amount": input.pay_amount,
                        "pay_currency": input.pay_currency,
                        "payment_channel": input.payment_channel,
                        "callback_url": input.notify_url,
                        "return_url": return_url,
                        "expires_at": input.expires_at.to_rfc3339(),
                    })
                })
                .ok_or_else(|| "支付宝预下单响应缺少 qr_code".to_string())
        }) {
        Ok(value) => Ok(value),
        Err(precreate_err) => {
            let params = alipay_signed_params(
                &config,
                "alipay.trade.page.pay",
                json!({
                    "out_trade_no": input.order_no,
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
                "gateway_order_id": input.order_no,
                "payment_url": url_with_query(&gateway_url, &params)?,
                "submit_method": "GET",
                "qr_code": Value::Null,
                "pay_amount": input.pay_amount,
                "pay_currency": input.pay_currency,
                "payment_channel": input.payment_channel,
                "callback_url": input.notify_url,
                "return_url": return_url,
                "expires_at": input.expires_at.to_rfc3339(),
                "integration_status": format!("precreate_fallback: {precreate_err}"),
            }))
        }
    }
}

pub(crate) async fn verify_alipay_notify_callback(
    state: &AppState,
    body: &[u8],
) -> Result<aether_data::repository::wallet::ProcessPaymentCallbackInput, String> {
    let config = load_direct_gateway_config(state, "alipay").await?;
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
    if let Some(app_id) = params.get("app_id").map(String::as_str) {
        if app_id != alipay_app_id(&config)? {
            return Err("支付宝通知 app_id 不匹配".to_string());
        }
    }
    if !matches!(
        params.get("trade_status").map(String::as_str),
        Some("TRADE_SUCCESS" | "TRADE_FINISHED")
    ) {
        return Err("支付宝通知不是成功支付状态".to_string());
    }
    let order_no = params
        .get("out_trade_no")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "支付宝通知缺少 out_trade_no".to_string())?;
    let pay_amount = params
        .get("total_amount")
        .or_else(|| params.get("receipt_amount"))
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| *value > 0.0)
        .ok_or_else(|| "支付宝通知金额无效".to_string())?;
    let payload = serde_json::to_value(&params).unwrap_or_else(|_| json!({}));
    let payload_hash = payment_payload_hash(&payload)?;
    let callback_key = params
        .get("notify_id")
        .or_else(|| params.get("trade_no"))
        .cloned()
        .unwrap_or_else(|| format!("alipay:{order_no}:{payload_hash}"));
    let exchange_rate = config.record.usd_exchange_rate;
    Ok(
        aether_data::repository::wallet::ProcessPaymentCallbackInput {
            payment_method: "alipay".to_string(),
            payment_provider: Some("alipay".to_string()),
            payment_channel: Some("alipay".to_string()),
            callback_key,
            order_no: Some(order_no),
            gateway_order_id: params.get("trade_no").cloned(),
            amount_usd: if exchange_rate > 0.0 {
                pay_amount / exchange_rate
            } else {
                pay_amount
            },
            pay_amount: Some(pay_amount),
            pay_currency: Some(config.record.pay_currency),
            exchange_rate: Some(exchange_rate),
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
) -> Result<Value, String> {
    let body =
        serde_json::to_string(&body).map_err(|err| format!("微信支付请求体编码失败: {err}"))?;
    let auth = wxpay_authorization(config, "POST", canonical_url, &body)?;
    let url = format!("{}{}", wxpay_base_url(config), canonical_url);
    let response = state
        .client
        .post(url)
        .header(http::header::AUTHORIZATION, auth)
        .header(http::header::ACCEPT, "application/json")
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(body)
        .send()
        .await
        .map_err(|err| format!("微信支付请求失败: {err}"))?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|err| format!("微信支付响应读取失败: {err}"))?;
    if !status.is_success() {
        let detail = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|value| {
                value
                    .get("message")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
                    .or_else(|| {
                        value
                            .get("code")
                            .and_then(Value::as_str)
                            .map(ToOwned::to_owned)
                    })
            })
            .unwrap_or_else(|| format!("微信支付 HTTP 状态异常: {status}"));
        return Err(detail);
    }
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str::<Value>(&text).map_err(|_| "微信支付响应格式无效".to_string())
}

async fn wxpay_post_empty_success(
    state: &AppState,
    config: &DirectGatewayConfig,
    canonical_url: &str,
    body: Value,
) -> Result<Value, String> {
    wxpay_post_json(state, config, canonical_url, body).await
}

fn wxpay_client_ip(headers: &http::HeaderMap) -> Option<String> {
    crate::headers::header_value_str(headers, "x-forwarded-for")
        .and_then(|value| {
            value
                .split(',')
                .map(str::trim)
                .find(|segment| !segment.is_empty() && !segment.eq_ignore_ascii_case("unknown"))
                .map(ToOwned::to_owned)
        })
        .or_else(|| {
            crate::headers::header_value_str(headers, "x-real-ip").and_then(|value| {
                let value = value.trim();
                (!value.is_empty() && !value.eq_ignore_ascii_case("unknown"))
                    .then(|| value.to_string())
            })
        })
}

pub(crate) fn direct_payment_client_ip(headers: &http::HeaderMap) -> Option<String> {
    wxpay_client_ip(headers)
}

pub(crate) async fn create_wxpay_direct_checkout(
    state: &AppState,
    input: &DirectPaymentCheckoutInput,
) -> Result<Value, String> {
    let config = load_direct_gateway_config(state, "wxpay").await?;
    if !input.pay_currency.eq_ignore_ascii_case(WXPAY_CURRENCY) {
        return Err("微信支付官方直连当前仅支持 CNY".to_string());
    }
    let total_fen = wxpay_money_to_fen(input.pay_amount)?;
    let app_id = wxpay_config_string(&config, "app_id")?;
    let mch_id = wxpay_config_string(&config, "mch_id")?;
    let common = json!({
        "appid": app_id,
        "mchid": mch_id,
        "description": input.subject,
        "out_trade_no": input.order_no,
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
                .ok_or_else(|| "微信 Native 响应缺少 code_url".to_string())?;
            Ok(json!({
                "gateway": "wxpay",
                "display_name": input.display_name,
                "gateway_order_id": input.order_no,
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
                .ok_or_else(|| "微信 H5 响应缺少 h5_url".to_string())?
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
                "gateway_order_id": input.order_no,
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
        "jsapi" => Err("微信 JSAPI 需要前端提供 OpenID，当前充值入口尚未接入".to_string()),
        _ => Err("微信支付通道不可用".to_string()),
    }
}

pub(crate) async fn create_stripe_direct_checkout(
    state: &AppState,
    input: &DirectPaymentCheckoutInput,
) -> Result<Value, String> {
    let config = load_direct_gateway_config(state, "stripe").await?;
    let Some(secret_key) = secret_string(&config.secrets, "secret_key") else {
        return Err("Stripe secret_key 未配置".to_string());
    };
    let Some(publishable_key) = config_string(&config.config, "publishable_key") else {
        return Err("Stripe publishable_key 未配置".to_string());
    };
    let amount = stripe_minor_unit_amount(input.pay_amount, &config.record.pay_currency)?;
    let currency = config.record.pay_currency.trim().to_ascii_lowercase();
    let mut form = vec![
        ("amount".to_string(), amount.to_string()),
        ("currency".to_string(), currency.clone()),
        ("description".to_string(), input.subject.clone()),
        ("metadata[order_no]".to_string(), input.order_no.clone()),
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
    let associated_data = resource
        .get("associated_data")
        .and_then(Value::as_str)
        .unwrap_or("");
    let ciphertext = resource
        .get("ciphertext")
        .and_then(Value::as_str)
        .ok_or_else(|| "微信支付通知 resource.ciphertext 缺失".to_string())?;
    let ciphertext = BASE64_STANDARD
        .decode(ciphertext)
        .map_err(|err| format!("微信支付通知密文解码失败: {err}"))?;
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

pub(crate) async fn verify_wxpay_notify_callback(
    state: &AppState,
    headers: &http::HeaderMap,
    body: &[u8],
) -> Result<aether_data::repository::wallet::ProcessPaymentCallbackInput, String> {
    let config = load_direct_gateway_config(state, "wxpay").await?;
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
    let order_no = tx
        .get("out_trade_no")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| "微信支付通知缺少 out_trade_no".to_string())?;
    let transaction_id = tx
        .get("transaction_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let amount_fen = tx
        .get("amount")
        .and_then(|value| value.get("total"))
        .and_then(Value::as_i64)
        .filter(|value| *value > 0)
        .ok_or_else(|| "微信支付通知金额无效".to_string())?;
    let pay_amount = amount_fen as f64 / 100.0;
    let exchange_rate = config.record.usd_exchange_rate;
    let callback_key = payload
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| transaction_id.clone())
        .unwrap_or_else(|| format!("wxpay:{order_no}"));
    let callback_payload = json!({
        "notification": payload,
        "transaction": tx,
    });
    let payload_hash = payment_payload_hash(&callback_payload)?;
    Ok(
        aether_data::repository::wallet::ProcessPaymentCallbackInput {
            payment_method: "wxpay".to_string(),
            payment_provider: Some("wxpay".to_string()),
            payment_channel: None,
            callback_key,
            order_no: Some(order_no),
            gateway_order_id: transaction_id,
            amount_usd: if exchange_rate > 0.0 {
                pay_amount / exchange_rate
            } else {
                pay_amount
            },
            pay_amount: Some(pay_amount),
            pay_currency: Some(config.record.pay_currency),
            exchange_rate: Some(exchange_rate),
            payload_hash,
            payload: callback_payload,
            signature_valid: true,
        },
    )
}

pub(crate) async fn close_direct_gateway_order(
    state: &AppState,
    order: &crate::AdminWalletPaymentOrderRecord,
) -> Result<Option<Value>, String> {
    match order.payment_method.as_str() {
        "alipay" => {
            let config = load_direct_gateway_config(state, "alipay").await?;
            let params = alipay_signed_params(
                &config,
                "alipay.trade.close",
                json!({ "out_trade_no": order.order_no }),
                None,
                None,
            )?;
            let value = alipay_post(state, &config, &params).await?;
            let response = alipay_response_success(&value, "alipay_trade_close_response")?;
            Ok(Some(json!({
                "gateway": "alipay",
                "closed": true,
                "gateway_order_id": response.get("trade_no").and_then(Value::as_str),
                "payload": value,
            })))
        }
        "wxpay" => {
            let config = load_direct_gateway_config(state, "wxpay").await?;
            let canonical_url =
                format!("/v3/pay/transactions/out-trade-no/{}/close", order.order_no);
            let value = wxpay_post_empty_success(
                state,
                &config,
                &canonical_url,
                json!({ "mchid": wxpay_config_string(&config, "mch_id")? }),
            )
            .await?;
            Ok(Some(json!({
                "gateway": "wxpay",
                "closed": true,
                "gateway_order_id": order.gateway_order_id,
                "payload": value,
            })))
        }
        _ => Ok(None),
    }
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
    if order.amount_usd <= 0.0 || total_pay_amount <= 0.0 {
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
            let config = load_direct_gateway_config(state, "alipay").await?;
            let refund_amount = refund_pay_amount(order, amount_usd)?;
            let params = alipay_signed_params(
                &config,
                "alipay.trade.refund",
                json!({
                    "out_trade_no": order.order_no,
                    "refund_amount": format!("{refund_amount:.2}"),
                    "refund_reason": reason.unwrap_or("wallet refund"),
                    "out_request_no": refund_no,
                }),
                None,
                None,
            )?;
            let value = alipay_post(state, &config, &params).await?;
            let response = alipay_response_success(&value, "alipay_trade_refund_response")?;
            let gateway_refund_id = response
                .get("trade_no")
                .and_then(Value::as_str)
                .unwrap_or(&order.order_no)
                .to_string();
            Ok(Some(DirectGatewayRefundResult {
                gateway_refund_id,
                status: "success".to_string(),
                payload: value,
            }))
        }
        "wxpay" => {
            let config = load_direct_gateway_config(state, "wxpay").await?;
            let total_pay_amount = order.pay_amount.ok_or_else(|| {
                "微信支付退款需要原订单 pay_amount，请确认订单已通过官方直连创建".to_string()
            })?;
            let refund_amount = refund_pay_amount(order, amount_usd)?;
            let value = wxpay_post_json(
                state,
                &config,
                "/v3/refund/domestic/refunds",
                json!({
                    "out_trade_no": order.order_no,
                    "out_refund_no": refund_no,
                    "reason": reason.unwrap_or("wallet refund"),
                    "amount": {
                        "refund": wxpay_money_to_fen(refund_amount)?,
                        "total": wxpay_money_to_fen(total_pay_amount)?,
                        "currency": WXPAY_CURRENCY,
                    },
                }),
            )
            .await?;
            let gateway_refund_id = value
                .get("refund_id")
                .and_then(Value::as_str)
                .unwrap_or(refund_no)
                .to_string();
            let status = value
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("PROCESSING")
                .to_ascii_lowercase();
            Ok(Some(DirectGatewayRefundResult {
                gateway_refund_id,
                status,
                payload: value,
            }))
        }
        _ => Ok(None),
    }
}
