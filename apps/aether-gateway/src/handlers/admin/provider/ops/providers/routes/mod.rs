mod actions;
mod batch;
mod config;
mod connect;
mod read;
mod verify;

use crate::handlers::admin::provider::shared::paths::{
    admin_provider_id_for_provider_ops_balance, admin_provider_id_for_provider_ops_checkin,
    admin_provider_id_for_provider_ops_config, admin_provider_id_for_provider_ops_connect,
    admin_provider_id_for_provider_ops_disconnect, admin_provider_id_for_provider_ops_status,
    admin_provider_id_for_provider_ops_verify, admin_provider_ops_action_route_parts,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use axum::{
    body::{Body, Bytes},
    response::Response,
};

pub(crate) async fn maybe_build_local_admin_provider_ops_providers_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.decision() else {
        return Ok(None);
    };
    if decision.route_family.as_deref() != Some("provider_ops_manage") {
        return Ok(None);
    }

    let route_kind = decision.route_kind.as_deref().unwrap_or_default();
    if !state.has_provider_catalog_data_reader() && route_kind != "disconnect_provider" {
        return Ok(None);
    }

    if route_kind == "batch_balance" {
        return batch::handle_admin_provider_ops_batch_balance(state, request_body)
            .await
            .map(Some);
    }

    let action_route = (route_kind == "execute_provider_action")
        .then(|| admin_provider_ops_action_route_parts(request_context.path()))
        .flatten();
    let Some(provider_id) =
        provider_id_for_route(request_context.path(), route_kind, action_route.as_ref())
    else {
        return Ok(None);
    };

    if decision.route_kind.as_deref() != Some(route_kind) {
        return Ok(None);
    }

    let response = match route_kind {
        "save_provider_config" => {
            let Some(response) =
                config::handle_admin_provider_ops_save_config(state, &provider_id, request_body)
                    .await?
            else {
                return Ok(None);
            };
            response
        }
        "verify_provider" => {
            verify::handle_admin_provider_ops_verify(state, &provider_id, request_body).await?
        }
        "connect_provider" => {
            connect::handle_admin_provider_ops_connect(state, &provider_id, request_body).await?
        }
        "get_provider_balance"
        | "refresh_provider_balance"
        | "provider_checkin"
        | "execute_provider_action" => {
            let Some(response) = actions::handle_admin_provider_ops_action(
                state,
                &provider_id,
                route_kind,
                action_route.as_ref(),
                request_body,
            )
            .await?
            else {
                return Ok(None);
            };
            response
        }
        "delete_provider_config" => {
            let Some(response) =
                config::handle_admin_provider_ops_delete_config(state, &provider_id).await?
            else {
                return Ok(None);
            };
            response
        }
        "disconnect_provider" => connect::handle_admin_provider_ops_disconnect(),
        "get_provider_status" | "get_provider_config" => {
            read::handle_admin_provider_ops_read(state, &provider_id, route_kind).await?
        }
        _ => return Ok(None),
    };

    Ok(Some(response))
}

fn provider_id_for_route(
    request_path: &str,
    route_kind: &str,
    action_route: Option<&(String, String)>,
) -> Option<String> {
    if !matches!(
        route_kind,
        "get_provider_status"
            | "get_provider_config"
            | "save_provider_config"
            | "delete_provider_config"
            | "verify_provider"
            | "connect_provider"
            | "disconnect_provider"
            | "get_provider_balance"
            | "refresh_provider_balance"
            | "provider_checkin"
            | "execute_provider_action"
    ) {
        return None;
    }

    admin_provider_id_for_provider_ops_config(request_path)
        .or_else(|| admin_provider_id_for_provider_ops_status(request_path))
        .or_else(|| admin_provider_id_for_provider_ops_verify(request_path))
        .or_else(|| admin_provider_id_for_provider_ops_connect(request_path))
        .or_else(|| admin_provider_id_for_provider_ops_balance(request_path))
        .or_else(|| admin_provider_id_for_provider_ops_checkin(request_path))
        .or_else(|| action_route.map(|(provider_id, _)| provider_id.clone()))
        .or_else(|| admin_provider_id_for_provider_ops_disconnect(request_path))
}
