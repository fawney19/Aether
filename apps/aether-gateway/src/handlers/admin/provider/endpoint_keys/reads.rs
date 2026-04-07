use crate::control::GatewayPublicRequestContext;
use crate::handlers::admin::provider::shared::paths::{
    admin_export_key_id, admin_provider_id_for_keys, admin_reveal_key_id,
};
use crate::handlers::admin::shared::{attach_admin_audit_response, query_param_value};
use crate::handlers::public::build_admin_keys_grouped_by_format_payload;
use crate::{AppState, GatewayError};
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

use super::super::write::keys::build_admin_provider_keys_payload;
use super::super::write::reveal::{build_admin_export_key_payload, build_admin_reveal_key_payload};

pub(super) async fn maybe_handle(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    _request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.control_decision.as_ref() else {
        return Ok(None);
    };

    if decision.route_family.as_deref() == Some("endpoints_manage")
        && decision.route_kind.as_deref() == Some("keys_grouped_by_format")
        && request_context.request_path == "/api/admin/endpoints/keys/grouped-by-format"
    {
        let Some(payload) = build_admin_keys_grouped_by_format_payload(state).await else {
            return Ok(None);
        };
        return Ok(Some(Json(payload).into_response()));
    }

    if decision.route_family.as_deref() == Some("endpoints_manage")
        && decision.route_kind.as_deref() == Some("reveal_key")
        && request_context
            .request_path
            .starts_with("/api/admin/endpoints/keys/")
        && request_context.request_path.ends_with("/reveal")
    {
        let Some(key_id) = admin_reveal_key_id(&request_context.request_path) else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "Key 不存在" })),
                )
                    .into_response(),
            ));
        };
        let Some(key) = state
            .read_provider_catalog_keys_by_ids(std::slice::from_ref(&key_id))
            .await?
            .into_iter()
            .next()
        else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": format!("Key {key_id} 不存在") })),
                )
                    .into_response(),
            ));
        };
        return Ok(Some(match build_admin_reveal_key_payload(state, &key) {
            Ok(payload) => attach_admin_audit_response(
                Json(payload).into_response(),
                "admin_provider_key_revealed",
                "reveal_provider_key",
                "provider_key",
                &key_id,
            ),
            Err(detail) => (
                http::StatusCode::BAD_REQUEST,
                Json(json!({ "detail": detail })),
            )
                .into_response(),
        }));
    }

    if decision.route_family.as_deref() == Some("endpoints_manage")
        && decision.route_kind.as_deref() == Some("export_key")
        && request_context
            .request_path
            .starts_with("/api/admin/endpoints/keys/")
        && request_context.request_path.ends_with("/export")
    {
        let Some(key_id) = admin_export_key_id(&request_context.request_path) else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "Key 不存在" })),
                )
                    .into_response(),
            ));
        };
        let Some(key) = state
            .read_provider_catalog_keys_by_ids(std::slice::from_ref(&key_id))
            .await?
            .into_iter()
            .next()
        else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": format!("Key {key_id} 不存在") })),
                )
                    .into_response(),
            ));
        };
        return Ok(Some(
            match build_admin_export_key_payload(state, &key).await {
                Ok(payload) => attach_admin_audit_response(
                    Json(payload).into_response(),
                    "admin_provider_key_exported",
                    "export_provider_key",
                    "provider_key_export",
                    &key_id,
                ),
                Err(detail) => (
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": detail })),
                )
                    .into_response(),
            },
        ));
    }

    if decision.route_family.as_deref() == Some("endpoints_manage")
        && decision.route_kind.as_deref() == Some("list_provider_keys")
        && request_context
            .request_path
            .starts_with("/api/admin/endpoints/providers/")
        && request_context.request_path.ends_with("/keys")
    {
        let Some(provider_id) = admin_provider_id_for_keys(&request_context.request_path) else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "Provider 不存在" })),
                )
                    .into_response(),
            ));
        };
        let skip = query_param_value(request_context.request_query_string.as_deref(), "skip")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0);
        let limit = query_param_value(request_context.request_query_string.as_deref(), "limit")
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(100);
        return Ok(Some(
            match build_admin_provider_keys_payload(state, &provider_id, skip, limit).await {
                Some(payload) => Json(payload).into_response(),
                None => (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": format!("Provider {provider_id} 不存在") })),
                )
                    .into_response(),
            },
        ));
    }

    Ok(None)
}
