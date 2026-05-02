use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use axum::{
    body::{Body, Bytes},
    http,
    response::Response,
};
#[path = "batch_routes/action.rs"]
mod batch_action;
#[path = "batch_routes/cleanup.rs"]
mod batch_cleanup;
#[path = "batch_routes/import.rs"]
mod batch_import;
#[path = "batch_routes/shared.rs"]
mod batch_shared;
#[path = "batch_routes/task_status.rs"]
mod batch_task_status;
pub(crate) mod payloads;
#[path = "read_routes/keys.rs"]
mod read_keys;
#[path = "read_routes/overview.rs"]
mod read_overview;
#[path = "read_routes/presets.rs"]
mod read_presets;
#[path = "read_routes/resolve_selection.rs"]
mod read_resolve_selection;
pub(crate) mod selection;
mod support;

pub(crate) use self::batch_shared::{
    admin_pool_batch_delete_task_parts, admin_pool_key_proxy_value,
    admin_pool_resolved_api_formats, attach_admin_pool_batch_delete_task_terminal_audit,
    build_admin_pool_batch_delete_task_payload, AdminPoolBatchActionRequest,
    AdminPoolBatchImportRequest,
};
pub(crate) use self::support::{
    admin_pool_provider_id_from_path, parse_admin_pool_key_sort, parse_admin_pool_page,
    parse_admin_pool_page_size, parse_admin_pool_quick_selectors, parse_admin_pool_search,
    parse_admin_pool_status_filter, AdminPoolKeySort, AdminPoolKeySortDirection,
    AdminPoolKeySortField, AdminPoolResolveSelectionRequest,
    ADMIN_POOL_BANNED_KEY_CLEANUP_EMPTY_MESSAGE,
    ADMIN_POOL_PROVIDER_CATALOG_READER_UNAVAILABLE_DETAIL,
    ADMIN_POOL_PROVIDER_CATALOG_WRITER_UNAVAILABLE_DETAIL,
};
use self::support::{build_admin_pool_error_response, is_admin_pool_route};
pub(crate) use self::{payloads as pool_payloads, selection as pool_selection};
pub(crate) use crate::handlers::admin::provider::pool::config::admin_provider_pool_config;
pub(crate) use crate::handlers::admin::provider::pool::runtime::{
    read_admin_provider_pool_cooldown_counts, read_admin_provider_pool_cooldown_key_ids,
    read_admin_provider_pool_runtime_state,
};
pub(crate) use crate::handlers::admin::provider::shared::support::AdminProviderPoolRuntimeState;
pub(crate) use crate::handlers::admin::shared::attach_admin_audit_response;
pub(crate) use aether_data_contracts::repository::provider_catalog::{
    ProviderCatalogKeyListOrder, ProviderCatalogKeyListQuery,
};

pub(crate) async fn maybe_build_local_admin_pool_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.decision() else {
        return Ok(None);
    };

    if decision.route_family.as_deref() != Some("pool_manage") {
        return Ok(None);
    }

    if !is_admin_pool_route(request_context) {
        return Ok(None);
    }

    match request_context
        .decision()
        .and_then(|decision| decision.route_kind.as_deref())
    {
        Some("overview")
            if request_context.method() == http::Method::GET
                && matches!(
                    request_context.path().trim_end_matches('/'),
                    "/api/admin/pool/overview"
                ) =>
        {
            return Ok(Some(
                read_overview::build_admin_pool_overview_response(state).await?,
            ));
        }
        Some("scheduling_presets")
            if request_context.method() == http::Method::GET
                && matches!(
                    request_context.path().trim_end_matches('/'),
                    "/api/admin/pool/scheduling-presets"
                ) =>
        {
            return Ok(Some(
                read_presets::build_admin_pool_scheduling_presets_response(),
            ));
        }
        Some("list_keys") => {
            return Ok(Some(
                read_keys::build_admin_pool_list_keys_response(state, request_context).await?,
            ));
        }
        Some("resolve_selection") => {
            return Ok(Some(
                read_resolve_selection::build_admin_pool_resolve_selection_response(
                    state,
                    request_context,
                    request_body,
                )
                .await?,
            ));
        }
        Some("cleanup_banned_keys") if request_context.method() == http::Method::POST => {
            let Some(provider_id) = admin_pool_provider_id_from_path(request_context.path()) else {
                return Ok(Some(build_admin_pool_error_response(
                    http::StatusCode::BAD_REQUEST,
                    "provider_id 无效",
                )));
            };
            return Ok(Some(
                batch_cleanup::build_admin_pool_cleanup_banned_keys_response(state, provider_id)
                    .await?,
            ));
        }
        Some("batch_import_keys") => {
            return Ok(Some(
                batch_import::build_admin_pool_batch_import_response(
                    state,
                    request_context,
                    request_body,
                )
                .await?,
            ));
        }
        Some("batch_action_keys") => {
            return Ok(Some(
                batch_action::build_admin_pool_batch_action_response(
                    state,
                    request_context,
                    request_body,
                )
                .await?,
            ));
        }
        Some("batch_delete_task_status") => {
            return Ok(Some(
                batch_task_status::build_admin_pool_batch_delete_task_status_response(
                    state,
                    request_context,
                )
                .await?,
            ));
        }
        _ => {}
    }

    Ok(Some(build_admin_pool_error_response(
        http::StatusCode::NOT_FOUND,
        format!(
            "Unsupported admin pool route {} {}",
            request_context.method(),
            request_context.path()
        ),
    )))
}
