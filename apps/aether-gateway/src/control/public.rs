use axum::http::Uri;

use crate::{AppState, GatewayError};

use super::{resolve_control_route, GatewayControlDecision};

pub(crate) type GatewayPublicRequestContext =
    aether_gateway_control::PublicRequestContext<GatewayControlDecision>;

pub(crate) async fn resolve_public_request_context(
    state: &AppState,
    method: &http::Method,
    uri: &Uri,
    headers: &http::HeaderMap,
    trace_id: &str,
) -> Result<GatewayPublicRequestContext, GatewayError> {
    let control_decision = resolve_control_route(state, method, uri, headers, trace_id).await?;
    Ok(GatewayPublicRequestContext::from_request_parts(
        trace_id,
        method,
        uri,
        headers,
        control_decision,
    ))
}
