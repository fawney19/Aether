pub(crate) fn admin_provider_id_for_health_monitor(request_path: &str) -> Option<String> {
    admin_provider_id_for_suffix(request_path, "/health-monitor")
}

pub(crate) fn admin_provider_id_for_mapping_preview(request_path: &str) -> Option<String> {
    admin_provider_id_for_suffix(request_path, "/mapping-preview")
}

pub(crate) fn admin_provider_id_for_pool_status(request_path: &str) -> Option<String> {
    admin_provider_id_for_suffix(request_path, "/pool-status")
}

pub(crate) fn admin_provider_clear_pool_cooldown_parts(
    request_path: &str,
) -> Option<(String, String)> {
    admin_provider_pool_key_route_parts(request_path, "/pool/clear-cooldown/")
}

pub(crate) fn admin_provider_reset_pool_cost_parts(request_path: &str) -> Option<(String, String)> {
    admin_provider_pool_key_route_parts(request_path, "/pool/reset-cost/")
}

pub(crate) fn admin_provider_delete_task_parts(request_path: &str) -> Option<(String, String)> {
    let raw = request_path.strip_prefix("/api/admin/providers/")?;
    let (provider_id, task_id) = raw.split_once("/delete-task/")?;
    let provider_id = provider_id.trim().trim_matches('/');
    let task_id = task_id.trim().trim_matches('/');
    if provider_id.is_empty()
        || task_id.is_empty()
        || provider_id.contains('/')
        || task_id.contains('/')
    {
        None
    } else {
        Some((provider_id.to_string(), task_id.to_string()))
    }
}

pub(crate) fn admin_provider_id_for_summary(request_path: &str) -> Option<String> {
    admin_provider_id_for_suffix(request_path, "/summary")
}

pub(crate) fn admin_provider_id_for_manage_path(request_path: &str) -> Option<String> {
    let raw = request_path.strip_prefix("/api/admin/providers/")?;
    let normalized = raw.trim().trim_matches('/');
    if normalized.is_empty() || normalized.contains('/') {
        None
    } else {
        Some(normalized.to_string())
    }
}

pub(crate) fn is_admin_providers_root(request_path: &str) -> bool {
    matches!(
        request_path,
        "/api/admin/providers" | "/api/admin/providers/"
    )
}

pub(crate) fn admin_provider_id_for_models_list(request_path: &str) -> Option<String> {
    admin_provider_id_for_suffix(request_path, "/models")
}

pub(crate) fn admin_provider_model_route_parts(request_path: &str) -> Option<(String, String)> {
    let raw = request_path.strip_prefix("/api/admin/providers/")?;
    let (provider_id, model_id) = raw.split_once("/models/")?;
    let provider_id = provider_id.trim().trim_matches('/');
    let model_id = model_id.trim().trim_matches('/');
    if provider_id.is_empty()
        || model_id.is_empty()
        || provider_id.contains('/')
        || model_id.contains('/')
    {
        None
    } else {
        Some((provider_id.to_string(), model_id.to_string()))
    }
}

pub(crate) fn admin_provider_models_batch_path(request_path: &str) -> Option<String> {
    admin_provider_id_for_suffix(request_path, "/models/batch")
}

pub(crate) fn admin_provider_available_source_models_path(request_path: &str) -> Option<String> {
    admin_provider_id_for_suffix(request_path, "/available-source-models")
}

pub(crate) fn admin_provider_assign_global_models_path(request_path: &str) -> Option<String> {
    admin_provider_id_for_suffix(request_path, "/assign-global-models")
}

pub(crate) fn admin_provider_import_models_path(request_path: &str) -> Option<String> {
    admin_provider_id_for_suffix(request_path, "/import-from-upstream")
}

fn admin_provider_id_for_suffix(request_path: &str, suffix: &str) -> Option<String> {
    let raw = request_path.strip_prefix("/api/admin/providers/")?;
    let raw = raw.strip_suffix(suffix)?;
    let normalized = raw.trim().trim_matches('/');
    if normalized.is_empty() || normalized.contains('/') {
        None
    } else {
        Some(normalized.to_string())
    }
}

fn admin_provider_pool_key_route_parts(
    request_path: &str,
    marker: &str,
) -> Option<(String, String)> {
    let raw = request_path.strip_prefix("/api/admin/providers/")?;
    let (provider_id, key_id) = raw.split_once(marker)?;
    let provider_id = provider_id.trim().trim_matches('/');
    let key_id = key_id.trim().trim_matches('/');
    if provider_id.is_empty()
        || key_id.is_empty()
        || provider_id.contains('/')
        || key_id.contains('/')
    {
        None
    } else {
        Some((provider_id.to_string(), key_id.to_string()))
    }
}
