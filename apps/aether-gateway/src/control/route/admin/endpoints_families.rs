use axum::http;

use super::{classified, ClassifiedRoute};

pub(super) fn classify_admin_endpoints_family_route(
    method: &http::Method,
    normalized_path: &str,
) -> Option<ClassifiedRoute> {
    if method == http::Method::GET && normalized_path == "/api/admin/endpoints/health/summary" {
        Some(classified(
            "admin_proxy",
            "endpoints_health",
            "health_summary",
            "admin:endpoints_health",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path.starts_with("/api/admin/endpoints/health/key/")
    {
        Some(classified(
            "admin_proxy",
            "endpoints_health",
            "key_health",
            "admin:endpoints_health",
            false,
        ))
    } else if method == http::Method::PATCH
        && normalized_path.starts_with("/api/admin/endpoints/health/keys/")
    {
        Some(classified(
            "admin_proxy",
            "endpoints_health",
            "recover_key_health",
            "admin:endpoints_health",
            false,
        ))
    } else if method == http::Method::PATCH && normalized_path == "/api/admin/endpoints/health/keys"
    {
        Some(classified(
            "admin_proxy",
            "endpoints_health",
            "recover_all_keys_health",
            "admin:endpoints_health",
            false,
        ))
    } else if method == http::Method::GET && normalized_path == "/api/admin/endpoints/health/status"
    {
        Some(classified(
            "admin_proxy",
            "endpoints_health",
            "health_status",
            "admin:endpoints_health",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path == "/api/admin/endpoints/health/api-formats"
    {
        Some(classified(
            "admin_proxy",
            "endpoints_health",
            "health_api_formats",
            "admin:endpoints_health",
            false,
        ))
    } else if method == http::Method::GET && normalized_path == "/api/admin/endpoints/health/models"
    {
        Some(classified(
            "admin_proxy",
            "endpoints_health",
            "health_models",
            "admin:endpoints_health",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path == "/api/admin/endpoints/health/providers"
    {
        Some(classified(
            "admin_proxy",
            "endpoints_health",
            "health_providers",
            "admin:endpoints_health",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path.starts_with("/api/admin/endpoints/rpm/key/")
    {
        Some(classified(
            "admin_proxy",
            "endpoints_rpm",
            "key_rpm",
            "admin:endpoints_rpm",
            false,
        ))
    } else if method == http::Method::DELETE
        && normalized_path.starts_with("/api/admin/endpoints/rpm/key/")
    {
        Some(classified(
            "admin_proxy",
            "endpoints_rpm",
            "reset_key_rpm",
            "admin:endpoints_rpm",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path == "/api/admin/endpoints/keys/grouped-by-format"
    {
        Some(classified(
            "admin_proxy",
            "endpoints_manage",
            "keys_grouped_by_format",
            "admin:endpoints_manage",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path.starts_with("/api/admin/endpoints/keys/")
        && normalized_path.ends_with("/reveal")
    {
        Some(classified(
            "admin_proxy",
            "endpoints_manage",
            "reveal_key",
            "admin:endpoints_manage",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path.starts_with("/api/admin/endpoints/keys/")
        && normalized_path.ends_with("/export")
    {
        Some(classified(
            "admin_proxy",
            "endpoints_manage",
            "export_key",
            "admin:endpoints_manage",
            false,
        ))
    } else if method == http::Method::PUT
        && normalized_path.starts_with("/api/admin/endpoints/keys/")
    {
        Some(classified(
            "admin_proxy",
            "endpoints_manage",
            "update_key",
            "admin:endpoints_manage",
            false,
        ))
    } else if method == http::Method::DELETE
        && normalized_path.starts_with("/api/admin/endpoints/keys/")
    {
        Some(classified(
            "admin_proxy",
            "endpoints_manage",
            "delete_key",
            "admin:endpoints_manage",
            false,
        ))
    } else if method == http::Method::POST
        && normalized_path == "/api/admin/endpoints/keys/batch-delete"
    {
        Some(classified(
            "admin_proxy",
            "endpoints_manage",
            "batch_delete_keys",
            "admin:endpoints_manage",
            false,
        ))
    } else if method == http::Method::POST
        && normalized_path.starts_with("/api/admin/endpoints/keys/")
        && normalized_path.ends_with("/clear-oauth-invalid")
    {
        Some(classified(
            "admin_proxy",
            "endpoints_manage",
            "clear_oauth_invalid",
            "admin:endpoints_manage",
            false,
        ))
    } else if method == http::Method::POST
        && normalized_path.starts_with("/api/admin/endpoints/keys/")
        && normalized_path.ends_with("/reset-cycle-stats")
    {
        Some(classified(
            "admin_proxy",
            "endpoints_manage",
            "reset_cycle_stats",
            "admin:endpoints_manage",
            false,
        ))
    } else if method == http::Method::POST
        && normalized_path.starts_with("/api/admin/endpoints/providers/")
        && normalized_path.ends_with("/refresh-quota")
    {
        Some(classified(
            "admin_proxy",
            "endpoints_manage",
            "refresh_quota",
            "admin:endpoints_manage",
            false,
        ))
    } else if method == http::Method::POST
        && normalized_path.starts_with("/api/admin/endpoints/providers/")
        && normalized_path.ends_with("/keys")
    {
        Some(classified(
            "admin_proxy",
            "endpoints_manage",
            "create_provider_key",
            "admin:endpoints_manage",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path.starts_with("/api/admin/endpoints/providers/")
        && normalized_path.ends_with("/keys")
    {
        Some(classified(
            "admin_proxy",
            "endpoints_manage",
            "list_provider_keys",
            "admin:endpoints_manage",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path.starts_with("/api/admin/endpoints/providers/")
        && normalized_path.ends_with("/endpoints")
    {
        Some(classified(
            "admin_proxy",
            "endpoints_manage",
            "list_provider_endpoints",
            "admin:endpoints_manage",
            false,
        ))
    } else if method == http::Method::POST
        && normalized_path.starts_with("/api/admin/endpoints/providers/")
        && normalized_path.ends_with("/endpoints")
    {
        Some(classified(
            "admin_proxy",
            "endpoints_manage",
            "create_endpoint",
            "admin:endpoints_manage",
            false,
        ))
    } else if method == http::Method::PUT
        && normalized_path.starts_with("/api/admin/endpoints/")
        && !normalized_path.starts_with("/api/admin/endpoints/health/")
        && !normalized_path.starts_with("/api/admin/endpoints/rpm/")
        && !normalized_path.starts_with("/api/admin/endpoints/providers/")
        && !normalized_path.starts_with("/api/admin/endpoints/defaults/")
        && !normalized_path.starts_with("/api/admin/endpoints/keys/")
    {
        Some(classified(
            "admin_proxy",
            "endpoints_manage",
            "update_endpoint",
            "admin:endpoints_manage",
            false,
        ))
    } else if method == http::Method::DELETE
        && normalized_path.starts_with("/api/admin/endpoints/")
        && !normalized_path.starts_with("/api/admin/endpoints/health/")
        && !normalized_path.starts_with("/api/admin/endpoints/rpm/")
        && !normalized_path.starts_with("/api/admin/endpoints/providers/")
        && !normalized_path.starts_with("/api/admin/endpoints/defaults/")
        && !normalized_path.starts_with("/api/admin/endpoints/keys/")
    {
        Some(classified(
            "admin_proxy",
            "endpoints_manage",
            "delete_endpoint",
            "admin:endpoints_manage",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path.starts_with("/api/admin/endpoints/defaults/")
        && normalized_path.ends_with("/body-rules")
    {
        Some(classified(
            "admin_proxy",
            "endpoints_manage",
            "default_body_rules",
            "admin:endpoints_manage",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path.starts_with("/api/admin/endpoints/")
        && !normalized_path.starts_with("/api/admin/endpoints/health/")
        && !normalized_path.starts_with("/api/admin/endpoints/rpm/")
        && !normalized_path.starts_with("/api/admin/endpoints/providers/")
        && !normalized_path.starts_with("/api/admin/endpoints/defaults/")
        && !normalized_path.starts_with("/api/admin/endpoints/keys/")
    {
        Some(classified(
            "admin_proxy",
            "endpoints_manage",
            "get_endpoint",
            "admin:endpoints_manage",
            false,
        ))
    } else {
        None
    }
}
