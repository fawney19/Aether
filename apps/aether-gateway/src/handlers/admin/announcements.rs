use crate::handlers::admin::request::{AdminRouteRequest, AdminRouteResult};
use crate::handlers::public;

pub(crate) async fn maybe_build_local_admin_announcements_response(
    request: AdminRouteRequest<'_>,
) -> AdminRouteResult {
    public::maybe_build_local_admin_announcements_response(
        request.state().app(),
        &request.request_context(),
        request.request_body(),
    )
    .await
}
