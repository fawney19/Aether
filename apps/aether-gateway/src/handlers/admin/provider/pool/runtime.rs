use super::config::admin_provider_pool_config;
use crate::handlers::admin::provider::shared::support::{
    AdminProviderPoolConfig, AdminProviderPoolRuntimeState, ADMIN_PROVIDER_POOL_SCAN_BATCH,
};
use crate::{AppState, GatewayError};
use aether_data::redis::{RedisKeyspace, RedisKvRunner};
use serde_json::json;
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::warn;

fn pool_sticky_pattern(keyspace: &RedisKeyspace, provider_id: &str) -> String {
    keyspace.key(&format!("ap:{provider_id}:sticky:*"))
}

fn pool_lru_key(keyspace: &RedisKeyspace, provider_id: &str) -> String {
    keyspace.key(&format!("ap:{provider_id}:lru"))
}

fn pool_cooldown_key(keyspace: &RedisKeyspace, provider_id: &str, key_id: &str) -> String {
    keyspace.key(&format!("ap:{provider_id}:cooldown:{key_id}"))
}

fn pool_cooldown_index_key(keyspace: &RedisKeyspace, provider_id: &str) -> String {
    keyspace.key(&format!("ap:{provider_id}:cooldown_idx"))
}

fn pool_cost_key(keyspace: &RedisKeyspace, provider_id: &str, key_id: &str) -> String {
    keyspace.key(&format!("ap:{provider_id}:cost:{key_id}"))
}

fn parse_pool_cost_member(member: &str) -> u64 {
    member
        .rsplit_once(':')
        .and_then(|(_, suffix)| suffix.parse::<u64>().ok())
        .unwrap_or(0)
}

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

fn pool_cooldown_keys(
    keyspace: &RedisKeyspace,
    provider_id: &str,
    key_ids: &[String],
) -> Vec<String> {
    key_ids
        .iter()
        .map(|key_id| pool_cooldown_key(keyspace, provider_id, key_id))
        .collect()
}

fn pool_cost_keys(keyspace: &RedisKeyspace, provider_id: &str, key_ids: &[String]) -> Vec<String> {
    key_ids
        .iter()
        .map(|key_id| pool_cost_key(keyspace, provider_id, key_id))
        .collect()
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
            .zip(counts.into_iter())
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
            .zip(cooldown_reasons.into_iter())
            .zip(cooldown_ttls.into_iter())
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
        for (key_id, members) in key_ids.iter().zip(members_by_key.into_iter()) {
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
            for (key_id, score) in key_ids.iter().zip(scores.into_iter()) {
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

pub(crate) async fn build_admin_provider_pool_status_payload(
    state: &AppState,
    provider_id: &str,
) -> Option<serde_json::Value> {
    if !state.has_provider_catalog_data_reader() {
        return None;
    }

    let provider = state
        .read_provider_catalog_providers_by_ids(&[provider_id.to_string()])
        .await
        .ok()
        .and_then(|mut providers| providers.drain(..).next())?;
    let Some(pool_config) = admin_provider_pool_config(&provider) else {
        return Some(json!({
            "provider_id": provider.id,
            "provider_name": provider.name,
            "pool_enabled": false,
            "total_keys": 0,
            "total_sticky_sessions": 0,
            "keys": [],
        }));
    };

    let keys = state
        .list_provider_catalog_keys_by_provider_ids(std::slice::from_ref(&provider.id))
        .await
        .ok()
        .unwrap_or_default();
    let key_ids = keys.iter().map(|key| key.id.clone()).collect::<Vec<_>>();
    let runtime = match state.redis_kv_runner() {
        Some(runner) => {
            read_admin_provider_pool_runtime_state(&runner, &provider.id, &key_ids, pool_config)
                .await
        }
        None => AdminProviderPoolRuntimeState::default(),
    };
    let key_payloads = keys
        .into_iter()
        .map(|key| {
            let cooldown_reason = runtime.cooldown_reason_by_key.get(&key.id).cloned();
            json!({
                "key_id": key.id,
                "key_name": key.name,
                "is_active": key.is_active,
                "cooldown_reason": cooldown_reason,
                "cooldown_ttl_seconds": cooldown_reason
                    .as_ref()
                    .and_then(|_| runtime.cooldown_ttl_by_key.get(&key.id).copied()),
                "cost_window_usage": runtime.cost_window_usage_by_key.get(&key.id).copied().unwrap_or(0),
                "cost_limit": pool_config.cost_limit_per_key_tokens,
                "sticky_sessions": runtime.sticky_sessions_by_key.get(&key.id).copied().unwrap_or(0),
                "lru_score": runtime.lru_score_by_key.get(&key.id).copied(),
            })
        })
        .collect::<Vec<_>>();

    Some(json!({
        "provider_id": provider.id,
        "provider_name": provider.name,
        "pool_enabled": true,
        "total_keys": key_payloads.len(),
        "total_sticky_sessions": runtime.total_sticky_sessions,
        "keys": key_payloads,
    }))
}

pub(crate) async fn clear_admin_provider_pool_cooldown(
    state: &AppState,
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
    state: &AppState,
    provider_id: &str,
    key_id: &str,
) {
    let Some(runner) = state.redis_kv_runner() else {
        return;
    };
    let _ = runner.del(&format!("ap:{provider_id}:cost:{key_id}")).await;
}
