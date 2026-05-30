use crate::handlers::shared::{json_string_list, unix_secs_to_rfc3339};
use crate::provider_key_auth::{
    provider_key_auth_semantics, provider_key_can_refresh_oauth,
    provider_key_configured_api_formats, provider_key_inherits_provider_api_formats,
};
use crate::AppState;
use aether_admin::provider::quota as admin_provider_quota_pure;
use aether_admin::provider::status as admin_provider_status_pure;
#[cfg(test)]
use aether_crypto::DEVELOPMENT_ENCRYPTION_KEY;
use aether_crypto::{decrypt_python_fernet_ciphertext, encrypt_python_fernet_plaintext};
use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;
use aether_provider_pool::{
    grok_pool_tier_from_quota_bucket, grok_supported_quota_windows_for_tier,
};
use aether_scheduler_core::provider_key_circuit_payload_is_active_open_at;
use serde_json::{json, Map, Value};
use std::borrow::Cow;
use std::time::{SystemTime, UNIX_EPOCH};

const OAUTH_ACCOUNT_BLOCK_PREFIX: &str = "[ACCOUNT_BLOCK] ";
const OAUTH_EXPIRED_PREFIX: &str = "[OAUTH_EXPIRED] ";
const OAUTH_REFRESH_FAILED_PREFIX: &str = "[REFRESH_FAILED] ";
const OAUTH_REQUEST_FAILED_PREFIX: &str = "[REQUEST_FAILED] ";

pub(crate) fn provider_catalog_key_supports_format(
    key: &StoredProviderCatalogKey,
    provider_type: &str,
    api_format: &str,
) -> bool {
    if provider_key_inherits_provider_api_formats(key, provider_type) {
        return true;
    }
    let formats = provider_key_configured_api_formats(key);
    if formats.is_empty() {
        return true;
    }
    formats
        .iter()
        .any(|candidate| crate::ai_serving::api_format_alias_matches(candidate, api_format))
}

pub(crate) fn decrypt_catalog_secret_with_fallbacks(
    encryption_key: Option<&str>,
    ciphertext: &str,
) -> Option<String> {
    let encryption_key = encryption_key.map(str::trim).unwrap_or("");
    if !encryption_key.is_empty() {
        if let Ok(value) = decrypt_python_fernet_ciphertext(encryption_key, ciphertext) {
            return Some(value);
        }
    }
    for env_key in ["AETHER_GATEWAY_DATA_ENCRYPTION_KEY", "ENCRYPTION_KEY"] {
        let Ok(fallback) = std::env::var(env_key) else {
            continue;
        };
        let fallback = fallback.trim();
        if fallback.is_empty() || fallback == encryption_key {
            continue;
        }
        if let Ok(value) = decrypt_python_fernet_ciphertext(fallback, ciphertext) {
            return Some(value);
        }
    }
    #[cfg(test)]
    if encryption_key != DEVELOPMENT_ENCRYPTION_KEY {
        if let Ok(value) = decrypt_python_fernet_ciphertext(DEVELOPMENT_ENCRYPTION_KEY, ciphertext)
        {
            return Some(value);
        }
    }
    None
}

pub(crate) fn effective_catalog_encryption_key(state: &AppState) -> Option<Cow<'_, str>> {
    let encryption_key = state.encryption_key().map(str::trim).unwrap_or("");
    if !encryption_key.is_empty() {
        return Some(Cow::Borrowed(encryption_key));
    }
    for env_key in ["AETHER_GATEWAY_DATA_ENCRYPTION_KEY", "ENCRYPTION_KEY"] {
        let Ok(candidate) = std::env::var(env_key) else {
            continue;
        };
        let trimmed = candidate.trim();
        if !trimmed.is_empty() {
            return Some(if trimmed.len() == candidate.len() {
                Cow::Owned(candidate)
            } else {
                Cow::Owned(trimmed.to_string())
            });
        }
    }
    #[cfg(test)]
    {
        return Some(Cow::Borrowed(DEVELOPMENT_ENCRYPTION_KEY));
    }
    #[allow(unreachable_code)]
    None
}

pub(crate) fn encrypt_catalog_secret_with_fallbacks(
    state: &AppState,
    plaintext: &str,
) -> Option<String> {
    let encryption_key = effective_catalog_encryption_key(state)?;
    encrypt_python_fernet_plaintext(encryption_key.as_ref(), plaintext).ok()
}

pub(crate) fn take_secret_prefix(value: &str, prefix_chars: usize) -> &str {
    let end = value
        .char_indices()
        .nth(prefix_chars)
        .map(|(index, _)| index)
        .unwrap_or(value.len());
    &value[..end]
}

pub(crate) fn take_secret_suffix(value: &str, suffix_chars: usize) -> &str {
    if suffix_chars == 0 {
        return &value[value.len()..];
    }

    let start = value
        .char_indices()
        .rev()
        .nth(suffix_chars - 1)
        .map(|(index, _)| index)
        .unwrap_or(0);
    &value[start..]
}

pub(crate) fn masked_catalog_api_key(state: &AppState, key: &StoredProviderCatalogKey) -> String {
    match key.auth_type.trim() {
        "service_account" | "vertex_ai" => "[Service Account]".to_string(),
        "oauth" => "[OAuth Token]".to_string(),
        _ => {
            let Some(ciphertext) = key
                .encrypted_api_key
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                return "[未设置]".to_string();
            };
            decrypt_catalog_secret_with_fallbacks(state.encryption_key(), ciphertext)
                .map(|value| {
                    if value.chars().count() <= 12 {
                        format!("{value}***")
                    } else {
                        format!(
                            "{}***{}",
                            take_secret_prefix(&value, 8),
                            take_secret_suffix(&value, 4)
                        )
                    }
                })
                .unwrap_or_else(|| "***ERROR***".to_string())
        }
    }
}

pub(crate) fn parse_catalog_auth_config_json(
    state: &AppState,
    key: &StoredProviderCatalogKey,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    let ciphertext = key.encrypted_auth_config.as_deref()?.trim();
    if ciphertext.is_empty() {
        return None;
    }
    let plaintext = decrypt_catalog_secret_with_fallbacks(state.encryption_key(), ciphertext)?;
    serde_json::from_str::<serde_json::Value>(&plaintext)
        .ok()?
        .as_object()
        .cloned()
}

pub(crate) fn default_provider_key_status_snapshot() -> serde_json::Value {
    json!({
        "oauth": {
            "code": "none",
            "label": serde_json::Value::Null,
            "reason": serde_json::Value::Null,
            "expires_at": serde_json::Value::Null,
            "invalid_at": serde_json::Value::Null,
            "source": serde_json::Value::Null,
            "requires_reauth": false,
            "expiring_soon": false,
        },
        "account": {
            "code": "ok",
            "label": serde_json::Value::Null,
            "reason": serde_json::Value::Null,
            "blocked": false,
            "source": serde_json::Value::Null,
            "recoverable": false,
        },
        "quota": {
            "code": "unknown",
            "label": serde_json::Value::Null,
            "reason": serde_json::Value::Null,
            "exhausted": false,
            "usage_ratio": serde_json::Value::Null,
            "updated_at": serde_json::Value::Null,
            "reset_seconds": serde_json::Value::Null,
            "plan_type": serde_json::Value::Null,
        }
    })
}

fn default_oauth_status_snapshot_value() -> Value {
    default_provider_key_status_snapshot()
        .get("oauth")
        .cloned()
        .unwrap_or_else(|| {
            json!({
                "code": "none",
                "label": Value::Null,
                "reason": Value::Null,
                "expires_at": Value::Null,
                "invalid_at": Value::Null,
                "source": Value::Null,
                "requires_reauth": false,
                "expiring_soon": false,
            })
        })
}

fn trimmed_oauth_invalid_reason(reason: Option<&str>) -> Option<String> {
    reason
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn tagged_oauth_invalid_reason(reason: Option<&str>, prefix: &str) -> Option<String> {
    reason.and_then(|value| {
        value
            .lines()
            .map(str::trim)
            .find_map(|line| line.strip_prefix(prefix))
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    })
}

fn oauth_access_token_expired(expires_at_unix_secs: Option<u64>, now_unix_secs: u64) -> bool {
    expires_at_unix_secs.is_none_or(|expires_at| expires_at == 0 || expires_at <= now_unix_secs)
}

fn build_provider_key_oauth_status_snapshot(key: &StoredProviderCatalogKey) -> Value {
    if !key.auth_type.trim().eq_ignore_ascii_case("oauth") {
        return default_oauth_status_snapshot_value();
    }

    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let expires_at_unix_secs = key.expires_at_unix_secs;
    let invalid_at_unix_secs = key.oauth_invalid_at_unix_secs;
    let invalid_reason = trimmed_oauth_invalid_reason(key.oauth_invalid_reason.as_deref());

    if let Some(reason) =
        tagged_oauth_invalid_reason(invalid_reason.as_deref(), OAUTH_EXPIRED_PREFIX)
    {
        return json!({
            "code": "invalid",
            "label": "已失效",
            "reason": reason,
            "expires_at": expires_at_unix_secs,
            "invalid_at": invalid_at_unix_secs,
            "source": "oauth_invalid",
            "requires_reauth": true,
            "expiring_soon": false,
        });
    }
    if let Some(reason) =
        tagged_oauth_invalid_reason(invalid_reason.as_deref(), OAUTH_REFRESH_FAILED_PREFIX)
    {
        let access_token_expired = oauth_access_token_expired(expires_at_unix_secs, now_unix_secs);
        return json!({
            "code": if access_token_expired { "invalid" } else { "reauth_required" },
            "label": if access_token_expired { "已失效" } else { "续期失败" },
            "reason": reason,
            "expires_at": expires_at_unix_secs,
            "invalid_at": invalid_at_unix_secs,
            "source": "oauth_refresh",
            "requires_reauth": true,
            "usable_until_expiry": !access_token_expired,
            "expiring_soon": false,
        });
    }
    if let Some(reason) =
        tagged_oauth_invalid_reason(invalid_reason.as_deref(), OAUTH_REQUEST_FAILED_PREFIX)
    {
        return json!({
            "code": "check_failed",
            "label": "检查失败",
            "reason": reason,
            "expires_at": expires_at_unix_secs,
            "invalid_at": Value::Null,
            "source": "oauth_request",
            "requires_reauth": false,
            "expiring_soon": false,
        });
    }
    if invalid_reason
        .as_deref()
        .is_some_and(|reason| !reason.starts_with(OAUTH_ACCOUNT_BLOCK_PREFIX))
        || invalid_at_unix_secs.is_some()
    {
        return json!({
            "code": "invalid",
            "label": "已失效",
            "reason": invalid_reason,
            "expires_at": expires_at_unix_secs,
            "invalid_at": invalid_at_unix_secs,
            "source": "oauth_invalid",
            "requires_reauth": true,
            "expiring_soon": false,
        });
    }

    let Some(expires_at_unix_secs) = expires_at_unix_secs else {
        return default_oauth_status_snapshot_value();
    };
    if expires_at_unix_secs <= now_unix_secs {
        return json!({
            "code": "expired",
            "label": "已过期",
            "reason": "Access Token 已过期，等待自动续期",
            "expires_at": expires_at_unix_secs,
            "invalid_at": Value::Null,
            "source": "expires_at",
            "requires_reauth": false,
            "expiring_soon": false,
        });
    }

    let expiring_soon = expires_at_unix_secs.saturating_sub(now_unix_secs) < 24 * 60 * 60;
    json!({
        "code": if expiring_soon { "expiring" } else { "valid" },
        "label": if expiring_soon { "即将过期" } else { "有效" },
        "reason": Value::Null,
        "expires_at": expires_at_unix_secs,
        "invalid_at": Value::Null,
        "source": "expires_at",
        "requires_reauth": false,
        "expiring_soon": expiring_soon,
    })
}

pub(crate) fn sync_provider_key_oauth_status_snapshot(
    status_snapshot: Option<&Value>,
    key: &StoredProviderCatalogKey,
) -> Option<Value> {
    let mut snapshot = provider_key_status_snapshot_object(status_snapshot)
        .or_else(|| default_provider_key_status_snapshot().as_object().cloned())
        .unwrap_or_default();
    snapshot.insert(
        "oauth".to_string(),
        build_provider_key_oauth_status_snapshot(key),
    );
    Some(Value::Object(snapshot))
}

fn build_provider_key_account_status_snapshot(
    key: &StoredProviderCatalogKey,
    provider_type: &str,
) -> Value {
    let snapshot = admin_provider_status_pure::resolve_account_status_snapshot(
        Some(provider_type),
        key.upstream_metadata.as_ref(),
        key.oauth_invalid_reason.as_deref(),
    );
    json!({
        "code": snapshot.code,
        "label": snapshot.label,
        "reason": snapshot.reason,
        "blocked": snapshot.blocked,
        "source": snapshot.source,
        "recoverable": snapshot.recoverable,
    })
}

fn provider_key_status_snapshot_object(
    status_snapshot: Option<&Value>,
) -> Option<Map<String, Value>> {
    status_snapshot.and_then(|value| match value {
        Value::Object(object) => Some(object.clone()),
        _ => None,
    })
}

fn provider_quota_metadata_bucket<'a>(
    upstream_metadata: Option<&'a Value>,
    provider_type: &str,
) -> Option<&'a Map<String, Value>> {
    upstream_metadata
        .and_then(Value::as_object)
        .and_then(|metadata| metadata.get(&provider_type.trim().to_ascii_lowercase()))
        .and_then(Value::as_object)
}

fn provider_quota_timestamp_unix_secs(value: Option<&Value>) -> Option<u64> {
    let mut parsed = match value {
        Some(Value::Number(number)) => number.as_f64(),
        Some(Value::String(text)) => text.trim().parse::<f64>().ok(),
        _ => None,
    }?;
    if !parsed.is_finite() || parsed <= 0.0 {
        return None;
    }
    if parsed > 1_000_000_000_000.0 {
        parsed /= 1000.0;
    }
    Some(parsed.floor() as u64)
}

fn provider_quota_model_bucket(metadata: &Map<String, Value>) -> Option<&Map<String, Value>> {
    metadata
        .get("quota_by_model")
        .or_else(|| metadata.get("models"))
        .and_then(Value::as_object)
}

fn quota_window_reset_seconds(
    observed_at_unix_secs: Option<u64>,
    reset_at_unix_secs: Option<u64>,
) -> Option<u64> {
    observed_at_unix_secs
        .zip(reset_at_unix_secs)
        .map(|(observed_at, reset_at)| reset_at.saturating_sub(observed_at))
}

fn chatgpt_web_image_quota_limit(
    metadata: &Map<String, Value>,
    remaining: Option<f64>,
) -> Option<f64> {
    let explicit_limit = metadata
        .get("image_quota_total")
        .and_then(admin_provider_quota_pure::coerce_json_f64)
        .filter(|value| *value > 0.0);
    let plan_type = metadata
        .get("plan_type")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase());
    let limit_source = metadata
        .get("image_quota_limit_source")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if let Some(limit) = explicit_limit {
        if !chatgpt_web_image_quota_limit_is_legacy_free_default(
            limit,
            limit_source,
            plan_type.as_deref(),
            remaining,
        ) {
            return Some(limit);
        }
    }

    remaining.filter(|value| *value > 0.0)
}

fn chatgpt_web_image_quota_limit_is_legacy_free_default(
    limit: f64,
    limit_source: Option<&str>,
    plan_type: Option<&str>,
    remaining: Option<f64>,
) -> bool {
    let plan_type_is_free = plan_type
        .map(str::trim)
        .is_some_and(|value| value.eq_ignore_ascii_case("free"));
    if !plan_type_is_free || limit_source.is_some() {
        return false;
    }
    if (limit - 25.0).abs() > f64::EPSILON {
        return false;
    }
    remaining.is_none_or(|value| value < limit)
}

fn model_quota_window_snapshot(
    model_name: &str,
    item: &Map<String, Value>,
    observed_at_unix_secs: Option<u64>,
) -> Option<Value> {
    let remaining_value = item
        .get("remaining")
        .or_else(|| item.get("remaining_value"))
        .and_then(admin_provider_quota_pure::coerce_json_f64);
    let limit_value = item
        .get("total")
        .or_else(|| item.get("limit_value"))
        .and_then(admin_provider_quota_pure::coerce_json_f64)
        .filter(|value| *value > 0.0);
    let used_value = item
        .get("used")
        .or_else(|| item.get("used_value"))
        .and_then(admin_provider_quota_pure::coerce_json_f64)
        .or_else(|| {
            remaining_value
                .zip(limit_value)
                .map(|(remaining, limit)| (limit - remaining).max(0.0))
        });
    let used_ratio = item
        .get("used_percent")
        .and_then(admin_provider_quota_pure::coerce_json_f64)
        .map(|value| (value / 100.0).clamp(0.0, 1.0))
        .or_else(|| {
            item.get("remaining_fraction")
                .and_then(admin_provider_quota_pure::coerce_json_f64)
                .map(|value| (1.0 - value.clamp(0.0, 1.0)).clamp(0.0, 1.0))
        });
    let remaining_ratio = item
        .get("remaining_fraction")
        .and_then(admin_provider_quota_pure::coerce_json_f64)
        .map(|value| value.clamp(0.0, 1.0))
        .or_else(|| used_ratio.map(|value| (1.0 - value).max(0.0)));
    let reset_at = provider_quota_timestamp_unix_secs(
        item.get("reset_at").or_else(|| item.get("next_reset_at")),
    );
    let reset_seconds = quota_window_reset_seconds(observed_at_unix_secs, reset_at);
    let is_exhausted = item
        .get("is_exhausted")
        .and_then(admin_provider_quota_pure::coerce_json_bool)
        .or_else(|| used_ratio.map(|value| value >= 1.0 - 1e-6));

    if used_ratio.is_none()
        && remaining_ratio.is_none()
        && reset_at.is_none()
        && reset_seconds.is_none()
        && is_exhausted.is_none()
        && remaining_value.is_none()
        && limit_value.is_none()
    {
        return None;
    }

    let mut window = Map::new();
    let label = item
        .get("display_name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(model_name);
    window.insert("code".to_string(), json!(format!("model:{model_name}")));
    window.insert("label".to_string(), json!(label));
    window.insert("scope".to_string(), json!("model"));
    window.insert("unit".to_string(), json!("percent"));
    window.insert("model".to_string(), json!(model_name));
    window.insert("used_ratio".to_string(), json!(used_ratio));
    window.insert("remaining_ratio".to_string(), json!(remaining_ratio));
    window.insert("used_value".to_string(), json!(used_value));
    window.insert("remaining_value".to_string(), json!(remaining_value));
    window.insert("limit_value".to_string(), json!(limit_value));
    window.insert("reset_at".to_string(), json!(reset_at));
    window.insert("reset_seconds".to_string(), json!(reset_seconds));
    window.insert("is_exhausted".to_string(), json!(is_exhausted));
    Some(Value::Object(window))
}

fn provider_quota_metadata_string(
    metadata: &Map<String, Value>,
    fields: &[&str],
) -> Option<String> {
    fields.iter().find_map(|field| {
        metadata
            .get(*field)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    })
}

fn provider_quota_metadata_value_by_path<'a>(
    metadata: &'a Map<String, Value>,
    path: &[&str],
) -> Option<&'a Value> {
    let (first, rest) = path.split_first()?;
    let mut current = metadata.get(*first)?;
    for segment in rest {
        current = current.as_object()?.get(*segment)?;
    }
    Some(current)
}

fn provider_quota_metadata_number_by_paths(
    metadata: &Map<String, Value>,
    paths: &[&[&str]],
) -> Option<f64> {
    paths.iter().find_map(|path| {
        provider_quota_metadata_value_by_path(metadata, path)
            .and_then(admin_provider_quota_pure::coerce_json_f64)
            .filter(|value| value.is_finite())
    })
}

fn provider_quota_metadata_bool_by_paths(
    metadata: &Map<String, Value>,
    paths: &[&[&str]],
) -> Option<bool> {
    paths.iter().find_map(|path| {
        provider_quota_metadata_value_by_path(metadata, path)
            .and_then(admin_provider_quota_pure::coerce_json_bool)
    })
}

fn provider_quota_metadata_string_by_paths(
    metadata: &Map<String, Value>,
    paths: &[&[&str]],
) -> Option<String> {
    paths.iter().find_map(|path| {
        provider_quota_metadata_value_by_path(metadata, path)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    })
}

fn quota_windows_usage_ratio(windows: &[Value]) -> Option<f64> {
    windows
        .iter()
        .filter_map(Value::as_object)
        .filter_map(|window| window.get("used_ratio"))
        .filter_map(Value::as_f64)
        .max_by(f64::total_cmp)
}

fn quota_windows_min_reset_seconds(windows: &[Value]) -> Option<u64> {
    windows
        .iter()
        .filter_map(Value::as_object)
        .filter_map(|window| window.get("reset_seconds"))
        .filter_map(admin_provider_quota_pure::coerce_json_u64)
        .min()
}

fn quota_windows_min_reset_at(windows: &[Value]) -> Option<u64> {
    windows
        .iter()
        .filter_map(Value::as_object)
        .filter_map(|window| window.get("reset_at"))
        .filter_map(|value| provider_quota_timestamp_unix_secs(Some(value)))
        .min()
}

fn quota_windows_all_exhausted(windows: &[Value]) -> bool {
    let mut total = 0usize;
    let mut exhausted = 0usize;
    for window in windows.iter().filter_map(Value::as_object) {
        total += 1;
        let is_exhausted = window
            .get("is_exhausted")
            .and_then(admin_provider_quota_pure::coerce_json_bool)
            .or_else(|| {
                window
                    .get("used_ratio")
                    .and_then(Value::as_f64)
                    .map(|value| value >= 1.0 - 1e-6)
            })
            .unwrap_or(false);
        if is_exhausted {
            exhausted += 1;
        }
    }
    total > 0 && exhausted == total
}

fn preserve_quota_window_usage_state(current_status_snapshot: Option<&Value>, quota: &mut Value) {
    let Some(current_windows) = current_status_snapshot
        .and_then(Value::as_object)
        .and_then(|snapshot| snapshot.get("quota"))
        .and_then(Value::as_object)
        .and_then(|quota| quota.get("windows"))
        .and_then(Value::as_array)
    else {
        return;
    };
    let Some(next_windows) = quota.get_mut("windows").and_then(Value::as_array_mut) else {
        return;
    };

    for next_window in next_windows.iter_mut().filter_map(Value::as_object_mut) {
        let Some(code) = next_window
            .get("code")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|code| !code.is_empty())
        else {
            continue;
        };
        let current_window =
            current_windows
                .iter()
                .filter_map(Value::as_object)
                .find(|current_window| {
                    current_window
                        .get("code")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .is_some_and(|current_code| current_code.eq_ignore_ascii_case(code))
                });
        let Some(current_window) = current_window else {
            continue;
        };
        let current_reset_at = current_window
            .get("reset_at")
            .and_then(admin_provider_quota_pure::coerce_json_u64);
        let next_reset_at = next_window
            .get("reset_at")
            .and_then(admin_provider_quota_pure::coerce_json_u64);
        if current_reset_at.is_none() || current_reset_at != next_reset_at {
            continue;
        }

        if next_window
            .get("window_minutes")
            .and_then(admin_provider_quota_pure::coerce_json_u64)
            .is_none()
        {
            if let Some(window_minutes) = current_window
                .get("window_minutes")
                .and_then(admin_provider_quota_pure::coerce_json_u64)
                .or_else(|| codex_default_window_minutes(code))
            {
                next_window.insert("window_minutes".to_string(), json!(window_minutes));
            }
        }
        if let Some(usage_reset_at) = current_window
            .get("usage_reset_at")
            .and_then(admin_provider_quota_pure::coerce_json_u64)
        {
            next_window.insert("usage_reset_at".to_string(), json!(usage_reset_at));
        }
        if let Some(usage) = current_window.get("usage") {
            next_window.insert("usage".to_string(), usage.clone());
        }
    }
}

fn codex_default_window_minutes(code: &str) -> Option<u64> {
    if code.eq_ignore_ascii_case("5h") || code.eq_ignore_ascii_case("spark_5h") {
        Some(300)
    } else if code.eq_ignore_ascii_case("weekly") || code.eq_ignore_ascii_case("spark_weekly") {
        Some(10_080)
    } else {
        None
    }
}

fn codex_quota_window_snapshot(
    metadata: &Map<String, Value>,
    prefix: &str,
    code: &str,
    label: &str,
    observed_at_unix_secs: Option<u64>,
) -> Option<Value> {
    let used_percent_key = format!("{prefix}_used_percent");
    let reset_seconds_key = format!("{prefix}_reset_seconds");
    let reset_after_seconds_key = format!("{prefix}_reset_after_seconds");
    let reset_at_key = format!("{prefix}_reset_at");
    let window_minutes_key = format!("{prefix}_window_minutes");

    let used_percent = metadata
        .get(&used_percent_key)
        .and_then(admin_provider_quota_pure::coerce_json_f64);
    let explicit_reset_at = metadata
        .get(&reset_at_key)
        .and_then(admin_provider_quota_pure::coerce_json_u64);
    let reset_seconds = metadata
        .get(&reset_seconds_key)
        .and_then(admin_provider_quota_pure::coerce_json_u64)
        .or_else(|| {
            metadata
                .get(&reset_after_seconds_key)
                .and_then(admin_provider_quota_pure::coerce_json_u64)
        })
        .or_else(|| {
            observed_at_unix_secs
                .zip(explicit_reset_at)
                .map(|(observed_at, reset_at)| reset_at.saturating_sub(observed_at))
        });
    let reset_at = explicit_reset_at.or_else(|| {
        observed_at_unix_secs
            .zip(reset_seconds)
            .map(|(observed_at, reset_seconds)| observed_at.saturating_add(reset_seconds))
    });
    let explicit_window_minutes = metadata
        .get(&window_minutes_key)
        .and_then(admin_provider_quota_pure::coerce_json_u64);

    if used_percent.is_none()
        && reset_at.is_none()
        && reset_seconds.is_none()
        && explicit_window_minutes.is_none()
    {
        return None;
    }

    let window_minutes = explicit_window_minutes.or_else(|| codex_default_window_minutes(code));
    let used_ratio = used_percent.map(|value| (value / 100.0).clamp(0.0, 1.0));
    let remaining_ratio = used_ratio.map(|value| (1.0 - value).max(0.0));

    let mut window = Map::new();
    window.insert("code".to_string(), json!(code));
    window.insert("label".to_string(), json!(label));
    window.insert("scope".to_string(), json!("account"));
    window.insert("unit".to_string(), json!("percent"));
    window.insert("used_ratio".to_string(), json!(used_ratio));
    window.insert("remaining_ratio".to_string(), json!(remaining_ratio));
    window.insert("reset_at".to_string(), json!(reset_at));
    window.insert("reset_seconds".to_string(), json!(reset_seconds));
    window.insert("window_minutes".to_string(), json!(window_minutes));
    Some(Value::Object(window))
}

fn build_codex_quota_status_snapshot(
    upstream_metadata: Option<&Value>,
    source: &str,
) -> Option<Value> {
    let metadata = provider_quota_metadata_bucket(upstream_metadata, "codex")?;
    let observed_at_unix_secs = metadata
        .get("updated_at")
        .and_then(admin_provider_quota_pure::coerce_json_u64);
    let plan_type = metadata
        .get("plan_type")
        .and_then(Value::as_str)
        .and_then(|value| admin_provider_quota_pure::normalize_codex_plan_type(Some(value)));
    let credits_has_credits = metadata
        .get("has_credits")
        .and_then(admin_provider_quota_pure::coerce_json_bool);
    let credits_balance = metadata
        .get("credits_balance")
        .and_then(admin_provider_quota_pure::coerce_json_f64);
    let credits_unlimited = metadata
        .get("credits_unlimited")
        .and_then(admin_provider_quota_pure::coerce_json_bool);

    let windows = [
        codex_quota_window_snapshot(metadata, "primary", "weekly", "周", observed_at_unix_secs),
        codex_quota_window_snapshot(metadata, "secondary", "5h", "5H", observed_at_unix_secs),
        codex_quota_window_snapshot(
            metadata,
            "spark_primary",
            "spark_5h",
            "Spark 5H",
            observed_at_unix_secs,
        ),
        codex_quota_window_snapshot(
            metadata,
            "spark_secondary",
            "spark_weekly",
            "Spark 周",
            observed_at_unix_secs,
        ),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();

    if windows.is_empty()
        && plan_type.is_none()
        && credits_has_credits.is_none()
        && credits_balance.is_none()
        && credits_unlimited.is_none()
        && observed_at_unix_secs.is_none()
    {
        return None;
    }

    let primary_windows = windows
        .iter()
        .filter(|window| {
            window
                .get("code")
                .and_then(Value::as_str)
                .is_some_and(|code| {
                    code.eq_ignore_ascii_case("weekly") || code.eq_ignore_ascii_case("5h")
                })
        })
        .cloned()
        .collect::<Vec<_>>();
    let usage_ratio = primary_windows
        .iter()
        .filter_map(Value::as_object)
        .filter_map(|window| window.get("used_ratio"))
        .filter_map(Value::as_f64)
        .max_by(f64::total_cmp);
    let reset_seconds = primary_windows
        .iter()
        .filter_map(Value::as_object)
        .filter_map(|window| window.get("reset_seconds"))
        .filter_map(admin_provider_quota_pure::coerce_json_u64)
        .min();
    let reset_at = quota_windows_min_reset_at(&primary_windows);
    let exhausted_by_credits = primary_windows.is_empty()
        && credits_unlimited != Some(true)
        && credits_has_credits == Some(false);
    let exhausted_by_window = usage_ratio.is_some_and(|value| value >= 1.0 - 1e-6);
    let exhausted = exhausted_by_credits || exhausted_by_window;

    let mut credits = Map::new();
    if let Some(value) = credits_has_credits {
        credits.insert("has_credits".to_string(), json!(value));
    }
    if let Some(value) = credits_balance {
        credits.insert("balance".to_string(), json!(value));
    }
    if let Some(value) = credits_unlimited {
        credits.insert("unlimited".to_string(), json!(value));
    }

    let reason = if exhausted_by_credits {
        Some("无可用积分")
    } else if exhausted_by_window {
        Some("额度窗口已耗尽")
    } else {
        None
    };

    Some(json!({
        "version": 2,
        "provider_type": "codex",
        "code": if exhausted { "exhausted" } else { "ok" },
        "label": if exhausted { Some("额度耗尽") } else { None::<&str> },
        "reason": reason,
        "freshness": "fresh",
        "source": source,
        "observed_at": observed_at_unix_secs,
        "exhausted": exhausted,
        "usage_ratio": usage_ratio,
        "updated_at": observed_at_unix_secs,
        "reset_at": reset_at,
        "reset_seconds": reset_seconds,
        "plan_type": plan_type,
        "credits": if credits.is_empty() {
            Value::Null
        } else {
            Value::Object(credits)
        },
        "windows": windows,
    }))
}

fn build_kiro_quota_status_snapshot(
    upstream_metadata: Option<&Value>,
    source: &str,
) -> Option<Value> {
    let metadata = provider_quota_metadata_bucket(upstream_metadata, "kiro")?;
    let observed_at_unix_secs = provider_quota_timestamp_unix_secs(metadata.get("updated_at"));
    let usage_limit = metadata
        .get("usage_limit")
        .and_then(admin_provider_quota_pure::coerce_json_f64);
    let current_usage = metadata
        .get("current_usage")
        .and_then(admin_provider_quota_pure::coerce_json_f64);
    let remaining = metadata
        .get("remaining")
        .and_then(admin_provider_quota_pure::coerce_json_f64);
    let usage_ratio = metadata
        .get("usage_percentage")
        .and_then(admin_provider_quota_pure::coerce_json_f64)
        .map(|value| (value / 100.0).clamp(0.0, 1.0))
        .or_else(|| {
            current_usage
                .zip(usage_limit)
                .and_then(|(current_usage, usage_limit)| {
                    (usage_limit > 0.0).then_some((current_usage / usage_limit).clamp(0.0, 1.0))
                })
        });
    let remaining_ratio = usage_ratio.map(|value| (1.0 - value).max(0.0));
    let next_reset_at = provider_quota_timestamp_unix_secs(metadata.get("next_reset_at"));
    let reset_seconds = quota_window_reset_seconds(observed_at_unix_secs, next_reset_at);
    let plan_type = metadata
        .get("subscription_title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let is_banned = metadata
        .get("is_banned")
        .and_then(admin_provider_quota_pure::coerce_json_bool)
        == Some(true);
    let ban_reason = metadata
        .get("ban_reason")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);

    let mut windows = Vec::new();
    if usage_ratio.is_some()
        || remaining.is_some()
        || usage_limit.is_some()
        || current_usage.is_some()
        || next_reset_at.is_some()
    {
        windows.push(json!({
            "code": "usage",
            "label": "额度",
            "scope": "account",
            "unit": "count",
            "used_ratio": usage_ratio,
            "remaining_ratio": remaining_ratio,
            "used_value": current_usage,
            "remaining_value": remaining,
            "limit_value": usage_limit,
            "reset_at": next_reset_at,
            "reset_seconds": reset_seconds,
        }));
    }

    if windows.is_empty() && plan_type.is_none() && observed_at_unix_secs.is_none() && !is_banned {
        return None;
    }

    let exhausted = !is_banned
        && (remaining.is_some_and(|value| value <= 0.0)
            || usage_ratio.is_some_and(|value| value >= 1.0 - 1e-6));
    let reason = if is_banned {
        ban_reason
    } else if exhausted {
        Some("额度已耗尽".to_string())
    } else {
        None
    };
    let label = if is_banned {
        Some("账号已封禁")
    } else if exhausted {
        Some("额度耗尽")
    } else {
        None
    };
    let code = if is_banned {
        "banned"
    } else if exhausted {
        "exhausted"
    } else {
        "ok"
    };

    Some(json!({
        "version": 2,
        "provider_type": "kiro",
        "code": code,
        "label": label,
        "reason": reason,
        "freshness": "fresh",
        "source": source,
        "observed_at": observed_at_unix_secs,
        "exhausted": exhausted,
        "usage_ratio": usage_ratio,
        "updated_at": observed_at_unix_secs,
        "reset_at": next_reset_at,
        "reset_seconds": reset_seconds,
        "plan_type": plan_type,
        "windows": windows,
    }))
}

fn build_chatgpt_web_quota_status_snapshot(
    upstream_metadata: Option<&Value>,
    source: &str,
) -> Option<Value> {
    let metadata = provider_quota_metadata_bucket(upstream_metadata, "chatgpt_web")?;
    let observed_at_unix_secs = provider_quota_timestamp_unix_secs(metadata.get("updated_at"));
    let remaining = metadata
        .get("image_quota_remaining")
        .and_then(admin_provider_quota_pure::coerce_json_f64);
    let limit = chatgpt_web_image_quota_limit(metadata, remaining);
    let used = metadata
        .get("image_quota_used")
        .and_then(admin_provider_quota_pure::coerce_json_f64)
        .or_else(|| {
            limit
                .zip(remaining)
                .map(|(limit, remaining)| (limit - remaining).max(0.0))
        });
    let reset_at = provider_quota_timestamp_unix_secs(metadata.get("image_quota_reset_at"));
    let reset_seconds = quota_window_reset_seconds(observed_at_unix_secs, reset_at);
    let plan_type = metadata
        .get("plan_type")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase());
    let image_blocked = metadata
        .get("image_quota_blocked")
        .and_then(admin_provider_quota_pure::coerce_json_bool)
        == Some(true);
    let usage_ratio = used
        .zip(limit)
        .and_then(|(used, limit)| (limit > 0.0).then_some((used / limit).clamp(0.0, 1.0)));
    let remaining_ratio = remaining.zip(limit).and_then(|(remaining, limit)| {
        (limit > 0.0).then_some((remaining / limit).clamp(0.0, 1.0))
    });

    let mut windows = Vec::new();
    if remaining.is_some()
        || limit.is_some()
        || used.is_some()
        || reset_at.is_some()
        || image_blocked
    {
        windows.push(json!({
            "code": "image_gen",
            "label": "生图",
            "scope": "account",
            "unit": "count",
            "used_ratio": usage_ratio,
            "remaining_ratio": remaining_ratio,
            "used_value": used,
            "remaining_value": remaining,
            "limit_value": limit,
            "reset_at": reset_at,
            "reset_seconds": reset_seconds,
            "is_exhausted": image_blocked || remaining.is_some_and(|value| value <= 0.0),
        }));
    }

    if windows.is_empty() && plan_type.is_none() && observed_at_unix_secs.is_none() {
        return None;
    }

    let exhausted = image_blocked
        || remaining.is_some_and(|value| value <= 0.0)
        || usage_ratio.is_some_and(|value| value >= 1.0 - 1e-6);
    let reason = if exhausted {
        Some("生图额度已耗尽")
    } else {
        None
    };

    Some(json!({
        "version": 2,
        "provider_type": "chatgpt_web",
        "code": if exhausted { "exhausted" } else { "ok" },
        "label": if exhausted { Some("额度耗尽") } else { None::<&str> },
        "reason": reason,
        "freshness": "fresh",
        "source": source,
        "observed_at": observed_at_unix_secs,
        "exhausted": exhausted,
        "usage_ratio": usage_ratio,
        "updated_at": observed_at_unix_secs,
        "reset_at": reset_at,
        "reset_seconds": reset_seconds,
        "plan_type": plan_type,
        "windows": windows,
    }))
}

fn windsurf_percent_quota_window_snapshot(
    metadata: &Map<String, Value>,
    code: &str,
    label: &str,
    remaining_percent_key: &str,
    reset_at_key: &str,
    observed_at_unix_secs: Option<u64>,
) -> Option<Value> {
    let remaining_percent = metadata
        .get(remaining_percent_key)
        .and_then(admin_provider_quota_pure::coerce_json_f64);
    let reset_at = provider_quota_timestamp_unix_secs(metadata.get(reset_at_key));
    if remaining_percent.is_none() && reset_at.is_none() {
        return None;
    }

    let remaining_ratio = remaining_percent.map(|value| (value / 100.0).clamp(0.0, 1.0));
    let used_ratio = remaining_ratio.map(|value| (1.0 - value).clamp(0.0, 1.0));
    let reset_seconds = quota_window_reset_seconds(observed_at_unix_secs, reset_at);

    Some(json!({
        "code": code,
        "label": label,
        "scope": "account",
        "unit": "percent",
        "used_ratio": used_ratio,
        "remaining_ratio": remaining_ratio,
        "reset_at": reset_at,
        "reset_seconds": reset_seconds,
        "is_exhausted": remaining_ratio.map(|value| value <= 1e-6),
    }))
}

fn windsurf_count_quota_window_snapshot(
    metadata: &Map<String, Value>,
    code: &str,
    label: &str,
    used_key: &str,
    limit_key: &str,
    remaining_key: &str,
) -> Option<Value> {
    let used = metadata
        .get(used_key)
        .and_then(admin_provider_quota_pure::coerce_json_f64);
    let limit = metadata
        .get(limit_key)
        .and_then(admin_provider_quota_pure::coerce_json_f64);
    let remaining = metadata
        .get(remaining_key)
        .and_then(admin_provider_quota_pure::coerce_json_f64)
        .or_else(|| limit.zip(used).map(|(limit, used)| (limit - used).max(0.0)));
    if used.is_none() && limit.is_none() && remaining.is_none() {
        return None;
    }

    let used_ratio = used
        .zip(limit)
        .and_then(|(used, limit)| (limit > 0.0).then_some((used / limit).clamp(0.0, 1.0)));
    let remaining_ratio = remaining.zip(limit).and_then(|(remaining, limit)| {
        (limit > 0.0).then_some((remaining / limit).clamp(0.0, 1.0))
    });

    Some(json!({
        "code": code,
        "label": label,
        "scope": "account",
        "unit": "count",
        "used_ratio": used_ratio,
        "remaining_ratio": remaining_ratio,
        "used_value": used,
        "remaining_value": remaining,
        "limit_value": limit,
        "is_exhausted": remaining.is_some_and(|value| value <= 0.0),
    }))
}

fn build_windsurf_quota_status_snapshot(
    upstream_metadata: Option<&Value>,
    source: &str,
) -> Option<Value> {
    let metadata = provider_quota_metadata_bucket(upstream_metadata, "windsurf")?;
    let observed_at_unix_secs = provider_quota_timestamp_unix_secs(metadata.get("updated_at"));
    let plan_type = metadata
        .get("plan_name")
        .or_else(|| metadata.get("plan_type"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let rate_limit = metadata
        .get("rate_limit")
        .cloned()
        .filter(|value| !value.is_null());
    let last_error = metadata
        .get("last_error")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let banned = metadata
        .get("banned")
        .or_else(|| metadata.get("is_banned"))
        .and_then(admin_provider_quota_pure::coerce_json_bool)
        == Some(true);
    let quarantined = metadata
        .get("quarantined")
        .or_else(|| metadata.get("is_quarantined"))
        .and_then(admin_provider_quota_pure::coerce_json_bool)
        == Some(true);

    let mut windows = [
        windsurf_percent_quota_window_snapshot(
            metadata,
            "daily",
            "日",
            "daily_remaining_percent",
            "daily_reset_at",
            observed_at_unix_secs,
        ),
        windsurf_percent_quota_window_snapshot(
            metadata,
            "weekly",
            "周",
            "weekly_remaining_percent",
            "weekly_reset_at",
            observed_at_unix_secs,
        ),
        windsurf_count_quota_window_snapshot(
            metadata,
            "prompt",
            "Prompt",
            "prompt_used",
            "prompt_limit",
            "prompt_remaining",
        ),
        windsurf_count_quota_window_snapshot(
            metadata,
            "flex",
            "Flex",
            "flex_used",
            "flex_limit",
            "flex_remaining",
        ),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();

    let mut rate_limit_cooling = false;
    let mut rate_limit_reset_seconds = None;
    let mut rate_limit_reason = None::<String>;
    if let Some(rate_limit) = rate_limit.as_ref() {
        if let Some(rate_limit_object) = rate_limit.as_object() {
            let retry_after_ms = rate_limit_object
                .get("retry_after_ms")
                .or_else(|| rate_limit_object.get("retryAfterMs"))
                .and_then(admin_provider_quota_pure::coerce_json_u64)
                .filter(|value| *value > 0);
            if let Some(retry_after_ms) = retry_after_ms {
                rate_limit_cooling = true;
                rate_limit_reset_seconds = Some(retry_after_ms.saturating_add(999) / 1000);
                rate_limit_reason = rate_limit_object
                    .get("message")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned);
                windows.push(json!({
                    "code": "rate_limit",
                    "label": "速率",
                    "scope": "account",
                    "unit": "count",
                    "is_exhausted": false,
                    "reset_seconds": rate_limit_reset_seconds,
                }));
            }
        }
    }

    let allowed_models_count = metadata
        .get("allowed_models_count")
        .or_else(|| metadata.get("models_count"))
        .and_then(admin_provider_quota_pure::coerce_json_u64);

    if windows.is_empty()
        && plan_type.is_none()
        && observed_at_unix_secs.is_none()
        && rate_limit.is_none()
        && allowed_models_count.is_none()
        && !banned
        && !quarantined
    {
        return None;
    }

    let usage_ratio = quota_windows_usage_ratio(&windows);
    let reset_seconds = if rate_limit_cooling {
        rate_limit_reset_seconds.or_else(|| quota_windows_min_reset_seconds(&windows))
    } else {
        quota_windows_min_reset_seconds(&windows)
    };
    let reset_at = if rate_limit_cooling {
        None
    } else {
        quota_windows_min_reset_at(&windows)
    };
    let exhausted_by_window = windows.iter().filter_map(Value::as_object).any(|window| {
        window
            .get("code")
            .and_then(Value::as_str)
            .is_some_and(|code| {
                code.eq_ignore_ascii_case("daily") || code.eq_ignore_ascii_case("weekly")
            })
            && window
                .get("is_exhausted")
                .and_then(admin_provider_quota_pure::coerce_json_bool)
                .unwrap_or(false)
    });
    let exhausted = banned || quarantined || exhausted_by_window;
    let (code, label, reason) = if banned {
        (
            "banned",
            Some("账号已封禁"),
            last_error
                .clone()
                .or_else(|| Some("账号被 Windsurf 标记为不可用".to_string())),
        )
    } else if quarantined {
        (
            "quarantined",
            Some("账号隔离中"),
            last_error
                .clone()
                .or_else(|| Some("账号处于隔离状态".to_string())),
        )
    } else if rate_limit_cooling {
        (
            "cooldown",
            Some("冷却中"),
            last_error.clone().or(rate_limit_reason),
        )
    } else if exhausted {
        (
            "exhausted",
            Some("额度耗尽"),
            Some("额度窗口已耗尽".to_string()),
        )
    } else {
        ("ok", None, last_error)
    };

    Some(json!({
        "version": 2,
        "provider_type": "windsurf",
        "code": code,
        "label": label,
        "reason": reason,
        "freshness": "fresh",
        "source": source,
        "observed_at": observed_at_unix_secs,
        "exhausted": exhausted,
        "usage_ratio": usage_ratio,
        "updated_at": observed_at_unix_secs,
        "reset_at": reset_at,
        "reset_seconds": reset_seconds,
        "plan_type": plan_type,
        "allowed_models_count": allowed_models_count,
        "rate_limit": rate_limit.unwrap_or(Value::Null),
        "windows": windows,
    }))
}

fn build_antigravity_quota_status_snapshot(
    upstream_metadata: Option<&Value>,
    source: &str,
) -> Option<Value> {
    let metadata = provider_quota_metadata_bucket(upstream_metadata, "antigravity")?;
    let observed_at_unix_secs = provider_quota_timestamp_unix_secs(metadata.get("updated_at"));
    let is_forbidden = metadata
        .get("is_forbidden")
        .and_then(admin_provider_quota_pure::coerce_json_bool)
        == Some(true);
    let forbidden_reason = metadata
        .get("forbidden_reason")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let windows = provider_quota_model_bucket(metadata)
        .map(|models| {
            models
                .iter()
                .filter_map(|(model_name, item)| {
                    model_quota_window_snapshot(
                        model_name,
                        item.as_object()?,
                        observed_at_unix_secs,
                    )
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if windows.is_empty() && observed_at_unix_secs.is_none() && !is_forbidden {
        return None;
    }

    let usage_ratio = quota_windows_usage_ratio(&windows);
    let reset_seconds = quota_windows_min_reset_seconds(&windows);
    let reset_at = quota_windows_min_reset_at(&windows);
    let exhausted = !is_forbidden && quota_windows_all_exhausted(&windows);
    let reason = if is_forbidden {
        forbidden_reason
    } else if exhausted {
        Some("所有模型额度已耗尽".to_string())
    } else {
        None
    };
    let label = if is_forbidden {
        Some("访问受限")
    } else if exhausted {
        Some("额度耗尽")
    } else {
        None
    };
    let code = if is_forbidden {
        "forbidden"
    } else if exhausted {
        "exhausted"
    } else {
        "ok"
    };

    Some(json!({
        "version": 2,
        "provider_type": "antigravity",
        "code": code,
        "label": label,
        "reason": reason,
        "freshness": "fresh",
        "source": source,
        "observed_at": observed_at_unix_secs,
        "exhausted": exhausted,
        "usage_ratio": usage_ratio,
        "updated_at": observed_at_unix_secs,
        "reset_at": reset_at,
        "reset_seconds": reset_seconds,
        "plan_type": serde_json::Value::Null,
        "windows": windows,
    }))
}

fn build_grok_quota_status_snapshot(
    upstream_metadata: Option<&Value>,
    source: &str,
) -> Option<Value> {
    let metadata = provider_quota_metadata_bucket(upstream_metadata, "grok")?;
    let observed_at_unix_secs = provider_quota_timestamp_unix_secs(metadata.get("updated_at"));
    let inferred_pool_tier = grok_pool_tier_from_quota_bucket(metadata);
    let pool_tier = provider_quota_metadata_string(metadata, &["pool_tier", "tier"])
        .or_else(|| inferred_pool_tier.map(ToOwned::to_owned));
    let plan_type = provider_quota_metadata_string(metadata, &["plan_type", "plan"])
        .or_else(|| pool_tier.clone());
    let supported_windows = grok_supported_quota_windows_for_tier(pool_tier.as_deref());
    let windows = provider_quota_model_bucket(metadata)
        .map(|models| {
            models
                .iter()
                .filter_map(|(model_name, item)| {
                    if !supported_windows
                        .iter()
                        .any(|(quota_key, _)| *quota_key == model_name.as_str())
                    {
                        return None;
                    }
                    model_quota_window_snapshot(
                        model_name,
                        item.as_object()?,
                        observed_at_unix_secs,
                    )
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if windows.is_empty() && observed_at_unix_secs.is_none() {
        return None;
    }

    let usage_ratio = quota_windows_usage_ratio(&windows);
    let reset_seconds = quota_windows_min_reset_seconds(&windows);
    let reset_at = quota_windows_min_reset_at(&windows);
    let exhausted = quota_windows_all_exhausted(&windows);

    Some(json!({
        "version": 2,
        "provider_type": "grok",
        "code": if exhausted { "exhausted" } else { "ok" },
        "label": if exhausted { Some("额度耗尽") } else { None::<&str> },
        "reason": if exhausted {
            Some("所有 Grok 模式额度已耗尽")
        } else {
            None::<&str>
        },
        "freshness": "fresh",
        "source": source,
        "observed_at": observed_at_unix_secs,
        "exhausted": exhausted,
        "usage_ratio": usage_ratio,
        "updated_at": observed_at_unix_secs,
        "reset_at": reset_at,
        "reset_seconds": reset_seconds,
        "plan_type": plan_type,
        "pool_tier": pool_tier,
        "windows": windows,
    }))
}

fn gemini_cli_plan_type(metadata: &Map<String, Value>) -> Option<String> {
    provider_quota_metadata_string(metadata, &["plan_type", "tier", "plan"]).or_else(|| {
        provider_quota_metadata_string_by_paths(
            metadata,
            &[
                &["paidTier", "id"],
                &["paidTier", "tierType"],
                &["paidTier", "name"],
                &["currentTier", "id"],
                &["currentTier", "tierType"],
                &["currentTier", "name"],
            ],
        )
    })
}

fn gemini_cli_credits_status_snapshot(metadata: &Map<String, Value>) -> Option<Value> {
    let remaining = provider_quota_metadata_number_by_paths(
        metadata,
        &[
            &["credits", "remaining"],
            &["credits", "remainingCredits"],
            &["credits", "available"],
            &["credits", "availableCredits"],
            &["credits", "balance"],
            &["remainingCredits"],
            &["availableCredits"],
            &["paidTier", "remainingCredits"],
            &["paidTier", "availableCredits"],
            &["currentTier", "remainingCredits"],
            &["currentTier", "availableCredits"],
        ],
    );
    let balance = provider_quota_metadata_number_by_paths(
        metadata,
        &[
            &["credits", "balance"],
            &["credits", "remaining"],
            &["credits", "available"],
            &["paidTier", "availableCredits"],
            &["currentTier", "availableCredits"],
            &["availableCredits"],
            &["remainingCredits"],
        ],
    )
    .or(remaining);
    let consumed = provider_quota_metadata_number_by_paths(
        metadata,
        &[
            &["credits", "consumed"],
            &["credits", "consumedCredits"],
            &["consumedCredits"],
            &["paidTier", "consumedCredits"],
            &["currentTier", "consumedCredits"],
        ],
    );
    let total = provider_quota_metadata_number_by_paths(
        metadata,
        &[
            &["credits", "total"],
            &["credits", "totalCredits"],
            &["totalCredits"],
            &["paidTier", "totalCredits"],
            &["currentTier", "totalCredits"],
        ],
    );
    let unlimited = provider_quota_metadata_bool_by_paths(
        metadata,
        &[
            &["credits", "unlimited"],
            &["credits_unlimited"],
            &["paidTier", "unlimited"],
            &["currentTier", "unlimited"],
        ],
    );
    let explicit_has_credits = provider_quota_metadata_bool_by_paths(
        metadata,
        &[
            &["credits", "has_credits"],
            &["has_credits"],
            &["paidTier", "hasCredits"],
            &["currentTier", "hasCredits"],
        ],
    );
    let trace_id = provider_quota_metadata_string_by_paths(
        metadata,
        &[
            &["credits", "trace_id"],
            &["credits", "traceId"],
            &["trace_id"],
            &["traceId"],
        ],
    );
    let updated_at = provider_quota_metadata_value_by_path(metadata, &["credits", "updated_at"])
        .or_else(|| provider_quota_metadata_value_by_path(metadata, &["credits", "updatedAt"]))
        .or_else(|| metadata.get("updated_at"))
        .and_then(|value| provider_quota_timestamp_unix_secs(Some(value)));

    if remaining.is_none()
        && balance.is_none()
        && consumed.is_none()
        && total.is_none()
        && unlimited.is_none()
        && explicit_has_credits.is_none()
        && trace_id.is_none()
    {
        return None;
    }

    let has_credits = explicit_has_credits
        .or_else(|| unlimited.filter(|value| *value))
        .or_else(|| remaining.or(balance).map(|value| value > 0.0));
    let mut credits = Map::new();
    credits.insert("has_credits".to_string(), json!(has_credits));
    credits.insert("balance".to_string(), json!(balance));
    credits.insert("remaining".to_string(), json!(remaining.or(balance)));
    credits.insert("consumed".to_string(), json!(consumed));
    credits.insert("total".to_string(), json!(total));
    credits.insert("unlimited".to_string(), json!(unlimited));
    credits.insert("trace_id".to_string(), json!(trace_id));
    credits.insert("updated_at".to_string(), json!(updated_at));
    Some(Value::Object(credits))
}

fn build_gemini_cli_quota_status_snapshot(
    upstream_metadata: Option<&Value>,
    source: &str,
) -> Option<Value> {
    let metadata = provider_quota_metadata_bucket(upstream_metadata, "gemini_cli")?;
    let credits = gemini_cli_credits_status_snapshot(metadata);
    let plan_type = gemini_cli_plan_type(metadata);
    let observed_at_unix_secs = provider_quota_timestamp_unix_secs(metadata.get("updated_at"))
        .or_else(|| {
            credits
                .as_ref()
                .and_then(|value| value.get("updated_at"))
                .and_then(|value| provider_quota_timestamp_unix_secs(Some(value)))
        });
    let windows = provider_quota_model_bucket(metadata)
        .map(|models| {
            models
                .iter()
                .filter_map(|(model_name, item)| {
                    model_quota_window_snapshot(
                        model_name,
                        item.as_object()?,
                        observed_at_unix_secs,
                    )
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if windows.is_empty()
        && observed_at_unix_secs.is_none()
        && credits.is_none()
        && plan_type.is_none()
    {
        return None;
    }

    let usage_ratio = quota_windows_usage_ratio(&windows);
    let active_exhausted_windows = windows
        .iter()
        .filter_map(Value::as_object)
        .filter(|window| {
            window
                .get("is_exhausted")
                .and_then(admin_provider_quota_pure::coerce_json_bool)
                .or_else(|| {
                    window
                        .get("used_ratio")
                        .and_then(Value::as_f64)
                        .map(|value| value >= 1.0 - 1e-6)
                })
                .unwrap_or(false)
        })
        .filter(|window| {
            provider_quota_timestamp_unix_secs(window.get("reset_at"))
                .zip(observed_at_unix_secs)
                .map(|(reset_at, observed_at)| reset_at > observed_at)
                .unwrap_or(true)
        })
        .count();
    let exhausted = !windows.is_empty() && active_exhausted_windows == windows.len();
    let cooling = active_exhausted_windows > 0;
    let reset_seconds = if cooling {
        quota_windows_min_reset_seconds(&windows)
    } else {
        None
    };
    let reset_at = if cooling {
        quota_windows_min_reset_at(&windows)
    } else {
        None
    };

    Some(json!({
        "version": 2,
        "provider_type": "gemini_cli",
        "code": if exhausted {
            "exhausted"
        } else if cooling {
            "cooldown"
        } else {
            "ok"
        },
        "label": if cooling { Some("冷却中") } else { None::<&str> },
        "reason": if exhausted {
            Some("所有模型均处于冷却中")
        } else {
            None::<&str>
        },
        "freshness": "fresh",
        "source": source,
        "observed_at": observed_at_unix_secs,
        "exhausted": exhausted,
        "usage_ratio": usage_ratio,
        "updated_at": observed_at_unix_secs,
        "reset_at": reset_at,
        "reset_seconds": reset_seconds,
        "plan_type": plan_type,
        "credits": credits,
        "windows": windows,
    }))
}

pub(crate) fn sync_provider_key_quota_status_snapshot(
    status_snapshot: Option<&Value>,
    provider_type: &str,
    upstream_metadata: Option<&Value>,
    source: &str,
) -> Option<Value> {
    let normalized_provider_type = provider_type.trim().to_ascii_lowercase();
    let mut quota = match normalized_provider_type.as_str() {
        "codex" => build_codex_quota_status_snapshot(upstream_metadata, source),
        "kiro" => build_kiro_quota_status_snapshot(upstream_metadata, source),
        "chatgpt_web" => build_chatgpt_web_quota_status_snapshot(upstream_metadata, source),
        "windsurf" => build_windsurf_quota_status_snapshot(upstream_metadata, source),
        "antigravity" => build_antigravity_quota_status_snapshot(upstream_metadata, source),
        "grok" => build_grok_quota_status_snapshot(upstream_metadata, source),
        "gemini_cli" => build_gemini_cli_quota_status_snapshot(upstream_metadata, source),
        _ => None,
    }?;
    if normalized_provider_type == "codex" {
        preserve_quota_window_usage_state(status_snapshot, &mut quota);
    }

    let default_snapshot = default_provider_key_status_snapshot();
    let mut snapshot = provider_key_status_snapshot_object(status_snapshot)
        .or_else(|| default_snapshot.as_object().cloned())
        .unwrap_or_default();
    snapshot.insert("quota".to_string(), quota);
    Some(Value::Object(snapshot))
}

fn quota_snapshot_has_materialized_data(
    quota_snapshot: Option<&Map<String, Value>>,
    provider_type: &str,
) -> bool {
    let Some(quota_snapshot) = quota_snapshot else {
        return false;
    };

    let normalized_provider_type = provider_type.trim().to_ascii_lowercase();
    let snapshot_provider_type = quota_snapshot
        .get("provider_type")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !snapshot_provider_type.is_empty() && snapshot_provider_type != normalized_provider_type {
        return false;
    }

    if normalized_provider_type == "windsurf"
        && windsurf_quota_snapshot_has_stale_cooldown(quota_snapshot)
    {
        return false;
    }

    if quota_snapshot
        .get("windows")
        .and_then(Value::as_array)
        .is_some_and(|windows| !windows.is_empty())
    {
        return true;
    }
    if quota_snapshot
        .get("credits")
        .is_some_and(|credits| !credits.is_null())
    {
        return true;
    }

    quota_snapshot
        .get("code")
        .and_then(Value::as_str)
        .map(str::trim)
        .is_some_and(|code| {
            !code.is_empty()
                && !code.eq_ignore_ascii_case("unknown")
                && !code.eq_ignore_ascii_case("ok")
        })
}

fn windsurf_quota_snapshot_has_stale_cooldown(quota_snapshot: &Map<String, Value>) -> bool {
    let code = quota_snapshot
        .get("code")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if !code.eq_ignore_ascii_case("cooldown") {
        return false;
    }

    let rate_limit = quota_snapshot.get("rate_limit").and_then(Value::as_object);
    let retry_after_ms = rate_limit
        .and_then(|rate_limit| {
            rate_limit
                .get("retry_after_ms")
                .or_else(|| rate_limit.get("retryAfterMs"))
                .and_then(admin_provider_quota_pure::coerce_json_u64)
        })
        .unwrap_or(0);
    if retry_after_ms > 0 {
        return false;
    }

    let has_positive_rate_limit_reset = quota_snapshot
        .get("windows")
        .and_then(Value::as_array)
        .is_some_and(|windows| {
            windows.iter().filter_map(Value::as_object).any(|window| {
                window
                    .get("code")
                    .and_then(Value::as_str)
                    .is_some_and(|code| code.eq_ignore_ascii_case("rate_limit"))
                    && window
                        .get("reset_seconds")
                        .or_else(|| window.get("reset_at"))
                        .and_then(admin_provider_quota_pure::coerce_json_u64)
                        .is_some_and(|value| value > 0)
            })
        });
    if has_positive_rate_limit_reset {
        return false;
    }

    let exhausted = quota_snapshot
        .get("exhausted")
        .and_then(admin_provider_quota_pure::coerce_json_bool)
        .unwrap_or(false);
    let has_capacity = rate_limit
        .and_then(|rate_limit| rate_limit.get("has_capacity"))
        .and_then(admin_provider_quota_pure::coerce_json_bool)
        .unwrap_or(false);

    has_capacity || !exhausted
}

pub(crate) fn provider_key_status_snapshot_payload(
    key: &StoredProviderCatalogKey,
    provider_type: &str,
) -> serde_json::Value {
    let status_snapshot = key
        .status_snapshot
        .as_ref()
        .filter(|value| value.is_object());
    let quota_snapshot = status_snapshot
        .and_then(Value::as_object)
        .and_then(|snapshot| snapshot.get("quota"))
        .and_then(Value::as_object);

    let payload = if quota_snapshot_has_materialized_data(quota_snapshot, provider_type) {
        status_snapshot
            .cloned()
            .unwrap_or_else(default_provider_key_status_snapshot)
    } else {
        sync_provider_key_quota_status_snapshot(
            status_snapshot,
            provider_type,
            key.upstream_metadata.as_ref(),
            "catalog_fallback",
        )
        .or_else(|| status_snapshot.cloned())
        .unwrap_or_else(default_provider_key_status_snapshot)
    };

    let mut snapshot = provider_key_status_snapshot_object(Some(&payload))
        .or_else(|| default_provider_key_status_snapshot().as_object().cloned())
        .unwrap_or_default();
    snapshot.insert(
        "oauth".to_string(),
        build_provider_key_oauth_status_snapshot(key),
    );
    snapshot.insert(
        "account".to_string(),
        build_provider_key_account_status_snapshot(key, provider_type),
    );
    Value::Object(snapshot)
}

pub(crate) fn provider_key_health_summary(
    key: &StoredProviderCatalogKey,
) -> (
    f64,
    i64,
    Option<String>,
    bool,
    serde_json::Map<String, serde_json::Value>,
) {
    provider_key_health_summary_with_circuit_predicate(key, |value| {
        value
            .get("open")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
    })
}

pub(crate) fn provider_key_health_summary_at(
    key: &StoredProviderCatalogKey,
    now_unix_secs: u64,
) -> (
    f64,
    i64,
    Option<String>,
    bool,
    serde_json::Map<String, serde_json::Value>,
) {
    provider_key_health_summary_with_circuit_predicate(key, |value| {
        provider_key_circuit_payload_is_active_open_at(value, now_unix_secs)
    })
}

fn provider_key_health_summary_with_circuit_predicate(
    key: &StoredProviderCatalogKey,
    circuit_is_open: impl Fn(&serde_json::Value) -> bool,
) -> (
    f64,
    i64,
    Option<String>,
    bool,
    serde_json::Map<String, serde_json::Value>,
) {
    let health_by_format = key
        .health_by_format
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default();
    let circuit_by_format = key
        .circuit_breaker_by_format
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default();

    let mut min_health_score = 1.0_f64;
    let mut max_consecutive = 0_i64;
    let mut last_failure_at: Option<String> = None;
    for value in health_by_format.values() {
        let score = value
            .get("health_score")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(1.0);
        min_health_score = min_health_score.min(score);
        let consecutive = value
            .get("consecutive_failures")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0);
        max_consecutive = max_consecutive.max(consecutive);
        if let Some(last_failure) = value
            .get("last_failure_at")
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned)
        {
            if last_failure_at
                .as_ref()
                .is_none_or(|current| last_failure > *current)
            {
                last_failure_at = Some(last_failure);
            }
        }
    }

    let any_circuit_open = circuit_by_format.values().any(circuit_is_open);

    (
        if health_by_format.is_empty() {
            1.0
        } else {
            min_health_score
        },
        max_consecutive,
        last_failure_at,
        any_circuit_open,
        circuit_by_format,
    )
}

fn normalize_catalog_oauth_plan_type(value: &str, provider_type: &str) -> Option<String> {
    let mut normalized = value.trim().to_string();
    if normalized.is_empty() {
        return None;
    }

    let provider_type = provider_type.trim().to_ascii_lowercase();
    if !provider_type.is_empty() && normalized.to_ascii_lowercase().starts_with(&provider_type) {
        normalized = normalized[provider_type.len()..]
            .trim_matches(|ch: char| [' ', ':', '-', '_'].contains(&ch))
            .to_string();
    }

    let normalized = normalized.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

fn catalog_oauth_plan_type_from_source(
    source: &serde_json::Map<String, serde_json::Value>,
    provider_type: &str,
    fields: &[&str],
) -> Option<String> {
    for field in fields {
        let Some(value) = source.get(*field).and_then(serde_json::Value::as_str) else {
            continue;
        };
        if let Some(normalized) = normalize_catalog_oauth_plan_type(value, provider_type) {
            return Some(normalized);
        }
    }
    None
}

fn derive_catalog_oauth_plan_type(
    key: &StoredProviderCatalogKey,
    provider_type: &str,
    auth_config: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Option<String> {
    if !provider_key_auth_semantics(key, provider_type).oauth_managed() {
        return None;
    }

    let provider_type_key = provider_type.trim().to_ascii_lowercase();
    if let Some(upstream_metadata) = key
        .upstream_metadata
        .as_ref()
        .and_then(serde_json::Value::as_object)
    {
        let provider_bucket = if provider_type_key.is_empty() {
            None
        } else {
            upstream_metadata
                .get(&provider_type_key)
                .and_then(serde_json::Value::as_object)
        };
        for source in provider_bucket
            .into_iter()
            .chain(std::iter::once(upstream_metadata))
        {
            if let Some(plan_type) = catalog_oauth_plan_type_from_source(
                source,
                provider_type,
                &[
                    "plan_type",
                    "tier",
                    "subscription_title",
                    "subscription_plan",
                    "plan",
                ],
            ) {
                return Some(plan_type);
            }
        }
    }

    auth_config.and_then(|source| {
        catalog_oauth_plan_type_from_source(
            source,
            provider_type,
            &["plan_type", "tier", "plan", "subscription_plan"],
        )
    })
}

pub(crate) fn build_admin_provider_key_response(
    state: &AppState,
    key: &StoredProviderCatalogKey,
    provider_type: &str,
    api_formats: &[String],
    now_unix_secs: u64,
) -> serde_json::Value {
    let request_count = u64::from(key.request_count.unwrap_or(0));
    let success_count = u64::from(key.success_count.unwrap_or(0));
    let error_count = u64::from(key.error_count.unwrap_or(0));
    let total_response_time_ms = f64::from(key.total_response_time_ms.unwrap_or(0));
    let success_rate = if request_count > 0 {
        success_count as f64 / request_count as f64
    } else {
        0.0
    };
    let avg_response_time_ms = if success_count > 0 {
        total_response_time_ms / success_count as f64
    } else {
        0.0
    };
    let auth_semantics = provider_key_auth_semantics(key, provider_type);
    let auth_config = parse_catalog_auth_config_json(state, key);
    let oauth_organizations = if auth_semantics.can_show_oauth_metadata() {
        auth_config
            .as_ref()
            .and_then(|config| config.get("organizations"))
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let oauth_temporary = auth_semantics.can_show_oauth_metadata()
        && auth_config
            .as_ref()
            .and_then(|config| config.get("access_token_import_temporary"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
    let oauth_plan_type = derive_catalog_oauth_plan_type(key, provider_type, auth_config.as_ref());
    let (
        health_score,
        consecutive_failures,
        last_failure_at,
        circuit_breaker_open,
        circuit_by_format,
    ) = provider_key_health_summary_at(key, now_unix_secs);
    let circuit_sample = circuit_by_format
        .values()
        .find(|value| provider_key_circuit_payload_is_active_open_at(value, now_unix_secs))
        .or_else(|| circuit_by_format.values().next());
    let is_adaptive = key.rpm_limit.is_none();
    let effective_limit = if is_adaptive {
        key.learned_rpm_limit
    } else {
        key.rpm_limit
    };
    let mut payload = serde_json::Map::new();
    payload.insert("id".to_string(), json!(key.id));
    payload.insert("provider_id".to_string(), json!(key.provider_id));
    payload.insert(
        "api_formats".to_string(),
        serde_json::Value::Array(
            api_formats
                .iter()
                .cloned()
                .map(serde_json::Value::String)
                .collect(),
        ),
    );
    payload.insert(
        "api_key_masked".to_string(),
        json!(masked_catalog_api_key(state, key)),
    );
    payload.insert("api_key_plain".to_string(), serde_json::Value::Null);
    payload.insert("auth_type".to_string(), json!(key.auth_type));
    payload.insert(
        "auth_type_by_format".to_string(),
        json!(key.auth_type_by_format),
    );
    payload.insert(
        "allow_auth_channel_mismatch_formats".to_string(),
        json!(key.allow_auth_channel_mismatch_formats),
    );
    payload.insert(
        "credential_kind".to_string(),
        json!(auth_semantics.credential_kind().as_str()),
    );
    payload.insert(
        "runtime_auth_kind".to_string(),
        json!(auth_semantics.runtime_auth_kind().as_str()),
    );
    payload.insert(
        "oauth_managed".to_string(),
        json!(auth_semantics.oauth_managed()),
    );
    payload.insert(
        "can_refresh_oauth".to_string(),
        json!(provider_key_can_refresh_oauth(
            auth_semantics,
            auth_config.as_ref()
        )),
    );
    payload.insert(
        "can_export_oauth".to_string(),
        json!(auth_semantics.can_export_oauth()),
    );
    payload.insert(
        "can_edit_oauth".to_string(),
        json!(auth_semantics.can_edit_oauth()),
    );
    payload.insert("name".to_string(), json!(key.name));
    payload.insert("rate_multipliers".to_string(), json!(key.rate_multipliers));
    payload.insert(
        "internal_priority".to_string(),
        json!(key.internal_priority),
    );
    payload.insert(
        "global_priority_by_format".to_string(),
        json!(key.global_priority_by_format),
    );
    payload.insert("rpm_limit".to_string(), json!(key.rpm_limit));
    payload.insert("concurrent_limit".to_string(), json!(key.concurrent_limit));
    payload.insert(
        "allowed_models".to_string(),
        serde_json::Value::Array(
            json_string_list(key.allowed_models.as_ref())
                .into_iter()
                .map(serde_json::Value::String)
                .collect(),
        ),
    );
    payload.insert("capabilities".to_string(), json!(key.capabilities));
    payload.insert(
        "oauth_expires_at".to_string(),
        json!(auth_semantics
            .can_show_oauth_metadata()
            .then_some(key.expires_at_unix_secs)
            .flatten()),
    );
    payload.insert(
        "oauth_email".to_string(),
        if auth_semantics.can_show_oauth_metadata() {
            auth_config
                .as_ref()
                .and_then(|config| config.get("email"))
                .cloned()
                .unwrap_or(serde_json::Value::Null)
        } else {
            serde_json::Value::Null
        },
    );
    payload.insert("oauth_plan_type".to_string(), json!(oauth_plan_type));
    payload.insert(
        "oauth_account_id".to_string(),
        if auth_semantics.can_show_oauth_metadata() {
            auth_config
                .as_ref()
                .and_then(|config| config.get("account_id"))
                .cloned()
                .unwrap_or(serde_json::Value::Null)
        } else {
            serde_json::Value::Null
        },
    );
    payload.insert(
        "oauth_account_name".to_string(),
        if auth_semantics.can_show_oauth_metadata() {
            auth_config
                .as_ref()
                .and_then(|config| config.get("account_name"))
                .cloned()
                .unwrap_or(serde_json::Value::Null)
        } else {
            serde_json::Value::Null
        },
    );
    payload.insert(
        "oauth_account_user_id".to_string(),
        if auth_semantics.can_show_oauth_metadata() {
            auth_config
                .as_ref()
                .and_then(|config| config.get("account_user_id"))
                .cloned()
                .unwrap_or(serde_json::Value::Null)
        } else {
            serde_json::Value::Null
        },
    );
    payload.insert(
        "oauth_organizations".to_string(),
        serde_json::Value::Array(oauth_organizations),
    );
    payload.insert("oauth_temporary".to_string(), json!(oauth_temporary));
    payload.insert(
        "oauth_invalid_at".to_string(),
        json!(auth_semantics
            .can_show_oauth_metadata()
            .then_some(key.oauth_invalid_at_unix_secs)
            .flatten()),
    );
    payload.insert(
        "oauth_invalid_reason".to_string(),
        json!(auth_semantics
            .can_show_oauth_metadata()
            .then_some(key.oauth_invalid_reason.clone())
            .flatten()),
    );
    payload.insert(
        "status_snapshot".to_string(),
        provider_key_status_snapshot_payload(key, provider_type),
    );
    payload.insert(
        "cache_ttl_minutes".to_string(),
        json!(key.cache_ttl_minutes),
    );
    payload.insert(
        "max_probe_interval_minutes".to_string(),
        json!(key.max_probe_interval_minutes),
    );
    payload.insert("health_by_format".to_string(), json!(key.health_by_format));
    payload.insert(
        "circuit_breaker_by_format".to_string(),
        json!(key.circuit_breaker_by_format),
    );
    payload.insert("health_score".to_string(), json!(health_score));
    payload.insert(
        "consecutive_failures".to_string(),
        json!(consecutive_failures),
    );
    payload.insert("last_failure_at".to_string(), json!(last_failure_at));
    payload.insert(
        "circuit_breaker_open".to_string(),
        json!(circuit_breaker_open),
    );
    payload.insert(
        "circuit_breaker_open_at".to_string(),
        circuit_sample
            .and_then(|value| value.get("open_at"))
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    );
    payload.insert(
        "next_probe_at".to_string(),
        circuit_sample
            .and_then(|value| value.get("next_probe_at"))
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    );
    payload.insert(
        "half_open_until".to_string(),
        circuit_sample
            .and_then(|value| value.get("half_open_until"))
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    );
    payload.insert(
        "half_open_successes".to_string(),
        json!(circuit_sample
            .and_then(|value| value.get("half_open_successes"))
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0)),
    );
    payload.insert(
        "half_open_failures".to_string(),
        json!(circuit_sample
            .and_then(|value| value.get("half_open_failures"))
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0)),
    );
    payload.insert(
        "request_results_window".to_string(),
        circuit_sample
            .and_then(|value| value.get("request_results_window"))
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    );
    payload.insert("request_count".to_string(), json!(request_count));
    payload.insert("success_count".to_string(), json!(success_count));
    payload.insert("error_count".to_string(), json!(error_count));
    payload.insert("success_rate".to_string(), json!(success_rate));
    payload.insert(
        "avg_response_time_ms".to_string(),
        json!(avg_response_time_ms),
    );
    payload.insert("is_active".to_string(), json!(key.is_active));
    payload.insert("is_adaptive".to_string(), json!(is_adaptive));
    payload.insert(
        "learned_rpm_limit".to_string(),
        json!(key.learned_rpm_limit),
    );
    payload.insert("effective_limit".to_string(), json!(effective_limit));
    payload.insert(
        "utilization_samples".to_string(),
        json!(key.utilization_samples),
    );
    payload.insert(
        "last_probe_increase_at".to_string(),
        json!(key
            .last_probe_increase_at_unix_secs
            .and_then(unix_secs_to_rfc3339)),
    );
    payload.insert(
        "concurrent_429_count".to_string(),
        json!(key.concurrent_429_count),
    );
    payload.insert("rpm_429_count".to_string(), json!(key.rpm_429_count));
    payload.insert(
        "last_429_at".to_string(),
        json!(key.last_429_at_unix_secs.and_then(unix_secs_to_rfc3339)),
    );
    payload.insert("last_429_type".to_string(), json!(key.last_429_type));
    payload.insert("note".to_string(), json!(key.note));
    payload.insert(
        "auto_fetch_models".to_string(),
        json!(key.auto_fetch_models),
    );
    payload.insert(
        "last_models_fetch_at".to_string(),
        json!(key
            .last_models_fetch_at_unix_secs
            .and_then(unix_secs_to_rfc3339)),
    );
    payload.insert(
        "last_models_fetch_error".to_string(),
        json!(key.last_models_fetch_error),
    );
    payload.insert("locked_models".to_string(), json!(key.locked_models));
    payload.insert(
        "model_include_patterns".to_string(),
        json!(key.model_include_patterns),
    );
    payload.insert(
        "model_exclude_patterns".to_string(),
        json!(key.model_exclude_patterns),
    );
    payload.insert(
        "upstream_metadata".to_string(),
        json!(key.upstream_metadata),
    );
    payload.insert("proxy".to_string(), json!(key.proxy));
    payload.insert("fingerprint".to_string(), json!(key.fingerprint));
    payload.insert(
        "last_used_at".to_string(),
        json!(key.last_used_at_unix_secs.and_then(unix_secs_to_rfc3339)),
    );
    payload.insert(
        "created_at".to_string(),
        json!(unix_secs_to_rfc3339(
            key.created_at_unix_ms.unwrap_or(now_unix_secs)
        )),
    );
    payload.insert(
        "updated_at".to_string(),
        json!(unix_secs_to_rfc3339(
            key.updated_at_unix_secs.unwrap_or(now_unix_secs)
        )),
    );
    serde_json::Value::Object(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_catalog_key() -> StoredProviderCatalogKey {
        let encrypted_api_key =
            encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "sk-test-123")
                .expect("api key ciphertext should build");
        StoredProviderCatalogKey::new(
            "key-test".to_string(),
            "provider-test".to_string(),
            "default".to_string(),
            "api_key".to_string(),
            None,
            true,
        )
        .expect("key should build")
        .with_transport_fields(
            Some(json!(["openai:chat"])),
            encrypted_api_key,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("key transport should build")
    }

    #[test]
    fn masked_catalog_api_key_handles_unicode_plaintext_without_panicking() {
        let state = AppState::new().expect("gateway should build");
        let encrypted_api_key =
            encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "测试-密钥-1234567890")
                .expect("api key ciphertext should build");
        let key = StoredProviderCatalogKey::new(
            "key-unicode".to_string(),
            "provider-test".to_string(),
            "default".to_string(),
            "api_key".to_string(),
            None,
            true,
        )
        .expect("key should build")
        .with_transport_fields(
            Some(json!(["openai:chat"])),
            encrypted_api_key,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("key transport should build");

        let masked = masked_catalog_api_key(&state, &key);
        assert!(masked.contains("***"));
        assert_ne!(masked, "***ERROR***");
    }

    #[test]
    fn provider_key_status_snapshot_payload_backfills_missing_quota_from_upstream_metadata() {
        let mut key = sample_catalog_key();
        key.upstream_metadata = Some(json!({
            "codex": {
                "updated_at": 1_775_553_285u64,
                "plan_type": "plus",
                "primary_used_percent": 55.0,
                "primary_reset_at": 1_900_000_000u64,
                "secondary_used_percent": 12.5,
                "secondary_reset_at": 1_900_500_000u64,
                "has_credits": true,
                "credits_balance": 42.0
            }
        }));

        let payload = provider_key_status_snapshot_payload(&key, "codex");
        let quota = payload
            .get("quota")
            .and_then(Value::as_object)
            .expect("quota snapshot should be object");

        assert_eq!(quota.get("provider_type"), Some(&json!("codex")));
        assert_eq!(quota.get("plan_type"), Some(&json!("plus")));
        assert_eq!(quota.get("updated_at"), Some(&json!(1_775_553_285u64)));
        assert_eq!(quota.get("reset_at"), Some(&json!(1_900_000_000u64)));
        assert_eq!(
            quota
                .get("credits")
                .and_then(Value::as_object)
                .and_then(|credits| credits.get("balance")),
            Some(&json!(42.0))
        );
        assert_eq!(
            quota.get("windows").and_then(Value::as_array).map(Vec::len),
            Some(2usize)
        );
    }

    #[test]
    fn provider_key_status_snapshot_payload_backfills_codex_spark_windows() {
        let mut key = sample_catalog_key();
        key.upstream_metadata = Some(json!({
            "codex": {
                "updated_at": 1_775_553_285u64,
                "plan_type": "plus",
                "primary_used_percent": 55.0,
                "primary_reset_at": 1_900_000_000u64,
                "secondary_used_percent": 12.5,
                "secondary_reset_at": 1_900_500_000u64,
                "spark_primary_used_percent": 40.0,
                "spark_primary_reset_at": 1_900_100_000u64,
                "spark_primary_window_minutes": 300u64,
                "spark_secondary_used_percent": 5.0,
                "spark_secondary_reset_at": 1_900_600_000u64,
                "spark_secondary_window_minutes": 10_080u64
            }
        }));

        let payload = provider_key_status_snapshot_payload(&key, "codex");
        let windows = payload["quota"]["windows"]
            .as_array()
            .expect("quota windows should exist");
        let spark_5h = windows
            .iter()
            .filter_map(Value::as_object)
            .find(|window| window.get("code") == Some(&json!("spark_5h")))
            .expect("Spark 5H window should exist");
        let spark_weekly = windows
            .iter()
            .filter_map(Value::as_object)
            .find(|window| window.get("code") == Some(&json!("spark_weekly")))
            .expect("Spark weekly window should exist");

        assert_eq!(windows.len(), 4);
        assert_eq!(spark_5h.get("label"), Some(&json!("Spark 5H")));
        assert_eq!(spark_5h.get("remaining_ratio"), Some(&json!(0.6)));
        assert_eq!(spark_weekly.get("label"), Some(&json!("Spark 周")));
        assert_eq!(spark_weekly.get("remaining_ratio"), Some(&json!(0.95)));
    }

    #[test]
    fn provider_key_status_snapshot_payload_keeps_codex_free_window_quota_available() {
        let mut key = sample_catalog_key();
        key.upstream_metadata = Some(json!({
            "codex": {
                "updated_at": 1_775_553_285u64,
                "plan_type": "free",
                "primary_used_percent": 64.0,
                "primary_reset_at": 1_900_000_000u64,
                "secondary_used_percent": 3.0,
                "secondary_reset_at": 1_900_500_000u64,
                "has_credits": false,
                "credits_balance": 0.0,
                "credits_unlimited": false
            }
        }));

        let payload = provider_key_status_snapshot_payload(&key, "codex");
        let quota = payload
            .get("quota")
            .and_then(Value::as_object)
            .expect("quota snapshot should be object");

        assert_eq!(quota.get("code"), Some(&json!("ok")));
        assert_eq!(quota.get("exhausted"), Some(&json!(false)));
        assert_eq!(quota.get("usage_ratio"), Some(&json!(0.64)));
        assert_eq!(
            quota
                .get("credits")
                .and_then(Value::as_object)
                .and_then(|credits| credits.get("has_credits")),
            Some(&json!(false))
        );
        assert_eq!(
            quota.get("windows").and_then(Value::as_array).map(Vec::len),
            Some(2usize)
        );
    }

    #[test]
    fn provider_key_status_snapshot_payload_derives_codex_reset_at_from_countdown() {
        let mut key = sample_catalog_key();
        key.upstream_metadata = Some(json!({
            "codex": {
                "updated_at": 1_775_553_285u64,
                "primary_used_percent": 55.0,
                "primary_reset_after_seconds": 3_600u64
            }
        }));

        let payload = provider_key_status_snapshot_payload(&key, "codex");
        let quota = payload
            .get("quota")
            .and_then(Value::as_object)
            .expect("quota snapshot should be object");
        let window = quota
            .get("windows")
            .and_then(Value::as_array)
            .and_then(|windows| windows.first())
            .and_then(Value::as_object)
            .expect("quota window should exist");

        assert_eq!(quota.get("reset_at"), Some(&json!(1_775_556_885u64)));
        assert_eq!(quota.get("reset_seconds"), Some(&json!(3_600u64)));
        assert_eq!(window.get("reset_at"), Some(&json!(1_775_556_885u64)));
        assert_eq!(window.get("reset_seconds"), Some(&json!(3_600u64)));
    }

    #[test]
    fn provider_key_status_snapshot_payload_backfills_chatgpt_web_image_quota() {
        let mut key = sample_catalog_key();
        key.upstream_metadata = Some(json!({
            "chatgpt_web": {
                "updated_at": 1_778_067_246u64,
                "plan_type": "free",
                "image_quota_remaining": 24.0,
                "image_quota_reset_at": 1_778_157_172u64
            }
        }));

        let payload = provider_key_status_snapshot_payload(&key, "chatgpt_web");
        let quota = payload
            .get("quota")
            .and_then(Value::as_object)
            .expect("quota snapshot should be object");
        let window = quota
            .get("windows")
            .and_then(Value::as_array)
            .and_then(|windows| windows.first())
            .and_then(Value::as_object)
            .expect("image quota window should exist");

        assert_eq!(quota.get("provider_type"), Some(&json!("chatgpt_web")));
        assert_eq!(quota.get("code"), Some(&json!("ok")));
        assert_eq!(quota.get("plan_type"), Some(&json!("free")));
        assert_eq!(quota.get("reset_at"), Some(&json!(1_778_157_172u64)));
        assert_eq!(quota.get("usage_ratio"), Some(&json!(0.0)));
        assert_eq!(window.get("code"), Some(&json!("image_gen")));
        assert_eq!(window.get("remaining_value"), Some(&json!(24.0)));
        assert_eq!(window.get("limit_value"), Some(&json!(24.0)));
        assert_eq!(window.get("used_value"), Some(&json!(0.0)));
        assert_eq!(window.get("remaining_ratio"), Some(&json!(1.0)));
    }

    #[test]
    fn provider_key_status_snapshot_payload_ignores_chatgpt_web_legacy_free_25_limit() {
        let mut key = sample_catalog_key();
        key.upstream_metadata = Some(json!({
            "chatgpt_web": {
                "updated_at": 1_778_067_246u64,
                "plan_type": "free",
                "image_quota_remaining": 19.0,
                "image_quota_total": 25.0
            }
        }));

        let payload = provider_key_status_snapshot_payload(&key, "chatgpt_web");
        let window = payload
            .get("quota")
            .and_then(Value::as_object)
            .and_then(|quota| quota.get("windows"))
            .and_then(Value::as_array)
            .and_then(|windows| windows.first())
            .and_then(Value::as_object)
            .expect("image quota window should exist");

        assert_eq!(window.get("remaining_value"), Some(&json!(19.0)));
        assert_eq!(window.get("limit_value"), Some(&json!(19.0)));
        assert_eq!(window.get("used_value"), Some(&json!(0.0)));
    }

    #[test]
    fn provider_key_status_snapshot_payload_backfills_grok_model_quota() {
        let mut key = sample_catalog_key();
        key.upstream_metadata = Some(json!({
            "grok": {
                "updated_at": 1_778_067_246u64,
                "pool_tier": "heavy",
                "plan_type": "heavy",
                "quota_by_model": {
                    "quota_auto": {
                        "display_name": "auto",
                        "remaining_fraction": 0.4,
                        "used_percent": 60.0,
                        "remaining": 60.0,
                        "total": 150.0,
                        "reset_at": 1_778_157_172u64,
                        "is_exhausted": false
                    },
                    "quota_heavy": {
                        "display_name": "heavy",
                        "remaining_fraction": 0.0,
                        "used_percent": 100.0,
                        "reset_at": 1_778_157_172u64,
                        "is_exhausted": true
                    }
                }
            }
        }));

        let payload = provider_key_status_snapshot_payload(&key, "grok");
        let quota = payload
            .get("quota")
            .and_then(Value::as_object)
            .expect("quota snapshot should be object");
        let windows = quota
            .get("windows")
            .and_then(Value::as_array)
            .expect("grok quota windows should exist");

        assert_eq!(quota.get("provider_type"), Some(&json!("grok")));
        assert_eq!(quota.get("code"), Some(&json!("ok")));
        assert_eq!(quota.get("plan_type"), Some(&json!("heavy")));
        assert_eq!(quota.get("pool_tier"), Some(&json!("heavy")));
        assert_eq!(quota.get("exhausted"), Some(&json!(false)));
        assert_eq!(quota.get("usage_ratio"), Some(&json!(1.0)));
        assert_eq!(quota.get("reset_at"), Some(&json!(1_778_157_172u64)));
        assert_eq!(windows.len(), 2);
        assert!(windows.iter().any(|window| {
            window
                .get("code")
                .and_then(Value::as_str)
                .is_some_and(|code| code == "model:quota_auto")
        }));
        let auto = windows
            .iter()
            .filter_map(Value::as_object)
            .find(|window| window.get("code") == Some(&json!("model:quota_auto")))
            .expect("auto quota window should exist");
        assert_eq!(auto.get("remaining_value"), Some(&json!(60.0)));
        assert_eq!(auto.get("limit_value"), Some(&json!(150.0)));
        assert_eq!(auto.get("used_value"), Some(&json!(90.0)));
    }

    #[test]
    fn provider_key_status_snapshot_payload_backfills_gemini_cli_account_credits() {
        let mut key = sample_catalog_key();
        key.upstream_metadata = Some(json!({
            "gemini_cli": {
                "updated_at": 1_778_067_246u64,
                "plan_type": "g1-pro-tier",
                "paidTier": {
                    "availableCredits": 123.5,
                    "consumedCredits": 7.0,
                    "totalCredits": 200.0
                },
                "quota_by_model": {
                    "gemini-2.5-pro": {
                        "display_name": "Gemini 2.5 Pro",
                        "remaining_fraction": 0.75,
                        "is_exhausted": false
                    }
                }
            }
        }));

        let payload = provider_key_status_snapshot_payload(&key, "gemini_cli");
        let quota = payload
            .get("quota")
            .and_then(Value::as_object)
            .expect("quota snapshot should be object");
        let credits = quota
            .get("credits")
            .and_then(Value::as_object)
            .expect("credits snapshot should exist");
        let windows = quota
            .get("windows")
            .and_then(Value::as_array)
            .expect("Gemini CLI model windows should exist");

        assert_eq!(quota.get("provider_type"), Some(&json!("gemini_cli")));
        assert_eq!(quota.get("code"), Some(&json!("ok")));
        assert_eq!(quota.get("plan_type"), Some(&json!("g1-pro-tier")));
        assert_eq!(quota.get("updated_at"), Some(&json!(1_778_067_246u64)));
        assert_eq!(credits.get("remaining"), Some(&json!(123.5)));
        assert_eq!(credits.get("balance"), Some(&json!(123.5)));
        assert_eq!(credits.get("consumed"), Some(&json!(7.0)));
        assert_eq!(credits.get("total"), Some(&json!(200.0)));
        assert_eq!(credits.get("has_credits"), Some(&json!(true)));
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].get("remaining_ratio"), Some(&json!(0.75)));
    }

    #[test]
    fn provider_key_status_snapshot_payload_backfills_windsurf_daily_and_weekly_quota() {
        let mut key = sample_catalog_key();
        key.upstream_metadata = Some(json!({
            "windsurf": {
                "updated_at": 1_778_067_246u64,
                "plan_name": "Pro",
                "daily_remaining_percent": 40.0,
                "weekly_remaining_percent": 65.0,
                "daily_reset_at": 1_778_100_000u64,
                "weekly_reset_at": 1_778_600_000u64,
                "prompt_used": 12.0,
                "prompt_limit": 100.0,
                "prompt_remaining": 88.0,
                "flex_used": 3.0,
                "flex_limit": 10.0,
                "flex_remaining": 7.0,
                "allowed_models_count": 82
            }
        }));

        let payload = provider_key_status_snapshot_payload(&key, "windsurf");
        let quota = payload
            .get("quota")
            .and_then(Value::as_object)
            .expect("quota snapshot should be object");
        let windows = quota
            .get("windows")
            .and_then(Value::as_array)
            .expect("windsurf quota windows should exist");
        let daily = windows
            .iter()
            .filter_map(Value::as_object)
            .find(|window| window.get("code") == Some(&json!("daily")))
            .expect("daily quota window should exist");
        let weekly = windows
            .iter()
            .filter_map(Value::as_object)
            .find(|window| window.get("code") == Some(&json!("weekly")))
            .expect("weekly quota window should exist");

        assert_eq!(quota.get("provider_type"), Some(&json!("windsurf")));
        assert_eq!(quota.get("code"), Some(&json!("ok")));
        assert_eq!(quota.get("plan_type"), Some(&json!("Pro")));
        assert_eq!(quota.get("usage_ratio"), Some(&json!(0.6)));
        assert_eq!(quota.get("reset_at"), Some(&json!(1_778_100_000u64)));
        assert_eq!(daily.get("remaining_ratio"), Some(&json!(0.4)));
        assert_eq!(daily.get("used_ratio"), Some(&json!(0.6)));
        assert_eq!(daily.get("reset_seconds"), Some(&json!(32_754u64)));
        assert_eq!(weekly.get("remaining_ratio"), Some(&json!(0.65)));
        assert_eq!(weekly.get("used_ratio"), Some(&json!(0.35)));
        assert_eq!(weekly.get("reset_seconds"), Some(&json!(532_754u64)));
        assert_eq!(quota.get("allowed_models_count"), Some(&json!(82)));
    }

    #[test]
    fn provider_key_status_snapshot_payload_treats_windsurf_rate_limit_as_cooldown() {
        let mut key = sample_catalog_key();
        key.upstream_metadata = Some(json!({
            "windsurf": {
                "updated_at": 1_778_067_246u64,
                "daily_remaining_percent": 80.0,
                "rate_limit": {
                    "limited": true,
                    "retry_after_ms": 60_001u64,
                    "message": "slow down"
                },
                "last_error": "slow down"
            }
        }));

        let payload = provider_key_status_snapshot_payload(&key, "windsurf");
        let quota = payload
            .get("quota")
            .and_then(Value::as_object)
            .expect("quota snapshot should be object");
        let rate_window = quota
            .get("windows")
            .and_then(Value::as_array)
            .and_then(|windows| {
                windows
                    .iter()
                    .filter_map(Value::as_object)
                    .find(|window| window.get("code") == Some(&json!("rate_limit")))
            })
            .expect("rate limit window should exist");

        assert_eq!(quota.get("code"), Some(&json!("cooldown")));
        assert_eq!(quota.get("exhausted"), Some(&json!(false)));
        assert_eq!(quota.get("reset_seconds"), Some(&json!(61u64)));
        assert_eq!(rate_window.get("is_exhausted"), Some(&json!(false)));
        assert_eq!(rate_window.get("reset_seconds"), Some(&json!(61u64)));
    }

    #[test]
    fn provider_key_status_snapshot_payload_keeps_windsurf_capacity_probe_without_retry_after_ok() {
        let mut key = sample_catalog_key();
        key.upstream_metadata = Some(json!({
            "windsurf": {
                "updated_at": 1_778_067_246u64,
                "daily_remaining_percent": 100.0,
                "weekly_remaining_percent": 100.0,
                "rate_limit": {
                    "limited": true,
                    "has_capacity": false,
                    "messages_remaining": 0.0,
                    "max_messages": 100.0
                }
            }
        }));

        let payload = provider_key_status_snapshot_payload(&key, "windsurf");
        let quota = payload
            .get("quota")
            .and_then(Value::as_object)
            .expect("quota snapshot should be object");
        let has_rate_limit_window =
            quota
                .get("windows")
                .and_then(Value::as_array)
                .is_some_and(|windows| {
                    windows
                        .iter()
                        .filter_map(Value::as_object)
                        .any(|window| window.get("code") == Some(&json!("rate_limit")))
                });

        assert_eq!(quota.get("code"), Some(&json!("ok")));
        assert_eq!(quota.get("label"), Some(&Value::Null));
        assert_eq!(quota.get("exhausted"), Some(&json!(false)));
        assert_eq!(
            payload.pointer("/quota/rate_limit/limited"),
            Some(&json!(true))
        );
        assert!(!has_rate_limit_window);
    }

    #[test]
    fn provider_key_status_snapshot_payload_refreshes_stale_windsurf_cooldown_when_probe_has_capacity(
    ) {
        let mut key = sample_catalog_key();
        key.status_snapshot = Some(json!({
            "quota": {
                "version": 2,
                "provider_type": "windsurf",
                "code": "cooldown",
                "label": "冷却中",
                "exhausted": false,
                "windows": [
                    {
                        "code": "daily",
                        "unit": "percent",
                        "label": "日",
                        "scope": "account",
                        "remaining_ratio": 0.99,
                        "is_exhausted": false
                    },
                    {
                        "code": "rate_limit",
                        "unit": "count",
                        "label": "速率",
                        "scope": "account",
                        "is_exhausted": false,
                        "reset_seconds": null
                    }
                ],
                "rate_limit": {
                    "limited": true,
                    "has_capacity": true,
                    "messages_remaining": -1,
                    "max_messages": -1
                }
            }
        }));
        key.upstream_metadata = Some(json!({
            "windsurf": {
                "updated_at": 1_778_067_246u64,
                "daily_remaining_percent": 99.0,
                "weekly_remaining_percent": 100.0,
                "allowed_models_count": 118,
                "rate_limit": {
                    "limited": true,
                    "has_capacity": true,
                    "messages_remaining": -1,
                    "max_messages": -1
                }
            }
        }));

        let payload = provider_key_status_snapshot_payload(&key, "windsurf");
        let quota = payload
            .get("quota")
            .and_then(Value::as_object)
            .expect("quota snapshot should be object");
        let has_rate_limit_window =
            quota
                .get("windows")
                .and_then(Value::as_array)
                .is_some_and(|windows| {
                    windows
                        .iter()
                        .filter_map(Value::as_object)
                        .any(|window| window.get("code") == Some(&json!("rate_limit")))
                });

        assert_eq!(quota.get("code"), Some(&json!("ok")));
        assert_eq!(quota.get("label"), Some(&Value::Null));
        assert_eq!(quota.get("allowed_models_count"), Some(&json!(118u64)));
        assert!(!has_rate_limit_window);
    }

    #[test]
    fn provider_key_status_snapshot_payload_marks_windsurf_banned_and_quarantined_blocking() {
        let mut banned_key = sample_catalog_key();
        banned_key.upstream_metadata = Some(json!({
            "windsurf": {
                "updated_at": 1_778_067_246u64,
                "banned": true,
                "reason": "forbidden"
            }
        }));
        let banned_payload = provider_key_status_snapshot_payload(&banned_key, "windsurf");

        assert_eq!(
            banned_payload.pointer("/quota/code"),
            Some(&json!("banned"))
        );
        assert_eq!(
            banned_payload.pointer("/quota/exhausted"),
            Some(&json!(true))
        );
        assert_eq!(
            banned_payload.pointer("/account/code"),
            Some(&json!("account_banned"))
        );
        assert_eq!(
            banned_payload.pointer("/account/blocked"),
            Some(&json!(true))
        );

        let mut quarantined_key = sample_catalog_key();
        quarantined_key.upstream_metadata = Some(json!({
            "windsurf": {
                "updated_at": 1_778_067_246u64,
                "quarantined": true
            }
        }));
        let quarantined_payload =
            provider_key_status_snapshot_payload(&quarantined_key, "windsurf");

        assert_eq!(
            quarantined_payload.pointer("/quota/code"),
            Some(&json!("quarantined"))
        );
        assert_eq!(
            quarantined_payload.pointer("/quota/exhausted"),
            Some(&json!(true))
        );
        assert_eq!(
            quarantined_payload.pointer("/account/code"),
            Some(&json!("account_quarantined"))
        );
        assert_eq!(
            quarantined_payload.pointer("/account/blocked"),
            Some(&json!(true))
        );
    }

    #[test]
    fn provider_key_status_snapshot_payload_preserves_existing_materialized_quota_snapshot() {
        let mut key = sample_catalog_key();
        key.upstream_metadata = Some(json!({
            "codex": {
                "updated_at": 100u64,
                "primary_used_percent": 100.0
            }
        }));
        key.status_snapshot = Some(json!({
            "oauth": {
                "code": "none",
                "label": serde_json::Value::Null,
                "reason": serde_json::Value::Null,
                "expires_at": serde_json::Value::Null,
                "invalid_at": serde_json::Value::Null,
                "source": serde_json::Value::Null,
                "requires_reauth": false,
                "expiring_soon": false
            },
            "account": {
                "code": "ok",
                "label": serde_json::Value::Null,
                "reason": serde_json::Value::Null,
                "blocked": false,
                "source": serde_json::Value::Null,
                "recoverable": false
            },
            "quota": {
                "version": 2,
                "provider_type": "codex",
                "code": "ok",
                "label": serde_json::Value::Null,
                "reason": serde_json::Value::Null,
                "freshness": "fresh",
                "source": "refresh_api",
                "observed_at": 200u64,
                "exhausted": false,
                "usage_ratio": 0.25,
                "updated_at": 200u64,
                "reset_seconds": 3600u64,
                "plan_type": "team",
                "windows": [{
                    "code": "weekly",
                    "label": "周",
                    "scope": "account",
                    "unit": "percent",
                    "used_ratio": 0.25,
                    "remaining_ratio": 0.75,
                    "reset_at": 1_900_000_000u64,
                    "reset_seconds": 3600u64
                }]
            }
        }));

        let payload = provider_key_status_snapshot_payload(&key, "codex");
        let quota = payload
            .get("quota")
            .and_then(Value::as_object)
            .expect("quota snapshot should be object");

        assert_eq!(quota.get("updated_at"), Some(&json!(200u64)));
        assert_eq!(quota.get("plan_type"), Some(&json!("team")));
        assert_eq!(
            quota
                .get("windows")
                .and_then(Value::as_array)
                .and_then(|windows| windows.first())
                .and_then(Value::as_object)
                .and_then(|window| window.get("used_ratio")),
            Some(&json!(0.25))
        );
    }

    #[test]
    fn sync_provider_key_quota_status_snapshot_preserves_codex_usage_state() {
        let current_status_snapshot = json!({
            "quota": {
                "version": 2,
                "provider_type": "codex",
                "windows": [
                    {
                        "code": "weekly",
                        "usage_reset_at": 1_775_600_000u64,
                        "usage": {
                            "request_count": 3,
                            "total_tokens": 375,
                            "total_cost_usd": "0.60000000"
                        },
                        "reset_at": 1_900_000_000u64,
                        "window_minutes": 10_080u64
                    },
                    {
                        "code": "5h",
                        "usage_reset_at": 1_775_700_000u64,
                        "usage": {
                            "request_count": 2,
                            "total_tokens": 225,
                            "total_cost_usd": "0.30000000"
                        },
                        "reset_at": 1_900_500_000u64,
                        "window_minutes": 300u64
                    }
                ]
            }
        });
        let upstream_metadata = json!({
            "codex": {
                "updated_at": 1_775_800_000u64,
                "plan_type": "plus",
                "primary_used_percent": 5.0,
                "primary_reset_at": 1_900_000_000u64,
                "primary_window_minutes": 10_080u64,
                "secondary_used_percent": 1.0,
                "secondary_reset_at": 1_900_500_000u64,
                "secondary_window_minutes": 300u64
            }
        });

        let payload = sync_provider_key_quota_status_snapshot(
            Some(&current_status_snapshot),
            "codex",
            Some(&upstream_metadata),
            "refresh_api",
        )
        .expect("quota snapshot should sync");
        let windows = payload
            .get("quota")
            .and_then(Value::as_object)
            .and_then(|quota| quota.get("windows"))
            .and_then(Value::as_array)
            .expect("quota windows should exist");
        let weekly = windows
            .iter()
            .filter_map(Value::as_object)
            .find(|window| window.get("code") == Some(&json!("weekly")))
            .expect("weekly window should exist");
        let five_h = windows
            .iter()
            .filter_map(Value::as_object)
            .find(|window| window.get("code") == Some(&json!("5h")))
            .expect("5h window should exist");

        assert_eq!(weekly.get("usage_reset_at"), Some(&json!(1_775_600_000u64)));
        assert_eq!(
            weekly
                .get("usage")
                .and_then(|usage| usage.get("request_count")),
            Some(&json!(3))
        );
        assert_eq!(
            weekly
                .get("usage")
                .and_then(|usage| usage.get("total_tokens")),
            Some(&json!(375))
        );
        assert_eq!(
            weekly
                .get("usage")
                .and_then(|usage| usage.get("total_cost_usd")),
            Some(&json!("0.60000000"))
        );
        assert_eq!(five_h.get("usage_reset_at"), Some(&json!(1_775_700_000u64)));
        assert_eq!(
            five_h
                .get("usage")
                .and_then(|usage| usage.get("request_count")),
            Some(&json!(2))
        );
        assert_eq!(
            five_h
                .get("usage")
                .and_then(|usage| usage.get("total_tokens")),
            Some(&json!(225))
        );
        assert_eq!(
            five_h
                .get("usage")
                .and_then(|usage| usage.get("total_cost_usd")),
            Some(&json!("0.30000000"))
        );
    }

    #[test]
    fn sync_provider_key_quota_status_snapshot_drops_codex_usage_state_when_window_resets() {
        let current_status_snapshot = json!({
            "quota": {
                "version": 2,
                "provider_type": "codex",
                "windows": [
                    {
                        "code": "weekly",
                        "usage_reset_at": 1_775_600_000u64,
                        "usage": {
                            "request_count": 3,
                            "total_tokens": 375,
                            "total_cost_usd": "0.60000000"
                        },
                        "reset_at": 1_900_000_000u64,
                        "window_minutes": 10_080u64
                    }
                ]
            }
        });
        let upstream_metadata = json!({
            "codex": {
                "updated_at": 1_900_000_100u64,
                "plan_type": "plus",
                "primary_used_percent": 0.0,
                "primary_reset_at": 1_960_480_100u64,
                "primary_window_minutes": 10_080u64
            }
        });

        let payload = sync_provider_key_quota_status_snapshot(
            Some(&current_status_snapshot),
            "codex",
            Some(&upstream_metadata),
            "refresh_api",
        )
        .expect("quota snapshot should sync");
        let weekly = payload["quota"]["windows"]
            .as_array()
            .expect("quota windows should exist")
            .iter()
            .filter_map(Value::as_object)
            .find(|window| window.get("code") == Some(&json!("weekly")))
            .expect("weekly window should exist");

        assert_eq!(weekly.get("reset_at"), Some(&json!(1_960_480_100u64)));
        assert!(weekly.get("usage_reset_at").is_none());
        assert!(weekly.get("usage").is_none());
    }

    #[test]
    fn sync_provider_key_quota_status_snapshot_defaults_codex_window_minutes() {
        let upstream_metadata = json!({
            "codex": {
                "updated_at": 1_775_800_000u64,
                "plan_type": "plus",
                "primary_used_percent": 5.0,
                "primary_reset_at": 1_900_000_000u64,
                "secondary_used_percent": 1.0,
                "secondary_reset_at": 1_900_500_000u64
            }
        });

        let payload = sync_provider_key_quota_status_snapshot(
            None,
            "codex",
            Some(&upstream_metadata),
            "response_headers",
        )
        .expect("quota snapshot should sync");
        let windows = payload["quota"]["windows"]
            .as_array()
            .expect("quota windows should exist");
        let weekly = windows
            .iter()
            .filter_map(Value::as_object)
            .find(|window| window.get("code") == Some(&json!("weekly")))
            .expect("weekly window should exist");
        let five_h = windows
            .iter()
            .filter_map(Value::as_object)
            .find(|window| window.get("code") == Some(&json!("5h")))
            .expect("5h window should exist");

        assert_eq!(weekly.get("window_minutes"), Some(&json!(10_080u64)));
        assert_eq!(five_h.get("window_minutes"), Some(&json!(300u64)));
    }

    #[test]
    fn provider_key_status_snapshot_payload_backfills_thin_ok_snapshot_from_upstream_metadata() {
        let mut key = sample_catalog_key();
        key.upstream_metadata = Some(json!({
            "antigravity": {
                "updated_at": 1_775_553_285u64,
                "quota_by_model": {
                    "gemini-2.5-pro": { "used_percent": 0.0 },
                    "gemini-2.5-flash": { "used_percent": 25.0 }
                }
            }
        }));
        key.status_snapshot = Some(json!({
            "oauth": {
                "code": "none",
                "label": serde_json::Value::Null,
                "reason": serde_json::Value::Null,
                "expires_at": serde_json::Value::Null,
                "invalid_at": serde_json::Value::Null,
                "source": serde_json::Value::Null,
                "requires_reauth": false,
                "expiring_soon": false
            },
            "account": {
                "code": "ok",
                "label": serde_json::Value::Null,
                "reason": serde_json::Value::Null,
                "blocked": false,
                "source": serde_json::Value::Null,
                "recoverable": false
            },
            "quota": {
                "version": 2,
                "provider_type": "antigravity",
                "code": "ok",
                "label": serde_json::Value::Null,
                "reason": serde_json::Value::Null,
                "freshness": "fresh",
                "source": "refresh_api",
                "observed_at": 100u64,
                "exhausted": false,
                "usage_ratio": 0.0,
                "updated_at": 100u64,
                "reset_seconds": serde_json::Value::Null,
                "plan_type": serde_json::Value::Null
            }
        }));

        let payload = provider_key_status_snapshot_payload(&key, "antigravity");
        let quota = payload
            .get("quota")
            .and_then(Value::as_object)
            .expect("quota snapshot should be object");

        assert_eq!(quota.get("provider_type"), Some(&json!("antigravity")));
        assert_eq!(quota.get("updated_at"), Some(&json!(1_775_553_285u64)));
        assert_eq!(
            quota.get("windows").and_then(Value::as_array).map(Vec::len),
            Some(2usize)
        );
    }

    #[test]
    fn provider_key_status_snapshot_payload_backfills_account_block_from_oauth_invalid_reason() {
        let mut key = sample_catalog_key();
        key.oauth_invalid_reason = Some("[ACCOUNT_BLOCK] account has been deactivated".to_string());

        let payload = provider_key_status_snapshot_payload(&key, "codex");
        let account = payload
            .get("account")
            .and_then(Value::as_object)
            .expect("account snapshot should be object");

        assert_eq!(account.get("code"), Some(&json!("account_disabled")));
        assert_eq!(account.get("label"), Some(&json!("账号停用")));
        assert_eq!(
            account.get("reason"),
            Some(&json!("account has been deactivated"))
        );
        assert_eq!(account.get("blocked"), Some(&json!(true)));
        assert_eq!(account.get("source"), Some(&json!("oauth_invalid")));
    }

    #[test]
    fn provider_key_status_snapshot_payload_backfills_workspace_deactivated_from_metadata() {
        let mut key = sample_catalog_key();
        key.upstream_metadata = Some(json!({
            "codex": {
                "account_disabled": true,
                "reason": "deactivated_workspace"
            }
        }));

        let payload = provider_key_status_snapshot_payload(&key, "codex");
        let account = payload
            .get("account")
            .and_then(Value::as_object)
            .expect("account snapshot should be object");

        assert_eq!(account.get("code"), Some(&json!("workspace_deactivated")));
        assert_eq!(account.get("label"), Some(&json!("工作区停用")));
        assert_eq!(account.get("blocked"), Some(&json!(true)));
        assert_eq!(account.get("source"), Some(&json!("metadata")));
    }
}
