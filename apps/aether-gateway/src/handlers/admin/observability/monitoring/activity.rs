use super::route_filters::{
    admin_monitoring_escape_like_pattern, parse_admin_monitoring_days,
    parse_admin_monitoring_event_type_filter, parse_admin_monitoring_hours,
    parse_admin_monitoring_limit, parse_admin_monitoring_offset,
    parse_admin_monitoring_username_filter,
};
use super::usage_helpers::admin_monitoring_usage_is_error;
use crate::constants::INTERNAL_GATEWAY_PATH_PREFIXES;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::query::monitoring as monitoring_query;
use crate::GatewayError;
use aether_admin::observability::monitoring::{
    admin_monitoring_bad_request_response, admin_monitoring_user_behavior_user_id_from_path,
    build_admin_monitoring_audit_logs_payload_response,
    build_admin_monitoring_suspicious_activities_payload_response,
    build_admin_monitoring_system_status_payload_response,
    build_admin_monitoring_user_behavior_payload_response,
};
use aether_data_contracts::repository::usage::UsageAuditListQuery;
use axum::{body::Body, response::Response};

pub(super) async fn build_admin_monitoring_audit_logs_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let state = state.as_ref();
    let query = request_context.request_query_string.as_deref();
    let username = parse_admin_monitoring_username_filter(query);
    let event_type = parse_admin_monitoring_event_type_filter(query);
    let limit = match parse_admin_monitoring_limit(query) {
        Ok(value) => value,
        Err(detail) => return Ok(admin_monitoring_bad_request_response(detail)),
    };
    let offset = match parse_admin_monitoring_offset(query) {
        Ok(value) => value,
        Err(detail) => return Ok(admin_monitoring_bad_request_response(detail)),
    };
    let days = match parse_admin_monitoring_days(query) {
        Ok(value) => value,
        Err(detail) => return Ok(admin_monitoring_bad_request_response(detail)),
    };

    let Some(pool) = state.postgres_pool() else {
        return Ok(build_admin_monitoring_audit_logs_payload_response(
            Vec::new(),
            0,
            limit,
            offset,
            username,
            event_type,
            days,
        ));
    };

    let cutoff_time = chrono::Utc::now() - chrono::Duration::days(days);
    let username_pattern = username
        .as_deref()
        .map(admin_monitoring_escape_like_pattern)
        .map(|value| format!("%{value}%"));

    let (items, total) = monitoring_query::list_admin_audit_logs(
        &pool,
        cutoff_time,
        username_pattern.as_deref(),
        event_type.as_deref(),
        limit,
        offset,
    )
    .await?;

    Ok(build_admin_monitoring_audit_logs_payload_response(
        items, total, limit, offset, username, event_type, days,
    ))
}

pub(super) async fn build_admin_monitoring_suspicious_activities_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let state = state.as_ref();
    let query = request_context.request_query_string.as_deref();
    let hours = match parse_admin_monitoring_hours(query) {
        Ok(value) => value,
        Err(detail) => return Ok(admin_monitoring_bad_request_response(detail)),
    };

    let Some(pool) = state.postgres_pool() else {
        return Ok(
            build_admin_monitoring_suspicious_activities_payload_response(Vec::new(), hours),
        );
    };

    let cutoff_time = chrono::Utc::now() - chrono::Duration::hours(hours);
    let activities = monitoring_query::list_admin_suspicious_activities(&pool, cutoff_time).await?;

    Ok(build_admin_monitoring_suspicious_activities_payload_response(activities, hours))
}

pub(super) async fn build_admin_monitoring_user_behavior_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let state = state.as_ref();
    let Some(user_id) =
        admin_monitoring_user_behavior_user_id_from_path(&request_context.request_path)
    else {
        return Ok(admin_monitoring_bad_request_response("缺少 user_id"));
    };
    let days = match parse_admin_monitoring_days(request_context.request_query_string.as_deref()) {
        Ok(value) => value,
        Err(detail) => return Ok(admin_monitoring_bad_request_response(detail)),
    };

    let Some(pool) = state.postgres_pool() else {
        return Ok(build_admin_monitoring_user_behavior_payload_response(
            user_id,
            days,
            std::collections::BTreeMap::new(),
            0,
            0,
            0,
        ));
    };

    let cutoff_time = chrono::Utc::now() - chrono::Duration::days(days);

    let event_counts =
        monitoring_query::read_admin_user_behavior_event_counts(&pool, &user_id, cutoff_time)
            .await?;

    let failed_requests = event_counts
        .get("request_failed")
        .copied()
        .unwrap_or_default();
    let success_requests = event_counts
        .get("request_success")
        .copied()
        .unwrap_or_default();
    let suspicious_activities = event_counts
        .get("suspicious_activity")
        .copied()
        .unwrap_or_default()
        .saturating_add(
            event_counts
                .get("unauthorized_access")
                .copied()
                .unwrap_or_default(),
        );

    Ok(build_admin_monitoring_user_behavior_payload_response(
        user_id,
        days,
        event_counts,
        failed_requests,
        success_requests,
        suspicious_activities,
    ))
}

pub(super) async fn build_admin_monitoring_system_status_response(
    state: &AdminAppState<'_>,
) -> Result<Response<Body>, GatewayError> {
    let state = state.as_ref();
    let now = chrono::Utc::now();
    let today_start = now
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .expect("midnight should be valid")
        .and_utc();
    let recent_error_from = now - chrono::Duration::hours(1);
    let now_unix_secs = now.timestamp().max(0) as u64;

    let user_summary = state.summarize_export_users().await?;
    let total_users = user_summary.total;
    let active_users = user_summary.active;

    let providers = state
        .data
        .list_provider_catalog_providers(false)
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    let total_providers = providers.len();
    let active_providers = providers.iter().filter(|item| item.is_active).count();

    let user_api_key_summary = state
        .summarize_auth_api_key_export_non_standalone_records(now_unix_secs)
        .await?;
    let standalone_api_key_summary = state
        .summarize_auth_api_key_export_standalone_records(now_unix_secs)
        .await?;
    let total_api_keys = user_api_key_summary
        .total
        .saturating_add(standalone_api_key_summary.total);
    let active_api_keys = user_api_key_summary
        .active
        .saturating_add(standalone_api_key_summary.active);

    let today_usage = state
        .list_usage_audits(&UsageAuditListQuery {
            created_from_unix_secs: Some(today_start.timestamp().max(0) as u64),
            ..Default::default()
        })
        .await?;
    let today_requests = today_usage.len();
    let today_tokens = today_usage
        .iter()
        .map(|item| item.total_tokens)
        .sum::<u64>();
    let today_cost = today_usage
        .iter()
        .map(|item| item.total_cost_usd)
        .sum::<f64>();

    let recent_errors = state
        .list_usage_audits(&UsageAuditListQuery {
            created_from_unix_secs: Some(recent_error_from.timestamp().max(0) as u64),
            ..Default::default()
        })
        .await?
        .into_iter()
        .filter(admin_monitoring_usage_is_error)
        .count();
    let tunnel = state.tunnel.stats();

    Ok(build_admin_monitoring_system_status_payload_response(
        now,
        total_users,
        active_users,
        total_providers,
        active_providers,
        total_api_keys,
        active_api_keys,
        today_requests,
        today_tokens,
        today_cost,
        tunnel.proxy_connections,
        tunnel.nodes,
        tunnel.active_streams,
        INTERNAL_GATEWAY_PATH_PREFIXES,
        recent_errors,
    ))
}
