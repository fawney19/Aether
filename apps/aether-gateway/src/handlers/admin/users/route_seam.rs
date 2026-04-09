use super::routes;
use crate::handlers::admin::request::{AdminRouteRequest, AdminRouteResult};

pub(crate) async fn maybe_build_local_admin_users_response(
    request: AdminRouteRequest<'_>,
) -> AdminRouteResult {
    routes::maybe_build_local_admin_users_routes_response(
        &request.state(),
        &request.request_context(),
        request.request_body(),
    )
    .await
}
