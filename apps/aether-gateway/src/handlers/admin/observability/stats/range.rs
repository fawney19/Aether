pub(crate) use aether_admin::observability::stats::parse_bounded_u32;
use aether_admin::observability::stats::AdminStatsTimeRange;
pub(super) use aether_admin::observability::stats::{
    admin_usage_default_days, build_comparison_range, build_time_range_from_days, parse_naive_date,
    parse_nonnegative_usize, parse_tz_offset_minutes, resolve_preset_dates, user_today,
};

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
