use aether_contracts::ExecutionResult;
use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;
use serde_json::json;
use std::collections::BTreeMap;

const OAUTH_ACCOUNT_BLOCK_PREFIX: &str = "[ACCOUNT_BLOCK] ";
const OAUTH_REFRESH_FAILED_PREFIX: &str = "[REFRESH_FAILED] ";
const OAUTH_EXPIRED_PREFIX: &str = "[OAUTH_EXPIRED] ";
const OAUTH_REQUEST_FAILED_PREFIX: &str = "[REQUEST_FAILED] ";

pub fn provider_auto_remove_banned_keys(config: Option<&serde_json::Value>) -> bool {
    config
        .and_then(|value| value.get("pool_advanced"))
        .and_then(serde_json::Value::as_object)
        .and_then(|object| object.get("auto_remove_banned_keys"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

pub fn should_auto_remove_structured_reason(reason: Option<&str>) -> bool {
    reason
        .map(str::trim)
        .is_some_and(|value| value.starts_with(OAUTH_ACCOUNT_BLOCK_PREFIX))
}

pub fn normalize_string_id_list(values: Option<Vec<String>>) -> Option<Vec<String>> {
    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for value in values.into_iter().flatten() {
        let trimmed = value.trim();
        if trimmed.is_empty() || !seen.insert(trimmed.to_string()) {
            continue;
        }
        out.push(trimmed.to_string());
    }
    (!out.is_empty()).then_some(out)
}

pub fn coerce_json_u64(value: &serde_json::Value) -> Option<u64> {
    match value {
        serde_json::Value::Number(number) => number.as_u64(),
        serde_json::Value::String(text) => text.trim().parse::<u64>().ok(),
        _ => None,
    }
}

pub fn coerce_json_f64(value: &serde_json::Value) -> Option<f64> {
    match value {
        serde_json::Value::Number(number) => number.as_f64(),
        serde_json::Value::String(text) => text.trim().parse::<f64>().ok(),
        _ => None,
    }
}

pub fn coerce_json_bool(value: &serde_json::Value) -> Option<bool> {
    match value {
        serde_json::Value::Bool(value) => Some(*value),
        serde_json::Value::String(text) => match text.trim().to_ascii_lowercase().as_str() {
            "true" | "1" => Some(true),
            "false" | "0" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

pub fn coerce_json_string(value: Option<&serde_json::Value>) -> Option<String> {
    value
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub fn extract_execution_error_message(result: &ExecutionResult) -> Option<String> {
    if let Some(body_json) = result
        .body
        .as_ref()
        .and_then(|body| body.json_body.as_ref())
        .and_then(serde_json::Value::as_object)
    {
        if let Some(error) = body_json
            .get("error")
            .and_then(serde_json::Value::as_object)
        {
            if let Some(message) = error.get("message").and_then(serde_json::Value::as_str) {
                let trimmed = message.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
        if let Some(message) = body_json.get("message").and_then(serde_json::Value::as_str) {
            let trimmed = message.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }

    result
        .error
        .as_ref()
        .map(|error| error.message.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub fn quota_refresh_success_invalid_state(
    key: &StoredProviderCatalogKey,
) -> (Option<u64>, Option<String>) {
    let current_reason = key
        .oauth_invalid_reason
        .as_deref()
        .map(str::trim)
        .unwrap_or_default();
    if current_reason.starts_with(OAUTH_REFRESH_FAILED_PREFIX) {
        return (
            key.oauth_invalid_at_unix_secs,
            (!current_reason.is_empty()).then_some(current_reason.to_string()),
        );
    }
    (None, None)
}

pub fn parse_antigravity_usage_response(
    value: &serde_json::Value,
    updated_at_unix_secs: u64,
) -> Option<serde_json::Value> {
    let models = value.get("models")?.as_object()?;
    let mut quota_by_model = serde_json::Map::new();

    for (model_id, model_value) in models {
        let mut payload = serde_json::Map::new();
        if let Some(display_name) = coerce_json_string(
            model_value
                .get("displayName")
                .or_else(|| model_value.get("display_name")),
        ) {
            payload.insert("display_name".to_string(), json!(display_name));
        }

        let quota_info = model_value
            .get("quotaInfo")
            .and_then(serde_json::Value::as_object);
        let remaining_fraction = quota_info
            .and_then(|object| object.get("remainingFraction"))
            .and_then(coerce_json_f64);
        let used_percent = remaining_fraction
            .map(|value| ((1.0 - value).max(0.0) * 100.0).min(100.0))
            .unwrap_or(100.0);
        payload.insert(
            "remaining_fraction".to_string(),
            json!(remaining_fraction.unwrap_or(0.0)),
        );
        payload.insert("used_percent".to_string(), json!(used_percent));
        if let Some(reset_time) = quota_info
            .and_then(|object| object.get("resetTime"))
            .cloned()
            .filter(|value| !value.is_null())
        {
            payload.insert("reset_time".to_string(), reset_time);
        }
        quota_by_model.insert(model_id.clone(), serde_json::Value::Object(payload));
    }

    Some(json!({
        "updated_at": updated_at_unix_secs,
        "is_forbidden": false,
        "forbidden_reason": serde_json::Value::Null,
        "forbidden_at": serde_json::Value::Null,
        "models": quota_by_model,
    }))
}

pub fn normalize_codex_plan_type(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase())
}

pub fn build_codex_quota_exhausted_fallback_metadata(
    plan_type: Option<&str>,
    updated_at_unix_secs: u64,
) -> serde_json::Value {
    let mut object = serde_json::Map::new();
    if let Some(plan_type) = normalize_codex_plan_type(plan_type) {
        object.insert(
            "plan_type".to_string(),
            serde_json::Value::String(plan_type),
        );
    }
    object.insert("updated_at".to_string(), json!(updated_at_unix_secs));
    object.insert("primary_used_percent".to_string(), json!(100.0));
    if normalize_codex_plan_type(plan_type) != Some("free".to_string()) {
        object.insert("secondary_used_percent".to_string(), json!(100.0));
    }
    serde_json::Value::Object(object)
}

fn codex_write_window(
    target: &mut serde_json::Map<String, serde_json::Value>,
    source: &serde_json::Map<String, serde_json::Value>,
    target_prefix: &str,
) {
    if let Some(value) = source.get("used_percent").and_then(coerce_json_f64) {
        target.insert(format!("{target_prefix}_used_percent"), json!(value));
    }
    if let Some(value) = source.get("reset_after_seconds").and_then(coerce_json_u64) {
        target.insert(format!("{target_prefix}_reset_after_seconds"), json!(value));
    }
    if let Some(value) = source.get("reset_at").and_then(coerce_json_u64) {
        target.insert(format!("{target_prefix}_reset_at"), json!(value));
    }
    if let Some(value) = source.get("window_minutes").and_then(coerce_json_u64) {
        target.insert(format!("{target_prefix}_window_minutes"), json!(value));
    }
}

pub fn parse_codex_wham_usage_response(
    value: &serde_json::Value,
    updated_at_unix_secs: u64,
) -> Option<serde_json::Value> {
    let root = value.as_object()?;
    if root.is_empty() {
        return None;
    }

    let mut result = serde_json::Map::new();
    let plan_type =
        normalize_codex_plan_type(root.get("plan_type").and_then(serde_json::Value::as_str));
    if let Some(plan_type) = plan_type.as_ref() {
        result.insert("plan_type".to_string(), json!(plan_type));
    }

    let rate_limit = root
        .get("rate_limit")
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default();
    let primary_window = rate_limit
        .get("primary_window")
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default();
    let secondary_window = rate_limit
        .get("secondary_window")
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default();

    let use_paid_windows = !secondary_window.is_empty() && plan_type.as_deref() != Some("free");
    if use_paid_windows {
        codex_write_window(&mut result, &secondary_window, "primary");
        codex_write_window(&mut result, &primary_window, "secondary");
    } else {
        codex_write_window(&mut result, &primary_window, "primary");
    }

    if let Some(credits) = root.get("credits").and_then(serde_json::Value::as_object) {
        if let Some(value) = credits.get("has_credits").and_then(coerce_json_bool) {
            result.insert("has_credits".to_string(), json!(value));
        }
        if let Some(value) = credits.get("balance").and_then(coerce_json_f64) {
            result.insert("credits_balance".to_string(), json!(value));
        }
        if let Some(value) = credits.get("unlimited").and_then(coerce_json_bool) {
            result.insert("credits_unlimited".to_string(), json!(value));
        }
    }

    if result.is_empty() {
        return None;
    }
    result.insert("updated_at".to_string(), json!(updated_at_unix_secs));
    Some(serde_json::Value::Object(result))
}

pub fn parse_codex_usage_headers(
    headers: &BTreeMap<String, String>,
    updated_at_unix_secs: u64,
) -> Option<serde_json::Value> {
    let mut result = serde_json::Map::new();
    let normalized = headers
        .iter()
        .map(|(key, value)| (key.trim().to_ascii_lowercase(), value.trim().to_string()))
        .collect::<BTreeMap<_, _>>();
    if !normalized.keys().any(|key| key.starts_with("x-codex-")) {
        return None;
    }

    let plan_type =
        normalize_codex_plan_type(normalized.get("x-codex-plan-type").map(String::as_str));
    if let Some(plan_type) = plan_type.as_ref() {
        result.insert("plan_type".to_string(), json!(plan_type));
    }

    let read_window = |prefix: &str| -> serde_json::Map<String, serde_json::Value> {
        let mut object = serde_json::Map::new();
        let used_key = format!("x-codex-{prefix}-used-percent");
        let reset_after_key = format!("x-codex-{prefix}-reset-after-seconds");
        let reset_at_key = format!("x-codex-{prefix}-reset-at");
        let window_minutes_key = format!("x-codex-{prefix}-window-minutes");
        if let Some(value) = normalized
            .get(&used_key)
            .and_then(|value| value.parse::<f64>().ok())
        {
            object.insert("used_percent".to_string(), json!(value));
        }
        if let Some(value) = normalized
            .get(&reset_after_key)
            .and_then(|value| value.parse::<u64>().ok())
        {
            object.insert("reset_after_seconds".to_string(), json!(value));
        }
        if let Some(value) = normalized
            .get(&reset_at_key)
            .and_then(|value| value.parse::<u64>().ok())
        {
            object.insert("reset_at".to_string(), json!(value));
        }
        if let Some(value) = normalized
            .get(&window_minutes_key)
            .and_then(|value| value.parse::<u64>().ok())
        {
            object.insert("window_minutes".to_string(), json!(value));
        }
        object
    };

    let primary_window = read_window("primary");
    let secondary_window = read_window("secondary");
    let use_paid_windows = !secondary_window.is_empty() && plan_type.as_deref() != Some("free");
    if use_paid_windows {
        codex_write_window(&mut result, &secondary_window, "primary");
        codex_write_window(&mut result, &primary_window, "secondary");
    } else {
        codex_write_window(&mut result, &primary_window, "primary");
    }

    if let Some(value) = normalized
        .get("x-codex-primary-over-secondary-limit-percent")
        .and_then(|value| value.parse::<f64>().ok())
    {
        result.insert(
            "primary_over_secondary_limit_percent".to_string(),
            json!(value),
        );
    }
    if let Some(value) = normalized
        .get("x-codex-credits-has-credits")
        .and_then(|value| match value.to_ascii_lowercase().as_str() {
            "true" | "1" => Some(true),
            "false" | "0" => Some(false),
            _ => None,
        })
    {
        result.insert("has_credits".to_string(), json!(value));
    }
    if let Some(value) = normalized
        .get("x-codex-credits-balance")
        .and_then(|value| value.parse::<f64>().ok())
    {
        result.insert("credits_balance".to_string(), json!(value));
    }
    if let Some(value) = normalized
        .get("x-codex-credits-unlimited")
        .and_then(|value| match value.to_ascii_lowercase().as_str() {
            "true" | "1" => Some(true),
            "false" | "0" => Some(false),
            _ => None,
        })
    {
        result.insert("credits_unlimited".to_string(), json!(value));
    }

    if result.is_empty() {
        return None;
    }
    result.insert("updated_at".to_string(), json!(updated_at_unix_secs));
    Some(serde_json::Value::Object(result))
}

fn codex_current_invalid_reason(key: &StoredProviderCatalogKey) -> String {
    key.oauth_invalid_reason
        .as_deref()
        .map(str::trim)
        .unwrap_or_default()
        .to_string()
}

fn codex_merge_invalid_reason(current: &str, candidate_reason: &str) -> String {
    if current.is_empty() {
        return candidate_reason.to_string();
    }
    if current.starts_with(OAUTH_ACCOUNT_BLOCK_PREFIX) {
        return current.to_string();
    }
    if current.starts_with(OAUTH_EXPIRED_PREFIX)
        && candidate_reason.starts_with(OAUTH_REQUEST_FAILED_PREFIX)
    {
        return current.to_string();
    }
    candidate_reason.to_string()
}

pub fn codex_build_invalid_state(
    key: &StoredProviderCatalogKey,
    candidate_reason: String,
    now_unix_secs: u64,
) -> (Option<u64>, Option<String>) {
    let current_reason = codex_current_invalid_reason(key);
    let merged_reason = codex_merge_invalid_reason(&current_reason, &candidate_reason);
    if merged_reason == current_reason {
        return (key.oauth_invalid_at_unix_secs, Some(merged_reason));
    }
    (Some(now_unix_secs), Some(merged_reason))
}

pub fn codex_looks_like_token_invalidated(message: Option<&str>) -> bool {
    let lowered = message.unwrap_or_default().trim().to_ascii_lowercase();
    lowered.contains("token invalid")
        || lowered.contains("token invalidated")
        || lowered.contains("session has expired")
        || lowered.contains("session expired")
}

fn codex_looks_like_account_deactivated(message: Option<&str>) -> bool {
    let lowered = message.unwrap_or_default().trim().to_ascii_lowercase();
    lowered.contains("account has been deactivated") || lowered.contains("account deactivated")
}

pub fn codex_looks_like_workspace_deactivated(message: Option<&str>) -> bool {
    let lowered = message.unwrap_or_default().trim().to_ascii_lowercase();
    lowered.contains("deactivated_workspace")
        || (lowered.contains("workspace") && lowered.contains("deactivated"))
}

pub fn codex_structured_invalid_reason(status_code: u16, upstream_message: Option<&str>) -> String {
    let message = upstream_message.unwrap_or_default().trim();
    if status_code == 402 && codex_looks_like_workspace_deactivated(Some(message)) {
        return format!("{OAUTH_ACCOUNT_BLOCK_PREFIX}工作区已停用 (deactivated_workspace)");
    }
    if codex_looks_like_account_deactivated(Some(message)) {
        let detail = if message.is_empty() {
            "OpenAI 账号已停用"
        } else {
            message
        };
        return format!("{OAUTH_ACCOUNT_BLOCK_PREFIX}{detail}");
    }
    if codex_looks_like_token_invalidated(Some(message)) {
        let detail = if message.is_empty() {
            "Codex Token 无效或已过期"
        } else {
            message
        };
        return format!("{OAUTH_EXPIRED_PREFIX}{detail}");
    }
    if status_code == 401 {
        let detail = if message.is_empty() {
            "Codex Token 无效或已过期 (401)"
        } else {
            message
        };
        return format!("{OAUTH_EXPIRED_PREFIX}{detail}");
    }
    if status_code == 403 {
        let detail = if message.is_empty() {
            "Codex 账户访问受限 (403)"
        } else {
            message
        };
        return format!("{OAUTH_ACCOUNT_BLOCK_PREFIX}{detail}");
    }
    message.to_string()
}

pub fn codex_soft_request_failure_reason(
    status_code: u16,
    upstream_message: Option<&str>,
) -> String {
    let detail = upstream_message
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| format!("Codex 请求失败 ({status_code})"));
    format!("{OAUTH_REQUEST_FAILED_PREFIX}{detail}")
}

fn compute_kiro_total_usage_limit(breakdown: &serde_json::Value) -> f64 {
    let mut total = breakdown
        .get("usageLimitWithPrecision")
        .and_then(coerce_json_f64)
        .unwrap_or(0.0);

    if breakdown
        .get("freeTrialInfo")
        .and_then(serde_json::Value::as_object)
        .is_some_and(|free_trial| {
            free_trial
                .get("freeTrialStatus")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .is_some_and(|value| value.eq_ignore_ascii_case("ACTIVE"))
        })
    {
        total += breakdown
            .get("freeTrialInfo")
            .and_then(|value| value.get("usageLimitWithPrecision"))
            .and_then(coerce_json_f64)
            .unwrap_or(0.0);
    }

    if let Some(bonuses) = breakdown
        .get("bonuses")
        .and_then(serde_json::Value::as_array)
    {
        for bonus in bonuses {
            let is_active = bonus
                .get("status")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .is_some_and(|value| value.eq_ignore_ascii_case("ACTIVE"));
            if is_active {
                total += bonus
                    .get("usageLimit")
                    .and_then(coerce_json_f64)
                    .unwrap_or(0.0);
            }
        }
    }

    total
}

fn compute_kiro_current_usage(breakdown: &serde_json::Value) -> f64 {
    let mut total = breakdown
        .get("currentUsageWithPrecision")
        .and_then(coerce_json_f64)
        .unwrap_or(0.0);

    if breakdown
        .get("freeTrialInfo")
        .and_then(serde_json::Value::as_object)
        .is_some_and(|free_trial| {
            free_trial
                .get("freeTrialStatus")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .is_some_and(|value| value.eq_ignore_ascii_case("ACTIVE"))
        })
    {
        total += breakdown
            .get("freeTrialInfo")
            .and_then(|value| value.get("currentUsageWithPrecision"))
            .and_then(coerce_json_f64)
            .unwrap_or(0.0);
    }

    if let Some(bonuses) = breakdown
        .get("bonuses")
        .and_then(serde_json::Value::as_array)
    {
        for bonus in bonuses {
            let is_active = bonus
                .get("status")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .is_some_and(|value| value.eq_ignore_ascii_case("ACTIVE"));
            if is_active {
                total += bonus
                    .get("currentUsage")
                    .and_then(coerce_json_f64)
                    .unwrap_or(0.0);
            }
        }
    }

    total
}

pub fn parse_kiro_usage_response(
    value: &serde_json::Value,
    updated_at_unix_secs: u64,
) -> Option<serde_json::Value> {
    let root = value.as_object()?;
    let breakdown = root
        .get("usageBreakdownList")
        .and_then(serde_json::Value::as_array)
        .and_then(|items| items.first())?;

    let usage_limit = compute_kiro_total_usage_limit(breakdown);
    let current_usage = compute_kiro_current_usage(breakdown);
    let remaining = (usage_limit - current_usage).max(0.0);
    let usage_percentage = if usage_limit > 0.0 {
        ((current_usage / usage_limit) * 100.0).min(100.0)
    } else {
        0.0
    };

    let mut result = serde_json::Map::new();
    result.insert("current_usage".to_string(), json!(current_usage));
    result.insert("usage_limit".to_string(), json!(usage_limit));
    result.insert("remaining".to_string(), json!(remaining));
    result.insert("usage_percentage".to_string(), json!(usage_percentage));
    result.insert("updated_at".to_string(), json!(updated_at_unix_secs));

    if let Some(subscription_title) = root
        .get("subscriptionInfo")
        .and_then(|value| value.get("subscriptionTitle"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        result.insert("subscription_title".to_string(), json!(subscription_title));
    }

    if let Some(next_reset_at) = root
        .get("nextDateReset")
        .and_then(coerce_json_f64)
        .or_else(|| breakdown.get("nextDateReset").and_then(coerce_json_f64))
    {
        result.insert("next_reset_at".to_string(), json!(next_reset_at));
    }

    let email = root
        .get("desktopUserInfo")
        .and_then(|value| value.get("email"))
        .or_else(|| root.get("userInfo").and_then(|value| value.get("email")))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if let Some(email) = email {
        result.insert("email".to_string(), json!(email));
    }

    Some(serde_json::Value::Object(result))
}
