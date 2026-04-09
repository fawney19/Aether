use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use axum::{body::Body, response::Response};

mod builders;
mod routes;

pub(crate) async fn maybe_build_local_admin_video_tasks_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Option<Response<Body>>, GatewayError> {
    routes::maybe_build_local_admin_video_tasks_response(state, request_context).await
}
