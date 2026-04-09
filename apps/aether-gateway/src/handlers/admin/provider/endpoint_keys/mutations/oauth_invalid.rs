use crate::handlers::admin::provider::shared::paths::admin_clear_oauth_invalid_key_id;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
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
    let Some(decision) = request_context.decision() else {
        return Ok(None);
    };
    if decision.route_family.as_deref() != Some("endpoints_manage")
        || decision.route_kind.as_deref() != Some("clear_oauth_invalid")
        || request_context.method() != http::Method::POST
        || !request_context
            .path()
            .starts_with("/api/admin/endpoints/keys/")
        || !request_context.path().ends_with("/clear-oauth-invalid")
    {
        return Ok(None);
    }

    let Some(key_id) = admin_clear_oauth_invalid_key_id(request_context.path()) else {
        return Ok(Some(not_found_response("Key 不存在")));
    };
    let Some(key) = state
        .read_provider_catalog_keys_by_ids(std::slice::from_ref(&key_id))
        .await?
        .into_iter()
        .next()
    else {
        return Ok(Some(not_found_response(format!("Key {key_id} 不存在"))));
    };
    if key.oauth_invalid_at_unix_secs.is_none() {
        return Ok(Some(
            Json(json!({
                "message": "该 Key 当前无失效标记，无需清除"
            }))
            .into_response(),
        ));
    }
    state
        .clear_provider_catalog_key_oauth_invalid_marker(&key_id)
        .await?;
    Ok(Some(
        Json(json!({
            "message": "已清除 OAuth 失效标记"
        }))
        .into_response(),
    ))
}

fn not_found_response(detail: impl Into<String>) -> Response<Body> {
    (
        http::StatusCode::NOT_FOUND,
        Json(json!({ "detail": detail.into() })),
    )
        .into_response()
}
