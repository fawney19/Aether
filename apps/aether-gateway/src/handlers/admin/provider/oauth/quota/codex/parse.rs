use aether_admin::provider::quota as admin_provider_quota_pure;
use std::collections::BTreeMap;

pub(super) fn normalize_codex_plan_type(value: Option<&str>) -> Option<String> {
    admin_provider_quota_pure::normalize_codex_plan_type(value)
}

pub(super) fn build_codex_quota_exhausted_fallback_metadata(
    plan_type: Option<&str>,
    updated_at_unix_secs: u64,
) -> serde_json::Value {
    admin_provider_quota_pure::build_codex_quota_exhausted_fallback_metadata(
        plan_type,
        updated_at_unix_secs,
    )
}

pub(super) fn parse_codex_wham_usage_response(
    value: &serde_json::Value,
    updated_at_unix_secs: u64,
) -> Option<serde_json::Value> {
    admin_provider_quota_pure::parse_codex_wham_usage_response(value, updated_at_unix_secs)
}

pub(super) fn parse_codex_usage_headers(
    headers: &BTreeMap<String, String>,
    updated_at_unix_secs: u64,
) -> Option<serde_json::Value> {
    admin_provider_quota_pure::parse_codex_usage_headers(headers, updated_at_unix_secs)
}
