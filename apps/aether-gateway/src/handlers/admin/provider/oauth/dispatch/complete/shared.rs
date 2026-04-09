use super::super::super::errors::build_internal_control_error_response;
use super::super::super::state::parse_provider_oauth_callback_params;
use axum::{
    body::{Body, Bytes},
    http,
    response::Response,
};

pub(super) struct AdminProviderOAuthCompleteRequest {
    pub(super) callback_url: String,
    pub(super) name: Option<String>,
    pub(super) proxy_node_id: Option<String>,
}

pub(super) struct AdminProviderOAuthCompleteCallback {
    pub(super) code: String,
    pub(super) state_nonce: String,
}

pub(super) fn parse_admin_provider_oauth_callback_url(
    raw_payload: &serde_json::Map<String, serde_json::Value>,
) -> Result<String, Response<Body>> {
    raw_payload
        .get("callback_url")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                "callback_url 缺少 code/state",
            )
        })
}

pub(super) fn extract_admin_provider_oauth_code(
    params: &std::collections::BTreeMap<String, String>,
) -> Result<String, Response<Body>> {
    params
        .get("code")
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                "callback_url 缺少 code/state",
            )
        })
}

pub(super) fn extract_admin_provider_oauth_state(
    params: &std::collections::BTreeMap<String, String>,
) -> Result<String, Response<Body>> {
    params
        .get("state")
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                "callback_url 缺少 code/state",
            )
        })
}

pub(super) fn parse_admin_provider_oauth_complete_request_body(
    request_body: Option<&Bytes>,
) -> Result<AdminProviderOAuthCompleteRequest, Response<Body>> {
    let Some(request_body) = request_body else {
        return Err(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "请求体必须是合法的 JSON 对象",
        ));
    };
    let raw_payload = match serde_json::from_slice::<serde_json::Value>(request_body) {
        Ok(serde_json::Value::Object(map)) => map,
        _ => {
            return Err(build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                "请求体必须是合法的 JSON 对象",
            ));
        }
    };
    let callback_url = parse_admin_provider_oauth_callback_url(&raw_payload)?;

    Ok(AdminProviderOAuthCompleteRequest {
        callback_url,
        name: raw_payload
            .get("name")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        proxy_node_id: raw_payload
            .get("proxy_node_id")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
    })
}

pub(super) fn parse_admin_provider_oauth_complete_callback(
    callback_url: &str,
) -> Result<AdminProviderOAuthCompleteCallback, Response<Body>> {
    let params = parse_provider_oauth_callback_params(callback_url);
    let code = extract_admin_provider_oauth_code(&params)?;
    let state_nonce = extract_admin_provider_oauth_state(&params)?;

    Ok(AdminProviderOAuthCompleteCallback { code, state_nonce })
}
