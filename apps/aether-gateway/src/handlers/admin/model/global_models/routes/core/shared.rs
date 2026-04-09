use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde::de::DeserializeOwned;
use serde_json::{json, Map, Value};
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) fn global_model_missing_response() -> Response<Body> {
    (
        http::StatusCode::NOT_FOUND,
        Json(json!({ "detail": "GlobalModel 不存在" })),
    )
        .into_response()
}

pub(super) fn global_model_not_found_response(global_model_id: &str) -> Response<Body> {
    (
        http::StatusCode::NOT_FOUND,
        Json(json!({ "detail": format!("GlobalModel {global_model_id} 不存在") })),
    )
        .into_response()
}

pub(super) fn not_found_detail_response(detail: impl Into<String>) -> Response<Body> {
    (
        http::StatusCode::NOT_FOUND,
        Json(json!({ "detail": detail.into() })),
    )
        .into_response()
}

pub(super) fn bad_request_response(detail: impl Into<String>) -> Response<Body> {
    (
        http::StatusCode::BAD_REQUEST,
        Json(json!({ "detail": detail.into() })),
    )
        .into_response()
}

pub(super) fn parse_required_json_body<T: DeserializeOwned>(
    request_body: Option<&Bytes>,
) -> Result<T, Response<Body>> {
    let Some(request_body) = request_body else {
        return Err(bad_request_response("请求体不能为空"));
    };
    serde_json::from_slice::<T>(request_body)
        .map_err(|_| bad_request_response("请求体必须是合法的 JSON 对象"))
}

pub(super) fn parse_required_json_value(
    request_body: Option<&Bytes>,
) -> Result<Value, Response<Body>> {
    let Some(request_body) = request_body else {
        return Err(bad_request_response("请求体不能为空"));
    };
    serde_json::from_slice::<Value>(request_body)
        .map_err(|_| bad_request_response("请求体必须是合法的 JSON 对象"))
}

pub(super) fn require_json_object(value: &Value) -> Result<Map<String, Value>, Response<Body>> {
    value
        .as_object()
        .cloned()
        .ok_or_else(|| bad_request_response("请求体必须是合法的 JSON 对象"))
}

pub(super) fn current_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}
