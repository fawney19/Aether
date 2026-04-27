use crate::handlers::admin::request::AdminAppState;
use crate::LocalProviderDeleteTaskState;
use serde_json::json;
use std::collections::BTreeMap;

pub(crate) const ADMIN_PROVIDER_MAPPING_PREVIEW_MAX_KEYS: usize = 200;
pub(crate) const ADMIN_PROVIDER_MAPPING_PREVIEW_MAX_MODELS: usize = 500;
pub(crate) const ADMIN_PROVIDER_MAPPING_PREVIEW_FETCH_LIMIT: usize = 10_000;
pub(crate) const ADMIN_PROVIDER_POOL_SCAN_BATCH: u64 = 200;
pub(crate) const ADMIN_PROVIDER_OAUTH_DATA_UNAVAILABLE_DETAIL: &str =
    "Admin provider OAuth data unavailable";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AdminProviderPoolSchedulingPreset {
    pub(crate) preset: String,
    pub(crate) enabled: bool,
    pub(crate) mode: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AdminProviderPoolUnschedulableRule {
    pub(crate) keyword: String,
    pub(crate) duration_minutes: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct AdminProviderPoolConfig {
    pub(crate) scheduling_presets: Vec<AdminProviderPoolSchedulingPreset>,
    pub(crate) unschedulable_rules: Vec<AdminProviderPoolUnschedulableRule>,
    pub(crate) lru_enabled: bool,
    pub(crate) skip_exhausted_accounts: bool,
    pub(crate) sticky_session_ttl_seconds: u64,
    pub(crate) latency_window_seconds: u64,
    pub(crate) latency_sample_limit: u64,
    pub(crate) cost_window_seconds: u64,
    pub(crate) cost_limit_per_key_tokens: Option<u64>,
    pub(crate) rate_limit_cooldown_seconds: u64,
    pub(crate) overload_cooldown_seconds: u64,
    pub(crate) health_policy_enabled: bool,
    pub(crate) probing_enabled: bool,
    pub(crate) probing_interval_minutes: u64,
    pub(crate) stream_timeout_threshold: u64,
    pub(crate) stream_timeout_window_seconds: u64,
    pub(crate) stream_timeout_cooldown_seconds: u64,
}

#[derive(Debug, Default)]
pub(crate) struct AdminProviderPoolRuntimeState {
    pub(crate) total_sticky_sessions: usize,
    pub(crate) sticky_sessions_by_key: BTreeMap<String, usize>,
    pub(crate) sticky_bound_key_id: Option<String>,
    pub(crate) cooldown_reason_by_key: BTreeMap<String, String>,
    pub(crate) cooldown_ttl_by_key: BTreeMap<String, u64>,
    pub(crate) cost_window_usage_by_key: BTreeMap<String, u64>,
    pub(crate) latency_avg_ms_by_key: BTreeMap<String, f64>,
    pub(crate) lru_score_by_key: BTreeMap<String, f64>,
}

pub(crate) fn build_admin_provider_delete_task_payload(
    task: &LocalProviderDeleteTaskState,
) -> serde_json::Value {
    json!({
        "task_id": task.task_id,
        "provider_id": task.provider_id,
        "status": task.status,
        "stage": task.stage,
        "total_keys": task.total_keys,
        "deleted_keys": task.deleted_keys,
        "total_endpoints": task.total_endpoints,
        "deleted_endpoints": task.deleted_endpoints,
        "message": task.message,
    })
}

pub(crate) fn put_admin_provider_delete_task(
    state: &AdminAppState<'_>,
    task: &LocalProviderDeleteTaskState,
) {
    state.as_ref().put_provider_delete_task(task.clone());
}

pub(crate) fn normalize_provider_billing_type(value: &str) -> Result<String, String> {
    let normalized = value.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "monthly_quota" | "pay_as_you_go" | "free_tier" => Ok(normalized),
        _ => Err("billing_type 仅支持 monthly_quota / pay_as_you_go / free_tier".to_string()),
    }
}

pub(crate) fn parse_optional_rfc3339_unix_secs(
    value: &str,
    field_name: &str,
) -> Result<u64, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{field_name} 不能为空"));
    }
    let parsed = chrono::DateTime::parse_from_rfc3339(trimmed)
        .map_err(|_| format!("{field_name} 必须是合法的 RFC3339 时间"))?;
    u64::try_from(parsed.timestamp()).map_err(|_| format!("{field_name} 超出有效时间范围"))
}
