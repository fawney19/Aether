use crate::handlers::shared::{json_string_list, unix_secs_to_rfc3339};
use crate::provider_key_auth::{
    provider_key_auth_semantics, provider_key_configured_api_formats,
    provider_key_inherits_provider_api_formats,
};
use crate::AppState;
use aether_admin::provider::quota as admin_provider_quota_pure;
use aether_admin::provider::status as admin_provider_status_pure;
#[cfg(test)]
use aether_crypto::DEVELOPMENT_ENCRYPTION_KEY;
use aether_crypto::{decrypt_python_fernet_ciphertext, encrypt_python_fernet_plaintext};
use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;
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
                    if value.len() <= 12 {
                        format!("{value}***")
                    } else {
                        format!(
                            "{}***{}",
                            &value[..8],
                            &value[value.len().saturating_sub(4)..]
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
        return json!({
            "code": "invalid",
            "label": "已失效",
            "reason": reason,
            "expires_at": expires_at_unix_secs,
            "invalid_at": invalid_at_unix_secs,
            "source": "oauth_refresh",
            "requires_reauth": true,
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
            "reason": "Token 已过期，请重新授权",
            "expires_at": expires_at_unix_secs,
            "invalid_at": Value::Null,
            "source": "expires_at",
            "requires_reauth": true,
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

fn model_quota_window_snapshot(
    model_name: &str,
    item: &Map<String, Value>,
    observed_at_unix_secs: Option<u64>,
) -> Option<Value> {
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
    window.insert("reset_at".to_string(), json!(reset_at));
    window.insert("reset_seconds".to_string(), json!(reset_seconds));
    window.insert("is_exhausted".to_string(), json!(is_exhausted));
    Some(Value::Object(window))
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
    let window_minutes = metadata
        .get(&window_minutes_key)
        .and_then(admin_provider_quota_pure::coerce_json_u64);

    if used_percent.is_none()
        && reset_at.is_none()
        && reset_seconds.is_none()
        && window_minutes.is_none()
    {
        return None;
    }

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

    let usage_ratio = windows
        .iter()
        .filter_map(Value::as_object)
        .filter_map(|window| window.get("used_ratio"))
        .filter_map(Value::as_f64)
        .max_by(f64::total_cmp);
    let reset_seconds = windows
        .iter()
        .filter_map(Value::as_object)
        .filter_map(|window| window.get("reset_seconds"))
        .filter_map(admin_provider_quota_pure::coerce_json_u64)
        .min();
    let reset_at = quota_windows_min_reset_at(&windows);
    let exhausted_by_credits =
        windows.is_empty() && credits_unlimited != Some(true) && credits_has_credits == Some(false);
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

fn build_gemini_cli_quota_status_snapshot(
    upstream_metadata: Option<&Value>,
    source: &str,
) -> Option<Value> {
    let metadata = provider_quota_metadata_bucket(upstream_metadata, "gemini_cli")?;
    let observed_at_unix_secs = provider_quota_timestamp_unix_secs(metadata.get("updated_at"));
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

    if windows.is_empty() && observed_at_unix_secs.is_none() {
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
        "plan_type": serde_json::Value::Null,
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
    let quota = match normalized_provider_type.as_str() {
        "codex" => build_codex_quota_status_snapshot(upstream_metadata, source),
        "kiro" => build_kiro_quota_status_snapshot(upstream_metadata, source),
        "antigravity" => build_antigravity_quota_status_snapshot(upstream_metadata, source),
        "gemini_cli" => build_gemini_cli_quota_status_snapshot(upstream_metadata, source),
        _ => None,
    }?;

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

    let any_circuit_open = circuit_by_format.values().any(|value| {
        value
            .get("open")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
    });

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
    ) = provider_key_health_summary(key);
    let circuit_sample = circuit_by_format
        .values()
        .find(|value| {
            value
                .get("open")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
        })
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
        json!(auth_semantics.can_refresh_oauth()),
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
