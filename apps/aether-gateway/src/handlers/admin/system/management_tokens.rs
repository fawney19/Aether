use crate::control::GatewayPublicRequestContext;
use crate::handlers::admin::shared::{query_param_optional_bool, query_param_value};
use crate::handlers::admin::system::shared::paths::{
    admin_management_token_id_from_path, admin_management_token_status_id_from_path,
    is_admin_management_tokens_root,
};
use crate::handlers::internal::build_management_token_payload;
use crate::{AppState, GatewayError};
use aether_data::repository::management_tokens::ManagementTokenListQuery;
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(crate) async fn maybe_build_local_admin_management_tokens_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.control_decision.as_ref() else {
        return Ok(None);
    };
    if decision.route_family.as_deref() != Some("management_tokens_manage") {
        return Ok(None);
    }

    if decision
        .admin_principal
        .as_ref()
        .and_then(|principal| principal.management_token_id.as_deref())
        .is_some()
    {
        return Ok(Some(
            (
                http::StatusCode::FORBIDDEN,
                Json(json!({
                    "detail": "不允许使用 Management Token 管理其他 Token，请使用 Web 界面或 JWT 认证"
                })),
            )
                .into_response(),
        ));
    }

    if decision.route_kind.as_deref() == Some("list_tokens")
        && request_context.request_method == http::Method::GET
        && is_admin_management_tokens_root(&request_context.request_path)
    {
        if !state.has_management_token_reader() {
            return Ok(None);
        }
        let user_id = query_param_value(request_context.request_query_string.as_deref(), "user_id");
        let is_active =
            query_param_optional_bool(request_context.request_query_string.as_deref(), "is_active");
        let skip = query_param_value(request_context.request_query_string.as_deref(), "skip")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0);
        let limit = query_param_value(request_context.request_query_string.as_deref(), "limit")
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value > 0 && *value <= 100)
            .unwrap_or(50);
        let page = state
            .list_management_tokens(&ManagementTokenListQuery {
                user_id,
                is_active,
                offset: skip,
                limit,
            })
            .await?;
        let items = page
            .items
            .iter()
            .map(|item| build_management_token_payload(&item.token, Some(&item.user)))
            .collect::<Vec<_>>();
        return Ok(Some(
            Json(json!({
                "items": items,
                "total": page.total,
                "skip": skip,
                "limit": limit,
            }))
            .into_response(),
        ));
    }

    if decision.route_kind.as_deref() == Some("get_token")
        && request_context.request_method == http::Method::GET
    {
        if !state.has_management_token_reader() {
            return Ok(None);
        }
        let Some(token_id) = admin_management_token_id_from_path(&request_context.request_path)
        else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "Management Token 不存在" })),
                )
                    .into_response(),
            ));
        };
        return Ok(Some(
            match state.get_management_token_with_user(&token_id).await? {
                Some(token) => Json(build_management_token_payload(
                    &token.token,
                    Some(&token.user),
                ))
                .into_response(),
                None => (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "Management Token 不存在" })),
                )
                    .into_response(),
            },
        ));
    }

    if decision.route_kind.as_deref() == Some("delete_token")
        && request_context.request_method == http::Method::DELETE
    {
        if !state.has_management_token_writer() {
            return Ok(None);
        }
        let Some(token_id) = admin_management_token_id_from_path(&request_context.request_path)
        else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "Management Token 不存在" })),
                )
                    .into_response(),
            ));
        };
        let existing = match state.get_management_token_with_user(&token_id).await? {
            Some(token) => token,
            None => {
                return Ok(Some(
                    (
                        http::StatusCode::NOT_FOUND,
                        Json(json!({ "detail": "Management Token 不存在" })),
                    )
                        .into_response(),
                ));
            }
        };
        let deleted = state.delete_management_token(&existing.token.id).await?;
        return Ok(Some(if deleted {
            Json(json!({ "message": "删除成功" })).into_response()
        } else {
            (
                http::StatusCode::NOT_FOUND,
                Json(json!({ "detail": "Management Token 不存在" })),
            )
                .into_response()
        }));
    }

    if decision.route_kind.as_deref() == Some("toggle_status")
        && request_context.request_method == http::Method::PATCH
    {
        if !state.has_management_token_writer() {
            return Ok(None);
        }
        let Some(token_id) =
            admin_management_token_status_id_from_path(&request_context.request_path)
        else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "Management Token 不存在" })),
                )
                    .into_response(),
            ));
        };
        let existing = match state.get_management_token_with_user(&token_id).await? {
            Some(token) => token,
            None => {
                return Ok(Some(
                    (
                        http::StatusCode::NOT_FOUND,
                        Json(json!({ "detail": "Management Token 不存在" })),
                    )
                        .into_response(),
                ));
            }
        };
        let Some(updated) = state
            .set_management_token_active(&existing.token.id, !existing.token.is_active)
            .await?
        else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "Management Token 不存在" })),
                )
                    .into_response(),
            ));
        };
        return Ok(Some(
            Json(json!({
                "message": format!("Token 已{}", if updated.is_active { "启用" } else { "禁用" }),
                "data": build_management_token_payload(&updated, Some(&existing.user)),
            }))
            .into_response(),
        ));
    }

    Ok(None)
}
