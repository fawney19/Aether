use crate::handlers::admin::model::build_admin_model_catalog_payload;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

const ADMIN_MODEL_CATALOG_DATA_UNAVAILABLE_DETAIL: &str = "Admin model catalog data unavailable";

fn build_admin_model_catalog_data_unavailable_response() -> Response<Body> {
    (
        http::StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "detail": ADMIN_MODEL_CATALOG_DATA_UNAVAILABLE_DETAIL })),
    )
        .into_response()
}

pub(crate) async fn maybe_build_local_admin_model_catalog_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.decision() else {
        return Ok(None);
    };

    if decision.route_family.as_deref() == Some("model_catalog_manage")
        && decision.route_kind.as_deref() == Some("catalog")
        && request_context.method() == http::Method::GET
        && request_context.path() == "/api/admin/models/catalog"
    {
        if !state.has_global_model_data_reader() || !state.has_provider_catalog_data_reader() {
            return Ok(Some(build_admin_model_catalog_data_unavailable_response()));
        }
        let Some(payload) = build_admin_model_catalog_payload(state).await else {
            return Ok(Some(build_admin_model_catalog_data_unavailable_response()));
        };
        return Ok(Some(Json(payload).into_response()));
    }

    if decision.route_family.as_deref() == Some("model_external_manage")
        && decision.route_kind.as_deref() == Some("external")
        && request_context.method() == http::Method::GET
        && request_context.path() == "/api/admin/models/external"
    {
        return Ok(Some(
            match state.read_admin_external_models_cache().await? {
                Some(payload) => Json(payload).into_response(),
                None => (
                    http::StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({
                        "detail": "External models catalog requires Rust admin backend"
                    })),
                )
                    .into_response(),
            },
        ));
    }

    if decision.route_family.as_deref() == Some("model_external_manage")
        && decision.route_kind.as_deref() == Some("clear_external_cache")
        && request_context.method() == http::Method::DELETE
        && request_context.path() == "/api/admin/models/external/cache"
    {
        return Ok(Some(
            Json(state.clear_admin_external_models_cache().await?).into_response(),
        ));
    }

    Ok(None)
}
