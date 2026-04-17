use super::keys::{
    parse_pool_cost_member, pool_cooldown_index_key, pool_cooldown_keys, pool_cost_keys,
    pool_lru_key, pool_sticky_pattern,
};
use crate::handlers::admin::provider::shared::support::{
    AdminProviderPoolConfig, AdminProviderPoolRuntimeState, ADMIN_PROVIDER_POOL_SCAN_BATCH,
};
use crate::GatewayError;
use aether_data::redis::RedisKvRunner;
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::warn;

async fn scan_redis_keys(
    connection: &mut redis::aio::MultiplexedConnection,
    pattern: &str,
) -> Result<Vec<String>, GatewayError> {
    let mut cursor = 0u64;
    let mut keys = Vec::new();
    loop {
        let (next_cursor, batch): (u64, Vec<String>) = redis::cmd("SCAN")
            .arg(cursor)
            .arg("MATCH")
            .arg(pattern)
            .arg("COUNT")
            .arg(ADMIN_PROVIDER_POOL_SCAN_BATCH)
            .query_async(connection)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        keys.extend(batch);
        if next_cursor == 0 {
            break;
        }
        cursor = next_cursor;
    }
    Ok(keys)
}

pub(crate) async fn read_admin_provider_pool_cooldown_counts(
    runner: &RedisKvRunner,
    provider_ids: &[String],
) -> BTreeMap<String, usize> {
    if provider_ids.is_empty() {
        return BTreeMap::new();
    }

    let Ok(mut connection) = runner.client().get_multiplexed_async_connection().await else {
        warn!("gateway admin provider pool: failed to connect redis for cooldown counts");
        return BTreeMap::new();
    };
    let keyspace = runner.keyspace().clone();
    let mut pipeline = redis::pipe();
    for provider_id in provider_ids {
        pipeline
            .cmd("SCARD")
            .arg(pool_cooldown_index_key(&keyspace, provider_id));
    }

    match pipeline.query_async::<Vec<u64>>(&mut connection).await {
        Ok(counts) => provider_ids
            .iter()
            .cloned()
            .zip(counts)
            .map(|(provider_id, count)| (provider_id, count as usize))
            .collect(),
        Err(err) => {
            warn!(
                "gateway admin provider pool: failed to batch read cooldown counts: {:?}",
                err
            );
            BTreeMap::new()
        }
    }
}

pub(crate) async fn read_admin_provider_pool_runtime_state(
    runner: &RedisKvRunner,
    provider_id: &str,
    key_ids: &[String],
    pool_config: AdminProviderPoolConfig,
) -> AdminProviderPoolRuntimeState {
    let mut runtime = AdminProviderPoolRuntimeState::default();
    let Ok(mut connection) = runner.client().get_multiplexed_async_connection().await else {
        warn!("gateway admin provider pool: failed to connect redis for provider {provider_id}");
        return runtime;
    };
    let keyspace = runner.keyspace().clone();
    let cooldown_keys = pool_cooldown_keys(&keyspace, provider_id, key_ids);
    let cost_keys = pool_cost_keys(&keyspace, provider_id, key_ids);

    let sticky_keys = match scan_redis_keys(
        &mut connection,
        &pool_sticky_pattern(&keyspace, provider_id),
    )
    .await
    {
        Ok(keys) => keys,
        Err(err) => {
            warn!(
                "gateway admin provider pool: failed to scan sticky keys for provider {provider_id}: {:?}",
                err
            );
            Vec::new()
        }
    };
    runtime.total_sticky_sessions = sticky_keys.len();
    if !sticky_keys.is_empty() {
        for chunk in sticky_keys.chunks(ADMIN_PROVIDER_POOL_SCAN_BATCH as usize) {
            let values = redis::cmd("MGET")
                .arg(chunk)
                .query_async::<Vec<Option<String>>>(&mut connection)
                .await;
            let Ok(values) = values else {
                warn!(
                    "gateway admin provider pool: failed to read sticky bindings for provider {provider_id}"
                );
                break;
            };
            for bound_key_id in values.into_iter().flatten() {
                *runtime
                    .sticky_sessions_by_key
                    .entry(bound_key_id)
                    .or_insert(0) += 1;
            }
        }
    }

    if !cooldown_keys.is_empty() {
        let cooldown_reasons = redis::cmd("MGET")
            .arg(&cooldown_keys)
            .query_async::<Vec<Option<String>>>(&mut connection)
            .await
            .unwrap_or_else(|err| {
                warn!(
                    "gateway admin provider pool: failed to batch read cooldown reasons for provider {provider_id}: {:?}",
                    err
                );
                vec![None; cooldown_keys.len()]
            });
        let mut ttl_pipeline = redis::pipe();
        for cooldown_key in &cooldown_keys {
            ttl_pipeline.cmd("TTL").arg(cooldown_key);
        }
        let cooldown_ttls = ttl_pipeline
            .query_async::<Vec<i64>>(&mut connection)
            .await
            .unwrap_or_else(|err| {
                warn!(
                    "gateway admin provider pool: failed to batch read cooldown ttl for provider {provider_id}: {:?}",
                    err
                );
                vec![-1; cooldown_keys.len()]
            });

        for (((key_id, _cooldown_key), reason), ttl) in key_ids
            .iter()
            .zip(cooldown_keys.iter())
            .zip(cooldown_reasons)
            .zip(cooldown_ttls)
        {
            if let Some(reason) = reason {
                runtime
                    .cooldown_reason_by_key
                    .insert(key_id.clone(), reason);
                if let Ok(ttl_seconds) = u64::try_from(ttl) {
                    if ttl_seconds > 0 {
                        runtime
                            .cooldown_ttl_by_key
                            .insert(key_id.clone(), ttl_seconds);
                    }
                }
            }
        }
    }

    if !cost_keys.is_empty() {
        let window_start = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .saturating_sub(pool_config.cost_window_seconds);
        let mut cost_pipeline = redis::pipe();
        for cost_key in &cost_keys {
            cost_pipeline
                .cmd("ZRANGEBYSCORE")
                .arg(cost_key)
                .arg(window_start)
                .arg("+inf");
        }
        let members_by_key = cost_pipeline
            .query_async::<Vec<Vec<String>>>(&mut connection)
            .await
            .unwrap_or_else(|err| {
                warn!(
                    "gateway admin provider pool: failed to batch read cost windows for provider {provider_id}: {:?}",
                    err
                );
                vec![Vec::new(); cost_keys.len()]
            });
        for (key_id, members) in key_ids.iter().zip(members_by_key) {
            let total = members
                .iter()
                .map(|member| parse_pool_cost_member(member))
                .sum::<u64>();
            runtime
                .cost_window_usage_by_key
                .insert(key_id.clone(), total);
        }
    }

    if pool_config.lru_enabled && !key_ids.is_empty() {
        let mut command = redis::cmd("ZMSCORE");
        command.arg(pool_lru_key(&keyspace, provider_id));
        for key_id in key_ids {
            command.arg(key_id);
        }
        if let Ok(scores) = command
            .query_async::<Vec<Option<f64>>>(&mut connection)
            .await
        {
            for (key_id, score) in key_ids.iter().zip(scores) {
                if let Some(score) = score {
                    runtime.lru_score_by_key.insert(key_id.clone(), score);
                }
            }
        }
    }

    runtime
}

pub(crate) async fn read_admin_provider_pool_cooldown_count(
    runner: &RedisKvRunner,
    provider_id: &str,
) -> usize {
    let Ok(mut connection) = runner.client().get_multiplexed_async_connection().await else {
        warn!("gateway admin provider pool: failed to connect redis for provider {provider_id}");
        return 0;
    };
    let keyspace = runner.keyspace().clone();
    redis::cmd("SCARD")
        .arg(pool_cooldown_index_key(&keyspace, provider_id))
        .query_async::<u64>(&mut connection)
        .await
        .map(|value| value as usize)
        .unwrap_or(0)
}

pub(crate) async fn read_admin_provider_pool_cooldown_key_ids(
    runner: &RedisKvRunner,
    provider_id: &str,
) -> Vec<String> {
    let Ok(mut connection) = runner.client().get_multiplexed_async_connection().await else {
        warn!("gateway admin provider pool: failed to connect redis for provider {provider_id}");
        return Vec::new();
    };
    let keyspace = runner.keyspace().clone();
    redis::cmd("SMEMBERS")
        .arg(pool_cooldown_index_key(&keyspace, provider_id))
        .query_async::<Vec<String>>(&mut connection)
        .await
        .unwrap_or_default()
}
