use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::system::shared::modules::{
    admin_module_name_from_enabled_path, admin_module_name_from_status_path,
};
use crate::GatewayError;
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(crate) async fn maybe_build_local_admin_modules_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.decision() else {
        return Ok(None);
    };
    if decision.route_family.as_deref() != Some("modules_manage") {
        return Ok(None);
    }

    if decision.route_kind.as_deref() == Some("status_list")
        && request_context.method() == http::Method::GET
        && request_context.path() == "/api/admin/modules/status"
    {
        return Ok(Some(
            Json(state.build_admin_modules_status_payload().await?).into_response(),
        ));
    }

    if decision.route_kind.as_deref() == Some("status_detail")
        && request_context.method() == http::Method::GET
        && request_context
            .path()
            .starts_with("/api/admin/modules/status/")
    {
        let Some(module_name) = admin_module_name_from_status_path(request_context.path()) else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "模块不存在" })),
                )
                    .into_response(),
            ));
        };
        return Ok(Some(
            match state
                .build_admin_module_status_payload(&module_name)
                .await?
            {
                Ok(payload) => Json(payload).into_response(),
                Err((status, payload)) => (status, Json(payload)).into_response(),
            },
        ));
    }

    if decision.route_kind.as_deref() == Some("set_enabled")
        && request_context.method() == http::Method::PUT
        && request_context
            .path()
            .starts_with("/api/admin/modules/status/")
        && request_context.path().ends_with("/enabled")
    {
        let Some(module_name) = admin_module_name_from_enabled_path(request_context.path()) else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "模块不存在" })),
                )
                    .into_response(),
            ));
        };
        let Some(request_body) = request_body else {
            return Ok(Some(
                (
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": "请求体不能为空" })),
                )
                    .into_response(),
            ));
        };
        return Ok(Some(
            match state
                .set_admin_module_enabled_payload(&module_name, request_body)
                .await?
            {
                Ok(payload) => Json(payload).into_response(),
                Err((status, payload)) => (status, Json(payload)).into_response(),
            },
        ));
    }

    Ok(None)
}
