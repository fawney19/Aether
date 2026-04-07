use super::support::{
    admin_pool_provider_id_from_path, build_admin_pool_error_response, parse_admin_pool_page,
    parse_admin_pool_page_size, parse_admin_pool_search, parse_admin_pool_status_filter,
    AdminPoolResolveSelectionRequest, ADMIN_POOL_PROVIDER_CATALOG_READER_UNAVAILABLE_DETAIL,
};
use super::{payloads as pool_payloads, selection as pool_selection};
use crate::control::GatewayPublicRequestContext;
use crate::handlers::admin::provider::pool::config::admin_provider_pool_config;
use crate::handlers::admin::provider::pool::runtime::{
    read_admin_provider_pool_cooldown_counts, read_admin_provider_pool_cooldown_key_ids,
    read_admin_provider_pool_runtime_state,
};
use crate::handlers::admin::provider::shared::support::AdminProviderPoolRuntimeState;
use crate::{AppState, GatewayError};
use aether_data_contracts::repository::provider_catalog::ProviderCatalogKeyListQuery;
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::collections::BTreeMap;

async fn build_admin_pool_overview_payload(
    state: &AppState,
) -> Result<serde_json::Value, GatewayError> {
    let providers = state.list_provider_catalog_providers(false).await?;
    let pool_enabled_providers = providers
        .into_iter()
        .filter_map(|provider| {
            admin_provider_pool_config(&provider).map(|config| (provider, config))
        })
        .collect::<Vec<_>>();
    let provider_ids = pool_enabled_providers
        .iter()
        .map(|(provider, _)| provider.id.clone())
        .collect::<Vec<_>>();
    let key_stats = if provider_ids.is_empty() {
        Vec::new()
    } else {
        state
            .list_provider_catalog_key_stats_by_provider_ids(&provider_ids)
            .await?
    };
    let key_stats_by_provider = key_stats
        .into_iter()
        .map(|item| (item.provider_id.clone(), item))
        .collect::<BTreeMap<_, _>>();
    let redis_runner = state.redis_kv_runner();
    let cooldown_counts_by_provider = match redis_runner.as_ref() {
        Some(runner) if !provider_ids.is_empty() => {
            read_admin_provider_pool_cooldown_counts(runner, &provider_ids).await
        }
        _ => BTreeMap::new(),
    };

    let mut items = Vec::with_capacity(pool_enabled_providers.len());
    for (provider, _pool_config) in pool_enabled_providers {
        let stats = key_stats_by_provider.get(&provider.id);
        let total_keys = stats.map(|item| item.total_keys as usize).unwrap_or(0);
        let active_keys = stats.map(|item| item.active_keys as usize).unwrap_or(0);
        let cooldown_count = cooldown_counts_by_provider
            .get(&provider.id)
            .copied()
            .unwrap_or(0);

        items.push(json!({
            "provider_id": provider.id,
            "provider_name": provider.name,
            "provider_type": provider.provider_type,
            "total_keys": total_keys,
            "active_keys": active_keys,
            "cooldown_count": cooldown_count,
            "pool_enabled": true,
        }));
    }

    Ok(json!({ "items": items }))
}

async fn build_admin_pool_list_keys_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
) -> Result<Response<Body>, GatewayError> {
    if !state.has_provider_catalog_data_reader() {
        return Ok(build_admin_pool_error_response(
            http::StatusCode::SERVICE_UNAVAILABLE,
            ADMIN_POOL_PROVIDER_CATALOG_READER_UNAVAILABLE_DETAIL,
        ));
    }

    let Some(provider_id) = admin_pool_provider_id_from_path(&request_context.request_path) else {
        return Ok(build_admin_pool_error_response(
            http::StatusCode::BAD_REQUEST,
            "provider_id 无效",
        ));
    };
    let query = request_context.request_query_string.as_deref();
    let page = match parse_admin_pool_page(query) {
        Ok(value) => value,
        Err(detail) => {
            return Ok(build_admin_pool_error_response(
                http::StatusCode::BAD_REQUEST,
                detail,
            ));
        }
    };
    let page_size = match parse_admin_pool_page_size(query) {
        Ok(value) => value,
        Err(detail) => {
            return Ok(build_admin_pool_error_response(
                http::StatusCode::BAD_REQUEST,
                detail,
            ));
        }
    };
    let search = parse_admin_pool_search(query).map(|value| value.to_ascii_lowercase());
    let status = match parse_admin_pool_status_filter(query) {
        Ok(value) => value,
        Err(detail) => {
            return Ok(build_admin_pool_error_response(
                http::StatusCode::BAD_REQUEST,
                detail,
            ));
        }
    };

    let Some(provider) = state
        .read_provider_catalog_providers_by_ids(std::slice::from_ref(&provider_id))
        .await?
        .into_iter()
        .next()
    else {
        return Ok(build_admin_pool_error_response(
            http::StatusCode::NOT_FOUND,
            format!("Provider {provider_id} 不存在"),
        ));
    };

    let pool_config = admin_provider_pool_config(&provider);
    let page_offset = page.saturating_sub(1).saturating_mul(page_size);

    let (keys, total) = if status == "cooldown" {
        let cooldown_key_ids = if let Some(runner) = state.redis_kv_runner() {
            read_admin_provider_pool_cooldown_key_ids(&runner, &provider.id).await
        } else {
            Vec::new()
        };
        let mut keys = if cooldown_key_ids.is_empty() {
            Vec::new()
        } else {
            state
                .list_provider_catalog_keys_by_ids(&cooldown_key_ids)
                .await?
        };
        if let Some(keyword) = search.as_ref() {
            keys.retain(|key| {
                key.name.to_ascii_lowercase().contains(keyword)
                    || key.id.to_ascii_lowercase().contains(keyword)
            });
        }
        pool_selection::admin_pool_sort_keys(&mut keys);
        let total = keys.len();
        let keys = keys
            .into_iter()
            .skip(page_offset)
            .take(page_size)
            .collect::<Vec<_>>();
        (keys, total)
    } else {
        let key_page = state
            .list_provider_catalog_key_page(&ProviderCatalogKeyListQuery {
                provider_id: provider.id.clone(),
                search: search.clone(),
                is_active: match status.as_str() {
                    "active" => Some(true),
                    "inactive" => Some(false),
                    _ => None,
                },
                offset: page_offset,
                limit: page_size,
            })
            .await?;
        (key_page.items, key_page.total)
    };

    let key_ids = keys.iter().map(|key| key.id.clone()).collect::<Vec<_>>();
    let runtime = match (state.redis_kv_runner(), pool_config) {
        (Some(runner), Some(pool_config)) if !key_ids.is_empty() => {
            read_admin_provider_pool_runtime_state(&runner, &provider.id, &key_ids, pool_config)
                .await
        }
        _ => AdminProviderPoolRuntimeState::default(),
    };

    let items = keys
        .into_iter()
        .map(|key| pool_payloads::build_admin_pool_key_payload(&key, &runtime, pool_config))
        .collect::<Vec<_>>();

    Ok(Json(json!({
        "total": total,
        "page": page,
        "page_size": page_size,
        "keys": items,
    }))
    .into_response())
}

async fn build_admin_pool_resolve_selection_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    request_body: Option<&Bytes>,
) -> Result<Response<Body>, GatewayError> {
    if !state.has_provider_catalog_data_reader() {
        return Ok(build_admin_pool_error_response(
            http::StatusCode::SERVICE_UNAVAILABLE,
            ADMIN_POOL_PROVIDER_CATALOG_READER_UNAVAILABLE_DETAIL,
        ));
    }

    let Some(provider_id) = admin_pool_provider_id_from_path(&request_context.request_path) else {
        return Ok(build_admin_pool_error_response(
            http::StatusCode::BAD_REQUEST,
            "provider_id 无效",
        ));
    };

    let payload = match request_body {
        None => AdminPoolResolveSelectionRequest::default(),
        Some(body) if body.is_empty() => AdminPoolResolveSelectionRequest::default(),
        Some(body) => match serde_json::from_slice::<AdminPoolResolveSelectionRequest>(body) {
            Ok(value) => value,
            Err(_) => {
                return Ok(build_admin_pool_error_response(
                    http::StatusCode::BAD_REQUEST,
                    "Invalid JSON request body",
                ));
            }
        },
    };

    let Some(provider) = state
        .read_provider_catalog_providers_by_ids(std::slice::from_ref(&provider_id))
        .await?
        .into_iter()
        .next()
    else {
        return Ok(build_admin_pool_error_response(
            http::StatusCode::NOT_FOUND,
            format!("Provider {provider_id} 不存在"),
        ));
    };

    let provider_type = provider.provider_type.clone();
    let search = payload.search.trim();
    let mut quick_selectors = payload
        .quick_selectors
        .into_iter()
        .map(pool_selection::admin_pool_normalize_text)
        .filter(|value| {
            matches!(
                value.as_str(),
                "banned"
                    | "no_5h_limit"
                    | "no_weekly_limit"
                    | "plan_free"
                    | "plan_team"
                    | "oauth_invalid"
                    | "proxy_unset"
                    | "proxy_set"
                    | "disabled"
                    | "enabled"
            )
        })
        .collect::<Vec<_>>();
    quick_selectors.sort();
    quick_selectors.dedup();

    let mut keys = state
        .list_provider_catalog_keys_by_provider_ids(std::slice::from_ref(&provider.id))
        .await?
        .into_iter()
        .filter(|key| {
            pool_selection::admin_pool_matches_search(state, key, &provider_type, Some(search))
        })
        .filter(|key| {
            quick_selectors.is_empty()
                || quick_selectors.iter().all(|selector| {
                    pool_selection::admin_pool_matches_quick_selector(
                        state,
                        key,
                        &provider_type,
                        selector,
                    )
                })
        })
        .collect::<Vec<_>>();

    keys.sort_by(|left, right| {
        left.internal_priority
            .cmp(&right.internal_priority)
            .then_with(|| left.name.cmp(&right.name))
    });

    let items = keys
        .iter()
        .map(|key| {
            json!({
                "key_id": key.id,
                "key_name": key.name,
                "auth_type": key.auth_type,
            })
        })
        .collect::<Vec<_>>();

    Ok(Json(json!({
        "total": items.len(),
        "items": items,
    }))
    .into_response())
}

fn build_admin_pool_scheduling_presets_payload() -> serde_json::Value {
    json!([
        {
            "name": "lru",
            "label": "LRU 轮转",
            "description": "最久未使用的 Key 优先",
            "providers": [],
            "modes": serde_json::Value::Null,
            "default_mode": serde_json::Value::Null,
            "mutex_group": "distribution_mode",
            "evidence_hint": "依据 LRU 时间戳（最近未使用优先）",
        },
        {
            "name": "cache_affinity",
            "label": "缓存亲和",
            "description": "优先复用最近使用过的 Key，利用 Prompt Caching",
            "providers": [],
            "modes": serde_json::Value::Null,
            "default_mode": serde_json::Value::Null,
            "mutex_group": "distribution_mode",
            "evidence_hint": "依据 LRU 时间戳（最近使用优先，与 LRU 轮转相反）",
        },
        {
            "name": "cost_first",
            "label": "成本优先",
            "description": "优先选择窗口消耗更低的账号",
            "providers": [],
            "modes": serde_json::Value::Null,
            "default_mode": serde_json::Value::Null,
            "mutex_group": serde_json::Value::Null,
            "evidence_hint": "依据窗口成本/Token 用量，缺失时回退配额使用率",
        },
        {
            "name": "free_first",
            "label": "Free 优先",
            "description": "优先消耗 Free 账号（依赖 plan_type）",
            "providers": ["codex", "kiro"],
            "modes": serde_json::Value::Null,
            "default_mode": serde_json::Value::Null,
            "mutex_group": serde_json::Value::Null,
            "evidence_hint": "依据 plan_type（Free 账号优先调度）",
        },
        {
            "name": "health_first",
            "label": "健康优先",
            "description": "优先选择健康分更高、失败更少的账号",
            "providers": [],
            "modes": serde_json::Value::Null,
            "default_mode": serde_json::Value::Null,
            "mutex_group": serde_json::Value::Null,
            "evidence_hint": "依据 health_by_format 聚合分（含熔断/失败衰减）",
        },
        {
            "name": "latency_first",
            "label": "延迟优先",
            "description": "优先选择最近延迟更低的账号",
            "providers": [],
            "modes": serde_json::Value::Null,
            "default_mode": serde_json::Value::Null,
            "mutex_group": serde_json::Value::Null,
            "evidence_hint": "依据号池延迟窗口均值（latency_window_seconds）",
        },
        {
            "name": "load_balance",
            "label": "负载均衡",
            "description": "随机分散 Key 使用，均匀分摊负载",
            "providers": [],
            "modes": serde_json::Value::Null,
            "default_mode": serde_json::Value::Null,
            "mutex_group": "distribution_mode",
            "evidence_hint": "每次随机分值，实现完全均匀分散",
        },
        {
            "name": "plus_first",
            "label": "Plus 优先",
            "description": "优先消耗 Plus/Pro 账号（依赖 plan_type）",
            "providers": ["codex", "kiro"],
            "modes": serde_json::Value::Null,
            "default_mode": serde_json::Value::Null,
            "mutex_group": serde_json::Value::Null,
            "evidence_hint": "依据 plan_type（Plus/Pro 账号优先调度）",
        },
        {
            "name": "priority_first",
            "label": "优先级优先",
            "description": "按账号优先级顺序调度（数字越小越优先）",
            "providers": [],
            "modes": serde_json::Value::Null,
            "default_mode": serde_json::Value::Null,
            "mutex_group": serde_json::Value::Null,
            "evidence_hint": "依据 internal_priority（支持拖拽/手工编辑）",
        },
        {
            "name": "quota_balanced",
            "label": "额度平均",
            "description": "优先选额度消耗最少的账号",
            "providers": [],
            "modes": serde_json::Value::Null,
            "default_mode": serde_json::Value::Null,
            "mutex_group": serde_json::Value::Null,
            "evidence_hint": "依据账号配额使用率；无配额时回退到窗口成本使用",
        },
        {
            "name": "recent_refresh",
            "label": "额度刷新优先",
            "description": "优先选即将刷新额度的账号",
            "providers": ["codex", "kiro"],
            "modes": serde_json::Value::Null,
            "default_mode": serde_json::Value::Null,
            "mutex_group": serde_json::Value::Null,
            "evidence_hint": "依据账号额度重置倒计时（next_reset / reset_seconds）",
        },
        {
            "name": "single_account",
            "label": "单号优先",
            "description": "集中使用同一账号（反向 LRU）",
            "providers": [],
            "modes": serde_json::Value::Null,
            "default_mode": serde_json::Value::Null,
            "mutex_group": "distribution_mode",
            "evidence_hint": "先按账号优先级（internal_priority），同级再按反向 LRU 集中",
        },
        {
            "name": "team_first",
            "label": "Team 优先",
            "description": "优先消耗 Team 账号（依赖 plan_type）",
            "providers": ["codex", "kiro"],
            "modes": serde_json::Value::Null,
            "default_mode": serde_json::Value::Null,
            "mutex_group": serde_json::Value::Null,
            "evidence_hint": "依据 plan_type（Team 账号优先调度）",
        }
    ])
}

pub(super) async fn maybe_build_local_admin_pool_read_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    request_body: Option<&axum::body::Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    match request_context
        .control_decision
        .as_ref()
        .and_then(|decision| decision.route_kind.as_deref())
    {
        Some("overview")
            if request_context.request_method == http::Method::GET
                && matches!(
                    request_context.request_path.trim_end_matches('/'),
                    "/api/admin/pool/overview"
                ) =>
        {
            if !state.has_provider_catalog_data_reader() {
                return Ok(Some(build_admin_pool_error_response(
                    http::StatusCode::SERVICE_UNAVAILABLE,
                    ADMIN_POOL_PROVIDER_CATALOG_READER_UNAVAILABLE_DETAIL,
                )));
            }
            Ok(Some(
                Json(build_admin_pool_overview_payload(state).await?).into_response(),
            ))
        }
        Some("scheduling_presets")
            if request_context.request_method == http::Method::GET
                && matches!(
                    request_context.request_path.trim_end_matches('/'),
                    "/api/admin/pool/scheduling-presets"
                ) =>
        {
            Ok(Some(
                Json(build_admin_pool_scheduling_presets_payload()).into_response(),
            ))
        }
        Some("list_keys") => Ok(Some(
            build_admin_pool_list_keys_response(state, request_context).await?,
        )),
        Some("resolve_selection") => Ok(Some(
            build_admin_pool_resolve_selection_response(state, request_context, request_body)
                .await?,
        )),
        _ => Ok(None),
    }
}
