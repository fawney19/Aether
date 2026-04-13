use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey,
};
use chrono::{TimeZone, Utc};
use serde_json::{json, Value};
use std::collections::BTreeMap;

fn unix_secs_to_rfc3339(unix_secs: u64) -> Option<String> {
    Utc.timestamp_opt(unix_secs as i64, 0)
        .single()
        .map(|value| value.to_rfc3339())
}

pub fn key_api_formats_without_entry(
    key: &StoredProviderCatalogKey,
    api_format: &str,
) -> Option<Vec<String>> {
    let current_formats = key
        .api_formats
        .as_ref()
        .and_then(serde_json::Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if !current_formats
        .iter()
        .any(|candidate| candidate == api_format)
    {
        return None;
    }
    Some(
        current_formats
            .into_iter()
            .filter(|candidate| candidate != api_format)
            .collect(),
    )
}

pub fn endpoint_key_counts_by_format(
    keys: &[StoredProviderCatalogKey],
) -> (BTreeMap<String, usize>, BTreeMap<String, usize>) {
    let mut total = BTreeMap::new();
    let mut active = BTreeMap::new();
    for key in keys {
        let Some(formats) = key
            .api_formats
            .as_ref()
            .and_then(serde_json::Value::as_array)
        else {
            continue;
        };
        for api_format in formats.iter().filter_map(serde_json::Value::as_str) {
            *total.entry(api_format.to_string()).or_insert(0) += 1;
            if key.is_active {
                *active.entry(api_format.to_string()).or_insert(0) += 1;
            }
        }
    }
    (total, active)
}

fn masked_proxy_value(proxy: Option<&serde_json::Value>) -> serde_json::Value {
    let Some(proxy) = proxy.and_then(serde_json::Value::as_object) else {
        return serde_json::Value::Null;
    };
    let mut masked = proxy.clone();
    if masked
        .get("password")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
    {
        masked.insert("password".to_string(), json!("***"));
    }
    serde_json::Value::Object(masked)
}

fn endpoint_timestamp_or_now(value: Option<u64>, now_unix_secs: u64) -> serde_json::Value {
    unix_secs_to_rfc3339(value.unwrap_or(now_unix_secs))
        .map(serde_json::Value::String)
        .unwrap_or(serde_json::Value::Null)
}

pub fn build_admin_provider_endpoint_response(
    endpoint: &StoredProviderCatalogEndpoint,
    provider_name: &str,
    total_keys: usize,
    active_keys: usize,
    now_unix_secs: u64,
) -> serde_json::Value {
    json!({
        "id": endpoint.id,
        "provider_id": endpoint.provider_id,
        "provider_name": provider_name,
        "api_format": endpoint.api_format,
        "base_url": endpoint.base_url,
        "custom_path": endpoint.custom_path,
        "header_rules": endpoint.header_rules,
        "body_rules": endpoint.body_rules,
        "max_retries": endpoint.max_retries.unwrap_or(2),
        "is_active": endpoint.is_active,
        "config": endpoint.config,
        "proxy": masked_proxy_value(endpoint.proxy.as_ref()),
        "format_acceptance_config": endpoint.format_acceptance_config,
        "total_keys": total_keys,
        "active_keys": active_keys,
        "created_at": endpoint_timestamp_or_now(endpoint.created_at_unix_ms, now_unix_secs),
        "updated_at": endpoint_timestamp_or_now(endpoint.updated_at_unix_secs, now_unix_secs),
    })
}

#[derive(Debug, Clone, Default)]
pub struct AdminProviderEndpointUpdateFields {
    pub base_url: Option<String>,
    pub custom_path: Option<String>,
    pub header_rules: Option<Value>,
    pub body_rules: Option<Value>,
    pub max_retries: Option<i32>,
    pub is_active: Option<bool>,
    pub config: Option<Value>,
    pub proxy: Option<Value>,
    pub format_acceptance_config: Option<Value>,
}

fn trimmed_non_empty_string(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim().to_string();
        (!trimmed.is_empty()).then_some(trimmed)
    })
}

#[allow(clippy::too_many_arguments)]
pub fn build_admin_provider_endpoint_record(
    id: String,
    provider_id: String,
    normalized_api_format: String,
    api_family: String,
    endpoint_kind: String,
    base_url: String,
    custom_path: Option<String>,
    header_rules: Option<Value>,
    body_rules: Option<Value>,
    max_retries: i32,
    config: Option<Value>,
    proxy: Option<Value>,
    format_acceptance_config: Option<Value>,
    now_unix_secs: u64,
) -> Result<StoredProviderCatalogEndpoint, String> {
    StoredProviderCatalogEndpoint::new(
        id,
        provider_id,
        normalized_api_format,
        Some(api_family),
        Some(endpoint_kind),
        true,
    )
    .map_err(|err| err.to_string())?
    .with_timestamps(Some(now_unix_secs), Some(now_unix_secs))
    .with_transport_fields(
        base_url,
        header_rules,
        body_rules,
        Some(max_retries),
        trimmed_non_empty_string(custom_path),
        config,
        format_acceptance_config,
        proxy,
    )
    .map_err(|err| err.to_string())
}

pub fn apply_admin_provider_endpoint_update_fields<FC, FN>(
    existing_endpoint: &StoredProviderCatalogEndpoint,
    contains_field: FC,
    is_null_field: FN,
    payload: &AdminProviderEndpointUpdateFields,
) -> Result<StoredProviderCatalogEndpoint, String>
where
    FC: Fn(&str) -> bool,
    FN: Fn(&str) -> bool,
{
    let mut updated = existing_endpoint.clone();

    if contains_field("base_url") {
        let Some(base_url) = payload.base_url.as_deref() else {
            return Err(if is_null_field("base_url") {
                "base_url 不能为空".to_string()
            } else {
                "base_url 必须是字符串".to_string()
            });
        };
        updated.base_url = base_url.to_string();
    }

    if contains_field("custom_path") {
        updated.custom_path = payload.custom_path.clone();
    }

    if contains_field("header_rules") {
        updated.header_rules = if is_null_field("header_rules") {
            None
        } else {
            let Some(header_rules) = payload.header_rules.as_ref() else {
                return Err("header_rules 必须是数组或 null".to_string());
            };
            if !header_rules.is_array() {
                return Err("header_rules 必须是数组或 null".to_string());
            }
            Some(header_rules.clone())
        };
    }

    if contains_field("body_rules") {
        updated.body_rules = if is_null_field("body_rules") {
            None
        } else {
            let Some(body_rules) = payload.body_rules.as_ref() else {
                return Err("body_rules 必须是数组或 null".to_string());
            };
            if !body_rules.is_array() {
                return Err("body_rules 必须是数组或 null".to_string());
            }
            Some(body_rules.clone())
        };
    }

    if contains_field("max_retries") {
        let Some(max_retries) = payload.max_retries else {
            return Err(if is_null_field("max_retries") {
                "max_retries 必须是 0 到 999 之间的整数".to_string()
            } else {
                "max_retries 必须是整数".to_string()
            });
        };
        if !(0..=999).contains(&max_retries) {
            return Err("max_retries 必须在 0 到 999 之间".to_string());
        }
        updated.max_retries = Some(max_retries);
    }

    if contains_field("is_active") {
        let Some(is_active) = payload.is_active else {
            return Err("is_active 必须是布尔值".to_string());
        };
        updated.is_active = is_active;
    }

    if contains_field("config") {
        updated.config = if is_null_field("config") {
            None
        } else {
            let Some(config) = payload.config.as_ref() else {
                return Err("config 必须是对象或 null".to_string());
            };
            if !config.is_object() {
                return Err("config 必须是对象或 null".to_string());
            }
            Some(config.clone())
        };
    }

    if contains_field("proxy") {
        if is_null_field("proxy") {
            updated.proxy = None;
        } else {
            let Some(mut proxy) = payload
                .proxy
                .clone()
                .and_then(|value| value.as_object().cloned())
            else {
                return Err("proxy 必须是对象或 null".to_string());
            };
            if !proxy.contains_key("password") {
                if let Some(old_password) = existing_endpoint
                    .proxy
                    .as_ref()
                    .and_then(Value::as_object)
                    .and_then(|proxy| proxy.get("password"))
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                {
                    proxy.insert("password".to_string(), json!(old_password));
                }
            }
            updated.proxy = Some(Value::Object(proxy));
        }
    }

    if contains_field("format_acceptance_config") {
        updated.format_acceptance_config = if is_null_field("format_acceptance_config") {
            None
        } else {
            let Some(config) = payload.format_acceptance_config.as_ref() else {
                return Err("format_acceptance_config 必须是对象或 null".to_string());
            };
            if !config.is_object() {
                return Err("format_acceptance_config 必须是对象或 null".to_string());
            }
            Some(config.clone())
        };
    }

    Ok(updated)
}
