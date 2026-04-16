use super::leaderboard::{
    build_admin_stats_leaderboard_response, build_api_key_leaderboard_items_from_summaries,
    build_model_leaderboard_items_from_summaries, build_user_leaderboard_items_from_summaries,
    compare_leaderboard_items, load_user_leaderboard_metadata, AdminStatsLeaderboardNameMode,
};
use super::range::{parse_bounded_u32, parse_nonnegative_usize};
use super::resolve_admin_usage_time_range;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::{query_param_bool, query_param_value};
use crate::GatewayError;
use aether_admin::observability::stats::{
    admin_stats_bad_request_response, admin_stats_leaderboard_empty_response,
    AdminStatsLeaderboardMetric, AdminStatsSortOrder, AdminStatsUsageFilter,
};
use aether_data_contracts::repository::usage::{UsageLeaderboardGroupBy, UsageLeaderboardQuery};
use axum::{body::Body, http, response::Response};

pub(super) async fn maybe_build_local_admin_stats_leaderboard_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let query = request_context.query_string();

    if request_context
        .decision()
        .and_then(|decision| decision.route_kind.as_deref())
        == Some("leaderboard_models")
        && request_context.method() == http::Method::GET
        && matches!(
            request_context.path(),
            "/api/admin/stats/leaderboard/models" | "/api/admin/stats/leaderboard/models/"
        )
    {
        let time_range = match resolve_admin_usage_time_range(query) {
            Ok(value) => value,
            Err(detail) => return Ok(Some(admin_stats_bad_request_response(detail))),
        };
        let metric = match AdminStatsLeaderboardMetric::parse(query) {
            Ok(value) => value,
            Err(detail) => return Ok(Some(admin_stats_bad_request_response(detail))),
        };
        let order = match AdminStatsSortOrder::parse(query) {
            Ok(value) => value,
            Err(detail) => return Ok(Some(admin_stats_bad_request_response(detail))),
        };
        let limit = match query_param_value(query, "limit")
            .map(|value| parse_bounded_u32("limit", &value, 1, 100))
            .transpose()
        {
            Ok(Some(value)) => value as usize,
            Ok(None) => 10,
            Err(detail) => return Ok(Some(admin_stats_bad_request_response(detail))),
        };
        let offset = match query_param_value(query, "offset")
            .map(|value| parse_nonnegative_usize("offset", &value))
            .transpose()
        {
            Ok(Some(value)) => value,
            Ok(None) => 0,
            Err(detail) => return Ok(Some(admin_stats_bad_request_response(detail))),
        };
        if !state.has_usage_data_reader() {
            return Ok(Some(admin_stats_leaderboard_empty_response(
                metric,
                Some(&time_range),
            )));
        }
        let filters = AdminStatsUsageFilter::from_query(query);
        let Some((created_from_unix_secs, created_until_unix_secs)) = time_range.to_unix_bounds()
        else {
            return Ok(Some(admin_stats_leaderboard_empty_response(
                metric,
                Some(&time_range),
            )));
        };
        let summaries = state
            .summarize_usage_leaderboard(&UsageLeaderboardQuery {
                created_from_unix_secs,
                created_until_unix_secs,
                group_by: UsageLeaderboardGroupBy::Model,
                user_id: filters.user_id,
                provider_name: filters.provider_name,
                model: filters.model,
            })
            .await?;
        let mut leaderboard = build_model_leaderboard_items_from_summaries(&summaries);
        leaderboard.sort_by(|left, right| compare_leaderboard_items(metric, order, left, right));

        return Ok(Some(build_admin_stats_leaderboard_response(
            metric,
            Some(&time_range),
            &leaderboard,
            offset,
            limit,
            AdminStatsLeaderboardNameMode::Id,
        )));
    }

    if request_context
        .decision()
        .and_then(|decision| decision.route_kind.as_deref())
        == Some("leaderboard_api_keys")
        && request_context.method() == http::Method::GET
        && matches!(
            request_context.path(),
            "/api/admin/stats/leaderboard/api-keys" | "/api/admin/stats/leaderboard/api-keys/"
        )
    {
        let time_range = match resolve_admin_usage_time_range(query) {
            Ok(value) => value,
            Err(detail) => return Ok(Some(admin_stats_bad_request_response(detail))),
        };
        let metric = match AdminStatsLeaderboardMetric::parse(query) {
            Ok(value) => value,
            Err(detail) => return Ok(Some(admin_stats_bad_request_response(detail))),
        };
        let order = match AdminStatsSortOrder::parse(query) {
            Ok(value) => value,
            Err(detail) => return Ok(Some(admin_stats_bad_request_response(detail))),
        };
        let limit = match query_param_value(query, "limit")
            .map(|value| parse_bounded_u32("limit", &value, 1, 100))
            .transpose()
        {
            Ok(Some(value)) => value as usize,
            Ok(None) => 10,
            Err(detail) => return Ok(Some(admin_stats_bad_request_response(detail))),
        };
        let offset = match query_param_value(query, "offset")
            .map(|value| parse_nonnegative_usize("offset", &value))
            .transpose()
        {
            Ok(Some(value)) => value,
            Ok(None) => 0,
            Err(detail) => return Ok(Some(admin_stats_bad_request_response(detail))),
        };
        if !state.has_usage_data_reader() {
            return Ok(Some(admin_stats_leaderboard_empty_response(
                metric,
                Some(&time_range),
            )));
        }
        let include_inactive = query_param_bool(query, "include_inactive", false);
        let exclude_admin = query_param_bool(query, "exclude_admin", false);
        let filters = AdminStatsUsageFilter::from_query(query);
        let Some((created_from_unix_secs, created_until_unix_secs)) = time_range.to_unix_bounds()
        else {
            return Ok(Some(admin_stats_leaderboard_empty_response(
                metric,
                Some(&time_range),
            )));
        };
        let summaries = state
            .summarize_usage_leaderboard(&UsageLeaderboardQuery {
                created_from_unix_secs,
                created_until_unix_secs,
                group_by: UsageLeaderboardGroupBy::ApiKey,
                user_id: filters.user_id,
                provider_name: filters.provider_name,
                model: filters.model,
            })
            .await?;
        let api_key_ids: Vec<String> = summaries
            .iter()
            .map(|item| item.group_key.clone())
            .collect();
        let snapshots = if state.has_auth_api_key_data_reader() {
            Some(
                state
                    .resolve_auth_api_key_snapshots_by_ids(&api_key_ids)
                    .await?,
            )
        } else {
            None
        };
        let api_key_names = if state.has_auth_api_key_data_reader() {
            state
                .resolve_auth_api_key_names_by_ids(&api_key_ids)
                .await?
        } else {
            std::collections::BTreeMap::new()
        };
        let mut leaderboard = build_api_key_leaderboard_items_from_summaries(
            &summaries,
            snapshots.as_deref(),
            &api_key_names,
            include_inactive,
            exclude_admin,
        );
        leaderboard.sort_by(|left, right| compare_leaderboard_items(metric, order, left, right));

        return Ok(Some(build_admin_stats_leaderboard_response(
            metric,
            Some(&time_range),
            &leaderboard,
            offset,
            limit,
            AdminStatsLeaderboardNameMode::Name,
        )));
    }

    if request_context
        .decision()
        .and_then(|decision| decision.route_kind.as_deref())
        == Some("leaderboard_users")
        && request_context.method() == http::Method::GET
        && matches!(
            request_context.path(),
            "/api/admin/stats/leaderboard/users" | "/api/admin/stats/leaderboard/users/"
        )
    {
        let time_range = match resolve_admin_usage_time_range(query) {
            Ok(value) => value,
            Err(detail) => return Ok(Some(admin_stats_bad_request_response(detail))),
        };
        let metric = match AdminStatsLeaderboardMetric::parse(query) {
            Ok(value) => value,
            Err(detail) => return Ok(Some(admin_stats_bad_request_response(detail))),
        };
        let order = match AdminStatsSortOrder::parse(query) {
            Ok(value) => value,
            Err(detail) => return Ok(Some(admin_stats_bad_request_response(detail))),
        };
        let limit = match query_param_value(query, "limit")
            .map(|value| parse_bounded_u32("limit", &value, 1, 100))
            .transpose()
        {
            Ok(Some(value)) => value as usize,
            Ok(None) => 10,
            Err(detail) => return Ok(Some(admin_stats_bad_request_response(detail))),
        };
        let offset = match query_param_value(query, "offset")
            .map(|value| parse_nonnegative_usize("offset", &value))
            .transpose()
        {
            Ok(Some(value)) => value,
            Ok(None) => 0,
            Err(detail) => return Ok(Some(admin_stats_bad_request_response(detail))),
        };
        if !state.has_usage_data_reader() {
            return Ok(Some(admin_stats_leaderboard_empty_response(
                metric,
                Some(&time_range),
            )));
        }
        let include_inactive = query_param_bool(query, "include_inactive", false);
        let exclude_admin = query_param_bool(query, "exclude_admin", false);
        let filters = AdminStatsUsageFilter::from_query(query);
        let Some((created_from_unix_secs, created_until_unix_secs)) = time_range.to_unix_bounds()
        else {
            return Ok(Some(admin_stats_leaderboard_empty_response(
                metric,
                Some(&time_range),
            )));
        };
        let summaries = state
            .summarize_usage_leaderboard(&UsageLeaderboardQuery {
                created_from_unix_secs,
                created_until_unix_secs,
                group_by: UsageLeaderboardGroupBy::User,
                user_id: filters.user_id,
                provider_name: filters.provider_name,
                model: filters.model,
            })
            .await?;
        let user_ids: Vec<String> = summaries
            .iter()
            .map(|item| item.group_key.clone())
            .collect();
        let user_metadata = load_user_leaderboard_metadata(state, &user_ids).await?;
        let mut leaderboard = build_user_leaderboard_items_from_summaries(
            &summaries,
            &user_metadata,
            state.has_auth_user_data_reader(),
            state.has_user_data_reader(),
            include_inactive,
            exclude_admin,
        );
        leaderboard.sort_by(|left, right| compare_leaderboard_items(metric, order, left, right));

        return Ok(Some(build_admin_stats_leaderboard_response(
            metric,
            Some(&time_range),
            &leaderboard,
            offset,
            limit,
            AdminStatsLeaderboardNameMode::Name,
        )));
    }

    Ok(None)
}
