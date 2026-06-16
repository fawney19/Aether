use axum::http;

use super::{classified, ClassifiedRoute};

pub(super) fn classify_admin_global_models_route(
    method: &http::Method,
    normalized_path: &str,
) -> Option<ClassifiedRoute> {
    if method == http::Method::POST && normalized_path == "/api/admin/models/global/batch-delete" {
        Some(classified(
            "admin_proxy",
            "global_models_manage",
            "batch_delete_global_models",
            "admin:models",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/models/global" | "/api/admin/models/global/"
        )
    {
        Some(classified(
            "admin_proxy",
            "global_models_manage",
            "list_global_models",
            "admin:models",
            false,
        ))
    } else if method == http::Method::POST
        && matches!(
            normalized_path,
            "/api/admin/models/global" | "/api/admin/models/global/"
        )
    {
        Some(classified(
            "admin_proxy",
            "global_models_manage",
            "create_global_model",
            "admin:models",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/models/global/codex-adapter-compatibility"
                | "/api/admin/models/global/codex-adapter-compatibility/"
        )
    {
        Some(classified(
            "admin_proxy",
            "global_models_manage",
            "codex_adapter_compatibility",
            "admin:models",
            false,
        ))
    } else if method == http::Method::POST
        && normalized_path.starts_with("/api/admin/models/global/")
        && normalized_path.ends_with("/assign-to-providers")
    {
        Some(classified(
            "admin_proxy",
            "global_models_manage",
            "assign_to_providers",
            "admin:models",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path.starts_with("/api/admin/models/global/")
        && normalized_path.ends_with("/providers")
    {
        Some(classified(
            "admin_proxy",
            "global_models_manage",
            "global_model_providers",
            "admin:models",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path.starts_with("/api/admin/models/global/")
        && normalized_path.ends_with("/routing")
    {
        Some(classified(
            "admin_proxy",
            "global_models_manage",
            "routing_preview",
            "admin:models",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path.starts_with("/api/admin/models/global/")
        && normalized_path.matches('/').count() == 5
    {
        Some(classified(
            "admin_proxy",
            "global_models_manage",
            "get_global_model",
            "admin:models",
            false,
        ))
    } else if method == http::Method::PATCH
        && normalized_path.starts_with("/api/admin/models/global/")
        && normalized_path.matches('/').count() == 5
    {
        Some(classified(
            "admin_proxy",
            "global_models_manage",
            "update_global_model",
            "admin:models",
            false,
        ))
    } else if method == http::Method::DELETE
        && normalized_path.starts_with("/api/admin/models/global/")
        && normalized_path.matches('/').count() == 5
    {
        Some(classified(
            "admin_proxy",
            "global_models_manage",
            "delete_global_model",
            "admin:models",
            false,
        ))
    } else {
        None
    }
}
