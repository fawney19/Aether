use super::super::balance_cache::clear_admin_provider_ops_balance_cache;
use super::super::config::build_admin_provider_ops_saved_config_value;
use super::super::support::AdminProviderOpsSaveConfigRequest;
use crate::handlers::admin::request::AdminAppState;
use crate::GatewayError;
use aether_data_contracts::repository::provider_catalog::ProviderCatalogProviderConfigCasUpdate;
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

const ADMIN_PROVIDER_OPS_CONFIG_SAVE_RETRIES: usize = 8;

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
    let Some(mut existing_provider) = state
        .read_provider_catalog_providers_by_ids(&provider_ids)
        .await?
        .into_iter()
        .next()
    else {
        return Ok(Some(provider_not_found_response()));
    };

    let mut saved = false;
    for _ in 0..ADMIN_PROVIDER_OPS_CONFIG_SAVE_RETRIES {
        let snapshot = match build_admin_provider_ops_saved_config_value(
            state,
            &existing_provider,
            payload.clone(),
        )
        .await
        {
            Ok(snapshot) => snapshot,
            Err(detail) => return Ok(Some(bad_request_detail_response(&detail))),
        };
        let mut provider_config = snapshot
            .provider
            .config
            .as_ref()
            .and_then(serde_json::Value::as_object)
            .cloned()
            .unwrap_or_default();
        provider_config.insert("provider_ops".to_string(), snapshot.provider_ops_config);
        let update = ProviderCatalogProviderConfigCasUpdate {
            provider_id: snapshot.provider.id.clone(),
            expected_config: snapshot.provider.config.clone(),
            config: Some(serde_json::Value::Object(provider_config)),
        };
        if state
            .compare_and_swap_provider_catalog_provider_config(&update)
            .await?
        {
            saved = true;
            break;
        }
        let Some(current) = state
            .read_provider_catalog_providers_by_ids(&provider_ids)
            .await?
            .into_iter()
            .next()
        else {
            return Ok(Some(provider_not_found_response()));
        };
        existing_provider = current;
    }
    if !saved {
        return Ok(Some(
            (
                http::StatusCode::CONFLICT,
                Json(json!({ "detail": "Provider Ops 配置并发更新冲突，请重试" })),
            )
                .into_response(),
        ));
    }
    clear_admin_provider_ops_balance_cache(state, provider_id).await;

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
    let Some(mut existing_provider) = state
        .read_provider_catalog_providers_by_ids(&provider_ids)
        .await?
        .into_iter()
        .next()
    else {
        return Ok(Some(provider_not_found_response()));
    };

    let mut removed = false;
    for _ in 0..ADMIN_PROVIDER_OPS_CONFIG_SAVE_RETRIES {
        let mut provider_config = existing_provider
            .config
            .as_ref()
            .and_then(serde_json::Value::as_object)
            .cloned()
            .unwrap_or_default();
        if provider_config.remove("provider_ops").is_none() {
            break;
        }
        let update = ProviderCatalogProviderConfigCasUpdate {
            provider_id: existing_provider.id.clone(),
            expected_config: existing_provider.config.clone(),
            config: Some(serde_json::Value::Object(provider_config)),
        };
        if state
            .compare_and_swap_provider_catalog_provider_config(&update)
            .await?
        {
            removed = true;
            break;
        }
        let Some(current) = state
            .read_provider_catalog_providers_by_ids(&provider_ids)
            .await?
            .into_iter()
            .next()
        else {
            return Ok(Some(provider_not_found_response()));
        };
        existing_provider = current;
    }
    if admin_provider_ops_config_still_exists(&existing_provider) && !removed {
        return Ok(Some(
            (
                http::StatusCode::CONFLICT,
                Json(json!({ "detail": "Provider Ops 配置并发更新冲突，请重试" })),
            )
                .into_response(),
        ));
    }
    if removed {
        clear_admin_provider_ops_balance_cache(state, provider_id).await;
    }

    Ok(Some(
        Json(json!({
            "success": true,
            "message": "配置已删除",
        }))
        .into_response(),
    ))
}

fn admin_provider_ops_config_still_exists(
    provider: &aether_data_contracts::repository::provider_catalog::StoredProviderCatalogProvider,
) -> bool {
    provider
        .config
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .is_some_and(|config| config.contains_key("provider_ops"))
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
