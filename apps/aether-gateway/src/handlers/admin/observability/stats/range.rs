use crate::handlers::admin::request::AdminAppState;
use crate::GatewayError;
pub(crate) use aether_admin::observability::stats::parse_bounded_u32;
pub(super) use aether_admin::observability::stats::{
    admin_usage_default_days, build_comparison_range, build_time_range_from_days, parse_naive_date,
    parse_nonnegative_usize, parse_tz_offset_minutes, resolve_preset_dates, user_today,
};
use aether_admin::observability::stats::{AdminStatsTimeRange, AdminStatsUsageFilter};
use aether_data_contracts::repository::usage::{StoredRequestUsageAudit, UsageAuditListQuery};

pub(crate) fn resolve_admin_usage_time_range(
    query: Option<&str>,
) -> Result<AdminStatsTimeRange, String> {
    match AdminStatsTimeRange::resolve_optional(query)? {
        Some(time_range) => Ok(time_range),
        None => {
            let tz_offset_minutes = parse_tz_offset_minutes(query)?;
            let default_days = u32::try_from(admin_usage_default_days())
                .ok()
                .filter(|value| *value > 0)
                .unwrap_or(1);
            build_time_range_from_days(default_days, tz_offset_minutes)
        }
    }
}

pub(crate) async fn list_usage_for_range(
    state: &AdminAppState<'_>,
    time_range: &AdminStatsTimeRange,
    filters: &AdminStatsUsageFilter,
) -> Result<Vec<StoredRequestUsageAudit>, GatewayError> {
    let Some((created_from_unix_secs, created_until_unix_secs)) = time_range.to_unix_bounds()
    else {
        return Ok(Vec::new());
    };

    state
        .list_usage_audits(&UsageAuditListQuery {
            created_from_unix_secs: Some(created_from_unix_secs),
            created_until_unix_secs: Some(created_until_unix_secs),
            user_id: filters.user_id.clone(),
            provider_name: filters.provider_name.clone(),
            model: filters.model.clone(),
            api_format: None,
            statuses: None,
            is_stream: None,
            error_only: false,
            limit: None,
            offset: None,
            newest_first: false,
        })
        .await
}

pub(crate) async fn list_usage_for_optional_range(
    state: &AdminAppState<'_>,
    time_range: Option<&AdminStatsTimeRange>,
    filters: &AdminStatsUsageFilter,
) -> Result<Vec<StoredRequestUsageAudit>, GatewayError> {
    match time_range {
        Some(time_range) => list_usage_for_range(state, time_range, filters).await,
        None => {
            let default_time_range = build_time_range_from_days(1, 0)
                .map_err(|detail| GatewayError::Internal(detail.to_string()))?;
            list_usage_for_range(state, &default_time_range, filters).await
        }
    }
}
