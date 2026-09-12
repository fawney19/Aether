use axum::{body::Body, response::Response};

pub(super) use super::{query_param_value, AppState, GatewayPublicRequestContext};

#[path = "models/responses.rs"]
mod models_responses;
#[path = "models/route.rs"]
mod models_route;
#[path = "models/shared.rs"]
mod models_shared;

pub(crate) use self::models_responses::build_models_auth_error_response;
#[cfg(test)]
pub(crate) use self::models_shared::filter_eligible_model_rows;
pub(crate) use self::models_shared::{
    filter_eligible_model_rows_for_constraints, filter_eligible_model_rows_for_state,
    matches_model_mapping_for_models, models_api_format,
};

pub(super) async fn maybe_build_local_models_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
) -> Option<Response<Body>> {
    models_route::maybe_build_local_models_route_response(state, request_context).await
}
