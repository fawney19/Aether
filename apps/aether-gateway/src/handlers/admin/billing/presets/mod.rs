mod apply;
mod support;

use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};

pub(super) async fn maybe_build_local_admin_billing_presets_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.decision() else {
        return Ok(None);
    };
    let path = request_context.path();

    match decision.route_kind.as_deref() {
        Some("list_presets")
            if request_context.method() == http::Method::GET
                && matches!(
                    path,
                    "/api/admin/billing/presets" | "/api/admin/billing/presets/"
                ) =>
        {
            Ok(Some(
                Json(support::build_admin_billing_presets_payload()).into_response(),
            ))
        }
        Some("apply_preset")
            if request_context.method() == http::Method::POST
                && matches!(
                    path,
                    "/api/admin/billing/presets/apply" | "/api/admin/billing/presets/apply/"
                ) =>
        {
            Ok(Some(
                apply::build_admin_apply_billing_preset_response(
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
