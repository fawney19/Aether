use crate::handlers::admin::request::AdminAppState;
use crate::GatewayError;
use aether_data_contracts::repository::usage::{StoredRequestUsageAudit, UsageAuditListQuery};

pub(in super::super) async fn list_recent_completed_usage_for_cache_affinity(
    state: &AdminAppState<'_>,
    hours: u32,
    user_id: Option<&str>,
) -> Result<Vec<StoredRequestUsageAudit>, GatewayError> {
    let now_unix_secs = u64::try_from(chrono::Utc::now().timestamp()).unwrap_or_default();
    let created_from_unix_secs = now_unix_secs.saturating_sub(u64::from(hours) * 3600);
    let mut items = state
        .list_usage_audits(&UsageAuditListQuery {
            created_from_unix_secs: Some(created_from_unix_secs),
            created_until_unix_secs: None,
            user_id: user_id.map(ToOwned::to_owned),
            provider_name: None,
            model: None,
            api_format: None,
            statuses: None,
            is_stream: None,
            error_only: false,
            limit: None,
            offset: None,
            newest_first: false,
        })
        .await?;
    items.retain(|item| item.status == "completed");
    items.sort_by(|left, right| {
        left.created_at_unix_ms
            .cmp(&right.created_at_unix_ms)
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok(items)
}
