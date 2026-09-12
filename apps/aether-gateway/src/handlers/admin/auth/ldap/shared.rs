use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(super) const ADMIN_LDAP_DATA_UNAVAILABLE_DETAIL: &str = "Admin LDAP data unavailable";
pub(super) const ADMIN_LDAP_TEST_FAILURE_MESSAGE: &str = "连接失败，请检查服务器地址、端口和凭据";

pub(super) fn admin_ldap_default_search_filter() -> String {
    "(uid={username})".to_string()
}

pub(super) fn admin_ldap_default_username_attr() -> String {
    "uid".to_string()
}

pub(super) fn admin_ldap_default_email_attr() -> String {
    "mail".to_string()
}

pub(super) fn admin_ldap_default_display_name_attr() -> String {
    "cn".to_string()
}

pub(super) fn admin_ldap_default_connect_timeout() -> i32 {
    10
}

pub(super) fn is_admin_ldap_config_root(request_path: &str) -> bool {
    matches!(
        request_path,
        "/api/admin/ldap/config" | "/api/admin/ldap/config/"
    )
}

pub(super) fn is_admin_ldap_test_root(request_path: &str) -> bool {
    matches!(
        request_path,
        "/api/admin/ldap/test" | "/api/admin/ldap/test/"
    )
}

pub(super) fn build_admin_ldap_config_payload(
    config: Option<&aether_data::repository::auth_modules::StoredLdapModuleConfig>,
) -> serde_json::Value {
    match config {
        Some(config) => json!({
            "server_url": config.server_url,
            "bind_dn": config.bind_dn,
            "base_dn": config.base_dn,
            "has_bind_password": config
                .bind_password_encrypted
                .as_deref()
                .map(str::trim)
                .is_some_and(|value| !value.is_empty()),
            "user_search_filter": config
                .user_search_filter
                .clone()
                .unwrap_or_else(admin_ldap_default_search_filter),
            "username_attr": config
                .username_attr
                .clone()
                .unwrap_or_else(admin_ldap_default_username_attr),
            "email_attr": config
                .email_attr
                .clone()
                .unwrap_or_else(admin_ldap_default_email_attr),
            "display_name_attr": config
                .display_name_attr
                .clone()
                .unwrap_or_else(admin_ldap_default_display_name_attr),
            "is_enabled": config.is_enabled,
            "is_exclusive": config.is_exclusive,
            "use_starttls": config.use_starttls,
            "connect_timeout": config.connect_timeout.unwrap_or(admin_ldap_default_connect_timeout()),
        }),
        None => json!({
            "server_url": serde_json::Value::Null,
            "bind_dn": serde_json::Value::Null,
            "base_dn": serde_json::Value::Null,
            "has_bind_password": false,
            "user_search_filter": admin_ldap_default_search_filter(),
            "username_attr": admin_ldap_default_username_attr(),
            "email_attr": admin_ldap_default_email_attr(),
            "display_name_attr": admin_ldap_default_display_name_attr(),
            "is_enabled": false,
            "is_exclusive": false,
            "use_starttls": false,
            "connect_timeout": admin_ldap_default_connect_timeout(),
        }),
    }
}

pub(super) fn admin_ldap_bad_request_response(detail: impl Into<String>) -> Response<Body> {
    (
        http::StatusCode::BAD_REQUEST,
        Json(json!({ "detail": detail.into() })),
    )
        .into_response()
}

pub(super) fn admin_ldap_conflict_response(detail: impl Into<String>) -> Response<Body> {
    (
        http::StatusCode::CONFLICT,
        Json(json!({ "detail": detail.into() })),
    )
        .into_response()
}

pub(super) fn admin_ldap_unavailable_response() -> Response<Body> {
    (
        http::StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "detail": ADMIN_LDAP_DATA_UNAVAILABLE_DETAIL })),
    )
        .into_response()
}

pub(super) fn admin_ldap_normalize_server_url(
    server_url: &str,
    use_starttls: bool,
) -> Option<String> {
    crate::handlers::shared::normalize_ldap_transport_server_url(server_url, use_starttls)
}
