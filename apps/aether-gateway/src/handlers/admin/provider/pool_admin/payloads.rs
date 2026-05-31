use crate::handlers::admin::provider::shared::support::{
    AdminProviderPoolConfig, AdminProviderPoolRuntimeState,
};
use crate::handlers::admin::request::AdminAppState;
use crate::handlers::admin::shared::{provider_key_status_snapshot_payload, unix_secs_to_rfc3339};
use crate::provider_key_auth::{
    provider_key_auth_semantics, provider_key_can_refresh_oauth, provider_key_effective_api_formats,
};
use aether_admin::provider::pool as admin_provider_pool_pure;
use aether_admin::provider::quota as admin_provider_quota_pure;
use aether_data_contracts::repository::pool_scores::StoredPoolMemberScore;
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey,
};
use aether_data_contracts::repository::usage::StoredProviderApiKeyWindowUsageSummary;
use serde_json::json;
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

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

fn admin_pool_derive_oauth_expires_at(
    provider_type: &str,
    key: &StoredProviderCatalogKey,
    auth_config: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Option<u64> {
    if !provider_key_auth_semantics(key, provider_type).oauth_managed() {
        return None;
    }

    if key.expires_at_unix_secs.is_some() {
        return key.expires_at_unix_secs;
    }

    for field in ["expires_at", "expiresAt", "expiry", "exp"] {
        let expires_at = admin_pool_json_to_u64(auth_config.and_then(|config| config.get(field)));
        if expires_at.is_some() {
            return expires_at;
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

fn admin_pool_quota_snapshot_matches_provider(
    quota_snapshot: &serde_json::Map<String, serde_json::Value>,
    provider_type: &str,
) -> bool {
    let normalized_provider_type = provider_type.trim().to_ascii_lowercase();
    match quota_snapshot
        .get("provider_type")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(provider_type) => provider_type.eq_ignore_ascii_case(&normalized_provider_type),
        None => {
            quota_snapshot
                .get("code")
                .and_then(serde_json::Value::as_str)
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
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(|windows| !windows.is_empty())
                || quota_snapshot
                    .get("credits")
                    .and_then(serde_json::Value::as_object)
                    .is_some_and(|credits| !credits.is_empty())
        }
    }
}

fn admin_pool_quota_window<'a>(
    quota_snapshot: &'a serde_json::Map<String, serde_json::Value>,
    code: &str,
) -> Option<&'a serde_json::Map<String, serde_json::Value>> {
    quota_snapshot
        .get("windows")
        .and_then(serde_json::Value::as_array)?
        .iter()
        .filter_map(serde_json::Value::as_object)
        .find(|window| {
            window
                .get("code")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|value| value.eq_ignore_ascii_case(code))
        })
}

fn admin_pool_quota_window_used_percent(
    window: &serde_json::Map<String, serde_json::Value>,
) -> Option<f64> {
    admin_pool_json_to_f64(window.get("used_ratio"))
        .map(|value| (value * 100.0).clamp(0.0, 100.0))
        .or_else(|| {
            admin_pool_json_to_f64(window.get("remaining_ratio"))
                .map(|value| ((1.0 - value) * 100.0).clamp(0.0, 100.0))
        })
}

fn admin_pool_quota_window_reset_seconds(
    quota_snapshot: &serde_json::Map<String, serde_json::Value>,
    window: &serde_json::Map<String, serde_json::Value>,
    now_unix_secs: u64,
) -> Option<f64> {
    if let Some(remaining) = admin_pool_json_to_f64(window.get("reset_seconds")) {
        let observed_at_unix_secs = admin_pool_json_to_u64(quota_snapshot.get("observed_at"))
            .or_else(|| admin_pool_json_to_u64(quota_snapshot.get("updated_at")));
        let elapsed = observed_at_unix_secs
            .map(|observed_at| now_unix_secs.saturating_sub(observed_at) as f64)
            .unwrap_or(0.0);
        return Some((remaining - elapsed).max(0.0));
    }

    admin_pool_json_to_u64(window.get("reset_at"))
        .map(|reset_at| reset_at.saturating_sub(now_unix_secs) as f64)
}

fn admin_pool_codex_quota_part_from_window(
    quota_snapshot: &serde_json::Map<String, serde_json::Value>,
    window_code: &str,
    label: &str,
    now_unix_secs: u64,
    show_reset_without_consumption: bool,
) -> Option<String> {
    let window = admin_pool_quota_window(quota_snapshot, window_code)?;
    let used_percent = admin_pool_quota_window_used_percent(window)?;
    let reset_seconds =
        admin_pool_quota_window_reset_seconds(quota_snapshot, window, now_unix_secs);
    let effective_used_percent = if reset_seconds.is_some_and(|value| value <= 0.0) {
        0.0
    } else {
        used_percent
    };

    let mut part = format!(
        "{label}剩余 {}",
        admin_pool_format_percent(100.0 - effective_used_percent)
    );
    if show_reset_without_consumption
        || admin_pool_has_quota_consumption(Some(effective_used_percent))
    {
        if let Some(reset_text) = reset_seconds.and_then(admin_pool_format_reset_after) {
            part.push_str(&format!(" ({reset_text})"));
        }
    }
    Some(part)
}

fn admin_pool_build_codex_account_quota_from_snapshot(
    quota_snapshot: &serde_json::Map<String, serde_json::Value>,
) -> Option<String> {
    let now_unix_secs = chrono::Utc::now().timestamp().max(0) as u64;
    let mut parts = Vec::new();
    let exhausted = quota_snapshot
        .get("exhausted")
        .and_then(admin_provider_quota_pure::coerce_json_bool)
        .unwrap_or(false);

    if let Some(part) = admin_pool_codex_quota_part_from_window(
        quota_snapshot,
        "weekly",
        "周",
        now_unix_secs,
        exhausted,
    ) {
        parts.push(part);
    }
    if let Some(part) = admin_pool_codex_quota_part_from_window(
        quota_snapshot,
        "5h",
        "5H",
        now_unix_secs,
        exhausted,
    ) {
        parts.push(part);
    }

    if !parts.is_empty() {
        return Some(parts.join(" | "));
    }

    let credits = quota_snapshot
        .get("credits")
        .and_then(serde_json::Value::as_object);
    let has_credits = credits
        .and_then(|credits| credits.get("has_credits"))
        .and_then(admin_provider_quota_pure::coerce_json_bool)
        .unwrap_or(false);
    let credits_balance = credits
        .and_then(|credits| credits.get("balance"))
        .and_then(admin_provider_quota_pure::coerce_json_f64);
    if has_credits && credits_balance.is_some() {
        return credits_balance.map(|value| format!("积分 {value:.2}"));
    }
    if has_credits {
        return Some("有积分".to_string());
    }

    None
}

fn admin_pool_current_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn admin_pool_prune_expired_codex_window_usage_at(
    status_snapshot: &mut serde_json::Value,
    now_unix_secs: u64,
) {
    let Some(windows) = status_snapshot
        .get_mut("quota")
        .and_then(serde_json::Value::as_object_mut)
        .and_then(|quota| quota.get_mut("windows"))
        .and_then(serde_json::Value::as_array_mut)
    else {
        return;
    };

    for window in windows
        .iter_mut()
        .filter_map(serde_json::Value::as_object_mut)
    {
        let code = window
            .get("code")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .unwrap_or_default();
        if !code.eq_ignore_ascii_case("5h") && !code.eq_ignore_ascii_case("weekly") {
            continue;
        }
        let Some(reset_at) = admin_pool_json_to_u64(window.get("reset_at")) else {
            continue;
        };
        if now_unix_secs < reset_at {
            continue;
        }
        window.insert(
            "usage".to_string(),
            json!({
                "request_count": 0,
                "total_tokens": 0,
                "total_cost_usd": "0.00000000",
            }),
        );
    }
}

fn admin_pool_prune_expired_codex_window_usage(status_snapshot: &mut serde_json::Value) {
    admin_pool_prune_expired_codex_window_usage_at(status_snapshot, admin_pool_current_unix_secs());
}

fn admin_pool_apply_codex_window_usage_summaries(
    status_snapshot: &mut serde_json::Value,
    usage_by_code: Option<&BTreeMap<String, StoredProviderApiKeyWindowUsageSummary>>,
) {
    let Some(usage_by_code) = usage_by_code else {
        return;
    };
    let Some(windows) = status_snapshot
        .get_mut("quota")
        .and_then(serde_json::Value::as_object_mut)
        .and_then(|quota| quota.get_mut("windows"))
        .and_then(serde_json::Value::as_array_mut)
    else {
        return;
    };

    for window in windows
        .iter_mut()
        .filter_map(serde_json::Value::as_object_mut)
    {
        let Some(summary) = window
            .get("code")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .and_then(|code| usage_by_code.get(&code))
        else {
            continue;
        };
        let total_cost_usd = if summary.total_cost_usd.is_finite() {
            summary.total_cost_usd.max(0.0)
        } else {
            0.0
        };
        window.insert(
            "usage".to_string(),
            json!({
                "request_count": summary.request_count,
                "total_tokens": summary.total_tokens,
                "total_cost_usd": format!("{total_cost_usd:.8}"),
            }),
        );
    }
}

fn admin_pool_quota_windows<'a>(
    quota_snapshot: &'a serde_json::Map<String, serde_json::Value>,
) -> Vec<&'a serde_json::Map<String, serde_json::Value>> {
    quota_snapshot
        .get("windows")
        .and_then(serde_json::Value::as_array)
        .map(|windows| {
            windows
                .iter()
                .filter_map(serde_json::Value::as_object)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn admin_pool_build_kiro_account_quota_from_snapshot(
    quota_snapshot: &serde_json::Map<String, serde_json::Value>,
) -> Option<String> {
    let code = quota_snapshot
        .get("code")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if code.eq_ignore_ascii_case("banned") {
        return quota_snapshot
            .get("label")
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned)
            .or_else(|| Some("账号已封禁".to_string()));
    }

    let window = admin_pool_quota_windows(quota_snapshot)
        .into_iter()
        .next()?;
    let used_ratio = admin_pool_json_to_f64(window.get("used_ratio"));
    let remaining_ratio = admin_pool_json_to_f64(window.get("remaining_ratio"))
        .or_else(|| used_ratio.map(|value| (1.0 - value).max(0.0)));
    let used_value = admin_pool_json_to_f64(window.get("used_value"));
    let remaining_value = admin_pool_json_to_f64(window.get("remaining_value"));
    let limit_value = admin_pool_json_to_f64(window.get("limit_value"));

    if let (Some(remaining_value), Some(limit_value)) = (remaining_value, limit_value) {
        if limit_value > 0.0 && remaining_value <= 0.0 {
            return Some(format!(
                "剩余 {}/{}",
                admin_pool_format_quota_value(remaining_value),
                admin_pool_format_quota_value(limit_value),
            ));
        }
    }

    if let Some(remaining_ratio) = remaining_ratio {
        let remaining_percent = (remaining_ratio * 100.0).clamp(0.0, 100.0);
        if let (Some(used_value), Some(limit_value)) = (used_value, limit_value) {
            if limit_value > 0.0 {
                return Some(format!(
                    "剩余 {} ({}/{})",
                    admin_pool_format_percent(remaining_percent),
                    admin_pool_format_quota_value(used_value),
                    admin_pool_format_quota_value(limit_value),
                ));
            }
        }
        return Some(format!(
            "剩余 {}",
            admin_pool_format_percent(remaining_percent)
        ));
    }

    match (remaining_value, limit_value) {
        (Some(remaining_value), Some(limit_value)) if limit_value > 0.0 => Some(format!(
            "剩余 {}/{}",
            admin_pool_format_quota_value(remaining_value),
            admin_pool_format_quota_value(limit_value),
        )),
        _ => None,
    }
}

fn admin_pool_build_chatgpt_web_account_quota_from_snapshot(
    quota_snapshot: &serde_json::Map<String, serde_json::Value>,
) -> Option<String> {
    let now_unix_secs = chrono::Utc::now().timestamp().max(0) as u64;
    let window = admin_pool_quota_window(quota_snapshot, "image_gen")
        .or_else(|| admin_pool_quota_windows(quota_snapshot).into_iter().next())?;
    let remaining_value = admin_pool_json_to_f64(window.get("remaining_value"));
    let limit_value = admin_pool_json_to_f64(window.get("limit_value"));
    let remaining_percent = admin_pool_json_to_f64(window.get("remaining_ratio"))
        .map(|value| (value * 100.0).clamp(0.0, 100.0))
        .or_else(|| {
            admin_pool_json_to_f64(window.get("used_ratio"))
                .map(|value| ((1.0 - value) * 100.0).clamp(0.0, 100.0))
        });
    let reset_seconds =
        admin_pool_quota_window_reset_seconds(quota_snapshot, window, now_unix_secs);

    let mut text = match (remaining_value, limit_value, remaining_percent) {
        (Some(remaining), Some(limit), _) if limit > 0.0 => Some(format!(
            "生图剩余 {}/{}",
            admin_pool_format_quota_value(remaining),
            admin_pool_format_quota_value(limit),
        )),
        (Some(remaining), _, _) => Some(format!(
            "生图剩余 {}",
            admin_pool_format_quota_value(remaining),
        )),
        (_, _, Some(percent)) => Some(format!("生图剩余 {}", admin_pool_format_percent(percent))),
        _ => None,
    }?;

    if let Some(reset_text) = reset_seconds.and_then(admin_pool_format_reset_after) {
        text.push_str(&format!(" ({reset_text})"));
    }
    Some(text)
}

fn admin_pool_build_antigravity_account_quota_from_snapshot(
    quota_snapshot: &serde_json::Map<String, serde_json::Value>,
) -> Option<String> {
    if quota_snapshot
        .get("code")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|code| code.eq_ignore_ascii_case("forbidden"))
    {
        return quota_snapshot
            .get("label")
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned)
            .or_else(|| Some("访问受限".to_string()));
    }

    let remaining_list = admin_pool_quota_windows(quota_snapshot)
        .into_iter()
        .filter(|window| {
            window
                .get("scope")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|scope| scope.eq_ignore_ascii_case("model"))
        })
        .filter_map(|window| {
            admin_pool_json_to_f64(window.get("remaining_ratio"))
                .map(|value| (value * 100.0).clamp(0.0, 100.0))
                .or_else(|| {
                    admin_pool_json_to_f64(window.get("used_ratio"))
                        .map(|value| ((1.0 - value) * 100.0).clamp(0.0, 100.0))
                })
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

fn admin_pool_grok_quota_window_label(
    window: &serde_json::Map<String, serde_json::Value>,
) -> String {
    let raw_code = window
        .get("code")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .unwrap_or_default()
        .trim_start_matches("model:")
        .to_ascii_lowercase();
    let raw_label = window
        .get("label")
        .or_else(|| window.get("model"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(raw_code.as_str())
        .to_ascii_lowercase();
    match raw_label.as_str() {
        "quota_auto" | "auto" => "Auto".to_string(),
        "quota_fast" | "fast" => "Fast".to_string(),
        "quota_expert" | "expert" => "Expert".to_string(),
        "quota_heavy" | "heavy" => "Heavy".to_string(),
        "quota_grok_4_3" | "grok-420-computer-use-sa" => "Grok 4.3".to_string(),
        _ => match raw_code.as_str() {
            "quota_auto" | "auto" => "Auto".to_string(),
            "quota_fast" | "fast" => "Fast".to_string(),
            "quota_expert" | "expert" => "Expert".to_string(),
            "quota_heavy" | "heavy" => "Heavy".to_string(),
            "quota_grok_4_3" | "grok-420-computer-use-sa" => "Grok 4.3".to_string(),
            _ => window
                .get("label")
                .or_else(|| window.get("model"))
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("模式")
                .to_string(),
        },
    }
}

fn admin_pool_quota_window_remaining_percent(
    window: &serde_json::Map<String, serde_json::Value>,
) -> Option<f64> {
    admin_pool_json_to_f64(window.get("remaining_ratio"))
        .map(|value| (value * 100.0).clamp(0.0, 100.0))
        .or_else(|| {
            admin_pool_json_to_f64(window.get("used_ratio"))
                .map(|value| ((1.0 - value) * 100.0).clamp(0.0, 100.0))
        })
        .or_else(|| {
            admin_pool_json_to_f64(window.get("remaining_value"))
                .zip(admin_pool_json_to_f64(window.get("limit_value")))
                .and_then(|(remaining, limit)| {
                    (limit > 0.0).then_some((remaining / limit * 100.0).clamp(0.0, 100.0))
                })
        })
        .or_else(|| {
            admin_pool_json_to_f64(window.get("used_value"))
                .zip(admin_pool_json_to_f64(window.get("limit_value")))
                .and_then(|(used, limit)| {
                    (limit > 0.0).then_some(((1.0 - used / limit) * 100.0).clamp(0.0, 100.0))
                })
        })
}

fn admin_pool_quota_window_value_text(
    window: &serde_json::Map<String, serde_json::Value>,
) -> Option<String> {
    let limit_value =
        admin_pool_json_to_f64(window.get("limit_value")).filter(|value| *value > 0.0)?;
    if let Some(remaining_value) = admin_pool_json_to_f64(window.get("remaining_value")) {
        return Some(format!(
            "{}/{}",
            admin_pool_format_quota_value(remaining_value),
            admin_pool_format_quota_value(limit_value),
        ));
    }
    admin_pool_json_to_f64(window.get("used_value")).map(|used_value| {
        format!(
            "{}/{}",
            admin_pool_format_quota_value((limit_value - used_value).max(0.0)),
            admin_pool_format_quota_value(limit_value),
        )
    })
}

fn admin_pool_build_grok_account_quota_from_snapshot(
    quota_snapshot: &serde_json::Map<String, serde_json::Value>,
) -> Option<String> {
    let code = quota_snapshot
        .get("code")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if code.eq_ignore_ascii_case("banned") {
        return quota_snapshot
            .get("label")
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned)
            .or_else(|| Some("账号已封禁".to_string()));
    }
    if code.eq_ignore_ascii_case("forbidden") {
        return quota_snapshot
            .get("label")
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned)
            .or_else(|| Some("访问受限".to_string()));
    }

    let model_parts = admin_pool_quota_windows(quota_snapshot)
        .into_iter()
        .filter(|window| {
            window
                .get("scope")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|scope| scope.eq_ignore_ascii_case("model"))
        })
        .filter_map(|window| {
            let remaining_percent = admin_pool_quota_window_remaining_percent(window)?;
            let mut part = format!(
                "{}剩余 {}",
                admin_pool_grok_quota_window_label(window),
                admin_pool_format_percent(remaining_percent),
            );
            if let Some(value_text) = admin_pool_quota_window_value_text(window) {
                part.push_str(&format!(" ({value_text})"));
            }
            Some(part)
        })
        .collect::<Vec<_>>();

    if !model_parts.is_empty() {
        return Some(model_parts.join(" | "));
    }

    let window = admin_pool_quota_window(quota_snapshot, "usage")
        .or_else(|| admin_pool_quota_windows(quota_snapshot).into_iter().next())?;
    let remaining_value = admin_pool_json_to_f64(window.get("remaining_value"));
    let limit_value = admin_pool_json_to_f64(window.get("limit_value"));
    if let (Some(remaining_value), Some(limit_value)) = (remaining_value, limit_value) {
        if limit_value > 0.0 && remaining_value <= 0.0 {
            return Some(format!(
                "剩余 {}/{}",
                admin_pool_format_quota_value(remaining_value),
                admin_pool_format_quota_value(limit_value),
            ));
        }
    }

    if let Some(remaining_percent) = admin_pool_quota_window_remaining_percent(window) {
        if let Some(value_text) = admin_pool_quota_window_value_text(window) {
            return Some(format!(
                "剩余 {} ({value_text})",
                admin_pool_format_percent(remaining_percent),
            ));
        }
        return Some(format!(
            "剩余 {}",
            admin_pool_format_percent(remaining_percent),
        ));
    }

    match (remaining_value, limit_value) {
        (Some(remaining_value), Some(limit_value)) if limit_value > 0.0 => Some(format!(
            "剩余 {}/{}",
            admin_pool_format_quota_value(remaining_value),
            admin_pool_format_quota_value(limit_value),
        )),
        _ => quota_snapshot
            .get("label")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
    }
}

fn admin_pool_build_gemini_cli_account_quota_from_snapshot(
    quota_snapshot: &serde_json::Map<String, serde_json::Value>,
) -> Option<String> {
    if let Some(credits) = quota_snapshot
        .get("credits")
        .and_then(serde_json::Value::as_object)
    {
        if let Some(remaining) = admin_pool_json_to_f64(credits.get("remaining")) {
            return Some(format!(
                "AI Credits 剩余 {}",
                admin_pool_format_quota_value(remaining)
            ));
        }
    }

    let now = chrono::Utc::now().timestamp();
    let mut active = admin_pool_quota_windows(quota_snapshot)
        .into_iter()
        .filter(|window| {
            window
                .get("scope")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|scope| scope.eq_ignore_ascii_case("model"))
        })
        .filter(|window| {
            window
                .get("is_exhausted")
                .and_then(admin_provider_quota_pure::coerce_json_bool)
                .or_else(|| {
                    admin_pool_json_to_f64(window.get("used_ratio"))
                        .map(|value| value >= 1.0 - 1e-6)
                })
                .unwrap_or(false)
        })
        .filter_map(|window| {
            let reset_at = admin_pool_json_to_u64(window.get("reset_at")).map(|value| value as i64);
            if reset_at.is_some_and(|value| value <= now) {
                return None;
            }
            let label = window
                .get("label")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .or_else(|| window.get("model").and_then(serde_json::Value::as_str))
                .unwrap_or("模型");
            Some((label.to_string(), reset_at))
        })
        .collect::<Vec<_>>();

    if active.is_empty() {
        return None;
    }

    active.sort_by_key(|(_, reset_at)| reset_at.unwrap_or(i64::MAX));
    let (first_model, first_reset_at) = &active[0];
    if active.len() == 1 {
        if let Some(reset_at) = first_reset_at {
            if let Some(reset_text) = admin_pool_format_reset_after((*reset_at - now) as f64) {
                return Some(format!("{first_model} 冷却中 ({reset_text})"));
            }
        }
        return Some(format!("{first_model} 冷却中"));
    }

    if let Some(reset_at) = first_reset_at {
        if let Some(reset_text) = admin_pool_format_reset_after((*reset_at - now) as f64) {
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
    quota_snapshot: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Option<String> {
    let normalized_provider_type = provider_type.trim().to_ascii_lowercase();
    let quota_snapshot = quota_snapshot.filter(|quota_snapshot| {
        admin_pool_quota_snapshot_matches_provider(quota_snapshot, &normalized_provider_type)
    })?;

    match normalized_provider_type.as_str() {
        "codex" => {
            if let Some(account_quota) =
                admin_pool_build_codex_account_quota_from_snapshot(quota_snapshot)
            {
                return Some(account_quota);
            }
        }
        "kiro" => {
            if let Some(account_quota) =
                admin_pool_build_kiro_account_quota_from_snapshot(quota_snapshot)
            {
                return Some(account_quota);
            }
        }
        "chatgpt_web" => {
            if let Some(account_quota) =
                admin_pool_build_chatgpt_web_account_quota_from_snapshot(quota_snapshot)
            {
                return Some(account_quota);
            }
        }
        "antigravity" => {
            if let Some(account_quota) =
                admin_pool_build_antigravity_account_quota_from_snapshot(quota_snapshot)
            {
                return Some(account_quota);
            }
        }
        "grok" => {
            if let Some(account_quota) =
                admin_pool_build_grok_account_quota_from_snapshot(quota_snapshot)
            {
                return Some(account_quota);
            }
        }
        "gemini_cli" => {
            if let Some(account_quota) =
                admin_pool_build_gemini_cli_account_quota_from_snapshot(quota_snapshot)
            {
                return Some(account_quota);
            }
        }
        _ => {}
    }

    None
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

fn admin_pool_scheduling_payload(
    key: &StoredProviderCatalogKey,
    cooldown_reason: Option<&str>,
    cooldown_ttl_seconds: Option<u64>,
    account_blocked: bool,
    account_status_code: Option<&str>,
    account_status_label: Option<&str>,
    account_status_reason: Option<&str>,
    account_status_source: Option<&str>,
    account_quota_exhausted: bool,
    oauth_status_code: Option<&str>,
    oauth_status_label: Option<&str>,
    oauth_status_reason: Option<&str>,
    oauth_status_source: Option<&str>,
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
    if account_blocked {
        return (
            "blocked".to_string(),
            "account_blocked".to_string(),
            account_status_label.unwrap_or("账号异常").to_string(),
            vec![json!({
                "code": account_status_code.unwrap_or("account_blocked"),
                "label": account_status_label.unwrap_or("账号异常"),
                "blocking": true,
                "source": account_status_source,
                "ttl_seconds": serde_json::Value::Null,
                "detail": account_status_reason,
            })],
        );
    }
    if account_quota_exhausted {
        return (
            "blocked".to_string(),
            "account_quota_exhausted".to_string(),
            "额度耗尽".to_string(),
            vec![json!({
                "code": "account_quota_exhausted",
                "label": "额度耗尽",
                "blocking": true,
                "source": "quota",
                "ttl_seconds": serde_json::Value::Null,
                "detail": serde_json::Value::Null,
            })],
        );
    }
    if matches!(oauth_status_code, Some("invalid" | "expired")) {
        let label = oauth_status_label.unwrap_or(if oauth_status_code == Some("expired") {
            "已过期"
        } else {
            "已失效"
        });
        return (
            "blocked".to_string(),
            oauth_status_code.unwrap_or("invalid").to_string(),
            label.to_string(),
            vec![json!({
                "code": oauth_status_code.unwrap_or("invalid"),
                "label": label,
                "blocking": true,
                "source": oauth_status_source.unwrap_or("oauth"),
                "ttl_seconds": serde_json::Value::Null,
                "detail": oauth_status_reason,
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
pub(super) fn build_admin_pool_key_payload(
    state: &AdminAppState<'_>,
    provider_type: &str,
    endpoints: &[StoredProviderCatalogEndpoint],
    key: &StoredProviderCatalogKey,
    runtime: &AdminProviderPoolRuntimeState,
    pool_config: Option<AdminProviderPoolConfig>,
    pool_score: Option<&StoredPoolMemberScore>,
    codex_cycle_usage_by_code: Option<&BTreeMap<String, StoredProviderApiKeyWindowUsageSummary>>,
    now_unix_secs: u64,
) -> serde_json::Value {
    let cooldown_reason = runtime.cooldown_reason_by_key.get(&key.id).cloned();
    let cooldown_ttl_seconds = cooldown_reason
        .as_ref()
        .and_then(|_| runtime.cooldown_ttl_by_key.get(&key.id).copied());
    let health_score = admin_pool_health_score(key);
    let circuit_breaker_open = false;
    let auth_semantics = provider_key_auth_semantics(key, provider_type);
    let account_quota_exhausted = pool_config
        .as_ref()
        .is_some_and(|config| config.skip_exhausted_accounts)
        && admin_provider_pool_pure::admin_pool_key_account_quota_exhausted(key, provider_type);
    let auth_config = state.parse_catalog_auth_config_json(key);
    let oauth_expires_at =
        admin_pool_derive_oauth_expires_at(provider_type, key, auth_config.as_ref());
    let oauth_plan_type = if auth_semantics.oauth_managed() {
        aether_provider_pool::derive_plan_tier(provider_type, key, auth_config.as_ref())
    } else {
        None
    };
    let mut status_snapshot = provider_key_status_snapshot_payload(key, provider_type);
    if provider_type.trim().eq_ignore_ascii_case("codex") {
        admin_pool_apply_codex_window_usage_summaries(
            &mut status_snapshot,
            codex_cycle_usage_by_code,
        );
        admin_pool_prune_expired_codex_window_usage_at(&mut status_snapshot, now_unix_secs);
    }
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
    let account_quota = admin_pool_build_account_quota(provider_type, quota_snapshot);
    let oauth_invalid_at = if auth_semantics.can_show_oauth_metadata() {
        admin_pool_json_to_u64(oauth_snapshot.and_then(|item| item.get("invalid_at")))
            .or(key.oauth_invalid_at_unix_secs)
    } else {
        None
    };
    let oauth_account_id = auth_semantics
        .can_show_oauth_metadata()
        .then(|| admin_pool_trimmed_string_from_map(auth_config.as_ref(), "account_id"))
        .flatten();
    let oauth_account_name = auth_semantics
        .can_show_oauth_metadata()
        .then(|| admin_pool_trimmed_string_from_map(auth_config.as_ref(), "account_name"))
        .flatten();
    let oauth_account_user_id = auth_semantics
        .can_show_oauth_metadata()
        .then(|| admin_pool_trimmed_string_from_map(auth_config.as_ref(), "account_user_id"))
        .flatten();
    let oauth_organizations = if auth_semantics.can_show_oauth_metadata() {
        admin_pool_oauth_organizations(auth_config.as_ref())
    } else {
        Vec::new()
    };
    let oauth_temporary = auth_semantics.can_show_oauth_metadata()
        && auth_config
            .as_ref()
            .and_then(|config| config.get("access_token_import_temporary"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
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
    let oauth_status_code = admin_pool_trimmed_string_from_map(oauth_snapshot, "code");
    let oauth_status_label =
        admin_pool_trimmed_string(oauth_snapshot.and_then(|item| item.get("label")));
    let oauth_status_reason =
        admin_pool_trimmed_string(oauth_snapshot.and_then(|item| item.get("reason")));
    let oauth_status_source =
        admin_pool_trimmed_string(oauth_snapshot.and_then(|item| item.get("source")));
    let (scheduling_status, scheduling_reason, scheduling_label, scheduling_reasons) =
        admin_pool_scheduling_payload(
            key,
            cooldown_reason.as_deref(),
            cooldown_ttl_seconds,
            account_status_blocked,
            account_status_code.as_deref(),
            account_status_label.as_deref(),
            account_status_reason.as_deref(),
            account_status_source.as_deref(),
            account_quota_exhausted,
            oauth_status_code.as_deref(),
            oauth_status_label.as_deref(),
            oauth_status_reason.as_deref(),
            oauth_status_source.as_deref(),
        );

    let mut payload = serde_json::Map::new();
    payload.insert("key_id".to_string(), json!(key.id));
    payload.insert("key_name".to_string(), json!(key.name));
    payload.insert("is_active".to_string(), json!(key.is_active));
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
    payload.insert("oauth_expires_at".to_string(), json!(oauth_expires_at));
    payload.insert("oauth_invalid_at".to_string(), json!(oauth_invalid_at));
    payload.insert(
        "oauth_invalid_reason".to_string(),
        json!(auth_semantics
            .can_show_oauth_metadata()
            .then_some(key.oauth_invalid_reason.clone())
            .flatten()),
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
    payload.insert("oauth_temporary".to_string(), json!(oauth_temporary));
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
        "pool_score".to_string(),
        pool_score
            .map(|score| {
                json!({
                    "id": score.id.clone(),
                    "capability": score.capability.clone(),
                    "scope_kind": score.scope_kind.clone(),
                    "scope_id": score.scope_id.clone(),
                    "score": score.score,
                    "hard_state": score.hard_state.as_database(),
                    "score_version": score.score_version,
                    "score_reason": score.score_reason.clone(),
                    "last_ranked_at": score.last_ranked_at,
                    "last_scheduled_at": score.last_scheduled_at,
                    "last_success_at": score.last_success_at,
                    "last_failure_at": score.last_failure_at,
                    "failure_count": score.failure_count,
                    "last_probe_attempt_at": score.last_probe_attempt_at,
                    "last_probe_success_at": score.last_probe_success_at,
                    "last_probe_failure_at": score.last_probe_failure_at,
                    "probe_failure_count": score.probe_failure_count,
                    "probe_status": score.probe_status.as_database(),
                    "updated_at": score.updated_at,
                })
            })
            .unwrap_or(serde_json::Value::Null),
    );
    payload.insert(
        "circuit_breaker_open".to_string(),
        json!(circuit_breaker_open),
    );
    payload.insert(
        "api_formats".to_string(),
        json!(provider_key_effective_api_formats(
            key,
            provider_type,
            endpoints,
        )),
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
    payload.insert(
        "upstream_metadata".to_string(),
        json!(key.upstream_metadata.clone()),
    );
    payload.insert("proxy".to_string(), json!(key.proxy.clone()));
    payload.insert("fingerprint".to_string(), json!(key.fingerprint.clone()));
    payload.insert("account_quota".to_string(), json!(account_quota));
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
        json!(pool_config
            .as_ref()
            .map(|config| config.cost_limit_per_key_tokens)),
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
        json!(key.created_at_unix_ms.and_then(unix_secs_to_rfc3339)),
    );
    payload.insert(
        "imported_at".to_string(),
        json!(key.created_at_unix_ms.and_then(unix_secs_to_rfc3339)),
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn expired_codex_window_usage_is_zeroed_for_display() {
        let mut snapshot = json!({
            "quota": {
                "windows": [
                    {
                        "code": "5h",
                        "reset_at": 1,
                        "usage": {
                            "request_count": 7,
                            "total_tokens": 700,
                            "total_cost_usd": "7.00000000"
                        }
                    }
                ]
            }
        });

        admin_pool_prune_expired_codex_window_usage(&mut snapshot);

        let usage = &snapshot["quota"]["windows"][0]["usage"];
        assert_eq!(usage["request_count"], json!(0));
        assert_eq!(usage["total_tokens"], json!(0));
        assert_eq!(usage["total_cost_usd"], json!("0.00000000"));
    }

    #[test]
    fn active_codex_window_usage_is_preserved_for_display() {
        let mut snapshot = json!({
            "quota": {
                "windows": [
                    {
                        "code": "weekly",
                        "reset_at": 4_102_444_800u64,
                        "usage": {
                            "request_count": 3,
                            "total_tokens": 375,
                            "total_cost_usd": "0.60000000"
                        }
                    }
                ]
            }
        });

        admin_pool_prune_expired_codex_window_usage(&mut snapshot);

        let usage = &snapshot["quota"]["windows"][0]["usage"];
        assert_eq!(usage["request_count"], json!(3));
        assert_eq!(usage["total_tokens"], json!(375));
        assert_eq!(usage["total_cost_usd"], json!("0.60000000"));
    }

    #[test]
    fn grok_model_quota_is_rendered_for_pool_rows() {
        let quota_snapshot = json!({
            "provider_type": "grok",
            "code": "ok",
            "exhausted": false,
            "plan_type": "heavy",
            "pool_tier": "heavy",
            "windows": [
                {
                    "code": "model:quota_auto",
                    "label": "auto",
                    "scope": "model",
                    "remaining_ratio": 0.4,
                    "used_value": 90,
                    "limit_value": 150
                },
                {
                    "code": "model:quota_heavy",
                    "label": "heavy",
                    "scope": "model",
                    "remaining_ratio": 0.0,
                    "used_value": 20,
                    "limit_value": 20
                }
            ]
        });
        let quota_snapshot = quota_snapshot.as_object().unwrap();

        assert_eq!(
            admin_pool_build_account_quota("grok", Some(quota_snapshot)),
            Some("Auto剩余 40.0% (60/150) | Heavy剩余 0.0% (0/20)".to_string())
        );
    }
}
