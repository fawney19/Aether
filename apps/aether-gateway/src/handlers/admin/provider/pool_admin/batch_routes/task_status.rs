use super::{
    admin_pool_batch_delete_task_parts, attach_admin_pool_batch_delete_task_terminal_audit,
    build_admin_pool_batch_delete_task_payload, build_admin_pool_error_response,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};

pub(super) async fn build_admin_pool_batch_delete_task_status_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let Some((provider_id, task_id)) = admin_pool_batch_delete_task_parts(request_context.path())
    else {
        return Ok(build_admin_pool_error_response(
            http::StatusCode::NOT_FOUND,
            "批量删除任务不存在",
        ));
    };
    let task = match state.get_admin_pool_batch_delete_task_for_provider(&provider_id, &task_id) {
        Ok(task) => task,
        Err(response) => {
            return Ok(response);
        }
    };

    Ok(attach_admin_pool_batch_delete_task_terminal_audit(
        &provider_id,
        &task_id,
        task.status.as_str(),
        Json(build_admin_pool_batch_delete_task_payload(&task)).into_response(),
    ))
}
