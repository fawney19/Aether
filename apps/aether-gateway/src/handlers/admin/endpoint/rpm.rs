use super::extractors::admin_rpm_key_id;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) async fn maybe_build_local_admin_endpoints_rpm_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.decision() else {
        return Ok(None);
    };

    if decision.route_family.as_deref() == Some("endpoints_rpm")
        && decision.route_kind.as_deref() == Some("key_rpm")
        && request_context
            .path()
            .starts_with("/api/admin/endpoints/rpm/key/")
    {
        let Some(key_id) = admin_rpm_key_id(request_context.path()) else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "Key 不存在" })),
                )
                    .into_response(),
            ));
        };
        return Ok(Some(
            match state.build_admin_key_rpm_payload(&key_id).await {
                Some(payload) => Json(payload).into_response(),
                None => (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": format!("Key {key_id} 不存在") })),
                )
                    .into_response(),
            },
        ));
    }

    if decision.route_family.as_deref() == Some("endpoints_rpm")
        && decision.route_kind.as_deref() == Some("reset_key_rpm")
        && request_context.method() == http::Method::DELETE
        && request_context
            .path()
            .starts_with("/api/admin/endpoints/rpm/key/")
    {
        let Some(key_id) = admin_rpm_key_id(request_context.path()) else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "Key 不存在" })),
                )
                    .into_response(),
            ));
        };
        let Some(_key) = state
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
        let now_unix_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        state.mark_provider_key_rpm_reset(&key_id, now_unix_secs);
        return Ok(Some(
            Json(json!({
                "message": "RPM 计数已重置"
            }))
            .into_response(),
        ));
    }

    Ok(None)
}
