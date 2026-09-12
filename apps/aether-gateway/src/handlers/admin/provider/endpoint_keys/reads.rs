use crate::handlers::admin::provider::shared::paths::{
    admin_export_key_id, admin_provider_id_for_keys, admin_reveal_key_id,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::{
    attach_admin_audit_response, mark_sensitive_admin_response_no_store, query_param_value,
};
use crate::GatewayError;
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

fn parse_provider_keys_page_param(raw: Option<String>) -> Result<usize, String> {
    match raw {
        None => Ok(1),
        Some(value) => {
            let parsed = value
                .parse::<usize>()
                .map_err(|_| "page must be an integer between 1 and 10000".to_string())?;
            if (1..=10_000).contains(&parsed) {
                Ok(parsed)
            } else {
                Err("page must be an integer between 1 and 10000".to_string())
            }
        }
    }
}

fn parse_provider_keys_page_size_param(raw: Option<String>) -> Result<usize, String> {
    match raw {
        None => Ok(20),
        Some(value) => {
            let parsed = value
                .parse::<usize>()
                .map_err(|_| "page_size must be an integer between 1 and 1000".to_string())?;
            if (1..=1000).contains(&parsed) {
                Ok(parsed)
            } else {
                Err("page_size must be an integer between 1 and 1000".to_string())
            }
        }
    }
}

pub(super) async fn maybe_handle(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    _request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.decision() else {
        return Ok(None);
    };

    if decision.route_family.as_deref() == Some("endpoints_manage")
        && decision.route_kind.as_deref() == Some("keys_grouped_by_format")
        && request_context.path() == "/api/admin/endpoints/keys/grouped-by-format"
    {
        let Some(payload) = state.build_admin_keys_grouped_by_format_payload().await else {
            return Ok(None);
        };
        return Ok(Some(Json(payload).into_response()));
    }

    if decision.route_family.as_deref() == Some("endpoints_manage")
        && decision.route_kind.as_deref() == Some("reveal_key")
        && request_context
            .path()
            .starts_with("/api/admin/endpoints/keys/")
        && request_context.path().ends_with("/reveal")
    {
        let Some(key_id) = admin_reveal_key_id(request_context.path()) else {
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
        return Ok(Some(match state.build_admin_reveal_key_payload(&key) {
            Ok(payload) => mark_sensitive_admin_response_no_store(attach_admin_audit_response(
                Json(payload).into_response(),
                "admin_provider_key_revealed",
                "reveal_provider_key",
                "provider_key",
                &key_id,
            )),
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
            .path()
            .starts_with("/api/admin/endpoints/keys/")
        && request_context.path().ends_with("/export")
    {
        let Some(key_id) = admin_export_key_id(request_context.path()) else {
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
            match state.build_admin_export_key_payload(&key).await {
                Ok(payload) => mark_sensitive_admin_response_no_store(attach_admin_audit_response(
                    Json(payload).into_response(),
                    "admin_provider_key_exported",
                    "export_provider_key",
                    "provider_key_export",
                    &key_id,
                )),
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
            .path()
            .starts_with("/api/admin/endpoints/providers/")
        && request_context.path().ends_with("/keys")
    {
        let Some(provider_id) = admin_provider_id_for_keys(request_context.path()) else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "Provider 不存在" })),
                )
                    .into_response(),
            ));
        };
        let page_param = query_param_value(request_context.query_string(), "page");
        let page_size_param = query_param_value(request_context.query_string(), "page_size");
        if page_param.is_some() || page_size_param.is_some() {
            let page = match parse_provider_keys_page_param(page_param) {
                Ok(value) => value,
                Err(detail) => {
                    return Ok(Some(
                        (
                            http::StatusCode::BAD_REQUEST,
                            Json(json!({ "detail": detail })),
                        )
                            .into_response(),
                    ));
                }
            };
            let page_size = match parse_provider_keys_page_size_param(page_size_param) {
                Ok(value) => value,
                Err(detail) => {
                    return Ok(Some(
                        (
                            http::StatusCode::BAD_REQUEST,
                            Json(json!({ "detail": detail })),
                        )
                            .into_response(),
                    ));
                }
            };
            return Ok(Some(
                match state
                    .build_admin_provider_keys_page_payload(&provider_id, page, page_size)
                    .await
                {
                    Some(payload) => Json(payload).into_response(),
                    None => (
                        http::StatusCode::NOT_FOUND,
                        Json(json!({ "detail": format!("Provider {provider_id} 不存在") })),
                    )
                        .into_response(),
                },
            ));
        }
        let skip = query_param_value(request_context.query_string(), "skip")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0);
        let limit = query_param_value(request_context.query_string(), "limit")
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(100);
        return Ok(Some(
            match state
                .build_admin_provider_keys_payload(&provider_id, skip, limit)
                .await
            {
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
