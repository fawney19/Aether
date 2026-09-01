use super::super::actions::admin_provider_ops_local_action_response;
use super::super::balance_cache::{
    admin_provider_ops_batch_balance_concurrency, admin_provider_ops_pending_balance_response,
    read_admin_provider_ops_balance_cache, spawn_admin_provider_ops_balance_refresh,
    store_admin_provider_ops_balance_cache, AdminProviderOpsBalanceCacheLookup,
};
use super::super::config::{
    admin_provider_ops_config_object, admin_provider_ops_remote_quota_worker_eligible,
};
use crate::handlers::admin::request::AdminAppState;
use crate::handlers::shared::system_config_bool;
use crate::GatewayError;
use aether_admin::system::ENABLE_PROVIDER_REMOTE_QUOTA_SYNC_CONFIG_KEY;
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use futures_util::stream::{self, StreamExt};
use serde_json::json;
use std::collections::HashMap;

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
    let providers_by_id = providers
        .into_iter()
        .map(|provider| (provider.id.clone(), provider))
        .collect::<HashMap<_, _>>();
    let remote_quota_sync_enabled = system_config_bool(
        state
            .app()
            .read_system_config_json_value(ENABLE_PROVIDER_REMOTE_QUOTA_SYNC_CONFIG_KEY)
            .await?
            .as_ref(),
        true,
    );
    let mut endpoints_by_provider = HashMap::<String, Vec<_>>::new();
    for endpoint in endpoints {
        endpoints_by_provider
            .entry(endpoint.provider_id.clone())
            .or_default()
            .push(endpoint);
    }

    let results = stream::iter(provider_ids.into_iter().map(|provider_id| {
        let provider = providers_by_id.get(&provider_id).cloned();
        let provider_endpoints = endpoints_by_provider
            .get(&provider_id)
            .cloned()
            .unwrap_or_default();
        async move {
            let remote_quota_refresh_owned_by_worker = remote_quota_sync_enabled
                && provider
                    .as_ref()
                    .is_some_and(admin_provider_ops_remote_quota_worker_eligible);
            let result = if provider
                .as_ref()
                .is_some_and(|provider| admin_provider_ops_config_object(provider).is_some())
            {
                match read_admin_provider_ops_balance_cache(state, &provider_id).await {
                    AdminProviderOpsBalanceCacheLookup::Hit(cached) => {
                        if !remote_quota_refresh_owned_by_worker {
                            spawn_admin_provider_ops_balance_refresh(state, &provider_id).await;
                        }
                        cached
                    }
                    AdminProviderOpsBalanceCacheLookup::Miss
                        if remote_quota_refresh_owned_by_worker =>
                    {
                        admin_provider_ops_pending_balance_response("暂无缓存的余额数据")
                    }
                    AdminProviderOpsBalanceCacheLookup::Miss => {
                        if state.runtime_state().is_memory() {
                            let payload = admin_provider_ops_local_action_response(
                                state,
                                &provider_id,
                                provider.as_ref(),
                                &provider_endpoints,
                                "query_balance",
                                None,
                            )
                            .await;
                            store_admin_provider_ops_balance_cache(state, &provider_id, &payload)
                                .await;
                            payload
                        } else {
                            spawn_admin_provider_ops_balance_refresh(state, &provider_id).await;
                            admin_provider_ops_pending_balance_response(
                                "余额数据加载中，请稍后刷新",
                            )
                        }
                    }
                    AdminProviderOpsBalanceCacheLookup::Unavailable
                        if remote_quota_refresh_owned_by_worker =>
                    {
                        admin_provider_ops_pending_balance_response("余额缓存不可用")
                    }
                    AdminProviderOpsBalanceCacheLookup::Unavailable => {
                        let payload = admin_provider_ops_local_action_response(
                            state,
                            &provider_id,
                            provider.as_ref(),
                            &provider_endpoints,
                            "query_balance",
                            None,
                        )
                        .await;
                        store_admin_provider_ops_balance_cache(state, &provider_id, &payload).await;
                        payload
                    }
                }
            } else {
                admin_provider_ops_local_action_response(
                    state,
                    &provider_id,
                    provider.as_ref(),
                    &provider_endpoints,
                    "query_balance",
                    None,
                )
                .await
            };
            (provider_id, result)
        }
    }))
    .buffer_unordered(admin_provider_ops_batch_balance_concurrency())
    .collect::<Vec<_>>()
    .await;

    let payload = results.into_iter().collect::<serde_json::Map<_, _>>();

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
