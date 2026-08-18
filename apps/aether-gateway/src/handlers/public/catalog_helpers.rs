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
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use aether_data_contracts::repository::usage::{
    StoredRequestUsageAudit, StoredUsageBreakdownSummaryRow, UsageAuditListQuery,
    UsageBreakdownGroupBy, UsageBreakdownSummaryQuery,
};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};

const USER_CANCELLED_STATUS_CODE: u16 = 499;

#[derive(Clone, Copy, Debug, Default)]
struct HealthTimelineMetricBucket {
    total_count: u64,
    sla_eligible_count: u64,
    success_count: u64,
    failed_count: u64,
    user_error_count: u64,
    latency_sum_ms: u64,
    latency_samples: u64,
    first_byte_sum_ms: u64,
    first_byte_samples: u64,
    output_tokens: u64,
    response_time_sum_ms: u64,
}

#[derive(Clone, Copy, Debug)]
struct HealthTimelineDetailCounts {
    status: &'static str,
    total_attempts: u64,
    sla_eligible_count: u64,
    success_count: u64,
    failed_count: u64,
    user_error_count: u64,
}

#[derive(Clone, Copy, Debug)]
struct HealthTimelineWindow {
    since_unix_secs: u64,
    until_unix_secs: u64,
    segments: u32,
}

impl HealthTimelineMetricBucket {
    fn add_usage_event(&mut self, event: &StoredRequestUsageAudit) {
        self.total_count = self.total_count.saturating_add(1);
        let outcome = event.outcome_class();
        if outcome.is_sla_eligible() {
            self.sla_eligible_count = self.sla_eligible_count.saturating_add(1);
        }
        if outcome.is_success() {
            self.success_count = self.success_count.saturating_add(1);
        } else if outcome.is_service_error() {
            self.failed_count = self.failed_count.saturating_add(1);
        } else if outcome.is_user_error() {
            self.user_error_count = self.user_error_count.saturating_add(1);
        }
        if let Some(response_time_ms) = event.response_time_ms {
            self.latency_sum_ms = self.latency_sum_ms.saturating_add(response_time_ms);
            self.latency_samples = self.latency_samples.saturating_add(1);
            self.response_time_sum_ms = self.response_time_sum_ms.saturating_add(response_time_ms);
        }
        if let Some(first_byte_time_ms) = event.first_byte_time_ms {
            self.first_byte_sum_ms = self.first_byte_sum_ms.saturating_add(first_byte_time_ms);
            self.first_byte_samples = self.first_byte_samples.saturating_add(1);
        }
        self.output_tokens = self.output_tokens.saturating_add(event.output_tokens);
    }

    fn avg_latency_ms(self) -> Option<f64> {
        if self.latency_samples == 0 {
            None
        } else {
            Some(self.latency_sum_ms as f64 / self.latency_samples as f64)
        }
    }

    fn avg_first_byte_ms(self) -> Option<f64> {
        if self.first_byte_samples == 0 {
            None
        } else {
            Some(self.first_byte_sum_ms as f64 / self.first_byte_samples as f64)
        }
    }

    fn avg_tps(self) -> Option<f64> {
        if self.output_tokens == 0 || self.response_time_sum_ms == 0 {
            None
        } else {
            Some(self.output_tokens as f64 / (self.response_time_sum_ms as f64 / 1000.0))
        }
    }
}

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

pub(crate) fn sanitize_public_model_config_for_user(
    config: Option<serde_json::Value>,
) -> Option<serde_json::Value> {
    let Some(mut config) = config else {
        return None;
    };
    if let Some(object) = config.as_object_mut() {
        for key in [
            "model_mappings",
            "model_mapping",
            "global_model_mappings",
            "provider_model_mappings",
            "provider_model_aliases",
            "mapping_preview",
            "model_mapping_preview",
        ] {
            object.remove(key);
        }
    }
    Some(config)
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

#[derive(Clone, Copy)]
pub(crate) struct ModelHealthMonitorOptions {
    pub(crate) include_provider_count: bool,
}

const API_FORMAT_HEALTH_TIMELINE_SEGMENTS: u32 = 60;
const MODEL_HEALTH_TIMELINE_SEGMENTS: u32 = 60;

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
            let model_count = models_by_provider
                .get(&provider_id)
                .map(BTreeSet::len)
                .unwrap_or(0);
            json!({
                "id": provider_id.clone(),
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
        "supports_embedding": model.supports_embedding,
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
    let usage_data_available = state.has_usage_data_reader();
    let usage_breakdown_by_format = if usage_data_available {
        state
            .summarize_usage_breakdown(&UsageBreakdownSummaryQuery {
                created_from_unix_secs: since_unix_secs,
                created_until_unix_secs: now_unix_secs,
                user_id: None,
                provider_name: None,
                model: None,
                api_format: None,
                exclude_status_codes: vec![USER_CANCELLED_STATUS_CODE],
                group_by: UsageBreakdownGroupBy::ApiFormat,
            })
            .await
            .ok()
            .unwrap_or_default()
            .into_iter()
            .filter(|row| !row.group_key.trim().is_empty())
            .map(|row| (row.group_key.clone(), row))
            .collect::<BTreeMap<_, _>>()
    } else {
        BTreeMap::new()
    };

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
    let mut status_totals = BTreeMap::<String, (u64, u64, u64, u64)>::new();
    for row in status_counts {
        let Some(api_format) = endpoint_to_format.get(&row.endpoint_id) else {
            continue;
        };
        let entry = status_totals
            .entry(api_format.clone())
            .or_insert((0, 0, 0, 0));
        match row.status {
            RequestCandidateStatus::Success => {
                entry.0 += row.sla_eligible_count;
                entry.3 += row.user_error_count;
            }
            RequestCandidateStatus::Failed => {
                entry.1 += row.sla_eligible_count;
                entry.3 += row.user_error_count;
            }
            RequestCandidateStatus::Skipped => entry.2 += row.count,
            _ => {}
        }
    }

    let timeline_rows = state
        .aggregate_finalized_request_candidate_timeline_by_endpoint_ids_since(
            &all_endpoint_ids,
            since_unix_secs,
            now_unix_secs,
            API_FORMAT_HEALTH_TIMELINE_SEGMENTS,
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
                sla_eligible_count: 0,
                success_count: 0,
                failed_count: 0,
                user_error_count: 0,
                min_created_at_unix_ms: None,
                max_created_at_unix_ms: None,
            });
        bucket.total_count += row.total_count;
        bucket.sla_eligible_count += row.sla_eligible_count;
        bucket.success_count += row.success_count;
        bucket.failed_count += row.failed_count;
        bucket.user_error_count += row.user_error_count;
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
        let (success_count, failed_count, skipped_count, user_error_count) = status_totals
            .get(&api_format)
            .copied()
            .unwrap_or((0, 0, 0, 0));
        let sla_eligible_count = success_count + failed_count;
        let total_attempts = sla_eligible_count + skipped_count + user_error_count;
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
        let avg_latency_ms = usage_breakdown_by_format
            .get(&api_format)
            .and_then(model_health_average_latency_ms)
            .or_else(|| request_candidate_average_latency_ms(&attempts));
        let avg_tps = usage_breakdown_by_format
            .get(&api_format)
            .and_then(model_health_average_tps);
        let usage_events = if usage_data_available {
            state
                .list_usage_audits(&UsageAuditListQuery {
                    created_from_unix_secs: Some(since_unix_secs),
                    created_until_unix_secs: Some(now_unix_secs),
                    api_format: Some(api_format.clone()),
                    exclude_status_codes: vec![USER_CANCELLED_STATUS_CODE],
                    limit: Some(per_format_limit),
                    newest_first: true,
                    ..UsageAuditListQuery::default()
                })
                .await
                .ok()
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let avg_first_byte_ms = model_health_average_first_byte_ms(&usage_events);
        let events = attempts
            .into_iter()
            .filter_map(|candidate| {
                let timestamp = candidate
                    .finished_at_unix_ms
                    .or(candidate.started_at_unix_ms)
                    .unwrap_or(candidate.created_at_unix_ms);
                Some(json!({
                    "timestamp": unix_ms_to_rfc3339(timestamp)?,
                    "status": if candidate.outcome_class().is_user_error() {
                        "user_error"
                    } else {
                        request_candidate_status_label(candidate.status)
                    },
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
            build_public_health_timeline(timeline_source, API_FORMAT_HEALTH_TIMELINE_SEGMENTS);
        let timeline_details = build_public_health_timeline_details(
            timeline_source,
            since_unix_secs,
            now_unix_secs,
            API_FORMAT_HEALTH_TIMELINE_SEGMENTS,
            &usage_events,
        );

        let mut format_payload = json!({
            "api_format": api_format.clone(),
            "total_attempts": total_attempts,
            "sla_eligible_count": sla_eligible_count,
            "success_count": success_count,
            "failed_count": failed_count,
            "service_error_count": failed_count,
            "user_error_count": user_error_count,
            "skipped_count": skipped_count,
            "success_rate": success_rate,
            "avg_latency_ms": avg_latency_ms,
            "avg_first_byte_ms": avg_first_byte_ms,
            "avg_tps": avg_tps,
            "last_event_at": last_event_at.and_then(unix_ms_to_rfc3339),
            "events": events,
            "timeline": timeline,
            "timeline_details": timeline_details,
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

pub(crate) async fn build_model_health_monitor_payload(
    state: &AppState,
    lookback_hours: u64,
    model_limit: usize,
    per_model_limit: usize,
    options: ModelHealthMonitorOptions,
) -> Option<serde_json::Value> {
    if !state.has_usage_data_reader() {
        return None;
    }

    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    let since_unix_secs = now_unix_secs.saturating_sub(lookback_hours * 3600);

    let breakdown = state
        .summarize_usage_breakdown(&UsageBreakdownSummaryQuery {
            created_from_unix_secs: since_unix_secs,
            created_until_unix_secs: now_unix_secs,
            user_id: None,
            provider_name: None,
            model: None,
            api_format: None,
            exclude_status_codes: vec![USER_CANCELLED_STATUS_CODE],
            group_by: UsageBreakdownGroupBy::Model,
        })
        .await
        .ok()
        .unwrap_or_default();

    let selected_models = breakdown
        .into_iter()
        .filter(|row| !row.group_key.trim().is_empty())
        .take(model_limit)
        .collect::<Vec<_>>();

    let mut models = Vec::with_capacity(selected_models.len());
    for row in selected_models {
        let events = state
            .list_usage_audits(&UsageAuditListQuery {
                created_from_unix_secs: Some(since_unix_secs),
                created_until_unix_secs: Some(now_unix_secs),
                model: Some(row.group_key.clone()),
                exclude_status_codes: vec![USER_CANCELLED_STATUS_CODE],
                limit: Some(per_model_limit),
                newest_first: true,
                ..UsageAuditListQuery::default()
            })
            .await
            .ok()
            .unwrap_or_default();

        let (timeline, time_range_start, time_range_end) = build_model_health_timeline(
            &events,
            since_unix_secs,
            now_unix_secs,
            MODEL_HEALTH_TIMELINE_SEGMENTS,
        );
        let timeline_details = build_usage_health_timeline_details(
            &events,
            since_unix_secs,
            now_unix_secs,
            MODEL_HEALTH_TIMELINE_SEGMENTS,
        );
        let provider_count = model_health_provider_count(&events);
        let first_byte_average = model_health_average_first_byte_ms(&events);
        let last_event_at = events
            .first()
            .and_then(|item| unix_secs_to_rfc3339(item.created_at_unix_ms));
        let event_payload = events
            .iter()
            .rev()
            .map(model_health_event_payload)
            .collect::<Vec<_>>();

        let total_attempts = row.request_count;
        let sla_eligible_count = row.sla_eligible_count;
        let success_count = row.success_count.min(sla_eligible_count);
        let failed_count = sla_eligible_count.saturating_sub(success_count);
        let success_rate = if sla_eligible_count > 0 {
            success_count as f64 / sla_eligible_count as f64
        } else {
            1.0
        };
        let avg_latency_ms = model_health_average_latency_ms(&row);

        let model_name = row.group_key.clone();
        let mut model_payload = json!({
            "model": model_name,
            "display_name": model_health_display_name(&row.group_key),
            "total_attempts": total_attempts,
            "sla_eligible_count": sla_eligible_count,
            "success_count": success_count,
            "failed_count": failed_count,
            "service_error_count": failed_count,
            "user_error_count": row.user_error_count,
            "success_rate": success_rate,
            "avg_latency_ms": avg_latency_ms,
            "avg_first_byte_ms": first_byte_average,
            "avg_tps": model_health_average_tps(&row),
            "last_event_at": last_event_at,
            "events": event_payload,
            "timeline": timeline,
            "timeline_details": timeline_details,
            "time_range_start": unix_secs_to_rfc3339(time_range_start),
            "time_range_end": unix_secs_to_rfc3339(time_range_end),
        });
        if options.include_provider_count {
            model_payload["provider_count"] = json!(provider_count);
        }
        models.push(model_payload);
    }

    Some(json!({
        "generated_at": unix_secs_to_rfc3339(now_unix_secs),
        "models": models,
    }))
}

pub(crate) async fn build_provider_health_monitor_payload(
    state: &AppState,
    lookback_hours: u64,
    provider_limit: usize,
    per_provider_model_limit: usize,
    per_model_event_limit: usize,
) -> Option<serde_json::Value> {
    if !state.has_provider_catalog_data_reader() || !state.has_usage_data_reader() {
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
        .unwrap_or_default()
        .into_iter()
        .filter(|provider| provider.is_active)
        .take(provider_limit)
        .collect::<Vec<_>>();

    let provider_breakdown = state
        .summarize_usage_breakdown(&UsageBreakdownSummaryQuery {
            created_from_unix_secs: since_unix_secs,
            created_until_unix_secs: now_unix_secs,
            user_id: None,
            provider_name: None,
            model: None,
            api_format: None,
            exclude_status_codes: vec![USER_CANCELLED_STATUS_CODE],
            group_by: UsageBreakdownGroupBy::Provider,
        })
        .await
        .ok()
        .unwrap_or_default()
        .into_iter()
        .map(|row| (row.group_key.clone(), row))
        .collect::<BTreeMap<_, _>>();

    let mut payload = Vec::with_capacity(providers.len());
    for provider in providers {
        let provider_stats = provider_breakdown.get(&provider.name);
        payload.push(
            build_provider_health_payload(
                state,
                provider,
                provider_stats,
                since_unix_secs,
                now_unix_secs,
                per_provider_model_limit,
                per_model_event_limit,
            )
            .await,
        );
    }

    payload.sort_by(|left, right| {
        let left_rank = provider_health_sort_rank(left);
        let right_rank = provider_health_sort_rank(right);
        left_rank.cmp(&right_rank).then_with(|| {
            left.get("provider_name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .cmp(
                    right
                        .get("provider_name")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default(),
                )
        })
    });

    Some(json!({
        "generated_at": unix_secs_to_rfc3339(now_unix_secs),
        "providers": payload,
    }))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HealthMonitorRelationDimension {
    Endpoint,
    Model,
    Provider,
}

impl HealthMonitorRelationDimension {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "endpoint" | "api_format" | "api-format" => Some(Self::Endpoint),
            "model" => Some(Self::Model),
            "provider" => Some(Self::Provider),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Endpoint => "endpoint",
            Self::Model => "model",
            Self::Provider => "provider",
        }
    }
}

pub(crate) async fn build_related_health_monitor_payload(
    state: &AppState,
    lookback_hours: u64,
    dimension: HealthMonitorRelationDimension,
    value: &str,
    related_limit: usize,
    per_item_limit: usize,
    include_provider_info: bool,
) -> Option<serde_json::Value> {
    if !state.has_usage_data_reader() {
        return None;
    }

    let value = value.trim();
    if value.is_empty() {
        return None;
    }

    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    let since_unix_secs = now_unix_secs.saturating_sub(lookback_hours * 3600);
    let related_limit = related_limit.max(1);
    let per_item_limit = per_item_limit.max(1);

    let mut related_endpoints = Vec::new();
    let mut related_models = Vec::new();
    let mut related_providers = Vec::new();

    match dimension {
        HealthMonitorRelationDimension::Endpoint => {
            let api_format = value.to_string();
            related_models = build_related_health_items(
                state,
                breakdown_summary_query(
                    since_unix_secs,
                    now_unix_secs,
                    None,
                    None,
                    Some(api_format.clone()),
                    UsageBreakdownGroupBy::Model,
                ),
                since_unix_secs,
                now_unix_secs,
                related_limit,
                per_item_limit,
                "model",
                {
                    let api_format = api_format.clone();
                    move |row| {
                        usage_audit_query(
                            since_unix_secs,
                            now_unix_secs,
                            None,
                            Some(row.group_key.clone()),
                            Some(api_format.clone()),
                            per_item_limit,
                        )
                    }
                },
                |row, events| related_model_display_meta(row, events),
            )
            .await;

            if include_provider_info {
                let api_format = value.to_string();
                related_providers = build_related_health_items(
                    state,
                    breakdown_summary_query(
                        since_unix_secs,
                        now_unix_secs,
                        None,
                        None,
                        Some(api_format.clone()),
                        UsageBreakdownGroupBy::Provider,
                    ),
                    since_unix_secs,
                    now_unix_secs,
                    related_limit,
                    per_item_limit,
                    "provider",
                    {
                        let api_format = api_format.clone();
                        move |row| {
                            usage_audit_query(
                                since_unix_secs,
                                now_unix_secs,
                                Some(row.group_key.clone()),
                                None,
                                Some(api_format.clone()),
                                per_item_limit,
                            )
                        }
                    },
                    related_provider_display_meta,
                )
                .await;
            }
        }
        HealthMonitorRelationDimension::Model => {
            let model = value.to_string();
            related_endpoints = build_related_health_items(
                state,
                breakdown_summary_query(
                    since_unix_secs,
                    now_unix_secs,
                    None,
                    Some(model.clone()),
                    None,
                    UsageBreakdownGroupBy::ApiFormat,
                ),
                since_unix_secs,
                now_unix_secs,
                related_limit,
                per_item_limit,
                "endpoint",
                {
                    let model = model.clone();
                    move |row| {
                        usage_audit_query(
                            since_unix_secs,
                            now_unix_secs,
                            None,
                            Some(model.clone()),
                            Some(row.group_key.clone()),
                            per_item_limit,
                        )
                    }
                },
                related_endpoint_display_meta,
            )
            .await;

            if include_provider_info {
                let model = value.to_string();
                related_providers = build_related_health_items(
                    state,
                    breakdown_summary_query(
                        since_unix_secs,
                        now_unix_secs,
                        None,
                        Some(model.clone()),
                        None,
                        UsageBreakdownGroupBy::Provider,
                    ),
                    since_unix_secs,
                    now_unix_secs,
                    related_limit,
                    per_item_limit,
                    "provider",
                    {
                        let model = model.clone();
                        move |row| {
                            usage_audit_query(
                                since_unix_secs,
                                now_unix_secs,
                                Some(row.group_key.clone()),
                                Some(model.clone()),
                                None,
                                per_item_limit,
                            )
                        }
                    },
                    related_provider_display_meta,
                )
                .await;
            }
        }
        HealthMonitorRelationDimension::Provider => {
            let provider_name = value.to_string();
            related_endpoints = build_related_health_items(
                state,
                breakdown_summary_query(
                    since_unix_secs,
                    now_unix_secs,
                    Some(provider_name.clone()),
                    None,
                    None,
                    UsageBreakdownGroupBy::ApiFormat,
                ),
                since_unix_secs,
                now_unix_secs,
                related_limit,
                per_item_limit,
                "endpoint",
                {
                    let provider_name = provider_name.clone();
                    move |row| {
                        usage_audit_query(
                            since_unix_secs,
                            now_unix_secs,
                            Some(provider_name.clone()),
                            None,
                            Some(row.group_key.clone()),
                            per_item_limit,
                        )
                    }
                },
                related_endpoint_display_meta,
            )
            .await;

            let provider_name = value.to_string();
            related_models = build_related_health_items(
                state,
                breakdown_summary_query(
                    since_unix_secs,
                    now_unix_secs,
                    Some(provider_name.clone()),
                    None,
                    None,
                    UsageBreakdownGroupBy::Model,
                ),
                since_unix_secs,
                now_unix_secs,
                related_limit,
                per_item_limit,
                "model",
                {
                    let provider_name = provider_name.clone();
                    move |row| {
                        usage_audit_query(
                            since_unix_secs,
                            now_unix_secs,
                            Some(provider_name.clone()),
                            Some(row.group_key.clone()),
                            None,
                            per_item_limit,
                        )
                    }
                },
                |row, events| related_model_display_meta(row, events),
            )
            .await;
        }
    }

    Some(json!({
        "generated_at": unix_secs_to_rfc3339(now_unix_secs),
        "dimension": dimension.as_str(),
        "value": value,
        "related_endpoints": related_endpoints,
        "related_models": related_models,
        "related_providers": related_providers,
    }))
}

fn breakdown_summary_query(
    created_from_unix_secs: u64,
    created_until_unix_secs: u64,
    provider_name: Option<String>,
    model: Option<String>,
    api_format: Option<String>,
    group_by: UsageBreakdownGroupBy,
) -> UsageBreakdownSummaryQuery {
    UsageBreakdownSummaryQuery {
        created_from_unix_secs,
        created_until_unix_secs,
        user_id: None,
        provider_name,
        model,
        api_format,
        exclude_status_codes: vec![USER_CANCELLED_STATUS_CODE],
        group_by,
    }
}

fn usage_audit_query(
    created_from_unix_secs: u64,
    created_until_unix_secs: u64,
    provider_name: Option<String>,
    model: Option<String>,
    api_format: Option<String>,
    limit: usize,
) -> UsageAuditListQuery {
    UsageAuditListQuery {
        created_from_unix_secs: Some(created_from_unix_secs),
        created_until_unix_secs: Some(created_until_unix_secs),
        provider_name,
        model,
        api_format,
        exclude_status_codes: vec![USER_CANCELLED_STATUS_CODE],
        limit: Some(limit),
        newest_first: true,
        ..UsageAuditListQuery::default()
    }
}

async fn build_related_health_items<F, G>(
    state: &AppState,
    query: UsageBreakdownSummaryQuery,
    since_unix_secs: u64,
    now_unix_secs: u64,
    related_limit: usize,
    per_item_limit: usize,
    kind: &'static str,
    build_events_query: F,
    build_display_meta: G,
) -> Vec<serde_json::Value>
where
    F: Fn(&StoredUsageBreakdownSummaryRow) -> UsageAuditListQuery,
    G: Fn(&StoredUsageBreakdownSummaryRow, &[StoredRequestUsageAudit]) -> (String, Option<String>),
{
    let mut rows = state
        .summarize_usage_breakdown(&query)
        .await
        .ok()
        .unwrap_or_default()
        .into_iter()
        .filter(|row| !row.group_key.trim().is_empty())
        .collect::<Vec<_>>();

    rows.sort_by(|left, right| {
        related_health_sort_rank(left)
            .cmp(&related_health_sort_rank(right))
            .then_with(|| right.request_count.cmp(&left.request_count))
            .then_with(|| left.group_key.cmp(&right.group_key))
    });

    let mut items = Vec::new();
    for row in rows.into_iter().take(related_limit) {
        let events = state
            .list_usage_audits(&build_events_query(&row))
            .await
            .ok()
            .unwrap_or_default();
        let (display_name, meta_text) = build_display_meta(&row, &events);
        items.push(related_health_item_payload(
            kind,
            &row,
            &events,
            display_name,
            meta_text,
            since_unix_secs,
            now_unix_secs,
        ));
    }

    items
}

fn related_health_item_payload(
    kind: &'static str,
    row: &StoredUsageBreakdownSummaryRow,
    events: &[StoredRequestUsageAudit],
    display_name: String,
    meta_text: Option<String>,
    since_unix_secs: u64,
    now_unix_secs: u64,
) -> serde_json::Value {
    let (timeline, time_range_start, time_range_end) = build_model_health_timeline(
        events,
        since_unix_secs,
        now_unix_secs,
        MODEL_HEALTH_TIMELINE_SEGMENTS,
    );
    let timeline_details = build_usage_health_timeline_details(
        events,
        since_unix_secs,
        now_unix_secs,
        MODEL_HEALTH_TIMELINE_SEGMENTS,
    );
    let total_attempts = row.request_count;
    let sla_eligible_count = row.sla_eligible_count;
    let success_count = row.success_count.min(sla_eligible_count);
    let failed_count = sla_eligible_count.saturating_sub(success_count);
    let success_rate = if sla_eligible_count > 0 {
        success_count as f64 / sla_eligible_count as f64
    } else {
        1.0
    };
    let last_event_at = events
        .first()
        .and_then(|item| unix_secs_to_rfc3339(item.created_at_unix_ms));

    json!({
        "kind": kind,
        "key": row.group_key.clone(),
        "display_name": display_name,
        "meta_text": meta_text,
        "total_attempts": total_attempts,
        "sla_eligible_count": sla_eligible_count,
        "success_count": success_count,
        "failed_count": failed_count,
        "service_error_count": failed_count,
        "user_error_count": row.user_error_count,
        "success_rate": success_rate,
        "avg_latency_ms": model_health_average_latency_ms(row),
        "avg_first_byte_ms": model_health_average_first_byte_ms(events),
        "avg_tps": model_health_average_tps(row),
        "last_event_at": last_event_at,
        "timeline": timeline,
        "timeline_details": timeline_details,
        "time_range_start": unix_secs_to_rfc3339(time_range_start),
        "time_range_end": unix_secs_to_rfc3339(time_range_end),
    })
}

fn related_model_display_meta(
    row: &StoredUsageBreakdownSummaryRow,
    events: &[StoredRequestUsageAudit],
) -> (String, Option<String>) {
    let provider_count = model_health_provider_count(events);
    let meta_text = if provider_count > 0 {
        Some(format!("{provider_count} 个提供商"))
    } else {
        None
    };
    (model_health_display_name(&row.group_key), meta_text)
}

fn related_provider_display_meta(
    row: &StoredUsageBreakdownSummaryRow,
    _events: &[StoredRequestUsageAudit],
) -> (String, Option<String>) {
    (row.group_key.clone(), None)
}

fn related_endpoint_display_meta(
    row: &StoredUsageBreakdownSummaryRow,
    _events: &[StoredRequestUsageAudit],
) -> (String, Option<String>) {
    (
        api_format_display_name(&row.group_key),
        Some(public_api_format_local_path(&row.group_key).to_string()),
    )
}

fn related_health_sort_rank(row: &StoredUsageBreakdownSummaryRow) -> u8 {
    if row.sla_eligible_count == 0 {
        return 3;
    }
    let success_count = row.success_count.min(row.sla_eligible_count);
    let success_rate = success_count as f64 / row.sla_eligible_count as f64;
    if success_rate < 0.8 {
        0
    } else if success_rate < 0.95 {
        1
    } else {
        2
    }
}

async fn build_provider_health_payload(
    state: &AppState,
    provider: StoredProviderCatalogProvider,
    provider_stats: Option<&StoredUsageBreakdownSummaryRow>,
    since_unix_secs: u64,
    now_unix_secs: u64,
    per_provider_model_limit: usize,
    per_model_event_limit: usize,
) -> serde_json::Value {
    let model_breakdown = state
        .summarize_usage_breakdown(&UsageBreakdownSummaryQuery {
            created_from_unix_secs: since_unix_secs,
            created_until_unix_secs: now_unix_secs,
            user_id: None,
            provider_name: Some(provider.name.clone()),
            model: None,
            api_format: None,
            exclude_status_codes: vec![USER_CANCELLED_STATUS_CODE],
            group_by: UsageBreakdownGroupBy::Model,
        })
        .await
        .ok()
        .unwrap_or_default();

    let provider_event_limit = per_model_event_limit
        .saturating_mul(per_provider_model_limit.max(1))
        .max(per_model_event_limit);
    let provider_events = state
        .list_usage_audits(&UsageAuditListQuery {
            created_from_unix_secs: Some(since_unix_secs),
            created_until_unix_secs: Some(now_unix_secs),
            provider_name: Some(provider.name.clone()),
            exclude_status_codes: vec![USER_CANCELLED_STATUS_CODE],
            limit: Some(provider_event_limit),
            newest_first: true,
            ..UsageAuditListQuery::default()
        })
        .await
        .ok()
        .unwrap_or_default();
    let selected_model_rows = model_breakdown
        .iter()
        .filter(|row| !row.group_key.trim().is_empty())
        .take(per_provider_model_limit)
        .collect::<Vec<_>>();
    let mut events_by_model = selected_model_rows
        .iter()
        .map(|row| (row.group_key.clone(), Vec::new()))
        .collect::<BTreeMap<_, _>>();
    for event in &provider_events {
        if let Some(events) = events_by_model.get_mut(&event.model) {
            if events.len() < per_model_event_limit {
                events.push(event.clone());
            }
        }
    }

    let mut models = Vec::new();
    for row in selected_model_rows {
        let events = events_by_model.remove(&row.group_key).unwrap_or_default();
        models.push(model_health_payload_from_row(
            row,
            &events,
            since_unix_secs,
            now_unix_secs,
            None,
        ));
    }

    let (total_attempts, success_count, failed_count, success_rate, avg_latency_ms, avg_tps) =
        if let Some(row) = provider_stats {
            let total_attempts = row.request_count;
            let sla_eligible_count = row.sla_eligible_count;
            let success_count = row.success_count.min(sla_eligible_count);
            let failed_count = sla_eligible_count.saturating_sub(success_count);
            let success_rate = if sla_eligible_count > 0 {
                success_count as f64 / sla_eligible_count as f64
            } else {
                1.0
            };
            (
                total_attempts,
                success_count,
                failed_count,
                success_rate,
                model_health_average_latency_ms(row),
                model_health_average_tps(row),
            )
        } else {
            (0, 0, 0, 1.0, None, None)
        };

    let (timeline, time_range_start, time_range_end) = build_model_health_timeline(
        &provider_events,
        since_unix_secs,
        now_unix_secs,
        MODEL_HEALTH_TIMELINE_SEGMENTS,
    );
    let timeline_details = build_usage_health_timeline_details(
        &provider_events,
        since_unix_secs,
        now_unix_secs,
        MODEL_HEALTH_TIMELINE_SEGMENTS,
    );
    let last_event_at = provider_events
        .iter()
        .max_by_key(|event| event.created_at_unix_ms)
        .and_then(|event| unix_secs_to_rfc3339(event.created_at_unix_ms));

    json!({
        "provider_id": provider.id,
        "provider_name": provider.name,
        "provider_type": provider.provider_type,
        "is_active": provider.is_active,
        "total_attempts": total_attempts,
        "sla_eligible_count": provider_stats.map_or(0, |row| row.sla_eligible_count),
        "success_count": success_count,
        "failed_count": failed_count,
        "service_error_count": failed_count,
        "user_error_count": provider_stats.map_or(0, |row| row.user_error_count),
        "success_rate": success_rate,
        "avg_latency_ms": avg_latency_ms,
        "avg_first_byte_ms": model_health_average_first_byte_ms(&provider_events),
        "avg_tps": avg_tps,
        "model_count": model_breakdown.len(),
        "last_event_at": last_event_at,
        "timeline": timeline,
        "timeline_details": timeline_details,
        "time_range_start": unix_secs_to_rfc3339(time_range_start),
        "time_range_end": unix_secs_to_rfc3339(time_range_end),
        "models": models,
    })
}

fn provider_health_sort_rank(provider: &serde_json::Value) -> u8 {
    let sla_eligible_count = provider
        .get("sla_eligible_count")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    if sla_eligible_count == 0 {
        return 3;
    }
    let success_rate = provider
        .get("success_rate")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(1.0);
    if success_rate < 0.8 {
        0
    } else if success_rate < 0.95 {
        1
    } else {
        2
    }
}

fn model_health_payload_from_row(
    row: &StoredUsageBreakdownSummaryRow,
    events: &[StoredRequestUsageAudit],
    since_unix_secs: u64,
    now_unix_secs: u64,
    provider_count: Option<usize>,
) -> serde_json::Value {
    let (timeline, time_range_start, time_range_end) = build_model_health_timeline(
        events,
        since_unix_secs,
        now_unix_secs,
        MODEL_HEALTH_TIMELINE_SEGMENTS,
    );
    let timeline_details = build_usage_health_timeline_details(
        events,
        since_unix_secs,
        now_unix_secs,
        MODEL_HEALTH_TIMELINE_SEGMENTS,
    );
    let last_event_at = events
        .first()
        .and_then(|item| unix_secs_to_rfc3339(item.created_at_unix_ms));
    let event_payload = events
        .iter()
        .rev()
        .map(model_health_event_payload)
        .collect::<Vec<_>>();

    let total_attempts = row.request_count;
    let sla_eligible_count = row.sla_eligible_count;
    let success_count = row.success_count.min(sla_eligible_count);
    let failed_count = sla_eligible_count.saturating_sub(success_count);
    let success_rate = if sla_eligible_count > 0 {
        success_count as f64 / sla_eligible_count as f64
    } else {
        1.0
    };

    let mut model_payload = json!({
        "model": row.group_key.clone(),
        "display_name": model_health_display_name(&row.group_key),
        "total_attempts": total_attempts,
        "sla_eligible_count": sla_eligible_count,
        "success_count": success_count,
        "failed_count": failed_count,
        "service_error_count": failed_count,
        "user_error_count": row.user_error_count,
        "success_rate": success_rate,
        "avg_latency_ms": model_health_average_latency_ms(row),
        "avg_first_byte_ms": model_health_average_first_byte_ms(events),
        "avg_tps": model_health_average_tps(row),
        "last_event_at": last_event_at,
        "events": event_payload,
        "timeline": timeline,
        "timeline_details": timeline_details,
        "time_range_start": unix_secs_to_rfc3339(time_range_start),
        "time_range_end": unix_secs_to_rfc3339(time_range_end),
    });
    if let Some(provider_count) = provider_count {
        model_payload["provider_count"] = json!(provider_count);
    }
    model_payload
}

fn request_candidate_average_latency_ms(candidates: &[StoredRequestCandidate]) -> Option<f64> {
    let mut sum = 0u64;
    let mut count = 0u64;
    for candidate in candidates {
        if let Some(latency_ms) = candidate.latency_ms {
            sum = sum.saturating_add(latency_ms);
            count = count.saturating_add(1);
        }
    }
    if count == 0 {
        None
    } else {
        Some(sum as f64 / count as f64)
    }
}

fn model_health_average_latency_ms(row: &StoredUsageBreakdownSummaryRow) -> Option<f64> {
    if row.overall_response_time_samples > 0 {
        return Some(row.overall_response_time_sum_ms / row.overall_response_time_samples as f64);
    }
    if row.response_time_samples > 0 {
        return Some(row.response_time_sum_ms / row.response_time_samples as f64);
    }
    None
}

fn model_health_average_tps(row: &StoredUsageBreakdownSummaryRow) -> Option<f64> {
    if row.output_tokens == 0 || row.response_time_sum_ms <= 0.0 {
        return None;
    }
    Some(row.output_tokens as f64 / (row.response_time_sum_ms / 1000.0))
}

fn model_health_average_first_byte_ms(events: &[StoredRequestUsageAudit]) -> Option<f64> {
    let mut sum = 0u64;
    let mut count = 0u64;
    for event in events {
        if let Some(first_byte_time_ms) = event.first_byte_time_ms {
            sum = sum.saturating_add(first_byte_time_ms);
            count = count.saturating_add(1);
        }
    }
    if count == 0 {
        None
    } else {
        Some(sum as f64 / count as f64)
    }
}

fn model_health_provider_count(events: &[StoredRequestUsageAudit]) -> usize {
    let mut providers = BTreeSet::new();
    for event in events {
        if let Some(provider_id) = event.provider_id.as_deref() {
            providers.insert(provider_id.to_string());
        } else if !event.provider_name.trim().is_empty() {
            providers.insert(event.provider_name.clone());
        }
    }
    providers.len()
}

fn model_health_event_payload(event: &StoredRequestUsageAudit) -> serde_json::Value {
    json!({
        "timestamp": unix_secs_to_rfc3339(event.created_at_unix_ms),
        "status": model_health_event_status(event),
        "status_code": event.status_code,
        "latency_ms": event.response_time_ms,
        "first_byte_time_ms": event.first_byte_time_ms,
        "error_type": event.error_category,
    })
}

fn model_health_event_status(event: &StoredRequestUsageAudit) -> &'static str {
    match event.outcome_class() {
        aether_data_contracts::repository::usage::RequestOutcomeClass::Success => "success",
        aether_data_contracts::repository::usage::RequestOutcomeClass::UserError => "user_error",
        aether_data_contracts::repository::usage::RequestOutcomeClass::ServiceError => "failed",
        aether_data_contracts::repository::usage::RequestOutcomeClass::Cancelled => "cancelled",
        aether_data_contracts::repository::usage::RequestOutcomeClass::InFlight => "in_flight",
    }
}

fn model_health_event_success(event: &StoredRequestUsageAudit) -> bool {
    event.outcome_class().is_success()
}

fn health_timeline_status(success_count: u64, failed_count: u64) -> &'static str {
    let actual_completed = success_count.saturating_add(failed_count);
    if actual_completed == 0 {
        return "unknown";
    }
    let success_rate = success_count as f64 / actual_completed as f64;
    if success_rate >= 0.95 {
        "healthy"
    } else if success_rate >= 0.7 {
        "warning"
    } else {
        "unhealthy"
    }
}

fn health_timeline_success_rate(success_count: u64, failed_count: u64) -> Option<f64> {
    let actual_completed = success_count.saturating_add(failed_count);
    if actual_completed == 0 {
        None
    } else {
        Some(success_count as f64 / actual_completed as f64)
    }
}

fn health_timeline_segment_index(
    timestamp_unix_secs: u64,
    since_unix_secs: u64,
    until_unix_secs: u64,
    segments: u32,
) -> Option<usize> {
    if segments == 0
        || timestamp_unix_secs < since_unix_secs
        || timestamp_unix_secs > until_unix_secs
    {
        return None;
    }
    let safe_range = until_unix_secs.saturating_sub(since_unix_secs).max(1);
    let offset = timestamp_unix_secs.saturating_sub(since_unix_secs);
    let mut segment_idx = ((offset as u128 * segments as u128) / safe_range as u128) as usize;
    if segment_idx >= segments as usize {
        segment_idx = segments.saturating_sub(1) as usize;
    }
    Some(segment_idx)
}

fn health_timeline_segment_bounds(
    since_unix_secs: u64,
    until_unix_secs: u64,
    segments: u32,
    segment_idx: u32,
) -> (u64, u64) {
    let segment_count = segments.max(1);
    let safe_range = until_unix_secs.saturating_sub(since_unix_secs).max(1);
    let start_offset =
        (safe_range as u128 * u128::from(segment_idx) / u128::from(segment_count)) as u64;
    let end_offset = (safe_range as u128 * u128::from(segment_idx.saturating_add(1))
        / u128::from(segment_count)) as u64;
    let start = since_unix_secs.saturating_add(start_offset);
    let end = if segment_idx.saturating_add(1) >= segment_count {
        until_unix_secs
    } else {
        since_unix_secs.saturating_add(end_offset)
    };
    (start, end.max(start))
}

fn aggregate_usage_timeline_metrics(
    events: &[StoredRequestUsageAudit],
    since_unix_secs: u64,
    until_unix_secs: u64,
    segments: u32,
) -> Vec<HealthTimelineMetricBucket> {
    let mut buckets = (0..segments)
        .map(|_| HealthTimelineMetricBucket::default())
        .collect::<Vec<_>>();
    for event in events {
        let Some(segment_idx) = health_timeline_segment_index(
            event.created_at_unix_ms,
            since_unix_secs,
            until_unix_secs,
            segments,
        ) else {
            continue;
        };
        if let Some(bucket) = buckets.get_mut(segment_idx) {
            bucket.add_usage_event(event);
        }
    }
    buckets
}

fn health_timeline_detail_payload(
    segment_idx: u32,
    counts: HealthTimelineDetailCounts,
    metrics: HealthTimelineMetricBucket,
    window: HealthTimelineWindow,
) -> serde_json::Value {
    let (range_start, range_end) = health_timeline_segment_bounds(
        window.since_unix_secs,
        window.until_unix_secs,
        window.segments,
        segment_idx,
    );
    json!({
        "segment_index": segment_idx,
        "status": counts.status,
        "time_range_start": unix_secs_to_rfc3339(range_start),
        "time_range_end": unix_secs_to_rfc3339(range_end),
        "total_attempts": counts.total_attempts,
        "sla_eligible_count": counts.sla_eligible_count,
        "success_count": counts.success_count,
        "failed_count": counts.failed_count,
        "service_error_count": counts.failed_count,
        "user_error_count": counts.user_error_count,
        "success_rate": health_timeline_success_rate(counts.success_count, counts.failed_count),
        "avg_latency_ms": metrics.avg_latency_ms(),
        "avg_first_byte_ms": metrics.avg_first_byte_ms(),
        "avg_tps": metrics.avg_tps(),
    })
}

pub(crate) fn build_public_health_timeline_details(
    buckets_by_segment: &BTreeMap<u32, PublicHealthTimelineBucket>,
    since_unix_secs: u64,
    until_unix_secs: u64,
    segments: u32,
    usage_events: &[StoredRequestUsageAudit],
) -> Vec<serde_json::Value> {
    let usage_metrics =
        aggregate_usage_timeline_metrics(usage_events, since_unix_secs, until_unix_secs, segments);
    let window = HealthTimelineWindow {
        since_unix_secs,
        until_unix_secs,
        segments,
    };
    (0..segments)
        .map(|segment_idx| {
            let count_bucket = buckets_by_segment.get(&segment_idx);
            let metrics = usage_metrics
                .get(segment_idx as usize)
                .copied()
                .unwrap_or_default();
            let total_attempts = count_bucket
                .map(|bucket| bucket.total_count)
                .unwrap_or(metrics.total_count);
            let success_count = count_bucket
                .map(|bucket| bucket.success_count)
                .unwrap_or(metrics.success_count);
            let failed_count = count_bucket
                .map(|bucket| bucket.failed_count)
                .unwrap_or(metrics.failed_count);
            let sla_eligible_count = count_bucket
                .map(|bucket| bucket.sla_eligible_count)
                .unwrap_or(metrics.sla_eligible_count);
            let user_error_count = count_bucket
                .map(|bucket| bucket.user_error_count)
                .unwrap_or(metrics.user_error_count);
            let status = health_timeline_status(success_count, failed_count);
            health_timeline_detail_payload(
                segment_idx,
                HealthTimelineDetailCounts {
                    status,
                    total_attempts,
                    sla_eligible_count,
                    success_count,
                    failed_count,
                    user_error_count,
                },
                metrics,
                window,
            )
        })
        .collect()
}

fn build_usage_health_timeline_details(
    events: &[StoredRequestUsageAudit],
    since_unix_secs: u64,
    until_unix_secs: u64,
    segments: u32,
) -> Vec<serde_json::Value> {
    let usage_metrics =
        aggregate_usage_timeline_metrics(events, since_unix_secs, until_unix_secs, segments);
    let window = HealthTimelineWindow {
        since_unix_secs,
        until_unix_secs,
        segments,
    };
    usage_metrics
        .into_iter()
        .enumerate()
        .map(|(index, metrics)| {
            let status = health_timeline_status(metrics.success_count, metrics.failed_count);
            health_timeline_detail_payload(
                index as u32,
                HealthTimelineDetailCounts {
                    status,
                    total_attempts: metrics.total_count,
                    sla_eligible_count: metrics.sla_eligible_count,
                    success_count: metrics.success_count,
                    failed_count: metrics.failed_count,
                    user_error_count: metrics.user_error_count,
                },
                metrics,
                window,
            )
        })
        .collect()
}

fn build_model_health_timeline(
    events: &[StoredRequestUsageAudit],
    since_unix_secs: u64,
    until_unix_secs: u64,
    segments: u32,
) -> (Vec<&'static str>, u64, u64) {
    #[derive(Default)]
    struct Bucket {
        success_count: u64,
        failed_count: u64,
    }

    let safe_range = until_unix_secs.saturating_sub(since_unix_secs).max(1);
    let mut buckets = (0..segments).map(|_| Bucket::default()).collect::<Vec<_>>();

    for event in events {
        let timestamp = event.created_at_unix_ms;
        if timestamp < since_unix_secs || timestamp > until_unix_secs {
            continue;
        }
        let offset = timestamp.saturating_sub(since_unix_secs);
        let mut segment_idx = ((offset as u128 * segments as u128) / safe_range as u128) as usize;
        if segment_idx >= segments as usize {
            segment_idx = segments.saturating_sub(1) as usize;
        }
        let bucket = &mut buckets[segment_idx];
        let outcome = event.outcome_class();
        if outcome.is_success() {
            bucket.success_count = bucket.success_count.saturating_add(1);
        } else if outcome.is_service_error() {
            bucket.failed_count = bucket.failed_count.saturating_add(1);
        }
    }

    let timeline = buckets
        .into_iter()
        .map(|bucket| health_timeline_status(bucket.success_count, bucket.failed_count))
        .collect::<Vec<_>>();

    (timeline, since_unix_secs, until_unix_secs)
}

fn model_health_display_name(model: &str) -> String {
    model.trim().to_string()
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
        if actual_completed == 0 {
            timeline.push("unknown");
            continue;
        }
        let success_rate = bucket.success_count as f64 / actual_completed as f64;
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
        "messages" => "Messages",
        "generate_content" => "Generate Content",
        "responses" => "Responses",
        "responses:compact" => "Responses Compact",
        "compact" => "Compact",
        "video" => "Video",
        "image" => "Image",
        "files" => "Files",
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
