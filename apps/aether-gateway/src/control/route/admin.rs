use axum::http;

use super::{classified, ClassifiedRoute};

#[path = "admin/basic_families.rs"]
mod basic_families;
#[path = "admin/endpoints_families.rs"]
mod endpoints_families;
#[path = "admin/model_provider_families.rs"]
mod model_provider_families;
#[path = "admin/observability_families.rs"]
mod observability_families;
#[path = "admin/operations_families.rs"]
mod operations_families;
#[path = "admin/provider_ops_routes.rs"]
mod provider_ops_routes;
#[path = "admin/system_families.rs"]
mod system_families;

use basic_families::classify_admin_basic_family_route;
use endpoints_families::classify_admin_endpoints_family_route;
use model_provider_families::classify_admin_model_provider_family_route;
use observability_families::classify_admin_observability_family_route;
use operations_families::classify_admin_operations_family_route;
use provider_ops_routes::classify_admin_provider_ops_routes;
use system_families::classify_admin_system_family_route;

pub(super) fn classify_admin_route(
    method: &http::Method,
    normalized_path: &str,
) -> Option<ClassifiedRoute> {
    let normalized_path_no_trailing = normalized_path.trim_end_matches('/');
    let normalized_path_no_trailing = if normalized_path_no_trailing.is_empty() {
        "/"
    } else {
        normalized_path_no_trailing
    };

    if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/providers" | "/api/admin/providers/"
        )
    {
        Some(classified(
            "admin_proxy",
            "providers_manage",
            "list_providers",
            "admin:providers",
            false,
        ))
    } else if let Some(route) =
        classify_admin_basic_family_route(method, normalized_path, normalized_path_no_trailing)
    {
        Some(route)
    } else if let Some(route) = classify_admin_observability_family_route(
        method,
        normalized_path,
        normalized_path_no_trailing,
    ) {
        Some(route)
    } else if let Some(route) =
        classify_admin_operations_family_route(method, normalized_path, normalized_path_no_trailing)
    {
        Some(route)
    } else if let Some(route) =
        classify_admin_system_family_route(method, normalized_path, normalized_path_no_trailing)
    {
        Some(route)
    } else if let Some(route) = classify_admin_provider_ops_routes(method, normalized_path) {
        Some(route)
    } else if let Some(route) = classify_admin_model_provider_family_route(method, normalized_path)
    {
        Some(route)
    } else {
        classify_admin_endpoints_family_route(method, normalized_path)
    }
}
