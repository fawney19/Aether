use axum::http;

use super::{classified, ClassifiedRoute};

pub(super) fn classify_admin_system_family_route(
    method: &http::Method,
    normalized_path: &str,
    _normalized_path_no_trailing: &str,
) -> Option<ClassifiedRoute> {
    if method == http::Method::GET && normalized_path == "/api/admin/system/version" {
        Some(classified(
            "admin_proxy",
            "system_manage",
            "version",
            "admin:system",
            false,
        ))
    } else if method == http::Method::GET && normalized_path == "/api/admin/system/check-update" {
        Some(classified(
            "admin_proxy",
            "system_manage",
            "check_update",
            "admin:system",
            false,
        ))
    } else if method == http::Method::GET && normalized_path == "/api/admin/system/releases" {
        Some(classified(
            "admin_proxy",
            "system_manage",
            "releases",
            "admin:system",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path == "/api/admin/system/update-capability"
    {
        Some(classified(
            "admin_proxy",
            "system_manage",
            "update_capability",
            "admin:system",
            false,
        ))
    } else if method == http::Method::POST && normalized_path == "/api/admin/system/prepare-update"
    {
        Some(classified(
            "admin_proxy",
            "system_manage",
            "prepare_update",
            "admin:system",
            false,
        ))
    } else if method == http::Method::POST && normalized_path == "/api/admin/system/apply-update" {
        Some(classified(
            "admin_proxy",
            "system_manage",
            "apply_update",
            "admin:system",
            false,
        ))
    } else if method == http::Method::POST && normalized_path == "/api/admin/system/rollback" {
        Some(classified(
            "admin_proxy",
            "system_manage",
            "rollback",
            "admin:system",
            false,
        ))
    } else if method == http::Method::GET && normalized_path == "/api/admin/system/update-status" {
        Some(classified(
            "admin_proxy",
            "system_manage",
            "update_status",
            "admin:system",
            false,
        ))
    } else if method == http::Method::GET && normalized_path == "/api/admin/system/update-history" {
        Some(classified(
            "admin_proxy",
            "system_manage",
            "update_history",
            "admin:system",
            false,
        ))
    } else if method == http::Method::GET && normalized_path == "/api/admin/system/aws-regions" {
        Some(classified(
            "admin_proxy",
            "system_manage",
            "aws_regions",
            "admin:system",
            false,
        ))
    } else if method == http::Method::GET && normalized_path == "/api/admin/system/stats" {
        Some(classified(
            "admin_proxy",
            "system_manage",
            "stats",
            "admin:system",
            false,
        ))
    } else if method == http::Method::GET && normalized_path == "/api/admin/system/settings" {
        Some(classified(
            "admin_proxy",
            "system_manage",
            "settings_get",
            "admin:system",
            false,
        ))
    } else if method == http::Method::GET && normalized_path == "/api/admin/system/config/export" {
        Some(classified(
            "admin_proxy",
            "system_manage",
            "config_export",
            "admin:system",
            false,
        ))
    } else if method == http::Method::GET && normalized_path == "/api/admin/system/users/export" {
        Some(classified(
            "admin_proxy",
            "system_manage",
            "users_export",
            "admin:system",
            false,
        ))
    } else if method == http::Method::GET && normalized_path == "/api/admin/system/data/export" {
        Some(classified(
            "admin_proxy",
            "system_manage",
            "data_export",
            "admin:system",
            false,
        ))
    } else if method == http::Method::POST && normalized_path == "/api/admin/system/backups/s3/run"
    {
        Some(classified(
            "admin_proxy",
            "system_manage",
            "s3_backup_run",
            "admin:system",
            false,
        ))
    } else if method == http::Method::POST && normalized_path == "/api/admin/system/config/import" {
        Some(classified(
            "admin_proxy",
            "system_manage",
            "config_import",
            "admin:system",
            false,
        ))
    } else if method == http::Method::POST && normalized_path == "/api/admin/system/users/import" {
        Some(classified(
            "admin_proxy",
            "system_manage",
            "users_import",
            "admin:system",
            false,
        ))
    } else if method == http::Method::POST && normalized_path == "/api/admin/system/data/import" {
        Some(classified(
            "admin_proxy",
            "system_manage",
            "data_import",
            "admin:system",
            false,
        ))
    } else if method == http::Method::POST && normalized_path == "/api/admin/system/smtp/test" {
        Some(classified(
            "admin_proxy",
            "system_manage",
            "smtp_test",
            "admin:system",
            false,
        ))
    } else if method == http::Method::POST
        && normalized_path == "/api/admin/system/important-notification/test"
    {
        Some(classified(
            "admin_proxy",
            "system_manage",
            "important_notification_test",
            "admin:system",
            false,
        ))
    } else if method == http::Method::POST && normalized_path == "/api/admin/system/cleanup" {
        Some(classified(
            "admin_proxy",
            "system_manage",
            "cleanup",
            "admin:system",
            false,
        ))
    } else if method == http::Method::GET && normalized_path == "/api/admin/system/cleanup/runs" {
        Some(classified(
            "admin_proxy",
            "system_manage",
            "cleanup_runs",
            "admin:system",
            false,
        ))
    } else if method == http::Method::POST
        && normalized_path == "/api/admin/system/cleanup/usage/manual"
    {
        Some(classified(
            "admin_proxy",
            "system_manage",
            "cleanup_usage_manual",
            "admin:system",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path == "/api/admin/system/cleanup/usage/preview"
    {
        Some(classified(
            "admin_proxy",
            "system_manage",
            "cleanup_usage_preview",
            "admin:system",
            false,
        ))
    } else if method == http::Method::POST && normalized_path == "/api/admin/system/purge/config" {
        Some(classified(
            "admin_proxy",
            "system_manage",
            "purge_config",
            "admin:system",
            false,
        ))
    } else if method == http::Method::POST && normalized_path == "/api/admin/system/purge/users" {
        Some(classified(
            "admin_proxy",
            "system_manage",
            "purge_users",
            "admin:system",
            false,
        ))
    } else if method == http::Method::POST && normalized_path == "/api/admin/system/purge/usage" {
        Some(classified(
            "admin_proxy",
            "system_manage",
            "purge_usage",
            "admin:system",
            false,
        ))
    } else if method == http::Method::POST
        && normalized_path == "/api/admin/system/purge/audit-logs"
    {
        Some(classified(
            "admin_proxy",
            "system_manage",
            "purge_audit_logs",
            "admin:system",
            false,
        ))
    } else if method == http::Method::POST
        && normalized_path == "/api/admin/system/purge/request-bodies"
    {
        Some(classified(
            "admin_proxy",
            "system_manage",
            "purge_request_bodies",
            "admin:system",
            false,
        ))
    } else if method == http::Method::POST
        && normalized_path == "/api/admin/system/purge/request-bodies/task"
    {
        Some(classified(
            "admin_proxy",
            "system_manage",
            "purge_request_bodies_task",
            "admin:system",
            false,
        ))
    } else if method == http::Method::POST && normalized_path == "/api/admin/system/purge/stats" {
        Some(classified(
            "admin_proxy",
            "system_manage",
            "purge_stats",
            "admin:system",
            false,
        ))
    } else if method == http::Method::PUT && normalized_path == "/api/admin/system/settings" {
        Some(classified(
            "admin_proxy",
            "system_manage",
            "settings_set",
            "admin:system",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/system/configs" | "/api/admin/system/configs/"
        )
    {
        Some(classified(
            "admin_proxy",
            "system_manage",
            "configs_list",
            "admin:system",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path.starts_with("/api/admin/system/configs/")
        && normalized_path.matches('/').count() == 5
    {
        Some(classified(
            "admin_proxy",
            "system_manage",
            "config_get",
            "admin:system",
            false,
        ))
    } else if method == http::Method::PUT
        && normalized_path.starts_with("/api/admin/system/configs/")
        && normalized_path.matches('/').count() == 5
    {
        Some(classified(
            "admin_proxy",
            "system_manage",
            "config_set",
            "admin:system",
            false,
        ))
    } else if method == http::Method::DELETE
        && normalized_path.starts_with("/api/admin/system/configs/")
        && normalized_path.matches('/').count() == 5
    {
        Some(classified(
            "admin_proxy",
            "system_manage",
            "config_delete",
            "admin:system",
            false,
        ))
    } else if method == http::Method::GET && normalized_path == "/api/admin/system/api-formats" {
        Some(classified(
            "admin_proxy",
            "system_manage",
            "api_formats",
            "admin:system",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/system/email/templates" | "/api/admin/system/email/templates/"
        )
    {
        Some(classified(
            "admin_proxy",
            "system_manage",
            "email_templates_list",
            "admin:system",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path.starts_with("/api/admin/system/email/templates/")
        && normalized_path.matches('/').count() == 6
    {
        Some(classified(
            "admin_proxy",
            "system_manage",
            "email_template_get",
            "admin:system",
            false,
        ))
    } else if method == http::Method::PUT
        && normalized_path.starts_with("/api/admin/system/email/templates/")
        && normalized_path.matches('/').count() == 6
    {
        Some(classified(
            "admin_proxy",
            "system_manage",
            "email_template_set",
            "admin:system",
            false,
        ))
    } else if method == http::Method::POST
        && normalized_path.starts_with("/api/admin/system/email/templates/")
        && normalized_path.ends_with("/preview")
    {
        Some(classified(
            "admin_proxy",
            "system_manage",
            "email_template_preview",
            "admin:system",
            false,
        ))
    } else if method == http::Method::POST
        && normalized_path.starts_with("/api/admin/system/email/templates/")
        && normalized_path.ends_with("/reset")
    {
        Some(classified(
            "admin_proxy",
            "system_manage",
            "email_template_reset",
            "admin:system",
            false,
        ))
    } else {
        None
    }
}
