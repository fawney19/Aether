use super::cache_identity::{
    admin_monitoring_find_user_summary_by_id, admin_monitoring_list_export_api_key_records_by_ids,
    admin_monitoring_load_affinity_identity_maps,
};
use super::cache_payloads::{
    admin_monitoring_cache_affinity_sort_value, admin_monitoring_masked_provider_key_prefix,
    admin_monitoring_masked_user_api_key_prefix,
};
use super::cache_route_helpers::{
    admin_monitoring_cache_affinity_not_found_response,
    admin_monitoring_cache_affinity_user_identifier_from_path,
    parse_admin_monitoring_keyword_filter,
};
use super::cache_store::{
    list_admin_monitoring_cache_affinity_records,
    list_admin_monitoring_cache_affinity_records_by_affinity_keys,
};
use super::route_filters::{parse_admin_monitoring_limit, parse_admin_monitoring_offset};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use aether_admin::observability::monitoring::admin_monitoring_bad_request_response;
use axum::{
    body::Body,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

fn normalize_keyword<'a>(keyword: Option<&'a String>) -> Option<String> {
    keyword.map(|value| value.to_ascii_lowercase())
}

pub(super) async fn build_admin_monitoring_cache_affinities_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let limit = match parse_admin_monitoring_limit(request_context.request_query_string.as_deref())
    {
        Ok(value) => value,
        Err(detail) => return Ok(admin_monitoring_bad_request_response(detail)),
    };
    let offset =
        match parse_admin_monitoring_offset(request_context.request_query_string.as_deref()) {
            Ok(value) => value,
            Err(detail) => return Ok(admin_monitoring_bad_request_response(detail)),
        };
    let keyword =
        parse_admin_monitoring_keyword_filter(request_context.request_query_string.as_deref());

    let mut matched_user_id = None::<String>;
    let mut matched_api_key_id = None::<String>;
    let filtered_affinities = if let Some(keyword_value) = keyword.as_deref() {
        let direct_affinity_keys =
            std::iter::once(keyword_value.to_string()).collect::<std::collections::BTreeSet<_>>();
        let direct_affinities = list_admin_monitoring_cache_affinity_records_by_affinity_keys(
            state,
            &direct_affinity_keys,
        )
        .await?;
        if !direct_affinities.is_empty() {
            matched_api_key_id = Some(keyword_value.to_string());
            matched_user_id = admin_monitoring_list_export_api_key_records_by_ids(
                state,
                &[keyword_value.to_string()],
            )
            .await?
            .get(keyword_value)
            .map(|item| item.user_id.clone());
            direct_affinities
        } else if let Some(user) = state.find_user_auth_by_identifier(keyword_value).await? {
            matched_user_id = Some(user.id.clone());
            let user_api_key_ids = state
                .list_auth_api_key_export_records_by_user_ids(std::slice::from_ref(&user.id))
                .await?
                .into_iter()
                .map(|item| item.api_key_id)
                .collect::<std::collections::BTreeSet<_>>();
            list_admin_monitoring_cache_affinity_records_by_affinity_keys(state, &user_api_key_ids)
                .await?
        } else {
            list_admin_monitoring_cache_affinity_records(state).await?
        }
    } else {
        list_admin_monitoring_cache_affinity_records(state).await?
    };
    let (api_key_by_id, user_by_id) =
        admin_monitoring_load_affinity_identity_maps(state, &filtered_affinities).await?;

    let provider_ids = filtered_affinities
        .iter()
        .filter_map(|item| item.provider_id.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let endpoint_ids = filtered_affinities
        .iter()
        .filter_map(|item| item.endpoint_id.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let key_ids = filtered_affinities
        .iter()
        .filter_map(|item| item.key_id.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();

    let provider_by_id = state
        .read_provider_catalog_providers_by_ids(&provider_ids)
        .await?
        .into_iter()
        .map(|item| (item.id.clone(), item))
        .collect::<std::collections::BTreeMap<_, _>>();
    let endpoint_by_id = state
        .read_provider_catalog_endpoints_by_ids(&endpoint_ids)
        .await?
        .into_iter()
        .map(|item| (item.id.clone(), item))
        .collect::<std::collections::BTreeMap<_, _>>();
    let key_by_id = state
        .list_provider_catalog_keys_by_ids(&key_ids)
        .await?
        .into_iter()
        .map(|item| (item.id.clone(), item))
        .collect::<std::collections::BTreeMap<_, _>>();

    let keyword_lower = normalize_keyword(keyword.as_ref());
    let mut items = Vec::new();
    for affinity in filtered_affinities {
        let user_api_key = api_key_by_id.get(&affinity.affinity_key);
        let user_id = user_api_key.map(|item| item.user_id.clone());
        let user = user_id.as_ref().and_then(|id| user_by_id.get(id));
        let provider = affinity
            .provider_id
            .as_ref()
            .and_then(|id| provider_by_id.get(id));
        let endpoint = affinity
            .endpoint_id
            .as_ref()
            .and_then(|id| endpoint_by_id.get(id));
        let key = affinity.key_id.as_ref().and_then(|id| key_by_id.get(id));

        let user_api_key_name = user_api_key.and_then(|item| item.name.clone());
        let user_api_key_prefix = user_api_key.and_then(|item| {
            admin_monitoring_masked_user_api_key_prefix(state, item.key_encrypted.as_deref())
        });
        let provider_name = provider.map(|item| item.name.clone());
        let endpoint_url = endpoint
            .map(|item| item.base_url.clone())
            .filter(|value| !value.trim().is_empty());
        let key_name = key.map(|item| item.name.clone());
        let key_prefix =
            key.and_then(|item| admin_monitoring_masked_provider_key_prefix(state, item));
        let user_id_text = user_id.clone();
        let username = user.map(|item| item.username.clone());
        let email = user.and_then(|item| item.email.clone());
        let provider_id = affinity.provider_id.clone();
        let key_id = affinity.key_id.clone();

        if let Some(keyword_value) = keyword_lower.as_deref() {
            if matched_user_id.is_none() && matched_api_key_id.is_none() {
                let searchable = [
                    Some(affinity.affinity_key.as_str()),
                    user_api_key_name.as_deref(),
                    user_id_text.as_deref(),
                    username.as_deref(),
                    email.as_deref(),
                    provider_id.as_deref(),
                    key_id.as_deref(),
                ];
                if !searchable
                    .into_iter()
                    .flatten()
                    .any(|value| value.to_ascii_lowercase().contains(keyword_value))
                {
                    continue;
                }
            }
        }

        items.push(json!({
            "affinity_key": affinity.affinity_key,
            "user_api_key_name": user_api_key_name,
            "user_api_key_prefix": user_api_key_prefix,
            "is_standalone": user_api_key.map(|item| item.is_standalone).unwrap_or(false),
            "user_id": user_id_text,
            "username": username,
            "email": email,
            "provider_id": provider_id,
            "provider_name": provider_name,
            "endpoint_id": affinity.endpoint_id,
            "endpoint_url": endpoint_url,
            "key_id": key_id,
            "key_name": key_name,
            "key_prefix": key_prefix,
            "rate_multipliers": key.and_then(|item| item.rate_multipliers.clone()),
            "global_model_id": affinity.model_name,
            "model_name": affinity.model_name,
            "model_display_name": serde_json::Value::Null,
            "api_format": affinity.api_format,
            "created_at": affinity.created_at,
            "expire_at": affinity.expire_at,
            "request_count": affinity.request_count,
        }));
    }

    items.sort_by(|left, right| {
        admin_monitoring_cache_affinity_sort_value(right.get("expire_at"))
            .partial_cmp(&admin_monitoring_cache_affinity_sort_value(
                left.get("expire_at"),
            ))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let total = items.len();
    let paged_items = items
        .into_iter()
        .skip(offset)
        .take(limit)
        .collect::<Vec<_>>();
    let paged_count = paged_items.len();

    Ok(Json(json!({
        "status": "ok",
        "data": {
            "items": paged_items,
            "meta": {
                "total": total,
                "limit": limit,
                "offset": offset,
                "count": paged_count,
            },
            "matched_user_id": matched_user_id,
        }
    }))
    .into_response())
}

pub(super) async fn build_admin_monitoring_cache_affinity_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let Some(user_identifier) =
        admin_monitoring_cache_affinity_user_identifier_from_path(&request_context.request_path)
    else {
        return Ok(admin_monitoring_bad_request_response(
            "缺少 user_identifier",
        ));
    };
    let direct_api_key_by_id = admin_monitoring_list_export_api_key_records_by_ids(
        state,
        std::slice::from_ref(&user_identifier),
    )
    .await?;
    let direct_affinity_keys =
        std::iter::once(user_identifier.clone()).collect::<std::collections::BTreeSet<_>>();
    let direct_affinities =
        list_admin_monitoring_cache_affinity_records_by_affinity_keys(state, &direct_affinity_keys)
            .await?;

    let (resolved_user_id, username, email, filtered_affinities) = if !direct_affinities.is_empty()
        || direct_api_key_by_id.contains_key(&user_identifier)
    {
        let user_id = direct_api_key_by_id
            .get(&user_identifier)
            .map(|item| item.user_id.clone());
        let user = match user_id.as_deref() {
            Some(user_id) => admin_monitoring_find_user_summary_by_id(state, user_id).await?,
            None => None,
        };
        (
            user_id,
            user.as_ref().map(|item| item.username.clone()),
            user.and_then(|item| item.email),
            direct_affinities,
        )
    } else if let Some(user) = state.find_user_auth_by_identifier(&user_identifier).await? {
        let user_api_key_ids = state
            .list_auth_api_key_export_records_by_user_ids(std::slice::from_ref(&user.id))
            .await?
            .into_iter()
            .map(|item| item.api_key_id)
            .collect::<std::collections::BTreeSet<_>>();
        let affinities =
            list_admin_monitoring_cache_affinity_records_by_affinity_keys(state, &user_api_key_ids)
                .await?;
        (Some(user.id), Some(user.username), user.email, affinities)
    } else {
        return Ok(admin_monitoring_cache_affinity_not_found_response(
            &user_identifier,
        ));
    };

    if filtered_affinities.is_empty() {
        let display_name = username.clone().unwrap_or_else(|| user_identifier.clone());
        return Ok(Json(json!({
            "status": "not_found",
            "message": format!(
                "用户 {} ({}) 没有缓存亲和性",
                display_name,
                email.clone().unwrap_or_else(|| "null".to_string()),
            ),
            "user_info": {
                "user_id": resolved_user_id,
                "username": username,
                "email": email,
            },
            "affinities": [],
        }))
        .into_response());
    }

    let mut affinities = filtered_affinities
        .into_iter()
        .map(|item| {
            json!({
                "provider_id": item.provider_id,
                "endpoint_id": item.endpoint_id,
                "key_id": item.key_id,
                "api_format": item.api_format,
                "model_name": item.model_name,
                "created_at": item.created_at,
                "expire_at": item.expire_at,
                "request_count": item.request_count,
            })
        })
        .collect::<Vec<_>>();
    affinities.sort_by(|left, right| {
        admin_monitoring_cache_affinity_sort_value(right.get("expire_at"))
            .partial_cmp(&admin_monitoring_cache_affinity_sort_value(
                left.get("expire_at"),
            ))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let total_endpoints = affinities.len();

    Ok(Json(json!({
        "status": "ok",
        "user_info": {
            "user_id": resolved_user_id,
            "username": username,
            "email": email,
        },
        "affinities": affinities,
        "total_endpoints": total_endpoints,
    }))
    .into_response())
}
