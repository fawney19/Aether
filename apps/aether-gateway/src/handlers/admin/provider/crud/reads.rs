use super::responses::build_admin_providers_data_unavailable_response;
use crate::handlers::admin::provider::shared::paths::{
    admin_provider_id_for_health_monitor, admin_provider_id_for_mapping_preview,
    admin_provider_id_for_pool_status, admin_provider_id_for_summary, is_admin_providers_root,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::{query_param_optional_bool, query_param_value};
use crate::GatewayError;
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

fn build_admin_provider_not_found_response(detail: impl Into<String>) -> Response<Body> {
    (
        http::StatusCode::NOT_FOUND,
        Json(json!({ "detail": detail.into() })),
    )
        .into_response()
}

pub(crate) async fn maybe_build_local_admin_provider_reads_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    route_kind: Option<&str>,
) -> Result<Option<Response<Body>>, GatewayError> {
    if route_kind == Some("list_providers") && is_admin_providers_root(request_context.path()) {
        if !state.has_provider_catalog_data_reader() {
            return Ok(Some(build_admin_providers_data_unavailable_response()));
        }
        let skip = query_param_value(request_context.query_string(), "skip")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0);
        let limit = query_param_value(request_context.query_string(), "limit")
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value > 0 && *value <= 500)
            .unwrap_or(100);
        let is_active = query_param_optional_bool(request_context.query_string(), "is_active");
        let Some(payload) = state
            .build_admin_providers_payload(skip, limit, is_active)
            .await
        else {
            return Ok(Some(build_admin_providers_data_unavailable_response()));
        };
        return Ok(Some(Json(payload).into_response()));
    }

    if route_kind == Some("summary_list")
        && request_context.path() == "/api/admin/providers/summary"
    {
        if !state.has_provider_catalog_data_reader() {
            return Ok(Some(build_admin_providers_data_unavailable_response()));
        }
        let page = query_param_value(request_context.query_string(), "page")
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(1);
        let page_size = query_param_value(request_context.query_string(), "page_size")
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value > 0 && *value <= 10_000)
            .unwrap_or(20);
        let search =
            query_param_value(request_context.query_string(), "search").unwrap_or_default();
        let status = query_param_value(request_context.query_string(), "status")
            .unwrap_or_else(|| "all".to_string());
        let api_format = query_param_value(request_context.query_string(), "api_format")
            .unwrap_or_else(|| "all".to_string());
        let model_id = query_param_value(request_context.query_string(), "model_id")
            .unwrap_or_else(|| "all".to_string());
        let Some(payload) = state
            .build_admin_providers_summary_payload(
                page,
                page_size,
                &search,
                &status,
                &api_format,
                &model_id,
            )
            .await
        else {
            return Ok(Some(build_admin_providers_data_unavailable_response()));
        };
        return Ok(Some(Json(payload).into_response()));
    }

    if route_kind == Some("provider_summary")
        && request_context.path().starts_with("/api/admin/providers/")
        && request_context.path().ends_with("/summary")
    {
        if !state.has_provider_catalog_data_reader() {
            return Ok(Some(build_admin_providers_data_unavailable_response()));
        }
        let Some(provider_id) = admin_provider_id_for_summary(request_context.path()) else {
            return Ok(Some(build_admin_provider_not_found_response(
                "Provider 不存在",
            )));
        };
        return Ok(Some(
            match state
                .build_admin_provider_summary_payload(&provider_id)
                .await
            {
                Some(payload) => Json(payload).into_response(),
                None => build_admin_provider_not_found_response(format!(
                    "Provider {provider_id} 不存在"
                )),
            },
        ));
    }

    if route_kind == Some("health_monitor")
        && request_context.path().starts_with("/api/admin/providers/")
        && request_context.path().ends_with("/health-monitor")
    {
        let Some(provider_id) = admin_provider_id_for_health_monitor(request_context.path()) else {
            return Ok(Some(build_admin_provider_not_found_response(
                "Provider 不存在",
            )));
        };
        let lookback_hours = query_param_value(request_context.query_string(), "lookback_hours")
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| (1..=72).contains(value))
            .unwrap_or(6);
        let per_endpoint_limit =
            query_param_value(request_context.query_string(), "per_endpoint_limit")
                .and_then(|value| value.parse::<usize>().ok())
                .filter(|value| (10..=200).contains(value))
                .unwrap_or(48);
        return Ok(Some(
            match state
                .build_admin_provider_health_monitor_payload(
                    &provider_id,
                    lookback_hours,
                    per_endpoint_limit,
                )
                .await
            {
                Some(payload) => Json(payload).into_response(),
                None => build_admin_provider_not_found_response(format!(
                    "Provider {provider_id} 不存在"
                )),
            },
        ));
    }

    if route_kind == Some("mapping_preview")
        && request_context.path().starts_with("/api/admin/providers/")
        && request_context.path().ends_with("/mapping-preview")
    {
        let Some(provider_id) = admin_provider_id_for_mapping_preview(request_context.path())
        else {
            return Ok(Some(build_admin_provider_not_found_response(
                "Provider 不存在",
            )));
        };
        return Ok(Some(
            match state
                .build_admin_provider_mapping_preview_payload(&provider_id)
                .await
            {
                Some(payload) => Json(payload).into_response(),
                None => build_admin_provider_not_found_response(format!(
                    "Provider {provider_id} 不存在"
                )),
            },
        ));
    }

    if route_kind == Some("pool_status")
        && request_context.method() == http::Method::GET
        && request_context.path().ends_with("/pool-status")
    {
        let Some(provider_id) = admin_provider_id_for_pool_status(request_context.path()) else {
            return Ok(Some(build_admin_provider_not_found_response(
                "Provider 不存在",
            )));
        };
        return Ok(Some(
            match state
                .build_admin_provider_pool_status_payload(&provider_id)
                .await
            {
                Some(payload) => Json(payload).into_response(),
                None => build_admin_provider_not_found_response(format!(
                    "Provider {provider_id} 不存在"
                )),
            },
        ));
    }

    Ok(None)
}
