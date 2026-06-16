pub(crate) fn is_admin_global_models_root(request_path: &str) -> bool {
    matches!(
        request_path,
        "/api/admin/models/global" | "/api/admin/models/global/"
    )
}

pub(crate) fn admin_global_model_id_from_path(request_path: &str) -> Option<String> {
    let raw = request_path.strip_prefix("/api/admin/models/global/")?;
    let normalized = raw.trim().trim_matches('/');
    if normalized.is_empty()
        || normalized.contains('/')
        || normalized == "batch-delete"
        || normalized == "codex-adapter-compatibility"
        || normalized.ends_with("/providers")
        || normalized.ends_with("/assign-to-providers")
        || normalized.ends_with("/routing")
    {
        None
    } else {
        Some(normalized.to_string())
    }
}

pub(crate) fn admin_global_model_assign_to_providers_id(request_path: &str) -> Option<String> {
    request_path
        .strip_prefix("/api/admin/models/global/")?
        .strip_suffix("/assign-to-providers")
        .map(|value| value.trim().trim_matches('/').to_string())
        .filter(|value| !value.is_empty() && !value.contains('/'))
}

pub(crate) fn admin_global_model_routing_id(request_path: &str) -> Option<String> {
    request_path
        .strip_prefix("/api/admin/models/global/")?
        .strip_suffix("/routing")
        .map(|value| value.trim().trim_matches('/').to_string())
        .filter(|value| !value.is_empty() && !value.contains('/'))
}

pub(crate) fn admin_global_model_providers_id(request_path: &str) -> Option<String> {
    request_path
        .strip_prefix("/api/admin/models/global/")?
        .strip_suffix("/providers")
        .map(|value| value.trim().trim_matches('/').to_string())
        .filter(|value| !value.is_empty() && !value.contains('/'))
}
