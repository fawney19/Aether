use axum::body::Bytes;
use axum::response::{IntoResponse, Response};
use serde_json::json;

pub(super) fn parse_admin_provider_query_body(
    request_body: Option<&Bytes>,
) -> Result<serde_json::Value, Response<axum::body::Body>> {
    let Some(raw_body) = request_body else {
        return Ok(json!({}));
    };
    if raw_body.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_slice::<serde_json::Value>(raw_body).map_err(|_| {
        super::response::build_admin_provider_query_bad_request_response(
            super::response::ADMIN_PROVIDER_QUERY_INVALID_JSON_DETAIL,
        )
    })
}

pub(super) fn provider_query_extract_provider_id(payload: &serde_json::Value) -> Option<String> {
    payload
        .get("provider_id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub(super) fn provider_query_extract_api_key_id(payload: &serde_json::Value) -> Option<String> {
    payload
        .get("api_key_id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub(super) fn provider_query_extract_model(payload: &serde_json::Value) -> Option<String> {
    payload
        .get("model")
        .or_else(|| payload.get("model_name"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub(super) fn provider_query_extract_failover_models(payload: &serde_json::Value) -> Vec<String> {
    if let Some(items) = payload
        .get("failover_models")
        .or_else(|| payload.get("models"))
        .and_then(serde_json::Value::as_array)
    {
        return items
            .iter()
            .filter_map(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
    }

    provider_query_extract_model(payload)
        .into_iter()
        .collect::<Vec<_>>()
}

pub(super) fn provider_query_extract_request_id(payload: &serde_json::Value) -> Option<String> {
    payload
        .get("request_id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub(super) fn provider_query_payload_keys(payload: &serde_json::Value) -> Vec<String> {
    let Some(object) = payload.as_object() else {
        return Vec::new();
    };
    let mut keys = object.keys().cloned().collect::<Vec<_>>();
    keys.sort();
    keys
}
