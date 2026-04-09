pub(crate) fn is_admin_provider_ops_architectures_root(request_path: &str) -> bool {
    matches!(
        request_path,
        "/api/admin/provider-ops/architectures" | "/api/admin/provider-ops/architectures/"
    )
}

pub(crate) fn admin_provider_ops_architecture_id_from_path(request_path: &str) -> Option<String> {
    let raw = request_path.strip_prefix("/api/admin/provider-ops/architectures/")?;
    let normalized = raw.trim().trim_matches('/');
    if normalized.is_empty() || normalized.contains('/') {
        None
    } else {
        Some(normalized.to_string())
    }
}

pub(crate) fn admin_provider_id_for_provider_ops_status(request_path: &str) -> Option<String> {
    provider_ops_provider_id_for_suffix(request_path, "/status")
}

pub(crate) fn admin_provider_id_for_provider_ops_config(request_path: &str) -> Option<String> {
    provider_ops_provider_id_for_suffix(request_path, "/config")
}

pub(crate) fn admin_provider_id_for_provider_ops_disconnect(request_path: &str) -> Option<String> {
    provider_ops_provider_id_for_suffix(request_path, "/disconnect")
}

pub(crate) fn admin_provider_id_for_provider_ops_connect(request_path: &str) -> Option<String> {
    provider_ops_provider_id_for_suffix(request_path, "/connect")
}

pub(crate) fn admin_provider_id_for_provider_ops_verify(request_path: &str) -> Option<String> {
    provider_ops_provider_id_for_suffix(request_path, "/verify")
}

pub(crate) fn admin_provider_id_for_provider_ops_balance(request_path: &str) -> Option<String> {
    provider_ops_provider_id_for_suffix(request_path, "/balance")
}

pub(crate) fn admin_provider_id_for_provider_ops_checkin(request_path: &str) -> Option<String> {
    provider_ops_provider_id_for_suffix(request_path, "/checkin")
}

pub(crate) fn admin_provider_ops_action_route_parts(
    request_path: &str,
) -> Option<(String, String)> {
    let raw = request_path.strip_prefix("/api/admin/provider-ops/providers/")?;
    let (provider_id, action_type) = raw.split_once("/actions/")?;
    let provider_id = provider_id.trim().trim_matches('/');
    let action_type = action_type.trim().trim_matches('/');
    if provider_id.is_empty()
        || action_type.is_empty()
        || provider_id.contains('/')
        || action_type.contains('/')
    {
        None
    } else {
        Some((provider_id.to_string(), action_type.to_string()))
    }
}

pub(crate) fn is_admin_provider_ops_batch_balance_root(request_path: &str) -> bool {
    matches!(
        request_path,
        "/api/admin/provider-ops/batch/balance" | "/api/admin/provider-ops/batch/balance/"
    )
}

fn provider_ops_provider_id_for_suffix(request_path: &str, suffix: &str) -> Option<String> {
    request_path
        .strip_prefix("/api/admin/provider-ops/providers/")?
        .strip_suffix(suffix)
        .map(|value| value.trim().trim_matches('/').to_string())
        .filter(|value| !value.is_empty() && !value.contains('/'))
}
