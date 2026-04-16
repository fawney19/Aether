use super::super::stats::resolve_admin_usage_time_range;
use super::analytics::admin_usage_api_key_names;
use super::analytics::admin_usage_provider_key_names;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::query_param_value;
use crate::GatewayError;
use aether_admin::observability::usage::{
    admin_usage_bad_request_response, admin_usage_data_unavailable_response,
    admin_usage_matches_search, admin_usage_matches_username, admin_usage_parse_ids,
    admin_usage_parse_limit, admin_usage_parse_offset, build_admin_usage_active_requests_response,
    build_admin_usage_records_response, build_admin_usage_summary_stats_response_from_summary,
    ADMIN_USAGE_DATA_UNAVAILABLE_DETAIL,
};
use aether_data_contracts::repository::usage::{
    StoredRequestUsageAudit, UsageAuditListQuery, UsageAuditSummaryQuery,
};
use axum::{body::Body, http, response::Response};
use std::collections::{BTreeMap, BTreeSet};

const ADMIN_USAGE_ACTIVE_LIMIT: usize = 50;

async fn load_admin_usage_by_ids(
    state: &AdminAppState<'_>,
    requested_ids: &BTreeSet<String>,
) -> Result<Vec<StoredRequestUsageAudit>, GatewayError> {
    let mut items = Vec::with_capacity(requested_ids.len());
    for usage_id in requested_ids {
        if let Some(item) = state.find_request_usage_by_id(usage_id).await? {
            items.push(item);
        }
    }
    Ok(items)
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
        _ => {}
    }
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
            let mut items = if let Some(requested_ids) = requested_ids.as_ref() {
                load_admin_usage_by_ids(state, requested_ids).await?
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
            sort_usage_newest_first(&mut items);
            if requested_ids.is_none() && items.len() > ADMIN_USAGE_ACTIVE_LIMIT {
                items.truncate(ADMIN_USAGE_ACTIVE_LIMIT);
            }
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
            let use_metadata_fallback = search.is_some() || username_filter.is_some();
            let (usage, total) = if use_metadata_fallback {
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

                usage.retain(|item| {
                    admin_usage_matches_search(
                        item,
                        search.as_deref(),
                        &users_by_id,
                        &api_key_names,
                        state.has_auth_user_data_reader(),
                        state.has_auth_api_key_data_reader(),
                    ) && admin_usage_matches_username(
                        item,
                        username_filter.as_deref(),
                        &users_by_id,
                        state.has_auth_user_data_reader(),
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

            return Ok(Some(build_admin_usage_records_response(
                &usage,
                &users_by_id,
                &api_key_names,
                state.has_auth_user_data_reader(),
                state.has_auth_api_key_data_reader(),
                &provider_key_names,
                total,
                limit,
                offset,
            )));
        }
        _ => {}
    }

    Ok(None)
}
