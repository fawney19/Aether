use super::super::config::build_admin_provider_ops_saved_config_value;
use super::super::support::AdminProviderOpsSaveConfigRequest;
use crate::handlers::admin::request::AdminAppState;
use crate::GatewayError;
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) async fn handle_admin_provider_ops_save_config(
    state: &AdminAppState<'_>,
    provider_id: &str,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let payload = match parse_json_object_payload::<AdminProviderOpsSaveConfigRequest>(request_body)
    {
        Ok(payload) => payload,
        Err(response) => return Ok(Some(response)),
    };
    let provider_ids = [provider_id.to_string()];
    let Some(existing_provider) = state
        .read_provider_catalog_providers_by_ids(&provider_ids)
        .await?
        .into_iter()
        .next()
    else {
        return Ok(Some(provider_not_found_response()));
    };

    let provider_ops_config =
        match build_admin_provider_ops_saved_config_value(state, &existing_provider, payload) {
            Ok(config) => config,
            Err(detail) => return Ok(Some(bad_request_detail_response(&detail))),
        };

    let mut updated_provider = existing_provider.clone();
    let mut provider_config = updated_provider
        .config
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default();
    provider_config.insert("provider_ops".to_string(), provider_ops_config);
    updated_provider.config = Some(serde_json::Value::Object(provider_config));
    updated_provider.updated_at_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs());
    let Some(_updated) = state
        .update_provider_catalog_provider(&updated_provider)
        .await?
    else {
        return Ok(None);
    };

    Ok(Some(
        Json(json!({
            "success": true,
            "message": "配置保存成功",
        }))
        .into_response(),
    ))
}

pub(super) async fn handle_admin_provider_ops_delete_config(
    state: &AdminAppState<'_>,
    provider_id: &str,
) -> Result<Option<Response<Body>>, GatewayError> {
    let provider_ids = [provider_id.to_string()];
    let Some(existing_provider) = state
        .read_provider_catalog_providers_by_ids(&provider_ids)
        .await?
        .into_iter()
        .next()
    else {
        return Ok(Some(provider_not_found_response()));
    };

    let mut updated_provider = existing_provider.clone();
    let mut provider_config = updated_provider
        .config
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default();
    if provider_config.remove("provider_ops").is_some() {
        updated_provider.config = Some(serde_json::Value::Object(provider_config));
        updated_provider.updated_at_unix_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_secs());
        let Some(_updated) = state
            .update_provider_catalog_provider(&updated_provider)
            .await?
        else {
            return Ok(None);
        };
    }

    Ok(Some(
        Json(json!({
            "success": true,
            "message": "配置已删除",
        }))
        .into_response(),
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
        Json(json!({ "detail": detail })),
    )
        .into_response()
}

fn provider_not_found_response() -> Response<Body> {
    (
        http::StatusCode::NOT_FOUND,
        Json(json!({ "detail": "Provider 不存在" })),
    )
        .into_response()
}
