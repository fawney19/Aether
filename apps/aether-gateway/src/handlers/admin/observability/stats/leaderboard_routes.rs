use super::super::resolve_usage_user_group_scope;
use super::leaderboard::{
    build_admin_stats_leaderboard_response, build_admin_stats_user_group_leaderboard_response,
    build_api_key_leaderboard_items_from_summaries, build_model_leaderboard_items_from_summaries,
    build_user_leaderboard_items_from_summaries, compare_leaderboard_items,
    load_user_leaderboard_metadata, AdminStatsLeaderboardItem, AdminStatsLeaderboardNameMode,
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
use std::collections::{BTreeMap, BTreeSet};

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
                provider_names: None,
                created_from_unix_secs,
                created_until_unix_secs,
                group_by: UsageLeaderboardGroupBy::Model,
                user_id: filters.user_id,
                user_ids: None,
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
                provider_names: None,
                created_from_unix_secs,
                created_until_unix_secs,
                group_by: UsageLeaderboardGroupBy::ApiKey,
                user_id: filters.user_id,
                user_ids: None,
                provider_name: filters.provider_name,
                model: filters.model,
            })
            .await?;
        let api_key_ids: Vec<String> = summaries
            .iter()
            .map(|item| item.group_key.clone())
            .collect();
        let snapshots = if state.has_auth_api_key_data_reader() {
            // The bulk result is authoritative here. Historical aggregates can contain many
            // deleted key IDs, so retrying every missing ID as an individual lookup would turn a
            // single leaderboard request into an unbounded N+1 query pattern.
            Some(
                state
                    .list_auth_api_key_snapshots_by_ids(&api_key_ids)
                    .await?,
            )
        } else {
            None
        };
        let api_key_names = snapshots
            .as_deref()
            .unwrap_or_default()
            .iter()
            .filter_map(|snapshot| {
                snapshot
                    .api_key_name
                    .clone()
                    .map(|name| (snapshot.api_key_id.clone(), name))
            })
            .collect();
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
        == Some("leaderboard_user_groups")
        && request_context.method() == http::Method::GET
        && matches!(
            request_context.path(),
            "/api/admin/stats/leaderboard/user-groups"
                | "/api/admin/stats/leaderboard/user-groups/"
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
        let empty_counts = BTreeMap::new();
        if !state.has_usage_data_reader() || !state.has_user_data_reader() {
            return Ok(Some(build_admin_stats_user_group_leaderboard_response(
                metric,
                Some(&time_range),
                &[],
                &empty_counts,
                &empty_counts,
                offset,
                limit,
            )));
        }
        let include_inactive = query_param_bool(query, "include_inactive", false);
        let exclude_admin = query_param_bool(query, "exclude_admin", false);
        let filters = AdminStatsUsageFilter::from_query(query);
        if filters.user_id.is_some() {
            return Ok(Some(admin_stats_bad_request_response(
                "user_id is not supported for the user group leaderboard".to_string(),
            )));
        }
        let Some((created_from_unix_secs, created_until_unix_secs)) = time_range.to_unix_bounds()
        else {
            return Ok(Some(build_admin_stats_user_group_leaderboard_response(
                metric,
                Some(&time_range),
                &[],
                &empty_counts,
                &empty_counts,
                offset,
                limit,
            )));
        };

        let mut leaderboard = Vec::new();
        let mut member_counts = BTreeMap::new();
        let mut active_member_counts = BTreeMap::new();
        for group in state.list_user_groups().await? {
            let members = state.list_user_group_members(&group.id).await?;
            let member_count = members.iter().filter(|member| !member.is_deleted).count();
            let active_member_count = members
                .iter()
                .filter(|member| !member.is_deleted && member.is_active)
                .count();
            let user_ids = members
                .iter()
                .filter(|member| !member.is_deleted)
                .filter(|member| include_inactive || member.is_active)
                .filter(|member| !exclude_admin || !member.role.eq_ignore_ascii_case("admin"))
                .map(|member| member.user_id.clone())
                .collect::<Vec<_>>();
            let summaries = state
                .summarize_usage_leaderboard(&UsageLeaderboardQuery {
                    created_from_unix_secs,
                    created_until_unix_secs,
                    group_by: UsageLeaderboardGroupBy::User,
                    user_id: None,
                    user_ids: Some(user_ids),
                    provider_names: super::super::usage_group_provider_names(state, &group).await?,
                    provider_name: filters.provider_name.clone(),
                    model: filters.model.clone(),
                })
                .await?;
            let user_ids = summaries
                .iter()
                .map(|row| row.group_key.clone())
                .collect::<Vec<_>>();
            let metadata = load_user_leaderboard_metadata(state, &user_ids).await?;
            let users = build_user_leaderboard_items_from_summaries(
                &summaries,
                &metadata,
                state.has_auth_user_data_reader(),
                state.has_user_data_reader(),
                include_inactive,
                exclude_admin,
            );
            let mut item = AdminStatsLeaderboardItem {
                id: group.id.clone(),
                name: group.name,
                requests: 0,
                tokens: 0,
                cost: 0.0,
            };
            for user in users {
                item.requests = item.requests.saturating_add(user.requests);
                item.tokens = item.tokens.saturating_add(user.tokens);
                item.cost += user.cost;
            }
            member_counts.insert(group.id.clone(), member_count);
            active_member_counts.insert(group.id, active_member_count);
            leaderboard.push(item);
        }
        let ungrouped = super::super::ungrouped_usage_users(state).await?;
        let id = super::super::UNGROUPED_USAGE_ID.to_string();
        member_counts.insert(id.clone(), ungrouped.len());
        active_member_counts.insert(
            id.clone(),
            ungrouped.iter().filter(|user| user.is_active).count(),
        );
        let user_ids = ungrouped
            .into_iter()
            .filter(|user| include_inactive || user.is_active)
            .filter(|user| !exclude_admin || !user.role.eq_ignore_ascii_case("admin"))
            .map(|user| user.id)
            .collect();
        let rows = state
            .summarize_usage_leaderboard(&UsageLeaderboardQuery {
                created_from_unix_secs,
                created_until_unix_secs,
                group_by: UsageLeaderboardGroupBy::User,
                user_id: None,
                user_ids: Some(user_ids),
                provider_names: None,
                provider_name: filters.provider_name.clone(),
                model: filters.model.clone(),
            })
            .await?;
        leaderboard.push(AdminStatsLeaderboardItem {
            id,
            name: "Ungrouped".to_string(),
            requests: rows.iter().map(|row| row.request_count).sum(),
            tokens: rows.iter().map(|row| row.total_tokens).sum(),
            cost: rows.iter().map(|row| row.total_cost_usd).sum(),
        });
        leaderboard.sort_by(|left, right| compare_leaderboard_items(metric, order, left, right));

        return Ok(Some(build_admin_stats_user_group_leaderboard_response(
            metric,
            Some(&time_range),
            &leaderboard,
            &member_counts,
            &active_member_counts,
            offset,
            limit,
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
        let scoped_user_ids =
            match resolve_usage_user_group_scope(state, query, include_inactive, exclude_admin)
                .await?
            {
                Ok(value) => value,
                Err(detail) => return Ok(Some(admin_stats_bad_request_response(detail))),
            };
        let Some((created_from_unix_secs, created_until_unix_secs)) = time_range.to_unix_bounds()
        else {
            return Ok(Some(admin_stats_leaderboard_empty_response(
                metric,
                Some(&time_range),
            )));
        };
        let summaries = state
            .summarize_usage_leaderboard(&UsageLeaderboardQuery {
                provider_names: super::super::resolve_usage_group_provider_names(state, query)
                    .await?,
                created_from_unix_secs,
                created_until_unix_secs,
                group_by: UsageLeaderboardGroupBy::User,
                user_id: filters.user_id,
                user_ids: scoped_user_ids,
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
