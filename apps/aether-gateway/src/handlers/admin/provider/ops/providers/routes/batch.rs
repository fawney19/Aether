use super::super::actions::admin_provider_ops_local_action_response;
use crate::handlers::admin::request::AdminAppState;
use crate::GatewayError;
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(super) async fn handle_admin_provider_ops_batch_balance(
    state: &AdminAppState<'_>,
    request_body: Option<&Bytes>,
) -> Result<Response<Body>, GatewayError> {
    let requested_provider_ids = match request_body {
        Some(body) if !body.is_empty() => match parse_provider_ids(body) {
            Ok(provider_ids) => Some(provider_ids),
            Err(response) => return Ok(response),
        },
        _ => None,
    };

    let provider_ids = if let Some(provider_ids) = requested_provider_ids {
        provider_ids
    } else {
        state
            .list_provider_catalog_providers(true)
            .await?
            .into_iter()
            .filter(|provider| {
                provider
                    .config
                    .as_ref()
                    .and_then(serde_json::Value::as_object)
                    .is_some_and(|config| config.contains_key("provider_ops"))
            })
            .map(|provider| provider.id)
            .collect::<Vec<_>>()
    };

    if provider_ids.is_empty() {
        return Ok(Json(json!({})).into_response());
    }

    let providers = state
        .read_provider_catalog_providers_by_ids(&provider_ids)
        .await?;
    let endpoints = state
        .list_provider_catalog_endpoints_by_provider_ids(&provider_ids)
        .await?;
    let mut payload = serde_json::Map::new();
    for provider_id in &provider_ids {
        let provider = providers
            .iter()
            .find(|provider| provider.id == *provider_id);
        let provider_endpoints = endpoints
            .iter()
            .filter(|endpoint| endpoint.provider_id == *provider_id)
            .cloned()
            .collect::<Vec<_>>();
        let result = admin_provider_ops_local_action_response(
            state,
            provider_id,
            provider,
            &provider_endpoints,
            "query_balance",
            None,
        )
        .await;
        payload.insert(provider_id.clone(), result);
    }

    Ok(Json(serde_json::Value::Object(payload)).into_response())
}

fn parse_provider_ids(body: &Bytes) -> Result<Vec<String>, Response<Body>> {
    let raw_value = serde_json::from_slice::<serde_json::Value>(body).map_err(|_| {
        (
            http::StatusCode::BAD_REQUEST,
            Json(json!({ "detail": "请求体必须是 provider_id 数组" })),
        )
            .into_response()
    })?;

    let items = raw_value
        .as_array()
        .or_else(|| {
            raw_value
                .get("provider_ids")
                .and_then(serde_json::Value::as_array)
        })
        .ok_or_else(|| {
            (
                http::StatusCode::BAD_REQUEST,
                Json(json!({ "detail": "请求体必须是 provider_id 数组" })),
            )
                .into_response()
        })?;

    Ok(items
        .iter()
        .filter_map(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}
