use super::super::super::super::{
    build_admin_global_model_payload, build_admin_global_model_providers_payload,
    build_admin_global_model_routing_payload, build_admin_global_models_payload,
};
use super::super::super::helpers::build_admin_global_models_data_unavailable_response;
use super::shared::{global_model_missing_response, global_model_not_found_response};
use crate::handlers::admin::model::shared::{
    admin_global_model_id_from_path, admin_global_model_providers_id,
    admin_global_model_routing_id, is_admin_global_models_root,
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

pub(super) async fn maybe_build_local_admin_global_models_read_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.decision() else {
        return Ok(None);
    };

    if decision.route_family.as_deref() == Some("global_models_manage")
        && decision.route_kind.as_deref() == Some("routing_preview")
        && request_context.method() == http::Method::GET
    {
        return Ok(Some(
            build_routing_preview_response(state, request_context).await?,
        ));
    }

    if decision.route_family.as_deref() == Some("global_models_manage")
        && decision.route_kind.as_deref() == Some("list_global_models")
        && is_admin_global_models_root(request_context.path())
    {
        return Ok(Some(
            build_list_global_models_response(state, request_context).await?,
        ));
    }

    if decision.route_family.as_deref() == Some("global_models_manage")
        && decision.route_kind.as_deref() == Some("get_global_model")
        && request_context.method() == http::Method::GET
    {
        return Ok(Some(
            build_get_global_model_response(state, request_context).await?,
        ));
    }

    if decision.route_family.as_deref() == Some("global_models_manage")
        && decision.route_kind.as_deref() == Some("global_model_providers")
        && request_context.method() == http::Method::GET
    {
        return Ok(Some(
            build_global_model_providers_response(state, request_context).await?,
        ));
    }

    Ok(None)
}

async fn build_routing_preview_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    if !state.has_global_model_data_reader() || !state.has_provider_catalog_data_reader() {
        return Ok(build_admin_global_models_data_unavailable_response());
    }
    let Some(global_model_id) = admin_global_model_routing_id(request_context.path()) else {
        return Ok(global_model_missing_response());
    };
    Ok(
        match build_admin_global_model_routing_payload(state, &global_model_id).await {
            Some(payload) => Json::<serde_json::Value>(payload).into_response(),
            None => global_model_not_found_response(&global_model_id),
        },
    )
}

async fn build_list_global_models_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    if !state.has_global_model_data_reader() {
        return Ok(build_admin_global_models_data_unavailable_response());
    }
    let skip = query_param_value(request_context.query_string(), "skip")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let limit = query_param_value(request_context.query_string(), "limit")
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0 && *value <= 1000)
        .unwrap_or(100);
    let is_active = query_param_optional_bool(request_context.query_string(), "is_active");
    let search = query_param_value(request_context.query_string(), "search");
    let Some(payload): Option<serde_json::Value> =
        build_admin_global_models_payload(state, skip, limit, is_active, search).await
    else {
        return Ok(build_admin_global_models_data_unavailable_response());
    };
    Ok(Json(payload).into_response())
}

async fn build_get_global_model_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    if !state.has_global_model_data_reader() {
        return Ok(build_admin_global_models_data_unavailable_response());
    }
    let Some(global_model_id) = admin_global_model_id_from_path(request_context.path()) else {
        return Ok(global_model_missing_response());
    };
    Ok(
        match build_admin_global_model_payload(state, &global_model_id).await {
            Some(payload) => Json::<serde_json::Value>(payload).into_response(),
            None => global_model_not_found_response(&global_model_id),
        },
    )
}

async fn build_global_model_providers_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    if !state.has_global_model_data_reader() || !state.has_provider_catalog_data_reader() {
        return Ok(build_admin_global_models_data_unavailable_response());
    }
    let Some(global_model_id) = admin_global_model_providers_id(request_context.path()) else {
        return Ok(global_model_missing_response());
    };
    Ok(
        match build_admin_global_model_providers_payload(state, &global_model_id).await {
            Some(payload) => Json::<serde_json::Value>(payload).into_response(),
            None => global_model_not_found_response(&global_model_id),
        },
    )
}
