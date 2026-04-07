use super::extractors::admin_endpoint_id;
use super::payloads::key_api_formats_without_entry;
use super::support::build_admin_endpoints_data_unavailable_response;
use crate::control::GatewayPublicRequestContext;
use crate::{AppState, GatewayError};
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) async fn maybe_handle(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    _request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.control_decision.as_ref() else {
        return Ok(None);
    };

    if decision.route_family.as_deref() != Some("endpoints_manage")
        || decision.route_kind.as_deref() != Some("delete_endpoint")
        || request_context.request_method != http::Method::DELETE
        || !request_context
            .request_path
            .starts_with("/api/admin/endpoints/")
    {
        return Ok(None);
    }

    if !state.has_provider_catalog_data_reader() || !state.has_provider_catalog_data_writer() {
        return Ok(Some(build_admin_endpoints_data_unavailable_response()));
    }

    let Some(endpoint_id) = admin_endpoint_id(&request_context.request_path) else {
        return Ok(Some(
            (
                http::StatusCode::NOT_FOUND,
                Json(json!({ "detail": "Endpoint 不存在" })),
            )
                .into_response(),
        ));
    };
    let Some(existing_endpoint) = state
        .read_provider_catalog_endpoints_by_ids(std::slice::from_ref(&endpoint_id))
        .await?
        .into_iter()
        .next()
    else {
        return Ok(Some(
            (
                http::StatusCode::NOT_FOUND,
                Json(json!({ "detail": format!("Endpoint {endpoint_id} 不存在") })),
            )
                .into_response(),
        ));
    };
    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let keys = state
        .list_provider_catalog_keys_by_provider_ids(std::slice::from_ref(
            &existing_endpoint.provider_id,
        ))
        .await
        .unwrap_or_default();
    let mut affected_keys_count = 0usize;
    for key in keys {
        let Some(updated_formats) =
            key_api_formats_without_entry(&key, existing_endpoint.api_format.as_str())
        else {
            continue;
        };
        let mut updated_key = key.clone();
        updated_key.api_formats = Some(serde_json::Value::Array(
            updated_formats
                .into_iter()
                .map(serde_json::Value::String)
                .collect(),
        ));
        updated_key.updated_at_unix_secs = Some(now_unix_secs);
        if state
            .update_provider_catalog_key(&updated_key)
            .await?
            .is_none()
        {
            return Ok(Some(build_admin_endpoints_data_unavailable_response()));
        }
        affected_keys_count += 1;
    }
    if !state.delete_provider_catalog_endpoint(&endpoint_id).await? {
        return Ok(Some(
            (
                http::StatusCode::NOT_FOUND,
                Json(json!({ "detail": format!("Endpoint {endpoint_id} 不存在") })),
            )
                .into_response(),
        ));
    }

    Ok(Some(
        Json(json!({
            "message": format!("Endpoint {endpoint_id} 已删除"),
            "affected_keys_count": affected_keys_count,
        }))
        .into_response(),
    ))
}
