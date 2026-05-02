use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogKeyStats,
    StoredProviderCatalogProvider,
};
use chrono::{TimeZone, Utc};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

use super::status as provider_status;

#[derive(Debug, Default, Clone, serde::Deserialize)]
pub struct AdminPoolResolveSelectionRequest {
    #[serde(default)]
    pub search: String,
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
    RegenerateFingerprint,
    Delete,
}

#[derive(Debug, Clone)]
pub struct AdminPoolBatchActionPlan {
    pub key_ids: Vec<String>,
    pub action: AdminPoolBatchActionKind,
    pub action_label: &'static str,
    pub proxy_payload: Option<Value>,
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
}

#[derive(Debug, Default, Clone, serde::Deserialize)]
pub struct AdminPoolBatchImportItem {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub auth_type: String,
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

fn admin_pool_metadata_bucket<'a>(
    upstream_metadata: Option<&'a Value>,
    provider_type: &str,
) -> Option<&'a serde_json::Map<String, Value>> {
    upstream_metadata
        .and_then(Value::as_object)
        .and_then(|metadata| metadata.get(&provider_type.trim().to_ascii_lowercase()))
        .and_then(Value::as_object)
}

fn admin_pool_json_bool(value: Option<&Value>) -> Option<bool> {
    match value {
        Some(Value::Bool(value)) => Some(*value),
        Some(Value::String(value)) => match value.trim().to_ascii_lowercase().as_str() {
            "true" | "1" => Some(true),
            "false" | "0" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

fn admin_pool_json_f64(value: Option<&Value>) -> Option<f64> {
    match value {
        Some(Value::Number(number)) => number.as_f64(),
        Some(Value::String(value)) => value.trim().parse::<f64>().ok(),
        _ => None,
    }
    .filter(|value| value.is_finite())
}

fn admin_pool_quota_snapshot_matches_provider(
    quota_snapshot: &serde_json::Map<String, Value>,
    provider_type: &str,
) -> bool {
    let normalized_provider_type = provider_type.trim().to_ascii_lowercase();
    match quota_snapshot
        .get("provider_type")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(provider_type) => provider_type.eq_ignore_ascii_case(&normalized_provider_type),
        None => {
            admin_pool_json_bool(quota_snapshot.get("exhausted")) == Some(true)
                || quota_snapshot
                    .get("code")
                    .and_then(Value::as_str)
                    .is_some_and(|code| !code.trim().eq_ignore_ascii_case("unknown"))
                || quota_snapshot
                    .get("updated_at")
                    .is_some_and(|value| !value.is_null())
                || quota_snapshot
                    .get("observed_at")
                    .is_some_and(|value| !value.is_null())
                || quota_snapshot
                    .get("usage_ratio")
                    .is_some_and(|value| !value.is_null())
                || quota_snapshot
                    .get("reset_seconds")
                    .is_some_and(|value| !value.is_null())
                || quota_snapshot
                    .get("windows")
                    .and_then(Value::as_array)
                    .is_some_and(|windows| !windows.is_empty())
                || quota_snapshot
                    .get("credits")
                    .and_then(Value::as_object)
                    .is_some_and(|credits| !credits.is_empty())
        }
    }
}

fn admin_pool_key_quota_snapshot<'a>(
    key: &'a StoredProviderCatalogKey,
    provider_type: &str,
) -> Option<&'a serde_json::Map<String, Value>> {
    let quota_snapshot = key
        .status_snapshot
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|snapshot| snapshot.get("quota"))
        .and_then(Value::as_object)?;
    admin_pool_quota_snapshot_matches_provider(quota_snapshot, provider_type)
        .then_some(quota_snapshot)
}

pub fn admin_pool_key_account_quota_exhausted(
    key: &StoredProviderCatalogKey,
    provider_type: &str,
) -> bool {
    if let Some(exhausted) =
        admin_pool_key_quota_snapshot(key, provider_type).and_then(|quota_snapshot| {
            let exhausted = admin_pool_json_bool(quota_snapshot.get("exhausted"))?;
            if exhausted {
                let windows_max_ratio = quota_snapshot
                    .get("windows")
                    .and_then(Value::as_array)
                    .filter(|w| !w.is_empty())
                    .and_then(|windows| {
                        windows
                            .iter()
                            .filter_map(Value::as_object)
                            .filter_map(|w| w.get("used_ratio"))
                            .filter_map(Value::as_f64)
                            .max_by(f64::total_cmp)
                    });
                if windows_max_ratio.is_some_and(|ratio| ratio < 1.0 - 1e-6) {
                    return Some(false);
                }
            }
            Some(exhausted)
        })
    {
        return exhausted;
    }

    let provider_type = provider_type.trim().to_ascii_lowercase();
    let Some(bucket) = admin_pool_metadata_bucket(key.upstream_metadata.as_ref(), &provider_type)
    else {
        return false;
    };

    match provider_type.as_str() {
        "codex" => {
            if admin_pool_json_bool(bucket.get("credits_unlimited")) == Some(true) {
                return false;
            }
            let has_window_data = admin_pool_json_f64(bucket.get("primary_used_percent")).is_some()
                || admin_pool_json_f64(bucket.get("secondary_used_percent")).is_some();
            if !has_window_data && admin_pool_json_bool(bucket.get("has_credits")) == Some(false) {
                return true;
            }
            admin_pool_json_f64(bucket.get("primary_used_percent"))
                .is_some_and(|value| value >= 100.0)
                || admin_pool_json_f64(bucket.get("secondary_used_percent"))
                    .is_some_and(|value| value >= 100.0)
        }
        "kiro" => {
            if admin_pool_json_f64(bucket.get("remaining")).is_some_and(|value| value <= 0.0) {
                return true;
            }
            if admin_pool_json_f64(bucket.get("usage_percentage"))
                .is_some_and(|value| value >= 100.0)
            {
                return true;
            }
            match (
                admin_pool_json_f64(bucket.get("usage_limit")),
                admin_pool_json_f64(bucket.get("current_usage")),
            ) {
                (Some(limit), Some(current)) if limit > 0.0 => current >= limit,
                _ => false,
            }
        }
        _ => false,
    }
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

fn admin_pool_circuit_breaker_open(key: &StoredProviderCatalogKey) -> bool {
    key.circuit_breaker_by_format
        .as_ref()
        .and_then(Value::as_object)
        .map(|formats| {
            formats
                .values()
                .filter_map(Value::as_object)
                .any(|item| item.get("open").and_then(Value::as_bool).unwrap_or(false))
        })
        .unwrap_or(false)
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
    if key
        .oauth_invalid_reason
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
    {
        return true;
    }
    key.expires_at_unix_secs
        .is_some_and(|value| value > 0 && value <= now_unix_secs)
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

pub fn build_admin_pool_batch_action_plan(
    payload: AdminPoolBatchActionRequest,
) -> Result<AdminPoolBatchActionPlan, String> {
    let action = payload.action.trim().to_ascii_lowercase();
    let (action_kind, action_label) = match action.as_str() {
        "enable" => (AdminPoolBatchActionKind::Enable, "enabled"),
        "disable" => (AdminPoolBatchActionKind::Disable, "disabled"),
        "clear_proxy" => (AdminPoolBatchActionKind::ClearProxy, "proxy cleared"),
        "set_proxy" => (AdminPoolBatchActionKind::SetProxy, "proxy set"),
        "regenerate_fingerprint" => (
            AdminPoolBatchActionKind::RegenerateFingerprint,
            "fingerprint regenerated",
        ),
        "delete" => (AdminPoolBatchActionKind::Delete, "deleted"),
        _ => {
            return Err(format!(
                "Invalid action: {action}. Supported locally: enable, disable, clear_proxy, set_proxy, regenerate_fingerprint, delete"
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

    let proxy_payload = if action_kind == AdminPoolBatchActionKind::SetProxy {
        match payload.payload {
            Some(Value::Object(map)) if !map.is_empty() => Some(Value::Object(map)),
            _ => {
                return Err(
                    "set_proxy action requires a non-empty payload with proxy config".to_string(),
                );
            }
        }
    } else {
        None
    };

    Ok(AdminPoolBatchActionPlan {
        key_ids,
        action: action_kind,
        action_label,
        proxy_payload,
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
        build_admin_pool_key_payload, AdminPoolKeyPayloadContext,
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
        assert_eq!(payload["circuit_breaker_open"], json!(true));
        assert_eq!(payload["scheduling_status"], json!("available"));
        assert_eq!(payload["scheduling_reason"], json!("available"));
        assert_eq!(payload["scheduling_label"], json!("可用"));
    }
}

pub fn build_admin_pool_key_payload(
    key: &StoredProviderCatalogKey,
    context: &AdminPoolKeyPayloadContext,
) -> Value {
    let health_score = admin_pool_health_score(key);
    let circuit_breaker_open = admin_pool_circuit_breaker_open(key);
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
    json!([
        {
            "name": "lru",
            "label": "LRU 轮转",
            "description": "最久未使用的 Key 优先",
            "providers": [],
            "modes": Value::Null,
            "default_mode": Value::Null,
            "mutex_group": "distribution_mode",
            "evidence_hint": "依据 LRU 时间戳（最近未使用优先）",
        },
        {
            "name": "cache_affinity",
            "label": "缓存亲和",
            "description": "优先复用最近使用过的 Key，利用 Prompt Caching",
            "providers": [],
            "modes": Value::Null,
            "default_mode": Value::Null,
            "mutex_group": "distribution_mode",
            "evidence_hint": "依据 LRU 时间戳（最近使用优先，与 LRU 轮转相反）",
        },
        {
            "name": "cost_first",
            "label": "成本优先",
            "description": "优先选择窗口消耗更低的账号",
            "providers": [],
            "modes": Value::Null,
            "default_mode": Value::Null,
            "mutex_group": Value::Null,
            "evidence_hint": "依据窗口成本/Token 用量，缺失时回退配额使用率",
        },
        {
            "name": "free_first",
            "label": "Free 优先",
            "description": "优先消耗 Free 账号（依赖 plan_type）",
            "providers": ["codex", "kiro"],
            "modes": Value::Null,
            "default_mode": Value::Null,
            "mutex_group": Value::Null,
            "evidence_hint": "依据 plan_type（Free 账号优先调度）",
        },
        {
            "name": "health_first",
            "label": "健康优先",
            "description": "优先选择健康分更高、失败更少的账号",
            "providers": [],
            "modes": Value::Null,
            "default_mode": Value::Null,
            "mutex_group": Value::Null,
            "evidence_hint": "依据 health_by_format 聚合分（含熔断/失败衰减）",
        },
        {
            "name": "latency_first",
            "label": "延迟优先",
            "description": "优先选择最近延迟更低的账号",
            "providers": [],
            "modes": Value::Null,
            "default_mode": Value::Null,
            "mutex_group": Value::Null,
            "evidence_hint": "依据号池延迟窗口均值（latency_window_seconds）",
        },
        {
            "name": "load_balance",
            "label": "负载均衡",
            "description": "随机分散 Key 使用，均匀分摊负载",
            "providers": [],
            "modes": Value::Null,
            "default_mode": Value::Null,
            "mutex_group": "distribution_mode",
            "evidence_hint": "每次随机分值，实现完全均匀分散",
        },
        {
            "name": "plus_first",
            "label": "Plus 优先",
            "description": "优先消耗 Plus 账号（依赖 plan_type）",
            "providers": ["codex", "kiro"],
            "modes": Value::Null,
            "default_mode": Value::Null,
            "mutex_group": Value::Null,
            "evidence_hint": "依据 plan_type（Plus 账号优先调度）",
        },
        {
            "name": "pro_first",
            "label": "Pro 优先",
            "description": "优先消耗 Pro 账号（依赖 plan_type）",
            "providers": ["codex", "kiro"],
            "modes": Value::Null,
            "default_mode": Value::Null,
            "mutex_group": Value::Null,
            "evidence_hint": "依据 plan_type（Pro 账号优先调度）",
        },
        {
            "name": "priority_first",
            "label": "优先级优先",
            "description": "按账号优先级顺序调度（数字越小越优先）",
            "providers": [],
            "modes": Value::Null,
            "default_mode": Value::Null,
            "mutex_group": Value::Null,
            "evidence_hint": "依据 internal_priority（支持拖拽/手工编辑）",
        },
        {
            "name": "quota_balanced",
            "label": "额度平均",
            "description": "优先选额度消耗最少的账号",
            "providers": [],
            "modes": Value::Null,
            "default_mode": Value::Null,
            "mutex_group": Value::Null,
            "evidence_hint": "依据账号配额使用率；无配额时回退到窗口成本使用",
        },
        {
            "name": "recent_refresh",
            "label": "额度刷新优先",
            "description": "优先选即将刷新额度的账号",
            "providers": ["codex", "kiro"],
            "modes": Value::Null,
            "default_mode": Value::Null,
            "mutex_group": Value::Null,
            "evidence_hint": "依据账号额度重置倒计时（next_reset / reset_seconds）",
        },
        {
            "name": "single_account",
            "label": "单号优先",
            "description": "集中使用同一账号（反向 LRU）",
            "providers": [],
            "modes": Value::Null,
            "default_mode": Value::Null,
            "mutex_group": "distribution_mode",
            "evidence_hint": "先按账号优先级（internal_priority），同级再按反向 LRU 集中",
        },
        {
            "name": "team_first",
            "label": "Team 优先",
            "description": "优先消耗 Team 账号（依赖 plan_type）",
            "providers": ["codex", "kiro"],
            "modes": Value::Null,
            "default_mode": Value::Null,
            "mutex_group": Value::Null,
            "evidence_hint": "依据 plan_type（Team 账号优先调度）",
        }
    ])
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

#[allow(clippy::too_many_arguments)]
pub fn build_admin_pool_batch_import_key_record(
    id: String,
    provider_id: String,
    name: String,
    auth_type: String,
    api_formats: Vec<String>,
    encrypted_api_key: String,
    proxy: Option<Value>,
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
