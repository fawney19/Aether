use super::{
    admin_pool_provider_id_from_path, admin_provider_pool_config, build_admin_pool_error_response,
    parse_admin_pool_key_sort, parse_admin_pool_key_sort_values, parse_admin_pool_page,
    parse_admin_pool_page_size, parse_admin_pool_page_size_value, parse_admin_pool_page_value,
    parse_admin_pool_quick_selectors, parse_admin_pool_search, parse_admin_pool_search_scope,
    parse_admin_pool_search_scope_value, parse_admin_pool_status_filter,
    parse_admin_pool_status_filter_value, pool_payloads, pool_selection,
    read_admin_provider_pool_runtime_state, store_admin_pool_selection_snapshot, AdminPoolKeySort,
    AdminPoolKeySortDirection, AdminPoolKeySortField, AdminPoolSearchScope,
    AdminPoolSelectionSnapshotItem, AdminProviderPoolRuntimeState, ProviderCatalogKeyListOrder,
    ProviderCatalogKeyListQuery, ADMIN_POOL_PROVIDER_CATALOG_READER_UNAVAILABLE_DETAIL,
    ADMIN_POOL_SELECTION_SNAPSHOT_MAX_TOTAL,
};
use crate::ai_serving::{provider_key_pool_score_id, provider_key_pool_score_scope};
use crate::handlers::admin::provider::shared::support::AdminProviderPoolConfig;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::{provider_key_status_snapshot_payload, unix_secs_to_rfc3339};
use crate::provider_key_auth::provider_key_auth_semantics;
use crate::GatewayError;
use aether_admin::provider::pool as admin_provider_pool_pure;
use aether_data_contracts::repository::pool_scores::{
    GetPoolMemberScoresByIdsQuery, PoolMemberIdentity, StoredPoolMemberScore,
};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use aether_data_contracts::repository::usage::{
    ProviderApiKeyWindowUsageRequest, StoredProviderApiKeyWindowUsageSummary,
};
use axum::{
    body::{Body, Bytes},
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

const ADMIN_POOL_SELECTION_SNAPSHOT_TTL_SECONDS: u64 = 15 * 60;

type AdminPoolCodexCycleUsageByKey =
    BTreeMap<String, BTreeMap<String, StoredProviderApiKeyWindowUsageSummary>>;

fn admin_pool_json_u64(value: Option<&serde_json::Value>) -> Option<u64> {
    match value {
        Some(serde_json::Value::Number(number)) => number.as_u64(),
        Some(serde_json::Value::String(text)) => text.trim().parse::<u64>().ok(),
        _ => None,
    }
}

fn admin_pool_codex_default_window_minutes(code: &str) -> Option<u64> {
    if code.eq_ignore_ascii_case("5h") {
        Some(300)
    } else if code.eq_ignore_ascii_case("weekly") {
        Some(10_080)
    } else {
        None
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

fn admin_pool_codex_cycle_usage_request(
    key: &StoredProviderCatalogKey,
    window: &serde_json::Map<String, serde_json::Value>,
    now_unix_secs: u64,
) -> Option<ProviderApiKeyWindowUsageRequest> {
    let window_code = window
        .get("code")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|code| code.eq_ignore_ascii_case("5h") || code.eq_ignore_ascii_case("weekly"))?
        .to_ascii_lowercase();
    let reset_at = admin_pool_json_u64(window.get("reset_at"))?;
    let window_seconds = admin_pool_json_u64(window.get("window_minutes"))
        .or_else(|| admin_pool_codex_default_window_minutes(&window_code))?
        .checked_mul(60)?;
    if reset_at <= now_unix_secs {
        return None;
    }
    let mut start_unix_secs = reset_at.checked_sub(window_seconds)?;
    if let Some(usage_reset_at) = admin_pool_json_u64(window.get("usage_reset_at")) {
        start_unix_secs = start_unix_secs.max(usage_reset_at);
    }
    if start_unix_secs >= reset_at || start_unix_secs >= now_unix_secs {
        return None;
    }

    Some(ProviderApiKeyWindowUsageRequest {
        provider_api_key_id: key.id.clone(),
        window_code,
        start_unix_secs,
        end_unix_secs: now_unix_secs,
    })
}

fn admin_pool_codex_cycle_usage_requests(
    keys: &[StoredProviderCatalogKey],
    now_unix_secs: u64,
) -> Vec<ProviderApiKeyWindowUsageRequest> {
    keys.iter()
        .flat_map(|key| {
            key.status_snapshot
                .as_ref()
                .and_then(serde_json::Value::as_object)
                .and_then(|snapshot| snapshot.get("quota"))
                .and_then(serde_json::Value::as_object)
                .and_then(|quota| quota.get("windows"))
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_object)
                .filter_map(|window| {
                    admin_pool_codex_cycle_usage_request(key, window, now_unix_secs)
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

async fn read_admin_pool_codex_cycle_usage_by_key(
    state: &AdminAppState<'_>,
    provider_type: &str,
    keys: &[StoredProviderCatalogKey],
    now_unix_secs: u64,
) -> Result<AdminPoolCodexCycleUsageByKey, GatewayError> {
    if !provider_type.trim().eq_ignore_ascii_case("codex") || keys.is_empty() {
        return Ok(BTreeMap::new());
    }

    let requests = admin_pool_codex_cycle_usage_requests(keys, now_unix_secs);
    if requests.is_empty() {
        return Ok(BTreeMap::new());
    }

    let summaries = state
        .app()
        .summarize_usage_by_provider_api_key_windows(&requests)
        .await?;
    let mut usage_by_key = AdminPoolCodexCycleUsageByKey::new();
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

fn admin_pool_key_matches_name_search(
    key: &StoredProviderCatalogKey,
    search: Option<&str>,
) -> bool {
    let Some(search) = search else {
        return true;
    };
    let search = search.trim().to_ascii_lowercase();
    if search.is_empty() {
        return true;
    }
    key.name.to_ascii_lowercase().contains(&search) || key.id.to_ascii_lowercase().contains(&search)
}

fn admin_pool_normalized_expected_page_key_ids(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect()
}

fn admin_pool_sort_field_label(field: AdminPoolKeySortField) -> &'static str {
    match field {
        AdminPoolKeySortField::Default => "name",
        AdminPoolKeySortField::ImportedAt => "imported_at",
        AdminPoolKeySortField::LastUsedAt => "last_used_at",
        AdminPoolKeySortField::Score => "score",
    }
}

fn admin_pool_sort_direction_label(direction: AdminPoolKeySortDirection) -> &'static str {
    match direction {
        AdminPoolKeySortDirection::Asc => "asc",
        AdminPoolKeySortDirection::Desc => "desc",
    }
}

fn admin_pool_effective_search_scope(
    requested_scope: AdminPoolSearchScope,
    status: &str,
    quick_selectors: &[String],
    sort: AdminPoolKeySort,
) -> AdminPoolSearchScope {
    let legacy_full_search_path = status != "all"
        || !quick_selectors.is_empty()
        || matches!(sort.field, AdminPoolKeySortField::Score);
    match (requested_scope, legacy_full_search_path) {
        (AdminPoolSearchScope::Auto, true) => AdminPoolSearchScope::Full,
        (AdminPoolSearchScope::Auto, false) => AdminPoolSearchScope::Name,
        (scope, _) => scope,
    }
}

async fn build_admin_pool_selection_snapshot_payload(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    provider_id: &str,
    search: Option<&str>,
    quick_selectors: &[String],
    status: &str,
    search_scope: AdminPoolSearchScope,
    sort: AdminPoolKeySort,
    now_unix_secs: u64,
    keys: &[StoredProviderCatalogKey],
) -> Result<Value, GatewayError> {
    let mut items = Vec::with_capacity(keys.len());
    for (ordinal, key) in keys.iter().enumerate() {
        let ordinal = u64::try_from(ordinal).map_err(|_| {
            GatewayError::Internal("pool selection snapshot ordinal overflow".to_string())
        })?;
        items.push(AdminPoolSelectionSnapshotItem {
            ordinal,
            key_id: key.id.clone(),
            key_updated_at_unix_secs: key.updated_at_unix_secs,
        });
    }

    let snapshot = store_admin_pool_selection_snapshot(
        state,
        request_context,
        provider_id,
        Some(json!({
            "provider_id": provider_id,
            "search": search,
            "search_scope": match search_scope {
                AdminPoolSearchScope::Auto => "auto",
                AdminPoolSearchScope::Name => "name",
                AdminPoolSearchScope::Full => "full",
            },
            "quick_selectors": quick_selectors,
            "status": status,
            "sort_by": admin_pool_sort_field_label(sort.field),
            "sort_order": admin_pool_sort_direction_label(sort.direction),
        })),
        now_unix_secs,
        ADMIN_POOL_SELECTION_SNAPSHOT_TTL_SECONDS,
        items,
    )
    .await?;

    Ok(json!({
        "id": snapshot.id,
        "total": snapshot.total,
        "status": snapshot.status,
        "created_at_unix_secs": snapshot.created_at_unix_secs,
        "created_at": unix_secs_to_rfc3339(snapshot.created_at_unix_secs),
        "expires_at_unix_secs": snapshot.expires_at_unix_secs,
        "expires_at": unix_secs_to_rfc3339(snapshot.expires_at_unix_secs),
    }))
}

async fn maybe_build_admin_pool_selection_snapshot_payload(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    provider_id: &str,
    search: Option<&str>,
    quick_selectors: &[String],
    status: &str,
    search_scope: AdminPoolSearchScope,
    sort: AdminPoolKeySort,
    now_unix_secs: u64,
    keys: &[StoredProviderCatalogKey],
    page_offset: usize,
    page_size: usize,
    expected_total: Option<usize>,
    expected_page_key_ids: Option<&[String]>,
) -> Result<(Option<Value>, Option<Value>), GatewayError> {
    let total = keys.len();
    let mut mismatch = None;
    if let Some(expected_total) = expected_total {
        if expected_total != total {
            mismatch = Some(json!({
                "reason": "total_changed",
                "expected_total": expected_total,
                "actual_total": total,
            }));
        }
    }
    if let Some(expected_page_key_ids) = expected_page_key_ids {
        let actual_page_key_ids = keys
            .iter()
            .skip(page_offset)
            .take(page_size)
            .map(|key| key.id.clone())
            .collect::<Vec<_>>();
        if expected_page_key_ids != actual_page_key_ids.as_slice() {
            mismatch = Some(json!({
                "reason": "page_keys_changed",
                "expected_total": expected_total.unwrap_or(total),
                "actual_total": total,
            }));
        }
    }

    Ok((
        Some(
            build_admin_pool_selection_snapshot_payload(
                state,
                request_context,
                provider_id,
                search,
                quick_selectors,
                status,
                search_scope,
                sort,
                now_unix_secs,
                keys,
            )
            .await?,
        ),
        mismatch,
    ))
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

fn admin_pool_key_visible_status_filter(
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
    if pool_config.is_some_and(|config| config.skip_exhausted_accounts)
        && admin_provider_pool_pure::admin_pool_key_account_quota_exhausted(key, provider_type)
    {
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

async fn read_admin_pool_filtered_sorted_keys(
    state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
    pool_config: Option<&AdminProviderPoolConfig>,
    search: Option<&str>,
    search_scope: AdminPoolSearchScope,
    quick_selectors: &[String],
    status: &str,
    sort: AdminPoolKeySort,
    now_unix_secs: u64,
) -> Result<
    (
        Vec<StoredProviderCatalogKey>,
        Option<BTreeMap<String, StoredPoolMemberScore>>,
    ),
    GatewayError,
> {
    let mut keys = state
        .list_provider_catalog_keys_by_provider_ids(std::slice::from_ref(&provider.id))
        .await?
        .into_iter()
        .filter(|key| match search_scope {
            AdminPoolSearchScope::Auto | AdminPoolSearchScope::Name => {
                admin_pool_key_matches_name_search(key, search)
            }
            AdminPoolSearchScope::Full if search.is_some() => {
                pool_selection::admin_pool_matches_search(
                    state,
                    key,
                    &provider.provider_type,
                    search,
                )
            }
            AdminPoolSearchScope::Full => true,
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

    if status != "all" {
        let key_ids = keys.iter().map(|key| key.id.clone()).collect::<Vec<_>>();
        let runtime = match pool_config {
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
                pool_config,
                &runtime,
                now_unix_secs,
            ) == status
        });
    }

    if matches!(sort.field, AdminPoolKeySortField::Score) {
        let key_ids = keys.iter().map(|key| key.id.clone()).collect::<Vec<_>>();
        let scores = read_admin_pool_scores_by_key_id(state, &provider.id, &key_ids)
            .await
            .unwrap_or_default();
        admin_pool_sort_keys_by_score(&mut keys, &scores, sort.direction);
        Ok((keys, Some(scores)))
    } else {
        admin_pool_sort_keys_for_request(&mut keys, sort);
        Ok((keys, None))
    }
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
    let search_scope = match parse_admin_pool_search_scope(query) {
        Ok(value) => value,
        Err(detail) => {
            return Ok(build_admin_pool_error_response(
                http::StatusCode::BAD_REQUEST,
                detail,
            ));
        }
    };
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
    let search_scope =
        admin_pool_effective_search_scope(search_scope, &status, &quick_selectors, sort);

    let (keys, total, preloaded_pool_scores_by_key_id) = if status != "all"
        || !quick_selectors.is_empty()
        || sort_by_score
        || (matches!(search_scope, AdminPoolSearchScope::Full) && search.is_some())
    {
        let (keys, preloaded_pool_scores_by_key_id) = read_admin_pool_filtered_sorted_keys(
            state,
            &provider,
            pool_config.as_ref(),
            search.as_deref(),
            search_scope,
            &quick_selectors,
            &status,
            sort,
            now_unix_secs,
        )
        .await?;
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
    let codex_cycle_usage_by_key = read_admin_pool_codex_cycle_usage_by_key(
        state,
        &provider.provider_type,
        &keys,
        now_unix_secs,
    )
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
                codex_cycle_usage_by_key.get(&key.id),
                now_unix_secs,
            )
        })
        .collect::<Vec<_>>();

    let payload = json!({
        "total": total,
        "page": page,
        "page_size": page_size,
        "keys": items,
    });

    Ok(Json(payload).into_response())
}

pub(super) async fn build_admin_pool_create_selection_snapshot_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
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
    let payload = match request_body {
        Some(body) if !body.is_empty() => {
            match serde_json::from_slice::<
                admin_provider_pool_pure::AdminPoolCreateSelectionSnapshotRequest,
            >(body)
            {
                Ok(value) => value,
                Err(_) => {
                    return Ok(build_admin_pool_error_response(
                        http::StatusCode::BAD_REQUEST,
                        "Invalid JSON request body",
                    ));
                }
            }
        }
        _ => {
            return Ok(build_admin_pool_error_response(
                http::StatusCode::BAD_REQUEST,
                "Invalid JSON request body",
            ));
        }
    };

    let page = match parse_admin_pool_page_value(payload.page) {
        Ok(value) => value,
        Err(detail) => {
            return Ok(build_admin_pool_error_response(
                http::StatusCode::BAD_REQUEST,
                detail,
            ));
        }
    };
    let page_size = match parse_admin_pool_page_size_value(payload.page_size) {
        Ok(value) => value,
        Err(detail) => {
            return Ok(build_admin_pool_error_response(
                http::StatusCode::BAD_REQUEST,
                detail,
            ));
        }
    };
    let search = payload.search.trim().to_ascii_lowercase();
    let search = (!search.is_empty()).then_some(search);
    let search_scope = match parse_admin_pool_search_scope_value(
        (!payload.search_scope.trim().is_empty()).then_some(payload.search_scope.as_str()),
    ) {
        Ok(value) => value,
        Err(detail) => {
            return Ok(build_admin_pool_error_response(
                http::StatusCode::BAD_REQUEST,
                detail,
            ));
        }
    };
    let quick_selectors =
        admin_provider_pool_pure::admin_pool_sanitize_quick_selectors(payload.quick_selectors);
    let status_value = payload.status.trim();
    let status = match parse_admin_pool_status_filter_value(
        (!status_value.is_empty()).then_some(status_value),
    ) {
        Ok(value) => value,
        Err(detail) => {
            return Ok(build_admin_pool_error_response(
                http::StatusCode::BAD_REQUEST,
                detail,
            ));
        }
    };
    let sort = match parse_admin_pool_key_sort_values(
        payload.sort_by.as_deref(),
        payload.sort_order.as_deref(),
    ) {
        Ok(value) => value,
        Err(detail) => {
            return Ok(build_admin_pool_error_response(
                http::StatusCode::BAD_REQUEST,
                detail,
            ));
        }
    };
    let Some(expected_total) = payload.expected_total else {
        return Ok(build_admin_pool_error_response(
            http::StatusCode::BAD_REQUEST,
            "expected_total is required",
        ));
    };
    let expected_page_key_ids =
        admin_pool_normalized_expected_page_key_ids(payload.expected_page_key_ids);

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
    let now_unix_secs = admin_pool_current_unix_secs();
    let search_scope =
        admin_pool_effective_search_scope(search_scope, &status, &quick_selectors, sort);
    let (keys, _) = read_admin_pool_filtered_sorted_keys(
        state,
        &provider,
        pool_config.as_ref(),
        search.as_deref(),
        search_scope,
        &quick_selectors,
        &status,
        sort,
        now_unix_secs,
    )
    .await?;
    let total = keys.len();
    if total > ADMIN_POOL_SELECTION_SNAPSHOT_MAX_TOTAL {
        return Ok(build_admin_pool_error_response(
            http::StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "筛选结果过大（最多支持 {} 个账号），请缩小筛选范围后重试",
                ADMIN_POOL_SELECTION_SNAPSHOT_MAX_TOTAL
            ),
        ));
    }

    let (selection_snapshot, selection_snapshot_mismatch) =
        maybe_build_admin_pool_selection_snapshot_payload(
            state,
            request_context,
            &provider.id,
            search.as_deref(),
            &quick_selectors,
            &status,
            search_scope,
            sort,
            now_unix_secs,
            &keys,
            page_offset,
            page_size,
            Some(expected_total),
            Some(expected_page_key_ids.as_slice()),
        )
        .await?;

    let mut response_payload = json!({
        "total": total,
        "page": page,
        "page_size": page_size,
    });
    if let Some(selection_snapshot) = selection_snapshot {
        response_payload["selection_snapshot"] = selection_snapshot;
    }
    if let Some(selection_snapshot_mismatch) = selection_snapshot_mismatch {
        response_payload["selection_snapshot_mismatch"] = selection_snapshot_mismatch;
    }

    Ok(Json(response_payload).into_response())
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
