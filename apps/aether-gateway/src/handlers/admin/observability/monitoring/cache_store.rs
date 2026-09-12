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
    let keys = state
        .runtime_state()
        .scan_keys(pattern, 200)
        .await
        .map_err(|err| GatewayError::Internal(format!("runtime cache scan failed: {err}")))?;
    if !keys.is_empty() {
        return Ok(keys);
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

    let deleted = state
        .runtime_state()
        .kv_delete_many(raw_keys)
        .await
        .map_err(|err| GatewayError::Internal(format!("runtime cache delete failed: {err}")))?;
    if deleted > 0 {
        return Ok(deleted);
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
    let current_scheduler_affinity_epoch = state.as_ref().scheduler_affinity_epoch();

    {
        let patterns = affinity_keys
            .map(|keys| {
                keys.iter()
                    .flat_map(|affinity_key| {
                        [
                            format!("cache_affinity:{affinity_key}:*"),
                            format!("scheduler_affinity:{affinity_key}:*"),
                            format!("scheduler_affinity:v2:{affinity_key}:*"),
                        ]
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| {
                vec![
                    "cache_affinity:*".to_string(),
                    "scheduler_affinity:*".to_string(),
                ]
            });

        for pattern in patterns {
            let keys = state
                .runtime_state()
                .scan_keys(&pattern, 200)
                .await
                .map_err(|err| {
                    GatewayError::Internal(format!("runtime cache scan failed: {err}"))
                })?;
            if !keys.is_empty() {
                let raw_keys = keys
                    .iter()
                    .map(|key| state.runtime_state().strip_namespace(key).to_string())
                    .collect::<Vec<_>>();
                let values = state
                    .runtime_state()
                    .kv_get_many(&raw_keys)
                    .await
                    .map_err(|err| {
                        GatewayError::Internal(format!("runtime cache mget failed: {err}"))
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
                    if key.contains("scheduler_affinity:")
                        && record
                            .scheduler_affinity_epoch
                            .is_some_and(|epoch| epoch != current_scheduler_affinity_epoch)
                    {
                        continue;
                    }
                    if affinity_keys.is_some_and(|keys| !keys.contains(&record.affinity_key)) {
                        continue;
                    }
                    push_record(record);
                }
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
            entry.epoch,
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
    let ordering_config =
        crate::scheduler::config::read_system_default_routing_ordering_config(state.app())
            .await?
            .unwrap_or_default();
    let scheduling_mode = ordering_config.scheduling_mode_str().to_string();
    let provider_priority_mode = ordering_config.priority_mode_str().to_string();

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
    let storage_type = state.runtime_state().backend_kind().as_str();
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
