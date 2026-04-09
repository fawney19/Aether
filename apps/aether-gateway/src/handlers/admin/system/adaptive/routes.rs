use super::shared::admin_adaptive_key_id_from_path;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::{build_proxy_error_response, query_param_value};
use crate::GatewayError;
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

const ADMIN_ADAPTIVE_DATA_UNAVAILABLE_DETAIL: &str = "Admin adaptive data unavailable";

pub(super) async fn maybe_build_local_admin_adaptive_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.decision() else {
        return Ok(None);
    };

    if decision.route_family.as_deref() != Some("adaptive_manage") {
        return Ok(None);
    }

    if !state.has_provider_catalog_data_reader() {
        return Ok(Some(build_proxy_error_response(
            http::StatusCode::SERVICE_UNAVAILABLE,
            "data_unavailable",
            ADMIN_ADAPTIVE_DATA_UNAVAILABLE_DETAIL,
            Some(json!({
                "error": ADMIN_ADAPTIVE_DATA_UNAVAILABLE_DETAIL,
            })),
        )));
    }

    if decision.route_kind.as_deref() == Some("list_keys")
        && request_context.method() == http::Method::GET
        && matches!(
            request_context.path(),
            "/api/admin/adaptive/keys" | "/api/admin/adaptive/keys/"
        )
    {
        let provider_id = query_param_value(request_context.query_string(), "provider_id");
        return Ok(Some(
            state
                .build_admin_adaptive_keys_response(provider_id.as_deref())
                .await?,
        ));
    }

    if decision.route_kind.as_deref() == Some("summary")
        && request_context.method() == http::Method::GET
        && matches!(
            request_context.path(),
            "/api/admin/adaptive/summary" | "/api/admin/adaptive/summary/"
        )
    {
        return Ok(Some(state.build_admin_adaptive_summary_response().await?));
    }

    let key_id = admin_adaptive_key_id_from_path(request_context.path());
    if key_id.is_none() {
        return Ok(Some(state.admin_adaptive_dispatcher_not_found_response()));
    }
    let key_id = key_id.expect("checked is_some above");

    if decision.route_kind.as_deref() == Some("get_stats")
        && request_context.method() == http::Method::GET
        && request_context.path().ends_with("/stats")
    {
        return Ok(Some(
            state.build_admin_adaptive_stats_response(&key_id).await?,
        ));
    }

    if !state.has_provider_catalog_data_writer() {
        return Ok(Some(build_proxy_error_response(
            http::StatusCode::SERVICE_UNAVAILABLE,
            "data_unavailable",
            ADMIN_ADAPTIVE_DATA_UNAVAILABLE_DETAIL,
            Some(json!({
                "error": ADMIN_ADAPTIVE_DATA_UNAVAILABLE_DETAIL,
            })),
        )));
    }

    if decision.route_kind.as_deref() == Some("toggle_mode")
        && request_context.method() == http::Method::PATCH
        && request_context.path().ends_with("/mode")
    {
        let Some(request_body) = request_body else {
            return Ok(Some(build_proxy_error_response(
                http::StatusCode::BAD_REQUEST,
                "invalid_request",
                "请求数据验证失败",
                None,
            )));
        };
        return Ok(Some(
            state
                .toggle_admin_adaptive_mode_response(&key_id, request_body)
                .await?,
        ));
    }

    if decision.route_kind.as_deref() == Some("set_limit")
        && request_context.method() == http::Method::PATCH
        && request_context.path().ends_with("/limit")
    {
        let Some(limit_value) = query_param_value(request_context.query_string(), "limit") else {
            return Ok(Some(
                (
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": "缺少 limit 参数" })),
                )
                    .into_response(),
            ));
        };
        let Ok(limit) = limit_value.parse::<u32>() else {
            return Ok(Some(
                (
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": "limit 必须是数字" })),
                )
                    .into_response(),
            ));
        };
        if !(1..=100).contains(&limit) {
            return Ok(Some(
                (
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": "limit 超出范围（1-100）" })),
                )
                    .into_response(),
            ));
        }

        return Ok(Some(
            state
                .set_admin_adaptive_limit_response(&key_id, limit)
                .await?,
        ));
    }

    if decision.route_kind.as_deref() == Some("reset_learning")
        && request_context.method() == http::Method::DELETE
        && request_context.path().ends_with("/learning")
    {
        return Ok(Some(
            state
                .reset_admin_adaptive_learning_response(&key_id)
                .await?,
        ));
    }

    Ok(Some(state.admin_adaptive_dispatcher_not_found_response()))
}
