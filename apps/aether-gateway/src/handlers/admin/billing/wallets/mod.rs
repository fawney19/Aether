use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use axum::{body::Body, response::Response};

mod mutations;
mod reads;
mod routes;
mod shared;

pub(crate) async fn maybe_build_local_admin_wallets_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&axum::body::Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    routes::maybe_build_local_admin_wallets_routes_response(state, request_context, request_body)
        .await
}
