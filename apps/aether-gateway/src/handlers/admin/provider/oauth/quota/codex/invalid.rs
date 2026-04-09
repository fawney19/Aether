use aether_admin::provider::quota as admin_provider_quota_pure;
use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;

pub(super) fn codex_build_invalid_state(
    key: &StoredProviderCatalogKey,
    candidate_reason: String,
    now_unix_secs: u64,
) -> (Option<u64>, Option<String>) {
    admin_provider_quota_pure::codex_build_invalid_state(key, candidate_reason, now_unix_secs)
}

pub(super) fn codex_looks_like_token_invalidated(message: Option<&str>) -> bool {
    admin_provider_quota_pure::codex_looks_like_token_invalidated(message)
}

pub(super) fn codex_looks_like_workspace_deactivated(message: Option<&str>) -> bool {
    admin_provider_quota_pure::codex_looks_like_workspace_deactivated(message)
}

pub(super) fn codex_structured_invalid_reason(
    status_code: u16,
    upstream_message: Option<&str>,
) -> String {
    admin_provider_quota_pure::codex_structured_invalid_reason(status_code, upstream_message)
}

pub(super) fn codex_soft_request_failure_reason(
    status_code: u16,
    upstream_message: Option<&str>,
) -> String {
    admin_provider_quota_pure::codex_soft_request_failure_reason(status_code, upstream_message)
}
