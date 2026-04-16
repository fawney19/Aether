use crate::handlers::admin::request::AdminAppState;
use crate::GatewayError;
use aether_data_contracts::repository::usage::StoredRequestUsageAudit;

pub(super) use aether_admin::observability::stats::{
    build_admin_stats_leaderboard_response, build_api_key_leaderboard_items,
    build_api_key_leaderboard_items_from_summaries, build_model_leaderboard_items,
    build_model_leaderboard_items_from_summaries, build_user_leaderboard_items,
    build_user_leaderboard_items_from_summaries, compare_leaderboard_items, compute_dense_rank,
    AdminStatsLeaderboardItem, AdminStatsLeaderboardMetric, AdminStatsLeaderboardNameMode,
    AdminStatsSortOrder, AdminStatsUserMetadata,
};

pub(super) async fn load_user_leaderboard_metadata(
    state: &AdminAppState<'_>,
    user_ids: &[String],
) -> Result<std::collections::BTreeMap<String, AdminStatsUserMetadata>, GatewayError> {
    let mut metadata = std::collections::BTreeMap::new();

    if state.has_user_data_reader() {
        for user in state.list_users_by_ids(user_ids).await? {
            let name = if !user.username.trim().is_empty() {
                user.username
            } else {
                user.email.unwrap_or(user.id.clone())
            };
            metadata.insert(
                user.id,
                AdminStatsUserMetadata {
                    name,
                    role: user.role,
                    is_active: user.is_active,
                    is_deleted: user.is_deleted,
                },
            );
        }
    }

    for user_id in user_ids {
        if metadata.contains_key(user_id) {
            continue;
        }
        let Some(user) = state.find_user_auth_by_id(user_id).await? else {
            continue;
        };
        let name = if !user.username.trim().is_empty() {
            user.username.clone()
        } else {
            user.email.clone().unwrap_or_else(|| user.id.clone())
        };
        metadata.insert(
            user.id.clone(),
            AdminStatsUserMetadata {
                name,
                role: user.role.clone(),
                is_active: user.is_active,
                is_deleted: user.is_deleted,
            },
        );
    }

    Ok(metadata)
}
