use super::super::config::{
    build_admin_provider_ops_config_payload, build_admin_provider_ops_status_payload,
};
use crate::handlers::admin::request::AdminAppState;
use crate::handlers::admin::shared::attach_admin_audit_response;
use crate::GatewayError;
use axum::{
    body::Body,
    response::{IntoResponse, Response},
    Json,
};

pub(super) async fn handle_admin_provider_ops_read(
    state: &AdminAppState<'_>,
    provider_id: &str,
    route_kind: &str,
) -> Result<Response<Body>, GatewayError> {
    let provider_ids = [provider_id.to_string()];
    let providers = state
        .read_provider_catalog_providers_by_ids(&provider_ids)
        .await?;
    let provider = providers.first();
    let endpoints = if route_kind == "get_provider_config" && provider.is_some() {
        state
            .list_provider_catalog_endpoints_by_provider_ids(&provider_ids)
            .await?
    } else {
        Vec::new()
    };

    let payload = if route_kind == "get_provider_status" {
        build_admin_provider_ops_status_payload(provider_id, provider)
    } else {
        build_admin_provider_ops_config_payload(state, provider_id, provider, &endpoints)
    };

    let response = Json(payload).into_response();
    let response = if route_kind == "get_provider_config" {
        attach_admin_audit_response(
            response,
            "admin_provider_ops_config_viewed",
            "view_provider_ops_config",
            "provider",
            provider_id,
        )
    } else if route_kind == "get_provider_status" {
        attach_admin_audit_response(
            response,
            "admin_provider_ops_status_viewed",
            "view_provider_ops_status",
            "provider",
            provider_id,
        )
    } else {
        response
    };

    Ok(response)
}
