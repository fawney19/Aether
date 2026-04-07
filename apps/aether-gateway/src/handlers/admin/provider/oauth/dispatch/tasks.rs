use super::super::refresh::build_internal_control_error_response;
use super::super::state::read_provider_oauth_batch_task_payload;
use crate::control::GatewayPublicRequestContext;
use crate::handlers::admin::provider::shared::paths::admin_provider_oauth_batch_import_task_path;
use crate::handlers::admin::shared::attach_admin_audit_response;
use crate::{AppState, GatewayError};
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};

pub(super) async fn handle_admin_provider_oauth_batch_import_task_status(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
) -> Result<Response<Body>, GatewayError> {
    let Some((provider_id, task_id)) =
        admin_provider_oauth_batch_import_task_path(&request_context.request_path)
    else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::NOT_FOUND,
            "批量导入任务不存在",
        ));
    };
    let payload = match read_provider_oauth_batch_task_payload(state, &provider_id, &task_id).await
    {
        Ok(Some(payload)) => payload,
        Ok(None) => {
            return Ok(build_internal_control_error_response(
                http::StatusCode::NOT_FOUND,
                "批量导入任务不存在或已过期",
            ));
        }
        Err(_) => {
            return Ok(build_internal_control_error_response(
                http::StatusCode::SERVICE_UNAVAILABLE,
                "provider oauth batch task redis unavailable",
            ));
        }
    };
    let status = payload
        .get("status")
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned)
        .unwrap_or_default();
    let response = Json(payload).into_response();
    Ok(match status.as_str() {
        "completed" => attach_admin_audit_response(
            response,
            "admin_provider_oauth_batch_task_completed_viewed",
            "view_provider_oauth_batch_task_terminal_state",
            "provider_oauth_batch_task",
            &format!("{provider_id}:{task_id}"),
        ),
        "failed" => attach_admin_audit_response(
            response,
            "admin_provider_oauth_batch_task_failed_viewed",
            "view_provider_oauth_batch_task_terminal_state",
            "provider_oauth_batch_task",
            &format!("{provider_id}:{task_id}"),
        ),
        _ => response,
    })
}
