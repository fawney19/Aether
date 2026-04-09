use super::super::super::{
    build_admin_users_bad_request_response, build_admin_users_read_only_response,
};
use super::super::helpers::attach_audit_response;
use super::super::paths::admin_user_api_key_parts;

use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(crate) async fn build_admin_delete_user_api_key_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    if !state.has_auth_api_key_writer() {
        return Ok(build_admin_users_read_only_response(
            "当前为只读模式，无法删除用户 API Key",
        ));
    }

    let Some((user_id, api_key_id)) = admin_user_api_key_parts(request_context.path()) else {
        return Ok(build_admin_users_bad_request_response(
            "缺少 user_id 或 key_id",
        ));
    };

    match state.delete_user_api_key(&user_id, &api_key_id).await? {
        true => Ok(attach_audit_response(
            Json(json!({ "message": "API Key已删除" })).into_response(),
            "admin_user_api_key_deleted",
            "delete_user_api_key",
            "user_api_key",
            &api_key_id,
        )),
        false => Ok((
            http::StatusCode::NOT_FOUND,
            Json(json!({ "detail": "API Key不存在或不属于该用户" })),
        )
            .into_response()),
    }
}
