use std::collections::{BTreeMap, BTreeSet};

use aether_data_contracts::repository::global_models::{
    PublicCatalogModelListQuery, PublicGlobalModelQuery, StoredPublicGlobalModel,
    StoredPublicGlobalModelPage,
};
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

use super::{
    build_admin_endpoint_health_status_payload, build_auth_error_response, query_param_value,
    resolve_authenticated_local_user, AppState, GatewayPublicRequestContext,
    USERS_ME_AVAILABLE_MODELS_FETCH_LIMIT,
};

const USERS_ME_MODEL_CATALOG_UNAVAILABLE_DETAIL: &str = "用户模型目录暂不可用";
const USERS_ME_PROVIDER_CATALOG_UNAVAILABLE_DETAIL: &str = "用户提供商目录暂不可用";
const USERS_ME_ENDPOINT_STATUS_UNAVAILABLE_DETAIL: &str = "用户端点健康数据暂不可用";

fn build_users_me_available_model_payload(model: StoredPublicGlobalModel) -> serde_json::Value {
    json!({
        "id": model.id,
        "name": model.name,
        "display_name": model.display_name,
        "is_active": model.is_active,
        "default_price_per_request": model.default_price_per_request,
        "default_tiered_pricing": model.default_tiered_pricing,
        "supported_capabilities": model.supported_capabilities,
        "config": model.config,
        "usage_count": model.usage_count,
    })
}

fn parse_users_me_available_models_query(query: Option<&str>) -> (usize, usize, Option<String>) {
    let skip = query_param_value(query, "skip")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let limit = query_param_value(query, "limit")
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| (1..=1000).contains(value))
        .unwrap_or(100);
    let search = query_param_value(query, "search");
    (skip, limit, search)
}

fn users_me_allowed_provider_names(
    user: &aether_data::repository::users::StoredUserAuthRecord,
) -> Option<BTreeSet<String>> {
    if user.role.eq_ignore_ascii_case("admin") {
        return None;
    }

    user.allowed_providers
        .as_ref()
        .map(|providers| {
            providers
                .iter()
                .map(|value| value.trim().to_ascii_lowercase())
                .filter(|value| !value.is_empty())
                .collect::<BTreeSet<_>>()
        })
        .filter(|providers| !providers.is_empty())
}

async fn resolve_users_me_allowed_global_model_ids(
    state: &AppState,
    user: &aether_data::repository::users::StoredUserAuthRecord,
) -> Result<Option<BTreeSet<String>>, Response<Body>> {
    let Some(allowed_providers) = user
        .allowed_providers
        .as_ref()
        .filter(|providers| !providers.is_empty())
    else {
        return Ok(None);
    };

    if !state.has_provider_catalog_data_reader() {
        return Err(build_auth_error_response(
            http::StatusCode::SERVICE_UNAVAILABLE,
            USERS_ME_PROVIDER_CATALOG_UNAVAILABLE_DETAIL,
            false,
        ));
    }

    let allowed_provider_names: BTreeSet<String> = allowed_providers
        .iter()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .collect::<BTreeSet<_>>();
    let providers = match state.list_provider_catalog_providers(true).await {
        Ok(value) => value,
        Err(err) => {
            return Err(build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("user provider lookup failed: {err:?}"),
                false,
            ))
        }
    };
    let provider_ids = providers
        .into_iter()
        .filter(|provider| {
            allowed_provider_names.contains(&provider.id.to_ascii_lowercase())
                || allowed_provider_names.contains(&provider.name.to_ascii_lowercase())
        })
        .map(|provider| provider.id)
        .collect::<Vec<_>>();
    if provider_ids.is_empty() {
        return Ok(Some(BTreeSet::new()));
    }

    let refs = match state
        .list_active_global_model_ids_by_provider_ids(&provider_ids)
        .await
    {
        Ok(value) => value,
        Err(err) => {
            return Err(build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("user provider model lookup failed: {err:?}"),
                false,
            ))
        }
    };
    Ok(Some(
        refs.into_iter()
            .map(|entry| entry.global_model_id)
            .collect::<BTreeSet<_>>(),
    ))
}

pub(super) async fn handle_users_me_available_models(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    if !state.has_global_model_data_reader() {
        return build_auth_error_response(
            http::StatusCode::SERVICE_UNAVAILABLE,
            USERS_ME_MODEL_CATALOG_UNAVAILABLE_DETAIL,
            false,
        );
    }

    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let (skip, limit, search) =
        parse_users_me_available_models_query(request_context.request_query_string.as_deref());

    let provider_model_ids = if auth.user.role.eq_ignore_ascii_case("admin") {
        None
    } else {
        match resolve_users_me_allowed_global_model_ids(state, &auth.user).await {
            Ok(value) => value,
            Err(response) => return response,
        }
    };
    let allowed_models: Option<BTreeSet<String>> = if auth.user.role.eq_ignore_ascii_case("admin") {
        None
    } else {
        auth.user
            .allowed_models
            .as_ref()
            .map(|models: &Vec<String>| {
                models
                    .iter()
                    .map(|value: &String| value.trim().to_ascii_lowercase())
                    .filter(|value: &String| !value.is_empty())
                    .collect::<BTreeSet<_>>()
            })
            .filter(|models: &BTreeSet<String>| !models.is_empty())
    };

    let page = if provider_model_ids.is_none() && allowed_models.is_none() {
        match state
            .list_public_global_models(&PublicGlobalModelQuery {
                offset: skip,
                limit,
                is_active: Some(true),
                search,
            })
            .await
        {
            Ok(value) => value,
            Err(err) => {
                return build_auth_error_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("available model lookup failed: {err:?}"),
                    false,
                )
            }
        }
    } else {
        let page = match state
            .list_public_global_models(&PublicGlobalModelQuery {
                offset: 0,
                limit: USERS_ME_AVAILABLE_MODELS_FETCH_LIMIT,
                is_active: Some(true),
                search,
            })
            .await
        {
            Ok(value) => value,
            Err(err) => {
                return build_auth_error_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("available model lookup failed: {err:?}"),
                    false,
                )
            }
        };

        let filtered = page
            .items
            .into_iter()
            .filter(|model| {
                allowed_models
                    .as_ref()
                    .is_none_or(|allowed: &BTreeSet<String>| {
                        allowed.contains(&model.name.to_ascii_lowercase())
                    })
            })
            .filter(|model| {
                provider_model_ids
                    .as_ref()
                    .is_none_or(|allowed: &BTreeSet<String>| allowed.contains(&model.id))
            })
            .collect::<Vec<_>>();
        let total = filtered.len();
        let items = filtered
            .into_iter()
            .skip(skip)
            .take(limit)
            .collect::<Vec<_>>();
        StoredPublicGlobalModelPage { items, total }
    };

    Json(json!({
        "models": page
            .items
            .into_iter()
            .map(build_users_me_available_model_payload)
            .collect::<Vec<_>>(),
        "total": page.total,
    }))
    .into_response()
}

pub(super) async fn handle_users_me_providers_get(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    if !state.has_provider_catalog_data_reader() {
        return build_auth_error_response(
            http::StatusCode::SERVICE_UNAVAILABLE,
            USERS_ME_PROVIDER_CATALOG_UNAVAILABLE_DETAIL,
            false,
        );
    }

    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let allowed_provider_names = users_me_allowed_provider_names(&auth.user);

    let mut providers = match state.list_provider_catalog_providers(true).await {
        Ok(value) => value,
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("user provider lookup failed: {err:?}"),
                false,
            )
        }
    };
    if let Some(allowed_provider_names) = allowed_provider_names.as_ref() {
        providers.retain(|provider| {
            allowed_provider_names.contains(&provider.id.to_ascii_lowercase())
                || allowed_provider_names.contains(&provider.name.to_ascii_lowercase())
        });
    }
    providers.sort_by(|left, right| {
        left.provider_priority
            .cmp(&right.provider_priority)
            .then_with(|| left.name.cmp(&right.name))
    });

    let provider_ids = providers
        .iter()
        .map(|provider| provider.id.clone())
        .collect::<Vec<_>>();
    let endpoints = match state
        .list_provider_catalog_endpoints_by_provider_ids(&provider_ids)
        .await
    {
        Ok(value) => value,
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("user provider endpoint lookup failed: {err:?}"),
                false,
            )
        }
    };
    let mut endpoints_by_provider = BTreeMap::<String, Vec<serde_json::Value>>::new();
    for endpoint in endpoints {
        endpoints_by_provider
            .entry(endpoint.provider_id)
            .or_default()
            .push(json!({
                "id": endpoint.id,
                "api_format": endpoint.api_format,
                "base_url": endpoint.base_url,
                "is_active": endpoint.is_active,
            }));
    }

    let mut models_by_provider = BTreeMap::<String, Vec<serde_json::Value>>::new();
    if state.has_global_model_data_reader() {
        for provider_id in &provider_ids {
            let models = match state
                .list_public_catalog_models(&PublicCatalogModelListQuery {
                    provider_id: Some(provider_id.clone()),
                    offset: 0,
                    limit: 1000,
                })
                .await
            {
                Ok(value) => value,
                Err(err) => {
                    return build_auth_error_response(
                        http::StatusCode::INTERNAL_SERVER_ERROR,
                        format!("user provider model lookup failed: {err:?}"),
                        false,
                    )
                }
            };
            models_by_provider.insert(
                provider_id.clone(),
                models
                    .into_iter()
                    .map(|model| {
                        json!({
                            "id": model.id,
                            "name": model.name,
                            "display_name": model.display_name,
                            "input_price_per_1m": model.input_price_per_1m,
                            "output_price_per_1m": model.output_price_per_1m,
                            "cache_creation_price_per_1m": model.cache_creation_price_per_1m,
                            "cache_read_price_per_1m": model.cache_read_price_per_1m,
                            "supports_vision": model.supports_vision,
                            "supports_function_calling": model.supports_function_calling,
                            "supports_streaming": model.supports_streaming,
                        })
                    })
                    .collect::<Vec<_>>(),
            );
        }
    }

    Json(
        providers
            .into_iter()
            .map(|provider| {
                let provider_id = provider.id.clone();
                let description = provider
                    .config
                    .as_ref()
                    .and_then(|value| value.get("description"))
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned);
                json!({
                    "id": provider_id.clone(),
                    "name": provider.name,
                    "description": description,
                    "provider_priority": provider.provider_priority,
                    "endpoints": endpoints_by_provider.remove(&provider_id).unwrap_or_default(),
                    "models": models_by_provider.remove(&provider_id).unwrap_or_default(),
                })
            })
            .collect::<Vec<_>>(),
    )
    .into_response()
}

pub(super) async fn handle_users_me_endpoint_status_get(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(_) => {}
        Err(response) => return response,
    };

    let Some(payload) =
        build_admin_endpoint_health_status_payload(&crate::admin_api::AdminAppState::new(state), 6)
            .await
    else {
        return build_auth_error_response(
            http::StatusCode::SERVICE_UNAVAILABLE,
            USERS_ME_ENDPOINT_STATUS_UNAVAILABLE_DETAIL,
            false,
        );
    };
    let Some(items) = payload.as_array() else {
        return build_auth_error_response(
            http::StatusCode::INTERNAL_SERVER_ERROR,
            "endpoint status payload malformed",
            false,
        );
    };

    Json(serde_json::Value::Array(
        items.iter()
            .map(|item| {
                json!({
                    "api_format": item.get("api_format").cloned().unwrap_or(serde_json::Value::Null),
                    "display_name": item.get("display_name").cloned().unwrap_or(serde_json::Value::Null),
                    "health_score": item.get("health_score").cloned().unwrap_or(serde_json::Value::Null),
                    "timeline": item.get("timeline").cloned().unwrap_or_else(|| json!([])),
                    "time_range_start": item.get("time_range_start").cloned().unwrap_or(serde_json::Value::Null),
                    "time_range_end": item.get("time_range_end").cloned().unwrap_or(serde_json::Value::Null),
                })
            })
            .collect(),
    ))
    .into_response()
}
