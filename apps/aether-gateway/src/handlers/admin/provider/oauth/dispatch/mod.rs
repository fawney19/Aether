use super::state::{
    build_admin_provider_oauth_backend_unavailable_response,
    build_admin_provider_oauth_supported_types_payload,
};
use crate::handlers::admin::provider::shared::paths::{
    admin_provider_oauth_batch_import_provider_id,
    admin_provider_oauth_batch_import_task_provider_id, admin_provider_oauth_complete_key_id,
    admin_provider_oauth_complete_provider_id, admin_provider_oauth_device_authorize_provider_id,
    admin_provider_oauth_import_provider_id, admin_provider_oauth_refresh_key_id,
    admin_provider_oauth_start_key_id, admin_provider_oauth_start_provider_id,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};

mod batch;
mod complete;
mod device;
mod helpers;
mod import;
mod kiro;
mod refresh;
mod start;
mod tasks;
mod token_import;

pub(crate) async fn maybe_build_local_admin_provider_oauth_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.decision() else {
        return Ok(None);
    };
    if decision.route_family.as_deref() != Some("provider_oauth_manage") {
        return Ok(None);
    }

    let route_kind = decision.route_kind.as_deref();
    let method = &request_context.method();

    if route_kind == Some("supported_types")
        && *method == http::Method::GET
        && request_context.path() == "/api/admin/provider-oauth/supported-types"
    {
        return Ok(Some(
            Json(build_admin_provider_oauth_supported_types_payload()).into_response(),
        ));
    }

    if route_kind == Some("start_key_oauth") && *method == http::Method::POST {
        let response = start::handle_admin_provider_oauth_start_key(state, request_context).await?;
        return Ok(Some(helpers::attach_admin_provider_oauth_audit_response(
            response,
            "admin_provider_oauth_authorization_started",
            "start_provider_oauth_for_key",
            "provider_key",
            admin_provider_oauth_start_key_id(request_context.path()),
        )));
    }

    if route_kind == Some("start_provider_oauth") && *method == http::Method::POST {
        let response =
            start::handle_admin_provider_oauth_start_provider(state, request_context).await?;
        return Ok(Some(helpers::attach_admin_provider_oauth_audit_response(
            response,
            "admin_provider_oauth_authorization_started",
            "start_provider_oauth_for_provider",
            "provider",
            admin_provider_oauth_start_provider_id(request_context.path()),
        )));
    }

    if route_kind == Some("get_batch_import_task_status") && *method == http::Method::GET {
        return Ok(Some(
            tasks::handle_admin_provider_oauth_batch_import_task_status(state, request_context)
                .await?,
        ));
    }

    if route_kind == Some("complete_key_oauth") && *method == http::Method::POST {
        let response = complete::handle_admin_provider_oauth_complete_key(
            state,
            request_context,
            request_body,
        )
        .await?;
        return Ok(Some(helpers::attach_admin_provider_oauth_audit_response(
            response,
            "admin_provider_oauth_completed",
            "complete_provider_oauth_for_key",
            "provider_key",
            admin_provider_oauth_complete_key_id(request_context.path()),
        )));
    }

    if route_kind == Some("refresh_key_oauth") && *method == http::Method::POST {
        let response =
            refresh::handle_admin_provider_oauth_refresh_key(state, request_context).await?;
        return Ok(Some(helpers::attach_admin_provider_oauth_audit_response(
            response,
            "admin_provider_oauth_refreshed",
            "refresh_provider_oauth_for_key",
            "provider_key",
            admin_provider_oauth_refresh_key_id(request_context.path()),
        )));
    }

    if route_kind == Some("complete_provider_oauth") && *method == http::Method::POST {
        let response = complete::handle_admin_provider_oauth_complete_provider(
            state,
            request_context,
            request_body,
        )
        .await?;
        return Ok(Some(helpers::attach_admin_provider_oauth_audit_response(
            response,
            "admin_provider_oauth_completed",
            "complete_provider_oauth_for_provider",
            "provider",
            admin_provider_oauth_complete_provider_id(request_context.path()),
        )));
    }

    if route_kind == Some("import_refresh_token") && *method == http::Method::POST {
        let response = import::handle_admin_provider_oauth_import_refresh_token(
            state,
            request_context,
            request_body,
        )
        .await?;
        return Ok(Some(helpers::attach_admin_provider_oauth_audit_response(
            response,
            "admin_provider_oauth_refresh_token_imported",
            "import_provider_oauth_refresh_token",
            "provider",
            admin_provider_oauth_import_provider_id(request_context.path()),
        )));
    }

    if route_kind == Some("batch_import_oauth") && *method == http::Method::POST {
        let response =
            batch::handle_admin_provider_oauth_batch_import(state, request_context, request_body)
                .await?;
        return Ok(Some(helpers::attach_admin_provider_oauth_audit_response(
            response,
            "admin_provider_oauth_batch_import_completed",
            "batch_import_provider_oauth",
            "provider",
            admin_provider_oauth_batch_import_provider_id(request_context.path()),
        )));
    }

    if route_kind == Some("start_batch_import_oauth_task") && *method == http::Method::POST {
        let response = batch::handle_admin_provider_oauth_start_batch_import_task(
            state,
            request_context,
            request_body,
        )
        .await?;
        return Ok(Some(helpers::attach_admin_provider_oauth_audit_response(
            response,
            "admin_provider_oauth_batch_import_started",
            "start_provider_oauth_batch_import",
            "provider",
            admin_provider_oauth_batch_import_task_provider_id(request_context.path()),
        )));
    }

    if route_kind == Some("device_authorize") && *method == http::Method::POST {
        let response = device::handle_admin_provider_oauth_device_authorize(
            state,
            request_context,
            request_body,
        )
        .await?;
        return Ok(Some(helpers::attach_admin_provider_oauth_audit_response(
            response,
            "admin_provider_oauth_device_authorization_started",
            "start_provider_oauth_device_authorization",
            "provider",
            admin_provider_oauth_device_authorize_provider_id(request_context.path()),
        )));
    }

    if route_kind == Some("device_poll") && *method == http::Method::POST {
        return Ok(Some(
            device::handle_admin_provider_oauth_device_poll(state, request_context, request_body)
                .await?,
        ));
    }

    if matches!(
        route_kind,
        Some("refresh_key_oauth" | "import_refresh_token")
    ) {
        return Ok(Some(
            build_admin_provider_oauth_backend_unavailable_response(),
        ));
    }

    Ok(None)
}

// Dispatch-specific helpers have moved to helpers.rs, so nothing remains here.
