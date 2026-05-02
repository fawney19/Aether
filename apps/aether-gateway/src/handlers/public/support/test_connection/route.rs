use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

use super::test_connection_shared::select_test_connection_provider;
use super::{
    provider_catalog_key_supports_format, query_param_value, AppState, GatewayPublicRequestContext,
};

pub(super) async fn maybe_build_local_test_connection_route_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
) -> Option<Response<Body>> {
    if request_context.request_path != "/v1/test-connection" {
        return None;
    }
    if !state.has_provider_catalog_data_reader() {
        return None;
    }

    let provider_query =
        query_param_value(request_context.request_query_string.as_deref(), "provider");
    let model = query_param_value(request_context.request_query_string.as_deref(), "model")
        .unwrap_or_else(|| "claude-3-haiku-20240307".to_string());
    let requested_api_format = query_param_value(
        request_context.request_query_string.as_deref(),
        "api_format",
    );

    let providers = state
        .list_provider_catalog_providers(true)
        .await
        .ok()
        .unwrap_or_default();
    let Some(provider) = select_test_connection_provider(providers, provider_query.as_deref())
    else {
        return Some(
            (
                http::StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"detail": "No active provider available"})),
            )
                .into_response(),
        );
    };

    let provider_ids = vec![provider.id.clone()];
    let active_endpoints = state
        .list_provider_catalog_endpoints_by_provider_ids(&provider_ids)
        .await
        .ok()
        .unwrap_or_default()
        .into_iter()
        .filter(|endpoint| endpoint.is_active)
        .collect::<Vec<_>>();
    if active_endpoints.is_empty() {
        return Some(
            (
                http::StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"detail": "Provider has no active endpoints"})),
            )
                .into_response(),
        );
    }

    let (endpoint, format_value) = if let Some(api_format) = requested_api_format.as_deref() {
        let Some(endpoint) = active_endpoints
            .into_iter()
            .find(|endpoint| endpoint.api_format.eq_ignore_ascii_case(api_format))
        else {
            return Some(
                (
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({
                        "detail": format!(
                            "Provider has no active endpoint for api_format={api_format}"
                        ),
                    })),
                )
                    .into_response(),
            );
        };
        (endpoint, api_format.to_string())
    } else {
        let endpoint = active_endpoints.into_iter().next()?;
        let format_value = if endpoint.api_format.trim().is_empty() {
            "claude:messages".to_string()
        } else {
            crate::ai_serving::normalize_api_format_alias(&endpoint.api_format)
        };
        (endpoint, format_value)
    };

    let format_value = crate::ai_serving::normalize_api_format_alias(&format_value);
    if !matches!(
        format_value.as_str(),
        "openai:chat" | "claude:messages" | "gemini:generate_content"
    ) {
        return None;
    }

    let active_keys = state
        .list_provider_catalog_keys_by_provider_ids(&provider_ids)
        .await
        .ok()
        .unwrap_or_default()
        .into_iter()
        .filter(|key| key.is_active)
        .collect::<Vec<_>>();
    if active_keys.is_empty() {
        return Some(
            (
                http::StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"detail": "Provider has no active api keys"})),
            )
                .into_response(),
        );
    }
    let Some(key) = active_keys
        .iter()
        .find(|key| {
            provider_catalog_key_supports_format(
                key,
                provider.provider_type.as_str(),
                &format_value,
            )
        })
        .cloned()
        .or_else(|| active_keys.into_iter().next())
    else {
        return Some(
            (
                http::StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"detail": "Provider has no active api keys"})),
            )
                .into_response(),
        );
    };

    let transport = match state
        .read_provider_transport_snapshot(&provider.id, &endpoint.id, &key.id)
        .await
    {
        Ok(Some(transport)) => transport,
        Ok(None) => return None,
        Err(_) => {
            return Some(
                (
                    http::StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({"detail": "Provider transport snapshot unavailable"})),
                )
                    .into_response(),
            )
        }
    };

    if transport.provider.proxy.is_some()
        || transport.endpoint.proxy.is_some()
        || transport.key.proxy.is_some()
        || crate::provider_transport::resolve_transport_tls_profile(&transport).is_some()
    {
        return None;
    }

    let mut provider_request_body = match format_value.as_str() {
        "openai:chat" | "claude:messages" => json!({
            "model": model,
            "messages": [{"role": "user", "content": "Health check"}],
            "max_tokens": 5,
        }),
        "gemini:generate_content" => json!({
            "contents": [{
                "role": "user",
                "parts": [{"text": "Health check"}],
            }],
            "generationConfig": {
                "maxOutputTokens": 5,
            },
        }),
        _ => return None,
    };
    if !crate::provider_transport::apply_local_body_rules(
        &mut provider_request_body,
        transport.endpoint.body_rules.as_ref(),
        None,
    ) {
        return None;
    }

    let oauth_auth = match format_value.as_str() {
        "openai:chat" | "claude:messages" => {
            match state.resolve_local_oauth_request_auth(&transport).await {
                Ok(Some(crate::provider_transport::LocalResolvedOAuthRequestAuth::Header {
                    name,
                    value,
                })) => Some((name, value)),
                _ => None,
            }
        }
        _ => None,
    };

    let auth = match format_value.as_str() {
        "openai:chat" => {
            crate::provider_transport::auth::resolve_local_openai_bearer_auth(&transport)
                .or(oauth_auth.clone())
        }
        "claude:messages" => {
            crate::provider_transport::auth::resolve_local_standard_auth(&transport)
                .or(oauth_auth.clone())
        }
        "gemini:generate_content" => {
            crate::provider_transport::auth::resolve_local_gemini_auth(&transport)
        }
        _ => None,
    };
    let Some((auth_header, auth_value)) = auth else {
        return None;
    };
    let uses_vertex_query_auth = crate::provider_transport::uses_vertex_api_key_query_auth(
        &transport,
        format_value.as_str(),
    );

    let upstream_url = crate::provider_transport::build_transport_request_url(
        &transport,
        crate::provider_transport::TransportRequestUrlParams {
            provider_api_format: format_value.as_str(),
            mapped_model: Some(model.as_str()),
            upstream_is_stream: false,
            request_query: None,
            kiro_api_region: None,
        },
    );
    let Some(upstream_url) = upstream_url else {
        return None;
    };

    let mut provider_request_headers =
        BTreeMap::from([("content-type".to_string(), "application/json".to_string())]);
    if !auth_header.trim().is_empty() && !auth_value.trim().is_empty() {
        provider_request_headers.insert(auth_header.clone(), auth_value.clone());
    }
    if uses_vertex_query_auth {
        provider_request_headers.remove("x-goog-api-key");
    }
    let protected_headers = if uses_vertex_query_auth || auth_value.trim().is_empty() {
        vec!["content-type"]
    } else {
        vec![auth_header.as_str(), "content-type"]
    };
    if !crate::provider_transport::apply_local_header_rules(
        &mut provider_request_headers,
        transport.endpoint.header_rules.as_ref(),
        &protected_headers,
        &provider_request_body,
        None,
    ) {
        return None;
    }
    if !uses_vertex_query_auth {
        crate::provider_transport::ensure_upstream_auth_header(
            &mut provider_request_headers,
            &auth_header,
            &auth_value,
        );
    }

    let mut upstream_request = state.client.post(&upstream_url);
    for (name, value) in &provider_request_headers {
        upstream_request = upstream_request.header(name, value);
    }
    if let Some(total_ms) =
        crate::provider_transport::resolve_transport_execution_timeouts(&transport)
            .and_then(|timeouts| timeouts.total_ms.or(timeouts.first_byte_ms))
    {
        upstream_request = upstream_request.timeout(Duration::from_millis(total_ms));
    }

    let response = match upstream_request.json(&provider_request_body).send().await {
        Ok(response) => response,
        Err(error) => {
            return Some(
                (
                    http::StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({"detail": error.to_string()})),
                )
                    .into_response(),
            )
        }
    };

    let status = response.status();
    let response_json = response.json::<serde_json::Value>().await.ok();
    if !status.is_success() {
        let detail = response_json
            .as_ref()
            .and_then(|value| value.get("error"))
            .and_then(|value| value.get("message").or(Some(value)))
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| format!("upstream returned HTTP {status}"));
        return Some(
            (
                http::StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "detail": detail })),
            )
                .into_response(),
        );
    }

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string());
    let response_id = response_json
        .as_ref()
        .and_then(|value| value.get("id"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");

    Some(
        Json(json!({
            "status": "success",
            "provider": provider.name,
            "endpoint_id": endpoint.id,
            "api_format": format_value,
            "timestamp": timestamp,
            "response_id": response_id,
        }))
        .into_response(),
    )
}
