pub(crate) fn is_admin_provider_strategy_strategies_root(request_path: &str) -> bool {
    matches!(
        request_path,
        "/api/admin/provider-strategy/strategies" | "/api/admin/provider-strategy/strategies/"
    )
}

pub(crate) fn admin_provider_id_for_provider_strategy_billing(
    request_path: &str,
) -> Option<String> {
    strategy_provider_id_for_suffix(request_path, "/billing")
}

pub(crate) fn admin_provider_id_for_provider_strategy_stats(request_path: &str) -> Option<String> {
    strategy_provider_id_for_suffix(request_path, "/stats")
}

pub(crate) fn admin_provider_id_for_provider_strategy_quota(request_path: &str) -> Option<String> {
    strategy_provider_id_for_suffix(request_path, "/quota")
}

fn strategy_provider_id_for_suffix(request_path: &str, suffix: &str) -> Option<String> {
    request_path
        .strip_prefix("/api/admin/provider-strategy/providers/")?
        .strip_suffix(suffix)
        .map(|value| value.trim().trim_matches('/').to_string())
        .filter(|value| !value.is_empty() && !value.contains('/'))
}
