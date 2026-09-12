use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey,
};
use aether_provider_transport::provider_types::fixed_provider_key_inherits_api_formats;
use chrono::{TimeZone, Utc};
use serde_json::{json, Value};
use std::collections::BTreeMap;

use super::redaction::{
    admin_restore_secret_safe_body_rules, admin_restore_secret_safe_header_rules,
    admin_restore_secret_safe_json, admin_restore_secret_safe_proxy, admin_restore_secret_safe_url,
    admin_secret_safe_body_rules, admin_secret_safe_header_rules, admin_secret_safe_json,
    admin_secret_safe_proxy, admin_secret_safe_url, admin_validate_retained_body_rule_secrets,
    admin_validate_retained_header_rule_secrets,
};

pub fn normalize_endpoint_api_format(api_format: &str) -> String {
    aether_ai_formats::normalize_api_format_alias(api_format)
}

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

fn endpoint_api_format_sets(
    endpoints: &[StoredProviderCatalogEndpoint],
) -> (Vec<String>, Vec<String>) {
    let mut all = Vec::new();
    let mut active = Vec::new();
    for endpoint in endpoints {
        let api_format = normalize_endpoint_api_format(&endpoint.api_format);
        if !all.iter().any(|existing| existing == &api_format) {
            all.push(api_format.clone());
        }
        if endpoint.is_active && !active.iter().any(|existing| existing == &api_format) {
            active.push(api_format);
        }
    }
    (all, active)
}

fn configured_key_api_formats(key: &StoredProviderCatalogKey) -> Vec<String> {
    let Some(formats) = key
        .api_formats
        .as_ref()
        .and_then(serde_json::Value::as_array)
    else {
        return Vec::new();
    };
    let mut normalized = Vec::new();
    for api_format in formats.iter().filter_map(serde_json::Value::as_str) {
        let api_format = normalize_endpoint_api_format(api_format);
        if !normalized.iter().any(|existing| existing == &api_format) {
            normalized.push(api_format);
        }
    }
    normalized
}

pub fn endpoint_key_counts_by_format(
    provider_type: &str,
    endpoints: &[StoredProviderCatalogEndpoint],
    keys: &[StoredProviderCatalogKey],
) -> (BTreeMap<String, usize>, BTreeMap<String, usize>) {
    let mut total = BTreeMap::new();
    let mut active = BTreeMap::new();
    let (endpoint_api_formats, active_endpoint_api_formats) = endpoint_api_format_sets(endpoints);

    for key in keys {
        let inherits_api_formats = fixed_provider_key_inherits_api_formats(
            provider_type,
            &key.auth_type,
            key.encrypted_auth_config.as_deref(),
        );
        let has_unrestricted_api_format_scope = key
            .api_formats
            .as_ref()
            .is_none_or(serde_json::Value::is_null);
        let configured_api_formats = configured_key_api_formats(key);

        let candidate_api_formats = if inherits_api_formats {
            &active_endpoint_api_formats
        } else {
            &endpoint_api_formats
        };
        for api_format in candidate_api_formats.iter().filter(|api_format| {
            inherits_api_formats
                || has_unrestricted_api_format_scope
                || configured_api_formats.iter().any(|allowed| {
                    aether_ai_formats::api_format_permission_covers(allowed, api_format)
                })
        }) {
            *total.entry(api_format.clone()).or_insert(0) += 1;
            if key.is_active {
                *active.entry(api_format.clone()).or_insert(0) += 1;
            }
        }
    }
    (total, active)
}

#[cfg(test)]
mod endpoint_key_count_tests {
    use super::*;

    fn sample_endpoint(id: &str, api_format: &str) -> StoredProviderCatalogEndpoint {
        StoredProviderCatalogEndpoint::new(
            id.to_string(),
            "provider-1".to_string(),
            api_format.to_string(),
            None,
            None,
            true,
        )
        .expect("endpoint should build")
    }

    fn sample_key(id: &str, api_format: Option<&str>) -> StoredProviderCatalogKey {
        let mut key = StoredProviderCatalogKey::new(
            id.to_string(),
            "provider-1".to_string(),
            id.to_string(),
            "api_key".to_string(),
            None,
            true,
        )
        .expect("key should build");
        key.api_formats = api_format.map(|api_format| json!([api_format]));
        key
    }

    #[test]
    fn endpoint_counts_follow_one_way_format_permissions() {
        let endpoints = vec![
            sample_endpoint("responses", "openai:responses"),
            sample_endpoint("search", "openai:search"),
        ];
        let mut empty_scope_key = sample_key("empty-scope-key", None);
        empty_scope_key.api_formats = Some(json!([]));
        let keys = vec![
            sample_key("responses-key", Some("openai:responses")),
            sample_key("search-key", Some("openai:search")),
            sample_key("unrestricted-key", None),
            empty_scope_key,
        ];

        let (total, active) = endpoint_key_counts_by_format("custom", &endpoints, &keys);

        assert_eq!(total.get("openai:responses"), Some(&2));
        assert_eq!(total.get("openai:search"), Some(&3));
        assert_eq!(active, total);
    }

    #[test]
    fn endpoint_counts_keep_scoped_keys_visible_on_inactive_endpoints() {
        let mut endpoint = sample_endpoint("chat", "openai:chat");
        endpoint.is_active = false;
        let mut empty_scope_key = sample_key("empty-scope-key", None);
        empty_scope_key.api_formats = Some(json!([]));
        let keys = vec![
            sample_key("chat-key", Some("openai:chat")),
            sample_key("unrestricted-key", None),
            empty_scope_key,
        ];

        let (total, active) = endpoint_key_counts_by_format("custom", &[endpoint], &keys);

        assert_eq!(total.get("openai:chat"), Some(&2));
        assert_eq!(active, total);
    }

    #[test]
    fn endpoint_updates_reject_unresolved_rule_masks_before_persistence() {
        let header_rules = json!([{"action": "set", "key": "x-auth", "value": "header-secret"}]);
        let body_rules = json!([{"action": "set", "path": "auth.token", "value": "body-secret"}]);
        let mut endpoint = sample_endpoint("chat", "openai:chat");
        endpoint.header_rules = Some(header_rules.clone());
        endpoint.body_rules = Some(body_rules.clone());
        endpoint.config = Some(json!({"response_header_rules": header_rules}));
        let moved_header =
            json!([{"action": "set", "key": "x-other-auth", "value": "***", "has_value": true}]);
        let cases = [
            (
                "header_rules",
                super::AdminProviderEndpointUpdateFields {
                    header_rules: Some(moved_header.clone()),
                    ..Default::default()
                },
            ),
            (
                "body_rules",
                super::AdminProviderEndpointUpdateFields {
                    body_rules: Some(
                        json!([{"action": "set", "path": "auth.api_key", "value": "***", "has_value": true}]),
                    ),
                    ..Default::default()
                },
            ),
            (
                "config",
                super::AdminProviderEndpointUpdateFields {
                    config: Some(json!({"response_header_rules": moved_header})),
                    ..Default::default()
                },
            ),
        ];
        for (field, payload) in cases {
            let error = super::apply_admin_provider_endpoint_update_fields(
                &endpoint,
                |key| key == field,
                |_| false,
                &payload,
            )
            .expect_err("unresolved masks must not overwrite saved secrets");
            assert!(error.contains("无法匹配"));
            assert!(!error.contains("header-secret"));
            assert!(!error.contains("body-secret"));
        }
    }

    #[test]
    fn inherited_endpoint_counts_only_include_active_formats() {
        let responses_endpoint = sample_endpoint("responses", "openai:responses");
        let mut search_endpoint = sample_endpoint("search", "openai:search");
        search_endpoint.is_active = false;
        let mut inherited_key = sample_key("codex-key", Some("legacy:mismatch"));
        inherited_key.auth_type = "oauth".to_string();

        let (total, active) = endpoint_key_counts_by_format(
            "codex",
            &[responses_endpoint, search_endpoint],
            &[inherited_key],
        );

        assert_eq!(total.get("openai:responses"), Some(&1));
        assert!(!total.contains_key("openai:search"));
        assert_eq!(active, total);
    }
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
        "base_url": admin_secret_safe_url(Some(&endpoint.base_url)),
        "custom_path": endpoint.custom_path,
        "header_rules": admin_secret_safe_header_rules(endpoint.header_rules.as_ref()),
        "body_rules": admin_secret_safe_body_rules(endpoint.body_rules.as_ref()),
        "max_retries": endpoint.max_retries.unwrap_or(2),
        "is_active": endpoint.is_active,
        "config": admin_secret_safe_json(endpoint.config.as_ref()),
        "proxy": admin_secret_safe_proxy(endpoint.proxy.as_ref()),
        "format_acceptance_config": admin_secret_safe_json(endpoint.format_acceptance_config.as_ref()),
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
        updated.base_url =
            admin_restore_secret_safe_url(Some(&existing_endpoint.base_url), base_url);
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
            admin_validate_retained_header_rule_secrets(
                existing_endpoint.header_rules.as_ref(),
                header_rules,
            )?;
            Some(admin_restore_secret_safe_header_rules(
                existing_endpoint.header_rules.as_ref(),
                header_rules,
            ))
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
            admin_validate_retained_body_rule_secrets(
                existing_endpoint.body_rules.as_ref(),
                body_rules,
            )?;
            Some(admin_restore_secret_safe_body_rules(
                existing_endpoint.body_rules.as_ref(),
                body_rules,
            ))
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
            if let Some(rules) = config.get("response_header_rules") {
                admin_validate_retained_header_rule_secrets(
                    existing_endpoint
                        .config
                        .as_ref()
                        .and_then(|config| config.get("response_header_rules")),
                    rules,
                )?;
            }
            Some(admin_restore_secret_safe_json(
                existing_endpoint.config.as_ref(),
                config,
            ))
        };
    }

    if contains_field("proxy") {
        if is_null_field("proxy") {
            updated.proxy = None;
        } else {
            let Some(proxy) = payload.proxy.as_ref().and_then(Value::as_object) else {
                return Err("proxy 必须是对象或 null".to_string());
            };
            let restored = admin_restore_secret_safe_proxy(
                existing_endpoint.proxy.as_ref(),
                &Value::Object(proxy.clone()),
            );
            updated.proxy = Some(restored);
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
            Some(admin_restore_secret_safe_json(
                existing_endpoint.format_acceptance_config.as_ref(),
                config,
            ))
        };
    }

    Ok(updated)
}
