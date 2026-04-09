use super::cache_types::AdminMonitoringCacheAffinityRecord;
use crate::handlers::admin::request::AdminAppState;
use crate::GatewayError;

fn parse_admin_monitoring_cache_affinity_key(raw_key: &str) -> Option<(String, String, String)> {
    let parts = raw_key.split(':').collect::<Vec<_>>();
    let start = parts
        .iter()
        .position(|segment| *segment == "cache_affinity")?;
    let affinity_key = parts.get(start + 1)?.trim();
    if affinity_key.is_empty() {
        return None;
    }
    let api_format = parts
        .get(start + 2)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .unwrap_or("unknown")
        .to_string();
    let model_name = parts
        .get(start + 3..)
        .filter(|segments| !segments.is_empty())
        .map(|segments| segments.join(":"))
        .unwrap_or_else(|| "unknown".to_string());
    Some((affinity_key.to_string(), api_format, model_name))
}

pub(super) fn admin_monitoring_scheduler_affinity_cache_key(
    record: &AdminMonitoringCacheAffinityRecord,
) -> Option<String> {
    let affinity_key = record.affinity_key.trim();
    let api_format = record.api_format.trim().to_ascii_lowercase();
    let model_name = record.model_name.trim();
    if affinity_key.is_empty() || api_format.is_empty() || model_name.is_empty() {
        return None;
    }
    Some(format!(
        "scheduler_affinity:{affinity_key}:{api_format}:{model_name}"
    ))
}

pub(super) fn admin_monitoring_cache_affinity_record(
    raw_key: &str,
    raw_value: &str,
) -> Option<AdminMonitoringCacheAffinityRecord> {
    let payload = serde_json::from_str::<serde_json::Value>(raw_value).ok()?;
    let object = payload.as_object()?;
    let (affinity_key, parsed_api_format, parsed_model_name) =
        parse_admin_monitoring_cache_affinity_key(raw_key)?;
    let api_format = object
        .get("api_format")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(parsed_api_format.as_str())
        .to_string();
    let model_name = object
        .get("model_name")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(parsed_model_name.as_str())
        .to_string();
    let request_count = object
        .get("request_count")
        .and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_i64().and_then(|number| u64::try_from(number).ok()))
        })
        .unwrap_or(0);
    Some(AdminMonitoringCacheAffinityRecord {
        raw_key: raw_key.to_string(),
        affinity_key,
        api_format,
        model_name,
        provider_id: object
            .get("provider_id")
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned),
        endpoint_id: object
            .get("endpoint_id")
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned),
        key_id: object
            .get("key_id")
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned),
        created_at: object.get("created_at").cloned(),
        expire_at: object.get("expire_at").cloned(),
        request_count,
    })
}

pub(super) fn clear_admin_monitoring_scheduler_affinity_entries(
    state: &AdminAppState<'_>,
    records: &[AdminMonitoringCacheAffinityRecord],
) {
    let scheduler_keys = records
        .iter()
        .filter_map(admin_monitoring_scheduler_affinity_cache_key)
        .collect::<std::collections::BTreeSet<_>>();
    for scheduler_key in scheduler_keys {
        let _ = state
            .as_ref()
            .remove_scheduler_affinity_cache_entry(&scheduler_key);
    }
}

#[cfg(test)]
pub(super) fn delete_admin_monitoring_cache_affinity_entries_for_tests(
    state: &AdminAppState<'_>,
    raw_keys: &[String],
) -> usize {
    state
        .as_ref()
        .remove_admin_monitoring_cache_affinity_entries_for_tests(raw_keys)
}

#[cfg(not(test))]
pub(super) fn delete_admin_monitoring_cache_affinity_entries_for_tests(
    _state: &AdminAppState<'_>,
    _raw_keys: &[String],
) -> usize {
    0
}

pub(super) async fn delete_admin_monitoring_cache_affinity_raw_keys(
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

    Ok(delete_admin_monitoring_cache_affinity_entries_for_tests(
        state, raw_keys,
    ))
}
