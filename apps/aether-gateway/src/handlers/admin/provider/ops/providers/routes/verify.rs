use super::super::config::{
    admin_provider_ops_binding_from_config, admin_provider_ops_config_object,
    admin_provider_ops_merge_credentials, resolve_admin_provider_ops_base_url,
};
use super::super::support::AdminProviderOpsSaveConfigRequest;
use super::super::verify::admin_provider_ops_local_verify_response;
use crate::handlers::admin::request::AdminAppState;
use crate::handlers::admin::shared::attach_admin_audit_response;
use crate::GatewayError;
use aether_admin::provider::ops::admin_provider_ops_verify_failure;
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
    let (effective_provider, mut credentials, saved_binding, reused_saved_secret) =
        match existing_provider.as_ref() {
            Some(provider) if admin_provider_ops_config_object(provider).is_some() => {
                match admin_provider_ops_merge_credentials(
                    state,
                    &payload.architecture_id,
                    provider,
                    payload.connector.credentials.clone(),
                )
                .await
                {
                    Ok(snapshot) => (
                        Some(snapshot.provider),
                        snapshot.credentials,
                        Some(snapshot.saved_binding),
                        snapshot.reused_saved_secret,
                    ),
                    Err(detail) => {
                        return Ok(Json(admin_provider_ops_verify_failure(detail)).into_response())
                    }
                }
            }
            Some(provider) => (
                Some(provider.clone()),
                payload.connector.credentials.clone(),
                None,
                false,
            ),
            None => (None, payload.connector.credentials.clone(), None, false),
        };
    let fallback_base_url = saved_binding
        .as_ref()
        .map(|binding| binding.destination.base_url().to_string())
        .or_else(|| {
            effective_provider.as_ref().and_then(|provider| {
                resolve_admin_provider_ops_base_url(
                    provider,
                    &endpoints,
                    admin_provider_ops_config_object(provider),
                )
            })
        });
    let base_url = payload
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or(fallback_base_url);
    let Some(base_url) = base_url else {
        return Ok(Json(admin_provider_ops_verify_failure("请提供 API 地址")).into_response());
    };
    let actions = payload
        .actions
        .iter()
        .map(|(action_type, action)| {
            (
                action_type.clone(),
                serde_json::json!({
                    "enabled": action.enabled,
                    "config": action.config,
                }),
            )
        })
        .collect::<serde_json::Map<String, serde_json::Value>>();
    let requested_provider_ops_config = serde_json::json!({
        "architecture_id": payload.architecture_id,
        "base_url": base_url,
        "connector": {
            "auth_type": payload.connector.auth_type,
            "config": payload.connector.config,
            "credentials": {},
        },
        "actions": actions,
    });
    let requested_binding = match admin_provider_ops_binding_from_config(
        provider_id,
        requested_provider_ops_config
            .as_object()
            .expect("Provider Ops verify config should be an object"),
        &base_url,
    ) {
        Ok(binding) => binding,
        Err(detail) => return Ok(Json(admin_provider_ops_verify_failure(detail)).into_response()),
    };
    if let Some(saved_binding) = saved_binding.as_ref() {
        let same_secret_destination = saved_binding.provider_id == requested_binding.provider_id
            && saved_binding.architecture_id == requested_binding.architecture_id
            && saved_binding.auth_type == requested_binding.auth_type
            && saved_binding.destination == requested_binding.destination;
        if reused_saved_secret && !same_secret_destination {
            return Ok(Json(admin_provider_ops_verify_failure(
                "验证不同的 Provider Ops 架构、认证类型或目标地址时必须重新填写凭据",
            ))
            .into_response());
        }
        if saved_binding != &requested_binding {
            credentials.retain(|field, _| !field.starts_with("_cached_"));
        }
    };

    let payload = admin_provider_ops_local_verify_response(
        state,
        effective_provider.as_ref(),
        requested_binding.destination.base_url(),
        &requested_binding.architecture_id,
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
