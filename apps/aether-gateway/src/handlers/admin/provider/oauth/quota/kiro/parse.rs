use aether_admin::provider::quota as admin_provider_quota_pure;

pub(super) fn parse_kiro_usage_response(
    value: &serde_json::Value,
    updated_at_unix_secs: u64,
) -> Option<serde_json::Value> {
    admin_provider_quota_pure::parse_kiro_usage_response(value, updated_at_unix_secs)
}
