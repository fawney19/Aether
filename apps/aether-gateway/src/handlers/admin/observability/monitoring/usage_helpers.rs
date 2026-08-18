use aether_admin::observability::usage::admin_usage_is_failed;
use aether_data_contracts::repository::usage::StoredRequestUsageAudit;

pub(super) fn admin_monitoring_usage_is_error(item: &StoredRequestUsageAudit) -> bool {
    // A user-originated HTTP 400 is a failed lifecycle event, but it is not a
    // service/operational error and must stay out of resilience error counts.
    if item.status_code == Some(400) || item.outcome_class().is_user_error() {
        return false;
    }
    item.status.trim().eq_ignore_ascii_case("error")
        || admin_usage_is_failed(item)
        || item.error_category.is_some()
}
