use crate::handlers::admin::provider::shared::payloads::AdminProviderUpdatePatch;
use crate::handlers::admin::provider::shared::support::{
    normalize_provider_billing_type, normalize_provider_transfer_limit,
    normalize_provider_transfer_limit_json, parse_optional_rfc3339_unix_secs,
    PROVIDER_MAX_TRANSFER_COUNT_CONFIG_KEY, PROVIDER_MAX_TRANSFER_TIMEOUT_SECONDS_CONFIG_KEY,
};
use crate::handlers::admin::provider::write::normalize::normalize_chat_pii_redaction_config;
use crate::handlers::admin::provider::write::normalize::normalize_pool_advanced_config;
use crate::handlers::admin::provider::write::normalize::normalize_provider_type_input;
use crate::handlers::admin::provider::write::normalize::set_responses_websocket_enabled;
use crate::handlers::admin::provider::write::normalize::validate_responses_websocket_config;
use crate::handlers::admin::request::AdminAppState;
use crate::handlers::admin::shared::normalize_json_object;
use aether_admin::provider::redaction::{
    admin_restore_secret_safe_json, admin_restore_secret_safe_proxy,
};
use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogProvider;
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) async fn build_admin_update_provider_record(
    state: &AdminAppState<'_>,
    existing: &StoredProviderCatalogProvider,
    patch: AdminProviderUpdatePatch,
) -> Result<StoredProviderCatalogProvider, String> {
    let state = state.as_ref();
    let mut updated = existing.clone();
    let (fields, payload) = patch.into_parts();

    if fields.contains("name") {
        let Some(name) = payload.name.as_deref() else {
            return Err(if fields.is_null("name") {
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

    let target_provider_type = if fields.contains("provider_type") {
        let Some(provider_type) = payload.provider_type.as_deref() else {
            return Err(if fields.is_null("provider_type") {
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

    if fields.contains("description") {
        updated.description = payload
            .description
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
    }

    if fields.contains("website") {
        updated.website = match payload.website {
            None => {
                if fields.is_null("website") {
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

    if fields.contains("billing_type") {
        let Some(billing_type) = payload.billing_type.as_deref() else {
            return Err(if fields.is_null("billing_type") {
                "billing_type 不能为空".to_string()
            } else {
                "billing_type 必须是字符串".to_string()
            });
        };
        updated.billing_type = Some(normalize_provider_billing_type(billing_type)?);
    }

    if fields.contains("monthly_quota_usd") {
        if fields.is_null("monthly_quota_usd") {
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

    if fields.contains("quota_reset_day") {
        if fields.is_null("quota_reset_day") {
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

    if fields.contains("quota_last_reset_at") {
        if fields.is_null("quota_last_reset_at") {
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

    if fields.contains("quota_expires_at") {
        if fields.is_null("quota_expires_at") {
            updated.quota_expires_at_unix_secs = None;
        } else {
            let Some(raw) = payload.quota_expires_at.as_deref() else {
                return Err("quota_expires_at 必须是字符串".to_string());
            };
            updated.quota_expires_at_unix_secs =
                Some(parse_optional_rfc3339_unix_secs(raw, "quota_expires_at")?);
        }
    }

    if fields.contains("provider_priority") {
        let Some(provider_priority) = payload.provider_priority else {
            return Err(if fields.is_null("provider_priority") {
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

    if fields.contains("keep_priority_on_conversion") {
        let Some(keep_priority_on_conversion) = payload.keep_priority_on_conversion else {
            return Err("keep_priority_on_conversion 必须是布尔值".to_string());
        };
        updated.keep_priority_on_conversion = keep_priority_on_conversion;
    }

    if fields.contains("is_active") {
        let Some(is_active) = payload.is_active else {
            return Err("is_active 必须是布尔值".to_string());
        };
        updated.is_active = is_active;
    }

    if fields.contains("concurrent_limit") {
        updated.concurrent_limit = match payload.concurrent_limit {
            Some(value) if value >= 0 => Some(value),
            Some(_) => return Err("concurrent_limit 必须是非负整数".to_string()),
            None => None,
        };
    }

    if fields.contains("max_retries") {
        updated.max_retries = match payload.max_retries {
            Some(value) if (0..=999).contains(&value) => Some(value),
            Some(_) => return Err("max_retries 必须是 0 到 999 之间的整数".to_string()),
            None => None,
        };
    }

    if fields.contains("proxy") {
        updated.proxy = normalize_json_object(payload.proxy, "proxy")?
            .map(|value| admin_restore_secret_safe_proxy(existing.proxy.as_ref(), &value));
    }

    if fields.contains("stream_first_byte_timeout") {
        updated.stream_first_byte_timeout_secs =
            super::normalize_provider_stream_first_byte_timeout(payload.stream_first_byte_timeout)?;
    }

    if fields.contains("request_timeout") {
        updated.request_timeout_secs =
            super::normalize_provider_request_timeout(payload.request_timeout)?;
    }

    if fields.contains("enable_format_conversion") {
        let Some(enable_format_conversion) = payload.enable_format_conversion else {
            return Err("enable_format_conversion 必须是布尔值".to_string());
        };
        updated.enable_format_conversion = enable_format_conversion;
    }

    let mut config_map = updated
        .config
        .clone()
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    if fields.contains("config") {
        if fields.is_null("config") {
            config_map.clear();
        } else {
            let value = normalize_json_object(payload.config, "config")?
                .ok_or_else(|| "config 必须是 JSON 对象".to_string())?;
            let value = admin_restore_secret_safe_json(existing.config.as_ref(), &value);
            let serde_json::Value::Object(patch_map) = value else {
                return Err("config 必须是 JSON 对象".to_string());
            };
            for (key, value) in patch_map {
                if value.is_null() {
                    config_map.remove(&key);
                } else {
                    config_map.insert(key, value);
                }
            }
        }
    }

    if fields.contains("codex_fingerprint_convergence_enabled") {
        let Some(enabled) = payload.codex_fingerprint_convergence_enabled else {
            return Err("codex_fingerprint_convergence_enabled 必须是布尔值".to_string());
        };
        if target_provider_type != "codex" && enabled {
            return Err(
                "codex_fingerprint_convergence_enabled 仅适用于 provider_type=codex".to_string(),
            );
        }
        if target_provider_type == "codex" {
            let codex_config = config_map
                .entry(crate::provider_transport::CODEX_FINGERPRINT_CONFIG_NAMESPACE.to_string())
                .or_insert_with(|| json!({}));
            let Some(codex_config) = codex_config.as_object_mut() else {
                return Err("config.codex 必须是 JSON 对象".to_string());
            };
            codex_config.insert(
                crate::provider_transport::CODEX_FINGERPRINT_ENABLED_CONFIG_KEY.to_string(),
                json!(enabled),
            );
        }
    }
    if target_provider_type != "codex" {
        remove_codex_fingerprint_config(&mut config_map);
    }

    for (field_name, payload_value) in [
        (
            PROVIDER_MAX_TRANSFER_COUNT_CONFIG_KEY,
            payload.max_transfer_count,
        ),
        (
            PROVIDER_MAX_TRANSFER_TIMEOUT_SECONDS_CONFIG_KEY,
            payload.max_transfer_timeout_seconds,
        ),
    ] {
        if fields.contains(field_name) {
            let value = payload_value
                .map(|value| normalize_provider_transfer_limit(value, field_name))
                .transpose()?
                .unwrap_or(0);
            config_map.insert(field_name.to_string(), json!(value));
        } else if fields.contains("config") {
            if let Some(value) = config_map.get(field_name) {
                let value = normalize_provider_transfer_limit_json(value, field_name)?;
                config_map.insert(field_name.to_string(), json!(value));
            }
        }
    }

    if fields.contains("claude_code_advanced") {
        if fields.is_null("claude_code_advanced") {
            config_map.remove("claude_code_advanced");
        } else {
            if target_provider_type != "claude_code" {
                return Err("claude_code_advanced 仅适用于 provider_type=claude_code".to_string());
            }
            let value =
                normalize_json_object(payload.claude_code_advanced, "claude_code_advanced")?
                    .ok_or_else(|| "claude_code_advanced 必须是 JSON 对象".to_string())?;
            let value = admin_restore_secret_safe_json(
                existing
                    .config
                    .as_ref()
                    .and_then(|config| config.get("claude_code_advanced")),
                &value,
            );
            config_map.insert("claude_code_advanced".to_string(), value);
        }
    } else if target_provider_type != "claude_code" {
        config_map.remove("claude_code_advanced");
    }

    if fields.contains("pool_advanced") {
        if fields.is_null("pool_advanced") {
            config_map.remove("pool_advanced");
        } else {
            let value = normalize_pool_advanced_config(payload.pool_advanced)?
                .ok_or_else(|| "pool_advanced 必须是 JSON 对象".to_string())?;
            let value = admin_restore_secret_safe_json(
                existing
                    .config
                    .as_ref()
                    .and_then(|config| config.get("pool_advanced")),
                &value,
            );
            config_map.insert("pool_advanced".to_string(), value);
        }
    }

    if fields.contains("failover_rules") {
        if fields.is_null("failover_rules") {
            config_map.remove("failover_rules");
        } else {
            let value = normalize_json_object(payload.failover_rules, "failover_rules")?
                .ok_or_else(|| "failover_rules 必须是 JSON 对象".to_string())?;
            let value = admin_restore_secret_safe_json(
                existing
                    .config
                    .as_ref()
                    .and_then(|config| config.get("failover_rules")),
                &value,
            );
            config_map.insert("failover_rules".to_string(), value);
        }
    }

    if config_map.contains_key("chat_pii_redaction") {
        let value = normalize_chat_pii_redaction_config(config_map.remove("chat_pii_redaction"))?;
        if let Some(value) = value {
            config_map.insert("chat_pii_redaction".to_string(), value);
        }
    }

    if fields.contains("responses_websocket_enabled") {
        let enabled = payload
            .responses_websocket_enabled
            .ok_or_else(|| "responses_websocket_enabled 必须是布尔值".to_string())?;
        set_responses_websocket_enabled(&mut config_map, enabled)?;
    }
    validate_responses_websocket_config(&config_map)?;

    updated.config = (!config_map.is_empty()).then_some(serde_json::Value::Object(config_map));
    crate::provider_transport::validate_anthropic_compatibility_profile_config(
        updated.config.as_ref(),
    )
    .map_err(|_| "无效的 Anthropic compatibility profile".to_string())?;
    updated.updated_at_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs());
    Ok(updated)
}

fn remove_codex_fingerprint_config(config_map: &mut serde_json::Map<String, serde_json::Value>) {
    let namespace = crate::provider_transport::CODEX_FINGERPRINT_CONFIG_NAMESPACE;
    let key = crate::provider_transport::CODEX_FINGERPRINT_ENABLED_CONFIG_KEY;
    let mut remove_namespace = false;
    if let Some(codex_config) = config_map
        .get_mut(namespace)
        .and_then(|value| value.as_object_mut())
    {
        codex_config.remove(key);
        remove_namespace = codex_config.is_empty();
    }
    if remove_namespace {
        config_map.remove(namespace);
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn removing_fingerprint_setting_preserves_other_codex_config() {
        let mut config = json!({
            "codex": {
                "fingerprint_convergence_enabled": true,
                "pass_through_cyber_flag_interrupt": true
            },
            "other": {"kept": true}
        })
        .as_object()
        .expect("config object")
        .clone();

        super::remove_codex_fingerprint_config(&mut config);

        assert_eq!(
            config["codex"],
            json!({"pass_through_cyber_flag_interrupt": true})
        );
        assert_eq!(config["other"], json!({"kept": true}));
    }
}
