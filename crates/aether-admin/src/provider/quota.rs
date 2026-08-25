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

/// Keeps the structured upstream error intact for classifiers that depend on fields such as
/// `error.code`, while retaining the execution-error fallback used by transport failures.
pub fn extract_execution_error_detail(result: &ExecutionResult) -> Option<String> {
    result
        .body
        .as_ref()
        .and_then(|body| body.json_body.as_ref())
        .and_then(|body| serde_json::to_string(body).ok())
        .filter(|value| !value.is_empty())
        .or_else(|| extract_execution_error_message(result))
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

const CODEX_QUOTA_WINDOW_SUFFIXES: &[&str] = &[
    "used_percent",
    "reset_seconds",
    "reset_after_seconds",
    "reset_at",
    "next_reset_at",
    "window_minutes",
];
const CODEX_QUOTA_RESET_DEADLINE_TOLERANCE_SECONDS: u64 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexQuotaWindowCoverage {
    Patch,
    AccountSnapshot,
    FullSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodexQuotaMergeContext<'a> {
    pub observed_at_unix_secs: u64,
    pub request_started_at_unix_ms: Option<u64>,
    pub request_order_id: Option<&'a str>,
    /// Reset generation captured before the upstream request was sent.
    /// Once a key has entered generation 1, an absent or different value is
    /// treated as a pre-reset observation for account quota and metadata.
    pub observed_reset_generation: Option<u64>,
    /// Generation owned by an explicit reset reconciliation request. Normal
    /// quota observations leave this unset, but a complete account snapshot
    /// started after the reset fence may still reconcile a delayed reset.
    pub authoritative_reset_generation: Option<u64>,
    /// Non-secret credential generation captured with the transport snapshot.
    pub observed_credential_generation: Option<&'a str>,
    /// Identifies the reset-credit fence that authorized this observation.
    /// Retained for generation-0 rolling-upgrade compatibility.
    pub account_reset_fence_id: Option<&'a str>,
    pub coverage: CodexQuotaWindowCoverage,
}

impl<'a> CodexQuotaMergeContext<'a> {
    fn request_order(self) -> Option<CodexQuotaRequestOrder<'a>> {
        codex_quota_request_order(self.request_started_at_unix_ms, self.request_order_id)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodexQuotaMergeOutcome {
    pub metadata: serde_json::Value,
    pub changed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CodexQuotaWindowFamily {
    Account,
    Spark,
}

impl CodexQuotaWindowFamily {
    fn watermark_key(self) -> &'static str {
        match self {
            Self::Account => CODEX_QUOTA_ACCOUNT_REQUEST_WATERMARK_KEY,
            Self::Spark => CODEX_QUOTA_SPARK_REQUEST_WATERMARK_KEY,
        }
    }

    fn watermark_id_key(self) -> &'static str {
        match self {
            Self::Account => CODEX_QUOTA_ACCOUNT_REQUEST_WATERMARK_ID_KEY,
            Self::Spark => CODEX_QUOTA_SPARK_REQUEST_WATERMARK_ID_KEY,
        }
    }
}

pub const CODEX_QUOTA_ACCOUNT_REQUEST_WATERMARK_KEY: &str =
    "account_quota_request_started_at_unix_ms";
pub const CODEX_QUOTA_ACCOUNT_REQUEST_WATERMARK_ID_KEY: &str = "account_quota_request_id";
pub const CODEX_QUOTA_SPARK_REQUEST_WATERMARK_KEY: &str = "spark_quota_request_started_at_unix_ms";
pub const CODEX_QUOTA_SPARK_REQUEST_WATERMARK_ID_KEY: &str = "spark_quota_request_id";
pub const CODEX_QUOTA_METADATA_REQUEST_WATERMARK_KEY: &str =
    "quota_metadata_request_started_at_unix_ms";
pub const CODEX_QUOTA_METADATA_REQUEST_WATERMARK_ID_KEY: &str = "quota_metadata_request_id";
pub const CODEX_OAUTH_STATE_REQUEST_WATERMARK_KEY: &str = "oauth_state_request_started_at_unix_ms";
pub const CODEX_OAUTH_STATE_REQUEST_WATERMARK_ID_KEY: &str = "oauth_state_request_id";
pub const CODEX_QUOTA_ACCOUNT_RESET_FENCE_UNIX_MS_KEY: &str = "account_quota_reset_fence_unix_ms";
pub const CODEX_QUOTA_ACCOUNT_RESET_FENCE_ID_KEY: &str = "account_quota_reset_fence_id";
pub const CODEX_QUOTA_ACCOUNT_RESET_PROCESSED_IDS_KEY: &str = "account_quota_reset_processed_ids";
pub const CODEX_QUOTA_ACCOUNT_RESET_PENDING_KEY: &str = "account_quota_reset_pending";
pub const CODEX_QUOTA_ACCOUNT_RESET_SEQUENCE_KEY: &str = "account_quota_reset_sequence";
pub const CODEX_QUOTA_ACCOUNT_RESET_GENERATION_KEY: &str = "account_quota_reset_generation";
pub const CODEX_QUOTA_ACCOUNT_RESET_PENDING_GENERATION_KEY: &str =
    "account_quota_reset_pending_generation";
pub const CODEX_QUOTA_ACCOUNT_RESET_RESERVATION_KEY: &str = "account_quota_reset_reservation";
pub const CODEX_QUOTA_ACCOUNT_RESET_HISTORY_KEY: &str = "account_quota_reset_history";
pub const CODEX_CREDENTIAL_GENERATION_KEY: &str = "credential_generation";

pub fn codex_quota_account_reset_generation(codex: Option<&serde_json::Value>) -> u64 {
    codex
        .and_then(serde_json::Value::as_object)
        .and_then(|codex| codex.get(CODEX_QUOTA_ACCOUNT_RESET_GENERATION_KEY))
        .and_then(coerce_json_u64)
        .unwrap_or(0)
}

pub fn codex_credential_generation(codex: Option<&serde_json::Value>) -> Option<&str> {
    codex
        .and_then(serde_json::Value::as_object)
        .and_then(|codex| codex.get(CODEX_CREDENTIAL_GENERATION_KEY))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

pub fn codex_credential_generation_matches(
    codex: Option<&serde_json::Value>,
    observed: Option<&str>,
) -> bool {
    let observed = observed.map(str::trim).filter(|value| !value.is_empty());
    codex_credential_generation(codex) == observed
}

fn codex_quota_observation_matches_reset_generation(
    object: &serde_json::Map<String, serde_json::Value>,
    context: CodexQuotaMergeContext<'_>,
) -> bool {
    let active_generation = object
        .get(CODEX_QUOTA_ACCOUNT_RESET_GENERATION_KEY)
        .and_then(coerce_json_u64)
        .unwrap_or(0);
    if active_generation == 0 {
        context.observed_reset_generation.unwrap_or(0) == 0
    } else {
        context.observed_reset_generation == Some(active_generation)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct CodexQuotaRequestOrder<'a> {
    started_at_unix_ms: u64,
    request_id: Option<&'a str>,
}

fn codex_quota_request_order<'a>(
    started_at_unix_ms: Option<u64>,
    request_id: Option<&'a str>,
) -> Option<CodexQuotaRequestOrder<'a>> {
    started_at_unix_ms.map(|started_at_unix_ms| CodexQuotaRequestOrder {
        started_at_unix_ms,
        request_id: request_id
            .map(str::trim)
            .filter(|request_id| !request_id.is_empty()),
    })
}

fn codex_quota_read_request_order<'a>(
    object: &'a serde_json::Map<String, serde_json::Value>,
    watermark_key: &str,
    watermark_id_key: &str,
) -> Option<CodexQuotaRequestOrder<'a>> {
    object
        .get(watermark_key)
        .and_then(coerce_json_u64)
        .map(|started_at_unix_ms| CodexQuotaRequestOrder {
            started_at_unix_ms,
            request_id: object
                .get(watermark_id_key)
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|request_id| !request_id.is_empty()),
        })
}

fn codex_quota_request_order_is_stale(
    incoming: Option<CodexQuotaRequestOrder<'_>>,
    current: Option<CodexQuotaRequestOrder<'_>>,
) -> bool {
    match (incoming, current) {
        (Some(incoming), Some(current)) => incoming <= current,
        (None, Some(_)) => true,
        _ => false,
    }
}

/// Returns whether an incoming Codex observation is older than or identical
/// to the stored request order. Request ids break ties within one millisecond;
/// a legacy watermark without an id sorts before one that has an id.
pub fn codex_request_order_is_stale(
    incoming_started_at_unix_ms: Option<u64>,
    incoming_request_id: Option<&str>,
    stored_started_at_unix_ms: Option<u64>,
    stored_request_id: Option<&str>,
) -> bool {
    codex_quota_request_order_is_stale(
        codex_quota_request_order(incoming_started_at_unix_ms, incoming_request_id),
        codex_quota_request_order(stored_started_at_unix_ms, stored_request_id),
    )
}

/// Compares an OAuth-state observation against every persisted Codex response
/// watermark. Quota-only responses also prove request ordering, so an older
/// runtime authentication failure cannot override them.
pub fn codex_oauth_state_request_order_is_stale(
    codex: Option<&serde_json::Map<String, serde_json::Value>>,
    incoming_started_at_unix_ms: Option<u64>,
    incoming_request_id: Option<&str>,
) -> bool {
    let stored = codex.and_then(|codex| {
        [
            (
                CODEX_OAUTH_STATE_REQUEST_WATERMARK_KEY,
                CODEX_OAUTH_STATE_REQUEST_WATERMARK_ID_KEY,
            ),
            (
                CODEX_QUOTA_METADATA_REQUEST_WATERMARK_KEY,
                CODEX_QUOTA_METADATA_REQUEST_WATERMARK_ID_KEY,
            ),
            (
                CODEX_QUOTA_ACCOUNT_REQUEST_WATERMARK_KEY,
                CODEX_QUOTA_ACCOUNT_REQUEST_WATERMARK_ID_KEY,
            ),
            (
                CODEX_QUOTA_SPARK_REQUEST_WATERMARK_KEY,
                CODEX_QUOTA_SPARK_REQUEST_WATERMARK_ID_KEY,
            ),
        ]
        .into_iter()
        .filter_map(|(watermark_key, watermark_id_key)| {
            codex_quota_read_request_order(codex, watermark_key, watermark_id_key)
        })
        .max()
    });
    codex_quota_request_order_is_stale(
        codex_quota_request_order(incoming_started_at_unix_ms, incoming_request_id),
        stored,
    )
}

/// Compares a successful OAuth observation against every persisted Codex
/// response watermark. Equality is allowed because quota persistence and the
/// success effect may independently process the same upstream response.
pub fn codex_oauth_success_request_order_is_stale(
    codex: Option<&serde_json::Map<String, serde_json::Value>>,
    incoming_started_at_unix_ms: Option<u64>,
    incoming_request_id: Option<&str>,
) -> bool {
    let stored = codex.and_then(|codex| {
        [
            (
                CODEX_OAUTH_STATE_REQUEST_WATERMARK_KEY,
                CODEX_OAUTH_STATE_REQUEST_WATERMARK_ID_KEY,
            ),
            (
                CODEX_QUOTA_METADATA_REQUEST_WATERMARK_KEY,
                CODEX_QUOTA_METADATA_REQUEST_WATERMARK_ID_KEY,
            ),
            (
                CODEX_QUOTA_ACCOUNT_REQUEST_WATERMARK_KEY,
                CODEX_QUOTA_ACCOUNT_REQUEST_WATERMARK_ID_KEY,
            ),
            (
                CODEX_QUOTA_SPARK_REQUEST_WATERMARK_KEY,
                CODEX_QUOTA_SPARK_REQUEST_WATERMARK_ID_KEY,
            ),
        ]
        .into_iter()
        .filter_map(|(watermark_key, watermark_id_key)| {
            codex_quota_read_request_order(codex, watermark_key, watermark_id_key)
        })
        .max()
    });
    match (
        codex_quota_request_order(incoming_started_at_unix_ms, incoming_request_id),
        stored,
    ) {
        (Some(incoming), Some(stored)) => incoming < stored,
        (None, Some(_)) => true,
        _ => false,
    }
}

fn codex_quota_request_order_is_newer(
    incoming: CodexQuotaRequestOrder<'_>,
    current: Option<CodexQuotaRequestOrder<'_>>,
) -> bool {
    current.is_none_or(|current| incoming > current)
}

fn codex_quota_write_request_order(
    object: &mut serde_json::Map<String, serde_json::Value>,
    watermark_key: &str,
    watermark_id_key: &str,
    order: CodexQuotaRequestOrder<'_>,
) {
    object.insert(watermark_key.to_string(), json!(order.started_at_unix_ms));
    if let Some(request_id) = order.request_id {
        object.insert(watermark_id_key.to_string(), json!(request_id));
    } else {
        object.remove(watermark_id_key);
    }
}

fn codex_quota_is_request_order_key(key: &str) -> bool {
    key == CODEX_QUOTA_METADATA_REQUEST_WATERMARK_KEY
        || key == CODEX_QUOTA_METADATA_REQUEST_WATERMARK_ID_KEY
        || [
            CodexQuotaWindowFamily::Account,
            CodexQuotaWindowFamily::Spark,
        ]
        .into_iter()
        .any(|family| key == family.watermark_key() || key == family.watermark_id_key())
}

fn codex_quota_is_reset_fence_key(key: &str) -> bool {
    matches!(
        key,
        CODEX_QUOTA_ACCOUNT_RESET_FENCE_UNIX_MS_KEY
            | CODEX_QUOTA_ACCOUNT_RESET_FENCE_ID_KEY
            | CODEX_QUOTA_ACCOUNT_RESET_PROCESSED_IDS_KEY
            | CODEX_QUOTA_ACCOUNT_RESET_PENDING_KEY
            | CODEX_QUOTA_ACCOUNT_RESET_SEQUENCE_KEY
            | CODEX_QUOTA_ACCOUNT_RESET_GENERATION_KEY
            | CODEX_QUOTA_ACCOUNT_RESET_PENDING_GENERATION_KEY
            | CODEX_QUOTA_ACCOUNT_RESET_RESERVATION_KEY
            | CODEX_QUOTA_ACCOUNT_RESET_HISTORY_KEY
            | CODEX_CREDENTIAL_GENERATION_KEY
    )
}

#[derive(Debug, Clone, Copy)]
struct CodexQuotaAccountResetFence<'a> {
    unix_ms: u64,
    id: &'a str,
    pending: bool,
}

fn codex_quota_account_reset_fence(
    object: &serde_json::Map<String, serde_json::Value>,
) -> Option<CodexQuotaAccountResetFence<'_>> {
    let unix_ms = object
        .get(CODEX_QUOTA_ACCOUNT_RESET_FENCE_UNIX_MS_KEY)
        .and_then(coerce_json_u64)
        .filter(|value| *value > 0)?;
    let id = object
        .get(CODEX_QUOTA_ACCOUNT_RESET_FENCE_ID_KEY)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let pending = object
        .get(CODEX_QUOTA_ACCOUNT_RESET_PENDING_KEY)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    Some(CodexQuotaAccountResetFence {
        unix_ms,
        id,
        pending,
    })
}

fn codex_quota_reset_fence_authorizes(
    fence: CodexQuotaAccountResetFence<'_>,
    context: CodexQuotaMergeContext<'_>,
) -> bool {
    context
        .account_reset_fence_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        == Some(fence.id)
}

fn codex_quota_request_started_after_reset_fence(
    fence: CodexQuotaAccountResetFence<'_>,
    context: CodexQuotaMergeContext<'_>,
) -> bool {
    context
        .request_started_at_unix_ms
        .is_some_and(|started_at| started_at > fence.unix_ms)
}

fn codex_quota_reset_fence_blocks(
    fence: CodexQuotaAccountResetFence<'_>,
    context: CodexQuotaMergeContext<'_>,
) -> bool {
    !codex_quota_reset_fence_authorizes(fence, context)
        && !codex_quota_request_started_after_reset_fence(fence, context)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum CodexQuotaWindowSlot {
    Primary,
    Secondary,
    SparkPrimary,
    SparkSecondary,
}

impl CodexQuotaWindowSlot {
    const ALL: [Self; 4] = [
        Self::Primary,
        Self::Secondary,
        Self::SparkPrimary,
        Self::SparkSecondary,
    ];

    fn prefix(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Secondary => "secondary",
            Self::SparkPrimary => "spark_primary",
            Self::SparkSecondary => "spark_secondary",
        }
    }

    fn family(self) -> CodexQuotaWindowFamily {
        match self {
            Self::Primary | Self::Secondary => CodexQuotaWindowFamily::Account,
            Self::SparkPrimary | Self::SparkSecondary => CodexQuotaWindowFamily::Spark,
        }
    }
}

#[derive(Debug, Clone)]
struct CodexQuotaWindowObservation {
    slot: CodexQuotaWindowSlot,
    fields: serde_json::Map<String, serde_json::Value>,
    window_minutes: Option<u64>,
    deadline: Option<u64>,
    disabled: bool,
}

impl CodexQuotaWindowObservation {
    fn active(&self) -> bool {
        !self.disabled && !self.fields.is_empty()
    }

    fn used_percent(&self) -> Option<f64> {
        self.fields
            .get("used_percent")
            .and_then(coerce_json_f64)
            .filter(|value| value.is_finite())
    }

    fn persist_deadline(&mut self) {
        if let Some(deadline) = self.deadline {
            self.fields.remove("next_reset_at");
            self.fields.insert("reset_at".to_string(), json!(deadline));
        }
    }
}

fn codex_quota_window_key(slot: CodexQuotaWindowSlot, suffix: &str) -> String {
    format!("{}_{suffix}", slot.prefix())
}

fn codex_quota_is_window_key(key: &str) -> bool {
    CodexQuotaWindowSlot::ALL.iter().any(|slot| {
        CODEX_QUOTA_WINDOW_SUFFIXES
            .iter()
            .any(|suffix| key == codex_quota_window_key(*slot, suffix))
    })
}

fn codex_quota_read_window(
    object: &serde_json::Map<String, serde_json::Value>,
    slot: CodexQuotaWindowSlot,
    observed_at_unix_secs: Option<u64>,
) -> Option<CodexQuotaWindowObservation> {
    let mut fields = serde_json::Map::new();
    for suffix in CODEX_QUOTA_WINDOW_SUFFIXES {
        let key = codex_quota_window_key(slot, suffix);
        let Some(raw) = object.get(&key) else {
            continue;
        };
        let normalized = match *suffix {
            "used_percent" => coerce_json_f64(raw)
                .filter(|value| value.is_finite())
                .map(|value| json!(value)),
            _ => coerce_json_u64(raw).map(|value| json!(value)),
        };
        if let Some(value) = normalized {
            fields.insert((*suffix).to_string(), value);
        }
    }
    if fields.is_empty() {
        return None;
    }

    let window_minutes = fields.get("window_minutes").and_then(coerce_json_u64);
    let disabled = window_minutes == Some(0);
    let explicit_deadline = fields
        .get("reset_at")
        .and_then(coerce_json_u64)
        .filter(|value| *value > 0)
        .or_else(|| {
            fields
                .get("next_reset_at")
                .and_then(coerce_json_u64)
                .filter(|value| *value > 0)
        });
    let reset_after_seconds = fields
        .get("reset_after_seconds")
        .and_then(coerce_json_u64)
        .or_else(|| fields.get("reset_seconds").and_then(coerce_json_u64));
    let deadline = explicit_deadline.or_else(|| {
        observed_at_unix_secs
            .zip(reset_after_seconds)
            .map(|(observed_at, reset_after)| observed_at.saturating_add(reset_after))
    });

    Some(CodexQuotaWindowObservation {
        slot,
        fields,
        window_minutes: window_minutes.filter(|value| *value > 0),
        deadline,
        disabled,
    })
}

fn codex_quota_read_family_windows(
    object: &serde_json::Map<String, serde_json::Value>,
    family: CodexQuotaWindowFamily,
    observed_at_unix_secs: Option<u64>,
) -> Vec<CodexQuotaWindowObservation> {
    CodexQuotaWindowSlot::ALL
        .iter()
        .copied()
        .filter(|slot| slot.family() == family)
        .filter_map(|slot| codex_quota_read_window(object, slot, observed_at_unix_secs))
        .collect()
}

pub fn codex_quota_metadata_has_account_windows(value: &serde_json::Value) -> bool {
    value.as_object().is_some_and(|object| {
        !codex_quota_read_family_windows(object, CodexQuotaWindowFamily::Account, None).is_empty()
    })
}

pub fn codex_quota_metadata_has_spark_windows(value: &serde_json::Value) -> bool {
    value.as_object().is_some_and(|object| {
        !codex_quota_read_family_windows(object, CodexQuotaWindowFamily::Spark, None).is_empty()
    })
}

fn codex_quota_same_window_identity(
    current: &CodexQuotaWindowObservation,
    incoming: &CodexQuotaWindowObservation,
) -> bool {
    match (current.window_minutes, incoming.window_minutes) {
        (Some(current), Some(incoming)) => current == incoming,
        // Old metadata did not always carry a duration. Only fall back to its
        // storage slot when at least one side has that legacy shape.
        _ => current.slot == incoming.slot,
    }
}

fn codex_quota_merge_same_window(
    current: &CodexQuotaWindowObservation,
    incoming: &CodexQuotaWindowObservation,
) -> CodexQuotaWindowObservation {
    // A deadline must be present on both sides to prove a natural generation
    // change. Legacy metadata without one stays monotonic until a later pair of
    // observations establishes the deadline.
    if current
        .deadline
        .zip(incoming.deadline)
        .is_some_and(|(current, incoming)| {
            incoming.saturating_add(CODEX_QUOTA_RESET_DEADLINE_TOLERANCE_SECONDS) < current
        })
    {
        return current.clone();
    }

    if current
        .deadline
        .zip(incoming.deadline)
        .is_some_and(|(current, incoming)| {
            incoming > current.saturating_add(CODEX_QUOTA_RESET_DEADLINE_TOLERANCE_SECONDS)
        })
    {
        let mut next = incoming.clone();
        next.persist_deadline();
        return next;
    }

    let mut merged = current.clone();
    for (suffix, value) in &incoming.fields {
        if suffix != "used_percent" {
            merged.fields.insert(suffix.clone(), value.clone());
        }
    }
    merged.window_minutes = incoming.window_minutes.or(current.window_minutes);
    // Keep the established deadline when observations differ only by normal
    // countdown/clock jitter so repeated responses cannot inch it forward.
    merged.deadline = current.deadline.or(incoming.deadline);
    merged.persist_deadline();

    let used_percent = match (current.used_percent(), incoming.used_percent()) {
        (Some(current), Some(incoming)) => Some(current.max(incoming)),
        (Some(current), None) => Some(current),
        (None, Some(incoming)) => Some(incoming),
        (None, None) => None,
    };
    if let Some(used_percent) = used_percent {
        merged
            .fields
            .insert("used_percent".to_string(), json!(used_percent));
    } else {
        merged.fields.remove("used_percent");
    }
    merged
}

fn codex_quota_merge_stale_same_window_usage(
    current: &CodexQuotaWindowObservation,
    incoming: &CodexQuotaWindowObservation,
) -> CodexQuotaWindowObservation {
    let same_generation = match (current.deadline, incoming.deadline) {
        (Some(current), Some(incoming)) => {
            current.abs_diff(incoming) <= CODEX_QUOTA_RESET_DEADLINE_TOLERANCE_SECONDS
        }
        (None, None) => true,
        _ => false,
    };
    if !same_generation {
        return current.clone();
    }

    let Some(incoming_used_percent) = incoming.used_percent() else {
        return current.clone();
    };
    if current
        .used_percent()
        .is_some_and(|current_used_percent| current_used_percent >= incoming_used_percent)
    {
        return current.clone();
    }

    let mut merged = current.clone();
    merged
        .fields
        .insert("used_percent".to_string(), json!(incoming_used_percent));
    merged
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct CodexQuotaWindowAssignmentScore {
    unmatched: usize,
    worst_deadline_rank: u8,
    deadline_rank_sum: usize,
    deadline_distance_sum: u128,
    slot_mismatches: usize,
    assignment_key: Vec<usize>,
}

fn codex_quota_window_deadline_match_score(
    current: &CodexQuotaWindowObservation,
    incoming: &CodexQuotaWindowObservation,
) -> (u8, u64) {
    match (current.deadline, incoming.deadline) {
        (Some(current), Some(incoming)) => {
            let distance = current.abs_diff(incoming);
            (
                u8::from(distance > CODEX_QUOTA_RESET_DEADLINE_TOLERANCE_SECONDS),
                distance,
            )
        }
        (None, None) => (0, 0),
        _ => (2, 0),
    }
}

fn codex_quota_window_assignment_score(
    current: &[CodexQuotaWindowObservation],
    incoming: &[CodexQuotaWindowObservation],
    assignment: &[Option<usize>],
) -> CodexQuotaWindowAssignmentScore {
    let mut unmatched = 0;
    let mut worst_deadline_rank = 0;
    let mut deadline_rank_sum = 0;
    let mut deadline_distance_sum = 0u128;
    let mut slot_mismatches = 0;

    for (incoming_index, current_index) in assignment.iter().copied().enumerate() {
        if !incoming[incoming_index].active() {
            continue;
        }
        let Some(current_index) = current_index else {
            unmatched += 1;
            continue;
        };
        let current_window = &current[current_index];
        let incoming_window = &incoming[incoming_index];
        let (deadline_rank, deadline_distance) =
            codex_quota_window_deadline_match_score(current_window, incoming_window);
        worst_deadline_rank = worst_deadline_rank.max(deadline_rank);
        deadline_rank_sum += usize::from(deadline_rank);
        deadline_distance_sum += u128::from(deadline_distance);
        slot_mismatches += usize::from(current_window.slot != incoming_window.slot);
    }

    CodexQuotaWindowAssignmentScore {
        unmatched,
        worst_deadline_rank,
        deadline_rank_sum,
        deadline_distance_sum,
        slot_mismatches,
        assignment_key: assignment
            .iter()
            .map(|index| index.unwrap_or(usize::MAX))
            .collect(),
    }
}

fn codex_quota_search_window_assignments(
    current: &[CodexQuotaWindowObservation],
    incoming: &[CodexQuotaWindowObservation],
    incoming_index: usize,
    used_current: &mut [bool],
    assignment: &mut [Option<usize>],
    best: &mut Option<(CodexQuotaWindowAssignmentScore, Vec<Option<usize>>)>,
) {
    if incoming_index == incoming.len() {
        let score = codex_quota_window_assignment_score(current, incoming, assignment);
        if best
            .as_ref()
            .is_none_or(|(best_score, _)| score < *best_score)
        {
            *best = Some((score, assignment.to_vec()));
        }
        return;
    }

    assignment[incoming_index] = None;
    codex_quota_search_window_assignments(
        current,
        incoming,
        incoming_index + 1,
        used_current,
        assignment,
        best,
    );
    if !incoming[incoming_index].active() {
        return;
    }

    for (current_index, current_window) in current.iter().enumerate() {
        if used_current[current_index]
            || !current_window.active()
            || !codex_quota_same_window_identity(current_window, &incoming[incoming_index])
        {
            continue;
        }
        used_current[current_index] = true;
        assignment[incoming_index] = Some(current_index);
        codex_quota_search_window_assignments(
            current,
            incoming,
            incoming_index + 1,
            used_current,
            assignment,
            best,
        );
        used_current[current_index] = false;
    }
    assignment[incoming_index] = None;
}

fn codex_quota_match_windows(
    current: &[CodexQuotaWindowObservation],
    incoming: &[CodexQuotaWindowObservation],
) -> Vec<Option<usize>> {
    let mut best = None;
    codex_quota_search_window_assignments(
        current,
        incoming,
        0,
        &mut vec![false; current.len()],
        &mut vec![None; incoming.len()],
        &mut best,
    );
    best.map(|(_, assignment)| assignment)
        .unwrap_or_else(|| vec![None; incoming.len()])
}

fn codex_quota_reset_observation_proves_new_baseline(
    current: &[CodexQuotaWindowObservation],
    incoming: &[CodexQuotaWindowObservation],
) -> bool {
    let current = current
        .iter()
        .filter(|window| window.active())
        .cloned()
        .collect::<Vec<_>>();
    let incoming = incoming
        .iter()
        .filter(|window| window.active())
        .cloned()
        .collect::<Vec<_>>();
    if current.is_empty() {
        return !incoming.is_empty();
    }

    let incoming_is_confirmed_zero_baseline = !incoming.is_empty()
        && incoming
            .iter()
            .all(|window| window.used_percent() == Some(0.0));
    if incoming_is_confirmed_zero_baseline
        && current
            .iter()
            .all(|window| window.used_percent() == Some(0.0))
    {
        return true;
    }

    let window_matches = codex_quota_match_windows(&current, &incoming);
    incoming
        .iter()
        .enumerate()
        .any(|(incoming_index, incoming_window)| {
            let Some(current_index) = window_matches[incoming_index] else {
                return true;
            };
            let current_window = &current[current_index];
            if current_window
                .deadline
                .zip(incoming_window.deadline)
                .is_some_and(|(current, incoming)| {
                    incoming > current.saturating_add(CODEX_QUOTA_RESET_DEADLINE_TOLERANCE_SECONDS)
                })
            {
                return true;
            }
            current_window
                .used_percent()
                .zip(incoming_window.used_percent())
                .is_some_and(|(current, incoming)| incoming < current)
        })
}

fn codex_quota_write_family_windows(
    object: &mut serde_json::Map<String, serde_json::Value>,
    family: CodexQuotaWindowFamily,
    windows: &[CodexQuotaWindowObservation],
) {
    for slot in CodexQuotaWindowSlot::ALL
        .iter()
        .copied()
        .filter(|slot| slot.family() == family)
    {
        for suffix in CODEX_QUOTA_WINDOW_SUFFIXES {
            object.remove(&codex_quota_window_key(slot, suffix));
        }
    }
    for window in windows.iter().filter(|window| window.active()) {
        for (suffix, value) in &window.fields {
            object.insert(codex_quota_window_key(window.slot, suffix), value.clone());
        }
    }
}

fn codex_quota_stabilize_legacy_deadlines(
    current: &serde_json::Map<String, serde_json::Value>,
    merged: &mut serde_json::Map<String, serde_json::Value>,
) {
    let observed_at = current.get("updated_at").and_then(coerce_json_u64);
    for family in [
        CodexQuotaWindowFamily::Account,
        CodexQuotaWindowFamily::Spark,
    ] {
        let mut windows = codex_quota_read_family_windows(current, family, observed_at);
        if windows.iter().any(|window| window.deadline.is_some()) {
            for window in &mut windows {
                window.persist_deadline();
            }
            codex_quota_write_family_windows(merged, family, &windows);
        }
    }
}

fn codex_quota_family_authoritative(
    coverage: CodexQuotaWindowCoverage,
    family: CodexQuotaWindowFamily,
) -> bool {
    matches!(coverage, CodexQuotaWindowCoverage::FullSnapshot)
        || matches!(
            (coverage, family),
            (
                CodexQuotaWindowCoverage::AccountSnapshot,
                CodexQuotaWindowFamily::Account
            )
        )
}

fn codex_quota_processes_family(
    coverage: CodexQuotaWindowCoverage,
    family: CodexQuotaWindowFamily,
    incoming: &[CodexQuotaWindowObservation],
) -> bool {
    match coverage {
        CodexQuotaWindowCoverage::Patch => !incoming.is_empty(),
        CodexQuotaWindowCoverage::AccountSnapshot => family == CodexQuotaWindowFamily::Account,
        CodexQuotaWindowCoverage::FullSnapshot => true,
    }
}

fn codex_quota_apply_family(
    current_object: &serde_json::Map<String, serde_json::Value>,
    incoming_object: &serde_json::Map<String, serde_json::Value>,
    merged: &mut serde_json::Map<String, serde_json::Value>,
    family: CodexQuotaWindowFamily,
    context: CodexQuotaMergeContext<'_>,
) {
    let active_reset_generation = current_object
        .get(CODEX_QUOTA_ACCOUNT_RESET_GENERATION_KEY)
        .and_then(coerce_json_u64)
        .unwrap_or(0);
    let generation_matches =
        codex_quota_observation_matches_reset_generation(current_object, context);
    if family == CodexQuotaWindowFamily::Account && !generation_matches {
        return;
    }
    let current_observed_at = current_object.get("updated_at").and_then(coerce_json_u64);
    let current = codex_quota_read_family_windows(current_object, family, current_observed_at);
    let mut incoming = codex_quota_read_family_windows(
        incoming_object,
        family,
        Some(context.observed_at_unix_secs),
    );
    if context.coverage == CodexQuotaWindowCoverage::Patch
        && current.iter().filter(|window| window.active()).count() > 1
    {
        // A partial header reports upstream's primary/secondary name, while
        // paid accounts may store those windows in the opposite slots. Without
        // a duration there is no stable identity, so leave both windows alone.
        incoming.retain(|window| !window.active() || window.window_minutes.is_some());
    }
    if !codex_quota_processes_family(context.coverage, family, &incoming) {
        return;
    }

    let account_reset_fence = (family == CodexQuotaWindowFamily::Account)
        .then(|| codex_quota_account_reset_fence(current_object))
        .flatten();
    if active_reset_generation == 0
        && account_reset_fence.is_some_and(|fence| codex_quota_reset_fence_blocks(fence, context))
    {
        return;
    }
    let authoritative = codex_quota_family_authoritative(context.coverage, family);
    let pending_generation_matches = active_reset_generation > 0
        && current_object
            .get(CODEX_QUOTA_ACCOUNT_RESET_PENDING_GENERATION_KEY)
            .and_then(coerce_json_u64)
            == Some(active_reset_generation);
    let generation_authorizes_reset = pending_generation_matches
        && (context.authoritative_reset_generation == Some(active_reset_generation)
            || account_reset_fence.is_some_and(|fence| {
                codex_quota_request_started_after_reset_fence(fence, context)
            }));
    let legacy_fence_authorizes_reset = active_reset_generation == 0
        && account_reset_fence.is_some_and(|fence| {
            codex_quota_reset_fence_authorizes(fence, context)
                || codex_quota_request_started_after_reset_fence(fence, context)
        });
    let reset_baseline = account_reset_fence.is_some_and(|fence| {
        fence.pending
            && authoritative
            && incoming.iter().any(CodexQuotaWindowObservation::active)
            && codex_quota_reset_observation_proves_new_baseline(&current, &incoming)
            && (generation_authorizes_reset || legacy_fence_authorizes_reset)
    });
    if account_reset_fence.is_some_and(|fence| fence.pending) && !reset_baseline {
        return;
    }
    let stored_watermark = codex_quota_read_request_order(
        current_object,
        family.watermark_key(),
        family.watermark_id_key(),
    );
    let stale_family = !reset_baseline
        && codex_quota_request_order_is_stale(context.request_order(), stored_watermark);
    let window_matches = (!reset_baseline).then(|| codex_quota_match_windows(&current, &incoming));
    let mut next = if reset_baseline || (authoritative && !stale_family) {
        Vec::new()
    } else {
        current
            .iter()
            .filter(|window| window.active())
            .cloned()
            .collect::<Vec<_>>()
    };

    for (incoming_index, incoming_window) in incoming
        .iter()
        .enumerate()
        .filter(|(_, window)| window.active())
    {
        let current_index = window_matches
            .as_ref()
            .and_then(|matches| matches[incoming_index]);
        let Some(current_index) = current_index else {
            if !stale_family {
                next.retain(|window| window.slot != incoming_window.slot);
                let mut accepted = incoming_window.clone();
                accepted.persist_deadline();
                next.push(accepted);
            }
            continue;
        };
        let mut merged_window = if stale_family {
            codex_quota_merge_stale_same_window_usage(&current[current_index], incoming_window)
        } else {
            codex_quota_merge_same_window(&current[current_index], incoming_window)
        };
        let target_slot = if authoritative && !stale_family {
            incoming_window.slot
        } else {
            current[current_index].slot
        };
        merged_window.slot = target_slot;
        next.retain(|window| window.slot != target_slot);
        next.push(merged_window);
    }

    if !authoritative && !stale_family {
        for incoming_window in incoming.iter().filter(|window| window.disabled) {
            next.retain(|window| window.slot != incoming_window.slot);
        }
    }
    next.sort_by_key(|window| window.slot);

    codex_quota_write_family_windows(merged, family, &next);
    if reset_baseline {
        merged.insert(
            CODEX_QUOTA_ACCOUNT_RESET_PENDING_KEY.to_string(),
            json!(false),
        );
    }
    if !stale_family {
        if let Some(incoming_order) = context
            .request_order()
            .filter(|incoming| codex_quota_request_order_is_newer(*incoming, stored_watermark))
        {
            codex_quota_write_request_order(
                merged,
                family.watermark_key(),
                family.watermark_id_key(),
                incoming_order,
            );
        }
    }
}

fn codex_quota_semantic_metadata(
    object: &serde_json::Map<String, serde_json::Value>,
) -> serde_json::Map<String, serde_json::Value> {
    object
        .iter()
        .filter(|(key, _)| {
            key.as_str() != "updated_at"
                && !key.ends_with("_reset_seconds")
                && !key.ends_with("_reset_after_seconds")
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// Merge a parsed Codex quota observation into the stored flat metadata.
///
/// Positive `window_minutes` values identify windows independently of the
/// primary/secondary storage slot. Within one reset deadline usage is
/// monotonic; advancing the deadline starts a new generation. Snapshot modes
/// replace the covered window families, while patch mode leaves absent windows
/// alone. A request-start/id watermark prevents a delayed request from
/// restoring a superseded window shape.
pub fn merge_codex_quota_metadata_snapshot(
    current: Option<&serde_json::Value>,
    incoming: &serde_json::Value,
    context: CodexQuotaMergeContext<'_>,
) -> Option<CodexQuotaMergeOutcome> {
    let incoming_object = incoming.as_object()?;
    let current_object = current
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default();
    if !codex_credential_generation_matches(current, context.observed_credential_generation) {
        return Some(CodexQuotaMergeOutcome {
            metadata: serde_json::Value::Object(current_object),
            changed: false,
        });
    }
    let mut merged = current_object.clone();
    codex_quota_stabilize_legacy_deadlines(&current_object, &mut merged);

    let stored_metadata_watermark = std::iter::once(codex_quota_read_request_order(
        &current_object,
        CODEX_QUOTA_METADATA_REQUEST_WATERMARK_KEY,
        CODEX_QUOTA_METADATA_REQUEST_WATERMARK_ID_KEY,
    ))
    .chain(
        [
            CodexQuotaWindowFamily::Account,
            CodexQuotaWindowFamily::Spark,
        ]
        .into_iter()
        .map(|family| {
            codex_quota_read_request_order(
                &current_object,
                family.watermark_key(),
                family.watermark_id_key(),
            )
        }),
    )
    .flatten()
    .max();
    let has_incoming_metadata = incoming_object.keys().any(|key| {
        key != "updated_at"
            && !codex_quota_is_request_order_key(key)
            && !codex_quota_is_reset_fence_key(key)
            && !codex_quota_is_window_key(key)
    });
    let stale_metadata = has_incoming_metadata
        && (codex_quota_request_order_is_stale(context.request_order(), stored_metadata_watermark)
            || !codex_quota_observation_matches_reset_generation(&current_object, context)
            || (codex_quota_account_reset_generation(Some(&serde_json::Value::Object(
                current_object.clone(),
            ))) == 0
                && codex_quota_account_reset_fence(&current_object)
                    .is_some_and(|fence| codex_quota_reset_fence_blocks(fence, context))));

    if has_incoming_metadata && !stale_metadata {
        for (key, value) in incoming_object {
            if key == "updated_at"
                || codex_quota_is_request_order_key(key)
                || codex_quota_is_reset_fence_key(key)
                || codex_quota_is_window_key(key)
            {
                continue;
            }
            merged.insert(key.clone(), value.clone());
        }
        if let Some(incoming_order) = context.request_order().filter(|incoming| {
            codex_quota_request_order_is_newer(*incoming, stored_metadata_watermark)
        }) {
            codex_quota_write_request_order(
                &mut merged,
                CODEX_QUOTA_METADATA_REQUEST_WATERMARK_KEY,
                CODEX_QUOTA_METADATA_REQUEST_WATERMARK_ID_KEY,
                incoming_order,
            );
        }
    }

    codex_quota_apply_family(
        &current_object,
        incoming_object,
        &mut merged,
        CodexQuotaWindowFamily::Account,
        context,
    );
    codex_quota_apply_family(
        &current_object,
        incoming_object,
        &mut merged,
        CodexQuotaWindowFamily::Spark,
        context,
    );

    let changed =
        codex_quota_semantic_metadata(&current_object) != codex_quota_semantic_metadata(&merged);
    if changed {
        let updated_at_unix_secs = current_object
            .get("updated_at")
            .and_then(coerce_json_u64)
            .unwrap_or_default()
            .max(context.observed_at_unix_secs);
        merged.insert("updated_at".to_string(), json!(updated_at_unix_secs));
        Some(CodexQuotaMergeOutcome {
            metadata: serde_json::Value::Object(merged),
            changed: true,
        })
    } else {
        Some(CodexQuotaMergeOutcome {
            metadata: serde_json::Value::Object(current_object),
            changed: false,
        })
    }
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

fn codex_window_is_explicitly_disabled(
    source: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    let used_percent_is_zero = source
        .get("used_percent")
        .and_then(coerce_json_f64)
        .is_some_and(|value| value == 0.0);
    let reset_after_is_zero = source
        .get("reset_after_seconds")
        .and_then(coerce_json_u64)
        .is_some_and(|value| value == 0);
    let reset_at_is_empty = source.get("reset_at").is_some_and(|value| {
        value.is_null()
            || value.as_str().is_some_and(|value| value.trim().is_empty())
            || coerce_json_u64(value).is_some_and(|value| value == 0)
    });
    let duration_is_zero = ["window_minutes", "limit_window_seconds"]
        .iter()
        .find_map(|key| source.get(*key).and_then(coerce_json_u64))
        .is_some_and(|value| value == 0);

    used_percent_is_zero && reset_after_is_zero && reset_at_is_empty && duration_is_zero
}

fn codex_write_disabled_window(
    target: &mut serde_json::Map<String, serde_json::Value>,
    target_prefix: &str,
) {
    target.insert(format!("{target_prefix}_window_minutes"), json!(0u64));
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
        if codex_window_is_explicitly_disabled(&secondary_window) {
            codex_write_disabled_window(&mut result, "secondary");
        }
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

/// Normalizes quota metadata emitted by the Codex Responses WebSocket.
///
/// The upstream normally sends a `codex.rate_limits` item inside a `chunks`
/// envelope. When the account is already exhausted it can instead send a
/// terminal `usage_limit_reached` error whose embedded `X-Codex-*` headers
/// contain the authoritative final quota snapshot.
pub fn parse_codex_websocket_rate_limits_response(
    value: &serde_json::Value,
    updated_at_unix_secs: u64,
) -> Option<serde_json::Value> {
    let mut latest = parse_codex_websocket_quota_event(value, updated_at_unix_secs);
    if let Some(body) = value.get("body") {
        if let Some(parsed) = parse_codex_websocket_quota_event(body, updated_at_unix_secs) {
            latest = Some(parsed);
        }
        for chunk in body
            .get("chunks")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(parsed) = parse_codex_websocket_quota_event(chunk, updated_at_unix_secs) {
                latest = Some(parsed);
            }
        }
    }
    for chunk in value
        .get("chunks")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(parsed) = parse_codex_websocket_quota_event(chunk, updated_at_unix_secs) {
            latest = Some(parsed);
        }
    }
    latest
}

fn parse_codex_websocket_quota_event(
    value: &serde_json::Value,
    updated_at_unix_secs: u64,
) -> Option<serde_json::Value> {
    parse_codex_websocket_rate_limits_chunk(value, updated_at_unix_secs)
        .or_else(|| parse_codex_websocket_usage_limit_error(value, updated_at_unix_secs))
}

/// Returns whether normalized Codex rate-limit metadata says that the account
/// cannot accept another request. Explicit upstream flags take precedence, and
/// the percentage fallback keeps older payloads working when those flags are
/// absent.
pub fn codex_rate_limit_metadata_exhausted(value: &serde_json::Value) -> bool {
    let allowed = value.get("allowed").and_then(coerce_json_bool);
    let limit_reached = value.get("limit_reached").and_then(coerce_json_bool);
    if allowed == Some(false) || limit_reached == Some(true) {
        return true;
    }
    if allowed == Some(true) || limit_reached == Some(false) {
        return false;
    }
    ["primary_used_percent", "secondary_used_percent"]
        .into_iter()
        .filter_map(|key| value.get(key))
        .filter_map(coerce_json_f64)
        .any(|used_percent| used_percent >= 100.0 - 1e-6)
}

fn parse_codex_websocket_rate_limits_chunk(
    value: &serde_json::Value,
    updated_at_unix_secs: u64,
) -> Option<serde_json::Value> {
    let root = value.as_object()?;
    if root.get("type").and_then(serde_json::Value::as_str) != Some("codex.rate_limits") {
        return None;
    }
    let rate_limits = root
        .get("rate_limits")
        .and_then(serde_json::Value::as_object)?;

    let mut result = serde_json::Map::new();
    let plan_type = root
        .get("plan_type")
        .or_else(|| rate_limits.get("plan_type"))
        .and_then(serde_json::Value::as_str)
        .and_then(|value| normalize_codex_plan_type(Some(value)));
    if let Some(plan_type) = plan_type {
        result.insert("plan_type".to_string(), json!(plan_type));
    }
    if let Some(allowed) = rate_limits.get("allowed").and_then(coerce_json_bool) {
        result.insert("allowed".to_string(), json!(allowed));
    }
    if let Some(limit_reached) = rate_limits.get("limit_reached").and_then(coerce_json_bool) {
        result.insert("limit_reached".to_string(), json!(limit_reached));
    }
    if let Some(primary) = rate_limits
        .get("primary")
        .and_then(serde_json::Value::as_object)
    {
        codex_write_window(&mut result, primary, "primary");
    }
    if let Some(secondary) = rate_limits
        .get("secondary")
        .and_then(serde_json::Value::as_object)
    {
        codex_write_window(&mut result, secondary, "secondary");
    }
    if result.is_empty() {
        return None;
    }
    result.insert("updated_at".to_string(), json!(updated_at_unix_secs));
    Some(serde_json::Value::Object(result))
}

fn parse_codex_websocket_usage_limit_error(
    value: &serde_json::Value,
    updated_at_unix_secs: u64,
) -> Option<serde_json::Value> {
    let root = value.as_object()?;
    if root.get("type").and_then(serde_json::Value::as_str) != Some("error") {
        return None;
    }
    let status_code = root
        .get("status_code")
        .or_else(|| root.get("status"))
        .and_then(coerce_json_u64);
    if status_code != Some(429) {
        return None;
    }
    let error = root.get("error").and_then(serde_json::Value::as_object)?;
    if error.get("type").and_then(serde_json::Value::as_str) != Some("usage_limit_reached") {
        return None;
    }

    let headers = root
        .get("headers")
        .and_then(serde_json::Value::as_object)
        .map(|headers| {
            headers
                .iter()
                .filter_map(|(name, value)| {
                    value
                        .as_str()
                        .map(|value| (name.clone(), value.to_string()))
                })
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    let mut result = parse_codex_usage_headers(&headers, updated_at_unix_secs)
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();

    if !result.contains_key("plan_type") {
        if let Some(plan_type) = error
            .get("plan_type")
            .and_then(serde_json::Value::as_str)
            .and_then(|value| normalize_codex_plan_type(Some(value)))
        {
            result.insert("plan_type".to_string(), json!(plan_type));
        }
    }
    if !result.contains_key("primary_reset_at") {
        if let Some(reset_at) = error.get("resets_at").and_then(coerce_json_u64) {
            result.insert("primary_reset_at".to_string(), json!(reset_at));
        }
    }
    if !result.contains_key("primary_reset_after_seconds") {
        if let Some(reset_after_seconds) = error.get("resets_in_seconds").and_then(coerce_json_u64)
        {
            result.insert(
                "primary_reset_after_seconds".to_string(),
                json!(reset_after_seconds),
            );
        }
    }

    // `usage_limit_reached` is a definitive, account-wide terminal signal.
    // Preserve that fact even if an intermediary strips some Codex headers.
    result.insert("allowed".to_string(), json!(false));
    result.insert("limit_reached".to_string(), json!(true));
    if result
        .get("primary_used_percent")
        .and_then(coerce_json_f64)
        .is_none()
    {
        result.insert("primary_used_percent".to_string(), json!(100.0));
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
        } else if normalized
            .get(&reset_at_key)
            .is_some_and(|value| value.is_empty())
        {
            // Preserve an explicitly empty reset-at long enough to recognize
            // the complete all-zero secondary-window disabled marker.
            object.insert("reset_at".to_string(), serde_json::Value::Null);
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
        if codex_window_is_explicitly_disabled(&secondary_window) {
            codex_write_disabled_window(&mut result, "secondary");
        }
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
    if candidate_reason.starts_with(OAUTH_EXPIRED_PREFIX) {
        let candidate_lines = candidate_reason
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>();
        let missing_refresh_failures = current
            .lines()
            .map(str::trim)
            .filter(|line| line.starts_with(OAUTH_REFRESH_FAILED_PREFIX))
            .filter(|line| !candidate_lines.contains(line))
            .collect::<Vec<_>>();
        if missing_refresh_failures.is_empty() {
            return candidate_reason.to_string();
        }
        return format!(
            "{candidate_reason}\n{}",
            missing_refresh_failures.join("\n")
        );
    }
    if candidate_reason.starts_with(OAUTH_REQUEST_FAILED_PREFIX)
        && current.lines().map(str::trim).any(|line| {
            line.starts_with(OAUTH_EXPIRED_PREFIX) || line.starts_with(OAUTH_REFRESH_FAILED_PREFIX)
        })
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
        || lowered.contains("agent runtime has been deleted")
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
        codex_build_invalid_state, codex_oauth_success_request_order_is_stale,
        codex_rate_limit_metadata_exhausted, codex_runtime_invalid_reason,
        extract_execution_error_detail, merge_codex_quota_metadata_snapshot,
        normalize_codex_reset_credit_consume_outcome, parse_antigravity_usage_response,
        parse_chatgpt_web_conversation_init_response, parse_codex_backend_me_response,
        parse_codex_usage_headers, parse_codex_websocket_rate_limits_response,
        parse_codex_wham_reset_credits_detail_response, parse_codex_wham_usage_response,
        parse_gemini_cli_retrieve_user_quota_response,
        parse_gemini_cli_v1internal_credits_response, parse_windsurf_model_configs_response,
        parse_windsurf_rate_limit_response, parse_windsurf_user_status_response,
        provider_auto_remove_quota_exhausted_keys, quota_refresh_success_invalid_state,
        should_auto_remove_structured_reason, CodexQuotaMergeContext, CodexQuotaWindowCoverage,
        OAUTH_ACCOUNT_BLOCK_PREFIX, OAUTH_EXPIRED_PREFIX, OAUTH_REFRESH_FAILED_PREFIX,
        OAUTH_REQUEST_FAILED_PREFIX,
    };
    use aether_contracts::{ExecutionResult, ResponseBody};
    use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;
    use serde_json::json;
    use std::collections::BTreeMap;

    fn merge_codex_quota(
        current: Option<&serde_json::Value>,
        incoming: &serde_json::Value,
        observed_at_unix_secs: u64,
        request_started_at_unix_ms: u64,
        coverage: CodexQuotaWindowCoverage,
    ) -> super::CodexQuotaMergeOutcome {
        merge_codex_quota_metadata_snapshot(
            current,
            incoming,
            CodexQuotaMergeContext {
                observed_at_unix_secs,
                request_started_at_unix_ms: Some(request_started_at_unix_ms),
                request_order_id: None,
                observed_reset_generation: Some(0),
                authoritative_reset_generation: None,
                observed_credential_generation: None,
                account_reset_fence_id: None,
                coverage,
            },
        )
        .expect("quota metadata should merge")
    }

    fn merge_codex_quota_ordered(
        current: Option<&serde_json::Value>,
        incoming: &serde_json::Value,
        observed_at_unix_secs: u64,
        request_started_at_unix_ms: u64,
        request_order_id: &str,
        coverage: CodexQuotaWindowCoverage,
    ) -> super::CodexQuotaMergeOutcome {
        merge_codex_quota_metadata_snapshot(
            current,
            incoming,
            CodexQuotaMergeContext {
                observed_at_unix_secs,
                request_started_at_unix_ms: Some(request_started_at_unix_ms),
                request_order_id: Some(request_order_id),
                observed_reset_generation: Some(0),
                authoritative_reset_generation: None,
                observed_credential_generation: None,
                account_reset_fence_id: None,
                coverage,
            },
        )
        .expect("quota metadata should merge")
    }

    fn merge_codex_quota_after_explicit_reset(
        current: Option<&serde_json::Value>,
        incoming: &serde_json::Value,
        observed_at_unix_secs: u64,
        request_started_at_unix_ms: u64,
    ) -> super::CodexQuotaMergeOutcome {
        merge_codex_quota_metadata_snapshot(
            current,
            incoming,
            CodexQuotaMergeContext {
                observed_at_unix_secs,
                request_started_at_unix_ms: Some(request_started_at_unix_ms),
                request_order_id: Some("reset-refresh"),
                observed_reset_generation: Some(0),
                authoritative_reset_generation: None,
                observed_credential_generation: None,
                account_reset_fence_id: Some("reset-fence"),
                coverage: CodexQuotaWindowCoverage::AccountSnapshot,
            },
        )
        .expect("reset quota metadata should merge")
    }

    #[test]
    fn codex_oauth_success_allows_equal_quota_and_oauth_watermarks() {
        let quota_watermark = json!({
            "account_quota_request_started_at_unix_ms": 1_000_u64,
            "account_quota_request_id": "01900000-0000-7000-8000-000000000010"
        });
        let oauth_watermark = json!({
            "oauth_state_request_started_at_unix_ms": 1_000_u64,
            "oauth_state_request_id": "01900000-0000-7000-8000-000000000010"
        });

        for current in [&quota_watermark, &oauth_watermark] {
            assert!(!codex_oauth_success_request_order_is_stale(
                current.as_object(),
                Some(1_000),
                Some("01900000-0000-7000-8000-000000000010"),
            ));
        }
    }

    #[test]
    fn codex_oauth_success_rejects_older_order_and_same_millisecond_lower_id() {
        let current = json!({
            "quota_metadata_request_started_at_unix_ms": 1_000_u64,
            "quota_metadata_request_id": "01900000-0000-7000-8000-000000000010"
        });

        assert!(codex_oauth_success_request_order_is_stale(
            current.as_object(),
            Some(999),
            Some("01900000-0000-7000-8000-000000000099"),
        ));
        assert!(codex_oauth_success_request_order_is_stale(
            current.as_object(),
            Some(1_000),
            Some("01900000-0000-7000-8000-000000000009"),
        ));
    }

    #[test]
    fn codex_quota_merge_keeps_usage_monotonic_within_one_generation() {
        let current = json!({
            "primary_used_percent": 60.0,
            "primary_reset_at": 2_000u64,
            "primary_window_minutes": 300u64,
            "account_quota_request_started_at_unix_ms": 90_000u64,
            "updated_at": 100u64
        });
        let incoming = json!({
            "primary_used_percent": 50.0,
            "primary_reset_at": 2_012u64,
            "primary_window_minutes": 300u64
        });

        let outcome = merge_codex_quota(
            Some(&current),
            &incoming,
            101,
            90_000,
            CodexQuotaWindowCoverage::AccountSnapshot,
        );

        assert!(!outcome.changed);
        assert_eq!(outcome.metadata, current);
    }

    #[test]
    fn codex_quota_same_value_advances_request_watermark_once() {
        let current = json!({
            "primary_used_percent": 60.0,
            "primary_reset_at": 2_000u64,
            "primary_window_minutes": 300u64,
            "account_quota_request_started_at_unix_ms": 90_000u64,
            "updated_at": 100u64
        });
        let incoming = json!({
            "primary_used_percent": 60.0,
            "primary_reset_at": 2_000u64,
            "primary_window_minutes": 300u64
        });

        let outcome = merge_codex_quota(
            Some(&current),
            &incoming,
            101,
            100_000,
            CodexQuotaWindowCoverage::AccountSnapshot,
        );

        assert!(outcome.changed);
        assert_eq!(
            outcome.metadata["account_quota_request_started_at_unix_ms"],
            json!(100_000u64)
        );
        assert_eq!(outcome.metadata["updated_at"], json!(101u64));

        let duplicate = merge_codex_quota(
            Some(&outcome.metadata),
            &incoming,
            102,
            100_000,
            CodexQuotaWindowCoverage::AccountSnapshot,
        );
        assert!(!duplicate.changed);
        assert_eq!(duplicate.metadata, outcome.metadata);
    }

    #[test]
    fn codex_quota_request_id_breaks_same_millisecond_ties() {
        let first = merge_codex_quota_ordered(
            None,
            &json!({
                "plan_type": "free",
                "primary_used_percent": 60.0,
                "primary_reset_at": 2_000u64,
                "primary_window_minutes": 300u64
            }),
            100,
            100_000,
            "request-a",
            CodexQuotaWindowCoverage::AccountSnapshot,
        );
        assert_eq!(
            first.metadata["account_quota_request_id"],
            json!("request-a")
        );
        assert_eq!(
            first.metadata["quota_metadata_request_id"],
            json!("request-a")
        );

        let newer = merge_codex_quota_ordered(
            Some(&first.metadata),
            &json!({
                "plan_type": "team",
                "primary_used_percent": 5.0,
                "primary_reset_at": 3_000_000u64,
                "primary_window_minutes": 43_800u64
            }),
            101,
            100_000,
            "request-z",
            CodexQuotaWindowCoverage::AccountSnapshot,
        );
        assert!(newer.changed);
        assert_eq!(newer.metadata["plan_type"], json!("team"));
        assert_eq!(newer.metadata["primary_window_minutes"], json!(43_800u64));
        assert_eq!(
            newer.metadata["account_quota_request_id"],
            json!("request-z")
        );
        assert_eq!(
            newer.metadata["quota_metadata_request_id"],
            json!("request-z")
        );

        let delayed = merge_codex_quota_ordered(
            Some(&newer.metadata),
            &json!({
                "plan_type": "plus",
                "primary_used_percent": 80.0,
                "primary_reset_at": 2_000u64,
                "primary_window_minutes": 300u64
            }),
            102,
            100_000,
            "request-m",
            CodexQuotaWindowCoverage::AccountSnapshot,
        );
        assert!(!delayed.changed);
        assert_eq!(delayed.metadata, newer.metadata);
    }

    #[test]
    fn codex_quota_stale_same_millisecond_request_cannot_advance_generation() {
        let newer = merge_codex_quota_ordered(
            None,
            &json!({
                "primary_used_percent": 60.0,
                "primary_reset_at": 1_001u64,
                "primary_window_minutes": 300u64
            }),
            1,
            100_000,
            "request-b",
            CodexQuotaWindowCoverage::AccountSnapshot,
        );
        assert_eq!(newer.metadata["primary_reset_at"], json!(1_001u64));
        assert_eq!(newer.metadata["primary_used_percent"], json!(60.0));

        let delayed = merge_codex_quota_ordered(
            Some(&newer.metadata),
            &json!({
                "primary_used_percent": 5.0,
                "primary_reset_after_seconds": 60u64,
                "primary_window_minutes": 300u64
            }),
            1_000,
            100_000,
            "request-a",
            CodexQuotaWindowCoverage::AccountSnapshot,
        );

        assert!(!delayed.changed);
        assert_eq!(delayed.metadata, newer.metadata);
        assert_eq!(delayed.metadata["primary_reset_at"], json!(1_001u64));
        assert_eq!(delayed.metadata["primary_used_percent"], json!(60.0));
        assert_eq!(
            delayed.metadata["account_quota_request_id"],
            json!("request-b")
        );
    }

    #[test]
    fn codex_quota_stale_request_can_raise_usage_within_same_generation() {
        let current = json!({
            "primary_used_percent": 60.0,
            "primary_reset_at": 1_001u64,
            "primary_window_minutes": 300u64,
            "account_quota_request_started_at_unix_ms": 100_000u64,
            "account_quota_request_id": "request-b",
            "updated_at": 1u64
        });
        let delayed = merge_codex_quota_ordered(
            Some(&current),
            &json!({
                "primary_used_percent": 80.0,
                "primary_reset_at": 1_010u64,
                "primary_window_minutes": 300u64
            }),
            2,
            100_000,
            "request-a",
            CodexQuotaWindowCoverage::AccountSnapshot,
        );

        assert!(delayed.changed);
        assert_eq!(delayed.metadata["primary_used_percent"], json!(80.0));
        assert_eq!(delayed.metadata["primary_reset_at"], json!(1_001u64));
        assert_eq!(
            delayed.metadata["account_quota_request_id"],
            json!("request-b")
        );
    }

    #[test]
    fn codex_quota_request_id_supersedes_legacy_same_millisecond_watermark() {
        let legacy = json!({
            "primary_used_percent": 60.0,
            "primary_reset_at": 2_000u64,
            "primary_window_minutes": 300u64,
            "account_quota_request_started_at_unix_ms": 100_000u64,
            "updated_at": 100u64
        });

        let outcome = merge_codex_quota_ordered(
            Some(&legacy),
            &json!({
                "primary_used_percent": 5.0,
                "primary_reset_at": 3_000_000u64,
                "primary_window_minutes": 43_800u64
            }),
            101,
            100_000,
            "request-a",
            CodexQuotaWindowCoverage::AccountSnapshot,
        );

        assert!(outcome.changed);
        assert_eq!(outcome.metadata["primary_window_minutes"], json!(43_800u64));
        assert_eq!(
            outcome.metadata["account_quota_request_id"],
            json!("request-a")
        );
    }

    #[test]
    fn codex_quota_merge_allows_usage_drop_after_deadline_advances() {
        let current = json!({
            "primary_used_percent": 100.0,
            "primary_reset_at": 2_000u64,
            "primary_window_minutes": 300u64,
            "updated_at": 100u64
        });
        let incoming = json!({
            "primary_used_percent": 2.0,
            "primary_reset_at": 20_000u64,
            "primary_window_minutes": 300u64
        });

        let outcome = merge_codex_quota(
            Some(&current),
            &incoming,
            101,
            100_000,
            CodexQuotaWindowCoverage::AccountSnapshot,
        );

        assert!(outcome.changed);
        assert_eq!(outcome.metadata["primary_used_percent"], json!(2.0));
        assert_eq!(outcome.metadata["primary_reset_at"], json!(20_000u64));
        assert_eq!(outcome.metadata["updated_at"], json!(101u64));
    }

    #[test]
    fn codex_quota_deadline_tolerance_has_inclusive_thirty_second_boundary() {
        let current = json!({
            "primary_used_percent": 90.0,
            "primary_reset_at": 2_000u64,
            "primary_window_minutes": 300u64,
            "updated_at": 100u64
        });
        let cases = [
            (2_030u64, 10.0, 90.0, 2_000u64),
            (2_031u64, 10.0, 10.0, 2_031u64),
            (1_970u64, 95.0, 95.0, 2_000u64),
            (1_969u64, 95.0, 90.0, 2_000u64),
        ];

        for (incoming_deadline, incoming_usage, expected_usage, expected_deadline) in cases {
            let outcome = merge_codex_quota(
                Some(&current),
                &json!({
                    "primary_used_percent": incoming_usage,
                    "primary_reset_at": incoming_deadline,
                    "primary_window_minutes": 300u64
                }),
                110,
                110_000,
                CodexQuotaWindowCoverage::AccountSnapshot,
            );

            assert_eq!(
                outcome.metadata["primary_used_percent"],
                json!(expected_usage),
                "incoming deadline {incoming_deadline}"
            );
            assert_eq!(
                outcome.metadata["primary_reset_at"],
                json!(expected_deadline),
                "incoming deadline {incoming_deadline}"
            );
        }
    }

    #[test]
    fn codex_quota_legacy_window_without_deadline_stays_conservative() {
        let current = json!({
            "primary_used_percent": 100.0,
            "primary_window_minutes": 300u64,
            "updated_at": 100u64
        });

        let outcome = merge_codex_quota(
            Some(&current),
            &json!({
                "primary_used_percent": 1.0,
                "primary_reset_at": 20_000u64,
                "primary_window_minutes": 300u64
            }),
            110,
            110_000,
            CodexQuotaWindowCoverage::AccountSnapshot,
        );

        assert_eq!(outcome.metadata["primary_used_percent"], json!(100.0));
        assert_eq!(outcome.metadata["primary_reset_at"], json!(20_000u64));
    }

    #[test]
    fn codex_quota_explicit_reset_allows_usage_drop_with_same_deadline() {
        let current = json!({
            "primary_used_percent": 100.0,
            "primary_reset_at": 20_000u64,
            "primary_window_minutes": 300u64,
            "account_quota_request_started_at_unix_ms": 100_000u64,
            "account_quota_request_id": "before-reset",
                "account_quota_reset_fence_unix_ms": 105_000u64,
                "account_quota_reset_fence_id": "reset-fence",
                "account_quota_reset_processed_ids": ["redeem-once"],
            "account_quota_reset_pending": true,
            "updated_at": 100u64
        });
        let incoming = json!({
            "primary_used_percent": 0.0,
            "primary_reset_at": 20_000u64,
            "primary_window_minutes": 300u64
        });

        let outcome =
            merge_codex_quota_after_explicit_reset(Some(&current), &incoming, 110, 110_000);

        assert!(outcome.changed);
        assert_eq!(outcome.metadata["primary_used_percent"], json!(0.0));
        assert_eq!(outcome.metadata["primary_reset_at"], json!(20_000u64));
        assert_eq!(
            outcome.metadata["account_quota_reset_pending"],
            json!(false)
        );
    }

    #[test]
    fn codex_quota_reset_fence_rejects_pre_reset_response_after_baseline() {
        let baseline = merge_codex_quota_after_explicit_reset(
            Some(&json!({
                "primary_used_percent": 100.0,
                "primary_reset_at": 20_000u64,
                "primary_window_minutes": 300u64,
                "account_quota_request_started_at_unix_ms": 100_000u64,
                "account_quota_request_id": "before-reset",
                "account_quota_reset_fence_unix_ms": 105_000u64,
                "account_quota_reset_fence_id": "reset-fence",
                "account_quota_reset_pending": true,
                "updated_at": 100u64
            })),
            &json!({
                "primary_used_percent": 0.0,
                "primary_reset_at": 20_000u64,
                "primary_window_minutes": 300u64
            }),
            110,
            110_000,
        );

        let delayed = merge_codex_quota_ordered(
            Some(&baseline.metadata),
            &json!({
                "primary_used_percent": 100.0,
                "primary_reset_at": 20_000u64,
                "primary_window_minutes": 300u64
            }),
            120,
            100_000,
            "old-in-flight",
            CodexQuotaWindowCoverage::AccountSnapshot,
        );

        assert!(!delayed.changed);
        assert_eq!(delayed.metadata, baseline.metadata);
        assert_eq!(delayed.metadata["primary_used_percent"], json!(0.0));
    }

    #[test]
    fn codex_quota_reset_fence_rejects_pre_reset_account_metadata() {
        let current = json!({
            "plan_type": "plus",
            "reset_credits": {
                "available_count": 0,
                "updated_at": 110,
            },
            "primary_used_percent": 0.0,
            "primary_reset_at": 20_000u64,
            "primary_window_minutes": 300u64,
            "account_quota_request_started_at_unix_ms": 110_000u64,
            "account_quota_request_id": "after-reset",
            "quota_metadata_request_started_at_unix_ms": 110_000u64,
            "quota_metadata_request_id": "after-reset",
            "account_quota_reset_fence_unix_ms": 105_000u64,
            "account_quota_reset_fence_id": "reset-fence",
            "account_quota_reset_pending": false,
            "updated_at": 110u64
        });
        let delayed = merge_codex_quota_ordered(
            Some(&current),
            &json!({
                "plan_type": "team",
                "reset_credits": {
                    "available_count": 1,
                    "updated_at": 100,
                },
                "primary_used_percent": 100.0,
                "primary_reset_at": 20_000u64,
                "primary_window_minutes": 300u64
            }),
            120,
            100_000,
            "before-reset",
            CodexQuotaWindowCoverage::AccountSnapshot,
        );

        assert!(!delayed.changed);
        assert_eq!(delayed.metadata, current);
        assert_eq!(delayed.metadata["plan_type"], json!("plus"));
        assert_eq!(
            delayed.metadata["reset_credits"]["available_count"],
            json!(0)
        );
    }

    #[test]
    fn codex_quota_pending_reset_ignores_runtime_patch_but_not_spark() {
        let current = json!({
            "primary_used_percent": 100.0,
            "primary_reset_at": 20_000u64,
            "primary_window_minutes": 300u64,
            "spark_primary_used_percent": 20.0,
            "spark_primary_reset_at": 30_000u64,
            "spark_primary_window_minutes": 300u64,
            "account_quota_reset_fence_unix_ms": 105_000u64,
            "account_quota_reset_fence_id": "reset-fence",
            "account_quota_reset_pending": true,
            "updated_at": 100u64
        });
        let incoming = json!({
            "primary_used_percent": 0.0,
            "primary_reset_at": 20_000u64,
            "primary_window_minutes": 300u64,
            "spark_primary_used_percent": 30.0,
            "spark_primary_reset_at": 30_000u64,
            "spark_primary_window_minutes": 300u64
        });

        let outcome = merge_codex_quota_ordered(
            Some(&current),
            &incoming,
            110,
            110_000,
            "after-reset-runtime",
            CodexQuotaWindowCoverage::Patch,
        );

        assert_eq!(outcome.metadata["primary_used_percent"], json!(100.0));
        assert_eq!(outcome.metadata["spark_primary_used_percent"], json!(30.0));
        assert_eq!(outcome.metadata["account_quota_reset_pending"], json!(true));
    }

    #[test]
    fn codex_quota_reset_fence_treats_same_millisecond_request_as_pre_reset() {
        let current = json!({
            "primary_used_percent": 100.0,
            "primary_reset_at": 20_000u64,
            "primary_window_minutes": 300u64,
            "account_quota_reset_fence_unix_ms": 105_000u64,
            "account_quota_reset_fence_id": "reset-fence",
            "account_quota_reset_pending": true,
            "updated_at": 100u64
        });

        let outcome = merge_codex_quota_ordered(
            Some(&current),
            &json!({
                "primary_used_percent": 0.0,
                "primary_reset_at": 20_000u64,
                "primary_window_minutes": 300u64
            }),
            110,
            105_000,
            "uuid-that-sorts-after-fence",
            CodexQuotaWindowCoverage::AccountSnapshot,
        );

        assert!(!outcome.changed);
        assert_eq!(outcome.metadata, current);
    }

    #[test]
    fn codex_quota_reset_pending_waits_for_usage_to_drop() {
        let current = json!({
            "primary_used_percent": 100.0,
            "primary_reset_at": 20_000u64,
            "primary_window_minutes": 300u64,
            "account_quota_reset_fence_unix_ms": 105_000u64,
            "account_quota_reset_fence_id": "reset-fence",
            "account_quota_reset_pending": true,
            "updated_at": 100u64
        });
        let unchanged = merge_codex_quota_after_explicit_reset(
            Some(&current),
            &json!({
                "primary_used_percent": 100.0,
                "primary_reset_at": 20_000u64,
                "primary_window_minutes": 300u64
            }),
            110,
            110_000,
        );
        assert!(!unchanged.changed);
        assert_eq!(
            unchanged.metadata["account_quota_reset_pending"],
            json!(true)
        );

        let reset = merge_codex_quota_after_explicit_reset(
            Some(&unchanged.metadata),
            &json!({
                "primary_used_percent": 0.0,
                "primary_reset_at": 20_000u64,
                "primary_window_minutes": 300u64
            }),
            111,
            111_000,
        );
        assert!(reset.changed);
        assert_eq!(reset.metadata["primary_used_percent"], json!(0.0));
        assert_eq!(reset.metadata["account_quota_reset_pending"], json!(false));
    }

    #[test]
    fn codex_quota_reset_pending_disambiguates_equal_duration_slot_swap() {
        let current = json!({
            "primary_used_percent": 90.0,
            "primary_reset_at": 1_000u64,
            "primary_window_minutes": 300u64,
            "secondary_used_percent": 10.0,
            "secondary_reset_at": 2_000u64,
            "secondary_window_minutes": 300u64,
            "account_quota_reset_fence_unix_ms": 105_000u64,
            "account_quota_reset_fence_id": "reset-fence",
            "account_quota_reset_pending": true,
            "updated_at": 100u64
        });
        let swapped_but_increased = json!({
            "primary_used_percent": 20.0,
            "primary_reset_at": 2_005u64,
            "primary_window_minutes": 300u64,
            "secondary_used_percent": 95.0,
            "secondary_reset_at": 1_005u64,
            "secondary_window_minutes": 300u64
        });

        let outcome = merge_codex_quota_after_explicit_reset(
            Some(&current),
            &swapped_but_increased,
            110,
            110_000,
        );

        assert!(!outcome.changed);
        assert_eq!(outcome.metadata, current);
        assert_eq!(outcome.metadata["account_quota_reset_pending"], json!(true));
    }

    #[test]
    fn codex_quota_reset_pending_accepts_authoritative_zero_after_zero_baseline() {
        let current = json!({
            "primary_used_percent": 0.0,
            "primary_reset_at": 20_000u64,
            "primary_window_minutes": 300u64,
            "account_quota_reset_fence_unix_ms": 105_000u64,
            "account_quota_reset_fence_id": "reset-fence",
            "account_quota_reset_pending": true,
            "updated_at": 100u64
        });
        let confirmed = merge_codex_quota_after_explicit_reset(
            Some(&current),
            &json!({
                "primary_used_percent": 0.0,
                "primary_reset_at": 20_000u64,
                "primary_window_minutes": 300u64
            }),
            110,
            110_000,
        );

        assert!(confirmed.changed);
        assert_eq!(confirmed.metadata["primary_used_percent"], json!(0.0));
        assert_eq!(
            confirmed.metadata["account_quota_reset_pending"],
            json!(false)
        );
    }

    #[test]
    fn codex_quota_pre_reset_generation_cannot_touch_account_but_spark_still_merges() {
        let current = json!({
            "plan_type": "plus",
            "primary_used_percent": 20.0,
            "primary_reset_at": 2_000u64,
            "primary_window_minutes": 300u64,
            "spark_primary_used_percent": 10.0,
            "spark_primary_reset_at": 3_000u64,
            "spark_primary_window_minutes": 300u64,
            "account_quota_reset_generation": 2u64,
            "updated_at": 100u64
        });
        let incoming = json!({
            "plan_type": "team",
            "primary_used_percent": 90.0,
            "primary_reset_at": 2_000u64,
            "primary_window_minutes": 300u64,
            "spark_primary_used_percent": 30.0,
            "spark_primary_reset_at": 3_000u64,
            "spark_primary_window_minutes": 300u64
        });

        let outcome = merge_codex_quota_metadata_snapshot(
            Some(&current),
            &incoming,
            CodexQuotaMergeContext {
                observed_at_unix_secs: 110,
                request_started_at_unix_ms: Some(110_000),
                request_order_id: Some("pre-reset"),
                observed_reset_generation: Some(1),
                authoritative_reset_generation: None,
                observed_credential_generation: None,
                account_reset_fence_id: None,
                coverage: CodexQuotaWindowCoverage::FullSnapshot,
            },
        )
        .expect("quota metadata should merge");

        assert!(outcome.changed);
        assert_eq!(outcome.metadata["plan_type"], json!("plus"));
        assert_eq!(outcome.metadata["primary_used_percent"], json!(20.0));
        assert_eq!(outcome.metadata["spark_primary_used_percent"], json!(30.0));
    }

    #[test]
    fn codex_quota_credential_generation_mismatch_rejects_every_field() {
        let current = json!({
            "credential_generation": "credential-new",
            "plan_type": "plus",
            "primary_used_percent": 20.0,
            "primary_reset_at": 2_000u64,
            "primary_window_minutes": 300u64,
            "spark_primary_used_percent": 10.0,
            "spark_primary_reset_at": 3_000u64,
            "spark_primary_window_minutes": 300u64,
            "updated_at": 100u64
        });
        let incoming = json!({
            "plan_type": "team",
            "primary_used_percent": 90.0,
            "spark_primary_used_percent": 30.0
        });

        let outcome = merge_codex_quota_metadata_snapshot(
            Some(&current),
            &incoming,
            CodexQuotaMergeContext {
                observed_at_unix_secs: 110,
                request_started_at_unix_ms: Some(110_000),
                request_order_id: Some("old-credential"),
                observed_reset_generation: Some(0),
                authoritative_reset_generation: None,
                observed_credential_generation: Some("credential-old"),
                account_reset_fence_id: None,
                coverage: CodexQuotaWindowCoverage::FullSnapshot,
            },
        )
        .expect("quota metadata should be acknowledged");

        assert!(!outcome.changed);
        assert_eq!(outcome.metadata, current);
    }

    #[test]
    fn codex_quota_only_latest_reset_generation_can_close_pending_zero_baseline() {
        let current = json!({
            "primary_used_percent": 0.0,
            "primary_reset_at": 2_000u64,
            "primary_window_minutes": 300u64,
            "account_quota_reset_generation": 2u64,
            "account_quota_reset_pending_generation": 2u64,
            "account_quota_reset_pending": true,
            "account_quota_reset_fence_unix_ms": 105_000u64,
            "account_quota_reset_fence_id": "reset:b",
            "updated_at": 100u64
        });
        let incoming = json!({
            "primary_used_percent": 0.0,
            "primary_reset_at": 2_000u64,
            "primary_window_minutes": 300u64
        });
        let merge = |generation| {
            merge_codex_quota_metadata_snapshot(
                Some(&current),
                &incoming,
                CodexQuotaMergeContext {
                    observed_at_unix_secs: 110,
                    request_started_at_unix_ms: Some(110_000),
                    request_order_id: Some("reset-refresh"),
                    observed_reset_generation: Some(generation),
                    authoritative_reset_generation: Some(generation),
                    observed_credential_generation: None,
                    account_reset_fence_id: Some("reset:b"),
                    coverage: CodexQuotaWindowCoverage::AccountSnapshot,
                },
            )
            .expect("quota metadata should merge")
        };

        let stale = merge(1);
        assert!(!stale.changed);
        assert_eq!(stale.metadata["account_quota_reset_pending"], json!(true));

        let current_generation = merge(2);
        assert!(current_generation.changed);
        assert_eq!(
            current_generation.metadata["account_quota_reset_pending"],
            json!(false)
        );
    }

    #[test]
    fn codex_quota_pending_reset_recovers_from_later_current_generation_snapshot() {
        let current = json!({
            "primary_used_percent": 100.0,
            "primary_reset_at": 2_000u64,
            "primary_window_minutes": 300u64,
            "account_quota_reset_generation": 2u64,
            "account_quota_reset_pending_generation": 2u64,
            "account_quota_reset_pending": true,
            "account_quota_reset_fence_unix_ms": 105_000u64,
            "account_quota_reset_fence_id": "reset:b",
            "updated_at": 100u64
        });
        let incoming = json!({
            "primary_used_percent": 20.0,
            "primary_reset_at": 2_000u64,
            "primary_window_minutes": 300u64
        });
        let merge = |request_started_at_unix_ms, coverage| {
            merge_codex_quota_metadata_snapshot(
                Some(&current),
                &incoming,
                CodexQuotaMergeContext {
                    observed_at_unix_secs: 120,
                    request_started_at_unix_ms: Some(request_started_at_unix_ms),
                    request_order_id: Some("later-refresh"),
                    observed_reset_generation: Some(2),
                    authoritative_reset_generation: None,
                    observed_credential_generation: None,
                    account_reset_fence_id: None,
                    coverage,
                },
            )
            .expect("quota metadata should merge")
        };

        let pre_reset_request = merge(100_000, CodexQuotaWindowCoverage::AccountSnapshot);
        assert!(!pre_reset_request.changed);
        assert_eq!(
            pre_reset_request.metadata["account_quota_reset_pending"],
            json!(true)
        );

        let partial_headers = merge(120_000, CodexQuotaWindowCoverage::Patch);
        assert!(!partial_headers.changed);
        assert_eq!(
            partial_headers.metadata["account_quota_reset_pending"],
            json!(true)
        );

        let settled = merge(120_000, CodexQuotaWindowCoverage::AccountSnapshot);
        assert!(settled.changed);
        assert_eq!(settled.metadata["primary_used_percent"], json!(20.0));
        assert_eq!(
            settled.metadata["account_quota_reset_pending"],
            json!(false)
        );
    }

    #[test]
    fn codex_quota_merge_derives_stable_deadline_from_observation_time() {
        let first = merge_codex_quota(
            None,
            &json!({
                "primary_used_percent": 60.0,
                "primary_reset_after_seconds": 900u64,
                "primary_window_minutes": 300u64
            }),
            1_000,
            900_000,
            CodexQuotaWindowCoverage::AccountSnapshot,
        );
        assert_eq!(first.metadata["primary_reset_at"], json!(1_900u64));

        let delayed = merge_codex_quota(
            Some(&first.metadata),
            &json!({
                "primary_used_percent": 50.0,
                "primary_reset_after_seconds": 890u64,
                "primary_window_minutes": 300u64
            }),
            1_010,
            800_000,
            CodexQuotaWindowCoverage::AccountSnapshot,
        );

        assert!(!delayed.changed);
        assert_eq!(delayed.metadata["primary_used_percent"], json!(60.0));
        assert_eq!(delayed.metadata["primary_reset_at"], json!(1_900u64));
    }

    #[test]
    fn codex_quota_merge_replaces_paid_windows_with_monthly_shape() {
        let current = json!({
            "primary_used_percent": 70.0,
            "primary_reset_at": 10_000u64,
            "primary_window_minutes": 10_080u64,
            "secondary_used_percent": 20.0,
            "secondary_reset_at": 2_000u64,
            "secondary_window_minutes": 300u64,
            "account_quota_request_started_at_unix_ms": 100_000u64,
            "updated_at": 100u64
        });
        let monthly = json!({
            "primary_used_percent": 5.0,
            "primary_reset_at": 3_000_000u64,
            "primary_window_minutes": 43_800u64
        });

        let outcome = merge_codex_quota(
            Some(&current),
            &monthly,
            200,
            150_000,
            CodexQuotaWindowCoverage::AccountSnapshot,
        );

        assert!(outcome.changed);
        assert_eq!(outcome.metadata["primary_window_minutes"], json!(43_800u64));
        assert_eq!(outcome.metadata["primary_used_percent"], json!(5.0));
        assert!(outcome.metadata.get("secondary_used_percent").is_none());
        assert!(outcome.metadata.get("secondary_window_minutes").is_none());
    }

    #[test]
    fn codex_quota_merge_stale_request_cannot_restore_old_window_shape() {
        let monthly = json!({
            "primary_used_percent": 5.0,
            "primary_reset_at": 3_000_000u64,
            "primary_window_minutes": 43_800u64,
            "account_quota_request_started_at_unix_ms": 150_000u64,
            "updated_at": 200u64
        });
        let delayed_paid = json!({
            "primary_used_percent": 70.0,
            "primary_reset_at": 10_000u64,
            "primary_window_minutes": 10_080u64,
            "secondary_used_percent": 20.0,
            "secondary_reset_at": 2_000u64,
            "secondary_window_minutes": 300u64
        });

        let outcome = merge_codex_quota(
            Some(&monthly),
            &delayed_paid,
            210,
            100_000,
            CodexQuotaWindowCoverage::AccountSnapshot,
        );

        assert!(!outcome.changed);
        assert_eq!(outcome.metadata, monthly);
    }

    #[test]
    fn codex_quota_merge_matches_duration_when_windows_move_slots() {
        let free = json!({
            "primary_used_percent": 40.0,
            "primary_reset_at": 2_000u64,
            "primary_window_minutes": 300u64,
            "account_quota_request_started_at_unix_ms": 100_000u64,
            "updated_at": 100u64
        });
        let paid = json!({
            "primary_used_percent": 10.0,
            "primary_reset_at": 10_000u64,
            "primary_window_minutes": 10_080u64,
            "secondary_used_percent": 30.0,
            "secondary_reset_at": 2_005u64,
            "secondary_window_minutes": 300u64
        });

        let outcome = merge_codex_quota(
            Some(&free),
            &paid,
            110,
            110_000,
            CodexQuotaWindowCoverage::AccountSnapshot,
        );

        assert!(outcome.changed);
        assert_eq!(outcome.metadata["primary_window_minutes"], json!(10_080u64));
        assert_eq!(outcome.metadata["secondary_window_minutes"], json!(300u64));
        assert_eq!(outcome.metadata["secondary_used_percent"], json!(40.0));
    }

    #[test]
    fn codex_quota_merge_uses_deadline_to_disambiguate_equal_duration_slot_swap() {
        let current = json!({
            "primary_used_percent": 90.0,
            "primary_reset_at": 1_000u64,
            "primary_window_minutes": 300u64,
            "secondary_used_percent": 10.0,
            "secondary_reset_at": 2_000u64,
            "secondary_window_minutes": 300u64,
            "account_quota_request_started_at_unix_ms": 100_000u64,
            "updated_at": 100u64
        });
        let swapped = json!({
            "primary_used_percent": 20.0,
            "primary_reset_at": 2_005u64,
            "primary_window_minutes": 300u64,
            "secondary_used_percent": 95.0,
            "secondary_reset_at": 1_005u64,
            "secondary_window_minutes": 300u64
        });

        let outcome = merge_codex_quota(
            Some(&current),
            &swapped,
            110,
            110_000,
            CodexQuotaWindowCoverage::AccountSnapshot,
        );

        assert!(outcome.changed);
        assert_eq!(outcome.metadata["primary_used_percent"], json!(20.0));
        assert_eq!(outcome.metadata["primary_reset_at"], json!(2_000u64));
        assert_eq!(outcome.metadata["secondary_used_percent"], json!(95.0));
        assert_eq!(outcome.metadata["secondary_reset_at"], json!(1_000u64));
    }

    #[test]
    fn codex_quota_patch_ignores_primary_only_header_for_paid_windows() {
        let current = json!({
            "primary_used_percent": 20.0,
            "primary_reset_at": 10_000u64,
            "primary_window_minutes": 10_080u64,
            "secondary_used_percent": 40.0,
            "secondary_reset_at": 2_000u64,
            "secondary_window_minutes": 300u64,
            "account_quota_request_started_at_unix_ms": 100_000u64,
            "updated_at": 100u64
        });
        let headers =
            BTreeMap::from([("x-codex-primary-used-percent".to_string(), "80".to_string())]);
        let partial = parse_codex_usage_headers(&headers, 110)
            .expect("partial Codex usage headers should parse");

        let outcome = merge_codex_quota(
            Some(&current),
            &partial,
            110,
            110_000,
            CodexQuotaWindowCoverage::Patch,
        );

        assert!(!outcome.changed);
        assert_eq!(outcome.metadata, current);
    }

    #[test]
    fn codex_quota_patch_without_duration_uses_slot_for_single_window() {
        let current = json!({
            "primary_used_percent": 60.0,
            "primary_reset_at": 2_000u64,
            "primary_window_minutes": 300u64,
            "account_quota_request_started_at_unix_ms": 100_000u64,
            "updated_at": 100u64
        });

        let outcome = merge_codex_quota(
            Some(&current),
            &json!({ "primary_used_percent": 70.0 }),
            110,
            110_000,
            CodexQuotaWindowCoverage::Patch,
        );

        assert!(outcome.changed);
        assert_eq!(outcome.metadata["primary_used_percent"], json!(70.0));
        assert_eq!(outcome.metadata["primary_window_minutes"], json!(300u64));
    }

    #[test]
    fn codex_quota_patch_with_duration_matches_across_paid_slots() {
        let current = json!({
            "primary_used_percent": 20.0,
            "primary_reset_at": 10_000u64,
            "primary_window_minutes": 10_080u64,
            "secondary_used_percent": 40.0,
            "secondary_reset_at": 2_000u64,
            "secondary_window_minutes": 300u64,
            "account_quota_request_started_at_unix_ms": 100_000u64,
            "updated_at": 100u64
        });

        let outcome = merge_codex_quota(
            Some(&current),
            &json!({
                "primary_used_percent": 70.0,
                "primary_reset_at": 2_005u64,
                "primary_window_minutes": 300u64
            }),
            110,
            110_000,
            CodexQuotaWindowCoverage::Patch,
        );

        assert!(outcome.changed);
        assert_eq!(outcome.metadata["primary_used_percent"], json!(20.0));
        assert_eq!(outcome.metadata["secondary_used_percent"], json!(70.0));
        assert_eq!(outcome.metadata["secondary_window_minutes"], json!(300u64));
    }

    #[test]
    fn codex_quota_account_snapshot_preserves_spark_windows() {
        let current = json!({
            "primary_used_percent": 50.0,
            "primary_reset_at": 2_000u64,
            "primary_window_minutes": 300u64,
            "spark_primary_used_percent": 25.0,
            "spark_primary_reset_at": 3_000u64,
            "spark_primary_window_minutes": 300u64,
            "updated_at": 100u64
        });
        let outcome = merge_codex_quota(
            Some(&current),
            &json!({
                "primary_used_percent": 60.0,
                "primary_reset_at": 2_000u64,
                "primary_window_minutes": 300u64
            }),
            110,
            110_000,
            CodexQuotaWindowCoverage::AccountSnapshot,
        );

        assert!(outcome.changed);
        assert_eq!(outcome.metadata["spark_primary_used_percent"], json!(25.0));
        assert_eq!(outcome.metadata["spark_primary_reset_at"], json!(3_000u64));
    }

    #[test]
    fn codex_quota_full_snapshot_removes_absent_spark_and_zero_windows() {
        let current = json!({
            "primary_used_percent": 50.0,
            "primary_reset_at": 2_000u64,
            "primary_window_minutes": 300u64,
            "secondary_used_percent": 20.0,
            "secondary_window_minutes": 10_080u64,
            "spark_primary_used_percent": 25.0,
            "spark_primary_window_minutes": 300u64,
            "updated_at": 100u64
        });
        let outcome = merge_codex_quota(
            Some(&current),
            &json!({
                "primary_used_percent": 55.0,
                "primary_reset_at": 2_000u64,
                "primary_window_minutes": 300u64,
                "secondary_window_minutes": 0u64
            }),
            110,
            110_000,
            CodexQuotaWindowCoverage::FullSnapshot,
        );

        assert!(outcome.changed);
        assert!(outcome.metadata.get("secondary_window_minutes").is_none());
        assert!(outcome.metadata.get("spark_primary_used_percent").is_none());
        assert!(outcome
            .metadata
            .get("spark_primary_window_minutes")
            .is_none());
    }

    #[test]
    fn codex_quota_merge_legacy_slot_without_identity_is_monotonic() {
        let current = json!({
            "primary_used_percent": 60.0,
            "account_quota_request_started_at_unix_ms": 110_000u64,
            "updated_at": 100u64
        });
        let outcome = merge_codex_quota(
            Some(&current),
            &json!({ "primary_used_percent": 50.0 }),
            110,
            110_000,
            CodexQuotaWindowCoverage::Patch,
        );

        assert!(!outcome.changed);
        assert_eq!(outcome.metadata, current);
    }

    #[test]
    fn codex_quota_stale_request_cannot_overwrite_account_metadata() {
        let current = json!({
            "plan_type": "free",
            "credits_balance": 20.0,
            "primary_used_percent": 10.0,
            "primary_reset_at": 20_000u64,
            "primary_window_minutes": 43_800u64,
            "quota_metadata_request_started_at_unix_ms": 200_000u64,
            "account_quota_request_started_at_unix_ms": 200_000u64,
            "updated_at": 200u64
        });
        let stale = json!({
            "plan_type": "plus",
            "credits_balance": 2.0,
            "primary_used_percent": 80.0,
            "primary_reset_at": 2_000u64,
            "primary_window_minutes": 300u64
        });

        let outcome = merge_codex_quota(
            Some(&current),
            &stale,
            210,
            100_000,
            CodexQuotaWindowCoverage::AccountSnapshot,
        );

        assert!(!outcome.changed);
        assert_eq!(outcome.metadata, current);
    }

    #[test]
    fn codex_quota_metadata_uses_the_newest_family_watermark() {
        let current = json!({
            "plan_type": "free",
            "credits_balance": 20.0,
            "primary_used_percent": 40.0,
            "primary_reset_at": 20_000u64,
            "primary_window_minutes": 300u64,
            "quota_metadata_request_started_at_unix_ms": 100_000u64,
            "account_quota_request_started_at_unix_ms": 200_000u64,
            "updated_at": 200u64
        });
        let delayed = json!({
            "plan_type": "plus",
            "credits_balance": 2.0,
            "primary_used_percent": 50.0,
            "primary_reset_at": 20_000u64,
            "primary_window_minutes": 300u64
        });

        let outcome = merge_codex_quota(
            Some(&current),
            &delayed,
            210,
            150_000,
            CodexQuotaWindowCoverage::AccountSnapshot,
        );

        assert!(outcome.changed);
        assert_eq!(outcome.metadata["plan_type"], json!("free"));
        assert_eq!(outcome.metadata["credits_balance"], json!(20.0));
        assert_eq!(outcome.metadata["primary_used_percent"], json!(50.0));
        assert_eq!(
            outcome.metadata["quota_metadata_request_started_at_unix_ms"],
            json!(100_000u64)
        );
        assert_eq!(
            outcome.metadata["account_quota_request_started_at_unix_ms"],
            json!(200_000u64)
        );
    }

    #[test]
    fn execution_error_detail_preserves_structured_code_and_message() {
        let result = ExecutionResult {
            request_id: "quota-agent-identity".to_string(),
            candidate_id: None,
            status_code: 401,
            headers: BTreeMap::new(),
            response_observation: None,
            body: Some(ResponseBody {
                json_body: Some(json!({
                    "error": {
                        "code": "invalid_task_id",
                        "message": "registered task is no longer valid"
                    }
                })),
                body_bytes_b64: None,
            }),
            telemetry: None,
            error: None,
        };

        let detail = extract_execution_error_detail(&result)
            .expect("structured execution error should be retained");
        assert!(detail.contains(r#""code":"invalid_task_id""#));
        assert!(detail.contains(r#""message":"registered task is no longer valid""#));
        assert!(
            aether_provider_transport::is_codex_agent_identity_invalid_task_response(
                result.status_code,
                Some(&detail),
            )
        );
    }

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
    fn codex_runtime_invalid_reason_marks_deleted_agent_runtime_as_invalid() {
        assert_eq!(
            codex_runtime_invalid_reason(403, Some("Agent runtime has been deleted.")),
            Some(format!(
                "{OAUTH_EXPIRED_PREFIX}Agent runtime has been deleted."
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
    fn codex_invalid_state_preserves_refresh_failure_when_oauth_expired_arrives_later() {
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
        key.oauth_invalid_reason = Some(format!(
            "{OAUTH_REFRESH_FAILED_PREFIX}Token 续期失败 (401): refresh_token 无效"
        ));
        let expected = format!(
            "{OAUTH_EXPIRED_PREFIX}session expired\n{OAUTH_REFRESH_FAILED_PREFIX}Token 续期失败 (401): refresh_token 无效"
        );

        assert_eq!(
            codex_build_invalid_state(&key, format!("{OAUTH_EXPIRED_PREFIX}session expired"), 200,),
            (Some(200), Some(expected.clone()))
        );

        key.oauth_invalid_reason = Some(format!(
            "{OAUTH_REFRESH_FAILED_PREFIX}Token 续期失败 (401): refresh_token 无效"
        ));
        assert_eq!(
            codex_build_invalid_state(&key, expected.clone(), 200),
            (Some(200), Some(expected))
        );

        key.oauth_invalid_reason = Some(format!(
            "{OAUTH_EXPIRED_PREFIX}old session expired\n{OAUTH_REFRESH_FAILED_PREFIX}Token 续期失败 (401): refresh_token 无效"
        ));
        assert_eq!(
            codex_build_invalid_state(
                &key,
                format!("{OAUTH_EXPIRED_PREFIX}new session expired"),
                300,
            ),
            (
                Some(300),
                Some(format!(
                    "{OAUTH_EXPIRED_PREFIX}new session expired\n{OAUTH_REFRESH_FAILED_PREFIX}Token 续期失败 (401): refresh_token 无效"
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
    fn codex_invalid_state_keeps_refresh_failure_over_request_failure() {
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
        key.oauth_invalid_reason = Some(format!(
            "{OAUTH_REFRESH_FAILED_PREFIX}Token 续期失败 (401): refresh_token 无效"
        ));

        assert_eq!(
            codex_build_invalid_state(
                &key,
                format!("{OAUTH_REQUEST_FAILED_PREFIX}账号状态检查失败"),
                200,
            ),
            (
                Some(100),
                Some(format!(
                    "{OAUTH_REFRESH_FAILED_PREFIX}Token 续期失败 (401): refresh_token 无效"
                ))
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
    fn codex_quota_parses_monthly_header_with_zero_secondary_tombstone() {
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
        assert_eq!(parsed.get("secondary_window_minutes"), Some(&json!(0u64)));
    }

    #[test]
    fn codex_quota_monthly_header_patch_removes_previous_five_hour_window() {
        let current = json!({
            "primary_used_percent": 30.0,
            "primary_reset_at": 1_790_000_000u64,
            "primary_window_minutes": 10_080u64,
            "secondary_used_percent": 70.0,
            "secondary_reset_at": 1_785_000_000u64,
            "secondary_window_minutes": 300u64,
            "account_quota_request_started_at_unix_ms": 100_000u64,
            "updated_at": 1_784_000_000u64
        });
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
        let monthly = parse_codex_usage_headers(&headers, 1_784_287_450)
            .expect("monthly Codex headers should parse");

        let outcome = merge_codex_quota(
            Some(&current),
            &monthly,
            1_784_287_450,
            110_000,
            CodexQuotaWindowCoverage::Patch,
        );

        assert!(outcome.changed);
        assert_eq!(outcome.metadata["primary_used_percent"], json!(14.0));
        assert_eq!(outcome.metadata["primary_window_minutes"], json!(43_800u64));
        assert!(outcome.metadata.get("secondary_used_percent").is_none());
        assert!(outcome.metadata.get("secondary_window_minutes").is_none());
    }

    #[test]
    fn codex_quota_parses_monthly_body_with_complete_zero_secondary_tombstone() {
        let parsed = parse_codex_wham_usage_response(
            &json!({
                "plan_type": "team",
                "rate_limit": {
                    "primary_window": {
                        "used_percent": 14.0,
                        "reset_after_seconds": 2_627_672u64,
                        "reset_at": 1_786_915_122u64,
                        "window_minutes": 43_800u64
                    },
                    "secondary_window": {
                        "used_percent": 0.0,
                        "reset_after_seconds": 0u64,
                        "reset_at": null,
                        "window_minutes": 0u64
                    }
                }
            }),
            1_784_287_450,
        )
        .expect("monthly Codex body should parse");

        assert_eq!(parsed.get("secondary_window_minutes"), Some(&json!(0u64)));
    }

    #[test]
    fn codex_quota_partial_zero_secondary_header_does_not_emit_tombstone() {
        let headers = BTreeMap::from([
            ("x-codex-primary-used-percent".to_string(), "14".to_string()),
            (
                "x-codex-primary-window-minutes".to_string(),
                "43800".to_string(),
            ),
            (
                "x-codex-secondary-window-minutes".to_string(),
                "0".to_string(),
            ),
        ]);
        let parsed = parse_codex_usage_headers(&headers, 1_784_287_450)
            .expect("partial Codex headers should parse");

        assert!(parsed.get("secondary_window_minutes").is_none());
    }

    #[test]
    fn parses_codex_websocket_rate_limits_from_chunk_envelope() {
        let parsed = parse_codex_websocket_rate_limits_response(
            &json!({
                "chunks": [
                    {"type": "response.output_text.delta", "delta": "ignored"},
                    {
                        "type": "codex.rate_limits",
                        "plan_type": "free",
                        "rate_limits": {
                            "allowed": true,
                            "limit_reached": false,
                            "primary": {
                                "used_percent": 91,
                                "window_minutes": 43200,
                                "reset_after_seconds": 2590791,
                                "reset_at": 1787154563u64
                            }
                        }
                    }
                ]
            }),
            1_787_000_000,
        )
        .expect("Codex WebSocket quota chunk should parse");

        assert_eq!(parsed.get("plan_type"), Some(&json!("free")));
        assert_eq!(parsed.get("allowed"), Some(&json!(true)));
        assert_eq!(parsed.get("limit_reached"), Some(&json!(false)));
        assert_eq!(parsed.get("primary_used_percent"), Some(&json!(91.0)));
        assert_eq!(
            parsed.get("primary_window_minutes"),
            Some(&json!(43_200u64))
        );
        assert_eq!(
            parsed.get("primary_reset_after_seconds"),
            Some(&json!(2_590_791u64))
        );
        assert!(parsed.get("secondary_used_percent").is_none());
    }

    #[test]
    fn parses_codex_websocket_usage_limit_error_headers() {
        let parsed = parse_codex_websocket_rate_limits_response(
            &json!({
                "type": "error",
                "error": {
                    "type": "usage_limit_reached",
                    "plan_type": "free",
                    "resets_at": 1_787_274_385u64,
                    "resets_in_seconds": 2_590_077u64,
                },
                "status_code": 429,
                "headers": {
                    "X-Codex-Plan-Type": "free",
                    "X-Codex-Primary-Used-Percent": "100",
                    "X-Codex-Primary-Window-Minutes": "43200",
                    "X-Codex-Primary-Reset-After-Seconds": "2590078",
                    "X-Codex-Primary-Reset-At": "1787274385",
                    "X-Codex-Credits-Has-Credits": "False",
                },
            }),
            1_787_000_000,
        )
        .expect("Codex usage-limit error should parse as quota metadata");

        assert_eq!(parsed.get("allowed"), Some(&json!(false)));
        assert_eq!(parsed.get("limit_reached"), Some(&json!(true)));
        assert_eq!(parsed.get("plan_type"), Some(&json!("free")));
        assert_eq!(parsed.get("primary_used_percent"), Some(&json!(100.0)));
        assert_eq!(
            parsed.get("primary_reset_at"),
            Some(&json!(1_787_274_385u64))
        );
        assert!(codex_rate_limit_metadata_exhausted(&parsed));
    }

    #[test]
    fn parses_codex_websocket_usage_limit_error_from_logged_header_body_wrapper() {
        let parsed = parse_codex_websocket_rate_limits_response(
            &json!({
                "header": {
                    "upgrade": "websocket",
                    "connection": "upgrade",
                },
                "body": {
                    "type": "error",
                    "error": {
                        "type": "usage_limit_reached",
                        "plan_type": "pro",
                        "resets_at": 1_788_138_591u64,
                    },
                    "status_code": 429,
                    "headers": {
                        "X-Codex-Plan-Type": "pro",
                        "X-Codex-Primary-Used-Percent": "100",
                        "X-Codex-Primary-Window-Minutes": "10080",
                        "X-Codex-Primary-Reset-At": "1788138592",
                    },
                }
            }),
            1_787_558_431,
        )
        .expect("logged websocket wrapper should parse quota headers from body");

        assert_eq!(parsed.get("allowed"), Some(&json!(false)));
        assert_eq!(parsed.get("limit_reached"), Some(&json!(true)));
        assert_eq!(parsed.get("plan_type"), Some(&json!("pro")));
        assert_eq!(parsed.get("primary_used_percent"), Some(&json!(100.0)));
        assert_eq!(
            parsed.get("primary_window_minutes"),
            Some(&json!(10_080u64))
        );
    }

    #[test]
    fn parses_codex_websocket_usage_limit_error_without_used_percent_header_as_exhausted() {
        let parsed = parse_codex_websocket_rate_limits_response(
            &json!({
                "type": "error",
                "error": {
                    "type": "usage_limit_reached",
                    "plan_type": "pro",
                    "resets_at": 1_788_138_591u64,
                },
                "status_code": 429,
                "headers": {
                    "X-Codex-Plan-Type": "pro",
                },
            }),
            1_787_558_431,
        )
        .expect("usage-limit error without used-percent header should still parse");

        assert_eq!(parsed.get("allowed"), Some(&json!(false)));
        assert_eq!(parsed.get("limit_reached"), Some(&json!(true)));
        assert_eq!(parsed.get("primary_used_percent"), Some(&json!(100.0)));
    }

    #[test]
    fn codex_rate_limit_metadata_detects_explicit_and_window_exhaustion() {
        assert!(codex_rate_limit_metadata_exhausted(&json!({
            "allowed": false
        })));
        assert!(codex_rate_limit_metadata_exhausted(&json!({
            "limit_reached": true
        })));
        assert!(codex_rate_limit_metadata_exhausted(&json!({
            "primary_used_percent": 100
        })));
        assert!(!codex_rate_limit_metadata_exhausted(&json!({
            "allowed": true,
            "primary_used_percent": 100
        })));
        assert!(!codex_rate_limit_metadata_exhausted(&json!({
            "limit_reached": false,
            "secondary_used_percent": 100
        })));
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
