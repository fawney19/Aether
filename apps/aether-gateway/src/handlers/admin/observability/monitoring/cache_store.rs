use super::cache_affinity::{
    admin_monitoring_cache_affinity_record, admin_monitoring_cache_affinity_record_identity,
    admin_monitoring_scheduler_affinity_record,
    admin_monitoring_scheduler_affinity_record_from_raw,
};
use super::cache_types::{AdminMonitoringCacheAffinityRecord, AdminMonitoringCacheSnapshot};
use crate::handlers::admin::observability::stats::round_to;
use crate::handlers::admin::request::AdminAppState;
use crate::scheduler::affinity::SCHEDULER_AFFINITY_TTL;
use crate::GatewayError;
use aether_data_contracts::repository::usage::UsageCacheHitSummaryQuery;

async fn count_admin_monitoring_cache_affinity_entries(state: &AdminAppState<'_>) -> usize {
    list_admin_monitoring_cache_affinity_records(state)
        .await
        .map(|items| items.len())
        .unwrap_or_else(|_| {
            state
                .as_ref()
                .list_scheduler_affinity_entries(SCHEDULER_AFFINITY_TTL)
                .len()
        })
}

async fn scan_admin_monitoring_namespaced_keys(
    runner: &aether_data::redis::RedisKvRunner,
    pattern: &str,
) -> Result<Vec<String>, GatewayError> {
    let mut connection = runner
        .client()
        .get_multiplexed_async_connection()
        .await
        .map_err(|err| {
            GatewayError::Internal(format!("admin monitoring redis connect failed: {err}"))
        })?;
    let namespaced_pattern = runner.keyspace().key(pattern);
    let mut cursor = 0u64;
    let mut keys = Vec::new();
    loop {
        let (next_cursor, batch) = redis::cmd("SCAN")
            .arg(cursor)
            .arg("MATCH")
            .arg(&namespaced_pattern)
            .arg("COUNT")
            .arg(200)
            .query_async::<(u64, Vec<String>)>(&mut connection)
            .await
            .map_err(|err| {
                GatewayError::Internal(format!("admin monitoring redis scan failed: {err}"))
            })?;
        keys.extend(batch);
        if next_cursor == 0 {
            break;
        }
        cursor = next_cursor;
    }
    Ok(keys)
}

#[cfg(test)]
pub(super) fn load_admin_monitoring_cache_affinity_entries_for_tests(
    state: &AdminAppState<'_>,
) -> Vec<(String, String)> {
    state
        .as_ref()
        .list_admin_monitoring_cache_affinity_entries_for_tests()
}

#[cfg(not(test))]
pub(super) fn load_admin_monitoring_cache_affinity_entries_for_tests(
    _state: &AdminAppState<'_>,
) -> Vec<(String, String)> {
    Vec::new()
}

#[cfg(test)]
fn load_admin_monitoring_redis_keys_for_tests(state: &AdminAppState<'_>) -> Vec<String> {
    state.as_ref().list_admin_monitoring_redis_keys_for_tests()
}

#[cfg(not(test))]
fn load_admin_monitoring_redis_keys_for_tests(_state: &AdminAppState<'_>) -> Vec<String> {
    Vec::new()
}

#[cfg(test)]
fn delete_admin_monitoring_redis_keys_for_tests(
    state: &AdminAppState<'_>,
    raw_keys: &[String],
) -> usize {
    state
        .as_ref()
        .remove_admin_monitoring_redis_keys_for_tests(raw_keys)
}

#[cfg(not(test))]
fn delete_admin_monitoring_redis_keys_for_tests(
    _state: &AdminAppState<'_>,
    _raw_keys: &[String],
) -> usize {
    0
}

fn admin_monitoring_test_key_matches_pattern(key: &str, pattern: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => key.starts_with(prefix),
        None => key == pattern,
    }
}

pub(super) fn admin_monitoring_has_test_redis_keys(state: &AdminAppState<'_>) -> bool {
    !load_admin_monitoring_redis_keys_for_tests(state).is_empty()
}

pub(super) async fn list_admin_monitoring_namespaced_keys(
    state: &AdminAppState<'_>,
    pattern: &str,
) -> Result<Vec<String>, GatewayError> {
    if let Some(runner) = state.redis_kv_runner() {
        return scan_admin_monitoring_namespaced_keys(&runner, pattern).await;
    }

    let mut keys = load_admin_monitoring_redis_keys_for_tests(state)
        .into_iter()
        .filter(|key| admin_monitoring_test_key_matches_pattern(key, pattern))
        .collect::<Vec<_>>();
    keys.sort();
    Ok(keys)
}

pub(super) async fn delete_admin_monitoring_namespaced_keys(
    state: &AdminAppState<'_>,
    raw_keys: &[String],
) -> Result<usize, GatewayError> {
    if raw_keys.is_empty() {
        return Ok(0);
    }

    if let Some(runner) = state.redis_kv_runner() {
        let mut connection = runner
            .client()
            .get_multiplexed_async_connection()
            .await
            .map_err(|err| {
                GatewayError::Internal(format!("admin monitoring redis connect failed: {err}"))
            })?;
        let deleted = redis::cmd("DEL")
            .arg(raw_keys)
            .query_async::<i64>(&mut connection)
            .await
            .map_err(|err| {
                GatewayError::Internal(format!("admin monitoring redis delete failed: {err}"))
            })?;
        return Ok(usize::try_from(deleted).unwrap_or(0));
    }

    Ok(delete_admin_monitoring_redis_keys_for_tests(
        state, raw_keys,
    ))
}

pub(super) async fn list_admin_monitoring_cache_affinity_records(
    state: &AdminAppState<'_>,
) -> Result<Vec<AdminMonitoringCacheAffinityRecord>, GatewayError> {
    list_admin_monitoring_cache_affinity_records_matching(state, None).await
}

pub(super) async fn list_admin_monitoring_cache_affinity_records_by_affinity_keys(
    state: &AdminAppState<'_>,
    affinity_keys: &std::collections::BTreeSet<String>,
) -> Result<Vec<AdminMonitoringCacheAffinityRecord>, GatewayError> {
    if affinity_keys.is_empty() {
        return Ok(Vec::new());
    }
    list_admin_monitoring_cache_affinity_records_matching(state, Some(affinity_keys)).await
}

pub(super) fn admin_monitoring_has_runtime_scheduler_affinity_entries(
    state: &AdminAppState<'_>,
) -> bool {
    !state
        .as_ref()
        .list_scheduler_affinity_entries(SCHEDULER_AFFINITY_TTL)
        .is_empty()
}

async fn list_admin_monitoring_cache_affinity_records_matching(
    state: &AdminAppState<'_>,
    affinity_keys: Option<&std::collections::BTreeSet<String>>,
) -> Result<Vec<AdminMonitoringCacheAffinityRecord>, GatewayError> {
    let mut records = Vec::new();
    let mut seen_record_ids = std::collections::BTreeSet::new();

    let mut push_record = |record: AdminMonitoringCacheAffinityRecord| {
        if seen_record_ids.insert(admin_monitoring_cache_affinity_record_identity(&record)) {
            records.push(record);
        }
    };

    if let Some(runner) = state.redis_kv_runner() {
        let mut connection = runner
            .client()
            .get_multiplexed_async_connection()
            .await
            .map_err(|err| {
                GatewayError::Internal(format!("admin monitoring redis connect failed: {err}"))
            })?;
        let patterns = affinity_keys
            .map(|keys| {
                keys.iter()
                    .flat_map(|affinity_key| {
                        [
                            runner
                                .keyspace()
                                .key(&format!("cache_affinity:{affinity_key}:*")),
                            runner
                                .keyspace()
                                .key(&format!("scheduler_affinity:{affinity_key}:*")),
                        ]
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| {
                vec![
                    runner.keyspace().key("cache_affinity:*"),
                    runner.keyspace().key("scheduler_affinity:*"),
                ]
            });

        for pattern in patterns {
            let mut cursor = 0u64;
            loop {
                let (next_cursor, keys) = redis::cmd("SCAN")
                    .arg(cursor)
                    .arg("MATCH")
                    .arg(&pattern)
                    .arg("COUNT")
                    .arg(200)
                    .query_async::<(u64, Vec<String>)>(&mut connection)
                    .await
                    .map_err(|err| {
                        GatewayError::Internal(format!("admin monitoring redis scan failed: {err}"))
                    })?;
                if !keys.is_empty() {
                    let values = redis::cmd("MGET")
                        .arg(&keys)
                        .query_async::<Vec<Option<String>>>(&mut connection)
                        .await
                        .map_err(|err| {
                            GatewayError::Internal(format!(
                                "admin monitoring redis mget failed: {err}"
                            ))
                        })?;
                    for (key, raw_value) in keys.into_iter().zip(values) {
                        let Some(raw_value) = raw_value else {
                            continue;
                        };
                        let record = if key.contains("scheduler_affinity:") {
                            admin_monitoring_scheduler_affinity_record_from_raw(&key, &raw_value)
                        } else {
                            admin_monitoring_cache_affinity_record(&key, &raw_value)
                        };
                        let Some(record) = record else {
                            continue;
                        };
                        if affinity_keys.is_some_and(|keys| !keys.contains(&record.affinity_key)) {
                            continue;
                        }
                        push_record(record);
                    }
                }
                if next_cursor == 0 {
                    break;
                }
                cursor = next_cursor;
            }
        }
    }

    for (key, raw_value) in load_admin_monitoring_cache_affinity_entries_for_tests(state) {
        let Some(record) = admin_monitoring_cache_affinity_record(&key, &raw_value) else {
            continue;
        };
        if affinity_keys.is_some_and(|keys| !keys.contains(&record.affinity_key)) {
            continue;
        }
        push_record(record);
    }

    let now_unix_secs = chrono::Utc::now().timestamp().max(0) as u64;
    for entry in state
        .as_ref()
        .list_scheduler_affinity_entries(SCHEDULER_AFFINITY_TTL)
    {
        let Some(record) = admin_monitoring_scheduler_affinity_record(
            &entry.cache_key,
            &entry.target,
            entry.age,
            SCHEDULER_AFFINITY_TTL,
            now_unix_secs,
        ) else {
            continue;
        };
        if affinity_keys.is_some_and(|keys| !keys.contains(&record.affinity_key)) {
            continue;
        }
        push_record(record);
    }

    Ok(records)
}

pub(super) async fn build_admin_monitoring_cache_snapshot(
    state: &AdminAppState<'_>,
) -> Result<AdminMonitoringCacheSnapshot, GatewayError> {
    let scheduling_mode = state
        .read_system_config_json_value("scheduling_mode")
        .await?
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .unwrap_or_else(|| "cache_affinity".to_string());
    let provider_priority_mode = state
        .read_system_config_json_value("provider_priority_mode")
        .await?
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .unwrap_or_else(|| "provider".to_string());

    let now = chrono::Utc::now();
    let usage_summary = if state.has_usage_data_reader() {
        state
            .summarize_usage_cache_hit_summary(&UsageCacheHitSummaryQuery {
                created_from_unix_secs: (now - chrono::Duration::hours(24)).timestamp().max(0)
                    as u64,
                created_until_unix_secs: now.timestamp().max(0) as u64,
                user_id: None,
            })
            .await?
    } else {
        Default::default()
    };
    let cache_hits = usage_summary.cache_hit_requests as usize;
    let cache_misses = usage_summary
        .total_requests
        .saturating_sub(usage_summary.cache_hit_requests) as usize;
    let cache_hit_rate = if usage_summary.total_requests == 0 {
        0.0
    } else {
        round_to(cache_hits as f64 / usage_summary.total_requests as f64, 4)
    };
    let total_affinities = count_admin_monitoring_cache_affinity_entries(state).await;
    let storage_type = if state.redis_kv_runner().is_some() {
        "redis"
    } else {
        "memory"
    };
    let scheduler_name = if scheduling_mode == "cache_affinity" {
        "cache_aware".to_string()
    } else {
        "random".to_string()
    };

    Ok(AdminMonitoringCacheSnapshot {
        scheduler_name,
        scheduling_mode,
        provider_priority_mode,
        storage_type,
        total_affinities,
        cache_hits,
        cache_misses,
        cache_hit_rate,
        provider_switches: 0,
        key_switches: 0,
        cache_invalidations: 0,
    })
}
