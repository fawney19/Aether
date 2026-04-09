use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::query_param_value;
use crate::GatewayError;
use aether_admin::system::{
    admin_proxy_node_event_node_id_from_path, build_admin_proxy_nodes_data_unavailable_response,
    build_admin_proxy_nodes_not_found_response,
};
use axum::{body::Body, http, response::Response};

pub(crate) async fn maybe_build_local_admin_proxy_nodes_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.decision() else {
        return Ok(None);
    };

    if decision.route_family.as_deref() != Some("proxy_nodes_manage") {
        return Ok(None);
    }

    if decision.route_kind.as_deref() == Some("list_nodes")
        && request_context.method() == http::Method::GET
        && matches!(
            request_context.path(),
            "/api/admin/proxy-nodes" | "/api/admin/proxy-nodes/"
        )
    {
        let skip = query_param_value(request_context.query_string(), "skip")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0);
        let limit = query_param_value(request_context.query_string(), "limit")
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value > 0 && *value <= 1000)
            .unwrap_or(100);
        let status = query_param_value(request_context.query_string(), "status")
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty());
        return Ok(Some(
            state
                .build_admin_proxy_nodes_list_response(skip, limit, status)
                .await?,
        ));
    }

    if decision.route_kind.as_deref() == Some("list_node_events")
        && request_context.method() == http::Method::GET
    {
        let Some(node_id) = admin_proxy_node_event_node_id_from_path(request_context.path()) else {
            return Ok(Some(build_admin_proxy_nodes_not_found_response()));
        };

        let limit = query_param_value(request_context.query_string(), "limit")
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value > 0 && *value <= 200)
            .unwrap_or(50);
        return Ok(Some(
            state
                .build_admin_proxy_node_events_response(node_id, limit)
                .await?,
        ));
    }

    Ok(Some(build_admin_proxy_nodes_data_unavailable_response()))
}
