use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use aether_admin::observability::usage::{
    admin_usage_bad_request_response, admin_usage_data_unavailable_response,
    ADMIN_USAGE_DATA_UNAVAILABLE_DETAIL,
};
use aether_data_contracts::repository::{
    provider_catalog::StoredProviderCatalogEndpoint,
    usage::{StoredRequestUsageAudit, UsageBodyField},
};
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub(super) fn admin_usage_id_from_detail_path(request_path: &str) -> Option<String> {
    aether_admin::observability::usage::admin_usage_id_from_detail_path(request_path)
}

pub(super) fn admin_usage_id_from_action_path(request_path: &str, action: &str) -> Option<String> {
    aether_admin::observability::usage::admin_usage_id_from_action_path(request_path, action)
}

#[derive(Debug, Default, serde::Deserialize)]
struct AdminUsageReplayRequest {
    #[serde(default, alias = "target_provider_id")]
    provider_id: Option<String>,
    #[serde(default, alias = "target_endpoint_id")]
    endpoint_id: Option<String>,
    #[serde(default, alias = "target_api_key_id")]
    api_key_id: Option<String>,
    #[serde(default)]
    body_override: Option<serde_json::Value>,
}

pub(super) fn admin_usage_resolve_request_preview_body(
    item: &StoredRequestUsageAudit,
    body_override: Option<serde_json::Value>,
) -> serde_json::Value {
    aether_admin::observability::usage::admin_usage_resolve_request_preview_body(
        item,
        body_override,
    )
}

pub(super) async fn admin_usage_resolve_body_value(
    state: &AdminAppState<'_>,
    item: &StoredRequestUsageAudit,
    inline_body: Option<&Value>,
    field: UsageBodyField,
) -> Result<Option<Value>, GatewayError> {
    let resolved_ref_body = match item.body_ref(field) {
        Some(body_ref) => state.resolve_request_usage_body_ref(body_ref).await?,
        None => None,
    };
    Ok(admin_usage_body_value_from_sources(
        resolved_ref_body,
        inline_body,
    ))
}

fn admin_usage_body_value_from_sources(
    resolved_ref_body: Option<Value>,
    inline_body: Option<&Value>,
) -> Option<Value> {
    resolved_ref_body.or_else(|| inline_body.cloned())
}

pub(super) async fn admin_usage_resolve_request_preview_body_for_item(
    state: &AdminAppState<'_>,
    item: &StoredRequestUsageAudit,
    body_override: Option<serde_json::Value>,
) -> Result<serde_json::Value, GatewayError> {
    if let Some(body_override) = body_override {
        return Ok(body_override);
    }
    if let Some(body) = admin_usage_resolve_body_value(
        state,
        item,
        item.request_body.as_ref(),
        UsageBodyField::RequestBody,
    )
    .await?
    {
        return Ok(body);
    }
    Ok(admin_usage_resolve_request_preview_body(item, None))
}

pub(super) fn build_admin_usage_curl_response(
    item: &StoredRequestUsageAudit,
    url: Option<String>,
    headers_json: Option<Value>,
    headers: &BTreeMap<String, String>,
    body: &Value,
) -> Response<Body> {
    aether_admin::observability::usage::build_admin_usage_curl_response(
        item,
        url,
        headers_json,
        headers,
        body,
    )
}

#[cfg(test)]
mod tests {
    use super::admin_usage_body_value_from_sources;
    use serde_json::json;

    #[test]
    fn resolved_reference_body_wins_over_inline_fallback() {
        let inline_body = json!({
            "truncated": true,
            "reason": "usage_capture_limits_exceeded"
        });
        let ref_body = json!({
            "messages": [{"role": "user", "content": "real request body"}]
        });

        assert_eq!(
            admin_usage_body_value_from_sources(Some(ref_body.clone()), Some(&inline_body)),
            Some(ref_body)
        );
    }

    #[test]
    fn inline_body_is_used_when_reference_body_is_unavailable() {
        let inline_body = json!({
            "messages": [{"role": "user", "content": "fallback inline body"}]
        });

        assert_eq!(
            admin_usage_body_value_from_sources(None, Some(&inline_body)),
            Some(inline_body)
        );
    }
}

pub(super) fn build_admin_usage_detail_payload(
    item: &StoredRequestUsageAudit,
    users_by_id: &BTreeMap<String, aether_data::repository::users::StoredUserSummary>,
    api_key_names: &BTreeMap<String, String>,
    auth_user_reader_available: bool,
    auth_api_key_reader_available: bool,
    provider_key_name: Option<&str>,
    include_bodies: bool,
    request_body: Value,
    default_headers: &BTreeMap<String, String>,
) -> Value {
    aether_admin::observability::usage::build_admin_usage_detail_payload(
        item,
        users_by_id,
        api_key_names,
        auth_user_reader_available,
        auth_api_key_reader_available,
        provider_key_name,
        include_bodies,
        request_body,
        default_headers,
    )
}

pub(super) fn build_admin_usage_replay_plan_response(
    item: &StoredRequestUsageAudit,
    target_provider: &aether_data_contracts::repository::provider_catalog::StoredProviderCatalogProvider,
    target_endpoint: &StoredProviderCatalogEndpoint,
    target_api_key_id: Option<String>,
    request_body: Value,
    url: &str,
    headers: &BTreeMap<String, String>,
    same_provider: bool,
    same_endpoint: bool,
) -> Response<Body> {
    aether_admin::observability::usage::build_admin_usage_replay_plan_response(
        item,
        target_provider,
        target_endpoint,
        target_api_key_id,
        request_body,
        url,
        headers,
        same_provider,
        same_endpoint,
    )
}

pub(super) async fn build_admin_usage_replay_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&axum::body::Bytes>,
) -> Result<Response<Body>, GatewayError> {
    if !state.has_usage_data_reader() || !state.has_provider_catalog_data_reader() {
        return Ok(admin_usage_data_unavailable_response(
            ADMIN_USAGE_DATA_UNAVAILABLE_DETAIL,
        ));
    }

    let Some(usage_id) = admin_usage_id_from_action_path(&request_context.request_path, "/replay")
    else {
        return Ok(admin_usage_bad_request_response("usage_id 无效"));
    };

    let payload = match request_body {
        Some(body) if !body.is_empty() => {
            serde_json::from_slice::<AdminUsageReplayRequest>(body).unwrap_or_default()
        }
        _ => AdminUsageReplayRequest::default(),
    };

    let Some(item) = state.find_request_usage_by_id(&usage_id).await? else {
        return Ok((
            http::StatusCode::NOT_FOUND,
            Json(json!({ "detail": "Usage record not found" })),
        )
            .into_response());
    };

    let target_provider_id = payload
        .provider_id
        .clone()
        .or_else(|| item.provider_id.clone())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let Some(target_provider_id) = target_provider_id else {
        return Ok(admin_usage_bad_request_response(
            "Replay target provider is unavailable",
        ));
    };
    let Some(target_provider) = state
        .read_provider_catalog_providers_by_ids(std::slice::from_ref(&target_provider_id))
        .await?
        .into_iter()
        .next()
    else {
        return Ok((
            http::StatusCode::NOT_FOUND,
            Json(json!({ "detail": format!("Provider {target_provider_id} 不存在") })),
        )
            .into_response());
    };

    let requested_endpoint_id = payload
        .endpoint_id
        .clone()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let target_endpoint = if let Some(endpoint_id) = requested_endpoint_id.clone() {
        let Some(endpoint) = state
            .read_provider_catalog_endpoints_by_ids(std::slice::from_ref(&endpoint_id))
            .await?
            .into_iter()
            .next()
        else {
            return Ok((
                http::StatusCode::NOT_FOUND,
                Json(json!({ "detail": format!("Endpoint {endpoint_id} 不存在") })),
            )
                .into_response());
        };
        if endpoint.provider_id != target_provider.id {
            return Ok(admin_usage_bad_request_response(
                "Target endpoint does not belong to the target provider",
            ));
        }
        endpoint
    } else {
        let preferred_endpoint_id = item
            .provider_endpoint_id
            .clone()
            .filter(|_| item.provider_id.as_deref() == Some(target_provider.id.as_str()));
        if let Some(endpoint_id) = preferred_endpoint_id {
            if let Some(endpoint) = state
                .read_provider_catalog_endpoints_by_ids(std::slice::from_ref(&endpoint_id))
                .await?
                .into_iter()
                .find(|endpoint| endpoint.provider_id == target_provider.id)
            {
                endpoint
            } else {
                let mut endpoints = state
                    .list_provider_catalog_endpoints_by_provider_ids(std::slice::from_ref(
                        &target_provider.id,
                    ))
                    .await?;
                let preferred_api_format = item
                    .endpoint_api_format
                    .as_deref()
                    .or(item.api_format.as_deref())
                    .unwrap_or_default();
                endpoints
                    .iter()
                    .find(|endpoint| {
                        endpoint.is_active && endpoint.api_format == preferred_api_format
                    })
                    .cloned()
                    .or_else(|| {
                        endpoints
                            .iter()
                            .find(|endpoint| endpoint.is_active)
                            .cloned()
                    })
                    .or_else(|| endpoints.into_iter().next())
                    .ok_or_else(|| {
                        GatewayError::Internal("target provider has no endpoints".to_string())
                    })?
            }
        } else {
            let mut endpoints = state
                .list_provider_catalog_endpoints_by_provider_ids(std::slice::from_ref(
                    &target_provider.id,
                ))
                .await?;
            let preferred_api_format = item
                .endpoint_api_format
                .as_deref()
                .or(item.api_format.as_deref())
                .unwrap_or_default();
            endpoints
                .iter()
                .find(|endpoint| endpoint.is_active && endpoint.api_format == preferred_api_format)
                .cloned()
                .or_else(|| {
                    endpoints
                        .iter()
                        .find(|endpoint| endpoint.is_active)
                        .cloned()
                })
                .or_else(|| endpoints.into_iter().next())
                .ok_or_else(|| {
                    GatewayError::Internal("target provider has no endpoints".to_string())
                })?
        }
    };

    let same_provider = item.provider_id.as_deref() == Some(target_provider.id.as_str());
    let same_endpoint = item.provider_endpoint_id.as_deref() == Some(target_endpoint.id.as_str());
    let request_body =
        admin_usage_resolve_request_preview_body_for_item(state, &item, payload.body_override)
            .await?;

    let url = admin_usage_curl_url(state, &target_endpoint, &item);
    let headers = admin_usage_curl_headers();
    Ok(build_admin_usage_replay_plan_response(
        &item,
        &target_provider,
        &target_endpoint,
        payload.api_key_id.or(item.provider_api_key_id.clone()),
        request_body,
        &url,
        &headers,
        same_provider,
        same_endpoint,
    ))
}

pub(super) fn admin_usage_headers_from_value(
    value: &serde_json::Value,
) -> Option<BTreeMap<String, String>> {
    aether_admin::observability::usage::admin_usage_headers_from_value(value)
}

pub(super) fn admin_usage_curl_headers() -> BTreeMap<String, String> {
    aether_admin::observability::usage::admin_usage_curl_headers()
}

pub(super) fn admin_usage_curl_url(
    state: &AdminAppState<'_>,
    endpoint: &StoredProviderCatalogEndpoint,
    item: &StoredRequestUsageAudit,
) -> String {
    let api_format = item
        .endpoint_api_format
        .as_deref()
        .or(item.api_format.as_deref())
        .unwrap_or(endpoint.api_format.as_str());

    if let Some(custom_path) = endpoint
        .custom_path
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        return state
            .build_passthrough_path_url(&endpoint.base_url, custom_path, None, &[])
            .unwrap_or_else(|| endpoint.base_url.clone());
    }

    match api_format {
        value if value.starts_with("claude:") => {
            state.build_claude_messages_url(&endpoint.base_url, None)
        }
        value if value.starts_with("gemini:") => state
            .build_gemini_content_url(
                &endpoint.base_url,
                item.target_model.as_deref().unwrap_or(item.model.as_str()),
                item.is_stream,
                None,
            )
            .unwrap_or_else(|| endpoint.base_url.clone()),
        value if value.starts_with("openai:") => {
            state.build_openai_chat_url(&endpoint.base_url, None)
        }
        _ => endpoint.base_url.clone(),
    }
}

pub(super) fn admin_usage_build_curl_command(
    url: Option<&str>,
    headers: &BTreeMap<String, String>,
    body: Option<&serde_json::Value>,
) -> String {
    aether_admin::observability::usage::admin_usage_build_curl_command(url, headers, body)
}
