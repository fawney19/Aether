use super::super::stats::resolve_admin_usage_time_range;
use super::analytics::admin_usage_api_key_names;
use super::analytics::admin_usage_provider_key_names;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::query_param_value;
use crate::GatewayError;
use aether_admin::observability::usage::{
    admin_usage_bad_request_response, admin_usage_data_unavailable_response,
    admin_usage_has_fallback, admin_usage_is_failed, admin_usage_matches_search,
    admin_usage_matches_username, admin_usage_parse_ids, admin_usage_parse_limit,
    admin_usage_parse_offset, admin_usage_provider_key_name, admin_usage_record_json,
    build_admin_usage_active_requests_response, build_admin_usage_records_response,
    build_admin_usage_summary_stats_response_from_summary, ADMIN_USAGE_DATA_UNAVAILABLE_DETAIL,
};
use aether_data::repository::users::StoredUserSummary;
use aether_data_contracts::repository::{
    candidates::{RequestCandidateStatus, StoredRequestCandidate},
    usage::{
        StoredRequestUsageAudit, UsageAuditKeywordSearchQuery, UsageAuditListQuery,
        UsageAuditSummaryQuery,
    },
};
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

const ADMIN_USAGE_ACTIVE_LIMIT: usize = 50;

async fn load_admin_usage_by_ids(
    state: &AdminAppState<'_>,
    requested_ids: &BTreeSet<String>,
) -> Result<Vec<StoredRequestUsageAudit>, GatewayError> {
    let usage_ids = requested_ids.iter().cloned().collect::<Vec<_>>();
    state.list_request_usage_by_ids(&usage_ids).await
}

fn sort_usage_newest_first(items: &mut [StoredRequestUsageAudit]) {
    items.sort_by(|left, right| {
        right
            .created_at_unix_ms
            .cmp(&left.created_at_unix_ms)
            .then_with(|| left.id.cmp(&right.id))
    });
}

fn apply_admin_usage_status_filter(query: &mut UsageAuditListQuery, status: Option<&str>) {
    let Some(status) = status
        .map(str::trim)
        .filter(|candidate| !candidate.is_empty())
    else {
        return;
    };

    match status {
        "stream" => query.is_stream = Some(true),
        "standard" => query.is_stream = Some(false),
        "error" | "failed" => query.error_only = true,
        "active" => {
            query.statuses = Some(vec!["pending".to_string(), "streaming".to_string()]);
        }
        "pending" | "streaming" | "completed" | "cancelled" => {
            query.statuses = Some(vec![status.to_string()]);
        }
        "has_fallback" | "has_retry" => {}
        _ => {}
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct AdminUsageAttemptFlags {
    has_fallback: bool,
    has_retry: bool,
}

fn admin_usage_attempt_status_filter(status: Option<&str>) -> Option<&'static str> {
    match status?.trim().to_ascii_lowercase().as_str() {
        "has_fallback" => Some("has_fallback"),
        "has_retry" => Some("has_retry"),
        _ => None,
    }
}

fn admin_usage_candidate_failed_before_fallback(candidate: &StoredRequestCandidate) -> bool {
    candidate.status.is_attempted(candidate.started_at_unix_ms)
        && (matches!(
            candidate.status,
            RequestCandidateStatus::Failed | RequestCandidateStatus::Cancelled
        ) || candidate.status_code.is_some_and(|code| code >= 400))
}

fn admin_usage_candidate_was_retried(candidate: &StoredRequestCandidate) -> bool {
    candidate.retry_index > 0 && candidate.status.is_attempted(candidate.started_at_unix_ms)
}

fn admin_usage_final_candidate_index(
    item: &StoredRequestUsageAudit,
    candidates: &[StoredRequestCandidate],
) -> Option<u64> {
    if let Some(candidate_id) = item.routing_candidate_id() {
        if let Some(candidate) = candidates
            .iter()
            .find(|candidate| candidate.id == candidate_id)
        {
            return Some(u64::from(candidate.candidate_index));
        }
    }

    item.routing_candidate_index().or_else(|| {
        candidates
            .iter()
            .filter(|candidate| candidate.status.is_attempted(candidate.started_at_unix_ms))
            .max_by(|left, right| {
                left.candidate_index
                    .cmp(&right.candidate_index)
                    .then(left.retry_index.cmp(&right.retry_index))
            })
            .map(|candidate| u64::from(candidate.candidate_index))
    })
}

fn admin_usage_attempt_flags_from_candidates(
    item: &StoredRequestUsageAudit,
    candidates: &[StoredRequestCandidate],
) -> AdminUsageAttemptFlags {
    let final_candidate_index = admin_usage_final_candidate_index(item, candidates);
    let has_fallback = final_candidate_index.is_some_and(|final_index| {
        candidates.iter().any(|candidate| {
            u64::from(candidate.candidate_index) < final_index
                && admin_usage_candidate_failed_before_fallback(candidate)
        })
    });
    let has_retry = candidates.iter().any(admin_usage_candidate_was_retried);

    AdminUsageAttemptFlags {
        has_fallback,
        has_retry,
    }
}

fn admin_usage_attempt_flags_for_item(
    item: &StoredRequestUsageAudit,
    flags_by_usage_id: &BTreeMap<String, AdminUsageAttemptFlags>,
    request_candidate_reader_available: bool,
) -> AdminUsageAttemptFlags {
    flags_by_usage_id.get(&item.id).copied().unwrap_or_else(|| {
        if request_candidate_reader_available {
            AdminUsageAttemptFlags::default()
        } else {
            AdminUsageAttemptFlags {
                has_fallback: admin_usage_has_fallback(item),
                has_retry: false,
            }
        }
    })
}

async fn resolve_admin_usage_attempt_flags_by_usage_id(
    state: &AdminAppState<'_>,
    items: &[StoredRequestUsageAudit],
) -> Result<BTreeMap<String, AdminUsageAttemptFlags>, GatewayError> {
    if !state.has_request_candidate_data_reader() || items.is_empty() {
        return Ok(BTreeMap::new());
    }

    let request_ids = items
        .iter()
        .map(|item| item.request_id.clone())
        .collect::<BTreeSet<_>>();
    let mut candidates_by_request_id = BTreeMap::new();
    for request_id in request_ids {
        candidates_by_request_id.insert(
            request_id.clone(),
            state
                .app()
                .read_request_candidates_by_request_id(&request_id)
                .await?,
        );
    }

    Ok(items
        .iter()
        .filter_map(|item| {
            let candidates = candidates_by_request_id.get(&item.request_id)?;
            Some((
                item.id.clone(),
                admin_usage_attempt_flags_from_candidates(item, candidates),
            ))
        })
        .collect())
}

fn admin_usage_matches_attempt_status(
    item: &StoredRequestUsageAudit,
    status: &str,
    flags_by_usage_id: &BTreeMap<String, AdminUsageAttemptFlags>,
    request_candidate_reader_available: bool,
) -> bool {
    let flags = admin_usage_attempt_flags_for_item(
        item,
        flags_by_usage_id,
        request_candidate_reader_available,
    );
    match status {
        "has_fallback" => flags.has_fallback,
        "has_retry" => flags.has_retry,
        _ => true,
    }
}

#[allow(clippy::too_many_arguments)]
fn build_admin_usage_records_response_with_attempt_flags(
    items: &[StoredRequestUsageAudit],
    users_by_id: &BTreeMap<String, StoredUserSummary>,
    api_key_names: &BTreeMap<String, String>,
    auth_user_reader_available: bool,
    auth_api_key_reader_available: bool,
    provider_key_names: &BTreeMap<String, String>,
    attempt_flags_by_usage_id: &BTreeMap<String, AdminUsageAttemptFlags>,
    request_candidate_reader_available: bool,
    total: usize,
    limit: usize,
    offset: usize,
) -> Response<Body> {
    let records: Vec<_> = items
        .iter()
        .map(|item| {
            let provider_key_name = admin_usage_provider_key_name(item, provider_key_names);
            let mut record = admin_usage_record_json(
                item,
                users_by_id,
                api_key_names,
                auth_user_reader_available,
                auth_api_key_reader_available,
                provider_key_name.as_deref(),
            );
            let flags = admin_usage_attempt_flags_for_item(
                item,
                attempt_flags_by_usage_id,
                request_candidate_reader_available,
            );
            record["has_fallback"] = json!(flags.has_fallback);
            record["has_retry"] = json!(flags.has_retry);
            record
        })
        .collect();

    Json(json!({
        "records": records,
        "total": total,
        "limit": limit,
        "offset": offset,
    }))
    .into_response()
}

fn build_admin_usage_records_query(
    created_from_unix_secs: u64,
    created_until_unix_secs: u64,
    query: Option<&str>,
    limit: Option<usize>,
    offset: Option<usize>,
) -> UsageAuditListQuery {
    let mut list_query = UsageAuditListQuery {
        created_from_unix_secs: Some(created_from_unix_secs),
        created_until_unix_secs: Some(created_until_unix_secs),
        user_id: query_param_value(query, "user_id"),
        provider_name: query_param_value(query, "provider"),
        model: query_param_value(query, "model"),
        api_format: query_param_value(query, "api_format"),
        limit,
        offset,
        newest_first: true,
        ..Default::default()
    };
    apply_admin_usage_status_filter(
        &mut list_query,
        query_param_value(query, "status").as_deref(),
    );
    list_query
}

fn parse_admin_usage_search_keywords(search: &str) -> Vec<String> {
    search
        .split_whitespace()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase())
        .collect()
}

#[derive(Default)]
struct AdminUsageSearchContext {
    matched_user_ids_by_keyword: Vec<Vec<String>>,
    matched_api_key_ids_by_keyword: Vec<Vec<String>>,
    matched_user_ids_for_username: Vec<String>,
}

async fn resolve_admin_usage_search_context(
    state: &AdminAppState<'_>,
    keywords: &[String],
    username_filter: Option<&str>,
) -> Result<AdminUsageSearchContext, GatewayError> {
    let auth_user_reader_available = state.has_auth_user_data_reader();
    let auth_api_key_reader_available = state.has_auth_api_key_data_reader();
    let username_filter = username_filter
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase());
    let mut matched_user_ids_cache = BTreeMap::<String, Vec<String>>::new();
    let mut matched_api_key_ids_cache = BTreeMap::<String, Vec<String>>::new();

    if auth_user_reader_available {
        for keyword in keywords {
            matched_user_ids_cache.entry(keyword.clone()).or_insert(
                state
                    .search_auth_user_summaries_by_username(keyword)
                    .await?
                    .into_iter()
                    .map(|user| user.id)
                    .collect(),
            );
        }
        if let Some(username_keyword) = username_filter.as_ref() {
            matched_user_ids_cache
                .entry(username_keyword.clone())
                .or_insert(
                    state
                        .search_auth_user_summaries_by_username(username_keyword)
                        .await?
                        .into_iter()
                        .map(|user| user.id)
                        .collect(),
                );
        }
    }

    if auth_api_key_reader_available {
        for keyword in keywords {
            matched_api_key_ids_cache.entry(keyword.clone()).or_insert(
                state
                    .list_auth_api_key_export_records_by_name_search(keyword)
                    .await?
                    .into_iter()
                    .map(|record| record.api_key_id)
                    .collect(),
            );
        }
    }

    Ok(AdminUsageSearchContext {
        matched_user_ids_by_keyword: keywords
            .iter()
            .map(|keyword| {
                matched_user_ids_cache
                    .get(keyword)
                    .cloned()
                    .unwrap_or_default()
            })
            .collect(),
        matched_api_key_ids_by_keyword: keywords
            .iter()
            .map(|keyword| {
                matched_api_key_ids_cache
                    .get(keyword)
                    .cloned()
                    .unwrap_or_default()
            })
            .collect(),
        matched_user_ids_for_username: username_filter
            .as_ref()
            .and_then(|keyword| matched_user_ids_cache.get(keyword))
            .cloned()
            .unwrap_or_default(),
    })
}

fn build_admin_usage_keyword_search_query(
    base_query: &UsageAuditListQuery,
    keywords: Vec<String>,
    username_keyword: Option<String>,
    search_context: AdminUsageSearchContext,
    auth_user_reader_available: bool,
    auth_api_key_reader_available: bool,
    limit: Option<usize>,
    offset: Option<usize>,
) -> UsageAuditKeywordSearchQuery {
    UsageAuditKeywordSearchQuery {
        created_from_unix_secs: base_query.created_from_unix_secs,
        created_until_unix_secs: base_query.created_until_unix_secs,
        user_id: base_query.user_id.clone(),
        provider_name: base_query.provider_name.clone(),
        model: base_query.model.clone(),
        api_format: base_query.api_format.clone(),
        statuses: base_query.statuses.clone(),
        is_stream: base_query.is_stream,
        error_only: base_query.error_only,
        keywords,
        matched_user_ids_by_keyword: search_context.matched_user_ids_by_keyword,
        auth_user_reader_available,
        matched_api_key_ids_by_keyword: search_context.matched_api_key_ids_by_keyword,
        auth_api_key_reader_available,
        username_keyword,
        matched_user_ids_for_username: search_context.matched_user_ids_for_username,
        limit,
        offset,
        newest_first: true,
    }
}

pub(super) async fn maybe_build_local_admin_usage_summary_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let route_kind = request_context
        .control_decision
        .as_ref()
        .and_then(|decision| decision.route_kind.as_deref());

    match route_kind {
        Some("stats")
            if request_context.request_method == http::Method::GET
                && matches!(
                    request_context.request_path.as_str(),
                    "/api/admin/usage/stats" | "/api/admin/usage/stats/"
                ) =>
        {
            if !state.has_usage_data_reader() {
                return Ok(Some(admin_usage_data_unavailable_response(
                    ADMIN_USAGE_DATA_UNAVAILABLE_DETAIL,
                )));
            }

            let query = request_context.request_query_string.as_deref();
            let time_range = match resolve_admin_usage_time_range(query) {
                Ok(value) => value,
                Err(detail) => return Ok(Some(admin_usage_bad_request_response(detail))),
            };
            let Some((created_from_unix_secs, created_until_unix_secs)) =
                time_range.to_unix_bounds()
            else {
                return Ok(Some(build_admin_usage_summary_stats_response_from_summary(
                    &Default::default(),
                )));
            };
            let summary = state
                .summarize_usage_audits(&UsageAuditSummaryQuery {
                    created_from_unix_secs,
                    created_until_unix_secs,
                    ..Default::default()
                })
                .await?;
            return Ok(Some(build_admin_usage_summary_stats_response_from_summary(
                &summary,
            )));
        }
        Some("active")
            if request_context.request_method == http::Method::GET
                && matches!(
                    request_context.request_path.as_str(),
                    "/api/admin/usage/active" | "/api/admin/usage/active/"
                ) =>
        {
            if !state.has_usage_data_reader() {
                return Ok(Some(admin_usage_data_unavailable_response(
                    ADMIN_USAGE_DATA_UNAVAILABLE_DETAIL,
                )));
            }

            let query = request_context.request_query_string.as_deref();
            let requested_ids = admin_usage_parse_ids(query);
            let items = if let Some(requested_ids) = requested_ids.as_ref() {
                let mut items = load_admin_usage_by_ids(state, requested_ids).await?;
                sort_usage_newest_first(&mut items);
                items
            } else {
                let time_range = match resolve_admin_usage_time_range(query) {
                    Ok(value) => value,
                    Err(detail) => return Ok(Some(admin_usage_bad_request_response(detail))),
                };
                let Some((created_from_unix_secs, created_until_unix_secs)) =
                    time_range.to_unix_bounds()
                else {
                    return Ok(Some(build_admin_usage_active_requests_response(
                        &[],
                        &BTreeMap::new(),
                        state.has_auth_api_key_data_reader(),
                        &BTreeMap::new(),
                    )));
                };
                state
                    .list_usage_audits(&UsageAuditListQuery {
                        created_from_unix_secs: Some(created_from_unix_secs),
                        created_until_unix_secs: Some(created_until_unix_secs),
                        statuses: Some(vec!["pending".to_string(), "streaming".to_string()]),
                        limit: Some(ADMIN_USAGE_ACTIVE_LIMIT),
                        newest_first: true,
                        ..Default::default()
                    })
                    .await?
            };
            let items = if requested_ids.is_some() {
                items
            } else {
                items
                    .into_iter()
                    .filter(|item| !admin_usage_is_failed(item))
                    .collect::<Vec<_>>()
            };
            let api_key_names = admin_usage_api_key_names(state, &items).await?;
            let provider_key_names = admin_usage_provider_key_names(state, &items).await?;

            return Ok(Some(build_admin_usage_active_requests_response(
                &items,
                &api_key_names,
                state.has_auth_api_key_data_reader(),
                &provider_key_names,
            )));
        }
        Some("records")
            if request_context.request_method == http::Method::GET
                && matches!(
                    request_context.request_path.as_str(),
                    "/api/admin/usage/records" | "/api/admin/usage/records/"
                ) =>
        {
            if !state.has_usage_data_reader() {
                return Ok(Some(admin_usage_data_unavailable_response(
                    ADMIN_USAGE_DATA_UNAVAILABLE_DETAIL,
                )));
            }

            let query = request_context.request_query_string.as_deref();
            let time_range = match resolve_admin_usage_time_range(query) {
                Ok(value) => value,
                Err(detail) => return Ok(Some(admin_usage_bad_request_response(detail))),
            };
            let attempt_status_filter =
                admin_usage_attempt_status_filter(query_param_value(query, "status").as_deref());
            let search = query_param_value(query, "search");
            let username_filter = query_param_value(query, "username");
            let limit = match admin_usage_parse_limit(query) {
                Ok(value) => value,
                Err(detail) => return Ok(Some(admin_usage_bad_request_response(detail))),
            };
            let offset = match admin_usage_parse_offset(query) {
                Ok(value) => value,
                Err(detail) => return Ok(Some(admin_usage_bad_request_response(detail))),
            };
            let Some((created_from_unix_secs, created_until_unix_secs)) =
                time_range.to_unix_bounds()
            else {
                return Ok(Some(build_admin_usage_records_response(
                    &[],
                    &BTreeMap::new(),
                    &BTreeMap::new(),
                    state.has_auth_user_data_reader(),
                    state.has_auth_api_key_data_reader(),
                    &BTreeMap::new(),
                    0,
                    limit,
                    offset,
                )));
            };
            let base_query = build_admin_usage_records_query(
                created_from_unix_secs,
                created_until_unix_secs,
                query,
                None,
                None,
            );
            let active_search = search.as_deref().filter(|value| !value.trim().is_empty());
            let active_username_filter = username_filter
                .as_deref()
                .filter(|value| !value.trim().is_empty());
            let (usage, total) = if let Some(attempt_status) = attempt_status_filter {
                let mut usage = state.list_usage_audits(&base_query).await?;
                let user_ids: Vec<String> = usage
                    .iter()
                    .filter_map(|item| item.user_id.clone())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect();
                let users_by_id: BTreeMap<
                    String,
                    aether_data::repository::users::StoredUserSummary,
                > = state.resolve_auth_user_summaries_by_ids(&user_ids).await?;
                let api_key_names = admin_usage_api_key_names(state, &usage).await?;
                let attempt_flags_by_usage_id =
                    resolve_admin_usage_attempt_flags_by_usage_id(state, &usage).await?;
                let request_candidate_reader_available = state.has_request_candidate_data_reader();

                usage.retain(|item| {
                    admin_usage_matches_search(
                        item,
                        active_search,
                        &users_by_id,
                        &api_key_names,
                        state.has_auth_user_data_reader(),
                        state.has_auth_api_key_data_reader(),
                    ) && admin_usage_matches_username(
                        item,
                        active_username_filter,
                        &users_by_id,
                        state.has_auth_user_data_reader(),
                    ) && admin_usage_matches_attempt_status(
                        item,
                        attempt_status,
                        &attempt_flags_by_usage_id,
                        request_candidate_reader_available,
                    )
                });
                sort_usage_newest_first(&mut usage);
                let total = usage.len();
                let records = usage
                    .into_iter()
                    .skip(offset)
                    .take(limit)
                    .collect::<Vec<_>>();
                (records, total)
            } else if active_search.is_some() || active_username_filter.is_some() {
                let keywords = active_search
                    .map(parse_admin_usage_search_keywords)
                    .unwrap_or_default();
                let auth_user_reader_available = state.has_auth_user_data_reader();
                let auth_api_key_reader_available = state.has_auth_api_key_data_reader();
                let search_context =
                    resolve_admin_usage_search_context(state, &keywords, active_username_filter)
                        .await?;
                let keyword_query = build_admin_usage_keyword_search_query(
                    &base_query,
                    keywords,
                    active_username_filter.map(str::to_owned),
                    search_context,
                    auth_user_reader_available,
                    auth_api_key_reader_available,
                    None,
                    None,
                );
                let total = usize::try_from(
                    state
                        .count_usage_audits_by_keyword_search(&keyword_query)
                        .await?,
                )
                .unwrap_or(usize::MAX);
                let paged_query = UsageAuditKeywordSearchQuery {
                    limit: Some(limit),
                    offset: Some(offset),
                    ..keyword_query
                };
                (
                    state
                        .list_usage_audits_by_keyword_search(&paged_query)
                        .await?,
                    total,
                )
            } else {
                let total = usize::try_from(state.count_usage_audits(&base_query).await?)
                    .unwrap_or(usize::MAX);
                let mut paged_query = base_query.clone();
                paged_query.limit = Some(limit);
                paged_query.offset = Some(offset);
                (state.list_usage_audits(&paged_query).await?, total)
            };

            let user_ids: Vec<String> = usage
                .iter()
                .filter_map(|item| item.user_id.clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            let users_by_id: BTreeMap<String, aether_data::repository::users::StoredUserSummary> =
                state.resolve_auth_user_summaries_by_ids(&user_ids).await?;
            let api_key_names = admin_usage_api_key_names(state, &usage).await?;
            let provider_key_names = admin_usage_provider_key_names(state, &usage).await?;
            let attempt_flags_by_usage_id =
                resolve_admin_usage_attempt_flags_by_usage_id(state, &usage).await?;

            return Ok(Some(build_admin_usage_records_response_with_attempt_flags(
                &usage,
                &users_by_id,
                &api_key_names,
                state.has_auth_user_data_reader(),
                state.has_auth_api_key_data_reader(),
                &provider_key_names,
                &attempt_flags_by_usage_id,
                state.has_request_candidate_data_reader(),
                total,
                limit,
                offset,
            )));
        }
        _ => {}
    }

    Ok(None)
}
