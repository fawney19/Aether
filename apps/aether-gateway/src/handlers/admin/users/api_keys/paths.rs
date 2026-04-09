pub(in super::super) fn admin_user_api_key_full_key_parts(
    request_path: &str,
) -> Option<(String, String)> {
    let raw = request_path.strip_prefix("/api/admin/users/")?;
    let (user_id, key_id) = raw.split_once("/api-keys/")?;
    let user_id = user_id.trim().trim_matches('/');
    let key_id = key_id
        .trim()
        .trim_matches('/')
        .strip_suffix("/full-key")?
        .trim()
        .trim_matches('/');
    if user_id.is_empty() || key_id.is_empty() || user_id.contains('/') || key_id.contains('/') {
        None
    } else {
        Some((user_id.to_string(), key_id.to_string()))
    }
}

pub(in super::super) fn admin_user_api_key_parts(request_path: &str) -> Option<(String, String)> {
    let raw = request_path.strip_prefix("/api/admin/users/")?;
    let (user_id, key_id) = raw.split_once("/api-keys/")?;
    let user_id = user_id.trim().trim_matches('/');
    let key_id = key_id.trim().trim_matches('/');
    if user_id.is_empty() || key_id.is_empty() || user_id.contains('/') || key_id.contains('/') {
        None
    } else {
        Some((user_id.to_string(), key_id.to_string()))
    }
}

pub(in super::super) fn admin_user_api_key_lock_parts(
    request_path: &str,
) -> Option<(String, String)> {
    let raw = request_path.strip_prefix("/api/admin/users/")?;
    let (user_id, key_id) = raw.split_once("/api-keys/")?;
    let user_id = user_id.trim().trim_matches('/');
    let key_id = key_id
        .trim()
        .trim_matches('/')
        .strip_suffix("/lock")?
        .trim()
        .trim_matches('/');
    if user_id.is_empty() || key_id.is_empty() || user_id.contains('/') || key_id.contains('/') {
        None
    } else {
        Some((user_id.to_string(), key_id.to_string()))
    }
}

pub(in super::super) fn admin_user_id_from_api_keys_path(request_path: &str) -> Option<String> {
    request_path
        .strip_prefix("/api/admin/users/")?
        .strip_suffix("/api-keys")
        .map(|value| value.trim().trim_matches('/').to_string())
        .filter(|value| !value.is_empty() && !value.contains('/'))
}
