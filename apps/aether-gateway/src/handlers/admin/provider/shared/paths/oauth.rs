pub(crate) fn admin_provider_oauth_start_key_id(request_path: &str) -> Option<String> {
    request_path
        .strip_prefix("/api/admin/provider-oauth/keys/")?
        .strip_suffix("/start")
        .filter(|key_id| !key_id.is_empty() && !key_id.contains('/'))
        .map(ToOwned::to_owned)
}

pub(crate) fn admin_provider_oauth_start_provider_id(request_path: &str) -> Option<String> {
    provider_oauth_provider_id_for_suffix(request_path, "/start")
}

pub(crate) fn admin_provider_oauth_complete_key_id(request_path: &str) -> Option<String> {
    request_path
        .strip_prefix("/api/admin/provider-oauth/keys/")?
        .strip_suffix("/complete")
        .filter(|key_id| !key_id.is_empty() && !key_id.contains('/'))
        .map(ToOwned::to_owned)
}

pub(crate) fn admin_provider_oauth_refresh_key_id(request_path: &str) -> Option<String> {
    request_path
        .strip_prefix("/api/admin/provider-oauth/keys/")?
        .strip_suffix("/refresh")
        .filter(|key_id| !key_id.is_empty() && !key_id.contains('/'))
        .map(ToOwned::to_owned)
}

pub(crate) fn admin_provider_oauth_complete_provider_id(request_path: &str) -> Option<String> {
    provider_oauth_provider_id_for_suffix(request_path, "/complete")
}

pub(crate) fn admin_provider_oauth_import_provider_id(request_path: &str) -> Option<String> {
    provider_oauth_provider_id_for_suffix(request_path, "/import-refresh-token")
}

pub(crate) fn admin_provider_oauth_batch_import_provider_id(request_path: &str) -> Option<String> {
    provider_oauth_provider_id_for_suffix(request_path, "/batch-import")
}

pub(crate) fn admin_provider_oauth_batch_import_task_provider_id(
    request_path: &str,
) -> Option<String> {
    provider_oauth_provider_id_for_suffix(request_path, "/batch-import/tasks")
}

pub(crate) fn admin_provider_oauth_batch_import_task_path(
    request_path: &str,
) -> Option<(String, String)> {
    let suffix = request_path
        .strip_prefix("/api/admin/provider-oauth/providers/")?
        .strip_suffix("/")
        .unwrap_or(request_path.strip_prefix("/api/admin/provider-oauth/providers/")?);
    let (provider_id, task_path) = suffix.split_once("/batch-import/tasks/")?;
    if provider_id.is_empty()
        || provider_id.contains('/')
        || task_path.is_empty()
        || task_path.contains('/')
    {
        return None;
    }
    Some((provider_id.to_string(), task_path.to_string()))
}

pub(crate) fn admin_provider_oauth_device_authorize_provider_id(
    request_path: &str,
) -> Option<String> {
    provider_oauth_provider_id_for_suffix(request_path, "/device-authorize")
}

pub(crate) fn admin_provider_oauth_device_poll_provider_id(request_path: &str) -> Option<String> {
    provider_oauth_provider_id_for_suffix(request_path, "/device-poll")
}

fn provider_oauth_provider_id_for_suffix(request_path: &str, suffix: &str) -> Option<String> {
    request_path
        .strip_prefix("/api/admin/provider-oauth/providers/")?
        .strip_suffix(suffix)
        .filter(|provider_id| !provider_id.is_empty() && !provider_id.contains('/'))
        .map(ToOwned::to_owned)
}
