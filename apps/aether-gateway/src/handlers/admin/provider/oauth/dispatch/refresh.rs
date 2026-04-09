mod execution;
mod helpers;
mod request;
mod response;

use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use axum::{body::Body, response::Response};

pub(super) async fn handle_admin_provider_oauth_refresh_key(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let request =
        match request::parse_admin_provider_oauth_refresh_request(state, request_context).await? {
            helpers::RefreshDispatch::Continue(request) => request,
            helpers::RefreshDispatch::Respond(response) => return Ok(response),
        };

    let refreshed = match execution::execute_admin_provider_oauth_refresh(state, request).await? {
        helpers::RefreshDispatch::Continue(refreshed) => refreshed,
        helpers::RefreshDispatch::Respond(response) => return Ok(response),
    };

    Ok(response::admin_provider_oauth_refresh_success_response(
        refreshed,
    ))
}
