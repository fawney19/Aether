use crate::handlers::admin::provider::shared::support::{
    AdminProviderPoolConfig, AdminProviderPoolRuntimeState,
};
use crate::handlers::admin::shared::{
    provider_key_status_snapshot_payload, unix_secs_to_rfc3339,
};
use crate::handlers::admin::request::AdminAppState;
use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;
use serde_json::json;

pub(super) fn admin_pool_api_formats(key: &StoredProviderCatalogKey) -> Vec<String> {
    key.api_formats
        .as_ref()
        .and_then(serde_json::Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn admin_pool_string_list(value: Option<&serde_json::Value>) -> Option<Vec<String>> {
    let values = value
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(serde_json::Value::as_str)
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

fn admin_pool_json_object(
    value: Option<&serde_json::Value>,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    value
        .and_then(serde_json::Value::as_object)
        .cloned()
        .filter(|value| !value.is_empty())
}

fn admin_pool_json_to_f64(value: Option<&serde_json::Value>) -> Option<f64> {
    let parsed = match value {
        Some(serde_json::Value::Number(number)) => number.as_f64(),
        Some(serde_json::Value::String(text)) => text.trim().parse::<f64>().ok(),
        _ => None,
    }?;
    if parsed.is_finite() {
        Some(parsed)
    } else {
        None
    }
}

fn admin_pool_json_to_u64(value: Option<&serde_json::Value>) -> Option<u64> {
    let mut parsed = match value {
        Some(serde_json::Value::Number(number)) => number.as_f64(),
        Some(serde_json::Value::String(text)) => text.trim().parse::<f64>().ok(),
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

fn admin_pool_trimmed_string(value: Option<&serde_json::Value>) -> Option<String> {
    value
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn admin_pool_trimmed_string_from_map(
    value: Option<&serde_json::Map<String, serde_json::Value>>,
    field: &str,
) -> Option<String> {
    admin_pool_trimmed_string(value.and_then(|object| object.get(field)))
}

fn admin_pool_oauth_organizations(
    auth_config: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Vec<serde_json::Value> {
    auth_config
        .and_then(|config| config.get("organizations"))
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn admin_pool_normalize_oauth_plan_type(value: &str, provider_type: &str) -> Option<String> {
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

fn admin_pool_derive_oauth_expires_at(
    key: &StoredProviderCatalogKey,
    auth_config: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Option<u64> {
    if !key.auth_type.trim().eq_ignore_ascii_case("oauth") {
        return None;
    }

    for field in ["expires_at", "expiresAt", "expiry", "exp"] {
        let expires_at = admin_pool_json_to_u64(auth_config.and_then(|config| config.get(field)));
        if expires_at.is_some() {
            return expires_at;
        }
    }

    key.expires_at_unix_secs
}

fn admin_pool_derive_oauth_plan_type(
    key: &StoredProviderCatalogKey,
    provider_type: &str,
    auth_config: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Option<String> {
    if !key.auth_type.trim().eq_ignore_ascii_case("oauth") {
        return None;
    }

    if let Some(upstream_metadata) = key
        .upstream_metadata
        .as_ref()
        .and_then(serde_json::Value::as_object)
    {
        let provider_bucket = upstream_metadata
            .get(&provider_type.trim().to_ascii_lowercase())
            .and_then(serde_json::Value::as_object);
        for source in provider_bucket
            .into_iter()
            .chain(std::iter::once(upstream_metadata))
        {
            for field in [
                "plan_type",
                "tier",
                "subscription_title",
                "subscription_plan",
            ] {
                if let Some(value) = source.get(field).and_then(serde_json::Value::as_str) {
                    let normalized = admin_pool_normalize_oauth_plan_type(value, provider_type);
                    if normalized.is_some() {
                        return normalized;
                    }
                }
            }
        }
    }

    if let Some(config) = auth_config {
        for field in ["plan_type", "tier", "plan", "subscription_plan"] {
            if let Some(value) = config.get(field).and_then(serde_json::Value::as_str) {
                let normalized = admin_pool_normalize_oauth_plan_type(value, provider_type);
                if normalized.is_some() {
                    return normalized;
                }
            }
        }
    }

    None
}

fn admin_pool_format_percent(value: f64) -> String {
    format!("{:.1}%", value.clamp(0.0, 100.0))
}

fn admin_pool_format_quota_value(value: f64) -> String {
    let rounded = value.round();
    if (value - rounded).abs() < 1e-6 {
        rounded.to_string()
    } else {
        format!("{value:.1}")
    }
}

fn admin_pool_has_quota_consumption(used_percent: Option<f64>) -> bool {
    used_percent
        .map(|value| value.clamp(0.0, 100.0) > 1e-6)
        .unwrap_or(false)
}

fn admin_pool_format_reset_after(seconds: f64) -> Option<String> {
    if !seconds.is_finite() {
        return None;
    }

    let total_seconds = seconds.floor() as i64;
    if total_seconds <= 0 {
        return Some("已重置".to_string());
    }

    let days = total_seconds / 86_400;
    let hours = (total_seconds % 86_400) / 3_600;
    let minutes = (total_seconds % 3_600) / 60;

    if days > 0 {
        return Some(format!("{days}天{hours}小时后重置"));
    }
    if hours > 0 {
        return Some(format!("{hours}小时{minutes}分钟后重置"));
    }
    if minutes > 0 {
        return Some(format!("{minutes}分钟后重置"));
    }
    Some("即将重置".to_string())
}

fn admin_pool_build_codex_account_quota(
    data: &serde_json::Map<String, serde_json::Value>,
) -> Option<String> {
    fn codex_reset_seconds(
        data: &serde_json::Map<String, serde_json::Value>,
        reset_seconds_key: &str,
        reset_after_seconds_key: &str,
        reset_at_key: &str,
    ) -> Option<f64> {
        admin_pool_json_to_f64(data.get(reset_seconds_key))
            .or_else(|| admin_pool_json_to_f64(data.get(reset_after_seconds_key)))
            .or_else(|| {
                let reset_at = admin_pool_json_to_u64(data.get(reset_at_key))?;
                let now_unix_secs = chrono::Utc::now().timestamp().max(0) as u64;
                Some(reset_at.saturating_sub(now_unix_secs) as f64)
            })
    }

    let mut parts = Vec::new();

    let primary_used = admin_pool_json_to_f64(data.get("primary_used_percent"));
    if let Some(primary_used) = primary_used {
        let mut part = format!("周剩余 {}", admin_pool_format_percent(100.0 - primary_used));
        if admin_pool_has_quota_consumption(Some(primary_used)) {
            if let Some(reset_text) = codex_reset_seconds(
                data,
                "primary_reset_seconds",
                "primary_reset_after_seconds",
                "primary_reset_at",
            )
            .and_then(admin_pool_format_reset_after)
            {
                part.push_str(&format!(" ({reset_text})"));
            }
        }
        parts.push(part);
    }

    let secondary_used = admin_pool_json_to_f64(data.get("secondary_used_percent"));
    if let Some(secondary_used) = secondary_used {
        let mut part = format!(
            "5H剩余 {}",
            admin_pool_format_percent(100.0 - secondary_used)
        );
        if admin_pool_has_quota_consumption(Some(secondary_used)) {
            if let Some(reset_text) = codex_reset_seconds(
                data,
                "secondary_reset_seconds",
                "secondary_reset_after_seconds",
                "secondary_reset_at",
            )
            .and_then(admin_pool_format_reset_after)
            {
                part.push_str(&format!(" ({reset_text})"));
            }
        }
        parts.push(part);
    }

    if !parts.is_empty() {
        return Some(parts.join(" | "));
    }

    let has_credits = data
        .get("has_credits")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let credits_balance = admin_pool_json_to_f64(data.get("credits_balance"));
    if has_credits && credits_balance.is_some() {
        return credits_balance.map(|value| format!("积分 {value:.2}"));
    }
    if has_credits {
        return Some("有积分".to_string());
    }

    None
}

fn admin_pool_build_kiro_account_quota(
    data: &serde_json::Map<String, serde_json::Value>,
) -> Option<String> {
    if data
        .get("is_banned")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        return Some("账号已封禁".to_string());
    }

    let usage_percentage = admin_pool_json_to_f64(data.get("usage_percentage"));
    if let Some(usage_percentage) = usage_percentage {
        let remaining = 100.0 - usage_percentage;
        let current_usage = admin_pool_json_to_f64(data.get("current_usage"));
        let usage_limit = admin_pool_json_to_f64(data.get("usage_limit"));
        if let (Some(current_usage), Some(usage_limit)) = (current_usage, usage_limit) {
            if usage_limit > 0.0 {
                return Some(format!(
                    "剩余 {} ({}/{})",
                    admin_pool_format_percent(remaining),
                    admin_pool_format_quota_value(current_usage),
                    admin_pool_format_quota_value(usage_limit),
                ));
            }
        }
        return Some(format!("剩余 {}", admin_pool_format_percent(remaining)));
    }

    let remaining = admin_pool_json_to_f64(data.get("remaining"));
    let usage_limit = admin_pool_json_to_f64(data.get("usage_limit"));
    match (remaining, usage_limit) {
        (Some(remaining), Some(usage_limit)) if usage_limit > 0.0 => Some(format!(
            "剩余 {}/{}",
            admin_pool_format_quota_value(remaining),
            admin_pool_format_quota_value(usage_limit),
        )),
        _ => None,
    }
}

fn admin_pool_quota_by_model(
    data: &serde_json::Map<String, serde_json::Value>,
) -> Option<&serde_json::Map<String, serde_json::Value>> {
    data.get("quota_by_model")?.as_object()
}

fn admin_pool_build_antigravity_account_quota(
    data: &serde_json::Map<String, serde_json::Value>,
) -> Option<String> {
    if data
        .get("is_forbidden")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        return Some("访问受限".to_string());
    }

    let remaining_list = admin_pool_quota_by_model(data)?
        .values()
        .filter_map(serde_json::Value::as_object)
        .filter_map(|item| {
            let used_percent = admin_pool_json_to_f64(item.get("used_percent")).or_else(|| {
                admin_pool_json_to_f64(item.get("remaining_fraction"))
                    .map(|value| (1.0 - value) * 100.0)
            })?;
            Some((100.0 - used_percent).clamp(0.0, 100.0))
        })
        .collect::<Vec<_>>();

    if remaining_list.is_empty() {
        return None;
    }

    let min_remaining = remaining_list.iter().copied().fold(100.0_f64, f64::min);
    if remaining_list.len() == 1 {
        return Some(format!("剩余 {}", admin_pool_format_percent(min_remaining)));
    }
    Some(format!(
        "最低剩余 {} ({} 模型)",
        admin_pool_format_percent(min_remaining),
        remaining_list.len()
    ))
}

fn admin_pool_gemini_reset_at(item: &serde_json::Map<String, serde_json::Value>) -> Option<i64> {
    let reset_at = admin_pool_json_to_u64(item.get("reset_at"))?;
    Some(reset_at as i64)
}

fn admin_pool_gemini_model_exhausted(item: &serde_json::Map<String, serde_json::Value>) -> bool {
    if item
        .get("is_exhausted")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        return true;
    }
    if admin_pool_json_to_f64(item.get("remaining_fraction")).is_some_and(|value| value <= 0.0) {
        return true;
    }
    admin_pool_json_to_f64(item.get("used_percent")).is_some_and(|value| value >= 100.0 - 1e-6)
}

fn admin_pool_build_gemini_cli_account_quota(
    data: &serde_json::Map<String, serde_json::Value>,
) -> Option<String> {
    let now = chrono::Utc::now().timestamp();
    let mut active = admin_pool_quota_by_model(data)?
        .iter()
        .filter_map(|(model_name, item)| {
            let item = item.as_object()?;
            if !admin_pool_gemini_model_exhausted(item) {
                return None;
            }
            let reset_at = admin_pool_gemini_reset_at(item);
            if reset_at.is_some_and(|value| value <= now) {
                return None;
            }
            Some((model_name.as_str(), reset_at))
        })
        .collect::<Vec<_>>();

    if active.is_empty() {
        return None;
    }

    active.sort_by_key(|(_, reset_at)| reset_at.unwrap_or(i64::MAX));
    let (first_model, first_reset_at) = active[0];
    if active.len() == 1 {
        if let Some(reset_at) = first_reset_at {
            if let Some(reset_text) = admin_pool_format_reset_after((reset_at - now) as f64) {
                return Some(format!("{first_model} 冷却中 ({reset_text})"));
            }
        }
        return Some(format!("{first_model} 冷却中"));
    }

    if let Some(reset_at) = first_reset_at {
        if let Some(reset_text) = admin_pool_format_reset_after((reset_at - now) as f64) {
            return Some(format!(
                "{} 个模型冷却中（最早 {reset_text}）",
                active.len()
            ));
        }
    }
    Some(format!("{} 个模型冷却中", active.len()))
}

fn admin_pool_build_account_quota(
    provider_type: &str,
    upstream_metadata: Option<&serde_json::Value>,
) -> Option<String> {
    let normalized_provider_type = provider_type.trim().to_ascii_lowercase();
    let upstream_metadata = upstream_metadata?.as_object()?;
    let data = upstream_metadata
        .get(&normalized_provider_type)?
        .as_object()?;

    match normalized_provider_type.as_str() {
        "codex" => admin_pool_build_codex_account_quota(data),
        "kiro" => admin_pool_build_kiro_account_quota(data),
        "antigravity" => admin_pool_build_antigravity_account_quota(data),
        "gemini_cli" => admin_pool_build_gemini_cli_account_quota(data),
        _ => None,
    }
}

fn admin_pool_health_score(key: &StoredProviderCatalogKey) -> f64 {
    let scores = key
        .health_by_format
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .map(|formats| {
            formats
                .values()
                .filter_map(serde_json::Value::as_object)
                .filter_map(|item| item.get("health_score"))
                .filter_map(serde_json::Value::as_f64)
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
        .and_then(serde_json::Value::as_object)
        .map(|formats| {
            formats
                .values()
                .filter_map(serde_json::Value::as_object)
                .any(|item| {
                    item.get("open")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false)
                })
        })
        .unwrap_or(false)
}

fn admin_pool_scheduling_payload(
    key: &StoredProviderCatalogKey,
    cooldown_reason: Option<&str>,
    cooldown_ttl_seconds: Option<u64>,
    health_score: f64,
    circuit_breaker_open: bool,
) -> (String, String, String, Vec<serde_json::Value>) {
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
                "ttl_seconds": serde_json::Value::Null,
                "detail": serde_json::Value::Null,
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
    if circuit_breaker_open {
        return (
            "degraded".to_string(),
            "circuit_breaker".to_string(),
            "熔断中".to_string(),
            vec![json!({
                "code": "circuit_breaker",
                "label": "熔断中",
                "blocking": true,
                "source": "health",
                "ttl_seconds": serde_json::Value::Null,
                "detail": serde_json::Value::Null,
            })],
        );
    }
    if health_score < 0.5 {
        return (
            "degraded".to_string(),
            "health_low".to_string(),
            "健康度较低".to_string(),
            vec![json!({
                "code": "health_low",
                "label": "健康度较低",
                "blocking": false,
                "source": "health",
                "ttl_seconds": serde_json::Value::Null,
                "detail": serde_json::Value::Null,
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
pub(super) fn build_admin_pool_key_payload(
    state: &AdminAppState<'_>,
    provider_type: &str,
    key: &StoredProviderCatalogKey,
    runtime: &AdminProviderPoolRuntimeState,
    pool_config: Option<AdminProviderPoolConfig>,
) -> serde_json::Value {
    let cooldown_reason = runtime.cooldown_reason_by_key.get(&key.id).cloned();
    let cooldown_ttl_seconds = cooldown_reason
        .as_ref()
        .and_then(|_| runtime.cooldown_ttl_by_key.get(&key.id).copied());
    let health_score = admin_pool_health_score(key);
    let circuit_breaker_open = admin_pool_circuit_breaker_open(key);
    let (scheduling_status, scheduling_reason, scheduling_label, scheduling_reasons) =
        admin_pool_scheduling_payload(
            key,
            cooldown_reason.as_deref(),
            cooldown_ttl_seconds,
            health_score,
            circuit_breaker_open,
        );
    let auth_config = state.parse_catalog_auth_config_json(key);
    let oauth_expires_at = admin_pool_derive_oauth_expires_at(key, auth_config.as_ref());
    let oauth_plan_type =
        admin_pool_derive_oauth_plan_type(key, provider_type, auth_config.as_ref());
    let status_snapshot = provider_key_status_snapshot_payload(key);
    let account_snapshot = status_snapshot
        .get("account")
        .and_then(serde_json::Value::as_object);
    let quota_snapshot = status_snapshot
        .get("quota")
        .and_then(serde_json::Value::as_object);
    let oauth_snapshot = status_snapshot
        .get("oauth")
        .and_then(serde_json::Value::as_object);
    let quota_updated_at =
        admin_pool_json_to_u64(quota_snapshot.and_then(|item| item.get("updated_at")));
    let oauth_invalid_at =
        admin_pool_json_to_u64(oauth_snapshot.and_then(|item| item.get("invalid_at")))
            .or(key.oauth_invalid_at_unix_secs);
    let oauth_account_id = admin_pool_trimmed_string_from_map(auth_config.as_ref(), "account_id");
    let oauth_account_name =
        admin_pool_trimmed_string_from_map(auth_config.as_ref(), "account_name");
    let oauth_account_user_id =
        admin_pool_trimmed_string_from_map(auth_config.as_ref(), "account_user_id");
    let oauth_organizations = admin_pool_oauth_organizations(auth_config.as_ref());
    let account_status_code = admin_pool_trimmed_string_from_map(account_snapshot, "code");
    let account_status_label =
        admin_pool_trimmed_string(account_snapshot.and_then(|item| item.get("label")));
    let account_status_reason =
        admin_pool_trimmed_string(account_snapshot.and_then(|item| item.get("reason")));
    let account_status_blocked = account_snapshot
        .and_then(|item| item.get("blocked"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let account_status_recoverable = account_snapshot
        .and_then(|item| item.get("recoverable"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let account_status_source =
        admin_pool_trimmed_string(account_snapshot.and_then(|item| item.get("source")));

    let mut payload = serde_json::Map::new();
    payload.insert("key_id".to_string(), json!(key.id));
    payload.insert("key_name".to_string(), json!(key.name));
    payload.insert("is_active".to_string(), json!(key.is_active));
    payload.insert("auth_type".to_string(), json!(key.auth_type));
    payload.insert("oauth_expires_at".to_string(), json!(oauth_expires_at));
    payload.insert("oauth_invalid_at".to_string(), json!(oauth_invalid_at));
    payload.insert(
        "oauth_invalid_reason".to_string(),
        json!(key.oauth_invalid_reason),
    );
    payload.insert("oauth_plan_type".to_string(), json!(oauth_plan_type));
    payload.insert("oauth_account_id".to_string(), json!(oauth_account_id));
    payload.insert("oauth_account_name".to_string(), json!(oauth_account_name));
    payload.insert(
        "oauth_account_user_id".to_string(),
        json!(oauth_account_user_id),
    );
    payload.insert(
        "oauth_organizations".to_string(),
        serde_json::Value::Array(oauth_organizations),
    );
    payload.insert(
        "account_status_code".to_string(),
        json!(account_status_code),
    );
    payload.insert(
        "account_status_label".to_string(),
        json!(account_status_label),
    );
    payload.insert(
        "account_status_reason".to_string(),
        json!(account_status_reason),
    );
    payload.insert(
        "account_status_blocked".to_string(),
        json!(account_status_blocked),
    );
    payload.insert(
        "account_status_recoverable".to_string(),
        json!(account_status_recoverable),
    );
    payload.insert(
        "account_status_source".to_string(),
        json!(account_status_source),
    );
    payload.insert("status_snapshot".to_string(), status_snapshot);
    payload.insert("quota_updated_at".to_string(), json!(quota_updated_at));
    payload.insert("health_score".to_string(), json!(health_score));
    payload.insert(
        "circuit_breaker_open".to_string(),
        json!(circuit_breaker_open),
    );
    payload.insert(
        "api_formats".to_string(),
        json!(admin_pool_api_formats(key)),
    );
    payload.insert(
        "rate_multipliers".to_string(),
        json!(admin_pool_json_object(key.rate_multipliers.as_ref())),
    );
    payload.insert(
        "internal_priority".to_string(),
        json!(key.internal_priority),
    );
    payload.insert("rpm_limit".to_string(), json!(key.rpm_limit));
    payload.insert(
        "cache_ttl_minutes".to_string(),
        json!(key.cache_ttl_minutes),
    );
    payload.insert(
        "max_probe_interval_minutes".to_string(),
        json!(key.max_probe_interval_minutes),
    );
    payload.insert("note".to_string(), json!(key.note));
    payload.insert(
        "allowed_models".to_string(),
        json!(admin_pool_string_list(key.allowed_models.as_ref())),
    );
    payload.insert(
        "capabilities".to_string(),
        json!(admin_pool_json_object(key.capabilities.as_ref())),
    );
    payload.insert(
        "auto_fetch_models".to_string(),
        json!(key.auto_fetch_models),
    );
    payload.insert(
        "locked_models".to_string(),
        json!(admin_pool_string_list(key.locked_models.as_ref())),
    );
    payload.insert(
        "model_include_patterns".to_string(),
        json!(admin_pool_string_list(key.model_include_patterns.as_ref())),
    );
    payload.insert(
        "model_exclude_patterns".to_string(),
        json!(admin_pool_string_list(key.model_exclude_patterns.as_ref())),
    );
    payload.insert("proxy".to_string(), json!(key.proxy.clone()));
    payload.insert("fingerprint".to_string(), json!(key.fingerprint.clone()));
    payload.insert(
        "account_quota".to_string(),
        json!(admin_pool_build_account_quota(
            provider_type,
            key.upstream_metadata.as_ref(),
        )),
    );
    payload.insert("cooldown_reason".to_string(), json!(cooldown_reason));
    payload.insert(
        "cooldown_ttl_seconds".to_string(),
        json!(cooldown_ttl_seconds),
    );
    payload.insert(
        "cost_window_usage".to_string(),
        json!(runtime
            .cost_window_usage_by_key
            .get(&key.id)
            .copied()
            .unwrap_or(0)),
    );
    payload.insert(
        "cost_limit".to_string(),
        json!(pool_config.map(|config| config.cost_limit_per_key_tokens)),
    );
    payload.insert(
        "request_count".to_string(),
        json!(key.request_count.unwrap_or(0)),
    );
    payload.insert("total_tokens".to_string(), json!(key.total_tokens));
    payload.insert(
        "total_cost_usd".to_string(),
        json!(format!("{:.8}", key.total_cost_usd)),
    );
    payload.insert(
        "sticky_sessions".to_string(),
        json!(runtime
            .sticky_sessions_by_key
            .get(&key.id)
            .copied()
            .unwrap_or(0)),
    );
    payload.insert(
        "lru_score".to_string(),
        json!(runtime.lru_score_by_key.get(&key.id).copied()),
    );
    payload.insert(
        "created_at".to_string(),
        json!(key.created_at_unix_secs.and_then(unix_secs_to_rfc3339)),
    );
    payload.insert(
        "last_used_at".to_string(),
        json!(key.last_used_at_unix_secs.and_then(unix_secs_to_rfc3339)),
    );
    payload.insert("scheduling_status".to_string(), json!(scheduling_status));
    payload.insert("scheduling_reason".to_string(), json!(scheduling_reason));
    payload.insert("scheduling_label".to_string(), json!(scheduling_label));
    payload.insert("scheduling_reasons".to_string(), json!(scheduling_reasons));

    serde_json::Value::Object(payload)
}
