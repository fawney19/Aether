mod reads;
mod support;
mod writes;

use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use axum::{
    body::{Body, Bytes},
    http,
    response::Response,
};

pub(super) async fn maybe_build_local_admin_billing_collectors_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.decision() else {
        return Ok(None);
    };
    let path = request_context.path();

    match decision.route_kind.as_deref() {
        Some("list_collectors")
            if request_context.method() == http::Method::GET
                && matches!(
                    path,
                    "/api/admin/billing/collectors" | "/api/admin/billing/collectors/"
                ) =>
        {
            Ok(Some(
                reads::build_admin_list_dimension_collectors_response(state, request_context)
                    .await?,
            ))
        }
        Some("get_collector")
            if request_context.method() == http::Method::GET
                && path.starts_with("/api/admin/billing/collectors/") =>
        {
            Ok(Some(
                reads::build_admin_get_dimension_collector_response(state, request_context).await?,
            ))
        }
        Some("create_collector")
            if request_context.method() == http::Method::POST
                && matches!(
                    path,
                    "/api/admin/billing/collectors" | "/api/admin/billing/collectors/"
                ) =>
        {
            Ok(Some(
                writes::build_admin_create_dimension_collector_response(state, request_body)
                    .await?,
            ))
        }
        Some("update_collector")
            if request_context.method() == http::Method::PUT
                && path.starts_with("/api/admin/billing/collectors/") =>
        {
            Ok(Some(
                writes::build_admin_update_dimension_collector_response(
                    state,
                    request_context,
                    request_body,
                )
                .await?,
            ))
        }
        _ => Ok(None),
    }
}
