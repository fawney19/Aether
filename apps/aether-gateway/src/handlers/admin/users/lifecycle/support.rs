use super::super::format_optional_datetime_iso8601;
use crate::handlers::admin::request::AdminAppState;
use crate::GatewayError;
use serde_json::json;

pub(super) async fn admin_user_password_policy(
    state: &AdminAppState<'_>,
) -> Result<String, GatewayError> {
    let config = state
        .read_system_config_json_value("password_policy_level")
        .await?;
    Ok(
        match config
            .as_ref()
            .and_then(|value| value.as_str())
            .unwrap_or("weak")
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "medium" => "medium".to_string(),
            "strong" => "strong".to_string(),
            _ => "weak".to_string(),
        },
    )
}

pub(super) async fn find_admin_export_user(
    state: &AdminAppState<'_>,
    user_id: &str,
) -> Result<Option<aether_data::repository::users::StoredUserExportRow>, GatewayError> {
    state.find_export_user_by_id(user_id).await
}

pub(super) fn build_admin_user_payload(
    user: &aether_data::repository::users::StoredUserAuthRecord,
    rate_limit: Option<i32>,
    unlimited: bool,
) -> serde_json::Value {
    json!({
        "id": user.id,
        "email": user.email,
        "username": user.username,
        "role": user.role,
        "allowed_providers": user.allowed_providers,
        "allowed_api_formats": user.allowed_api_formats,
        "allowed_models": user.allowed_models,
        "rate_limit": rate_limit,
        "unlimited": unlimited,
        "is_active": user.is_active,
        "created_at": format_optional_datetime_iso8601(user.created_at),
        "updated_at": serde_json::Value::Null,
        "last_login_at": format_optional_datetime_iso8601(user.last_login_at),
    })
}

pub(super) fn admin_user_id_from_detail_path(request_path: &str) -> Option<String> {
    let value = request_path
        .strip_prefix("/api/admin/users/")?
        .trim()
        .trim_matches('/')
        .to_string();
    if value.is_empty() || value.contains('/') {
        None
    } else {
        Some(value)
    }
}
