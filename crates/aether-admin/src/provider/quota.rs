use aether_contracts::ExecutionResult;
use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;
use serde_json::json;
use std::collections::BTreeMap;

use super::status as provider_status;

const OAUTH_ACCOUNT_BLOCK_PREFIX: &str = "[ACCOUNT_BLOCK] ";
const OAUTH_REFRESH_FAILED_PREFIX: &str = "[REFRESH_FAILED] ";
const OAUTH_EXPIRED_PREFIX: &str = "[OAUTH_EXPIRED] ";
const OAUTH_REQUEST_FAILED_PREFIX: &str = "[REQUEST_FAILED] ";
const CODEX_SPARK_LIMIT_NAME: &str = "GPT-5.3-Codex-Spark";

pub fn provider_auto_remove_banned_keys(config: Option<&serde_json::Value>) -> bool {
    config
        .and_then(|value| value.get("pool_advanced"))
        .and_then(serde_json::Value::as_object)
        .and_then(|object| object.get("auto_remove_banned_keys"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

pub fn provider_auto_remove_quota_exhausted_keys(config: Option<&serde_json::Value>) -> bool {
    config
        .and_then(|value| value.get("pool_advanced"))
        .and_then(serde_json::Value::as_object)
        .and_then(|object| object.get("auto_remove_quota_exhausted_keys"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

pub fn should_auto_remove_structured_reason(reason: Option<&str>) -> bool {
    provider_status::should_auto_remove_account_state(&provider_status::resolve_pool_account_state(
        None, None, reason,
    ))
}

fn oauth_reason_has_tag(reason: Option<&str>, tag: &str) -> bool {
    reason
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_some_and(|reason| {
            reason
                .lines()
                .map(str::trim)
                .any(|line| line.starts_with(tag))
        })
}

fn oauth_refresh_failure_is_terminal(reason: Option<&str>) -> bool {
    reason
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_some_and(|reason| {
            reason
                .lines()
                .map(str::trim)
                .filter(|line| line.starts_with(OAUTH_REFRESH_FAILED_PREFIX))
                .any(|line| {
                    let lowered = line.to_ascii_lowercase();
                    lowered.contains("invalid_grant")
                        || lowered.contains("invalid_refresh_token")
                        || lowered.contains("refresh_token_expired")
                        || lowered.contains("could not validate your refresh token")
                        || lowered.contains("refresh_token 无效")
                        || lowered.contains("已过期或已撤销")
                        || lowered.contains("已被使用并轮换")
                        || (lowered.contains("refresh token")
                            && ["expired", "revoked", "invalid", "reused"]
                                .iter()
                                .any(|keyword| lowered.contains(keyword)))
                })
        })
}

fn oauth_access_token_expired(key: &StoredProviderCatalogKey, now_unix_secs: u64) -> bool {
    let now_unix_secs = if now_unix_secs == 0 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_secs())
            .unwrap_or(0)
    } else {
        now_unix_secs
    };
    key.expires_at_unix_secs
        .is_none_or(|expires_at| expires_at == 0 || expires_at <= now_unix_secs)
}

pub fn should_auto_remove_oauth_invalid_key(
    key: &StoredProviderCatalogKey,
    candidate_reason: Option<&str>,
    access_token_invalid_proven: bool,
    now_unix_secs: u64,
) -> bool {
    if should_auto_remove_structured_reason(candidate_reason)
        || should_auto_remove_structured_reason(key.oauth_invalid_reason.as_deref())
    {
        return true;
    }

    let refresh_token_failed = oauth_reason_has_tag(candidate_reason, OAUTH_REFRESH_FAILED_PREFIX)
        || oauth_reason_has_tag(
            key.oauth_invalid_reason.as_deref(),
            OAUTH_REFRESH_FAILED_PREFIX,
        );
    if !refresh_token_failed {
        return false;
    }
    if !oauth_refresh_failure_is_terminal(candidate_reason)
        && !oauth_refresh_failure_is_terminal(key.oauth_invalid_reason.as_deref())
    {
        return false;
    }

    access_token_invalid_proven
        || oauth_reason_has_tag(key.oauth_invalid_reason.as_deref(), OAUTH_EXPIRED_PREFIX)
        || oauth_access_token_expired(key, now_unix_secs)
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
    if current_reason.starts_with(OAUTH_ACCOUNT_BLOCK_PREFIX) {
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
    let mut opaque_key_display_index = 1usize;

    for (model_id, model_value) in models {
        let mut payload = serde_json::Map::new();
        let upstream_display_name = coerce_json_string(
            model_value
                .get("displayName")
                .or_else(|| model_value.get("display_name")),
        );
        if let Some(display_name) = friendly_quota_display_name(
            upstream_display_name,
            model_id,
            &mut opaque_key_display_index,
        ) {
            payload.insert("display_name".to_string(), json!(display_name));
        }

        let quota_info = model_value
            .get("quotaInfo")
            .and_then(serde_json::Value::as_object);
        let remaining_fraction = quota_info
            .and_then(|object| object.get("remainingFraction"))
            .and_then(coerce_json_f64);
        if let Some(remaining_fraction) = remaining_fraction {
            let used_percent = ((1.0 - remaining_fraction).max(0.0) * 100.0).min(100.0);
            payload.insert("remaining_fraction".to_string(), json!(remaining_fraction));
            payload.insert("used_percent".to_string(), json!(used_percent));
        }
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

pub fn parse_gemini_cli_retrieve_user_quota_response(
    value: &serde_json::Value,
    updated_at_unix_secs: u64,
) -> Option<serde_json::Value> {
    let buckets = value.get("buckets")?.as_array()?;
    let mut quota_by_model = serde_json::Map::new();
    let mut opaque_key_display_index = 1usize;

    for bucket in buckets {
        if !bucket.is_object() {
            continue;
        }
        let model_id = first_json_string_by_paths(
            bucket,
            &[
                &["modelId"],
                &["model_id"],
                &["model"],
                &["modelName"],
                &["metadata", "modelId"],
                &["metadata", "model_id"],
                &["labels", "modelId"],
                &["labels", "model_id"],
            ],
        );
        let token_type = first_json_string_by_paths(
            bucket,
            &[
                &["tokenType"],
                &["token_type"],
                &["metadata", "tokenType"],
                &["metadata", "token_type"],
                &["labels", "tokenType"],
                &["labels", "token_type"],
            ],
        );
        let Some(quota_key) = model_id
            .as_deref()
            .or(token_type.as_deref())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
        else {
            continue;
        };
        let display_name = first_json_string_by_paths(
            bucket,
            &[
                &["displayName"],
                &["display_name"],
                &["metadata", "displayName"],
                &["metadata", "display_name"],
            ],
        )
        .or_else(|| model_id.clone())
        .or_else(|| token_type.clone())
        .unwrap_or_else(|| quota_key.clone());
        let display_name = friendly_quota_display_name(
            Some(display_name),
            &quota_key,
            &mut opaque_key_display_index,
        )
        .unwrap_or_else(|| quota_key.clone());
        let remaining_fraction = first_json_f64_by_paths(
            bucket,
            &[
                &["remainingFraction"],
                &["remaining_fraction"],
                &["quotaInfo", "remainingFraction"],
                &["quotaInfo", "remaining_fraction"],
                &["quota", "remainingFraction"],
                &["quota", "remaining_fraction"],
            ],
        )
        .map(|value| value.clamp(0.0, 1.0));
        let reset_time = first_json_value_by_paths(
            bucket,
            &[
                &["resetTime"],
                &["reset_time"],
                &["nextResetTime"],
                &["next_reset_time"],
                &["quotaInfo", "resetTime"],
                &["quotaInfo", "reset_time"],
                &["quota", "resetTime"],
                &["quota", "reset_time"],
            ],
        )
        .cloned()
        .filter(|value| !value.is_null());
        let reset_at = reset_time
            .as_ref()
            .and_then(parse_gemini_cli_reset_timestamp);
        let is_exhausted = first_json_bool_by_paths(
            bucket,
            &[
                &["isExhausted"],
                &["is_exhausted"],
                &["exhausted"],
                &["quotaInfo", "isExhausted"],
                &["quotaInfo", "is_exhausted"],
                &["quota", "isExhausted"],
                &["quota", "is_exhausted"],
            ],
        )
        .or_else(|| remaining_fraction.map(|value| value <= 1e-9));
        let remaining_amount = first_json_f64_by_paths(
            bucket,
            &[
                &["remainingAmount"],
                &["remaining_amount"],
                &["remaining"],
                &["remaining_value"],
                &["quotaInfo", "remainingAmount"],
                &["quotaInfo", "remaining_amount"],
                &["quotaInfo", "remaining"],
                &["quotaInfo", "remaining_value"],
                &["quota", "remainingAmount"],
                &["quota", "remaining_amount"],
                &["quota", "remaining"],
                &["quota", "remaining_value"],
            ],
        );
        let explicit_total = first_json_f64_by_paths(
            bucket,
            &[
                &["limit"],
                &["limitAmount"],
                &["limit_amount"],
                &["total"],
                &["totalAmount"],
                &["total_amount"],
                &["quotaInfo", "limit"],
                &["quotaInfo", "limitAmount"],
                &["quotaInfo", "limit_amount"],
                &["quotaInfo", "total"],
                &["quotaInfo", "totalAmount"],
                &["quotaInfo", "total_amount"],
                &["quota", "limit"],
                &["quota", "limitAmount"],
                &["quota", "limit_amount"],
                &["quota", "total"],
                &["quota", "totalAmount"],
                &["quota", "total_amount"],
            ],
        )
        .filter(|value| *value > 0.0);
        let total_amount = explicit_total.or_else(|| {
            remaining_amount
                .zip(remaining_fraction)
                .and_then(|(remaining, fraction)| {
                    (fraction > 0.0).then_some((remaining / fraction).round())
                })
        });

        let mut payload = serde_json::Map::new();
        payload.insert("display_name".to_string(), json!(display_name));
        if let Some(model_id) = model_id {
            payload.insert("model_id".to_string(), json!(model_id));
        }
        if let Some(token_type) = token_type {
            payload.insert("token_type".to_string(), json!(token_type));
        }
        if let Some(remaining_fraction) = remaining_fraction {
            payload.insert("remaining_fraction".to_string(), json!(remaining_fraction));
            payload.insert(
                "used_percent".to_string(),
                json!(((1.0 - remaining_fraction) * 100.0).clamp(0.0, 100.0)),
            );
        }
        if let Some(reset_time) = reset_time {
            payload.insert("reset_time".to_string(), reset_time);
        }
        if let Some(reset_at) = reset_at {
            payload.insert("reset_at".to_string(), json!(reset_at));
        }
        if let Some(is_exhausted) = is_exhausted {
            payload.insert("is_exhausted".to_string(), json!(is_exhausted));
        }
        if let Some(value) = total_amount {
            payload.insert("total".to_string(), json!(value));
        }
        if let Some(value) = remaining_amount {
            payload.insert("remaining".to_string(), json!(value));
        }

        quota_by_model.insert(quota_key, serde_json::Value::Object(payload));
    }

    if quota_by_model.is_empty() {
        return None;
    }

    Some(json!({
        "updated_at": updated_at_unix_secs,
        "quota_by_model": quota_by_model,
    }))
}

fn is_opaque_reset_credit_quota_identifier(value: &str) -> bool {
    value.trim().starts_with("RateLimitResetCredit_")
}

fn friendly_quota_display_name(
    candidate: Option<String>,
    quota_key: &str,
    opaque_key_display_index: &mut usize,
) -> Option<String> {
    let candidate = candidate
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

    if let Some(candidate) = candidate.as_deref() {
        if !is_opaque_reset_credit_quota_identifier(candidate) {
            return Some(candidate.to_string());
        }
    }

    if is_opaque_reset_credit_quota_identifier(quota_key)
        || candidate
            .as_deref()
            .is_some_and(is_opaque_reset_credit_quota_identifier)
    {
        let label = format!("Key-{}", *opaque_key_display_index);
        *opaque_key_display_index += 1;
        return Some(label);
    }

    candidate
}

pub fn parse_gemini_cli_v1internal_credits_response(
    value: &serde_json::Value,
    updated_at_unix_secs: u64,
) -> Option<serde_json::Value> {
    let mut credits = serde_json::Map::new();
    if let Some(value) = value.get("remainingCredits").and_then(coerce_json_f64) {
        credits.insert("remaining".to_string(), json!(value));
    }
    if let Some(value) = value.get("consumedCredits").and_then(coerce_json_f64) {
        credits.insert("consumed".to_string(), json!(value));
    }
    if let Some(value) = coerce_json_string(value.get("traceId")) {
        credits.insert("trace_id".to_string(), json!(value));
    }
    if credits.is_empty() {
        return None;
    }
    credits.insert("updated_at".to_string(), json!(updated_at_unix_secs));
    Some(serde_json::Value::Object(credits))
}

fn first_json_value_by_paths<'a>(
    value: &'a serde_json::Value,
    paths: &[&[&str]],
) -> Option<&'a serde_json::Value> {
    for path in paths {
        let mut current = value;
        let mut matched = true;
        for segment in *path {
            let Some(next) = current.get(*segment) else {
                matched = false;
                break;
            };
            current = next;
        }
        if matched {
            return Some(current);
        }
    }
    None
}

fn first_json_string_by_paths(value: &serde_json::Value, paths: &[&[&str]]) -> Option<String> {
    paths
        .iter()
        .find_map(|path| coerce_json_string(first_json_value_by_paths(value, &[*path])))
}

fn first_json_f64_by_paths(value: &serde_json::Value, paths: &[&[&str]]) -> Option<f64> {
    paths
        .iter()
        .find_map(|path| first_json_value_by_paths(value, &[*path]).and_then(coerce_json_f64))
}

fn first_json_bool_by_paths(value: &serde_json::Value, paths: &[&[&str]]) -> Option<bool> {
    paths
        .iter()
        .find_map(|path| first_json_value_by_paths(value, &[*path]).and_then(coerce_json_bool))
}

fn parse_gemini_cli_reset_timestamp(value: &serde_json::Value) -> Option<u64> {
    if let Some(value) = coerce_json_u64(value) {
        return Some(if value > 1_000_000_000_000 {
            value / 1000
        } else {
            value
        });
    }
    let raw = value.as_str()?.trim();
    if raw.is_empty() {
        return None;
    }
    chrono::DateTime::parse_from_rfc3339(raw)
        .ok()
        .and_then(|timestamp| u64::try_from(timestamp.timestamp()).ok())
}

fn normalize_glm_limit_type(value: &str) -> String {
    value.trim().to_ascii_uppercase()
}

fn glm_usage_data(value: &serde_json::Value) -> &serde_json::Value {
    value.get("data").unwrap_or(value)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlmCodingPlanBusinessErrorClass {
    CredentialInvalid,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlmCodingPlanBusinessError {
    pub code: Option<i64>,
    pub message: String,
    pub class: GlmCodingPlanBusinessErrorClass,
}

fn glm_usage_business_error(value: &serde_json::Value) -> Option<GlmCodingPlanBusinessError> {
    let object = value.as_object()?;
    let success = object
        .get("success")
        .and_then(coerce_json_bool)
        .unwrap_or(true);
    let code = object.get("code").and_then(coerce_json_i64);
    if success && code.is_none_or(|value| value == 0 || value == 200) {
        return None;
    }
    let message = ["msg", "message", "error"]
        .iter()
        .find_map(|key| {
            object
                .get(*key)
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        })
        .or_else(|| code.map(|value| format!("GLM Coding Plan returned code {value}")))
        .unwrap_or_else(|| "GLM Coding Plan returned a business error".to_string());
    let class = match code {
        Some(401 | 1000 | 1003) => GlmCodingPlanBusinessErrorClass::CredentialInvalid,
        Some(_) | None => GlmCodingPlanBusinessErrorClass::Other,
    };
    Some(GlmCodingPlanBusinessError {
        code,
        message,
        class,
    })
}

fn coerce_json_i64(value: &serde_json::Value) -> Option<i64> {
    if let Some(value) = value.as_i64() {
        return Some(value);
    }
    value.as_str()?.trim().parse::<i64>().ok()
}

pub fn glm_coding_plan_business_error(
    value: &serde_json::Value,
) -> Option<GlmCodingPlanBusinessError> {
    glm_usage_business_error(value)
}

pub fn parse_glm_coding_plan_usage_response(
    value: &serde_json::Value,
    usage_kind: &str,
    usage_window: Option<&str>,
    updated_at_unix_secs: u64,
) -> Option<serde_json::Value> {
    if glm_usage_business_error(value).is_some() {
        return None;
    }
    let data = glm_usage_data(value);
    if data.is_null() {
        return None;
    }

    let mut result = serde_json::Map::new();
    result.insert("updated_at".to_string(), json!(updated_at_unix_secs));
    let normalized_window = normalize_glm_usage_window(usage_window);
    let usage_key = normalized_window
        .map(|window| format!("{usage_kind}_{window}_usage"))
        .unwrap_or_else(|| format!("{usage_kind}_usage"));
    result.insert(usage_key, data.clone());
    if usage_kind == "model" {
        if let Some(window) = normalized_window {
            insert_glm_model_usage_stats(&mut result, data, window);
        }
    }
    Some(serde_json::Value::Object(result))
}

fn normalize_glm_usage_window(value: Option<&str>) -> Option<&'static str> {
    match value?.trim().to_ascii_lowercase().as_str() {
        "5h" | "tokens_5h" | "token_5h" => Some("5h"),
        "week" | "weekly" | "tokens_weekly" | "token_weekly" => Some("weekly"),
        _ => None,
    }
}

fn insert_glm_model_usage_stats(
    result: &mut serde_json::Map<String, serde_json::Value>,
    data: &serde_json::Value,
    window: &str,
) {
    if let Some(request_count) = glm_model_usage_request_count(data) {
        result.insert(
            format!("official_usage_{window}_request_count"),
            json!(request_count),
        );
    }
    if let Some(total_tokens) = glm_model_usage_total_tokens(data) {
        result.insert(
            format!("official_usage_{window}_total_tokens"),
            json!(total_tokens),
        );
    }
}

fn glm_model_usage_total_tokens(data: &serde_json::Value) -> Option<u64> {
    glm_usage_explicit_u64(
        data,
        &[
            "totalTokensUsage",
            "total_tokens_usage",
            "totalTokens",
            "total_tokens",
            "totalTokenCount",
            "total_token_count",
        ],
    )
    .or_else(|| {
        glm_usage_sum_object_array_values(
            data,
            &[
                "modelSummaryList",
                "model_summary_list",
                "modelDataList",
                "model_data_list",
                "items",
                "records",
            ],
            &[
                "totalTokens",
                "total_tokens",
                "totalTokenCount",
                "total_token_count",
                "tokens",
                "token_count",
            ],
        )
    })
    .or_else(|| glm_usage_sum_number_array_values(data, &["tokensUsage", "tokens_usage"]))
}

fn glm_model_usage_request_count(data: &serde_json::Value) -> Option<u64> {
    glm_usage_explicit_u64(
        data,
        &[
            "totalModelCallCount",
            "total_model_call_count",
            "requestCount",
            "request_count",
            "totalRequests",
            "total_requests",
            "totalCount",
            "total_count",
            "count",
        ],
    )
    .or_else(|| glm_usage_sum_number_array_values(data, &["modelCallCount", "model_call_count"]))
    .or_else(|| glm_usage_array_len(data, &["items", "records"]))
}

fn glm_usage_explicit_u64(data: &serde_json::Value, keys: &[&str]) -> Option<u64> {
    data.get("totalUsage")
        .or_else(|| data.get("total_usage"))
        .and_then(serde_json::Value::as_object)
        .and_then(|object| glm_usage_u64_from_object(object, keys))
        .or_else(|| {
            data.as_object()
                .and_then(|object| glm_usage_u64_from_object(object, keys))
        })
}

fn glm_usage_u64_from_object(
    object: &serde_json::Map<String, serde_json::Value>,
    keys: &[&str],
) -> Option<u64> {
    keys.iter()
        .find_map(|key| object.get(*key).and_then(coerce_json_u64))
}

fn glm_usage_sum_object_array_values(
    data: &serde_json::Value,
    array_keys: &[&str],
    value_keys: &[&str],
) -> Option<u64> {
    for array_key in array_keys {
        let Some(items) = data.get(*array_key).and_then(serde_json::Value::as_array) else {
            continue;
        };
        let mut total = 0u64;
        let mut found = false;
        for item in items.iter().filter_map(serde_json::Value::as_object) {
            if let Some(value) = glm_usage_u64_from_object(item, value_keys) {
                total = total.saturating_add(value);
                found = true;
            }
        }
        if found {
            return Some(total);
        }
    }
    None
}

fn glm_usage_sum_number_array_values(data: &serde_json::Value, array_keys: &[&str]) -> Option<u64> {
    for array_key in array_keys {
        let Some(items) = data.get(*array_key).and_then(serde_json::Value::as_array) else {
            continue;
        };
        let mut total = 0u64;
        let mut found = false;
        for item in items {
            if let Some(value) = coerce_json_u64(item) {
                total = total.saturating_add(value);
                found = true;
            }
        }
        if found {
            return Some(total);
        }
    }
    None
}

fn glm_usage_array_len(data: &serde_json::Value, array_keys: &[&str]) -> Option<u64> {
    for array_key in array_keys {
        let Some(items) = data.get(*array_key).and_then(serde_json::Value::as_array) else {
            continue;
        };
        if let Ok(len) = u64::try_from(items.len()) {
            return Some(len);
        }
    }
    None
}

pub fn parse_glm_coding_plan_quota_limit_response(
    value: &serde_json::Value,
    updated_at_unix_secs: u64,
) -> Option<serde_json::Value> {
    if glm_usage_business_error(value).is_some() {
        return None;
    }
    let data = glm_usage_data(value);
    let limits = data.get("limits").and_then(serde_json::Value::as_array)?;
    let mut result = serde_json::Map::new();
    let mut max_token_percent: Option<f64> = None;
    if let Some(plan_type) = coerce_json_string(
        data.get("level")
            .or_else(|| data.get("plan_type"))
            .or_else(|| data.get("planType"))
            .or_else(|| data.get("tier")),
    ) {
        let normalized = plan_type.to_ascii_lowercase();
        result.insert("plan_type".to_string(), json!(normalized));
        result.insert("pool_tier".to_string(), json!(plan_type));
    }

    for item in limits.iter().filter_map(serde_json::Value::as_object) {
        let limit_type = item
            .get("type")
            .and_then(serde_json::Value::as_str)
            .map(normalize_glm_limit_type)
            .unwrap_or_default();
        if limit_type.is_empty() {
            continue;
        }

        let percentage = item.get("percentage").and_then(coerce_json_f64);
        let current_value = item
            .get("currentValue")
            .or_else(|| item.get("current_value"))
            .and_then(coerce_json_f64);
        let usage = item.get("usage").and_then(coerce_json_f64);
        let unit = item.get("unit").and_then(coerce_json_u64).unwrap_or(0);
        let number = item.get("number").and_then(coerce_json_u64).unwrap_or(0);
        let next_reset_ms = item
            .get("nextResetTime")
            .or_else(|| item.get("next_reset_time"))
            .and_then(coerce_json_u64);

        if limit_type.as_str() != "TOKENS_LIMIT" {
            continue;
        }
        let window = match (unit, number) {
            (3, 5) => "5h",
            (6, 1) => "weekly",
            _ => "other",
        };
        if let Some(percentage) = percentage {
            max_token_percent = Some(max_token_percent.map_or(percentage, |m| m.max(percentage)));
            result.insert(format!("token_{window}_used_percent"), json!(percentage));
        }
        if let Some(reset_ms) = next_reset_ms {
            result.insert(format!("token_{window}_reset_at"), json!(reset_ms / 1000));
        }
        if let Some(current_value) = current_value {
            result.insert(
                format!("token_{window}_current_usage"),
                json!(current_value),
            );
            result.insert("token_current_usage".to_string(), json!(current_value));
        }
        if let Some(usage) = usage {
            result.insert(format!("token_{window}_usage_limit"), json!(usage));
            result.insert("token_usage_limit".to_string(), json!(usage));
        }
    }

    if let Some(max_pct) = max_token_percent {
        result.insert("token_used_percent".to_string(), json!(max_pct));
    }

    if result.is_empty() {
        return None;
    }
    result.insert("updated_at".to_string(), json!(updated_at_unix_secs));
    result.insert("quota_limit".to_string(), data.clone());
    Some(serde_json::Value::Object(result))
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
    if let Some(value) = source
        .get("limit_window_seconds")
        .and_then(coerce_json_u64)
        .map(|seconds| seconds / 60)
    {
        target.insert(format!("{target_prefix}_window_minutes"), json!(value));
    }
}

fn codex_window_has_active_limit(source: &serde_json::Map<String, serde_json::Value>) -> bool {
    [
        "window_minutes",
        "limit_window_seconds",
        "reset_after_seconds",
        "reset_at",
    ]
    .iter()
    .any(|key| {
        source
            .get(*key)
            .and_then(coerce_json_u64)
            .is_some_and(|value| value > 0)
    }) || source
        .get("used_percent")
        .and_then(coerce_json_f64)
        .is_some_and(|value| value > 0.0)
}

fn codex_find_spark_rate_limit(
    root: &serde_json::Map<String, serde_json::Value>,
) -> Option<&serde_json::Map<String, serde_json::Value>> {
    root.get("additional_rate_limits")
        .and_then(serde_json::Value::as_array)?
        .iter()
        .filter_map(serde_json::Value::as_object)
        .find(|item| {
            item.get("limit_name")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|name| name.trim() == CODEX_SPARK_LIMIT_NAME)
        })?
        .get("rate_limit")
        .and_then(serde_json::Value::as_object)
}

fn codex_reset_credits_container(
    root: &serde_json::Map<String, serde_json::Value>,
) -> Option<&serde_json::Map<String, serde_json::Value>> {
    [
        "rate_limit_reset_credits",
        "rateLimitResetCredits",
        "reset_credits",
        "resetCredits",
    ]
    .iter()
    .find_map(|key| root.get(*key).and_then(serde_json::Value::as_object))
}

fn codex_reset_credits_available_count(
    root: &serde_json::Map<String, serde_json::Value>,
) -> Option<u64> {
    let container = codex_reset_credits_container(root)?;
    [
        "available_count",
        "availableCount",
        "available",
        "remaining",
        "count",
    ]
    .iter()
    .find_map(|key| container.get(*key).and_then(coerce_json_u64))
}

fn codex_reset_credits_count_snapshot(
    available_count: u64,
    updated_at_unix_secs: u64,
) -> serde_json::Value {
    json!({
        "available_count": available_count,
        "updated_at": updated_at_unix_secs,
        "detail_source": "wham_usage",
        "detail_status": "not_requested",
        "credits": [],
    })
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

    let use_paid_windows =
        codex_window_has_active_limit(&secondary_window) && plan_type.as_deref() != Some("free");
    if use_paid_windows {
        codex_write_window(&mut result, &secondary_window, "primary");
        codex_write_window(&mut result, &primary_window, "secondary");
    } else {
        codex_write_window(&mut result, &primary_window, "primary");
    }

    if let Some(spark_rate_limit) = codex_find_spark_rate_limit(root) {
        if let Some(primary_window) = spark_rate_limit
            .get("primary_window")
            .and_then(serde_json::Value::as_object)
        {
            codex_write_window(&mut result, primary_window, "spark_primary");
        }
        if let Some(secondary_window) = spark_rate_limit
            .get("secondary_window")
            .and_then(serde_json::Value::as_object)
        {
            codex_write_window(&mut result, secondary_window, "spark_secondary");
        }
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

    if let Some(available_count) = codex_reset_credits_available_count(root) {
        result.insert(
            "reset_credits".to_string(),
            codex_reset_credits_count_snapshot(available_count, updated_at_unix_secs),
        );
    }

    if result.is_empty() {
        return None;
    }
    result.insert("updated_at".to_string(), json!(updated_at_unix_secs));
    Some(serde_json::Value::Object(result))
}

fn parse_codex_reset_credit_timestamp(value: Option<&serde_json::Value>) -> Option<u64> {
    let value = value?;
    if let Some(timestamp) = coerce_json_u64(value) {
        return Some(if timestamp > 1_000_000_000_000 {
            timestamp / 1000
        } else {
            timestamp
        });
    }
    let raw = value.as_str()?.trim();
    if raw.is_empty() {
        return None;
    }
    chrono::DateTime::parse_from_rfc3339(raw)
        .ok()
        .and_then(|timestamp| u64::try_from(timestamp.timestamp()).ok())
}

fn codex_reset_credit_detail_items(value: &serde_json::Value) -> Option<&Vec<serde_json::Value>> {
    if let Some(items) = value.as_array() {
        return Some(items);
    }

    first_json_value_by_paths(
        value,
        &[
            &["credits"],
            &["data"],
            &["items"],
            &["rate_limit_reset_credits", "credits"],
            &["rate_limit_reset_credits", "data"],
            &["rateLimitResetCredits", "credits"],
            &["rateLimitResetCredits", "data"],
            &["reset_credits", "credits"],
            &["resetCredits", "credits"],
            &["rate_limit_reset_credits"],
            &["rateLimitResetCredits"],
            &["reset_credits"],
            &["resetCredits"],
        ],
    )
    .and_then(serde_json::Value::as_array)
}

fn codex_reset_credit_id(object: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
    [
        "id",
        "credit_id",
        "creditId",
        "key",
        "idempotency_key",
        "idempotencyKey",
    ]
    .iter()
    .find_map(|key| coerce_json_string(object.get(*key)))
}

fn codex_reset_credit_display_key(id: &str) -> Option<String> {
    id.split('-')
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn codex_reset_credit_status(
    object: &serde_json::Map<String, serde_json::Value>,
) -> Option<String> {
    ["status", "state"]
        .iter()
        .find_map(|key| coerce_json_string(object.get(*key)))
}

fn codex_reset_credit_is_available(object: &serde_json::Map<String, serde_json::Value>) -> bool {
    let reset_type = ["reset_type", "resetType"]
        .iter()
        .find_map(|key| coerce_json_string(object.get(*key)));
    if reset_type
        .as_deref()
        .is_some_and(|value| !value.trim().eq_ignore_ascii_case("codex_rate_limits"))
    {
        return false;
    }

    codex_reset_credit_status(object).is_none_or(|status| {
        let status = status.trim();
        status.eq_ignore_ascii_case("available") || status.eq_ignore_ascii_case("active")
    })
}

fn parse_codex_reset_credit_detail_item(item: &serde_json::Value) -> Option<serde_json::Value> {
    let object = item.as_object()?;
    if !codex_reset_credit_is_available(object) {
        return None;
    }
    let expires_at = parse_codex_reset_credit_timestamp(
        object
            .get("expires_at")
            .or_else(|| object.get("expiresAt"))
            .or_else(|| object.get("expiration_time"))
            .or_else(|| object.get("expirationTime")),
    )?;
    let granted_at = parse_codex_reset_credit_timestamp(
        object
            .get("granted_at")
            .or_else(|| object.get("grantedAt"))
            .or_else(|| object.get("created_at"))
            .or_else(|| object.get("createdAt")),
    );

    let mut out = serde_json::Map::new();
    if let Some(id) = codex_reset_credit_id(object) {
        if let Some(display_key) = codex_reset_credit_display_key(&id) {
            out.insert("display_key".to_string(), json!(display_key));
        }
        out.insert("id".to_string(), json!(id));
    }
    if let Some(status) = codex_reset_credit_status(object) {
        out.insert("status".to_string(), json!(status));
    }
    if let Some(granted_at) = granted_at {
        out.insert("granted_at".to_string(), json!(granted_at));
    }
    out.insert("expires_at".to_string(), json!(expires_at));
    Some(serde_json::Value::Object(out))
}

pub fn parse_codex_wham_reset_credits_detail_response(
    value: &serde_json::Value,
    updated_at_unix_secs: u64,
) -> Option<serde_json::Value> {
    let root = value.as_object();
    let detail_items = codex_reset_credit_detail_items(value);
    if root.is_none() && detail_items.is_none() {
        return None;
    }

    let available_item_count = detail_items
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_object)
        .filter(|item| codex_reset_credit_is_available(item))
        .count();
    let available_count = root
        .and_then(codex_reset_credits_available_count)
        .or_else(|| {
            root.and_then(|root| {
                [
                    "available_count",
                    "availableCount",
                    "available",
                    "remaining",
                    "count",
                ]
                .iter()
                .find_map(|key| root.get(*key).and_then(coerce_json_u64))
            })
        })
        .or_else(|| detail_items.and_then(|_| u64::try_from(available_item_count).ok()));
    let mut credits = detail_items
        .into_iter()
        .flatten()
        .filter_map(parse_codex_reset_credit_detail_item)
        .collect::<Vec<_>>();
    credits.sort_by_key(|item| {
        item.get("expires_at")
            .and_then(coerce_json_u64)
            .unwrap_or(u64::MAX)
    });

    let detail_status = if available_count.is_some_and(|count| count > 0) {
        "available"
    } else {
        "empty"
    };
    let mut reset_credits = serde_json::Map::new();
    reset_credits.insert("updated_at".to_string(), json!(updated_at_unix_secs));
    reset_credits.insert("detail_source".to_string(), json!("wham_readonly"));
    reset_credits.insert("detail_status".to_string(), json!(detail_status));
    reset_credits.insert("credits".to_string(), serde_json::Value::Array(credits));

    if let Some(available_count) = available_count {
        reset_credits.insert("available_count".to_string(), json!(available_count));
    }

    Some(json!({ "reset_credits": reset_credits }))
}

pub fn normalize_codex_reset_credit_consume_outcome(
    value: Option<&serde_json::Value>,
) -> Option<String> {
    let object = value.and_then(serde_json::Value::as_object)?;
    let raw = ["outcome", "status", "result", "code"]
        .iter()
        .find_map(|key| coerce_json_string(object.get(*key)));
    if let Some(raw) = raw {
        let normalized = raw.trim().replace(['-', ' '], "_").to_ascii_lowercase();
        return match normalized.as_str() {
            "reset" | "success" | "redeemed" => Some("reset".to_string()),
            "alreadyredeemed" | "already_redeemed" => Some("already_redeemed".to_string()),
            "nothingtoreset" | "nothing_to_reset" => Some("nothing_to_reset".to_string()),
            "nocredit" | "no_credit" => Some("no_credit".to_string()),
            "error" | "failed" => Some("error".to_string()),
            _ => None,
        };
    }

    for (field, outcome) in [
        ("reset", "reset"),
        ("alreadyRedeemed", "already_redeemed"),
        ("already_redeemed", "already_redeemed"),
        ("nothingToReset", "nothing_to_reset"),
        ("nothing_to_reset", "nothing_to_reset"),
        ("noCredit", "no_credit"),
        ("no_credit", "no_credit"),
    ] {
        if object.get(field).and_then(coerce_json_bool) == Some(true) {
            return Some(outcome.to_string());
        }
    }

    None
}

fn codex_json_object<'a>(
    root: &'a serde_json::Map<String, serde_json::Value>,
    keys: &[&str],
) -> Option<&'a serde_json::Map<String, serde_json::Value>> {
    keys.iter()
        .find_map(|key| root.get(*key).and_then(serde_json::Value::as_object))
}

fn codex_json_string_from_object(
    object: Option<&serde_json::Map<String, serde_json::Value>>,
    keys: &[&str],
) -> Option<String> {
    let object = object?;
    keys.iter()
        .find_map(|key| coerce_json_string(object.get(*key)))
}

fn codex_json_string_from_root(
    root: &serde_json::Map<String, serde_json::Value>,
    keys: &[&str],
) -> Option<String> {
    keys.iter()
        .find_map(|key| coerce_json_string(root.get(*key)))
}

fn codex_backend_me_account_object(
    root: &serde_json::Map<String, serde_json::Value>,
) -> Option<&serde_json::Map<String, serde_json::Value>> {
    codex_json_object(root, &["account", "current_account", "selected_account"])
        .or_else(|| {
            root.get("accounts")
                .and_then(serde_json::Value::as_array)?
                .iter()
                .filter_map(serde_json::Value::as_object)
                .find(|account| {
                    account
                        .get("is_default")
                        .or_else(|| account.get("selected"))
                        .or_else(|| account.get("current"))
                        .and_then(coerce_json_bool)
                        .unwrap_or(false)
                })
        })
        .or_else(|| {
            root.get("accounts")
                .and_then(serde_json::Value::as_array)?
                .iter()
                .find_map(serde_json::Value::as_object)
        })
}

fn codex_backend_me_plan_object<'a>(
    root: &'a serde_json::Map<String, serde_json::Value>,
    account: Option<&'a serde_json::Map<String, serde_json::Value>>,
) -> Option<&'a serde_json::Map<String, serde_json::Value>> {
    codex_json_object(root, &["plan", "subscription", "workspace_plan"]).or_else(|| {
        account
            .and_then(|account| account.get("plan"))
            .and_then(serde_json::Value::as_object)
    })
}

pub fn parse_codex_backend_me_response(
    value: &serde_json::Value,
    updated_at_unix_secs: u64,
) -> Option<serde_json::Value> {
    let root = value.as_object()?;
    if root.is_empty() {
        return None;
    }

    let user = codex_json_object(root, &["user", "auth_user", "profile"]);
    let account = codex_backend_me_account_object(root);
    let plan = codex_backend_me_plan_object(root, account);
    let mut result = serde_json::Map::new();

    if let Some(user_id) = codex_json_string_from_object(user, &["id", "user_id"])
        .or_else(|| codex_json_string_from_root(root, &["user_id"]))
    {
        result.insert("user_id".to_string(), json!(user_id));
    }
    if let Some(email) = codex_json_string_from_object(user, &["email"])
        .or_else(|| codex_json_string_from_root(root, &["email"]))
    {
        result.insert("email".to_string(), json!(email));
    }
    if let Some(name) = codex_json_string_from_object(user, &["name", "display_name", "full_name"])
        .or_else(|| codex_json_string_from_root(root, &["name", "display_name", "full_name"]))
    {
        result.insert("user_name".to_string(), json!(name));
    }
    if let Some(account_id) =
        codex_json_string_from_object(account, &["id", "account_id", "accountId", "workspace_id"])
            .or_else(|| {
                codex_json_string_from_root(root, &["account_id", "accountId", "workspace_id"])
            })
    {
        result.insert("account_id".to_string(), json!(account_id));
    }
    if let Some(account_name) =
        codex_json_string_from_object(account, &["name", "title", "display_name"])
    {
        result.insert("account_name".to_string(), json!(account_name));
    }

    let plan_type = codex_json_string_from_object(
        account,
        &["plan_type", "planType", "subscription_plan", "tier"],
    )
    .or_else(|| codex_json_string_from_object(plan, &["type", "plan_type", "name", "tier"]))
    .or_else(|| codex_json_string_from_root(root, &["plan_type", "planType"]));
    if let Some(plan_type) = normalize_codex_plan_type(plan_type.as_deref()) {
        result.insert("plan_type".to_string(), json!(plan_type));
    }
    if let Some(plan_title) =
        codex_json_string_from_object(plan, &["title", "display_name", "label"])
    {
        result.insert("plan_title".to_string(), json!(plan_title));
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
    let use_paid_windows =
        codex_window_has_active_limit(&secondary_window) && plan_type.as_deref() != Some("free");
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
        && candidate_reason.starts_with(OAUTH_REFRESH_FAILED_PREFIX)
    {
        if current
            .lines()
            .map(str::trim)
            .any(|line| line.starts_with(OAUTH_REFRESH_FAILED_PREFIX))
        {
            return current.to_string();
        }
        return format!("{current}\n{candidate_reason}");
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
    lowered.contains("token_invalidated")
        || lowered.contains("authentication token has been invalidated")
        || lowered.contains("token has been invalidated")
        || lowered.contains("token invalidated")
        || lowered.contains("personal access token owner is inactive")
        || lowered.contains("biscuit_baker_service_auth_credential_error_status")
        || lowered.contains("auth_credential")
        || lowered.contains("invalidated")
        || lowered.contains("revoked")
        || lowered.contains("已撤销")
        || lowered.contains("被撤销")
        || lowered.contains("撤销")
        || lowered.contains("作废")
}

pub fn codex_looks_like_token_expired(message: Option<&str>) -> bool {
    let lowered = message.unwrap_or_default().trim().to_ascii_lowercase();
    lowered.contains("session has expired")
        || lowered.contains("session expired")
        || lowered.contains("access token expired")
        || lowered.contains("expired access token")
        || lowered.contains("token has expired")
        || lowered.contains("token expired")
        || lowered.contains("security token included in the request is expired")
        || lowered.contains("已过期")
        || lowered.contains("过期")
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
            "Codex Token 已失效"
        } else {
            message
        };
        return format!("{OAUTH_EXPIRED_PREFIX}{detail}");
    }
    if codex_looks_like_token_expired(Some(message)) {
        let detail = if message.is_empty() {
            "Codex Token 已过期"
        } else {
            message
        };
        return format!("{OAUTH_EXPIRED_PREFIX}{detail}");
    }
    if status_code == 401 {
        let detail = if message.is_empty() {
            "Codex Token 已过期 (401)"
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
    if status_code == 402 {
        let detail = if message.is_empty() {
            "Codex 账户需要付款 (402)"
        } else {
            message
        };
        return format!("{OAUTH_ACCOUNT_BLOCK_PREFIX}{detail}");
    }
    message.to_string()
}

pub fn codex_runtime_invalid_reason(
    status_code: u16,
    upstream_message: Option<&str>,
) -> Option<String> {
    match status_code {
        401 => Some(codex_structured_invalid_reason(401, upstream_message)),
        402 => Some(codex_structured_invalid_reason(402, upstream_message)),
        403 if codex_looks_like_token_invalidated(upstream_message)
            || codex_looks_like_token_expired(upstream_message)
            || codex_looks_like_account_deactivated(upstream_message) =>
        {
            Some(codex_structured_invalid_reason(403, upstream_message))
        }
        403 => Some(codex_generic_forbidden_runtime_invalid_reason(
            upstream_message,
        )),
        _ => None,
    }
}

fn codex_generic_forbidden_runtime_invalid_reason(upstream_message: Option<&str>) -> String {
    let detail = upstream_message
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|message| format!("Codex Token 已失效 (403): {message}"))
        .unwrap_or_else(|| "Codex Token 已失效 (403)".to_string());
    format!("{OAUTH_EXPIRED_PREFIX}{detail}")
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

pub fn parse_windsurf_user_status_response(
    value: &serde_json::Value,
    updated_at_unix_secs: u64,
) -> Option<serde_json::Value> {
    let user_status = value
        .get("userStatus")
        .or_else(|| value.get("user_status"))?;
    let plan_status = user_status
        .get("planStatus")
        .or_else(|| user_status.get("plan_status"))?;
    let plan_info = plan_status
        .get("planInfo")
        .or_else(|| plan_status.get("plan_info"));

    let mut result = serde_json::Map::new();
    result.insert("updated_at".to_string(), json!(updated_at_unix_secs));

    if let Some(plan_name) = plan_info
        .and_then(|value| {
            coerce_json_string(value.get("planName").or_else(|| value.get("plan_name")))
        })
        .or_else(|| {
            coerce_json_string(
                plan_status
                    .get("planName")
                    .or_else(|| plan_status.get("plan_name")),
            )
        })
    {
        result.insert("plan_name".to_string(), json!(plan_name));
    }
    if let Some(email) = coerce_json_string(user_status.get("email")) {
        result.insert("email".to_string(), json!(email));
    }
    if let Some(value) = plan_status
        .get("dailyQuotaRemainingPercent")
        .or_else(|| plan_status.get("daily_quota_remaining_percent"))
        .and_then(coerce_json_f64)
    {
        result.insert("daily_remaining_percent".to_string(), json!(value));
    }
    if let Some(value) = plan_status
        .get("weeklyQuotaRemainingPercent")
        .or_else(|| plan_status.get("weekly_quota_remaining_percent"))
        .and_then(coerce_json_f64)
    {
        result.insert("weekly_remaining_percent".to_string(), json!(value));
    }
    if let Some(value) = plan_status
        .get("dailyQuotaResetAtUnix")
        .or_else(|| plan_status.get("daily_quota_reset_at_unix"))
        .and_then(coerce_json_u64)
    {
        result.insert("daily_reset_at".to_string(), json!(value));
    }
    if let Some(value) = plan_status
        .get("weeklyQuotaResetAtUnix")
        .or_else(|| plan_status.get("weekly_quota_reset_at_unix"))
        .and_then(coerce_json_u64)
    {
        result.insert("weekly_reset_at".to_string(), json!(value));
    }
    if let Some(value) = plan_status
        .get("overageBalanceMicros")
        .or_else(|| plan_status.get("overage_balance_micros"))
        .and_then(coerce_json_f64)
    {
        result.insert("overage_balance".to_string(), json!(value / 1_000_000.0));
    }

    let legacy_credit =
        |value: Option<&serde_json::Value>| value.and_then(coerce_json_f64).map(|n| n / 100.0);
    if let Some(value) = legacy_credit(
        plan_status
            .get("availablePromptCredits")
            .or_else(|| plan_status.get("available_prompt_credits")),
    ) {
        result.insert("prompt_remaining".to_string(), json!(value));
    }
    if let Some(value) = legacy_credit(
        plan_status
            .get("usedPromptCredits")
            .or_else(|| plan_status.get("used_prompt_credits")),
    ) {
        result.insert("prompt_used".to_string(), json!(value));
    }
    if let Some(value) = legacy_credit(plan_info.and_then(|plan_info| {
        plan_info
            .get("monthlyPromptCredits")
            .or_else(|| plan_info.get("monthly_prompt_credits"))
    })) {
        result.insert("prompt_limit".to_string(), json!(value));
    }
    if let Some(value) = legacy_credit(
        plan_status
            .get("availableFlexCredits")
            .or_else(|| plan_status.get("available_flex_credits")),
    ) {
        result.insert("flex_remaining".to_string(), json!(value));
    }
    if let Some(value) = legacy_credit(
        plan_status
            .get("usedFlexCredits")
            .or_else(|| plan_status.get("used_flex_credits")),
    ) {
        result.insert("flex_used".to_string(), json!(value));
    }
    if let Some(value) = legacy_credit(plan_info.and_then(|plan_info| {
        plan_info
            .get("monthlyFlexCreditPurchaseAmount")
            .or_else(|| plan_info.get("monthly_flex_credit_purchase_amount"))
    })) {
        result.insert("flex_limit".to_string(), json!(value));
    }

    let mut status_sources = vec![value, user_status, plan_status];
    if let Some(plan_info) = plan_info {
        status_sources.push(plan_info);
    }
    for (target, aliases) in [
        (
            "banned",
            &[
                "banned",
                "isBanned",
                "is_banned",
                "accountBanned",
                "account_banned",
            ][..],
        ),
        (
            "quarantined",
            &[
                "quarantined",
                "isQuarantined",
                "is_quarantined",
                "accountQuarantined",
                "account_quarantined",
            ][..],
        ),
        (
            "is_forbidden",
            &[
                "isForbidden",
                "is_forbidden",
                "forbidden",
                "accountForbidden",
                "account_forbidden",
            ][..],
        ),
    ] {
        if let Some(found) = status_sources.iter().find_map(|source| {
            aliases
                .iter()
                .find_map(|alias| source.get(*alias).and_then(coerce_json_bool))
        }) {
            result.insert(target.to_string(), json!(found));
        }
    }
    for (target, aliases) in [
        (
            "ban_reason",
            &[
                "banReason",
                "ban_reason",
                "blockedReason",
                "blocked_reason",
                "reason",
                "message",
            ][..],
        ),
        (
            "quarantine_reason",
            &["quarantineReason", "quarantine_reason", "reason", "message"][..],
        ),
        (
            "forbidden_reason",
            &["forbiddenReason", "forbidden_reason", "reason", "message"][..],
        ),
    ] {
        if let Some(found) = status_sources.iter().find_map(|source| {
            aliases
                .iter()
                .find_map(|alias| coerce_json_string(source.get(*alias)))
        }) {
            result.insert(target.to_string(), json!(found));
        }
    }

    Some(serde_json::Value::Object(result))
}

pub fn parse_windsurf_model_configs_response(
    value: &serde_json::Value,
    updated_at_unix_secs: u64,
) -> Option<serde_json::Value> {
    let configs = value
        .get("clientModelConfigs")
        .or_else(|| value.get("client_model_configs"))
        .and_then(serde_json::Value::as_array)?;
    let mut models = Vec::new();
    for config in configs {
        let Some(model_uid) = coerce_json_string(
            config
                .get("modelUid")
                .or_else(|| config.get("model_uid"))
                .or_else(|| config.get("id"))
                .or_else(|| config.get("name")),
        ) else {
            continue;
        };
        let mut model = serde_json::Map::new();
        model.insert("model_uid".to_string(), json!(model_uid));
        if let Some(label) = coerce_json_string(
            config
                .get("label")
                .or_else(|| config.get("displayName"))
                .or_else(|| config.get("display_name")),
        ) {
            model.insert("label".to_string(), json!(label));
        }
        if let Some(provider) = coerce_json_string(config.get("provider")) {
            model.insert("provider".to_string(), json!(provider));
        }
        if let Some(value) = config
            .get("supportsImages")
            .or_else(|| config.get("supports_images"))
            .and_then(coerce_json_bool)
        {
            model.insert("supports_images".to_string(), json!(value));
        }
        if let Some(value) = config
            .get("creditMultiplier")
            .or_else(|| config.get("credit_multiplier"))
            .and_then(coerce_json_f64)
        {
            model.insert("credit_multiplier".to_string(), json!(value));
        }
        models.push(serde_json::Value::Object(model));
    }

    let mut result = serde_json::Map::new();
    result.insert("updated_at".to_string(), json!(updated_at_unix_secs));
    result.insert(
        "allowed_models_count".to_string(),
        json!(models.len() as u64),
    );
    result.insert("models".to_string(), serde_json::Value::Array(models));
    if let Some(default_model_uid) = value
        .get("defaultOverrideModelConfig")
        .or_else(|| value.get("default_override_model_config"))
        .and_then(|default_config| {
            coerce_json_string(
                default_config
                    .get("modelUid")
                    .or_else(|| default_config.get("model_uid")),
            )
        })
    {
        result.insert("default_model_uid".to_string(), json!(default_model_uid));
    }

    Some(serde_json::Value::Object(result))
}

pub fn parse_windsurf_rate_limit_response(
    value: &serde_json::Value,
    updated_at_unix_secs: u64,
) -> Option<serde_json::Value> {
    let root = value.as_object()?;
    if root.is_empty() {
        return None;
    }
    let has_capacity = value
        .get("hasCapacity")
        .or_else(|| value.get("has_capacity"))
        .and_then(coerce_json_bool)
        .unwrap_or(true);
    let messages_remaining = value
        .get("messagesRemaining")
        .or_else(|| value.get("messages_remaining"))
        .and_then(coerce_json_f64);
    let max_messages = value
        .get("maxMessages")
        .or_else(|| value.get("max_messages"))
        .and_then(coerce_json_f64);
    let retry_after_ms = value
        .get("retryAfterMs")
        .or_else(|| value.get("retry_after_ms"))
        .and_then(coerce_json_u64);

    let limited = !has_capacity || messages_remaining.is_some_and(|value| value <= 0.0);
    let mut rate_limit = serde_json::Map::new();
    rate_limit.insert("limited".to_string(), json!(limited));
    rate_limit.insert("has_capacity".to_string(), json!(has_capacity));
    if let Some(value) = messages_remaining {
        rate_limit.insert("messages_remaining".to_string(), json!(value));
    }
    if let Some(value) = max_messages {
        rate_limit.insert("max_messages".to_string(), json!(value));
    }
    if let Some(value) = retry_after_ms {
        rate_limit.insert("retry_after_ms".to_string(), json!(value));
    }

    Some(json!({
        "updated_at": updated_at_unix_secs,
        "rate_limit": rate_limit,
    }))
}

fn chatgpt_web_quota_feature_name(value: &serde_json::Value) -> Option<String> {
    coerce_json_string(
        value
            .get("feature_name")
            .or_else(|| value.get("featureName"))
            .or_else(|| value.get("feature"))
            .or_else(|| value.get("name")),
    )
}

fn chatgpt_web_is_image_quota_feature(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "image_gen" | "image_generation" | "image_edit" | "img_gen"
    )
}

fn chatgpt_web_feature_number(feature: &serde_json::Value, fields: &[&str]) -> Option<f64> {
    fields
        .iter()
        .find_map(|field| feature.get(*field).and_then(coerce_json_f64))
}

fn parse_chatgpt_web_reset_timestamp(
    value: Option<&serde_json::Value>,
    observed_at: u64,
) -> Option<u64> {
    let value = value?;
    if let Some(text) = value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(text) {
            return u64::try_from(parsed.timestamp()).ok();
        }
        if let Ok(parsed) = text.parse::<f64>() {
            return normalize_chatgpt_web_numeric_reset(parsed, observed_at);
        }
        return None;
    }
    value
        .as_f64()
        .and_then(|parsed| normalize_chatgpt_web_numeric_reset(parsed, observed_at))
}

fn normalize_chatgpt_web_numeric_reset(value: f64, observed_at: u64) -> Option<u64> {
    if !value.is_finite() || value <= 0.0 {
        return None;
    }
    if value > 1_000_000_000_000.0 {
        return Some((value / 1000.0).floor() as u64);
    }
    if value > 1_000_000_000.0 {
        return Some(value.floor() as u64);
    }
    Some(observed_at.saturating_add(value.floor() as u64))
}

fn chatgpt_web_blocked_features(value: &serde_json::Value) -> Vec<String> {
    value
        .get("blocked_features")
        .or_else(|| value.get("blockedFeatures"))
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

pub fn parse_chatgpt_web_conversation_init_response(
    value: &serde_json::Value,
    updated_at_unix_secs: u64,
) -> Option<serde_json::Value> {
    let root = value.as_object()?;
    let limits_progress = root
        .get("limits_progress")
        .or_else(|| root.get("limitsProgress"))
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let image_limit = limits_progress
        .iter()
        .find(|item| {
            chatgpt_web_quota_feature_name(item)
                .as_deref()
                .is_some_and(chatgpt_web_is_image_quota_feature)
        })
        .cloned();
    let blocked_features = chatgpt_web_blocked_features(value);
    let image_blocked = blocked_features
        .iter()
        .any(|feature| chatgpt_web_is_image_quota_feature(feature));

    if image_limit.is_none() && !image_blocked {
        return None;
    }

    let mut result = serde_json::Map::new();
    result.insert("updated_at".to_string(), json!(updated_at_unix_secs));

    if let Some(default_model_slug) = coerce_json_string(
        root.get("default_model_slug")
            .or_else(|| root.get("defaultModelSlug")),
    ) {
        result.insert("default_model_slug".to_string(), json!(default_model_slug));
    }
    if let Some(plan_type) = coerce_json_string(
        root.get("plan_type")
            .or_else(|| root.get("planType"))
            .or_else(|| root.get("subscription_plan")),
    ) {
        result.insert(
            "plan_type".to_string(),
            json!(plan_type.to_ascii_lowercase()),
        );
    }
    result.insert("blocked_features".to_string(), json!(blocked_features));
    result.insert(
        "limits_progress".to_string(),
        serde_json::Value::Array(limits_progress),
    );

    if image_blocked {
        result.insert("image_quota_blocked".to_string(), json!(true));
    }

    if let Some(image_limit) = image_limit.as_ref() {
        if let Some(feature_name) = chatgpt_web_quota_feature_name(image_limit) {
            result.insert("image_quota_feature_name".to_string(), json!(feature_name));
        }

        let remaining = chatgpt_web_feature_number(
            image_limit,
            &[
                "remaining",
                "remaining_value",
                "remainingValue",
                "remaining_count",
                "remainingCount",
            ],
        );
        let total = chatgpt_web_feature_number(
            image_limit,
            &[
                "max_value",
                "maxValue",
                "cap",
                "total",
                "limit",
                "quota",
                "usage_limit",
                "usageLimit",
            ],
        );
        let used = chatgpt_web_feature_number(
            image_limit,
            &[
                "used",
                "used_value",
                "usedValue",
                "consumed",
                "current_usage",
                "currentUsage",
            ],
        )
        .or_else(|| {
            total
                .zip(remaining)
                .map(|(total, remaining)| (total - remaining).max(0.0))
        });
        let reset_source = image_limit
            .get("reset_at")
            .or_else(|| image_limit.get("resetAt"))
            .or_else(|| image_limit.get("next_reset_at"))
            .or_else(|| image_limit.get("nextResetAt"))
            .or_else(|| image_limit.get("reset_after"))
            .or_else(|| image_limit.get("resetAfter"));
        let reset_at = parse_chatgpt_web_reset_timestamp(reset_source, updated_at_unix_secs);

        if let Some(remaining) = remaining {
            result.insert("image_quota_remaining".to_string(), json!(remaining));
        } else if image_blocked {
            result.insert("image_quota_remaining".to_string(), json!(0.0));
        }
        if let Some(total) = total {
            result.insert("image_quota_total".to_string(), json!(total));
        }
        if let Some(used) = used {
            result.insert("image_quota_used".to_string(), json!(used));
        }
        if let Some(reset_at) = reset_at {
            result.insert("image_quota_reset_at".to_string(), json!(reset_at));
        }
        if let Some(reset_after) = coerce_json_string(
            image_limit
                .get("reset_after")
                .or_else(|| image_limit.get("resetAfter")),
        ) {
            result.insert("image_quota_reset_after".to_string(), json!(reset_after));
        }
    } else if image_blocked {
        result.insert("image_quota_remaining".to_string(), json!(0.0));
    }

    Some(serde_json::Value::Object(result))
}

#[cfg(test)]
mod tests {
    use super::{
        codex_build_invalid_state, codex_runtime_invalid_reason,
        normalize_codex_reset_credit_consume_outcome, parse_antigravity_usage_response,
        parse_chatgpt_web_conversation_init_response, parse_codex_backend_me_response,
        parse_codex_usage_headers, parse_codex_wham_reset_credits_detail_response,
        parse_codex_wham_usage_response, parse_gemini_cli_retrieve_user_quota_response,
        parse_gemini_cli_v1internal_credits_response, parse_glm_coding_plan_quota_limit_response,
        parse_glm_coding_plan_usage_response, parse_windsurf_model_configs_response,
        parse_windsurf_rate_limit_response, parse_windsurf_user_status_response,
        provider_auto_remove_quota_exhausted_keys, quota_refresh_success_invalid_state,
        should_auto_remove_structured_reason, GlmCodingPlanBusinessErrorClass,
        OAUTH_ACCOUNT_BLOCK_PREFIX, OAUTH_EXPIRED_PREFIX, OAUTH_REFRESH_FAILED_PREFIX,
        OAUTH_REQUEST_FAILED_PREFIX,
    };
    use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    fn provider_auto_remove_quota_exhausted_keys_defaults_to_false() {
        assert!(!provider_auto_remove_quota_exhausted_keys(None));
        assert!(!provider_auto_remove_quota_exhausted_keys(Some(&json!({
            "pool_advanced": {}
        }))));
    }

    #[test]
    fn provider_auto_remove_quota_exhausted_keys_reads_pool_advanced_flag() {
        assert!(provider_auto_remove_quota_exhausted_keys(Some(&json!({
            "pool_advanced": {
                "auto_remove_quota_exhausted_keys": true
            }
        }))));
    }

    #[test]
    fn codex_runtime_invalid_reason_marks_401_as_expired() {
        assert_eq!(
            codex_runtime_invalid_reason(401, Some("session expired")),
            Some(format!("{OAUTH_EXPIRED_PREFIX}session expired"))
        );
    }

    #[test]
    fn codex_runtime_invalid_reason_marks_account_deactivated_403() {
        assert_eq!(
            codex_runtime_invalid_reason(403, Some("account has been deactivated")),
            Some(format!(
                "{OAUTH_ACCOUNT_BLOCK_PREFIX}account has been deactivated"
            ))
        );
    }

    #[test]
    fn codex_runtime_invalid_reason_marks_inactive_pat_owner_403_as_token_invalid() {
        assert_eq!(
            codex_runtime_invalid_reason(403, Some("Personal access token owner is inactive.")),
            Some(format!(
                "{OAUTH_EXPIRED_PREFIX}Personal access token owner is inactive."
            ))
        );
        assert_eq!(
            codex_runtime_invalid_reason(
                403,
                Some("biscuit_baker_service_auth_credential_error_status")
            ),
            Some(format!(
                "{OAUTH_EXPIRED_PREFIX}biscuit_baker_service_auth_credential_error_status"
            ))
        );
    }

    #[test]
    fn codex_runtime_invalid_reason_marks_402_as_account_blocked() {
        assert_eq!(
            codex_runtime_invalid_reason(402, Some("payment required")),
            Some(format!("{OAUTH_ACCOUNT_BLOCK_PREFIX}payment required"))
        );
    }

    #[test]
    fn codex_runtime_invalid_reason_marks_generic_403_as_token_invalid() {
        assert_eq!(
            codex_runtime_invalid_reason(403, Some("forbidden")),
            Some(format!(
                "{OAUTH_EXPIRED_PREFIX}Codex Token 已失效 (403): forbidden"
            ))
        );
    }

    #[test]
    fn codex_invalid_state_appends_refresh_failure_to_oauth_expired() {
        let mut key = StoredProviderCatalogKey::new(
            "key-1".to_string(),
            "provider-1".to_string(),
            "key-1".to_string(),
            "oauth".to_string(),
            None,
            true,
        )
        .expect("key should build");
        key.oauth_invalid_at_unix_secs = Some(100);
        key.oauth_invalid_reason = Some(format!("{OAUTH_EXPIRED_PREFIX}session expired"));

        assert_eq!(
            codex_build_invalid_state(
                &key,
                format!("{OAUTH_REFRESH_FAILED_PREFIX}Token 续期失败"),
                200,
            ),
            (
                Some(200),
                Some(format!(
                    "{OAUTH_EXPIRED_PREFIX}session expired\n{OAUTH_REFRESH_FAILED_PREFIX}Token 续期失败"
                ))
            )
        );
    }

    #[test]
    fn codex_invalid_state_keeps_oauth_expired_over_request_failure() {
        let mut key = StoredProviderCatalogKey::new(
            "key-1".to_string(),
            "provider-1".to_string(),
            "key-1".to_string(),
            "oauth".to_string(),
            None,
            true,
        )
        .expect("key should build");
        key.oauth_invalid_at_unix_secs = Some(100);
        key.oauth_invalid_reason = Some(format!("{OAUTH_EXPIRED_PREFIX}session expired"));

        assert_eq!(
            codex_build_invalid_state(
                &key,
                format!("{OAUTH_REQUEST_FAILED_PREFIX}账号状态检查失败"),
                200,
            ),
            (
                Some(100),
                Some(format!("{OAUTH_EXPIRED_PREFIX}session expired"))
            )
        );
    }

    #[test]
    fn codex_invalid_state_allows_account_block_to_override_oauth_expired() {
        let mut key = StoredProviderCatalogKey::new(
            "key-1".to_string(),
            "provider-1".to_string(),
            "key-1".to_string(),
            "oauth".to_string(),
            None,
            true,
        )
        .expect("key should build");
        key.oauth_invalid_at_unix_secs = Some(100);
        key.oauth_invalid_reason = Some(format!("{OAUTH_EXPIRED_PREFIX}session expired"));

        assert_eq!(
            codex_build_invalid_state(
                &key,
                format!("{OAUTH_ACCOUNT_BLOCK_PREFIX}account has been deactivated"),
                200,
            ),
            (
                Some(200),
                Some(format!(
                    "{OAUTH_ACCOUNT_BLOCK_PREFIX}account has been deactivated"
                ))
            )
        );
    }

    #[test]
    fn auto_remove_structured_reason_removes_oauth_token_invalidated() {
        assert!(should_auto_remove_structured_reason(Some(
            "[OAUTH_EXPIRED] token invalidated"
        )));
    }

    #[test]
    fn auto_remove_structured_reason_keeps_oauth_token_expired() {
        assert!(!should_auto_remove_structured_reason(Some(
            "[OAUTH_EXPIRED] session expired"
        )));
    }

    #[test]
    fn auto_remove_refresh_failed_after_access_token_expiry() {
        let mut key = StoredProviderCatalogKey::new(
            "key-1".to_string(),
            "provider-1".to_string(),
            "key-1".to_string(),
            "oauth".to_string(),
            None,
            true,
        )
        .expect("key should build");
        key.expires_at_unix_secs = Some(1_000);
        key.oauth_invalid_reason = Some(format!(
            "{OAUTH_REFRESH_FAILED_PREFIX}Token 续期失败 (401): refresh_token 无效、已过期或已撤销，请重新登录授权"
        ));

        assert!(!super::should_auto_remove_oauth_invalid_key(
            &key, None, false, 999
        ));
        assert!(super::should_auto_remove_oauth_invalid_key(
            &key, None, false, 1_000
        ));
    }

    #[test]
    fn auto_remove_combined_refresh_and_access_token_failure() {
        let mut key = StoredProviderCatalogKey::new(
            "key-1".to_string(),
            "provider-1".to_string(),
            "key-1".to_string(),
            "oauth".to_string(),
            None,
            true,
        )
        .expect("key should build");
        key.expires_at_unix_secs = Some(2_000);
        key.oauth_invalid_reason = Some(format!(
            "{OAUTH_REFRESH_FAILED_PREFIX}Token 续期失败 (401): refresh_token 无效、已过期或已撤销，请重新登录授权"
        ));

        assert!(super::should_auto_remove_oauth_invalid_key(
            &key, None, true, 1_000,
        ));
    }

    #[test]
    fn auto_remove_existing_oauth_expired_after_terminal_refresh_failure() {
        let mut key = StoredProviderCatalogKey::new(
            "key-1".to_string(),
            "provider-1".to_string(),
            "key-1".to_string(),
            "oauth".to_string(),
            None,
            true,
        )
        .expect("key should build");
        key.expires_at_unix_secs = Some(2_000);
        key.oauth_invalid_reason = Some(format!(
            "{OAUTH_EXPIRED_PREFIX}access token invalid\n{OAUTH_REFRESH_FAILED_PREFIX}Token 续期失败 (401): refresh_token 无效、已过期或已撤销，请重新登录授权"
        ));

        assert!(super::should_auto_remove_oauth_invalid_key(
            &key, None, false, 1_000,
        ));
    }

    #[test]
    fn candidate_oauth_expired_is_not_auto_remove_proof_by_itself() {
        let mut key = StoredProviderCatalogKey::new(
            "key-1".to_string(),
            "provider-1".to_string(),
            "key-1".to_string(),
            "oauth".to_string(),
            None,
            true,
        )
        .expect("key should build");
        key.expires_at_unix_secs = Some(2_000);
        key.oauth_invalid_reason = Some(format!(
            "{OAUTH_REFRESH_FAILED_PREFIX}Token 续期失败 (401): refresh_token 无效、已过期或已撤销，请重新登录授权"
        ));

        assert!(!super::should_auto_remove_oauth_invalid_key(
            &key,
            Some("[OAUTH_EXPIRED] access token invalid"),
            false,
            1_000,
        ));
    }

    #[test]
    fn oauth_token_invalid_is_auto_remove_proof_by_itself() {
        let mut key = StoredProviderCatalogKey::new(
            "key-1".to_string(),
            "provider-1".to_string(),
            "key-1".to_string(),
            "oauth".to_string(),
            None,
            true,
        )
        .expect("key should build");
        key.expires_at_unix_secs = Some(1_000);
        key.oauth_invalid_reason = Some("oauth_token_invalid".to_string());

        assert!(super::should_auto_remove_oauth_invalid_key(
            &key,
            Some("oauth_token_invalid"),
            false,
            1_001,
        ));
    }

    #[test]
    fn does_not_auto_remove_access_token_failure_without_refresh_failure() {
        let mut key = StoredProviderCatalogKey::new(
            "key-1".to_string(),
            "provider-1".to_string(),
            "key-1".to_string(),
            "oauth".to_string(),
            None,
            true,
        )
        .expect("key should build");
        key.expires_at_unix_secs = Some(1_000);
        key.oauth_invalid_reason = Some(format!("{OAUTH_EXPIRED_PREFIX}session expired"));

        assert!(!super::should_auto_remove_oauth_invalid_key(
            &key, None, false, 1_001
        ));
    }

    #[test]
    fn does_not_auto_remove_non_terminal_refresh_failure() {
        let mut key = StoredProviderCatalogKey::new(
            "key-1".to_string(),
            "provider-1".to_string(),
            "key-1".to_string(),
            "oauth".to_string(),
            None,
            true,
        )
        .expect("key should build");
        key.expires_at_unix_secs = Some(1_000);
        key.oauth_invalid_reason = Some(format!("{OAUTH_REFRESH_FAILED_PREFIX}Token 续期失败"));

        assert!(!super::should_auto_remove_oauth_invalid_key(
            &key, None, true, 1_001
        ));
    }

    #[test]
    fn quota_refresh_success_clears_refresh_failed_marker() {
        let mut key = StoredProviderCatalogKey::new(
            "key-1".to_string(),
            "provider-1".to_string(),
            "key-1".to_string(),
            "oauth".to_string(),
            None,
            true,
        )
        .expect("key should build");
        key.oauth_invalid_reason = Some("[REFRESH_FAILED] Token 续期失败".to_string());

        assert_eq!(quota_refresh_success_invalid_state(&key), (None, None));
    }

    #[test]
    fn auto_remove_structured_reason_keeps_request_and_refresh_failures() {
        assert!(!should_auto_remove_structured_reason(Some(
            "[REQUEST_FAILED] 账号状态检查失败"
        )));
        assert!(!should_auto_remove_structured_reason(Some(
            "[REFRESH_FAILED] Token 续期失败 (401): refresh_token 已失效"
        )));
    }

    #[test]
    fn parses_codex_spark_quota_from_additional_rate_limits() {
        let parsed = parse_codex_wham_usage_response(
            &json!({
                "plan_type": "plus",
                "rate_limit": {
                    "primary_window": {
                        "used_percent": 25.0,
                        "reset_after_seconds": 604800,
                        "reset_at": 1_900_000_000u64
                    },
                    "secondary_window": {
                        "used_percent": 10.0,
                        "reset_after_seconds": 18000,
                        "reset_at": 1_800_000_000u64
                    }
                },
                "additional_rate_limits": [{
                    "limit_name": "GPT-5.3-Codex-Spark",
                    "metered_feature": "codex_bengalfox",
                    "rate_limit": {
                        "primary_window": {
                            "used_percent": 40.0,
                            "limit_window_seconds": 18000,
                            "reset_after_seconds": 9000,
                            "reset_at": 1_780_000_000u64
                        },
                        "secondary_window": {
                            "used_percent": 5.0,
                            "limit_window_seconds": 604800,
                            "reset_after_seconds": 300000,
                            "reset_at": 1_790_000_000u64
                        }
                    }
                }]
            }),
            1_777_000_000,
        )
        .expect("codex wham usage should parse");

        assert_eq!(parsed.get("primary_used_percent"), Some(&json!(10.0)));
        assert_eq!(parsed.get("secondary_used_percent"), Some(&json!(25.0)));
        assert_eq!(parsed.get("spark_primary_used_percent"), Some(&json!(40.0)));
        assert_eq!(
            parsed.get("spark_primary_window_minutes"),
            Some(&json!(300u64))
        );
        assert_eq!(
            parsed.get("spark_secondary_used_percent"),
            Some(&json!(5.0))
        );
        assert_eq!(
            parsed.get("spark_secondary_window_minutes"),
            Some(&json!(10_080u64))
        );
    }

    #[test]
    fn parses_codex_monthly_header_without_zero_secondary_placeholder() {
        let headers = BTreeMap::from([
            ("x-codex-plan-type".to_string(), "team".to_string()),
            ("x-codex-primary-used-percent".to_string(), "14".to_string()),
            (
                "x-codex-primary-reset-after-seconds".to_string(),
                "2627672".to_string(),
            ),
            (
                "x-codex-primary-reset-at".to_string(),
                "1786915122".to_string(),
            ),
            (
                "x-codex-primary-window-minutes".to_string(),
                "43800".to_string(),
            ),
            (
                "x-codex-secondary-used-percent".to_string(),
                "0".to_string(),
            ),
            (
                "x-codex-secondary-reset-after-seconds".to_string(),
                "0".to_string(),
            ),
            ("x-codex-secondary-reset-at".to_string(), "".to_string()),
            (
                "x-codex-secondary-window-minutes".to_string(),
                "0".to_string(),
            ),
        ]);

        let parsed = parse_codex_usage_headers(&headers, 1_784_287_450)
            .expect("Codex usage headers should parse");

        assert_eq!(parsed.get("primary_used_percent"), Some(&json!(14.0)));
        assert_eq!(
            parsed.get("primary_window_minutes"),
            Some(&json!(43_800u64))
        );
        assert!(parsed.get("secondary_used_percent").is_none());
        assert!(parsed.get("secondary_window_minutes").is_none());
    }

    #[test]
    fn parses_codex_reset_credit_count_from_wham_usage() {
        let parsed = parse_codex_wham_usage_response(
            &json!({
                "plan_type": "plus",
                "rate_limit": {
                    "primary_window": {
                        "used_percent": 25.0,
                        "reset_after_seconds": 604800
                    }
                },
                "rate_limit_reset_credits": {
                    "available_count": 2
                }
            }),
            1_777_000_000,
        )
        .expect("codex wham usage should parse");

        assert_eq!(
            parsed.pointer("/reset_credits/available_count"),
            Some(&json!(2u64))
        );
        assert_eq!(
            parsed.pointer("/reset_credits/detail_status"),
            Some(&json!("not_requested"))
        );
        assert_eq!(
            parsed.pointer("/reset_credits/updated_at"),
            Some(&json!(1_777_000_000u64))
        );
    }

    #[test]
    fn parses_codex_reset_credit_detail_sorted_by_expiry() {
        let parsed = parse_codex_wham_reset_credits_detail_response(
            &json!({
                "credits": [
                    {
                        "idempotencyKey": "bbbbbbbb-1111-2222-3333-444444444444",
                        "status": "available",
                        "expiresAt": "2030-01-04T00:00:00Z"
                    },
                    {
                        "idempotencyKey": "aaaaaaaa-1111-2222-3333-444444444444",
                        "status": "available",
                        "grantedAt": 1_893_456_000_000u64,
                        "expiresAt": "2030-01-02T00:00:00Z"
                    }
                ]
            }),
            1_777_000_000,
        )
        .expect("detail should parse");

        assert_eq!(
            parsed.pointer("/reset_credits/detail_status"),
            Some(&json!("available"))
        );
        assert_eq!(
            parsed.pointer("/reset_credits/credits/0/display_key"),
            Some(&json!("aaaaaaaa"))
        );
        assert_eq!(
            parsed.pointer("/reset_credits/credits/0/granted_at"),
            Some(&json!(1_893_456_000u64))
        );
        assert_eq!(
            parsed.pointer("/reset_credits/credits/1/display_key"),
            Some(&json!("bbbbbbbb"))
        );
    }

    #[test]
    fn parses_codex_reset_credit_detail_without_explicit_count_or_ids() {
        let parsed = parse_codex_wham_reset_credits_detail_response(
            &json!({
                "rate_limit_reset_credits": [
                    {
                        "resetType": "codex_rate_limits",
                        "status": "available",
                        "expiresAt": "2030-01-02T00:00:00Z"
                    },
                    {
                        "reset_type": "codex_rate_limits",
                        "status": "available"
                    },
                    {
                        "reset_type": "codex_rate_limits",
                        "status": "redeemed",
                        "expires_at": "2030-01-03T00:00:00Z"
                    }
                ]
            }),
            1_777_000_000,
        )
        .expect("detail array should parse");

        assert_eq!(
            parsed.pointer("/reset_credits/available_count"),
            Some(&json!(2u64))
        );
        assert_eq!(
            parsed.pointer("/reset_credits/credits/0/expires_at"),
            Some(&json!(1_893_542_400u64))
        );
        assert_eq!(parsed.pointer("/reset_credits/credits/0/id"), None);
    }

    #[test]
    fn parses_codex_reset_credit_detail_from_top_level_array() {
        let parsed = parse_codex_wham_reset_credits_detail_response(
            &json!([
                {
                    "status": "available",
                    "expires_at": "2030-01-04T00:00:00Z"
                }
            ]),
            1_777_000_000,
        )
        .expect("top-level detail array should parse");

        assert_eq!(
            parsed.pointer("/reset_credits/available_count"),
            Some(&json!(1u64))
        );
    }

    #[test]
    fn normalizes_codex_reset_credit_consume_outcome() {
        assert_eq!(
            normalize_codex_reset_credit_consume_outcome(Some(&json!({
                "outcome": "alreadyRedeemed"
            }))),
            Some("already_redeemed".to_string())
        );
        assert_eq!(
            normalize_codex_reset_credit_consume_outcome(Some(&json!({
                "noCredit": true
            }))),
            Some("no_credit".to_string())
        );
    }

    #[test]
    fn parses_codex_backend_me_identity_metadata_without_quota_windows() {
        let parsed = parse_codex_backend_me_response(
            &json!({
                "user": {
                    "id": "user-codex-123",
                    "email": "codex@example.com",
                    "name": "Codex User"
                },
                "account": {
                    "id": "acct-codex-123",
                    "name": "Personal",
                    "plan_type": "plus"
                },
                "plan": {
                    "type": "Plus",
                    "title": "ChatGPT Plus"
                }
            }),
            1_777_000_000,
        )
        .expect("codex backend me should parse");

        assert_eq!(parsed.get("user_id"), Some(&json!("user-codex-123")));
        assert_eq!(parsed.get("email"), Some(&json!("codex@example.com")));
        assert_eq!(parsed.get("account_id"), Some(&json!("acct-codex-123")));
        assert_eq!(parsed.get("account_name"), Some(&json!("Personal")));
        assert_eq!(parsed.get("plan_type"), Some(&json!("plus")));
        assert_eq!(parsed.get("plan_title"), Some(&json!("ChatGPT Plus")));
        assert_eq!(parsed.get("updated_at"), Some(&json!(1_777_000_000u64)));
        assert!(parsed.get("primary_used_percent").is_none());
        assert!(parsed.get("secondary_used_percent").is_none());
    }

    #[test]
    fn parses_antigravity_usage_response_labels_opaque_reset_credit_keys() {
        let parsed = parse_antigravity_usage_response(
            &json!({
                "models": {
                    "RateLimitResetCredit_05cbb6eeeb9c81918e011d8300f9ebfb": {
                        "quotaInfo": {
                            "remainingFraction": 0.75,
                            "resetTime": "2030-01-01T00:00:00Z"
                        }
                    },
                    "gemini-3-pro-preview": {
                        "displayName": "Gemini 3 Pro Preview",
                        "quotaInfo": {
                            "remainingFraction": 0.25
                        }
                    }
                }
            }),
            1_777_000_000,
        )
        .expect("antigravity quota should parse");

        assert_eq!(
            parsed["models"]["RateLimitResetCredit_05cbb6eeeb9c81918e011d8300f9ebfb"]
                ["display_name"],
            json!("Key-1")
        );
        assert_eq!(
            parsed["models"]["gemini-3-pro-preview"]["display_name"],
            json!("Gemini 3 Pro Preview")
        );
    }

    #[test]
    fn parses_gemini_cli_retrieve_user_quota_buckets() {
        let parsed = parse_gemini_cli_retrieve_user_quota_response(
            &json!({
                "buckets": [
                    {
                        "modelId": "gemini-2.5-pro",
                        "tokenType": "model",
                        "displayName": "Gemini 2.5 Pro",
                        "remainingFraction": 0.25,
                        "remainingAmount": "25",
                        "resetTime": "2030-01-01T00:00:00Z",
                        "isExhausted": false
                    },
                    {
                        "modelId": "gemini-2.5-flash",
                        "tokenType": "model",
                        "displayName": "Gemini 2.5 Flash",
                        "quotaInfo": {
                            "remainingFraction": 0.0,
                            "resetTime": 1_893_459_600_000u64,
                            "isExhausted": true
                        }
                    }
                ]
            }),
            1_777_000_000,
        )
        .expect("gemini cli quota should parse");

        assert_eq!(parsed.get("updated_at"), Some(&json!(1_777_000_000u64)));
        assert_eq!(
            parsed["quota_by_model"]["gemini-2.5-pro"]["remaining_fraction"],
            json!(0.25)
        );
        assert_eq!(
            parsed["quota_by_model"]["gemini-2.5-pro"]["remaining"],
            json!(25.0)
        );
        assert_eq!(
            parsed["quota_by_model"]["gemini-2.5-pro"]["total"],
            json!(100.0)
        );
        assert_eq!(
            parsed["quota_by_model"]["gemini-2.5-pro"]["reset_at"],
            json!(1_893_456_000u64)
        );
        assert_eq!(
            parsed["quota_by_model"]["gemini-2.5-flash"]["is_exhausted"],
            json!(true)
        );
        assert_eq!(
            parsed["quota_by_model"]["gemini-2.5-flash"]["used_percent"],
            json!(100.0)
        );
    }

    #[test]
    fn parses_gemini_cli_quota_buckets_labels_opaque_reset_credit_keys() {
        let parsed = parse_gemini_cli_retrieve_user_quota_response(
            &json!({
                "buckets": [
                    {
                        "tokenType": "RateLimitResetCredit_05cbb6eeeb9c81918e011d8300f9ebfb",
                        "remainingFraction": 0.5
                    },
                    {
                        "modelId": "RateLimitResetCredit_d18b8aac4ec2472697ad747a14975ac8",
                        "displayName": "RateLimitResetCredit_d18b8aac4ec2472697ad747a14975ac8",
                        "remainingFraction": 0.25
                    }
                ]
            }),
            1_777_000_000,
        )
        .expect("gemini cli quota should parse");

        assert_eq!(
            parsed["quota_by_model"]["RateLimitResetCredit_05cbb6eeeb9c81918e011d8300f9ebfb"]
                ["display_name"],
            json!("Key-1")
        );
        assert_eq!(
            parsed["quota_by_model"]["RateLimitResetCredit_d18b8aac4ec2472697ad747a14975ac8"]
                ["display_name"],
            json!("Key-2")
        );
    }

    #[test]
    fn parses_gemini_cli_v1internal_credits() {
        let parsed = parse_gemini_cli_v1internal_credits_response(
            &json!({
                "response": {"candidates": []},
                "remainingCredits": "41.5",
                "consumedCredits": 1,
                "traceId": "trace-upstream-sync-1"
            }),
            1_777_000_123,
        )
        .expect("gemini cli credits should parse");

        assert_eq!(parsed.get("remaining"), Some(&json!(41.5)));
        assert_eq!(parsed.get("consumed"), Some(&json!(1.0)));
        assert_eq!(
            parsed.get("trace_id"),
            Some(&json!("trace-upstream-sync-1"))
        );
        assert_eq!(parsed.get("updated_at"), Some(&json!(1_777_000_123u64)));
    }

    #[test]
    fn parses_glm_coding_plan_quota_limit_response() {
        let parsed = parse_glm_coding_plan_quota_limit_response(
            &json!({
                "data": {
                    "level": "max",
                    "limits": [
                        {
                            "type": "TOKENS_LIMIT",
                            "unit": 3,
                            "number": 5,
                            "percentage": 2,
                            "currentValue": 20,
                            "usage": 1000,
                            "nextResetTime": 1782257194399_u64
                        },
                        {
                            "type": "TOKENS_LIMIT",
                            "unit": 6,
                            "number": 1,
                            "percentage": 36,
                            "currentValue": 360,
                            "usage": 1000,
                            "nextResetTime": 1782698593995_u64
                        },
                        {
                            "type": "TIME_LIMIT",
                            "unit": 5,
                            "number": 1,
                            "percentage": 0,
                            "currentValue": 0,
                            "usage": 4000,
                            "nextResetTime": 1783303393993_u64
                        }
                    ]
                }
            }),
            1_234,
        )
        .expect("glm coding plan quota limit should parse");

        assert_eq!(parsed.get("updated_at"), Some(&json!(1_234)));
        assert_eq!(parsed.get("token_used_percent"), Some(&json!(36.0)));
        assert_eq!(parsed.get("plan_type"), Some(&json!("max")));
        assert_eq!(parsed.get("pool_tier"), Some(&json!("max")));
        assert_eq!(parsed.get("token_5h_used_percent"), Some(&json!(2.0)));
        assert_eq!(parsed.get("token_5h_current_usage"), Some(&json!(20.0)));
        assert_eq!(parsed.get("token_5h_usage_limit"), Some(&json!(1000.0)));
        assert_eq!(parsed.get("token_weekly_used_percent"), Some(&json!(36.0)));
        assert_eq!(
            parsed.get("token_weekly_current_usage"),
            Some(&json!(360.0))
        );
        assert_eq!(parsed.get("token_weekly_usage_limit"), Some(&json!(1000.0)));
        assert_eq!(
            parsed.get("token_5h_reset_at"),
            Some(&json!(1782257194_u64))
        );
        assert_eq!(
            parsed.get("token_weekly_reset_at"),
            Some(&json!(1782698593_u64))
        );
        assert!(parsed.get("mcp_used_percent").is_none());
        assert!(parsed.get("mcp_usage_limit").is_none());
    }

    #[test]
    fn parses_glm_coding_plan_model_usage_payloads_into_official_window_stats() {
        let parsed = parse_glm_coding_plan_usage_response(
            &json!({
                "data": {
                    "totalUsage": {
                        "totalModelCallCount": 90,
                        "totalTokensUsage": 3_718_682
                    },
                    "tokensUsage": [1_000, 2_000],
                    "modelCallCount": [10, 20],
                    "modelSummaryList": [
                        {"modelName": "GLM-5.2", "totalTokens": 3_700_000}
                    ]
                }
            }),
            "model",
            Some("5h"),
            1_234,
        )
        .expect("glm coding plan usage should parse");

        assert_eq!(parsed.get("updated_at"), Some(&json!(1_234)));
        assert_eq!(
            parsed["model_5h_usage"]["totalUsage"]["totalTokensUsage"],
            json!(3_718_682)
        );
        assert_eq!(
            parsed.get("official_usage_5h_request_count"),
            Some(&json!(90))
        );
        assert_eq!(
            parsed.get("official_usage_5h_total_tokens"),
            Some(&json!(3_718_682))
        );
    }

    #[test]
    fn parses_glm_coding_plan_model_usage_with_items_fallback() {
        let parsed = parse_glm_coding_plan_usage_response(
            &json!({
                "data": {
                    "items": [
                        {"model": "glm-a", "totalTokens": 100},
                        {"model": "glm-b", "totalTokens": 250}
                    ]
                }
            }),
            "model",
            Some("weekly"),
            1_234,
        )
        .expect("glm coding plan fallback usage should parse");

        assert_eq!(
            parsed.get("official_usage_weekly_request_count"),
            Some(&json!(2))
        );
        assert_eq!(
            parsed.get("official_usage_weekly_total_tokens"),
            Some(&json!(350))
        );
    }

    #[test]
    fn rejects_glm_coding_plan_business_error_payloads() {
        let payload = json!({
            "code": 401,
            "msg": "token expired or incorrect",
            "success": false
        });

        assert_eq!(
            parse_glm_coding_plan_usage_response(&payload, "model", Some("5h"), 1_234),
            None
        );
        assert_eq!(
            parse_glm_coding_plan_quota_limit_response(&payload, 1_234),
            None
        );
    }

    #[test]
    fn classifies_glm_business_errors_without_conflating_authentication() {
        // Given
        let credential_invalid_codes = [json!(401), json!("401"), json!(1000), json!(1003)];
        let other_codes = [
            json!(1001),
            json!(1005),
            json!(1220),
            json!(1308),
            json!(1309),
            json!(1310),
            json!(1313),
            json!(9999),
        ];

        // When / Then
        for code in credential_invalid_codes {
            let error = super::glm_coding_plan_business_error(&json!({
                "code": code,
                "success": false
            }))
            .expect("business error should parse");
            assert_eq!(
                error.class,
                GlmCodingPlanBusinessErrorClass::CredentialInvalid
            );
        }
        for code in other_codes {
            let error = super::glm_coding_plan_business_error(&json!({
                "code": code,
                "success": false
            }))
            .expect("business error should parse");
            assert_eq!(error.class, GlmCodingPlanBusinessErrorClass::Other);
        }
        let error = super::glm_coding_plan_business_error(&json!({
            "msg": "unknown business failure",
            "success": false
        }))
        .expect("business error should parse");
        assert_eq!(error.class, GlmCodingPlanBusinessErrorClass::Other);
    }

    #[test]
    fn parses_windsurf_user_status_response() {
        let parsed = parse_windsurf_user_status_response(
            &json!({
                "userStatus": {
                    "email": "windsurf@example.com",
                    "isQuarantined": true,
                    "quarantineReason": "quota review",
                    "planStatus": {
                        "dailyQuotaRemainingPercent": 45.5,
                        "weeklyQuotaRemainingPercent": 80,
                        "dailyQuotaResetAtUnix": "1775553285",
                        "weeklyQuotaResetAtUnix": 1776158085u64,
                        "availablePromptCredits": 900,
                        "usedPromptCredits": 100,
                        "availableFlexCredits": 250,
                        "usedFlexCredits": 50,
                        "overageBalanceMicros": 1250000,
                        "planInfo": {
                            "planName": "Pro",
                            "monthlyPromptCredits": 1000,
                            "monthlyFlexCreditPurchaseAmount": 300
                        }
                    }
                }
            }),
            1_770_000_000,
        )
        .expect("windsurf user status should parse");

        assert_eq!(parsed.get("plan_name"), Some(&json!("Pro")));
        assert_eq!(parsed.get("daily_remaining_percent"), Some(&json!(45.5)));
        assert_eq!(parsed.get("weekly_remaining_percent"), Some(&json!(80.0)));
        assert_eq!(parsed.get("daily_reset_at"), Some(&json!(1_775_553_285u64)));
        assert_eq!(
            parsed.get("weekly_reset_at"),
            Some(&json!(1_776_158_085u64))
        );
        assert_eq!(parsed.get("prompt_remaining"), Some(&json!(9.0)));
        assert_eq!(parsed.get("prompt_used"), Some(&json!(1.0)));
        assert_eq!(parsed.get("prompt_limit"), Some(&json!(10.0)));
        assert_eq!(parsed.get("flex_remaining"), Some(&json!(2.5)));
        assert_eq!(parsed.get("flex_used"), Some(&json!(0.5)));
        assert_eq!(parsed.get("flex_limit"), Some(&json!(3.0)));
        assert_eq!(parsed.get("overage_balance"), Some(&json!(1.25)));
        assert_eq!(parsed.get("email"), Some(&json!("windsurf@example.com")));
        assert_eq!(parsed.get("quarantined"), Some(&json!(true)));
        assert_eq!(
            parsed.get("quarantine_reason"),
            Some(&json!("quota review"))
        );
        assert_eq!(parsed.get("updated_at"), Some(&json!(1_770_000_000u64)));
    }

    #[test]
    fn parses_windsurf_model_configs_response() {
        let parsed = parse_windsurf_model_configs_response(
            &json!({
                "clientModelConfigs": [
                    {
                        "modelUid": "claude-sonnet-4-5",
                        "label": "Claude Sonnet 4.5",
                        "provider": "anthropic",
                        "supportsImages": true,
                        "creditMultiplier": 2
                    },
                    {
                        "modelUid": "gpt-5-mini",
                        "label": "GPT-5 mini"
                    }
                ],
                "defaultOverrideModelConfig": {
                    "modelUid": "claude-sonnet-4-5"
                }
            }),
            1_770_000_100,
        )
        .expect("windsurf model configs should parse");

        assert_eq!(parsed.get("allowed_models_count"), Some(&json!(2u64)));
        assert_eq!(
            parsed.get("default_model_uid"),
            Some(&json!("claude-sonnet-4-5"))
        );
        assert_eq!(parsed.get("updated_at"), Some(&json!(1_770_000_100u64)));
    }

    #[test]
    fn parses_windsurf_rate_limit_response() {
        let parsed = parse_windsurf_rate_limit_response(
            &json!({
                "hasCapacity": false,
                "messagesRemaining": 0,
                "maxMessages": 25,
                "retryAfterMs": 45000
            }),
            1_770_000_200,
        )
        .expect("windsurf rate limit should parse");

        assert_eq!(parsed.get("updated_at"), Some(&json!(1_770_000_200u64)));
        assert_eq!(parsed.pointer("/rate_limit/limited"), Some(&json!(true)));
        assert_eq!(
            parsed.pointer("/rate_limit/messages_remaining"),
            Some(&json!(0.0))
        );
        assert_eq!(
            parsed.pointer("/rate_limit/retry_after_ms"),
            Some(&json!(45000u64))
        );
    }

    #[test]
    fn parses_chatgpt_web_image_quota_from_conversation_init() {
        let parsed = parse_chatgpt_web_conversation_init_response(
            &json!({
                "default_model_slug": "auto",
                "blocked_features": [],
                "limits_progress": [
                    {
                        "feature_name": "image_gen",
                        "remaining": 24,
                        "reset_after": "2026-05-07T12:32:52.826482+00:00"
                    }
                ]
            }),
            1_778_067_246,
        )
        .expect("chatgpt web quota should parse");

        assert_eq!(parsed.get("default_model_slug"), Some(&json!("auto")));
        assert_eq!(parsed.get("image_quota_remaining"), Some(&json!(24.0)));
        assert_eq!(
            parsed.get("image_quota_reset_at"),
            Some(&json!(1_778_157_172u64))
        );
    }

    #[test]
    fn parses_chatgpt_web_blocked_image_feature_as_zero_remaining() {
        let parsed = parse_chatgpt_web_conversation_init_response(
            &json!({
                "blocked_features": ["image_generation"],
                "limits_progress": []
            }),
            1_778_067_246,
        )
        .expect("blocked image feature should produce metadata");

        assert_eq!(parsed.get("image_quota_blocked"), Some(&json!(true)));
        assert_eq!(parsed.get("image_quota_remaining"), Some(&json!(0.0)));
    }
}
