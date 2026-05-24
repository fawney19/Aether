use super::{
    build_admin_payments_backend_unavailable_response, build_admin_payments_bad_request_response,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::shared::{
    payment_gateway_allow_user_refund, payment_gateway_channels_config_json,
    payment_gateway_channels_json, payment_gateway_config_json, payment_gateway_refund_enabled,
    payment_gateway_secret_keys_json,
};
use crate::{GatewayError, LocalMutationOutcome};
use aether_data_contracts::repository::billing::PaymentGatewayConfigWriteInput;
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Deserialize)]
struct PaymentGatewayConfigRequest {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    endpoint_url: String,
    #[serde(default)]
    callback_base_url: Option<String>,
    #[serde(default)]
    merchant_id: String,
    #[serde(default)]
    merchant_key: Option<String>,
    #[serde(default = "default_pay_currency")]
    pay_currency: String,
    #[serde(default = "default_usd_exchange_rate")]
    usd_exchange_rate: f64,
    #[serde(default = "default_min_recharge_usd")]
    min_recharge_usd: f64,
    #[serde(default = "default_channels")]
    channels: Value,
    #[serde(default)]
    refund_enabled: bool,
    #[serde(default)]
    allow_user_refund: bool,
    #[serde(default)]
    config: Value,
    #[serde(default)]
    secrets: Value,
}

fn default_pay_currency() -> String {
    "CNY".to_string()
}

fn default_usd_exchange_rate() -> f64 {
    7.2
}

fn default_min_recharge_usd() -> f64 {
    1.0
}

fn default_channels() -> Value {
    json!([
        {"channel": "alipay", "display_name": "支付宝", "fee_rate": 0.0},
        {"channel": "wxpay", "display_name": "微信支付", "fee_rate": 0.0}
    ])
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

fn supported_payment_gateway_provider(provider: &str) -> bool {
    matches!(provider, "epay" | "alipay" | "wxpay" | "stripe")
}

fn admin_payment_gateway_provider_from_path(path: &str) -> Option<String> {
    let trimmed = path.trim_end_matches('/');
    let provider = trimmed
        .strip_prefix("/api/admin/payments/gateways/")?
        .strip_suffix("/test")
        .unwrap_or_else(|| {
            trimmed
                .strip_prefix("/api/admin/payments/gateways/")
                .unwrap_or("")
        })
        .trim()
        .to_ascii_lowercase();
    if provider.is_empty()
        || provider.contains('/')
        || !supported_payment_gateway_provider(&provider)
    {
        return None;
    }
    Some(provider)
}

fn default_provider_channels(provider: &str) -> Value {
    match provider {
        "epay" => default_channels(),
        "alipay" => json!([{"channel": "alipay", "display_name": "支付宝官方", "fee_rate": 0.0}]),
        "wxpay" => json!([
            {"channel": "native", "display_name": "微信 Native", "fee_rate": 0.0},
            {"channel": "h5", "display_name": "微信 H5", "fee_rate": 0.0}
        ]),
        "stripe" => json!([
            {"channel": "card", "display_name": "Card", "fee_rate": 0.0},
            {"channel": "alipay", "display_name": "Alipay", "fee_rate": 0.0},
            {"channel": "wechat_pay", "display_name": "WeChat Pay", "fee_rate": 0.0},
            {"channel": "link", "display_name": "Link", "fee_rate": 0.0}
        ]),
        _ => json!([]),
    }
}

fn split_gateway_channels_config(
    record: &aether_data_contracts::repository::billing::PaymentGatewayConfigRecord,
) -> (Value, Value, Value, bool, bool) {
    (
        payment_gateway_channels_json(&record.channels_json),
        payment_gateway_config_json(&record.channels_json),
        payment_gateway_secret_keys_json(&record.channels_json),
        payment_gateway_refund_enabled(&record.channels_json),
        payment_gateway_allow_user_refund(&record.channels_json),
    )
}

fn gateway_config_payload(
    record: aether_data_contracts::repository::billing::PaymentGatewayConfigRecord,
) -> Value {
    let (channels, config, secret_keys, refund_enabled, allow_user_refund) =
        split_gateway_channels_config(&record);
    json!({
        "provider": record.provider,
        "enabled": record.enabled,
        "endpoint_url": record.endpoint_url,
        "callback_base_url": record.callback_base_url,
        "merchant_id": record.merchant_id,
        "has_secret": record.merchant_key_encrypted.as_deref().is_some_and(|value| !value.trim().is_empty()),
        "has_secret_keys": secret_keys,
        "pay_currency": record.pay_currency,
        "usd_exchange_rate": record.usd_exchange_rate,
        "min_recharge_usd": record.min_recharge_usd,
        "channels": channels,
        "refund_enabled": refund_enabled,
        "allow_user_refund": allow_user_refund,
        "config": config,
        "created_at": record.created_at_unix_secs,
        "updated_at": record.updated_at_unix_secs,
    })
}

fn gateway_config_not_found_payload(provider: &str) -> Value {
    json!({
        "provider": provider,
        "enabled": false,
        "has_secret": false,
        "has_secret_keys": [],
        "channels": default_provider_channels(provider),
        "refund_enabled": false,
        "allow_user_refund": false,
        "config": {},
    })
}

fn normalize_gateway_channel_fee_rate(value: Option<&Value>, index: usize) -> Result<f64, String> {
    let Some(value) = value else {
        return Ok(0.0);
    };
    let fee_rate = match value {
        Value::Null => 0.0,
        Value::Number(number) => number
            .as_f64()
            .ok_or_else(|| format!("channels[{index}].fee_rate must be a number"))?,
        Value::String(value) => value
            .trim()
            .parse::<f64>()
            .map_err(|_| format!("channels[{index}].fee_rate must be a number"))?,
        _ => return Err(format!("channels[{index}].fee_rate must be a number")),
    };
    if !fee_rate.is_finite() || fee_rate < 0.0 {
        return Err(format!("channels[{index}].fee_rate must be non-negative"));
    }
    Ok(fee_rate)
}

fn normalize_gateway_channels(provider: &str, channels: Value) -> Result<Value, String> {
    if channels.is_null() {
        return Ok(default_provider_channels(provider));
    }
    let Some(items) = channels.as_array() else {
        return Err("channels must be an array".to_string());
    };
    if items.is_empty() {
        return Ok(default_provider_channels(provider));
    }

    let normalized = items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let Some(object) = item.as_object() else {
                return Err(format!("channels[{index}] must be an object"));
            };
            let channel = object
                .get("channel")
                .or_else(|| object.get("type"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| format!("channels[{index}].channel must not be empty"))?;
            let display_name = object
                .get("display_name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(channel);
            let fee_rate = normalize_gateway_channel_fee_rate(object.get("fee_rate"), index)?;
            Ok(json!({
                "channel": channel,
                "display_name": display_name,
                "fee_rate": fee_rate,
            }))
        })
        .collect::<Result<Vec<_>, String>>()?;

    Ok(Value::Array(normalized))
}

fn normalize_config_object(config: Value) -> Result<Value, String> {
    if config.is_null() {
        return Ok(json!({}));
    }
    if config.is_object() {
        return Ok(config);
    }
    Err("config must be an object".to_string())
}

fn encrypted_gateway_secret(
    state: &AdminAppState<'_>,
    provider: &str,
    payload: &PaymentGatewayConfigRequest,
) -> Result<Option<String>, Response<Body>> {
    let secret_plaintext = if provider == "epay" {
        payload
            .merchant_key
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    } else {
        let Some(secrets) = payload.secrets.as_object() else {
            return if payload.secrets.is_null() {
                Ok(None)
            } else {
                Err(build_admin_payments_bad_request_response(
                    "secrets must be an object",
                ))
            };
        };
        let filtered = secrets
            .iter()
            .filter_map(|(key, value)| {
                let value = value.as_str()?.trim();
                (!key.trim().is_empty() && !value.is_empty())
                    .then(|| (key.trim().to_string(), Value::String(value.to_string())))
            })
            .collect::<serde_json::Map<_, _>>();
        if filtered.is_empty() {
            None
        } else {
            Some(Value::Object(filtered).to_string())
        }
    };

    let Some(secret_plaintext) = secret_plaintext else {
        return Ok(None);
    };
    state
        .encrypt_catalog_secret_with_fallbacks(&secret_plaintext)
        .ok_or_else(|| {
            build_admin_payments_backend_unavailable_response("encryption key is not configured")
        })
        .map(Some)
}

async fn existing_gateway_secret_keys(
    state: &AdminAppState<'_>,
    provider: &str,
) -> Result<Vec<Value>, GatewayError> {
    let Some(record) = state.app().find_payment_gateway_config(provider).await? else {
        return Ok(Vec::new());
    };
    let (_, _, secret_keys, _, _) = split_gateway_channels_config(&record);
    Ok(secret_keys
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|value| value.as_str().is_some_and(|item| !item.trim().is_empty()))
        .collect())
}

pub(super) async fn maybe_build_local_admin_payment_gateways_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&axum::body::Bytes>,
    route_kind: Option<&str>,
) -> Result<Option<Response<Body>>, GatewayError> {
    match route_kind {
        Some("get_epay_gateway") | Some("get_payment_gateway") => {
            let provider = admin_payment_gateway_provider_from_path(request_context.path())
                .unwrap_or_else(|| "epay".to_string());
            let record = state.app().find_payment_gateway_config(&provider).await?;
            let payload = record
                .map(gateway_config_payload)
                .unwrap_or_else(|| gateway_config_not_found_payload(&provider));
            Ok(Some(Json(payload).into_response()))
        }
        Some("update_epay_gateway") | Some("update_payment_gateway") => {
            let provider = admin_payment_gateway_provider_from_path(request_context.path())
                .unwrap_or_else(|| "epay".to_string());
            let Some(body) = request_body else {
                return Ok(Some(build_admin_payments_bad_request_response(
                    "缺少请求体",
                )));
            };
            let payload = match serde_json::from_slice::<PaymentGatewayConfigRequest>(body) {
                Ok(value) => value,
                Err(_) => {
                    return Ok(Some(build_admin_payments_bad_request_response(
                        "输入验证失败",
                    )))
                }
            };
            if !payload.usd_exchange_rate.is_finite() || payload.usd_exchange_rate <= 0.0 {
                return Ok(Some(build_admin_payments_bad_request_response(
                    "usd_exchange_rate must be positive",
                )));
            }
            if !payload.min_recharge_usd.is_finite() || payload.min_recharge_usd <= 0.0 {
                return Ok(Some(build_admin_payments_bad_request_response(
                    "min_recharge_usd must be positive",
                )));
            }

            let merchant_key_encrypted = match encrypted_gateway_secret(state, &provider, &payload)
            {
                Ok(value) => value,
                Err(response) => return Ok(Some(response)),
            };
            let endpoint_url = if provider == "epay" {
                match normalize_text(payload.endpoint_url, "endpoint_url", 512) {
                    Ok(value) => value,
                    Err(detail) => {
                        return Ok(Some(build_admin_payments_bad_request_response(detail)))
                    }
                }
            } else {
                match normalize_optional_text(Some(payload.endpoint_url), 512) {
                    Ok(value) => value.unwrap_or_default(),
                    Err(detail) => {
                        return Ok(Some(build_admin_payments_bad_request_response(detail)))
                    }
                }
            };
            let callback_base_url = match normalize_optional_text(payload.callback_base_url, 512) {
                Ok(value) => value,
                Err(detail) => return Ok(Some(build_admin_payments_bad_request_response(detail))),
            };
            let merchant_id = if provider == "epay" {
                match normalize_text(payload.merchant_id, "merchant_id", 128) {
                    Ok(value) => value,
                    Err(detail) => {
                        return Ok(Some(build_admin_payments_bad_request_response(detail)))
                    }
                }
            } else {
                match normalize_optional_text(Some(payload.merchant_id), 128) {
                    Ok(value) => value.unwrap_or_default(),
                    Err(detail) => {
                        return Ok(Some(build_admin_payments_bad_request_response(detail)))
                    }
                }
            };
            let pay_currency = match normalize_text(payload.pay_currency, "pay_currency", 16) {
                Ok(value) => value,
                Err(detail) => return Ok(Some(build_admin_payments_bad_request_response(detail))),
            };
            let config = match normalize_config_object(payload.config) {
                Ok(value) => value,
                Err(detail) => return Ok(Some(build_admin_payments_bad_request_response(detail))),
            };
            let submitted_secret_keys = payload
                .secrets
                .as_object()
                .map(|secrets| {
                    secrets
                        .iter()
                        .filter(|(_, value)| {
                            value.as_str().is_some_and(|value| !value.trim().is_empty())
                        })
                        .map(|(key, _)| Value::String(key.clone()))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let secret_keys = if provider == "epay" || !submitted_secret_keys.is_empty() {
                submitted_secret_keys
            } else {
                existing_gateway_secret_keys(state, &provider).await?
            };
            let channels = match normalize_gateway_channels(&provider, payload.channels) {
                Ok(value) => value,
                Err(detail) => return Ok(Some(build_admin_payments_bad_request_response(detail))),
            };
            let refund_enabled = payload.refund_enabled;
            let allow_user_refund = refund_enabled && payload.allow_user_refund;
            let channels_json = payment_gateway_channels_config_json(
                channels,
                config,
                Value::Array(secret_keys),
                refund_enabled,
                allow_user_refund,
            );
            let input = PaymentGatewayConfigWriteInput {
                provider: provider.clone(),
                enabled: payload.enabled,
                endpoint_url,
                callback_base_url,
                merchant_id,
                preserve_existing_secret: merchant_key_encrypted.is_none(),
                merchant_key_encrypted,
                pay_currency,
                usd_exchange_rate: payload.usd_exchange_rate,
                min_recharge_usd: payload.min_recharge_usd,
                channels_json,
            };
            match state.app().upsert_payment_gateway_config(&input).await? {
                LocalMutationOutcome::Applied(record) => {
                    Ok(Some(Json(gateway_config_payload(record)).into_response()))
                }
                _ => Ok(Some(build_admin_payments_backend_unavailable_response(
                    "payment gateway config backend unavailable",
                ))),
            }
        }
        Some("test_epay_gateway") | Some("test_payment_gateway") => {
            let provider = admin_payment_gateway_provider_from_path(request_context.path())
                .unwrap_or_else(|| "epay".to_string());
            let status = state.app().find_payment_gateway_config(&provider).await?;
            let ok = status
                .as_ref()
                .is_some_and(|record| record.enabled && record.merchant_key_encrypted.is_some());
            Ok(Some(
                (
                    if ok {
                        http::StatusCode::OK
                    } else {
                        http::StatusCode::BAD_REQUEST
                    },
                    Json(json!({"ok": ok, "provider": provider})),
                )
                    .into_response(),
            ))
        }
        _ => Ok(None),
    }
}
