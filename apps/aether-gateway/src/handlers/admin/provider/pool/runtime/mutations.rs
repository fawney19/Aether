use super::keys::{pool_cooldown_index_key, pool_cooldown_key};
use crate::handlers::admin::request::AdminAppState;
use tracing::warn;

pub(crate) async fn clear_admin_provider_pool_cooldown(
    state: &AdminAppState<'_>,
    provider_id: &str,
    key_id: &str,
) {
    let Some(runner) = state.redis_kv_runner() else {
        return;
    };
    let Ok(mut connection) = runner.client().get_multiplexed_async_connection().await else {
        warn!("gateway admin provider pool: failed to connect redis to clear cooldown for key {key_id}");
        return;
    };
    let keyspace = runner.keyspace().clone();
    let _: Result<(), _> = redis::pipe()
        .cmd("DEL")
        .arg(pool_cooldown_key(&keyspace, provider_id, key_id))
        .ignore()
        .cmd("SREM")
        .arg(pool_cooldown_index_key(&keyspace, provider_id))
        .arg(key_id)
        .ignore()
        .query_async(&mut connection)
        .await;
}

pub(crate) async fn reset_admin_provider_pool_cost(
    state: &AdminAppState<'_>,
    provider_id: &str,
    key_id: &str,
) {
    let Some(runner) = state.redis_kv_runner() else {
        return;
    };
    let _ = runner.del(&format!("ap:{provider_id}:cost:{key_id}")).await;
}
