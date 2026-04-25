use crate::api::ai::public_api_format_local_path;
use crate::handlers::shared::{
    query_param_optional_bool, query_param_value, unix_ms_to_rfc3339, unix_secs_to_rfc3339,
};
use crate::provider_key_auth::{
    provider_key_configured_api_formats, provider_key_effective_api_formats,
};
use crate::AppState;
use aether_data_contracts::repository::candidates::{
    PublicHealthTimelineBucket, RequestCandidateStatus, StoredRequestCandidate,
};
use aether_data_contracts::repository::global_models::{
    PublicCatalogModelListQuery, PublicCatalogModelSearchQuery, StoredPublicCatalogModel,
};
use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) fn request_candidate_status_label(status: RequestCandidateStatus) -> &'static str {
    match status {
        RequestCandidateStatus::Available => "available",
        RequestCandidateStatus::Unused => "unused",
        RequestCandidateStatus::Pending => "pending",
        RequestCandidateStatus::Streaming => "streaming",
        RequestCandidateStatus::Success => "success",
        RequestCandidateStatus::Failed => "failed",
        RequestCandidateStatus::Cancelled => "cancelled",
        RequestCandidateStatus::Skipped => "skipped",
    }
}

pub(crate) fn request_candidate_event_unix_ms(candidate: &StoredRequestCandidate) -> u64 {
    candidate
        .finished_at_unix_ms
        .or(candidate.started_at_unix_ms)
        .unwrap_or(candidate.created_at_unix_ms)
}

pub(crate) fn normalize_admin_base_url(base_url: &str) -> Result<String, String> {
    let trimmed = base_url.trim();
    if trimmed.is_empty() {
        return Err("base_url 不能为空".to_string());
    }
    let normalized = trimmed.trim_end_matches('/');
    let lower = normalized.to_ascii_lowercase();
    if !lower.starts_with("http://") && !lower.starts_with("https://") {
        return Err("URL 必须以 http:// 或 https:// 开头".to_string());
    }
    Ok(normalized.to_string())
}

pub(crate) fn admin_requested_force_stream(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Bool(value) => *value,
        serde_json::Value::String(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "force_stream" | "stream" | "sse" | "true" | "1" | "yes"
        ),
        serde_json::Value::Number(value) => value.as_i64() == Some(1),
        _ => false,
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ApiFormatHealthMonitorOptions {
    pub(crate) include_api_path: bool,
    pub(crate) include_provider_count: bool,
    pub(crate) include_key_count: bool,
}

pub(crate) fn provider_key_api_formats(key: &StoredProviderCatalogKey) -> Vec<String> {
    provider_key_configured_api_formats(key)
}

pub(crate) async fn build_public_providers_payload(
    state: &AppState,
    query: Option<&str>,
) -> Option<serde_json::Value> {
    if !state.has_provider_catalog_data_reader() {
        return None;
    }

    let is_active = query_param_optional_bool(query, "is_active");
    let skip = query_param_value(query, "skip")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let limit = query_param_value(query, "limit")
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0 && *value <= 1000)
        .unwrap_or(100);

    let active_only = is_active.unwrap_or(true);
    let mut providers = state
        .list_provider_catalog_providers(active_only)
        .await
        .ok()
        .unwrap_or_default();
    if matches!(is_active, Some(false)) {
        providers.retain(|provider| !provider.is_active);
    }
    providers.sort_by(|left, right| {
        left.provider_priority
            .cmp(&right.provider_priority)
            .then_with(|| left.name.cmp(&right.name))
    });
    let providers = providers
        .into_iter()
        .skip(skip)
        .take(limit)
        .collect::<Vec<_>>();
    let provider_ids = providers
        .iter()
        .map(|provider| provider.id.clone())
        .collect::<Vec<_>>();
    let provider_ids_set = provider_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let endpoints = if provider_ids.is_empty() {
        Vec::new()
    } else {
        state
            .list_provider_catalog_endpoints_by_provider_ids(&provider_ids)
            .await
            .ok()
            .unwrap_or_default()
    };

    let mut endpoints_count_by_provider = BTreeMap::<String, usize>::new();
    let mut active_endpoints_count_by_provider = BTreeMap::<String, usize>::new();
    let mut api_formats = BTreeSet::<String>::new();
    for endpoint in &endpoints {
        *endpoints_count_by_provider
            .entry(endpoint.provider_id.clone())
            .or_default() += 1;
        if endpoint.is_active {
            *active_endpoints_count_by_provider
                .entry(endpoint.provider_id.clone())
                .or_default() += 1;
            api_formats.insert(endpoint.api_format.clone());
        }
    }

    let mut models_by_provider = BTreeMap::<String, BTreeSet<String>>::new();
    if state.has_minimal_candidate_selection_reader() {
        for api_format in api_formats {
            let rows = state
                .list_minimal_candidate_selection_rows_for_api_format(&api_format)
                .await
                .ok()
                .unwrap_or_default();
            for row in rows {
                if provider_ids_set.contains(row.provider_id.as_str()) {
                    models_by_provider
                        .entry(row.provider_id.clone())
                        .or_default()
                        .insert(row.global_model_id.clone());
                }
            }
        }
    }

    let providers = providers
        .into_iter()
        .map(|provider| {
            let provider_id = provider.id.clone();
            let description = provider
                .config
                .as_ref()
                .and_then(|value| value.get("description"))
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned);
            let model_count = models_by_provider
                .get(&provider_id)
                .map(BTreeSet::len)
                .unwrap_or(0);
            json!({
                "id": provider_id.clone(),
                "name": provider.name,
                "description": description,
                "website": provider.website,
                "is_active": provider.is_active,
                "provider_priority": provider.provider_priority,
                "models_count": model_count,
                "active_models_count": model_count,
                "endpoints_count": endpoints_count_by_provider.get(&provider_id).copied().unwrap_or(0),
                "active_endpoints_count": active_endpoints_count_by_provider.get(&provider_id).copied().unwrap_or(0),
            })
        })
        .collect::<Vec<_>>();

    Some(serde_json::Value::Array(providers))
}

fn serialize_public_catalog_model(model: StoredPublicCatalogModel) -> serde_json::Value {
    json!({
        "id": model.id,
        "provider_id": model.provider_id,
        "provider_name": model.provider_name,
        "name": model.name,
        "display_name": model.display_name,
        "description": model.description,
        "tags": serde_json::Value::Null,
        "icon_url": model.icon_url,
        "input_price_per_1m": model.input_price_per_1m,
        "output_price_per_1m": model.output_price_per_1m,
        "cache_creation_price_per_1m": model.cache_creation_price_per_1m,
        "cache_read_price_per_1m": model.cache_read_price_per_1m,
        "supports_vision": model.supports_vision,
        "supports_function_calling": model.supports_function_calling,
        "supports_streaming": model.supports_streaming,
        "is_active": model.is_active,
    })
}

pub(crate) async fn build_public_catalog_models_payload(
    state: &AppState,
    query: Option<&str>,
) -> Option<serde_json::Value> {
    if !state.has_global_model_data_reader() {
        return None;
    }

    let provider_id = query_param_value(query, "provider_id")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let skip = query_param_value(query, "skip")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let limit = query_param_value(query, "limit")
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0 && *value <= 1000)
        .unwrap_or(100);

    let items = state
        .list_public_catalog_models(&PublicCatalogModelListQuery {
            provider_id,
            offset: skip,
            limit,
        })
        .await
        .ok()?;

    Some(serde_json::Value::Array(
        items
            .into_iter()
            .map(serialize_public_catalog_model)
            .collect(),
    ))
}

pub(crate) async fn build_public_catalog_search_models_payload(
    state: &AppState,
    query: Option<&str>,
) -> Option<serde_json::Value> {
    if !state.has_global_model_data_reader() {
        return None;
    }

    let search = query_param_value(query, "q")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())?;
    let provider_id = query_param_value(query, "provider_id")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let limit = query_param_value(query, "limit")
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0 && *value <= 1000)
        .unwrap_or(20);

    let items = state
        .search_public_catalog_models(&PublicCatalogModelSearchQuery {
            search,
            provider_id,
            limit,
        })
        .await
        .ok()?;

    Some(serde_json::Value::Array(
        items
            .into_iter()
            .map(serialize_public_catalog_model)
            .collect(),
    ))
}

pub(crate) async fn build_api_format_health_monitor_payload(
    state: &AppState,
    lookback_hours: u64,
    per_format_limit: usize,
    options: ApiFormatHealthMonitorOptions,
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
    let mut active_endpoints_by_provider = BTreeMap::<String, Vec<_>>::new();
    let provider_type_by_id = providers
        .iter()
        .map(|provider| (provider.id.clone(), provider.provider_type.clone()))
        .collect::<BTreeMap<_, _>>();
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
        active_endpoints_by_provider
            .entry(endpoint.provider_id.clone())
            .or_default()
            .push(endpoint);
    }
    let all_endpoint_ids = endpoint_to_format.keys().cloned().collect::<Vec<_>>();

    let mut key_counts_by_format = BTreeMap::<String, usize>::new();
    if options.include_key_count && !provider_ids.is_empty() {
        let keys = state
            .list_provider_catalog_key_summaries_by_provider_ids(&provider_ids)
            .await
            .ok()
            .unwrap_or_default();
        for key in keys.into_iter().filter(|key| key.is_active) {
            let provider_type = provider_type_by_id
                .get(&key.provider_id)
                .map(String::as_str)
                .unwrap_or("");
            let endpoints = active_endpoints_by_provider
                .get(&key.provider_id)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            for api_format in provider_key_effective_api_formats(&key, provider_type, endpoints) {
                if provider_ids_by_format
                    .get(&api_format)
                    .is_some_and(|provider_ids| provider_ids.contains(key.provider_id.as_str()))
                {
                    *key_counts_by_format.entry(api_format).or_default() += 1;
                }
            }
        }
    }

    let status_counts = state
        .count_finalized_request_candidate_statuses_by_endpoint_ids_since(
            &all_endpoint_ids,
            since_unix_secs,
        )
        .await
        .ok()
        .unwrap_or_default();
    let mut status_totals = BTreeMap::<String, (u64, u64, u64)>::new();
    for row in status_counts {
        let Some(api_format) = endpoint_to_format.get(&row.endpoint_id) else {
            continue;
        };
        let entry = status_totals.entry(api_format.clone()).or_insert((0, 0, 0));
        match row.status {
            RequestCandidateStatus::Success => entry.0 += row.count,
            RequestCandidateStatus::Failed => entry.1 += row.count,
            RequestCandidateStatus::Skipped => entry.2 += row.count,
            _ => {}
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

    let mut formats = Vec::new();
    for (api_format, endpoint_ids) in endpoint_ids_by_format {
        let attempts = state
            .list_finalized_request_candidates_by_endpoint_ids_since(
                &endpoint_ids,
                since_unix_secs,
                per_format_limit,
            )
            .await
            .ok()
            .unwrap_or_default();
        let (success_count, failed_count, skipped_count) =
            status_totals.get(&api_format).copied().unwrap_or((0, 0, 0));
        let total_attempts = success_count + failed_count + skipped_count;
        let actual_completed = success_count + failed_count;
        let success_rate = if actual_completed > 0 {
            success_count as f64 / actual_completed as f64
        } else {
            1.0
        };
        let last_event_at = attempts.first().and_then(|candidate| {
            candidate
                .finished_at_unix_ms
                .or(candidate.started_at_unix_ms)
                .or(Some(candidate.created_at_unix_ms))
        });
        let events = attempts
            .into_iter()
            .filter_map(|candidate| {
                let timestamp = candidate
                    .finished_at_unix_ms
                    .or(candidate.started_at_unix_ms)
                    .unwrap_or(candidate.created_at_unix_ms);
                Some(json!({
                    "timestamp": unix_ms_to_rfc3339(timestamp)?,
                    "status": request_candidate_status_label(candidate.status),
                    "status_code": candidate.status_code,
                    "latency_ms": candidate.latency_ms,
                    "error_type": candidate.error_type,
                }))
            })
            .collect::<Vec<_>>();
        let empty_timeline = BTreeMap::new();
        let timeline_source = timeline_by_format
            .get(&api_format)
            .unwrap_or(&empty_timeline);
        let (timeline, time_range_start, time_range_end) =
            build_public_health_timeline(timeline_source, 100);

        let mut format_payload = json!({
            "api_format": api_format.clone(),
            "total_attempts": total_attempts,
            "success_count": success_count,
            "failed_count": failed_count,
            "skipped_count": skipped_count,
            "success_rate": success_rate,
            "last_event_at": last_event_at.and_then(unix_ms_to_rfc3339),
            "events": events,
            "timeline": timeline,
            "time_range_start": time_range_start.and_then(unix_ms_to_rfc3339),
            "time_range_end": time_range_end.map(|ms| unix_ms_to_rfc3339(ms)).unwrap_or_else(|| unix_secs_to_rfc3339(now_unix_secs)),
        });
        if options.include_api_path {
            format_payload["api_path"] = json!(public_api_format_local_path(&api_format));
        }
        if options.include_provider_count {
            format_payload["provider_count"] = json!(provider_ids_by_format
                .get(&api_format)
                .map(BTreeSet::len)
                .unwrap_or(0));
        }
        if options.include_key_count {
            format_payload["key_count"] =
                json!(*key_counts_by_format.get(&api_format).unwrap_or(&0));
        }
        formats.push(format_payload);
    }

    Some(json!({
        "generated_at": unix_secs_to_rfc3339(now_unix_secs),
        "formats": formats,
    }))
}

pub(crate) fn build_public_health_timeline(
    buckets_by_segment: &BTreeMap<u32, PublicHealthTimelineBucket>,
    segments: u32,
) -> (Vec<&'static str>, Option<u64>, Option<u64>) {
    let mut timeline = Vec::with_capacity(segments as usize);
    let mut earliest_time: Option<u64> = None;
    let mut latest_time: Option<u64> = None;

    for segment_idx in 0..segments {
        let Some(bucket) = buckets_by_segment.get(&segment_idx) else {
            timeline.push("unknown");
            continue;
        };
        if bucket.total_count == 0 {
            timeline.push("unknown");
            continue;
        }

        earliest_time = match (earliest_time, bucket.min_created_at_unix_ms) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (None, Some(right)) => Some(right),
            (left, None) => left,
        };
        latest_time = match (latest_time, bucket.max_created_at_unix_ms) {
            (Some(left), Some(right)) => Some(left.max(right)),
            (None, Some(right)) => Some(right),
            (left, None) => left,
        };

        let actual_completed = bucket.success_count + bucket.failed_count;
        let success_rate = if actual_completed > 0 {
            bucket.success_count as f64 / actual_completed as f64
        } else {
            1.0
        };
        if success_rate >= 0.95 {
            timeline.push("healthy");
        } else if success_rate >= 0.7 {
            timeline.push("warning");
        } else {
            timeline.push("unhealthy");
        }
    }

    (timeline, earliest_time, latest_time)
}

pub(crate) fn api_format_display_name(api_format: &str) -> String {
    let raw = api_format.trim();
    let normalized = raw.to_ascii_lowercase();
    let Some((family, kind)) = normalized.split_once(':') else {
        return if raw.is_empty() {
            api_format.to_string()
        } else {
            raw.to_string()
        };
    };

    let family_label = match family {
        "claude" => "Claude",
        "openai" => "OpenAI",
        "gemini" => "Gemini",
        other => other,
    };
    let kind_label = match kind {
        "chat" => "Chat",
        "cli" => "CLI",
        "compact" => "Compact",
        "video" => "Video",
        "image" => "Image",
        other => other,
    };
    format!("{family_label} {kind_label}")
}

#[cfg(test)]
mod tests {
    use super::request_candidate_event_unix_ms;
    use crate::handlers::shared::unix_ms_to_rfc3339;
    use aether_data_contracts::repository::candidates::{
        RequestCandidateStatus, StoredRequestCandidate,
    };

    #[test]
    fn request_candidate_event_timestamp_uses_millisecond_precision() {
        let candidate = StoredRequestCandidate::new(
            "cand-1".to_string(),
            "req-1".to_string(),
            None,
            None,
            None,
            None,
            0,
            0,
            Some("provider-1".to_string()),
            Some("endpoint-1".to_string()),
            Some("key-1".to_string()),
            RequestCandidateStatus::Success,
            None,
            false,
            Some(200),
            None,
            None,
            Some(42),
            Some(1),
            None,
            None,
            1_700_000_000_000,
            Some(1_700_000_000_111),
            Some(1_700_000_000_123),
        )
        .expect("candidate should build");

        let event_unix_ms = request_candidate_event_unix_ms(&candidate);
        assert_eq!(event_unix_ms, 1_700_000_000_123);
        assert_eq!(
            unix_ms_to_rfc3339(event_unix_ms).as_deref(),
            Some("2023-11-14T22:13:20.123Z")
        );
    }
}
