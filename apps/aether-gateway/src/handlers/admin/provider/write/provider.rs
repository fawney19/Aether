use super::normalize::normalize_provider_type_input;
use crate::api::ai::{admin_default_body_rules_for_signature, admin_endpoint_signature_parts};
use crate::handlers::admin::provider::shared::payloads::{
    AdminProviderCreateRequest, AdminProviderUpdateRequest,
};
use crate::handlers::admin::provider::shared::support::{
    normalize_provider_billing_type, parse_optional_rfc3339_unix_secs,
};
use crate::handlers::admin::shared::normalize_json_object;
use crate::handlers::public::normalize_admin_base_url;
use crate::provider_transport::provider_types::provider_type_enables_format_conversion_by_default;
use crate::AppState;
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogProvider,
};
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

pub(crate) async fn build_admin_update_provider_record(
    state: &AppState,
    existing: &StoredProviderCatalogProvider,
    raw_payload: &serde_json::Map<String, serde_json::Value>,
    payload: AdminProviderUpdateRequest,
) -> Result<StoredProviderCatalogProvider, String> {
    let mut updated = existing.clone();

    if let Some(value) = raw_payload.get("name") {
        let Some(name) = payload.name.as_deref() else {
            return Err(if value.is_null() {
                "name 不能为空".to_string()
            } else {
                "name 必须是字符串".to_string()
            });
        };
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err("name 不能为空".to_string());
        }
        let duplicate = state
            .list_provider_catalog_providers(false)
            .await
            .map_err(|err| format!("{err:?}"))?
            .into_iter()
            .any(|provider| provider.id != existing.id && provider.name == trimmed);
        if duplicate {
            return Err(format!("提供商名称 '{trimmed}' 已存在"));
        }
        updated.name = trimmed.to_string();
    }

    let target_provider_type = if let Some(value) = raw_payload.get("provider_type") {
        let Some(provider_type) = payload.provider_type.as_deref() else {
            return Err(if value.is_null() {
                "provider_type 不能为空".to_string()
            } else {
                "provider_type 必须是字符串".to_string()
            });
        };
        let normalized = normalize_provider_type_input(provider_type)?;
        updated.provider_type = normalized.clone();
        normalized
    } else {
        updated.provider_type.clone()
    };

    if raw_payload.contains_key("description") {
        updated.description = payload
            .description
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
    }

    if let Some(value) = raw_payload.get("website") {
        updated.website = match payload.website {
            None => {
                if value.is_null() {
                    None
                } else {
                    return Err("website 必须是字符串".to_string());
                }
            }
            Some(website) => {
                let trimmed = website.trim();
                if trimmed.is_empty() {
                    None
                } else if !trimmed.starts_with("http://") && !trimmed.starts_with("https://") {
                    return Err("website 必须以 http:// 或 https:// 开头".to_string());
                } else {
                    Some(trimmed.to_string())
                }
            }
        };
    }

    if let Some(value) = raw_payload.get("billing_type") {
        let Some(billing_type) = payload.billing_type.as_deref() else {
            return Err(if value.is_null() {
                "billing_type 不能为空".to_string()
            } else {
                "billing_type 必须是字符串".to_string()
            });
        };
        updated.billing_type = Some(normalize_provider_billing_type(billing_type)?);
    }

    if let Some(value) = raw_payload.get("monthly_quota_usd") {
        if value.is_null() {
            updated.monthly_quota_usd = None;
        } else {
            let Some(monthly_quota_usd) = payload.monthly_quota_usd else {
                return Err("monthly_quota_usd 必须是非负数".to_string());
            };
            if !monthly_quota_usd.is_finite() || monthly_quota_usd < 0.0 {
                return Err("monthly_quota_usd 必须是非负数".to_string());
            }
            updated.monthly_quota_usd = Some(monthly_quota_usd);
        }
    }

    if let Some(value) = raw_payload.get("quota_reset_day") {
        if value.is_null() {
            updated.quota_reset_day = None;
        } else {
            let Some(quota_reset_day) = payload.quota_reset_day else {
                return Err("quota_reset_day 必须是 1 到 365 之间的整数".to_string());
            };
            if !(1..=365).contains(&quota_reset_day) {
                return Err("quota_reset_day 必须是 1 到 365 之间的整数".to_string());
            }
            updated.quota_reset_day = Some(quota_reset_day);
        }
    }

    if let Some(value) = raw_payload.get("quota_last_reset_at") {
        if value.is_null() {
            updated.quota_last_reset_at_unix_secs = None;
        } else {
            let Some(raw) = payload.quota_last_reset_at.as_deref() else {
                return Err("quota_last_reset_at 必须是字符串".to_string());
            };
            updated.quota_last_reset_at_unix_secs = Some(parse_optional_rfc3339_unix_secs(
                raw,
                "quota_last_reset_at",
            )?);
        }
    }

    if let Some(value) = raw_payload.get("quota_expires_at") {
        if value.is_null() {
            updated.quota_expires_at_unix_secs = None;
        } else {
            let Some(raw) = payload.quota_expires_at.as_deref() else {
                return Err("quota_expires_at 必须是字符串".to_string());
            };
            updated.quota_expires_at_unix_secs =
                Some(parse_optional_rfc3339_unix_secs(raw, "quota_expires_at")?);
        }
    }

    if let Some(value) = raw_payload.get("provider_priority") {
        let Some(provider_priority) = payload.provider_priority else {
            return Err(if value.is_null() {
                "provider_priority 不能为空".to_string()
            } else {
                "provider_priority 必须是整数".to_string()
            });
        };
        if !(0..=10_000).contains(&provider_priority) {
            return Err("provider_priority 必须在 0 到 10000 之间".to_string());
        }
        updated.provider_priority = provider_priority;
    }

    if let Some(_value) = raw_payload.get("keep_priority_on_conversion") {
        let Some(keep_priority_on_conversion) = payload.keep_priority_on_conversion else {
            return Err("keep_priority_on_conversion 必须是布尔值".to_string());
        };
        updated.keep_priority_on_conversion = keep_priority_on_conversion;
    }

    if let Some(_value) = raw_payload.get("is_active") {
        let Some(is_active) = payload.is_active else {
            return Err("is_active 必须是布尔值".to_string());
        };
        updated.is_active = is_active;
    }

    if raw_payload.contains_key("concurrent_limit") {
        updated.concurrent_limit = match payload.concurrent_limit {
            Some(value) if value >= 0 => Some(value),
            Some(_) => return Err("concurrent_limit 必须是非负整数".to_string()),
            None => None,
        };
    }

    if raw_payload.contains_key("max_retries") {
        updated.max_retries = match payload.max_retries {
            Some(value) if (0..=999).contains(&value) => Some(value),
            Some(_) => return Err("max_retries 必须是 0 到 999 之间的整数".to_string()),
            None => None,
        };
    }

    if raw_payload.contains_key("proxy") {
        updated.proxy = normalize_json_object(payload.proxy, "proxy")?;
    }

    if raw_payload.contains_key("stream_first_byte_timeout") {
        updated.stream_first_byte_timeout_secs = match payload.stream_first_byte_timeout {
            Some(value) if (1.0..=300.0).contains(&value) => Some(value),
            Some(_) => {
                return Err("stream_first_byte_timeout 必须是 1 到 300 之间的数字".to_string())
            }
            None => None,
        };
    }

    if raw_payload.contains_key("request_timeout") {
        updated.request_timeout_secs = match payload.request_timeout {
            Some(value) if (1.0..=600.0).contains(&value) => Some(value),
            Some(_) => return Err("request_timeout 必须是 1 到 600 之间的数字".to_string()),
            None => None,
        };
    }

    if let Some(_value) = raw_payload.get("enable_format_conversion") {
        let Some(enable_format_conversion) = payload.enable_format_conversion else {
            return Err("enable_format_conversion 必须是布尔值".to_string());
        };
        updated.enable_format_conversion = enable_format_conversion;
    }

    let config_seed = if raw_payload.contains_key("config") {
        normalize_json_object(payload.config, "config")?
    } else {
        updated.config.clone()
    };
    let mut config_map = config_seed
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();

    if raw_payload.contains_key("claude_code_advanced") {
        if raw_payload
            .get("claude_code_advanced")
            .is_some_and(serde_json::Value::is_null)
        {
            config_map.remove("claude_code_advanced");
        } else {
            if target_provider_type != "claude_code" {
                return Err("claude_code_advanced 仅适用于 provider_type=claude_code".to_string());
            }
            let value =
                normalize_json_object(payload.claude_code_advanced, "claude_code_advanced")?
                    .ok_or_else(|| "claude_code_advanced 必须是 JSON 对象".to_string())?;
            config_map.insert("claude_code_advanced".to_string(), value);
        }
    } else if target_provider_type != "claude_code" {
        config_map.remove("claude_code_advanced");
    }

    if raw_payload.contains_key("pool_advanced") {
        if raw_payload
            .get("pool_advanced")
            .is_some_and(serde_json::Value::is_null)
        {
            config_map.remove("pool_advanced");
        } else {
            let value = normalize_json_object(payload.pool_advanced, "pool_advanced")?
                .ok_or_else(|| "pool_advanced 必须是 JSON 对象".to_string())?;
            config_map.insert("pool_advanced".to_string(), value);
        }
    }

    if raw_payload.contains_key("failover_rules") {
        if raw_payload
            .get("failover_rules")
            .is_some_and(serde_json::Value::is_null)
        {
            config_map.remove("failover_rules");
        } else {
            let value = normalize_json_object(payload.failover_rules, "failover_rules")?
                .ok_or_else(|| "failover_rules 必须是 JSON 对象".to_string())?;
            config_map.insert("failover_rules".to_string(), value);
        }
    }

    updated.config = (!config_map.is_empty()).then_some(serde_json::Value::Object(config_map));
    updated.updated_at_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs());
    Ok(updated)
}

pub(crate) async fn build_admin_create_provider_record(
    state: &AppState,
    payload: AdminProviderCreateRequest,
) -> Result<(StoredProviderCatalogProvider, Option<i32>), String> {
    let name = payload.name.trim();
    if name.is_empty() {
        return Err("name 为必填字段".to_string());
    }

    let existing_providers = state
        .list_provider_catalog_providers(false)
        .await
        .map_err(|err| format!("{err:?}"))?;
    if existing_providers
        .iter()
        .any(|provider| provider.name == name)
    {
        return Err(format!("提供商名称 '{name}' 已存在"));
    }

    let provider_type =
        normalize_provider_type_input(payload.provider_type.as_deref().unwrap_or("custom"))?;
    let billing_type = normalize_provider_billing_type(
        payload.billing_type.as_deref().unwrap_or("pay_as_you_go"),
    )?;

    let website = payload.website.and_then(|value| {
        let trimmed = value.trim().to_string();
        (!trimmed.is_empty()).then_some(trimmed)
    });
    let website = website.map(|value| {
        if value.starts_with("http://") || value.starts_with("https://") {
            value
        } else {
            format!("https://{value}")
        }
    });

    let monthly_quota_usd = match payload.monthly_quota_usd {
        Some(value) if value.is_finite() && value >= 0.0 => Some(value),
        Some(_) => return Err("monthly_quota_usd 必须是非负数".to_string()),
        None => None,
    };
    let quota_reset_day = match payload.quota_reset_day {
        Some(value) if (1..=365).contains(&value) => Some(value),
        Some(_) => return Err("quota_reset_day 必须是 1 到 365 之间的整数".to_string()),
        None => Some(30),
    };
    let quota_last_reset_at_unix_secs = payload
        .quota_last_reset_at
        .as_deref()
        .map(|value| parse_optional_rfc3339_unix_secs(value, "quota_last_reset_at"))
        .transpose()?;
    let quota_expires_at_unix_secs = payload
        .quota_expires_at
        .as_deref()
        .map(|value| parse_optional_rfc3339_unix_secs(value, "quota_expires_at"))
        .transpose()?;
    let provider_priority = match payload.provider_priority {
        Some(value) if (0..=10_000).contains(&value) => value,
        Some(_) => return Err("provider_priority 必须在 0 到 10000 之间".to_string()),
        None => {
            let current_min_priority = existing_providers
                .iter()
                .map(|provider| provider.provider_priority)
                .min();
            match current_min_priority {
                Some(value) if value <= 0 => 0,
                Some(value) => value - 1,
                None => 100,
            }
        }
    };
    let shift_existing_priorities_from = match payload.provider_priority {
        Some(_) => Some(provider_priority),
        None => existing_providers
            .iter()
            .map(|provider| provider.provider_priority)
            .min()
            .filter(|value| *value <= 0)
            .map(|_| 0),
    };

    let is_active = payload.is_active.unwrap_or(true);
    let concurrent_limit = match payload.concurrent_limit {
        Some(value) if value >= 0 => Some(value),
        Some(_) => return Err("concurrent_limit 必须是非负整数".to_string()),
        None => None,
    };
    let max_retries = match payload.max_retries {
        Some(value) if (0..=999).contains(&value) => Some(value),
        Some(_) => return Err("max_retries 必须是 0 到 999 之间的整数".to_string()),
        None => Some(2),
    };
    let proxy = normalize_json_object(payload.proxy, "proxy")?;
    let stream_first_byte_timeout_secs = match payload.stream_first_byte_timeout {
        Some(value) if (1.0..=300.0).contains(&value) => Some(value),
        Some(_) => return Err("stream_first_byte_timeout 必须是 1 到 300 之间的数字".to_string()),
        None => None,
    };
    let request_timeout_secs = match payload.request_timeout {
        Some(value) if (1.0..=600.0).contains(&value) => Some(value),
        Some(_) => return Err("request_timeout 必须是 1 到 600 之间的数字".to_string()),
        None => None,
    };

    let mut config_map = normalize_json_object(payload.config, "config")?
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    if let Some(value) = normalize_json_object(payload.pool_advanced, "pool_advanced")? {
        config_map.insert("pool_advanced".to_string(), value);
    }
    if let Some(value) = normalize_json_object(payload.failover_rules, "failover_rules")? {
        config_map.insert("failover_rules".to_string(), value);
    }
    if let Some(value) =
        normalize_json_object(payload.claude_code_advanced, "claude_code_advanced")?
    {
        if provider_type != "claude_code" {
            return Err("claude_code_advanced 仅适用于 provider_type=claude_code".to_string());
        }
        config_map.insert("claude_code_advanced".to_string(), value);
    }
    let config = (!config_map.is_empty()).then_some(serde_json::Value::Object(config_map));

    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0);

    let record = StoredProviderCatalogProvider::new(
        Uuid::new_v4().to_string(),
        name.to_string(),
        website,
        provider_type.clone(),
    )
    .map_err(|err| err.to_string())?
    .with_description(
        payload
            .description
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
    )
    .with_billing_fields(
        Some(billing_type),
        monthly_quota_usd,
        None,
        quota_reset_day,
        quota_last_reset_at_unix_secs,
        quota_expires_at_unix_secs,
    )
    .with_routing_fields(provider_priority)
    .with_transport_fields(
        is_active,
        payload.keep_priority_on_conversion.unwrap_or(false),
        provider_type_enables_format_conversion_by_default(&provider_type),
        concurrent_limit,
        max_retries,
        proxy,
        request_timeout_secs,
        stream_first_byte_timeout_secs,
        config,
    )
    .with_timestamps(Some(now_unix_secs), Some(now_unix_secs));

    Ok((record, shift_existing_priorities_from))
}

pub(crate) fn build_admin_fixed_provider_endpoint_record(
    provider: &StoredProviderCatalogProvider,
    api_format: &str,
    base_url: &str,
) -> Result<StoredProviderCatalogEndpoint, String> {
    let (normalized_api_format, api_family, endpoint_kind) =
        admin_endpoint_signature_parts(api_format)
            .ok_or_else(|| format!("无效的 api_format: {api_format}"))?;
    let body_rules = admin_default_body_rules_for_signature(
        normalized_api_format,
        Some(provider.provider_type.as_str()),
    )
    .and_then(|(_, rules)| (!rules.is_empty()).then_some(serde_json::Value::Array(rules)));
    let endpoint_config =
        if provider.provider_type == "codex" && normalized_api_format == "openai:cli" {
            Some(json!({ "upstream_stream_policy": "force_stream" }))
        } else {
            None
        };
    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0);

    StoredProviderCatalogEndpoint::new(
        Uuid::new_v4().to_string(),
        provider.id.clone(),
        normalized_api_format.to_string(),
        Some(api_family.to_string()),
        Some(endpoint_kind.to_string()),
        true,
    )
    .map_err(|err| err.to_string())?
    .with_timestamps(Some(now_unix_secs), Some(now_unix_secs))
    .with_transport_fields(
        normalize_admin_base_url(base_url)?,
        None,
        body_rules,
        Some(provider.max_retries.unwrap_or(2)),
        None,
        endpoint_config,
        None,
        None,
    )
    .map_err(|err| err.to_string())
}
