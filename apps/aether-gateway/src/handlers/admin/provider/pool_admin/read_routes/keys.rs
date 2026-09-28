use super::{
    admin_pool_provider_id_from_path, admin_provider_pool_config, build_admin_pool_error_response,
    parse_admin_pool_key_sort, parse_admin_pool_page, parse_admin_pool_page_size,
    parse_admin_pool_quick_selectors, parse_admin_pool_search, parse_admin_pool_status_filter,
    pool_payloads, pool_selection, read_admin_provider_pool_runtime_state, AdminPoolKeySort,
    AdminPoolKeySortDirection, AdminPoolKeySortField, AdminProviderPoolRuntimeState,
    ProviderCatalogKeyListOrder, ProviderCatalogKeyListQuery,
    ADMIN_POOL_PROVIDER_CATALOG_READER_UNAVAILABLE_DETAIL,
};
use crate::ai_serving::{provider_key_pool_score_id, provider_key_pool_score_scope};
use crate::handlers::admin::provider::shared::support::AdminProviderPoolConfig;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::provider_key_status_snapshot_payload;
use crate::provider_key_auth::provider_key_auth_semantics;
use crate::GatewayError;
use aether_admin::provider::pool as admin_provider_pool_pure;
use aether_data_contracts::repository::pool_scores::{
    GetPoolMemberScoresByIdsQuery, PoolMemberIdentity, StoredPoolMemberScore,
};
use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;
use aether_data_contracts::repository::usage::{
    ProviderApiKeyWindowUsageRequest, StoredProviderApiKeyWindowUsageSummary,
};
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};
use std::{
    cmp::Ordering,
    collections::BTreeMap,
    time::{SystemTime, UNIX_EPOCH},
};

type AdminPoolCycleUsageByKey =
    BTreeMap<String, BTreeMap<String, StoredProviderApiKeyWindowUsageSummary>>;

fn admin_pool_json_u64(value: Option<&serde_json::Value>) -> Option<u64> {
    match value {
        Some(serde_json::Value::Number(number)) => number.as_u64(),
        Some(serde_json::Value::String(text)) => text.trim().parse::<u64>().ok(),
        _ => None,
    }
}

fn admin_pool_current_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

async fn read_admin_pool_scores_by_key_id(
    state: &AdminAppState<'_>,
    provider_id: &str,
    key_ids: &[String],
) -> Result<BTreeMap<String, StoredPoolMemberScore>, GatewayError> {
    if key_ids.is_empty() {
        return Ok(BTreeMap::new());
    }

    let score_scope = provider_key_pool_score_scope();
    let score_ids = key_ids
        .iter()
        .map(|key_id| {
            let identity =
                PoolMemberIdentity::provider_api_key(provider_id.to_string(), key_id.clone());
            provider_key_pool_score_id(&identity, &score_scope)
        })
        .collect::<Vec<_>>();
    let scores = state
        .app()
        .data
        .get_pool_member_scores_by_ids(&GetPoolMemberScoresByIdsQuery { ids: score_ids })
        .await
        .map_err(|err| GatewayError::Internal(format!("{err:?}")))?;
    Ok(scores
        .into_iter()
        .map(|score| (score.member_id.clone(), score))
        .collect::<BTreeMap<_, _>>())
}

fn admin_pool_cycle_usage_request(
    provider_type: &str,
    key: &StoredProviderCatalogKey,
    window: &serde_json::Map<String, serde_json::Value>,
    now_unix_secs: u64,
) -> Option<ProviderApiKeyWindowUsageRequest> {
    let spec = pool_payloads::admin_pool_cycle_window_spec(provider_type, window)?;
    let reset_at = admin_pool_json_u64(window.get("reset_at"))?;
    let window_seconds = spec.window_minutes.checked_mul(60)?;
    if reset_at <= now_unix_secs {
        return None;
    }
    // A reported period start is exact; otherwise derive it from the window length.
    let mut start_unix_secs = match admin_pool_json_u64(window.get("period_start_at")) {
        Some(period_start_at) if period_start_at < reset_at => period_start_at,
        _ => reset_at.checked_sub(window_seconds)?,
    };
    if let Some(usage_reset_at) = admin_pool_json_u64(window.get("usage_reset_at")) {
        start_unix_secs = start_unix_secs.max(usage_reset_at);
    }
    if start_unix_secs >= reset_at || start_unix_secs >= now_unix_secs {
        return None;
    }

    Some(ProviderApiKeyWindowUsageRequest {
        provider_api_key_id: key.id.clone(),
        window_code: spec.code,
        start_unix_secs,
        end_unix_secs: now_unix_secs,
        include_model_prefix: spec.include_model_prefix.map(str::to_string),
        exclude_model_prefix: spec.exclude_model_prefix.map(str::to_string),
    })
}

fn admin_pool_cycle_usage_requests(
    provider_type: &str,
    keys: &[StoredProviderCatalogKey],
    now_unix_secs: u64,
) -> Vec<ProviderApiKeyWindowUsageRequest> {
    keys.iter()
        .flat_map(|key| {
            // Enumerate the same windows the pool page returns: the payload
            // materializes quota windows from upstream metadata when the stored
            // snapshot lacks them.
            let snapshot = provider_key_status_snapshot_payload(key, provider_type);
            snapshot
                .get("quota")
                .and_then(|quota| quota.get("windows"))
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_object)
                .filter_map(|window| {
                    admin_pool_cycle_usage_request(provider_type, key, window, now_unix_secs)
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

async fn read_admin_pool_cycle_usage_by_key(
    state: &AdminAppState<'_>,
    provider_type: &str,
    keys: &[StoredProviderCatalogKey],
    now_unix_secs: u64,
) -> Result<AdminPoolCycleUsageByKey, GatewayError> {
    if !pool_payloads::admin_pool_provider_supports_cycle_stats(provider_type) || keys.is_empty() {
        return Ok(BTreeMap::new());
    }

    let requests = admin_pool_cycle_usage_requests(provider_type, keys, now_unix_secs);
    if requests.is_empty() {
        return Ok(BTreeMap::new());
    }

    let summaries = state
        .app()
        .summarize_usage_by_provider_api_key_windows(&requests)
        .await?;
    let mut usage_by_key = AdminPoolCycleUsageByKey::new();
    for summary in summaries {
        let window_code = summary.window_code.trim().to_ascii_lowercase();
        if window_code.is_empty() {
            continue;
        }
        usage_by_key
            .entry(summary.provider_api_key_id.clone())
            .or_default()
            .insert(window_code, summary);
    }
    Ok(usage_by_key)
}

fn admin_pool_compare_optional_unix_secs(
    left: Option<u64>,
    right: Option<u64>,
    direction: AdminPoolKeySortDirection,
) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => match direction {
            AdminPoolKeySortDirection::Asc => left.cmp(&right),
            AdminPoolKeySortDirection::Desc => right.cmp(&left),
        },
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn admin_pool_compare_optional_score(
    left: Option<f64>,
    right: Option<f64>,
    direction: AdminPoolKeySortDirection,
) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => {
            let ordering = left.partial_cmp(&right).unwrap_or(Ordering::Equal);
            match direction {
                AdminPoolKeySortDirection::Asc => ordering,
                AdminPoolKeySortDirection::Desc => ordering.reverse(),
            }
        }
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn admin_pool_score_for_key(
    scores_by_key_id: &BTreeMap<String, StoredPoolMemberScore>,
    key: &StoredProviderCatalogKey,
) -> Option<f64> {
    scores_by_key_id
        .get(&key.id)
        .map(|score| score.score)
        .filter(|score| score.is_finite())
}

fn admin_pool_sort_keys_for_request(keys: &mut [StoredProviderCatalogKey], sort: AdminPoolKeySort) {
    match sort.field {
        AdminPoolKeySortField::Default => pool_selection::admin_pool_sort_keys(keys),
        AdminPoolKeySortField::ImportedAt => {
            keys.sort_by(|left, right| {
                admin_pool_compare_optional_unix_secs(
                    left.created_at_unix_ms,
                    right.created_at_unix_ms,
                    sort.direction,
                )
                .then(left.name.cmp(&right.name))
                .then(left.id.cmp(&right.id))
            });
        }
        AdminPoolKeySortField::LastUsedAt => {
            keys.sort_by(|left, right| {
                admin_pool_compare_optional_unix_secs(
                    left.last_used_at_unix_secs,
                    right.last_used_at_unix_secs,
                    sort.direction,
                )
                .then(left.name.cmp(&right.name))
                .then(left.id.cmp(&right.id))
            });
        }
        AdminPoolKeySortField::Score => {}
    }
}

fn admin_pool_sort_keys_by_score(
    keys: &mut [StoredProviderCatalogKey],
    scores_by_key_id: &BTreeMap<String, StoredPoolMemberScore>,
    direction: AdminPoolKeySortDirection,
) {
    keys.sort_by(|left, right| {
        admin_pool_compare_optional_score(
            admin_pool_score_for_key(scores_by_key_id, left),
            admin_pool_score_for_key(scores_by_key_id, right),
            direction,
        )
        .then(left.name.cmp(&right.name))
        .then(left.id.cmp(&right.id))
    });
}

fn admin_pool_repository_key_order(sort: AdminPoolKeySort) -> ProviderCatalogKeyListOrder {
    match (sort.field, sort.direction) {
        (AdminPoolKeySortField::Default, _) => ProviderCatalogKeyListOrder::Name,
        (AdminPoolKeySortField::ImportedAt, AdminPoolKeySortDirection::Asc) => {
            ProviderCatalogKeyListOrder::CreatedAtAsc
        }
        (AdminPoolKeySortField::ImportedAt, AdminPoolKeySortDirection::Desc) => {
            ProviderCatalogKeyListOrder::CreatedAtDesc
        }
        (AdminPoolKeySortField::LastUsedAt, AdminPoolKeySortDirection::Asc) => {
            ProviderCatalogKeyListOrder::LastUsedAtAsc
        }
        (AdminPoolKeySortField::LastUsedAt, AdminPoolKeySortDirection::Desc) => {
            ProviderCatalogKeyListOrder::LastUsedAtDesc
        }
        (AdminPoolKeySortField::Score, _) => ProviderCatalogKeyListOrder::Name,
    }
}

fn admin_pool_trimmed_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn admin_pool_account_code_status_filter(code: &str) -> Option<&'static str> {
    match code.trim().to_ascii_lowercase().as_str() {
        "oauth_token_invalid" => Some("invalid"),
        "oauth_token_expired" => Some("expired"),
        "account_banned" | "account_suspended" => Some("account_banned"),
        "account_disabled" => Some("account_disabled"),
        "workspace_deactivated" => Some("workspace_deactivated"),
        "account_forbidden" => Some("account_forbidden"),
        "account_verification" => Some("account_verification"),
        "account_quarantined" => Some("account_quarantined"),
        "account_blocked" => Some("account_blocked"),
        _ => None,
    }
}

fn admin_pool_label_status_filter(label: &str) -> Option<&'static str> {
    match label.trim() {
        "已失效" | "Token 失效" | "Token失效" => Some("invalid"),
        "已过期" => Some("expired"),
        "账号封禁" | "账号已封禁" | "封禁" => Some("account_banned"),
        "额度耗尽" => Some("quota_exhausted"),
        "访问受限" | "账号访问受限" => Some("account_forbidden"),
        "账号停用" => Some("account_disabled"),
        "工作区停用" | "工作区已停用" => Some("workspace_deactivated"),
        "需要验证" => Some("account_verification"),
        "账号隔离" => Some("account_quarantined"),
        "账号异常" => Some("account_blocked"),
        "速率受限" => Some("rate_limited"),
        "超限" => Some("cost_exhausted"),
        "冷却中" => Some("cooldown"),
        "已禁用" | "禁用" | "停用" => Some("inactive"),
        "可用" => Some("available"),
        _ => None,
    }
}

fn admin_pool_oauth_status_filter(
    key: &StoredProviderCatalogKey,
    provider_type: &str,
    auth_config: Option<&serde_json::Map<String, Value>>,
    oauth_snapshot: Option<&serde_json::Map<String, Value>>,
    now_unix_secs: u64,
) -> Option<&'static str> {
    let oauth_snapshot = oauth_snapshot?;
    let code = admin_pool_trimmed_string(oauth_snapshot.get("code"))
        .unwrap_or_default()
        .to_ascii_lowercase();
    match code.as_str() {
        "invalid" => Some("invalid"),
        "expired" => Some("expired"),
        _ => admin_pool_trimmed_string(oauth_snapshot.get("label"))
            .as_deref()
            .and_then(admin_pool_label_status_filter)
            .or_else(|| {
                admin_pool_derive_oauth_expires_at(provider_type, key, auth_config)
                    .is_some_and(|expires_at| expires_at <= now_unix_secs)
                    .then_some("expired")
            }),
    }
}

fn admin_pool_derive_oauth_expires_at(
    provider_type: &str,
    key: &StoredProviderCatalogKey,
    auth_config: Option<&serde_json::Map<String, Value>>,
) -> Option<u64> {
    if !provider_key_auth_semantics(key, provider_type).oauth_managed() {
        return None;
    }

    if key.expires_at_unix_secs.is_some() {
        return key.expires_at_unix_secs;
    }

    for field in ["expires_at", "expiresAt", "expiry", "exp"] {
        let expires_at = admin_pool_json_u64(auth_config.and_then(|config| config.get(field)));
        if expires_at.is_some() {
            return expires_at;
        }
    }

    None
}

fn admin_pool_account_status_filter(
    account_snapshot: Option<&serde_json::Map<String, Value>>,
) -> Option<&'static str> {
    let account_snapshot = account_snapshot?;
    let code = admin_pool_trimmed_string(account_snapshot.get("code"));
    let label = admin_pool_trimmed_string(account_snapshot.get("label"));
    let blocked = account_snapshot
        .get("blocked")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    if let Some(code) = code
        .as_deref()
        .and_then(admin_pool_account_code_status_filter)
    {
        return Some(code);
    }
    if let Some(label) = label.as_deref().and_then(admin_pool_label_status_filter) {
        return Some(label);
    }
    blocked.then_some("account_blocked")
}

fn admin_pool_quota_status_filter(
    quota_snapshot: Option<&serde_json::Map<String, Value>>,
) -> Option<&'static str> {
    let quota_snapshot = quota_snapshot?;
    let code = admin_pool_trimmed_string(quota_snapshot.get("code"))
        .unwrap_or_default()
        .to_ascii_lowercase();
    match code.as_str() {
        "banned" => Some("account_banned"),
        "forbidden" => Some("account_forbidden"),
        "quarantined" => Some("account_quarantined"),
        "rate_limited" => Some("rate_limited"),
        "exhausted" => Some("quota_exhausted"),
        _ => admin_pool_trimmed_string(quota_snapshot.get("label"))
            .as_deref()
            .and_then(admin_pool_label_status_filter),
    }
}

fn admin_pool_key_cost_exhausted(
    key: &StoredProviderCatalogKey,
    pool_config: Option<&AdminProviderPoolConfig>,
    runtime: &AdminProviderPoolRuntimeState,
) -> bool {
    let Some(limit) = pool_config
        .and_then(|config| config.cost_limit_per_key_tokens)
        .filter(|limit| *limit > 0)
    else {
        return false;
    };
    runtime
        .cost_window_usage_by_key
        .get(&key.id)
        .copied()
        .unwrap_or(0)
        >= limit
}

pub(super) fn admin_pool_key_visible_status_filter(
    state: &AdminAppState<'_>,
    key: &StoredProviderCatalogKey,
    provider_type: &str,
    pool_config: Option<&AdminProviderPoolConfig>,
    runtime: &AdminProviderPoolRuntimeState,
    now_unix_secs: u64,
) -> &'static str {
    let status_snapshot = provider_key_status_snapshot_payload(key, provider_type);
    let account_snapshot = status_snapshot.get("account").and_then(Value::as_object);
    let quota_snapshot = status_snapshot.get("quota").and_then(Value::as_object);
    let oauth_snapshot = status_snapshot.get("oauth").and_then(Value::as_object);
    let auth_config = state.parse_catalog_auth_config_json(key);

    if let Some(status) = admin_pool_account_status_filter(account_snapshot) {
        return status;
    }
    if let Some(status) = admin_pool_quota_status_filter(quota_snapshot) {
        return status;
    }
    if let Some(status) = admin_pool_oauth_status_filter(
        key,
        provider_type,
        auth_config.as_ref(),
        oauth_snapshot,
        now_unix_secs,
    ) {
        return status;
    }
    if pool_config.is_some_and(|config| {
        (config.skip_exhausted_accounts
            && admin_provider_pool_pure::admin_pool_key_account_quota_exhausted(key, provider_type))
            || (config.reserve_minimum_quota
                && admin_provider_pool_pure::admin_pool_key_minimum_quota_reached(
                    key,
                    provider_type,
                    None,
                ))
    }) {
        return "quota_exhausted";
    }
    if !key.is_active {
        return "inactive";
    }
    if runtime.cooldown_reason_by_key.contains_key(&key.id) {
        return "cooldown";
    }
    if admin_pool_key_cost_exhausted(key, pool_config, runtime) {
        return "cost_exhausted";
    }
    "available"
}

pub(super) async fn build_admin_pool_list_keys_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    if !state.has_provider_catalog_data_reader() {
        return Ok(build_admin_pool_error_response(
            http::StatusCode::SERVICE_UNAVAILABLE,
            ADMIN_POOL_PROVIDER_CATALOG_READER_UNAVAILABLE_DETAIL,
        ));
    }

    let Some(provider_id) = admin_pool_provider_id_from_path(request_context.path()) else {
        return Ok(build_admin_pool_error_response(
            http::StatusCode::BAD_REQUEST,
            "provider_id 无效",
        ));
    };
    let query = request_context.query_string();
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
    let quick_selectors = admin_provider_pool_pure::admin_pool_sanitize_quick_selectors(
        parse_admin_pool_quick_selectors(query),
    );
    let status = match parse_admin_pool_status_filter(query) {
        Ok(value) => value,
        Err(detail) => {
            return Ok(build_admin_pool_error_response(
                http::StatusCode::BAD_REQUEST,
                detail,
            ));
        }
    };
    let sort = match parse_admin_pool_key_sort(query) {
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
    let sort_by_score = matches!(sort.field, AdminPoolKeySortField::Score);
    let now_unix_secs = admin_pool_current_unix_secs();

    let (keys, total, preloaded_pool_scores_by_key_id) = if status != "all" {
        let mut keys = state
            .list_provider_catalog_keys_by_provider_ids(std::slice::from_ref(&provider.id))
            .await?
            .into_iter()
            .collect::<Vec<_>>();
        if let Some(keyword) = search.as_ref() {
            keys.retain(|key| {
                pool_selection::admin_pool_matches_search(
                    state,
                    key,
                    &provider.provider_type,
                    Some(keyword),
                )
            });
        }
        if !quick_selectors.is_empty() {
            keys.retain(|key| {
                quick_selectors.iter().all(|selector| {
                    pool_selection::admin_pool_matches_quick_selector(
                        state,
                        key,
                        &provider.provider_type,
                        selector,
                    )
                })
            });
        }

        let key_ids = keys.iter().map(|key| key.id.clone()).collect::<Vec<_>>();
        let runtime = match pool_config.as_ref() {
            Some(pool_config) if !key_ids.is_empty() => {
                read_admin_provider_pool_runtime_state(
                    state.runtime_state(),
                    &provider.id,
                    &key_ids,
                    pool_config,
                    None,
                )
                .await
            }
            _ => AdminProviderPoolRuntimeState::default(),
        };
        keys.retain(|key| {
            admin_pool_key_visible_status_filter(
                state,
                key,
                &provider.provider_type,
                pool_config.as_ref(),
                &runtime,
                now_unix_secs,
            ) == status
        });

        let preloaded_pool_scores_by_key_id = if sort_by_score {
            let key_ids = keys.iter().map(|key| key.id.clone()).collect::<Vec<_>>();
            let scores = read_admin_pool_scores_by_key_id(state, &provider.id, &key_ids)
                .await
                .unwrap_or_default();
            admin_pool_sort_keys_by_score(&mut keys, &scores, sort.direction);
            Some(scores)
        } else {
            admin_pool_sort_keys_for_request(&mut keys, sort);
            None
        };
        let total = keys.len();
        let keys = keys
            .into_iter()
            .skip(page_offset)
            .take(page_size)
            .collect::<Vec<_>>();
        (keys, total, preloaded_pool_scores_by_key_id)
    } else if !quick_selectors.is_empty() || sort_by_score {
        let use_full_search = !quick_selectors.is_empty();
        let mut keys = state
            .list_provider_catalog_keys_by_provider_ids(std::slice::from_ref(&provider.id))
            .await?
            .into_iter()
            .filter(|key| {
                if use_full_search {
                    pool_selection::admin_pool_matches_search(
                        state,
                        key,
                        &provider.provider_type,
                        search.as_deref(),
                    )
                } else {
                    pool_selection::admin_pool_matches_catalog_search(key, search.as_deref())
                }
            })
            .filter(|key| {
                quick_selectors.iter().all(|selector| {
                    pool_selection::admin_pool_matches_quick_selector(
                        state,
                        key,
                        &provider.provider_type,
                        selector,
                    )
                })
            })
            .collect::<Vec<_>>();
        let preloaded_pool_scores_by_key_id = if sort_by_score {
            let key_ids = keys.iter().map(|key| key.id.clone()).collect::<Vec<_>>();
            let scores = read_admin_pool_scores_by_key_id(state, &provider.id, &key_ids)
                .await
                .unwrap_or_default();
            admin_pool_sort_keys_by_score(&mut keys, &scores, sort.direction);
            Some(scores)
        } else {
            admin_pool_sort_keys_for_request(&mut keys, sort);
            None
        };
        let total = keys.len();
        let keys = keys
            .into_iter()
            .skip(page_offset)
            .take(page_size)
            .collect::<Vec<_>>();
        (keys, total, preloaded_pool_scores_by_key_id)
    } else {
        let key_page = state
            .list_provider_catalog_key_page(&ProviderCatalogKeyListQuery {
                provider_id: provider.id.clone(),
                search: search.clone(),
                is_active: None,
                offset: page_offset,
                limit: page_size,
                order: admin_pool_repository_key_order(sort),
            })
            .await?;
        (key_page.items, key_page.total, None)
    };

    let key_ids = keys.iter().map(|key| key.id.clone()).collect::<Vec<_>>();
    let pool_scores_by_key_id = match preloaded_pool_scores_by_key_id {
        Some(scores) => scores,
        None => read_admin_pool_scores_by_key_id(state, &provider.id, &key_ids)
            .await
            .unwrap_or_default(),
    };
    let endpoints = state
        .list_provider_catalog_endpoints_by_provider_ids(std::slice::from_ref(&provider.id))
        .await?;
    let runtime = match pool_config.as_ref() {
        Some(pool_config) if !key_ids.is_empty() => {
            read_admin_provider_pool_runtime_state(
                state.runtime_state(),
                &provider.id,
                &key_ids,
                pool_config,
                None,
            )
            .await
        }
        _ => AdminProviderPoolRuntimeState::default(),
    };
    let cycle_usage_by_key =
        read_admin_pool_cycle_usage_by_key(state, &provider.provider_type, &keys, now_unix_secs)
            .await?;

    let items = keys
        .into_iter()
        .map(|key| {
            pool_payloads::build_admin_pool_key_payload(
                state,
                &provider.provider_type,
                &endpoints,
                &key,
                &runtime,
                pool_config.clone(),
                pool_scores_by_key_id.get(&key.id),
                cycle_usage_by_key.get(&key.id),
                now_unix_secs,
            )
        })
        .collect::<Vec<_>>();

    Ok(Json(json!({
        "total": total,
        "page": page,
        "page_size": page_size,
        "keys": items,
    }))
    .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_key(auth_type: &str) -> StoredProviderCatalogKey {
        StoredProviderCatalogKey::new(
            "key-1".to_string(),
            "provider-codex".to_string(),
            "Codex OAuth".to_string(),
            auth_type.to_string(),
            None,
            true,
        )
        .expect("sample key should build")
    }

    #[test]
    fn oauth_status_filter_uses_auth_config_expiry_for_visible_expired_state() {
        let key = sample_key("oauth");
        let oauth_snapshot_value = json!({"code": "none", "label": null});
        let auth_config_value = json!({"expires_at": 1_000u64});

        assert_eq!(
            admin_pool_oauth_status_filter(
                &key,
                "codex",
                auth_config_value.as_object(),
                oauth_snapshot_value.as_object(),
                2_000,
            ),
            Some("expired")
        );
    }

    #[test]
    fn oauth_status_filter_matches_deleted_agent_runtime_as_invalid() {
        let mut key = sample_key("oauth");
        key.oauth_invalid_at_unix_secs = Some(1_784_728_663);
        key.oauth_invalid_reason =
            Some("[REQUEST_FAILED] Agent runtime has been deleted.".to_string());
        let status_snapshot = provider_key_status_snapshot_payload(&key, "codex");

        assert_eq!(
            admin_pool_oauth_status_filter(
                &key,
                "codex",
                None,
                status_snapshot.get("oauth").and_then(Value::as_object),
                1_784_729_000,
            ),
            Some("invalid")
        );
    }

    #[test]
    fn codex_cycle_usage_request_uses_actual_monthly_window_boundaries() {
        let key = sample_key("oauth");
        let reset_at = 5_000_000u64;
        let now = 3_000_000u64;
        let window = json!({
            "code": "monthly",
            "label": "月",
            "scope": "account",
            "reset_at": reset_at,
            "window_minutes": 43_800u64
        });

        let request = admin_pool_cycle_usage_request(
            "codex",
            &key,
            window.as_object().expect("window should be object"),
            now,
        )
        .expect("monthly usage request should build");

        assert_eq!(request.window_code, "monthly");
        assert_eq!(request.start_unix_secs, reset_at - 43_800 * 60);
        assert_eq!(request.end_unix_secs, now);
    }

    #[test]
    fn codex_cycle_usage_request_ignores_zero_and_spark_windows() {
        let key = sample_key("oauth");
        // The reset lies inside a weekly cycle from `now`, so only the zero or
        // spark marker can drop these windows.
        let reset_at = 3_100_000u64;
        for window in [
            json!({
                "code": "weekly",
                "scope": "account",
                "reset_at": reset_at,
                "window_minutes": 0
            }),
            json!({
                "code": "weekly",
                "scope": "account",
                "reset_at": reset_at,
                "window_minutes": "0"
            }),
            json!({
                "code": "spark_weekly",
                "scope": "account",
                "reset_at": reset_at,
                "window_minutes": 10_080
            }),
        ] {
            assert!(admin_pool_cycle_usage_request(
                "codex",
                &key,
                window.as_object().expect("window should be object"),
                3_000_000,
            )
            .is_none());
        }
        let weekly = json!({
            "code": "weekly",
            "scope": "account",
            "reset_at": reset_at
        });
        assert!(admin_pool_cycle_usage_request(
            "codex",
            &key,
            weekly.as_object().expect("window should be object"),
            3_000_000,
        )
        .is_some());
    }

    #[test]
    fn xai_cycle_usage_request_uses_weekly_period() {
        let key = sample_key("oauth");
        let reset_at = 5_000_000u64;
        let window = json!({
            "code": "usage",
            "scope": "account",
            "window": "weekly",
            "reset_at": reset_at
        });

        let request = admin_pool_cycle_usage_request(
            "xai",
            &key,
            window.as_object().expect("window should be object"),
            4_900_000,
        )
        .expect("xai weekly usage request should build");

        assert_eq!(request.window_code, "usage");
        assert_eq!(request.start_unix_secs, reset_at - 10_080 * 60);
        assert_eq!(request.include_model_prefix, None);
        assert_eq!(request.exclude_model_prefix, None);

        let prepaid = json!({"code": "prepaid", "scope": "account", "reset_at": reset_at});
        assert!(admin_pool_cycle_usage_request(
            "xai",
            &key,
            prepaid.as_object().expect("window should be object"),
            4_900_000,
        )
        .is_none());
    }

    #[test]
    fn cycle_usage_requests_cover_windows_materialized_from_upstream_metadata() {
        let mut key = sample_key("oauth");
        key.status_snapshot = None;
        key.upstream_metadata = Some(json!({
            "xai": {
                "updated_at": 4_900_000u64,
                "usage_percentage": 46.0,
                "period_type": "weekly",
                "next_reset_at": 5_000_000u64
            }
        }));

        let requests = admin_pool_cycle_usage_requests("xai", &[key], 4_900_000);

        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].window_code, "usage");
        assert_eq!(requests[0].start_unix_secs, 5_000_000 - 10_080 * 60);
    }

    #[test]
    fn xai_monthly_cycle_usage_request_requires_the_reported_period_start() {
        let key = sample_key("oauth");
        let reset_at = 5_000_000u64;
        let period_start_at = 2_500_000u64;
        let now = 4_900_000u64;
        let monthly = json!({
            "code": "usage",
            "scope": "account",
            "window": "monthly",
            "reset_at": reset_at,
            "period_start_at": period_start_at
        });

        let request = admin_pool_cycle_usage_request(
            "xai",
            &key,
            monthly.as_object().expect("window should be object"),
            now,
        )
        .expect("xai monthly usage request should build from its period start");
        assert_eq!(request.start_unix_secs, period_start_at);
        assert_eq!(request.end_unix_secs, now);

        for window in [
            json!({"code": "usage", "scope": "account", "window": "monthly", "reset_at": reset_at}),
            json!({"code": "usage", "scope": "account", "label": "月额度", "reset_at": reset_at}),
        ] {
            assert!(admin_pool_cycle_usage_request(
                "xai",
                &key,
                window.as_object().expect("window should be object"),
                now,
            )
            .is_none());
        }
    }

    #[test]
    fn antigravity_cycle_usage_request_splits_quota_groups_by_model_family() {
        let key = sample_key("oauth");
        let reset_at = 5_000_000u64;
        let now = 4_990_000u64;
        let window_for = |code: &str, bucket_id: &str, period: &str| {
            json!({
                "code": code,
                "scope": "quota_group",
                "quota_group": code.rsplit_once(':').map(|(group, _)| group),
                "bucket_id": bucket_id,
                "window": period,
                "reset_at": reset_at
            })
        };

        let gemini_5h = window_for("group:0:gemini-5h", "gemini-5h", "5h");
        let request = admin_pool_cycle_usage_request(
            "antigravity",
            &key,
            gemini_5h.as_object().expect("window should be object"),
            now,
        )
        .expect("gemini 5h request should build");
        assert_eq!(request.window_code, "group:0:gemini-5h");
        assert_eq!(request.start_unix_secs, reset_at - 300 * 60);
        assert_eq!(request.include_model_prefix.as_deref(), Some("gemini"));
        assert_eq!(request.exclude_model_prefix, None);

        let third_party_weekly = window_for("group:1:3p-weekly", "3p-weekly", "weekly");
        let request = admin_pool_cycle_usage_request(
            "antigravity",
            &key,
            third_party_weekly
                .as_object()
                .expect("window should be object"),
            now,
        )
        .expect("3p weekly request should build");
        assert_eq!(request.start_unix_secs, reset_at - 10_080 * 60);
        assert_eq!(request.include_model_prefix, None);
        assert_eq!(request.exclude_model_prefix.as_deref(), Some("gemini"));

        let model_window = json!({
            "code": "model:gemini-3-flash",
            "scope": "model",
            "model": "gemini-3-flash",
            "reset_at": reset_at
        });
        assert!(admin_pool_cycle_usage_request(
            "antigravity",
            &key,
            model_window.as_object().expect("window should be object"),
            now,
        )
        .is_none());

        let stale = window_for("group:0:gemini-5h", "gemini-5h", "5h");
        assert!(admin_pool_cycle_usage_request(
            "antigravity",
            &key,
            stale.as_object().expect("window should be object"),
            reset_at + 1,
        )
        .is_none());
    }

    #[test]
    fn oauth_status_filter_prefers_catalog_key_expiry_over_auth_config_expiry() {
        let mut key = sample_key("oauth");
        key.expires_at_unix_secs = Some(3_000);
        let oauth_snapshot_value = json!({"code": "none", "label": null});
        let auth_config_value = json!({"expires_at": 1_000u64});

        assert_eq!(
            admin_pool_oauth_status_filter(
                &key,
                "codex",
                auth_config_value.as_object(),
                oauth_snapshot_value.as_object(),
                2_000,
            ),
            None
        );
    }

    #[test]
    fn oauth_status_filter_ignores_auth_config_expiry_for_non_oauth_keys() {
        let key = sample_key("api_key");
        let oauth_snapshot_value = json!({"code": "none", "label": null});
        let auth_config_value = json!({"expires_at": 1_000u64});

        assert_eq!(
            admin_pool_oauth_status_filter(
                &key,
                "codex",
                auth_config_value.as_object(),
                oauth_snapshot_value.as_object(),
                2_000,
            ),
            None
        );
    }
}
