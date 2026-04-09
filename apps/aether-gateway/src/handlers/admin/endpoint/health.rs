use super::extractors::{admin_health_key_id, admin_recover_key_id};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::query_param_value;
use crate::handlers::public::ApiFormatHealthMonitorOptions;
use crate::GatewayError;
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

const ADMIN_ENDPOINT_HEALTH_DATA_UNAVAILABLE_DETAIL: &str =
    "Admin endpoint health data unavailable";

fn build_admin_endpoint_health_data_unavailable_response() -> Response<Body> {
    (
        http::StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "detail": ADMIN_ENDPOINT_HEALTH_DATA_UNAVAILABLE_DETAIL })),
    )
        .into_response()
}

pub(super) async fn maybe_build_local_admin_endpoints_health_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.decision() else {
        return Ok(None);
    };

    if decision.route_family.as_deref() == Some("endpoints_health")
        && decision.route_kind.as_deref() == Some("health_summary")
        && request_context.path() == "/api/admin/endpoints/health/summary"
    {
        if !state.has_provider_catalog_data_reader() {
            return Ok(Some(build_admin_endpoint_health_data_unavailable_response()));
        }
        let Some(payload) = state.build_admin_health_summary_payload().await else {
            return Ok(Some(build_admin_endpoint_health_data_unavailable_response()));
        };
        return Ok(Some(Json(payload).into_response()));
    }

    if decision.route_family.as_deref() == Some("endpoints_health")
        && decision.route_kind.as_deref() == Some("key_health")
        && request_context
            .request_path
            .starts_with("/api/admin/endpoints/health/key/")
    {
        if !state.has_provider_catalog_data_reader() {
            return Ok(Some(build_admin_endpoint_health_data_unavailable_response()));
        }
        let Some(key_id) = admin_health_key_id(request_context.path()) else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "Key 不存在" })),
                )
                    .into_response(),
            ));
        };
        let api_format = query_param_value(request_context.query_string(), "api_format");
        return Ok(Some(
            match state
                .build_admin_key_health_payload(&key_id, api_format.as_deref())
                .await
            {
                Some(payload) => Json(payload).into_response(),
                None => (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": format!("Key {key_id} 不存在") })),
                )
                    .into_response(),
            },
        ));
    }

    if decision.route_family.as_deref() == Some("endpoints_health")
        && decision.route_kind.as_deref() == Some("recover_key_health")
        && request_context
            .request_path
            .starts_with("/api/admin/endpoints/health/keys/")
    {
        if !state.has_provider_catalog_data_reader() || !state.has_provider_catalog_data_writer() {
            return Ok(Some(build_admin_endpoint_health_data_unavailable_response()));
        }
        let Some(key_id) = admin_recover_key_id(request_context.path()) else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "Key 不存在" })),
                )
                    .into_response(),
            ));
        };
        let api_format = query_param_value(request_context.query_string(), "api_format");
        return Ok(Some(
            match state
                .recover_admin_key_health(&key_id, api_format.as_deref())
                .await
            {
                Some(payload) => Json(payload).into_response(),
                None => (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": format!("Key {key_id} 不存在") })),
                )
                    .into_response(),
            },
        ));
    }

    if decision.route_family.as_deref() == Some("endpoints_health")
        && decision.route_kind.as_deref() == Some("recover_all_keys_health")
        && request_context.path() == "/api/admin/endpoints/health/keys"
    {
        if !state.has_provider_catalog_data_reader() || !state.has_provider_catalog_data_writer() {
            return Ok(Some(build_admin_endpoint_health_data_unavailable_response()));
        }
        let Some(payload) = state.recover_all_admin_key_health().await else {
            return Ok(Some(build_admin_endpoint_health_data_unavailable_response()));
        };
        return Ok(Some(Json(payload).into_response()));
    }

    if decision.route_family.as_deref() == Some("endpoints_health")
        && decision.route_kind.as_deref() == Some("health_api_formats")
        && request_context.path() == "/api/admin/endpoints/health/api-formats"
    {
        if !state.has_provider_catalog_data_reader() || !state.has_request_candidate_data_reader() {
            return Ok(Some(build_admin_endpoint_health_data_unavailable_response()));
        }
        let lookback_hours = query_param_value(request_context.query_string(), "lookback_hours")
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| (1..=72).contains(value))
            .unwrap_or(6);
        let per_format_limit =
            query_param_value(request_context.query_string(), "per_format_limit")
                .and_then(|value| value.parse::<usize>().ok())
                .filter(|value| (10..=200).contains(value))
                .unwrap_or(60);
        let Some(payload) = state
            .build_api_format_health_monitor_payload(
                lookback_hours,
                per_format_limit,
                ApiFormatHealthMonitorOptions {
                    include_api_path: false,
                    include_provider_count: true,
                    include_key_count: true,
                },
            )
            .await
        else {
            return Ok(Some(build_admin_endpoint_health_data_unavailable_response()));
        };
        return Ok(Some(Json(payload).into_response()));
    }

    if decision.route_family.as_deref() == Some("endpoints_health")
        && decision.route_kind.as_deref() == Some("health_status")
        && request_context.path() == "/api/admin/endpoints/health/status"
    {
        if !state.has_provider_catalog_data_reader() || !state.has_request_candidate_data_reader() {
            return Ok(Some(build_admin_endpoint_health_data_unavailable_response()));
        }
        let lookback_hours = query_param_value(request_context.query_string(), "lookback_hours")
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| (1..=72).contains(value))
            .unwrap_or(6);
        let Some(payload) = state
            .build_admin_endpoint_health_status_payload(lookback_hours)
            .await
        else {
            return Ok(Some(build_admin_endpoint_health_data_unavailable_response()));
        };
        return Ok(Some(Json(payload).into_response()));
    }

    Ok(None)
}
