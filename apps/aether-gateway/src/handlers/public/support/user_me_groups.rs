use axum::{body::Body, http, response::IntoResponse, response::Response, Json};
use serde_json::json;

use super::{
    build_auth_error_response, resolve_authenticated_local_user, AppState,
    GatewayPublicRequestContext,
};

pub(super) async fn handle_users_me_groups_get(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let groups = match state
        .list_effective_user_groups_for_user(&auth.user.id)
        .await
    {
        Ok(value) => value,
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("user group lookup failed: {err:?}"),
                false,
            )
        }
    };
    Json(json!({
        "items": groups.into_iter().map(|group| json!({
            "id": group.id,
            "name": group.name,
            "sell_rate_multiplier": group.sell_rate_multiplier,
        })).collect::<Vec<_>>()
    }))
    .into_response()
}
