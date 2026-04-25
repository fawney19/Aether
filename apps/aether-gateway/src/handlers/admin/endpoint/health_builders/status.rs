use crate::handlers::admin::request::AdminAppState;
use crate::handlers::admin::shared::unix_secs_to_rfc3339;
use crate::handlers::public::{api_format_display_name, build_public_health_timeline};
use crate::handlers::shared::unix_ms_to_rfc3339;
use crate::provider_key_auth::provider_key_effective_api_formats;
use aether_data_contracts::repository::candidates::PublicHealthTimelineBucket;
use aether_scheduler_core::{is_provider_key_circuit_open, provider_key_health_score};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) async fn build_admin_endpoint_health_status_payload(
    state: &AdminAppState<'_>,
    lookback_hours: u64,
) -> Option<serde_json::Value> {
    if !state.has_provider_catalog_data_reader() || !state.has_request_candidate_data_reader() {
        return None;
    }

    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    let since_unix_secs = now_unix_secs.saturating_sub(lookback_hours * 3600);

    let providers = state
        .list_provider_catalog_providers(true)
        .await
        .ok()
        .unwrap_or_default();
    let provider_ids = providers
        .iter()
        .map(|provider| provider.id.clone())
        .collect::<Vec<_>>();
    let active_endpoints = if provider_ids.is_empty() {
        Vec::new()
    } else {
        state
            .list_provider_catalog_endpoints_by_provider_ids(&provider_ids)
            .await
            .ok()
            .unwrap_or_default()
            .into_iter()
            .filter(|endpoint| endpoint.is_active)
            .collect::<Vec<_>>()
    };

    let mut endpoint_ids_by_format = BTreeMap::<String, Vec<String>>::new();
    let mut endpoint_to_format = BTreeMap::<String, String>::new();
    let mut provider_ids_by_format = BTreeMap::<String, BTreeSet<String>>::new();
    let mut active_provider_formats = BTreeSet::<(String, String)>::new();
    let provider_type_by_id = providers
        .iter()
        .map(|provider| (provider.id.clone(), provider.provider_type.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut active_endpoints_by_provider = BTreeMap::<String, Vec<_>>::new();
    for endpoint in active_endpoints {
        endpoint_to_format.insert(endpoint.id.clone(), endpoint.api_format.clone());
        endpoint_ids_by_format
            .entry(endpoint.api_format.clone())
            .or_default()
            .push(endpoint.id.clone());
        provider_ids_by_format
            .entry(endpoint.api_format.clone())
            .or_default()
            .insert(endpoint.provider_id.clone());
        active_provider_formats.insert((endpoint.provider_id.clone(), endpoint.api_format.clone()));
        active_endpoints_by_provider
            .entry(endpoint.provider_id.clone())
            .or_default()
            .push(endpoint);
    }
    let all_endpoint_ids = endpoint_to_format.keys().cloned().collect::<Vec<_>>();

    let mut total_keys_by_format = BTreeMap::<String, BTreeSet<String>>::new();
    let mut active_keys_by_format = BTreeMap::<String, BTreeSet<String>>::new();
    let mut health_scores_by_format = BTreeMap::<String, Vec<f64>>::new();
    if !provider_ids.is_empty() {
        let keys = state
            .list_provider_catalog_key_summaries_by_provider_ids(&provider_ids)
            .await
            .ok()
            .unwrap_or_default();
        for key in keys {
            let provider_type = provider_type_by_id
                .get(&key.provider_id)
                .map(String::as_str)
                .unwrap_or("");
            let endpoints = active_endpoints_by_provider
                .get(&key.provider_id)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            for api_format in provider_key_effective_api_formats(&key, provider_type, endpoints) {
                if !active_provider_formats.contains(&(key.provider_id.clone(), api_format.clone()))
                {
                    continue;
                }
                total_keys_by_format
                    .entry(api_format.clone())
                    .or_default()
                    .insert(key.id.clone());
                if key.is_active && !is_provider_key_circuit_open(&key, &api_format) {
                    let key_health_score =
                        provider_key_health_score(&key, &api_format).unwrap_or(1.0);
                    active_keys_by_format
                        .entry(api_format.clone())
                        .or_default()
                        .insert(key.id.clone());
                    health_scores_by_format
                        .entry(api_format)
                        .or_default()
                        .push(key_health_score);
                }
            }
        }
    }

    let timeline_rows = state
        .aggregate_finalized_request_candidate_timeline_by_endpoint_ids_since(
            &all_endpoint_ids,
            since_unix_secs,
            now_unix_secs,
            100,
        )
        .await
        .ok()
        .unwrap_or_default();
    let mut timeline_by_format =
        BTreeMap::<String, BTreeMap<u32, PublicHealthTimelineBucket>>::new();
    for row in timeline_rows {
        let Some(api_format) = endpoint_to_format.get(&row.endpoint_id) else {
            continue;
        };
        let bucket = timeline_by_format
            .entry(api_format.clone())
            .or_default()
            .entry(row.segment_idx)
            .or_insert_with(|| PublicHealthTimelineBucket {
                endpoint_id: api_format.clone(),
                segment_idx: row.segment_idx,
                total_count: 0,
                success_count: 0,
                failed_count: 0,
                min_created_at_unix_ms: None,
                max_created_at_unix_ms: None,
            });
        bucket.total_count += row.total_count;
        bucket.success_count += row.success_count;
        bucket.failed_count += row.failed_count;
        bucket.min_created_at_unix_ms =
            match (bucket.min_created_at_unix_ms, row.min_created_at_unix_ms) {
                (Some(left), Some(right)) => Some(left.min(right)),
                (None, Some(right)) => Some(right),
                (left, None) => left,
            };
        bucket.max_created_at_unix_ms =
            match (bucket.max_created_at_unix_ms, row.max_created_at_unix_ms) {
                (Some(left), Some(right)) => Some(left.max(right)),
                (None, Some(right)) => Some(right),
                (left, None) => left,
            };
    }

    let mut payload = endpoint_ids_by_format
        .into_iter()
        .map(|(api_format, endpoint_ids)| {
            let empty_timeline = BTreeMap::new();
            let timeline_source = timeline_by_format.get(&api_format).unwrap_or(&empty_timeline);
            let (timeline, time_range_start, time_range_end) =
                build_public_health_timeline(timeline_source, 100);
            let healthy_count = timeline.iter().filter(|status| **status == "healthy").count();
            let warning_count = timeline.iter().filter(|status| **status == "warning").count();
            let unhealthy_count = timeline.iter().filter(|status| **status == "unhealthy").count();
            let known_count = healthy_count + warning_count + unhealthy_count;
            let total_keys = total_keys_by_format
                .get(&api_format)
                .map(BTreeSet::len)
                .unwrap_or(0);
            let health_score = if known_count > 0 {
                (healthy_count as f64 + warning_count as f64 * 0.8) / known_count as f64
            } else if let Some(scores) = health_scores_by_format.get(&api_format) {
                if scores.is_empty() {
                    if total_keys == 0 { 0.0 } else { 0.1 }
                } else {
                    scores.iter().copied().sum::<f64>() / scores.len() as f64
                }
            } else if total_keys == 0 {
                0.0
            } else {
                0.1
            };

            json!({
                "api_format": api_format.clone(),
                "display_name": api_format_display_name(&api_format),
                "health_score": health_score,
                "timeline": timeline,
                "time_range_start": time_range_start.and_then(unix_ms_to_rfc3339),
                "time_range_end": time_range_end.map(|ms| unix_ms_to_rfc3339(ms)).unwrap_or_else(|| unix_secs_to_rfc3339(now_unix_secs)),
                "total_endpoints": endpoint_ids.len(),
                "total_keys": total_keys,
                "active_keys": active_keys_by_format.get(&api_format).map(BTreeSet::len).unwrap_or(0),
                "provider_count": provider_ids_by_format.get(&api_format).map(BTreeSet::len).unwrap_or(0),
            })
        })
        .collect::<Vec<_>>();

    payload.sort_by(|left, right| {
        let left_score = left
            .get("health_score")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0);
        let right_score = right
            .get("health_score")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0);
        right_score
            .partial_cmp(&left_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    Some(serde_json::Value::Array(payload))
}

pub(crate) async fn build_admin_health_summary_payload(
    state: &AdminAppState<'_>,
) -> Option<serde_json::Value> {
    if !state.has_provider_catalog_data_reader() {
        return None;
    }

    let providers = state
        .list_provider_catalog_providers(false)
        .await
        .ok()
        .unwrap_or_default();
    let provider_ids = providers
        .iter()
        .map(|provider| provider.id.clone())
        .collect::<Vec<_>>();
    let endpoints = if provider_ids.is_empty() {
        Vec::new()
    } else {
        state
            .list_provider_catalog_endpoints_by_provider_ids(&provider_ids)
            .await
            .ok()
            .unwrap_or_default()
    };
    let keys = if provider_ids.is_empty() {
        Vec::new()
    } else {
        state
            .list_provider_catalog_key_summaries_by_provider_ids(&provider_ids)
            .await
            .ok()
            .unwrap_or_default()
    };

    let active_endpoints = endpoints
        .iter()
        .filter(|endpoint| endpoint.is_active)
        .count();
    let unhealthy_endpoints = endpoints
        .iter()
        .filter(|endpoint| endpoint.health_score < 0.5)
        .count();
    let active_keys = keys.iter().filter(|key| key.is_active).count();
    let unhealthy_keys = keys
        .iter()
        .filter(|key| {
            key.health_by_format
                .as_ref()
                .and_then(serde_json::Value::as_object)
                .map(|formats| {
                    formats.values().any(|health| {
                        health
                            .get("health_score")
                            .and_then(serde_json::Value::as_f64)
                            .unwrap_or(1.0)
                            < 0.5
                    })
                })
                .unwrap_or(false)
        })
        .count();
    let circuit_open_keys = keys
        .iter()
        .filter(|key| {
            key.circuit_breaker_by_format
                .as_ref()
                .and_then(serde_json::Value::as_object)
                .map(|formats| {
                    formats.values().any(|circuit| {
                        circuit
                            .get("open")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false)
                    })
                })
                .unwrap_or(false)
        })
        .count();

    Some(json!({
        "endpoints": {
            "total": endpoints.len(),
            "active": active_endpoints,
            "unhealthy": unhealthy_endpoints,
        },
        "keys": {
            "total": keys.len(),
            "active": active_keys,
            "unhealthy": unhealthy_keys,
            "circuit_open": circuit_open_keys,
        },
    }))
}
