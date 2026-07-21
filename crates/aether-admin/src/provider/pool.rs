use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogKeyStats,
    StoredProviderCatalogProvider,
};
use chrono::{TimeZone, Utc};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};

use super::status as provider_status;

#[derive(Debug, Default, Clone, serde::Deserialize)]
pub struct AdminPoolResolveSelectionRequest {
    #[serde(default)]
    pub search: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub quick_selectors: Vec<String>,
}

#[derive(Debug, Default, Clone, serde::Deserialize)]
pub struct AdminPoolBatchActionRequest {
    #[serde(default)]
    pub key_ids: Vec<String>,
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub payload: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdminPoolBatchActionKind {
    Enable,
    Disable,
    ClearProxy,
    SetProxy,
    UpdateSettings,
    RegenerateFingerprint,
    Delete,
}

#[derive(Debug, Clone)]
pub struct AdminPoolBatchActionPlan {
    pub key_ids: Vec<String>,
    pub action: AdminPoolBatchActionKind,
    pub action_label: &'static str,
    pub proxy_payload: Option<Value>,
    pub settings_payload: Option<Value>,
}

#[derive(Debug, Clone, Default)]
pub struct AdminPoolKeyPayloadContext {
    pub cooldown_reason: Option<String>,
    pub cooldown_ttl_seconds: Option<u64>,
    pub cost_window_usage: u64,
    pub sticky_sessions: usize,
    pub lru_score: Option<f64>,
    pub cost_limit: Option<u64>,
}

#[derive(Debug, Default, Clone, serde::Deserialize)]
pub struct AdminPoolBatchImportRequest {
    #[serde(default)]
    pub keys: Vec<AdminPoolBatchImportItem>,
    #[serde(default)]
    pub proxy_node_id: Option<String>,
    #[serde(default)]
    pub api_formats: Vec<String>,
    #[serde(default)]
    pub settings: Option<Value>,
}

#[derive(Debug, Default, Clone, serde::Deserialize)]
pub struct AdminPoolBatchImportItem {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub auth_type: String,
    #[serde(default)]
    pub api_formats: Vec<String>,
    #[serde(default)]
    pub settings: Option<Value>,
}

fn admin_pool_reason_indicates_ban(reason: &str) -> bool {
    let normalized = reason.trim().to_ascii_lowercase();
    !normalized.is_empty()
        && [
            "banned",
            "forbidden",
            "blocked",
            "suspend",
            "deactivated",
            "disabled",
            "verification",
            "workspace",
            "受限",
            "封",
            "禁",
        ]
        .iter()
        .any(|hint| normalized.contains(hint))
}

pub fn admin_pool_key_account_quota_exhausted(
    key: &StoredProviderCatalogKey,
    provider_type: &str,
) -> bool {
    aether_provider_pool::provider_pool_key_account_quota_exhausted(key, provider_type)
}

fn admin_pool_has_proxy(key: &StoredProviderCatalogKey) -> bool {
    match key.proxy.as_ref() {
        Some(Value::Object(values)) => !values.is_empty(),
        Some(Value::String(value)) => !value.trim().is_empty(),
        Some(Value::Bool(value)) => *value,
        Some(Value::Number(_)) => true,
        Some(Value::Array(values)) => !values.is_empty(),
        _ => false,
    }
}

fn admin_pool_string_list(value: Option<&Value>) -> Option<Vec<String>> {
    let values = value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if values.is_empty() {
        None
    } else {
        Some(values)
    }
}

fn admin_pool_json_object(value: Option<&Value>) -> Option<serde_json::Map<String, Value>> {
    value
        .and_then(Value::as_object)
        .cloned()
        .filter(|value| !value.is_empty())
}

fn admin_pool_health_score(key: &StoredProviderCatalogKey) -> f64 {
    let scores = key
        .health_by_format
        .as_ref()
        .and_then(Value::as_object)
        .map(|formats| {
            formats
                .values()
                .filter_map(Value::as_object)
                .filter_map(|item| item.get("health_score"))
                .filter_map(Value::as_f64)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if scores.is_empty() {
        1.0
    } else {
        scores.into_iter().fold(1.0, f64::min)
    }
}

fn unix_secs_to_rfc3339(unix_secs: u64) -> Option<String> {
    Utc.timestamp_opt(unix_secs as i64, 0)
        .single()
        .map(|value| value.to_rfc3339())
}

fn admin_pool_scheduling_payload(
    key: &StoredProviderCatalogKey,
    cooldown_reason: Option<&str>,
    cooldown_ttl_seconds: Option<u64>,
) -> (String, String, String, Vec<Value>) {
    if !key.is_active {
        return (
            "blocked".to_string(),
            "inactive".to_string(),
            "已禁用".to_string(),
            vec![json!({
                "code": "inactive",
                "label": "已禁用",
                "blocking": true,
                "source": "manual",
                "ttl_seconds": Value::Null,
                "detail": Value::Null,
            })],
        );
    }
    if let Some(reason) = cooldown_reason {
        return (
            "degraded".to_string(),
            "cooldown".to_string(),
            "冷却中".to_string(),
            vec![json!({
                "code": "cooldown",
                "label": "冷却中",
                "blocking": true,
                "source": "pool",
                "ttl_seconds": cooldown_ttl_seconds,
                "detail": reason,
            })],
        );
    }
    (
        "available".to_string(),
        "available".to_string(),
        "可用".to_string(),
        Vec::new(),
    )
}

pub fn admin_pool_normalize_text(value: impl AsRef<str>) -> String {
    value.as_ref().trim().to_ascii_lowercase()
}

pub fn admin_pool_is_oauth_invalid(key: &StoredProviderCatalogKey, now_unix_secs: u64) -> bool {
    if key.auth_type.trim() != "oauth" {
        return false;
    }
    if let Some(reason) = key.oauth_invalid_reason.as_deref().map(str::trim) {
        let account_state = provider_status::resolve_pool_account_state(
            None,
            key.upstream_metadata.as_ref(),
            Some(reason),
        );
        if account_state.blocked && !account_state.recoverable {
            return true;
        }
        if admin_pool_reason_has_tag(reason, "[REFRESH_FAILED]") {
            return key
                .expires_at_unix_secs
                .is_none_or(|value| value == 0 || value <= now_unix_secs);
        }
        if admin_pool_reason_has_tag(reason, "[REQUEST_FAILED]") {
            return false;
        }
        if !reason.is_empty() {
            return true;
        }
    }
    key.expires_at_unix_secs
        .is_some_and(|value| value > 0 && value <= now_unix_secs)
}

fn admin_pool_reason_has_tag(reason: &str, tag: &str) -> bool {
    reason
        .lines()
        .map(str::trim)
        .any(|line| line.starts_with(tag))
}

pub fn admin_pool_matches_quick_selector(
    key: &StoredProviderCatalogKey,
    selector: &str,
    oauth_plan_type: Option<&str>,
    now_unix_secs: u64,
) -> bool {
    match selector {
        "banned" => admin_pool_key_is_known_banned(key),
        "oauth_invalid" => admin_pool_is_oauth_invalid(key, now_unix_secs),
        "proxy_unset" => !admin_pool_has_proxy(key),
        "proxy_set" => admin_pool_has_proxy(key),
        "disabled" => !key.is_active,
        "enabled" => key.is_active,
        "plan_free" => oauth_plan_type.is_some_and(|value| value.contains("free")),
        "plan_team" => oauth_plan_type.is_some_and(|value| value.contains("team")),
        "no_5h_limit" | "no_weekly_limit" => false,
        _ => false,
    }
}

pub fn admin_pool_matches_search(
    key: &StoredProviderCatalogKey,
    search: Option<&str>,
    oauth_plan_type: Option<&str>,
) -> bool {
    let Some(search) = search else {
        return true;
    };
    let search = admin_pool_normalize_text(search);
    if search.is_empty() {
        return true;
    }

    let mut search_fields = vec![
        key.id.clone(),
        key.name.clone(),
        key.auth_type.clone(),
        if key.is_active {
            "已启用".to_string()
        } else {
            "已禁用".to_string()
        },
        if admin_pool_has_proxy(key) {
            "独立代理".to_string()
        } else {
            "未配置代理".to_string()
        },
    ];
    if let Some(reason) = key.oauth_invalid_reason.as_ref() {
        search_fields.push(reason.clone());
    }
    if let Some(note) = key.note.as_ref() {
        search_fields.push(note.clone());
    }
    if let Some(plan_type) = oauth_plan_type {
        search_fields.push(plan_type.to_string());
    }

    search_fields
        .into_iter()
        .any(|value| admin_pool_normalize_text(&value).contains(&search))
}

pub fn admin_pool_key_is_known_banned(key: &StoredProviderCatalogKey) -> bool {
    let state = provider_status::resolve_pool_account_state(
        None,
        key.upstream_metadata.as_ref(),
        key.oauth_invalid_reason.as_deref(),
    );
    if provider_status::account_state_indicates_known_ban(&state) {
        return true;
    }
    key.oauth_invalid_reason
        .as_deref()
        .is_some_and(admin_pool_reason_indicates_ban)
}

pub fn admin_pool_sort_keys(keys: &mut [StoredProviderCatalogKey]) {
    keys.sort_by(|left, right| {
        left.internal_priority
            .cmp(&right.internal_priority)
            .then(left.name.cmp(&right.name))
            .then(left.id.cmp(&right.id))
    });
}

pub fn admin_pool_now_unix_secs() -> u64 {
    Utc::now().timestamp().max(0) as u64
}

pub fn admin_pool_api_formats(key: &StoredProviderCatalogKey) -> Vec<String> {
    key.api_formats
        .as_ref()
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

pub fn admin_pool_key_proxy_value(proxy_node_id: Option<&str>) -> Option<Value> {
    proxy_node_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| json!({ "node_id": value, "enabled": true }))
}

const ADMIN_POOL_KEY_SETTING_FIELDS: &[&str] = &[
    "internal_priority",
    "rpm_limit",
    "concurrent_limit",
    "cache_ttl_minutes",
    "max_probe_interval_minutes",
    "is_active",
    "note",
    "proxy_node_id",
];

fn admin_pool_settings_object(payload: &Value) -> Result<&Map<String, Value>, String> {
    let settings = payload
        .as_object()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "settings payload must be a non-empty object".to_string())?;
    if let Some(field) = settings
        .keys()
        .find(|field| !ADMIN_POOL_KEY_SETTING_FIELDS.contains(&field.as_str()))
    {
        return Err(format!("Unsupported key setting: {field}"));
    }
    Ok(settings)
}

fn admin_pool_setting_i64(
    settings: &Map<String, Value>,
    field: &str,
    min: i64,
    max: i64,
) -> Result<Option<i64>, String> {
    let Some(value) = settings.get(field) else {
        return Ok(None);
    };
    let number = value
        .as_i64()
        .ok_or_else(|| format!("{field} must be an integer"))?;
    if !(min..=max).contains(&number) {
        return Err(format!("{field} must be between {min} and {max}"));
    }
    Ok(Some(number))
}

pub fn validate_admin_pool_key_settings_payload(payload: &Value) -> Result<(), String> {
    let settings = admin_pool_settings_object(payload)?;
    admin_pool_setting_i64(settings, "internal_priority", 0, i32::MAX as i64)?;
    if settings
        .get("rpm_limit")
        .is_some_and(|value| !value.is_null())
    {
        admin_pool_setting_i64(settings, "rpm_limit", 1, 10_000)?;
    }
    if settings
        .get("concurrent_limit")
        .is_some_and(|value| !value.is_null())
    {
        admin_pool_setting_i64(settings, "concurrent_limit", 0, i32::MAX as i64)?;
    }
    admin_pool_setting_i64(settings, "cache_ttl_minutes", 0, 60)?;
    admin_pool_setting_i64(settings, "max_probe_interval_minutes", 0, 32)?;
    if settings
        .get("is_active")
        .is_some_and(|value| !value.is_boolean())
    {
        return Err("is_active must be a boolean".to_string());
    }
    if settings
        .get("note")
        .is_some_and(|value| !value.is_null() && !value.is_string())
    {
        return Err("note must be a string or null".to_string());
    }
    if settings
        .get("proxy_node_id")
        .is_some_and(|value| !value.is_null() && !value.is_string())
    {
        return Err("proxy_node_id must be a string or null".to_string());
    }
    Ok(())
}

pub fn apply_admin_pool_key_settings(
    key: &mut StoredProviderCatalogKey,
    payload: &Value,
) -> Result<(), String> {
    validate_admin_pool_key_settings_payload(payload)?;
    let settings = admin_pool_settings_object(payload)?;
    if let Some(value) = admin_pool_setting_i64(settings, "internal_priority", 0, i32::MAX as i64)?
    {
        key.internal_priority = value as i32;
    }
    if let Some(value) = settings.get("rpm_limit") {
        key.rpm_limit = if value.is_null() {
            None
        } else {
            Some(value.as_u64().expect("validated rpm_limit") as u32)
        };
    }
    if let Some(value) = settings.get("concurrent_limit") {
        key.concurrent_limit = if value.is_null() || value.as_i64() == Some(0) {
            None
        } else {
            Some(value.as_i64().expect("validated concurrent_limit") as i32)
        };
    }
    if let Some(value) = admin_pool_setting_i64(settings, "cache_ttl_minutes", 0, 60)? {
        key.cache_ttl_minutes = value as i32;
    }
    if let Some(value) = admin_pool_setting_i64(settings, "max_probe_interval_minutes", 0, 32)? {
        key.max_probe_interval_minutes = value as i32;
    }
    if let Some(value) = settings.get("is_active").and_then(Value::as_bool) {
        key.is_active = value;
    }
    if let Some(value) = settings.get("note") {
        key.note = value
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
    }
    if let Some(value) = settings.get("proxy_node_id") {
        key.proxy = if value.is_null() {
            None
        } else {
            admin_pool_key_proxy_value(value.as_str())
        };
    }
    Ok(())
}

pub fn build_admin_pool_batch_action_plan(
    payload: AdminPoolBatchActionRequest,
) -> Result<AdminPoolBatchActionPlan, String> {
    let action = payload.action.trim().to_ascii_lowercase();
    let (action_kind, action_label) = match action.as_str() {
        "enable" => (AdminPoolBatchActionKind::Enable, "enabled"),
        "disable" => (AdminPoolBatchActionKind::Disable, "disabled"),
        "clear_proxy" => (AdminPoolBatchActionKind::ClearProxy, "proxy cleared"),
        "set_proxy" => (AdminPoolBatchActionKind::SetProxy, "proxy set"),
        "update_settings" => (AdminPoolBatchActionKind::UpdateSettings, "settings updated"),
        "regenerate_fingerprint" => (
            AdminPoolBatchActionKind::RegenerateFingerprint,
            "fingerprint regenerated",
        ),
        "delete" => (AdminPoolBatchActionKind::Delete, "deleted"),
        _ => {
            return Err(format!(
                "Invalid action: {action}. Supported locally: enable, disable, clear_proxy, set_proxy, update_settings, regenerate_fingerprint, delete"
            ));
        }
    };

    let key_ids = payload
        .key_ids
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if key_ids.is_empty() {
        return Err("key_ids should not be empty".to_string());
    }

    let action_payload = payload.payload;
    let proxy_payload = if action_kind == AdminPoolBatchActionKind::SetProxy {
        match action_payload.as_ref() {
            Some(Value::Object(map)) if !map.is_empty() => Some(Value::Object(map.clone())),
            _ => {
                return Err(
                    "set_proxy action requires a non-empty payload with proxy config".to_string(),
                );
            }
        }
    } else {
        None
    };
    let settings_payload = if action_kind == AdminPoolBatchActionKind::UpdateSettings {
        let settings = action_payload
            .ok_or_else(|| "update_settings action requires a settings payload".to_string())?;
        validate_admin_pool_key_settings_payload(&settings)?;
        Some(settings)
    } else {
        None
    };

    Ok(AdminPoolBatchActionPlan {
        key_ids,
        action: action_kind,
        action_label,
        proxy_payload,
        settings_payload,
    })
}

pub fn build_admin_pool_batch_action_result_payload(affected: usize, action_label: &str) -> Value {
    json!({
        "affected": affected,
        "message": format!("{affected} keys {action_label}"),
    })
}

pub fn admin_pool_resolved_api_formats(
    endpoints: &[StoredProviderCatalogEndpoint],
    existing_keys: &[StoredProviderCatalogKey],
) -> Vec<String> {
    let mut formats = Vec::new();
    let mut seen = BTreeSet::new();
    for endpoint in endpoints.iter().filter(|endpoint| endpoint.is_active) {
        let api_format = endpoint.api_format.trim();
        if api_format.is_empty() || !seen.insert(api_format.to_string()) {
            continue;
        }
        formats.push(api_format.to_string());
    }
    if !formats.is_empty() {
        return formats;
    }

    for key in existing_keys {
        for api_format in admin_pool_api_formats(key) {
            if !seen.insert(api_format.clone()) {
                continue;
            }
            formats.push(api_format);
        }
    }
    formats
}

pub fn admin_pool_sanitize_quick_selectors(selectors: Vec<String>) -> Vec<String> {
    let mut selectors = selectors
        .into_iter()
        .map(admin_pool_normalize_text)
        .filter(|value| {
            matches!(
                value.as_str(),
                "banned"
                    | "no_5h_limit"
                    | "no_weekly_limit"
                    | "plan_free"
                    | "plan_team"
                    | "oauth_invalid"
                    | "proxy_unset"
                    | "proxy_set"
                    | "disabled"
                    | "enabled"
            )
        })
        .collect::<Vec<_>>();
    selectors.sort();
    selectors.dedup();
    selectors
}

pub fn build_admin_pool_selection_payload(keys: &[StoredProviderCatalogKey]) -> Value {
    let items = keys
        .iter()
        .map(|key| {
            json!({
                "key_id": key.id,
                "key_name": key.name,
                "auth_type": key.auth_type,
            })
        })
        .collect::<Vec<_>>();
    json!({
        "total": items.len(),
        "items": items,
    })
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::{
        admin_pool_key_account_quota_exhausted, admin_pool_key_is_known_banned,
        apply_admin_pool_key_settings, build_admin_pool_batch_action_plan,
        build_admin_pool_batch_import_key_record, build_admin_pool_key_payload,
        resolve_admin_pool_key_settings, validate_admin_pool_key_settings_payload,
        AdminPoolBatchActionKind, AdminPoolBatchActionRequest, AdminPoolKeyPayloadContext,
    };
    use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;
    use serde_json::json;

    fn sample_key(upstream_metadata: Option<serde_json::Value>) -> StoredProviderCatalogKey {
        let mut key = StoredProviderCatalogKey::new(
            "key-1".to_string(),
            "provider-1".to_string(),
            "key-1".to_string(),
            "oauth".to_string(),
            None,
            true,
        )
        .expect("key should build");
        key.upstream_metadata = upstream_metadata;
        key
    }

    #[test]
    fn detects_codex_exhaustion_from_metadata() {
        assert!(admin_pool_key_account_quota_exhausted(
            &sample_key(Some(json!({
                "codex": {
                    "has_credits": false,
                    "credits_unlimited": false
                }
            }))),
            "codex",
        ));
        assert!(admin_pool_key_account_quota_exhausted(
            &sample_key(Some(json!({
                "codex": {
                    "primary_used_percent": 100.0
                }
            }))),
            "codex",
        ));
        assert!(admin_pool_key_account_quota_exhausted(
            &sample_key(Some(json!({
                "codex": {
                    "secondary_used_percent": 100.0
                }
            }))),
            "codex",
        ));
        assert!(!admin_pool_key_account_quota_exhausted(
            &sample_key(Some(json!({
                "codex": {
                    "has_credits": false,
                    "credits_unlimited": false,
                    "primary_used_percent": 64.0,
                    "secondary_used_percent": 3.0
                }
            }))),
            "codex",
        ));
        assert!(!admin_pool_key_account_quota_exhausted(
            &sample_key(Some(json!({
                "codex": {
                    "has_credits": false,
                    "credits_unlimited": true
                }
            }))),
            "codex",
        ));
        assert!(!admin_pool_key_account_quota_exhausted(
            &sample_key(None),
            "codex",
        ));
    }

    #[test]
    fn prefers_quota_snapshot_over_metadata_for_codex_exhaustion() {
        let mut key = sample_key(Some(json!({
            "codex": {
                "secondary_used_percent": 100.0
            }
        })));
        key.status_snapshot = Some(json!({
            "quota": {
                "version": 2,
                "provider_type": "codex",
                "code": "ok",
                "exhausted": false,
                "usage_ratio": 0.0,
                "updated_at": 1_776_395_200u64,
                "windows": [
                    {
                        "code": "weekly",
                        "used_ratio": 0.0,
                        "remaining_ratio": 1.0
                    },
                    {
                        "code": "5h",
                        "used_ratio": 0.0,
                        "remaining_ratio": 1.0
                    }
                ]
            }
        }));

        assert!(!admin_pool_key_account_quota_exhausted(&key, "codex"));
    }

    #[test]
    fn clears_stale_codex_exhausted_snapshot_when_windows_have_capacity() {
        let mut key = sample_key(Some(json!({
            "codex": {
                "has_credits": false,
                "primary_used_percent": 100.0
            }
        })));
        key.status_snapshot = Some(json!({
            "quota": {
                "version": 2,
                "provider_type": "codex",
                "code": "exhausted",
                "exhausted": true,
                "usage_ratio": 0.0,
                "updated_at": 1_776_395_200u64,
                "windows": [
                    {
                        "code": "weekly",
                        "used_ratio": 0.0,
                        "remaining_ratio": 1.0
                    },
                    {
                        "code": "5h",
                        "used_ratio": 0.0,
                        "remaining_ratio": 1.0
                    }
                ]
            }
        }));

        assert!(!admin_pool_key_account_quota_exhausted(&key, "codex"));
    }

    #[test]
    fn detects_kiro_exhaustion_from_metadata() {
        assert!(admin_pool_key_account_quota_exhausted(
            &sample_key(Some(json!({
                "kiro": {
                    "remaining": 0
                }
            }))),
            "kiro",
        ));
        assert!(admin_pool_key_account_quota_exhausted(
            &sample_key(Some(json!({
                "kiro": {
                    "usage_percentage": 100.0
                }
            }))),
            "kiro",
        ));
        assert!(admin_pool_key_account_quota_exhausted(
            &sample_key(Some(json!({
                "kiro": {
                    "usage_limit": 100.0,
                    "current_usage": 100.0
                }
            }))),
            "kiro",
        ));
        assert!(!admin_pool_key_account_quota_exhausted(
            &sample_key(None),
            "kiro",
        ));
    }

    #[test]
    fn known_banned_detects_provider_bucket_account_blocks_without_provider_type() {
        let key = sample_key(Some(json!({
            "codex": {
                "account_disabled": true,
                "reason": "deactivated_workspace"
            }
        })));

        assert!(admin_pool_key_is_known_banned(&key));
    }

    #[test]
    fn build_admin_pool_key_payload_ignores_health_for_scheduling() {
        let mut key = sample_key(None);
        key.health_by_format = Some(json!({
            "openai:chat": {
                "health_score": 0.2
            }
        }));
        key.circuit_breaker_by_format = Some(json!({
            "openai:chat": {
                "open": true
            }
        }));

        let payload = build_admin_pool_key_payload(&key, &AdminPoolKeyPayloadContext::default());

        assert_eq!(payload["health_score"], json!(0.2));
        assert_eq!(payload["circuit_breaker_open"], json!(false));
        assert_eq!(payload["scheduling_status"], json!("available"));
        assert_eq!(payload["scheduling_reason"], json!("available"));
        assert_eq!(payload["scheduling_label"], json!("可用"));
    }

    #[test]
    fn validates_and_applies_shared_key_settings() {
        let settings = json!({
            "internal_priority": 12,
            "rpm_limit": 600,
            "concurrent_limit": 8,
            "cache_ttl_minutes": 10,
            "max_probe_interval_minutes": 16,
            "is_active": false,
            "note": " imported ",
            "proxy_node_id": "proxy-1"
        });
        let mut key = sample_key(None);

        validate_admin_pool_key_settings_payload(&settings).expect("settings should validate");
        apply_admin_pool_key_settings(&mut key, &settings).expect("settings should apply");

        assert_eq!(key.internal_priority, 12);
        assert_eq!(key.rpm_limit, Some(600));
        assert_eq!(key.concurrent_limit, Some(8));
        assert_eq!(key.cache_ttl_minutes, 10);
        assert_eq!(key.max_probe_interval_minutes, 16);
        assert!(!key.is_active);
        assert_eq!(key.note.as_deref(), Some("imported"));
        assert_eq!(
            key.proxy,
            Some(json!({ "node_id": "proxy-1", "enabled": true }))
        );

        assert!(validate_admin_pool_key_settings_payload(&json!({ "rpm_limit": 0 })).is_err());
        assert!(validate_admin_pool_key_settings_payload(&json!({ "unknown": true })).is_err());
    }

    #[test]
    fn builds_update_settings_action_plan() {
        let settings = json!({ "rpm_limit": null, "proxy_node_id": "proxy-1" });
        let plan = build_admin_pool_batch_action_plan(AdminPoolBatchActionRequest {
            key_ids: vec![
                "key-2".to_string(),
                "key-1".to_string(),
                "key-1".to_string(),
            ],
            action: "update_settings".to_string(),
            payload: Some(settings.clone()),
        })
        .expect("action plan should build");

        assert_eq!(plan.action, AdminPoolBatchActionKind::UpdateSettings);
        assert_eq!(plan.key_ids, vec!["key-1", "key-2"]);
        assert_eq!(plan.settings_payload, Some(settings));
        assert!(plan.proxy_payload.is_none());
    }

    #[test]
    fn batch_import_record_applies_shared_settings() {
        let record = build_admin_pool_batch_import_key_record(
            "key-1".to_string(),
            "provider-1".to_string(),
            "1".to_string(),
            "api_key".to_string(),
            vec!["openai:chat".to_string()],
            "encrypted".to_string(),
            None,
            Some(&json!({
                "rpm_limit": 900,
                "cache_ttl_minutes": 0,
                "max_probe_interval_minutes": 4,
                "is_active": false
            })),
            1_700_000_000,
        )
        .expect("record should build");

        assert_eq!(record.name, "1");
        assert_eq!(record.rpm_limit, Some(900));
        assert_eq!(record.cache_ttl_minutes, 0);
        assert_eq!(record.max_probe_interval_minutes, 4);
        assert!(!record.is_active);
    }

    #[test]
    fn batch_import_item_settings_override_shared_values() {
        let resolved = resolve_admin_pool_key_settings(
            Some(&json!({
                "rpm_limit": 900,
                "cache_ttl_minutes": 5,
                "is_active": true
            })),
            Some(&json!({
                "rpm_limit": 120,
                "is_active": false
            })),
        )
        .expect("item settings should override shared values")
        .expect("resolved settings should exist");

        assert_eq!(resolved["rpm_limit"], 120);
        assert_eq!(resolved["cache_ttl_minutes"], 5);
        assert_eq!(resolved["is_active"], false);
        assert!(resolve_admin_pool_key_settings(None, Some(&json!([]))).is_err());
    }
}

pub fn build_admin_pool_key_payload(
    key: &StoredProviderCatalogKey,
    context: &AdminPoolKeyPayloadContext,
) -> Value {
    let health_score = admin_pool_health_score(key);
    let circuit_breaker_open = false;
    let (scheduling_status, scheduling_reason, scheduling_label, scheduling_reasons) =
        admin_pool_scheduling_payload(
            key,
            context.cooldown_reason.as_deref(),
            context.cooldown_ttl_seconds,
        );

    json!({
        "key_id": key.id,
        "key_name": key.name,
        "is_active": key.is_active,
        "auth_type": key.auth_type,
        "status_snapshot": key.status_snapshot.clone().unwrap_or_else(|| json!({})),
        "health_score": health_score,
        "circuit_breaker_open": circuit_breaker_open,
        "api_formats": admin_pool_api_formats(key),
        "rate_multipliers": admin_pool_json_object(key.rate_multipliers.as_ref()),
        "internal_priority": key.internal_priority,
        "rpm_limit": key.rpm_limit,
        "cache_ttl_minutes": key.cache_ttl_minutes,
        "max_probe_interval_minutes": key.max_probe_interval_minutes,
        "note": key.note,
        "allowed_models": admin_pool_string_list(key.allowed_models.as_ref()),
        "capabilities": admin_pool_json_object(key.capabilities.as_ref()),
        "auto_fetch_models": key.auto_fetch_models,
        "locked_models": admin_pool_string_list(key.locked_models.as_ref()),
        "model_include_patterns": admin_pool_string_list(key.model_include_patterns.as_ref()),
        "model_exclude_patterns": admin_pool_string_list(key.model_exclude_patterns.as_ref()),
        "proxy": key.proxy.clone(),
        "fingerprint": key.fingerprint.clone(),
        "cooldown_reason": context.cooldown_reason,
        "cooldown_ttl_seconds": context.cooldown_ttl_seconds,
        "cost_window_usage": context.cost_window_usage,
        "cost_limit": context.cost_limit,
        "request_count": key.request_count.unwrap_or(0),
        "total_tokens": 0,
        "total_cost_usd": "0.00000000",
        "sticky_sessions": context.sticky_sessions,
        "lru_score": context.lru_score,
        "created_at": key.created_at_unix_ms.and_then(unix_secs_to_rfc3339),
        "last_used_at": key.last_used_at_unix_secs.and_then(unix_secs_to_rfc3339),
        "scheduling_status": scheduling_status,
        "scheduling_reason": scheduling_reason,
        "scheduling_label": scheduling_label,
        "scheduling_reasons": scheduling_reasons,
    })
}

pub fn build_admin_pool_scheduling_presets_payload() -> Value {
    aether_provider_pool::build_admin_pool_scheduling_presets_payload()
}

pub fn admin_pool_batch_delete_task_parts(request_path: &str) -> Option<(String, String)> {
    let raw = request_path.strip_prefix("/api/admin/pool/")?;
    let (provider_id, suffix) = raw.split_once("/keys/batch-delete-task/")?;
    let provider_id = provider_id.trim();
    let task_id = suffix.trim().trim_matches('/');
    if provider_id.is_empty()
        || provider_id.contains('/')
        || task_id.is_empty()
        || task_id.contains('/')
    {
        return None;
    }
    Some((provider_id.to_string(), task_id.to_string()))
}

#[allow(clippy::too_many_arguments)]
pub fn build_admin_pool_batch_delete_task_payload(
    task_id: &str,
    provider_id: &str,
    status: &str,
    stage: &str,
    total_keys: usize,
    deleted_keys: usize,
    total_endpoints: usize,
    deleted_endpoints: usize,
    message: &str,
) -> Value {
    json!({
        "task_id": task_id,
        "provider_id": provider_id,
        "status": status,
        "stage": stage,
        "total_keys": total_keys,
        "deleted_keys": deleted_keys,
        "total_endpoints": total_endpoints,
        "deleted_endpoints": deleted_endpoints,
        "message": message,
    })
}

pub fn build_admin_pool_overview_payload(
    providers: &[StoredProviderCatalogProvider],
    key_stats_by_provider: &BTreeMap<String, StoredProviderCatalogKeyStats>,
    cooldown_counts_by_provider: &BTreeMap<String, usize>,
) -> Value {
    let items = providers
        .iter()
        .map(|provider| {
            let stats = key_stats_by_provider.get(&provider.id);
            let total_keys = stats.map(|item| item.total_keys as usize).unwrap_or(0);
            let active_keys = stats.map(|item| item.active_keys as usize).unwrap_or(0);
            let cooldown_count = cooldown_counts_by_provider
                .get(&provider.id)
                .copied()
                .unwrap_or(0);
            json!({
                "provider_id": provider.id,
                "provider_name": provider.name,
                "provider_type": provider.provider_type,
                "total_keys": total_keys,
                "active_keys": active_keys,
                "cooldown_count": cooldown_count,
                "pool_enabled": true,
            })
        })
        .collect::<Vec<_>>();
    json!({ "items": items })
}

pub fn resolve_admin_pool_key_settings(
    shared: Option<&Value>,
    overrides: Option<&Value>,
) -> Result<Option<Value>, String> {
    let mut resolved = Map::new();
    for settings in [shared, overrides].into_iter().flatten() {
        let Value::Object(values) = settings else {
            return Err("settings payload must be an object".to_string());
        };
        resolved.extend(values.clone());
    }
    if resolved.is_empty() {
        return Ok(None);
    }
    let resolved = Value::Object(resolved);
    validate_admin_pool_key_settings_payload(&resolved)?;
    Ok(Some(resolved))
}

#[allow(clippy::too_many_arguments)]
pub fn build_admin_pool_batch_import_key_record(
    id: String,
    provider_id: String,
    name: String,
    auth_type: String,
    api_formats: Vec<String>,
    encrypted_api_key: String,
    proxy: Option<Value>,
    settings: Option<&Value>,
    now_unix_secs: u64,
) -> Result<StoredProviderCatalogKey, String> {
    let mut record = StoredProviderCatalogKey::new(id, provider_id, name, auth_type, None, true)
        .map_err(|err| err.to_string())?;
    record = record
        .with_transport_fields(
            Some(json!(api_formats)),
            encrypted_api_key,
            None,
            None,
            None,
            None,
            None,
            proxy,
            None,
        )
        .map_err(|err| err.to_string())?;
    record.request_count = Some(0);
    record.success_count = Some(0);
    record.error_count = Some(0);
    record.total_response_time_ms = Some(0);
    record.health_by_format = Some(json!({}));
    record.circuit_breaker_by_format = Some(json!({}));
    if let Some(settings) = settings {
        apply_admin_pool_key_settings(&mut record, settings)?;
    }
    record.created_at_unix_ms = Some(now_unix_secs);
    record.updated_at_unix_secs = Some(now_unix_secs);
    Ok(record)
}

pub fn build_admin_pool_batch_import_result_payload(
    imported: usize,
    skipped: usize,
    errors: Vec<Value>,
) -> Value {
    json!({
        "imported": imported,
        "skipped": skipped,
        "errors": errors,
    })
}

pub fn build_admin_pool_cleanup_empty_payload(message: &str) -> Value {
    json!({
        "affected": 0,
        "message": message,
    })
}

pub fn build_admin_pool_cleanup_result_payload(affected: usize) -> Value {
    json!({
        "affected": affected,
        "message": format!("已清理 {affected} 个异常账号"),
    })
}
