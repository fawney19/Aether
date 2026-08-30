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
    build_admin_endpoint_health_status_payload, build_auth_error_response,
    filter_eligible_model_rows_for_state, query_param_value, resolve_authenticated_local_user,
    sanitize_public_model_config_for_user, AppState, GatewayPublicRequestContext,
    USERS_ME_AVAILABLE_MODELS_FETCH_LIMIT, USERS_ME_CATALOG_API_FORMATS,
};

const USERS_ME_MODEL_CATALOG_UNAVAILABLE_DETAIL: &str = "用户模型目录暂不可用";
const USERS_ME_PROVIDER_CATALOG_UNAVAILABLE_DETAIL: &str = "用户提供商目录暂不可用";
const USERS_ME_ENDPOINT_STATUS_UNAVAILABLE_DETAIL: &str = "用户端点健康数据暂不可用";

fn build_users_me_available_model_payload(
    model: StoredPublicGlobalModel,
    hide_mapping_config: bool,
) -> serde_json::Value {
    let config = if hide_mapping_config {
        sanitize_public_model_config_for_user(model.config)
    } else {
        model.config
    };
    json!({
        "id": model.id,
        "name": model.name,
        "display_name": model.display_name,
        "is_active": model.is_active,
        "default_price_per_request": model.default_price_per_request,
        "default_tiered_pricing": model.default_tiered_pricing,
        "supported_capabilities": model.supported_capabilities,
        "config": config,
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
    allowed_providers: Option<&[String]>,
) -> Option<BTreeSet<String>> {
    allowed_providers.map(|providers| {
        providers
            .iter()
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty())
            .collect::<BTreeSet<_>>()
    })
}

async fn resolve_users_me_allowed_global_model_ids(
    state: &AppState,
    allowed_providers: Option<&[String]>,
    provider_key_policies: &BTreeMap<String, Vec<String>>,
) -> Result<Option<BTreeSet<String>>, Response<Body>> {
    if allowed_providers.is_none() && provider_key_policies.is_empty() {
        return Ok(None);
    }
    if allowed_providers.is_some_and(|values| values.is_empty()) {
        return Ok(Some(BTreeSet::new()));
    }

    if provider_key_policies.is_empty() {
        if !state.has_provider_catalog_data_reader() {
            return Err(build_auth_error_response(
                http::StatusCode::SERVICE_UNAVAILABLE,
                USERS_ME_PROVIDER_CATALOG_UNAVAILABLE_DETAIL,
                false,
            ));
        }

        let allowed_provider_names: BTreeSet<String> = allowed_providers
            .unwrap_or_default()
            .iter()
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty())
            .collect();
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
                    || allowed_provider_names.contains(&provider.provider_type.to_ascii_lowercase())
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
        return Ok(Some(
            refs.into_iter()
                .map(|entry| entry.global_model_id)
                .collect(),
        ));
    }

    if !state.has_minimal_candidate_selection_reader() {
        return Err(build_auth_error_response(
            http::StatusCode::SERVICE_UNAVAILABLE,
            USERS_ME_PROVIDER_CATALOG_UNAVAILABLE_DETAIL,
            false,
        ));
    }

    let constraints = aether_scheduler_core::SchedulerAuthConstraints {
        allowed_providers: allowed_providers.map(|values| values.to_vec()),
        provider_key_policies: provider_key_policies.clone(),
        allowed_api_formats: None,
        allowed_models: None,
    };
    let auth_snapshot = crate::data::auth::GatewayAuthApiKeySnapshot {
        user_id: "catalog-policy".to_string(),
        username: "catalog-policy".to_string(),
        email: None,
        user_role: "user".to_string(),
        user_auth_source: "local".to_string(),
        user_is_active: true,
        user_is_deleted: false,
        user_rate_limit: None,
        user_allowed_providers: constraints.allowed_providers.clone(),
        user_provider_key_policies: constraints.provider_key_policies.clone(),
        user_allowed_api_formats: None,
        user_allowed_models: None,
        api_key_id: "catalog-policy".to_string(),
        api_key_name: None,
        api_key_is_active: true,
        api_key_is_locked: false,
        api_key_is_standalone: false,
        api_key_rate_limit: None,
        api_key_concurrent_limit: None,
        api_key_expires_at_unix_secs: None,
        api_key_allowed_providers: None,
        api_key_allowed_api_formats: None,
        api_key_allowed_models: None,
        api_key_ip_rules: None,
        currently_usable: true,
    };
    let mut global_model_ids = BTreeSet::new();
    for api_format in USERS_ME_CATALOG_API_FORMATS {
        let rows = match state
            .list_minimal_candidate_selection_rows_for_api_format(api_format)
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
        let rows =
            filter_eligible_model_rows_for_state(state, rows, Some(&auth_snapshot), api_format)
                .await
                .map_err(|err| {
                    build_auth_error_response(
                        http::StatusCode::INTERNAL_SERVER_ERROR,
                        format!("user provider key model lookup failed: {err:?}"),
                        false,
                    )
                })?;
        global_model_ids.extend(rows.into_iter().map(|row| row.global_model_id));
    }
    Ok(Some(global_model_ids))
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

    let effective_policies = if auth.user.role.eq_ignore_ascii_case("admin") {
        None
    } else {
        match state
            .data
            .resolve_user_effective_list_policies(&auth.user)
            .await
        {
            Ok(value) => Some(value),
            Err(err) => {
                return build_auth_error_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("user policy lookup failed: {err:?}"),
                    false,
                )
            }
        }
    };
    let provider_model_ids = if auth.user.role.eq_ignore_ascii_case("admin") {
        None
    } else {
        let empty_key_policies = BTreeMap::new();
        match resolve_users_me_allowed_global_model_ids(
            state,
            effective_policies
                .as_ref()
                .and_then(|policies| policies.allowed_providers.as_deref()),
            effective_policies
                .as_ref()
                .map(|policies| &policies.provider_key_policies)
                .unwrap_or(&empty_key_policies),
        )
        .await
        {
            Ok(value) => value,
            Err(response) => return response,
        }
    };
    let allowed_models: Option<BTreeSet<String>> = if auth.user.role.eq_ignore_ascii_case("admin") {
        None
    } else {
        effective_policies
            .as_ref()
            .and_then(|policies| policies.allowed_models.as_ref())
            .map(|models: &Vec<String>| {
                models
                    .iter()
                    .map(|value: &String| value.trim().to_ascii_lowercase())
                    .filter(|value: &String| !value.is_empty())
                    .collect::<BTreeSet<_>>()
            })
    };

    let hide_mapping_config = !auth.user.role.eq_ignore_ascii_case("admin");

    let allowed_models: Option<BTreeSet<String>> = allowed_models.map(|models| {
        models
            .into_iter()
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty())
            .collect::<BTreeSet<_>>()
    });

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
            .map(|model| build_users_me_available_model_payload(model, hide_mapping_config))
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
    let expose_provider_details = auth.user.role.eq_ignore_ascii_case("admin");
    let effective_policies = if expose_provider_details {
        None
    } else {
        match state
            .data
            .resolve_user_effective_list_policies(&auth.user)
            .await
        {
            Ok(value) => Some(value),
            Err(err) => {
                return build_auth_error_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("user policy lookup failed: {err:?}"),
                    false,
                )
            }
        }
    };
    let allowed_provider_names = effective_policies.as_ref().and_then(|policies| {
        users_me_allowed_provider_names(policies.allowed_providers.as_deref())
    });

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
                || allowed_provider_names.contains(&provider.provider_type.to_ascii_lowercase())
        });
    }
    if let Some(policies) = effective_policies
        .as_ref()
        .filter(|policies| !policies.provider_key_policies.is_empty())
    {
        let restricted_provider_ids = providers
            .iter()
            .filter(|provider| policies.provider_key_policies.contains_key(&provider.id))
            .map(|provider| provider.id.clone())
            .collect::<Vec<_>>();
        let keys = match state
            .list_provider_catalog_key_summaries_by_provider_ids(&restricted_provider_ids)
            .await
        {
            Ok(value) => value,
            Err(err) => {
                return build_auth_error_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("user provider key lookup failed: {err:?}"),
                    false,
                )
            }
        };
        let providers_with_allowed_keys = keys
            .into_iter()
            .filter(|key| key.is_active)
            .filter(|key| {
                policies
                    .provider_key_policies
                    .get(&key.provider_id)
                    .is_none_or(|key_ids| key_ids.iter().any(|key_id| key_id == &key.id))
            })
            .map(|key| key.provider_id)
            .collect::<BTreeSet<_>>();
        providers.retain(|provider| {
            policies
                .provider_key_policies
                .get(&provider.id)
                .is_none_or(|_| providers_with_allowed_keys.contains(&provider.id))
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
        let mut endpoint_payload = json!({
            "id": endpoint.id,
            "api_format": endpoint.api_format,
            "is_active": endpoint.is_active,
        });
        if expose_provider_details {
            endpoint_payload["base_url"] = json!(endpoint.base_url);
        }
        endpoints_by_provider
            .entry(endpoint.provider_id)
            .or_default()
            .push(endpoint_payload);
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
                            "supports_embedding": model.supports_embedding,
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
                let mut payload = json!({
                    "id": provider_id.clone(),
                    "provider_priority": provider.provider_priority,
                    "endpoints": endpoints_by_provider.remove(&provider_id).unwrap_or_default(),
                    "models": models_by_provider.remove(&provider_id).unwrap_or_default(),
                });
                if expose_provider_details {
                    let description = provider
                        .config
                        .as_ref()
                        .and_then(|value| value.get("description"))
                        .and_then(serde_json::Value::as_str)
                        .map(ToOwned::to_owned);
                    payload["name"] = json!(provider.name);
                    payload["description"] = json!(description);
                }
                payload
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
