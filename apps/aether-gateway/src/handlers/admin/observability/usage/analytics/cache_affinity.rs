use crate::handlers::admin::request::AdminAppState;
use crate::GatewayError;
use aether_data_contracts::repository::usage::{
    StoredUsageCacheAffinityIntervalRow, UsageCacheAffinityIntervalGroupBy,
    UsageCacheAffinityIntervalQuery,
};

pub(in super::super) async fn list_usage_cache_affinity_intervals(
    state: &AdminAppState<'_>,
    hours: u32,
    group_by: UsageCacheAffinityIntervalGroupBy,
    user_id: Option<&str>,
    api_key_id: Option<&str>,
) -> Result<Vec<StoredUsageCacheAffinityIntervalRow>, GatewayError> {
    let now_unix_secs = u64::try_from(chrono::Utc::now().timestamp()).unwrap_or_default();
    state
        .list_usage_cache_affinity_intervals(&UsageCacheAffinityIntervalQuery {
            created_from_unix_secs: now_unix_secs.saturating_sub(u64::from(hours) * 3600),
            created_until_unix_secs: now_unix_secs.saturating_add(1),
            group_by,
            user_id: user_id.map(ToOwned::to_owned),
            api_key_id: api_key_id.map(ToOwned::to_owned),
        })
        .await
}
