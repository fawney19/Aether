use super::super::config::{
    admin_provider_ops_config_object, admin_provider_ops_merge_credentials,
    resolve_admin_provider_ops_base_url,
};
use super::super::support::AdminProviderOpsSaveConfigRequest;
use super::super::verify::{
    admin_provider_ops_local_verify_response, admin_provider_ops_normalized_verify_architecture_id,
    admin_provider_ops_verify_failure,
};
use crate::handlers::admin::request::AdminAppState;
use crate::handlers::admin::shared::attach_admin_audit_response;
use crate::GatewayError;
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};

pub(super) async fn handle_admin_provider_ops_verify(
    state: &AdminAppState<'_>,
    provider_id: &str,
    request_body: Option<&Bytes>,
) -> Result<Response<Body>, GatewayError> {
    let payload = match parse_json_object_payload::<AdminProviderOpsSaveConfigRequest>(request_body)
    {
        Ok(payload) => payload,
        Err(response) => return Ok(response),
    };

    let provider_ids = [provider_id.to_string()];
    let existing_provider = state
        .read_provider_catalog_providers_by_ids(&provider_ids)
        .await?
        .into_iter()
        .next();
    let endpoints = if existing_provider.is_some() {
        state
            .list_provider_catalog_endpoints_by_provider_ids(&provider_ids)
            .await?
    } else {
        Vec::new()
    };
    let base_url = payload
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            existing_provider.as_ref().and_then(|provider| {
                resolve_admin_provider_ops_base_url(
                    provider,
                    &endpoints,
                    admin_provider_ops_config_object(provider),
                )
            })
        });
    let Some(base_url) = base_url else {
        return Ok(Json(admin_provider_ops_verify_failure("请提供 API 地址")).into_response());
    };

    let architecture_id =
        admin_provider_ops_normalized_verify_architecture_id(&payload.architecture_id);
    let credentials = existing_provider.as_ref().map_or_else(
        || payload.connector.credentials.clone(),
        |provider| {
            admin_provider_ops_merge_credentials(
                state,
                provider,
                payload.connector.credentials.clone(),
            )
        },
    );
    let payload = admin_provider_ops_local_verify_response(
        state,
        &base_url,
        architecture_id,
        &payload.connector.config,
        &credentials,
    )
    .await;
    Ok(attach_admin_audit_response(
        Json(payload).into_response(),
        "admin_provider_ops_config_verified",
        "verify_provider_ops_config",
        "provider",
        provider_id,
    ))
}

fn parse_json_object_payload<T>(request_body: Option<&Bytes>) -> Result<T, Response<Body>>
where
    T: serde::de::DeserializeOwned,
{
    let Some(request_body) = request_body else {
        return Err(bad_request_detail_response("请求体不能为空"));
    };
    let raw_value = serde_json::from_slice::<serde_json::Value>(request_body)
        .map_err(|_| bad_request_detail_response("请求体必须是合法的 JSON 对象"))?;
    if !raw_value.is_object() {
        return Err(bad_request_detail_response("请求体必须是合法的 JSON 对象"));
    }
    serde_json::from_value::<T>(raw_value)
        .map_err(|_| bad_request_detail_response("请求体必须是合法的 JSON 对象"))
}

fn bad_request_detail_response(detail: &str) -> Response<Body> {
    (
        http::StatusCode::BAD_REQUEST,
        Json(serde_json::json!({ "detail": detail })),
    )
        .into_response()
}
