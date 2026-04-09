use super::super::stats::{AdminStatsTimeRange, AdminStatsUsageFilter};
use super::analytics::admin_usage_provider_key_names;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::query_param_value;
use crate::GatewayError;
use aether_admin::observability::usage::{
    admin_usage_bad_request_response, admin_usage_data_unavailable_response,
    admin_usage_matches_api_format, admin_usage_matches_eq, admin_usage_matches_search,
    admin_usage_matches_status, admin_usage_matches_username, admin_usage_parse_ids,
    admin_usage_parse_limit, admin_usage_parse_offset, build_admin_usage_active_requests_response,
    build_admin_usage_records_response, build_admin_usage_summary_stats_response,
    ADMIN_USAGE_DATA_UNAVAILABLE_DETAIL,
};
use aether_data_contracts::repository::usage::UsageAuditListQuery;
use axum::{body::Body, http, response::Response};
use std::collections::{BTreeMap, BTreeSet};

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
            let time_range = match AdminStatsTimeRange::resolve_optional(query) {
                Ok(value) => value,
                Err(detail) => return Ok(Some(admin_usage_bad_request_response(detail))),
            };
            let usage = state
                .list_admin_usage_for_optional_range(
                    time_range.as_ref(),
                    &AdminStatsUsageFilter::default(),
                )
                .await?;
            return Ok(Some(build_admin_usage_summary_stats_response(&usage)));
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
            let usage = state
                .list_usage_audits(&UsageAuditListQuery::default())
                .await?;
            let mut items: Vec<_> = usage
                .into_iter()
                .filter(|item| match requested_ids.as_ref() {
                    Some(ids) => ids.contains(&item.id),
                    None => matches!(item.status.as_str(), "pending" | "streaming"),
                })
                .collect();
            items.sort_by(|left, right| {
                right
                    .created_at_unix_secs
                    .cmp(&left.created_at_unix_secs)
                    .then_with(|| left.id.cmp(&right.id))
            });
            if requested_ids.is_none() && items.len() > 50 {
                items.truncate(50);
            }
            let provider_key_names = admin_usage_provider_key_names(state, &items).await?;

            return Ok(Some(build_admin_usage_active_requests_response(
                &items,
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
            let time_range = match AdminStatsTimeRange::resolve_optional(query) {
                Ok(value) => value,
                Err(detail) => return Ok(Some(admin_usage_bad_request_response(detail))),
            };
            let filters = AdminStatsUsageFilter {
                user_id: query_param_value(query, "user_id"),
                provider_name: None,
                model: None,
            };
            let mut usage = state
                .list_admin_usage_for_optional_range(time_range.as_ref(), &filters)
                .await?;

            let search = query_param_value(query, "search");
            let username_filter = query_param_value(query, "username");
            let model_filter = query_param_value(query, "model");
            let provider_filter = query_param_value(query, "provider");
            let api_format_filter = query_param_value(query, "api_format");
            let status_filter = query_param_value(query, "status");
            let limit = match admin_usage_parse_limit(query) {
                Ok(value) => value,
                Err(detail) => return Ok(Some(admin_usage_bad_request_response(detail))),
            };
            let offset = match admin_usage_parse_offset(query) {
                Ok(value) => value,
                Err(detail) => return Ok(Some(admin_usage_bad_request_response(detail))),
            };

            usage.retain(|item| {
                admin_usage_matches_search(item, search.as_deref())
                    && admin_usage_matches_username(item, username_filter.as_deref())
                    && admin_usage_matches_eq(item.model.as_str(), model_filter.as_deref())
                    && admin_usage_matches_eq(
                        item.provider_name.as_str(),
                        provider_filter.as_deref(),
                    )
                    && admin_usage_matches_api_format(item, api_format_filter.as_deref())
                    && admin_usage_matches_status(item, status_filter.as_deref())
            });
            usage.sort_by(|left, right| {
                right
                    .created_at_unix_secs
                    .cmp(&left.created_at_unix_secs)
                    .then_with(|| left.id.cmp(&right.id))
            });
            let total = usage.len();

            let user_ids: Vec<String> = usage
                .iter()
                .filter_map(|item| item.user_id.clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            let users_by_id: BTreeMap<String, aether_data::repository::users::StoredUserSummary> =
                if state.has_user_data_reader() && !user_ids.is_empty() {
                    state
                        .list_users_by_ids(&user_ids)
                        .await?
                        .into_iter()
                        .map(|user| (user.id.clone(), user))
                        .collect()
                } else {
                    BTreeMap::new()
                };

            let provider_key_names = admin_usage_provider_key_names(state, &usage).await?;

            let records = usage
                .into_iter()
                .skip(offset)
                .take(limit)
                .collect::<Vec<_>>();

            return Ok(Some(build_admin_usage_records_response(
                &records,
                &users_by_id,
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
