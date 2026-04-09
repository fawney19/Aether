use super::super::{
    build_admin_billing_bad_request_response, build_admin_billing_not_found_response,
    build_admin_billing_read_only_response,
};
use super::support::{
    parse_admin_billing_preset_apply_request, resolve_admin_billing_preset_collectors,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::attach_admin_audit_response;
use crate::GatewayError;
use axum::{
    body::{Body, Bytes},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(super) async fn build_admin_apply_billing_preset_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Response<Body>, GatewayError> {
    let (preset, mode) = match parse_admin_billing_preset_apply_request(request_body) {
        Ok(value) => value,
        Err(response) => return Ok(response),
    };
    let Some((resolved_preset, collectors)) = resolve_admin_billing_preset_collectors(&preset)
    else {
        let payload = json!({
            "ok": false,
            "preset": preset,
            "mode": mode,
            "created": 0,
            "updated": 0,
            "skipped": 0,
            "errors": ["Unknown preset: available presets are aether-core"],
        });
        return Ok(Json(payload).into_response());
    };

    match state
        .apply_admin_billing_preset(resolved_preset, &mode, &collectors)
        .await?
    {
        crate::LocalMutationOutcome::Applied(result) => {
            let response = Json(json!({
                "ok": result.errors.is_empty(),
                "preset": result.preset,
                "mode": result.mode,
                "created": result.created,
                "updated": result.updated,
                "skipped": result.skipped,
                "errors": result.errors,
            }))
            .into_response();
            Ok(attach_admin_audit_response(
                response,
                "admin_billing_preset_applied",
                "apply_billing_preset",
                "billing_preset",
                resolved_preset,
            ))
        }
        crate::LocalMutationOutcome::Unavailable => Ok(build_admin_billing_read_only_response(
            "当前为只读模式，无法应用计费预设",
        )),
        crate::LocalMutationOutcome::Invalid(detail) => {
            Ok(build_admin_billing_bad_request_response(detail))
        }
        crate::LocalMutationOutcome::NotFound => Ok(build_admin_billing_not_found_response(
            "Billing preset not found",
        )),
    }
}
