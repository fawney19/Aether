use super::super::build_admin_users_bad_request_response;
use super::support::admin_user_id_from_detail_path;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::attach_admin_audit_response;
use crate::GatewayError;
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(in super::super) async fn build_admin_delete_user_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let Some(user_id) = admin_user_id_from_detail_path(request_context.path()) else {
        return Ok(build_admin_users_bad_request_response("缺少 user_id"));
    };
    let Some(user) = state.find_user_auth_by_id(&user_id).await? else {
        return Ok((
            http::StatusCode::NOT_FOUND,
            Json(json!({ "detail": "用户不存在" })),
        )
            .into_response());
    };

    if user.role.eq_ignore_ascii_case("admin") && state.count_active_admin_users().await? <= 1 {
        return Ok((
            http::StatusCode::BAD_REQUEST,
            Json(json!({ "detail": "不能删除最后一个管理员账户" })),
        )
            .into_response());
    }
    if state.count_user_pending_refunds(&user_id).await? > 0 {
        return Ok((
            http::StatusCode::BAD_REQUEST,
            Json(json!({ "detail": "用户存在未完结退款，禁止删除" })),
        )
            .into_response());
    }
    if state.count_user_pending_payment_orders(&user_id).await? > 0 {
        return Ok((
            http::StatusCode::BAD_REQUEST,
            Json(json!({ "detail": "用户存在未完结充值订单，禁止删除" })),
        )
            .into_response());
    }

    if !state.delete_local_auth_user(&user_id).await? {
        return Ok((
            http::StatusCode::NOT_FOUND,
            Json(json!({ "detail": "用户不存在" })),
        )
            .into_response());
    }

    Ok(attach_admin_audit_response(
        Json(json!({ "message": "用户删除成功" })).into_response(),
        "admin_user_deleted",
        "delete_user",
        "user",
        &user_id,
    ))
}
