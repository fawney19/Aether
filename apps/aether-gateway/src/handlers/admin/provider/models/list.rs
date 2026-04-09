use super::payloads::build_admin_provider_models_payload;
use crate::handlers::admin::provider::shared::paths::admin_provider_id_for_models_list;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::{query_param_optional_bool, query_param_value};
use crate::GatewayError;
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(super) async fn maybe_handle(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    _request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    if request_context.route_family() == Some("provider_models_manage")
        && request_context.route_kind() == Some("list_provider_models")
        && request_context.method() == http::Method::GET
        && request_context.path().starts_with("/api/admin/providers/")
        && request_context.path().ends_with("/models")
    {
        let Some(provider_id) = admin_provider_id_for_models_list(request_context.path()) else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "Provider 不存在" })),
                )
                    .into_response(),
            ));
        };
        let skip = query_param_value(request_context.query_string(), "skip")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0);
        let limit = query_param_value(request_context.query_string(), "limit")
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value > 0 && *value <= 500)
            .unwrap_or(100);
        let is_active = query_param_optional_bool(request_context.query_string(), "is_active");
        return Ok(Some(
            match build_admin_provider_models_payload(state, &provider_id, skip, limit, is_active)
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
