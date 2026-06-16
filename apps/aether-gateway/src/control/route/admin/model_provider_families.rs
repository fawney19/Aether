use axum::http;

use super::{classified, ClassifiedRoute};

#[path = "model_provider_global_models.rs"]
mod model_provider_global_models;

use model_provider_global_models::classify_admin_global_models_route;

pub(super) fn classify_admin_model_provider_family_route(
    method: &http::Method,
    normalized_path: &str,
) -> Option<ClassifiedRoute> {
    if method == http::Method::GET && normalized_path == "/api/admin/models/catalog" {
        Some(classified(
            "admin_proxy",
            "model_catalog_manage",
            "catalog",
            "admin:models",
            false,
        ))
    } else if method == http::Method::GET && normalized_path == "/api/admin/models/external" {
        Some(classified(
            "admin_proxy",
            "model_external_manage",
            "external",
            "admin:models",
            false,
        ))
    } else if method == http::Method::DELETE
        && normalized_path == "/api/admin/models/external/cache"
    {
        Some(classified(
            "admin_proxy",
            "model_external_manage",
            "clear_external_cache",
            "admin:models",
            false,
        ))
    } else if method == http::Method::POST
        && matches!(
            normalized_path,
            "/api/admin/providers" | "/api/admin/providers/"
        )
    {
        Some(classified(
            "admin_proxy",
            "providers_manage",
            "create_provider",
            "admin:providers",
            false,
        ))
    } else if method == http::Method::PATCH
        && normalized_path.starts_with("/api/admin/providers/")
        && normalized_path.matches('/').count() == 4
    {
        Some(classified(
            "admin_proxy",
            "providers_manage",
            "update_provider",
            "admin:providers",
            false,
        ))
    } else if method == http::Method::DELETE
        && normalized_path.starts_with("/api/admin/providers/")
        && normalized_path.matches('/').count() == 4
    {
        Some(classified(
            "admin_proxy",
            "providers_manage",
            "delete_provider",
            "admin:providers",
            false,
        ))
    } else if method == http::Method::GET && normalized_path == "/api/admin/providers/summary" {
        Some(classified(
            "admin_proxy",
            "providers_manage",
            "summary_list",
            "admin:providers",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path.starts_with("/api/admin/providers/")
        && normalized_path.ends_with("/summary")
    {
        Some(classified(
            "admin_proxy",
            "providers_manage",
            "provider_summary",
            "admin:providers",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path.starts_with("/api/admin/providers/")
        && normalized_path.ends_with("/health-monitor")
    {
        Some(classified(
            "admin_proxy",
            "providers_manage",
            "health_monitor",
            "admin:providers",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path.starts_with("/api/admin/providers/")
        && normalized_path.ends_with("/mapping-preview")
    {
        Some(classified(
            "admin_proxy",
            "providers_manage",
            "mapping_preview",
            "admin:providers",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path.starts_with("/api/admin/providers/")
        && normalized_path.contains("/delete-task/")
    {
        Some(classified(
            "admin_proxy",
            "providers_manage",
            "delete_provider_task",
            "admin:providers",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path.starts_with("/api/admin/providers/")
        && normalized_path.ends_with("/pool-status")
    {
        Some(classified(
            "admin_proxy",
            "providers_manage",
            "pool_status",
            "admin:providers",
            false,
        ))
    } else if method == http::Method::POST
        && normalized_path.starts_with("/api/admin/providers/")
        && normalized_path.contains("/pool/clear-cooldown/")
    {
        Some(classified(
            "admin_proxy",
            "providers_manage",
            "clear_pool_cooldown",
            "admin:providers",
            false,
        ))
    } else if method == http::Method::POST
        && normalized_path.starts_with("/api/admin/providers/")
        && normalized_path.contains("/pool/reset-cost/")
    {
        Some(classified(
            "admin_proxy",
            "providers_manage",
            "reset_pool_cost",
            "admin:providers",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path.starts_with("/api/admin/providers/")
        && normalized_path.ends_with("/models")
        && normalized_path.matches('/').count() == 5
    {
        Some(classified(
            "admin_proxy",
            "provider_models_manage",
            "list_provider_models",
            "admin:providers",
            false,
        ))
    } else if method == http::Method::POST
        && normalized_path.starts_with("/api/admin/providers/")
        && normalized_path.ends_with("/models")
        && normalized_path.matches('/').count() == 5
    {
        Some(classified(
            "admin_proxy",
            "provider_models_manage",
            "create_provider_model",
            "admin:providers",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path.starts_with("/api/admin/providers/")
        && normalized_path.contains("/models/")
        && normalized_path.matches('/').count() == 6
    {
        Some(classified(
            "admin_proxy",
            "provider_models_manage",
            "get_provider_model",
            "admin:providers",
            false,
        ))
    } else if method == http::Method::PATCH
        && normalized_path.starts_with("/api/admin/providers/")
        && normalized_path.contains("/models/")
        && normalized_path.matches('/').count() == 6
    {
        Some(classified(
            "admin_proxy",
            "provider_models_manage",
            "update_provider_model",
            "admin:providers",
            false,
        ))
    } else if method == http::Method::DELETE
        && normalized_path.starts_with("/api/admin/providers/")
        && normalized_path.contains("/models/")
        && normalized_path.matches('/').count() == 6
    {
        Some(classified(
            "admin_proxy",
            "provider_models_manage",
            "delete_provider_model",
            "admin:providers",
            false,
        ))
    } else if method == http::Method::POST
        && normalized_path.starts_with("/api/admin/providers/")
        && normalized_path.ends_with("/models/batch")
    {
        Some(classified(
            "admin_proxy",
            "provider_models_manage",
            "batch_create_provider_models",
            "admin:providers",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path.starts_with("/api/admin/providers/")
        && normalized_path.ends_with("/available-source-models")
    {
        Some(classified(
            "admin_proxy",
            "provider_models_manage",
            "available_source_models",
            "admin:providers",
            false,
        ))
    } else if method == http::Method::POST
        && normalized_path.starts_with("/api/admin/providers/")
        && normalized_path.ends_with("/assign-global-models")
    {
        Some(classified(
            "admin_proxy",
            "provider_models_manage",
            "assign_global_models",
            "admin:providers",
            false,
        ))
    } else if method == http::Method::POST
        && normalized_path.starts_with("/api/admin/providers/")
        && normalized_path.ends_with("/import-from-upstream")
    {
        Some(classified(
            "admin_proxy",
            "provider_models_manage",
            "import_from_upstream",
            "admin:providers",
            false,
        ))
    } else {
        classify_admin_global_models_route(method, normalized_path)
    }
}
