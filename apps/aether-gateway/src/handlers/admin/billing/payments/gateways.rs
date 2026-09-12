use super::{
    build_admin_payments_backend_unavailable_response, build_admin_payments_bad_request_response,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::shared::{
    normalize_payment_callback_base_url, normalize_payment_currency, normalize_payment_https_url,
    payment_gateway_allow_user_refund, payment_gateway_channels_config_json,
    payment_gateway_channels_json, payment_gateway_config_json, payment_gateway_refund_enabled,
    payment_gateway_secret_is_legacy_unbound, payment_gateway_secret_keys_json,
    PaymentGatewaySecretBinding,
};
use crate::{GatewayError, LocalMutationOutcome};
use aether_data_contracts::repository::billing::{
    PaymentGatewayConfigCasWriteInput, PaymentGatewayConfigWriteInput,
};
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};

const PAYMENT_GATEWAY_CONFIG_CAS_MAX_ATTEMPTS: usize = 8;

#[derive(Deserialize)]
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

fn build_payment_gateway_conflict_response(detail: impl Into<String>) -> Response<Body> {
    (
        http::StatusCode::CONFLICT,
        Json(json!({ "detail": detail.into() })),
    )
        .into_response()
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

fn resolve_admin_payment_gateway_provider(path: &str, route_kind: &str) -> Option<String> {
    match route_kind {
        "get_epay_gateway" | "update_epay_gateway" | "test_epay_gateway" => {
            Some("epay".to_string())
        }
        "get_payment_gateway" | "update_payment_gateway" | "test_payment_gateway" => {
            admin_payment_gateway_provider_from_path(path)
        }
        _ => None,
    }
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

fn merge_gateway_secret_maps(
    existing_plaintext: Option<&str>,
    updates: serde_json::Map<String, Value>,
) -> Result<serde_json::Map<String, Value>, &'static str> {
    let mut merged = match existing_plaintext {
        Some(plaintext) => serde_json::from_str::<Value>(plaintext)
            .ok()
            .and_then(|value| value.as_object().cloned())
            .ok_or("existing gateway secrets have invalid format")?,
        None => serde_json::Map::new(),
    };
    merged.extend(updates);
    Ok(merged)
}

/// A legacy gateway ciphertext has no authenticated destination (or only the
/// provider in v2).  Reusing it while changing endpoint/merchant would carry
/// an unknown credential into a different payment account.  Require the
/// administrator to provide a replacement secret in that case.
fn legacy_secret_reuse_requires_reentry(
    existing: Option<&aether_data_contracts::repository::billing::PaymentGatewayConfigRecord>,
    requested_binding: &PaymentGatewaySecretBinding,
) -> bool {
    let Some(record) = existing else {
        return false;
    };
    let Some(ciphertext) = record.merchant_key_encrypted.as_deref() else {
        return false;
    };
    if !payment_gateway_secret_is_legacy_unbound(ciphertext) {
        return false;
    }

    // An invalid historical binding cannot establish that the legacy value
    // belongs to the requested destination, so fail closed as well.
    PaymentGatewaySecretBinding::from_record(record)
        .map(|stored_binding| stored_binding != requested_binding.clone())
        .unwrap_or(true)
}

fn encrypted_gateway_secret(
    state: &AdminAppState<'_>,
    binding: &PaymentGatewaySecretBinding,
    payload: &PaymentGatewayConfigRequest,
    existing: Option<&aether_data_contracts::repository::billing::PaymentGatewayConfigRecord>,
) -> Result<(Option<String>, Vec<Value>), Response<Body>> {
    let provider = binding.provider.as_str();
    let decrypt_existing = || {
        if legacy_secret_reuse_requires_reentry(existing, binding) {
            return Err(build_admin_payments_bad_request_response(
                "endpoint_url or merchant_id changed; re-enter the gateway secret",
            ));
        }
        existing
            .and_then(|record| record.merchant_key_encrypted.as_deref())
            .map(|ciphertext| {
                crate::handlers::shared::open_payment_gateway_secret(
                    state.app(), binding, ciphertext,
                )
                .map(|projection| projection.plaintext)
                .map_err(|_| {
                    build_admin_payments_backend_unavailable_response(
                        "existing gateway secrets are not valid for the requested destination; re-enter the secret",
                    )
                })
            })
            .transpose()
    };
    let secret_plaintext = if provider == "epay" {
        let supplied = payload
            .merchant_key
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
        if supplied.is_none() {
            decrypt_existing()?;
        }
        supplied
    } else {
        let Some(secrets) = payload.secrets.as_object() else {
            return if payload.secrets.is_null() {
                decrypt_existing()?;
                Ok((None, existing_gateway_secret_keys(existing)))
            } else {
                Err(build_admin_payments_bad_request_response(
                    "secrets must be an object",
                ))
            };
        };
        let updates = secrets
            .iter()
            .filter_map(|(key, value)| {
                let value = value.as_str()?.trim();
                (!key.trim().is_empty() && !value.is_empty())
                    .then(|| (key.trim().to_string(), Value::String(value.to_string())))
            })
            .collect::<serde_json::Map<_, _>>();
        if updates.is_empty() {
            decrypt_existing()?;
            return Ok((None, existing_gateway_secret_keys(existing)));
        }

        let existing_plaintext = decrypt_existing()?;
        let merged = match merge_gateway_secret_maps(existing_plaintext.as_deref(), updates) {
            Ok(value) => value,
            Err(detail) => {
                return Err(build_admin_payments_backend_unavailable_response(detail));
            }
        };
        Some(Value::Object(merged).to_string())
    };

    let Some(secret_plaintext) = secret_plaintext else {
        return Ok((None, existing_gateway_secret_keys(existing)));
    };
    let encrypted = crate::handlers::shared::seal_payment_gateway_secret(
        state.app(),
        binding,
        &secret_plaintext,
    )
    .map_err(build_admin_payments_backend_unavailable_response)?;
    let secret_keys = if provider == "epay" {
        Vec::new()
    } else {
        let mut keys = serde_json::from_str::<Value>(&secret_plaintext)
            .ok()
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default()
            .into_iter()
            .map(|(key, _)| Value::String(key))
            .collect::<Vec<_>>();
        keys.sort_by(|left, right| left.as_str().cmp(&right.as_str()));
        keys
    };
    Ok((Some(encrypted), secret_keys))
}

fn existing_gateway_secret_keys(
    record: Option<&aether_data_contracts::repository::billing::PaymentGatewayConfigRecord>,
) -> Vec<Value> {
    let Some(record) = record else {
        return Vec::new();
    };
    let (_, _, secret_keys, _, _) = split_gateway_channels_config(record);
    secret_keys
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|value| value.as_str().is_some_and(|item| !item.trim().is_empty()))
        .collect()
}

pub(super) async fn maybe_build_local_admin_payment_gateways_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&axum::body::Bytes>,
    route_kind: Option<&str>,
) -> Result<Option<Response<Body>>, GatewayError> {
    match route_kind {
        Some("get_epay_gateway") | Some("get_payment_gateway") => {
            let Some(provider) = resolve_admin_payment_gateway_provider(
                request_context.path(),
                route_kind.expect("matched payment gateway route kind"),
            ) else {
                return Ok(Some(build_admin_payments_bad_request_response(
                    "unsupported payment gateway provider",
                )));
            };
            let record = state.app().find_payment_gateway_config(&provider).await?;
            let payload = record
                .map(gateway_config_payload)
                .unwrap_or_else(|| gateway_config_not_found_payload(&provider));
            Ok(Some(Json(payload).into_response()))
        }
        Some("update_epay_gateway") | Some("update_payment_gateway") => {
            let Some(provider) = resolve_admin_payment_gateway_provider(
                request_context.path(),
                route_kind.expect("matched payment gateway route kind"),
            ) else {
                return Ok(Some(build_admin_payments_bad_request_response(
                    "unsupported payment gateway provider",
                )));
            };
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

            let endpoint_url = if provider == "epay" {
                match normalize_text(payload.endpoint_url.clone(), "endpoint_url", 512) {
                    Ok(value) => match normalize_payment_https_url(&value, "endpoint_url") {
                        Ok(value) => value,
                        Err(detail) => {
                            return Ok(Some(build_admin_payments_bad_request_response(detail)))
                        }
                    },
                    Err(detail) => {
                        return Ok(Some(build_admin_payments_bad_request_response(detail)))
                    }
                }
            } else {
                match normalize_optional_text(Some(payload.endpoint_url.clone()), 512) {
                    Ok(Some(value)) => match normalize_payment_https_url(&value, "endpoint_url") {
                        Ok(value) => value,
                        Err(detail) => {
                            return Ok(Some(build_admin_payments_bad_request_response(detail)))
                        }
                    },
                    Ok(None) => String::new(),
                    Err(detail) => {
                        return Ok(Some(build_admin_payments_bad_request_response(detail)))
                    }
                }
            };
            let callback_base_url =
                match normalize_optional_text(payload.callback_base_url.clone(), 512) {
                    Ok(Some(value)) => match normalize_payment_callback_base_url(&value) {
                        Ok(value) => Some(value),
                        Err(detail) => {
                            return Ok(Some(build_admin_payments_bad_request_response(detail)))
                        }
                    },
                    Ok(None) => None,
                    Err(detail) => {
                        return Ok(Some(build_admin_payments_bad_request_response(detail)))
                    }
                };
            let merchant_id = if provider == "epay" {
                match normalize_text(payload.merchant_id.clone(), "merchant_id", 128) {
                    Ok(value) => value,
                    Err(detail) => {
                        return Ok(Some(build_admin_payments_bad_request_response(detail)))
                    }
                }
            } else {
                match normalize_optional_text(Some(payload.merchant_id.clone()), 128) {
                    Ok(value) => value.unwrap_or_default(),
                    Err(detail) => {
                        return Ok(Some(build_admin_payments_bad_request_response(detail)))
                    }
                }
            };
            let pay_currency =
                match normalize_payment_currency(&payload.pay_currency, "pay_currency") {
                    Ok(value) => value,
                    Err(detail) => {
                        return Ok(Some(build_admin_payments_bad_request_response(detail)))
                    }
                };
            let config = match normalize_config_object(payload.config.clone()) {
                Ok(value) => value,
                Err(detail) => return Ok(Some(build_admin_payments_bad_request_response(detail))),
            };
            let channels = match normalize_gateway_channels(&provider, payload.channels.clone()) {
                Ok(value) => value,
                Err(detail) => return Ok(Some(build_admin_payments_bad_request_response(detail))),
            };
            let refund_enabled = payload.refund_enabled;
            let allow_user_refund = refund_enabled && payload.allow_user_refund;
            let binding =
                match PaymentGatewaySecretBinding::new(&provider, &endpoint_url, &merchant_id) {
                    Ok(value) => value,
                    Err(detail) => {
                        return Ok(Some(build_admin_payments_bad_request_response(detail)))
                    }
                };
            let mut existing_record = state.app().find_payment_gateway_config(&provider).await?;
            let expected_existing = existing_record.is_some();
            for _ in 0..PAYMENT_GATEWAY_CONFIG_CAS_MAX_ATTEMPTS {
                if expected_existing && existing_record.is_none() {
                    return Ok(Some(build_payment_gateway_conflict_response(
                        "payment gateway config was removed concurrently",
                    )));
                }
                let (merchant_key_encrypted, secret_keys) = match encrypted_gateway_secret(
                    state,
                    &binding,
                    &payload,
                    existing_record.as_ref(),
                ) {
                    Ok(value) => value,
                    Err(response) => return Ok(Some(response)),
                };
                let channels_json = payment_gateway_channels_config_json(
                    channels.clone(),
                    config.clone(),
                    Value::Array(secret_keys),
                    refund_enabled,
                    allow_user_refund,
                );
                let mutation = PaymentGatewayConfigCasWriteInput {
                    input: PaymentGatewayConfigWriteInput {
                        provider: provider.clone(),
                        enabled: payload.enabled,
                        endpoint_url: endpoint_url.clone(),
                        callback_base_url: callback_base_url.clone(),
                        merchant_id: merchant_id.clone(),
                        preserve_existing_secret: merchant_key_encrypted.is_none(),
                        merchant_key_encrypted,
                        pay_currency: pay_currency.clone(),
                        usd_exchange_rate: payload.usd_exchange_rate,
                        min_recharge_usd: payload.min_recharge_usd,
                        channels_json,
                    },
                    expected_existing,
                    expected_merchant_key_encrypted: existing_record
                        .as_ref()
                        .and_then(|record| record.merchant_key_encrypted.clone()),
                };
                match state
                    .app()
                    .compare_and_swap_payment_gateway_config(&mutation)
                    .await?
                {
                    LocalMutationOutcome::Applied(record) => {
                        return Ok(Some(Json(gateway_config_payload(record)).into_response()));
                    }
                    LocalMutationOutcome::NotFound if !expected_existing => {
                        return Ok(Some(build_payment_gateway_conflict_response(
                            "payment gateway config was created concurrently",
                        )));
                    }
                    LocalMutationOutcome::NotFound => {
                        existing_record =
                            state.app().find_payment_gateway_config(&provider).await?;
                    }
                    LocalMutationOutcome::Invalid(detail) => {
                        return Ok(Some(build_admin_payments_bad_request_response(detail)));
                    }
                    LocalMutationOutcome::Unavailable => {
                        return Ok(Some(build_admin_payments_backend_unavailable_response(
                            "payment gateway config backend unavailable",
                        )));
                    }
                }
            }
            Ok(Some(build_payment_gateway_conflict_response(
                "payment gateway config changed too frequently; retry the request",
            )))
        }
        Some("test_epay_gateway") | Some("test_payment_gateway") => {
            let Some(provider) = resolve_admin_payment_gateway_provider(
                request_context.path(),
                route_kind.expect("matched payment gateway route kind"),
            ) else {
                return Ok(Some(build_admin_payments_bad_request_response(
                    "unsupported payment gateway provider",
                )));
            };
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

#[cfg(test)]
mod tests {
    use aether_crypto::{encrypt_python_fernet_plaintext, DEVELOPMENT_ENCRYPTION_KEY};
    use aether_data_contracts::repository::billing::PaymentGatewayConfigRecord;
    use serde_json::{json, Value};

    use super::{
        legacy_secret_reuse_requires_reentry, merge_gateway_secret_maps,
        resolve_admin_payment_gateway_provider,
    };
    use crate::handlers::shared::PaymentGatewaySecretBinding;

    fn gateway_record(
        endpoint_url: &str,
        merchant_id: &str,
        merchant_key_encrypted: Option<String>,
    ) -> PaymentGatewayConfigRecord {
        PaymentGatewayConfigRecord {
            provider: "stripe".to_string(),
            enabled: true,
            endpoint_url: endpoint_url.to_string(),
            callback_base_url: None,
            merchant_id: merchant_id.to_string(),
            merchant_key_encrypted,
            pay_currency: "USD".to_string(),
            usd_exchange_rate: 1.0,
            min_recharge_usd: 1.0,
            channels_json: json!({}),
            created_at_unix_secs: 1,
            updated_at_unix_secs: 1,
        }
    }

    #[test]
    fn legacy_secret_reuse_requires_reentry_after_binding_change() {
        let legacy = encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "legacy-secret")
            .expect("legacy secret should encrypt");
        let old_record = gateway_record("https://api.stripe.com", "merchant-old", Some(legacy));
        let changed_binding =
            PaymentGatewaySecretBinding::new("stripe", "https://api.stripe.com", "merchant-new")
                .expect("changed binding should be valid");
        assert!(legacy_secret_reuse_requires_reentry(
            Some(&old_record),
            &changed_binding,
        ));

        let v2_record = gateway_record(
            "https://api.stripe.com",
            "merchant-old",
            Some("aether-payment-gateway-secret-v2:legacy".to_string()),
        );
        assert!(legacy_secret_reuse_requires_reentry(
            Some(&v2_record),
            &changed_binding,
        ));
    }

    #[test]
    fn legacy_secret_reuse_is_allowed_only_for_same_binding_or_bound_v3() {
        let legacy = encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "legacy-secret")
            .expect("legacy secret should encrypt");
        let old_record =
            gateway_record("https://API.STRIPE.COM:443/", "merchant-old", Some(legacy));
        let same_binding = PaymentGatewaySecretBinding::new(
            "stripe",
            "https://api.stripe.com:443/",
            " merchant-old ",
        )
        .expect("same binding should be valid");
        assert!(!legacy_secret_reuse_requires_reentry(
            Some(&old_record),
            &same_binding,
        ));

        let v3_record = gateway_record(
            "https://api.stripe.com",
            "merchant-old",
            Some("aether-payment-gateway-secret-v3:bound".to_string()),
        );
        let changed_binding =
            PaymentGatewaySecretBinding::new("stripe", "https://api.stripe.com", "merchant-new")
                .expect("changed binding should be valid");
        assert!(!legacy_secret_reuse_requires_reentry(
            Some(&v3_record),
            &changed_binding,
        ));
    }

    #[test]
    fn stripe_secret_rotation_preserves_omitted_secret_fields() {
        let existing = json!({
            "secret_key": "old-secret-key",
            "webhook_secret": "old-webhook"
        })
        .to_string();
        let updates = json!({"webhook_secret": "new-webhook"})
            .as_object()
            .cloned()
            .expect("updates should be an object");

        let merged = Value::Object(
            merge_gateway_secret_maps(Some(&existing), updates)
                .expect("valid secret maps should merge"),
        );
        assert_eq!(merged["secret_key"], "old-secret-key");
        assert_eq!(merged["webhook_secret"], "new-webhook");
    }

    #[test]
    fn wxpay_secret_rotation_preserves_omitted_secret_fields() {
        let existing = json!({
            "private_key": "old-private",
            "api_v3_key": "old-api-v3-key",
            "public_key": "old-public"
        })
        .to_string();
        let updates = json!({"api_v3_key": "new-api-v3-key"})
            .as_object()
            .cloned()
            .expect("updates should be an object");

        let merged = Value::Object(
            merge_gateway_secret_maps(Some(&existing), updates)
                .expect("valid secret maps should merge"),
        );
        assert_eq!(merged["private_key"], "old-private");
        assert_eq!(merged["api_v3_key"], "new-api-v3-key");
        assert_eq!(merged["public_key"], "old-public");
    }

    #[test]
    fn generic_gateway_routes_never_fall_back_to_epay() {
        assert_eq!(
            resolve_admin_payment_gateway_provider(
                "/api/admin/payments/gateways/stripe",
                "update_payment_gateway",
            )
            .as_deref(),
            Some("stripe")
        );
        assert!(resolve_admin_payment_gateway_provider(
            "/api/admin/payments/gateways/unsupported",
            "update_payment_gateway",
        )
        .is_none());
        assert!(resolve_admin_payment_gateway_provider(
            "/api/admin/payments/gateways/stripe/extra",
            "get_payment_gateway",
        )
        .is_none());
        assert_eq!(
            resolve_admin_payment_gateway_provider(
                "/api/admin/payments/epay",
                "update_epay_gateway",
            )
            .as_deref(),
            Some("epay")
        );
    }
}
