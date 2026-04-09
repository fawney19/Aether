use crate::handlers::admin::request::AdminAppState;
use crate::handlers::admin::shared::unix_secs_to_rfc3339;
use crate::handlers::public::{request_candidate_event_unix_secs, request_candidate_status_label};
use aether_data_contracts::repository::candidates::{
    RequestCandidateStatus, StoredRequestCandidate,
};
use serde_json::json;
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) async fn build_admin_provider_health_monitor_payload(
    state: &AdminAppState<'_>,
    provider_id: &str,
    lookback_hours: u64,
    per_endpoint_limit: usize,
) -> Option<serde_json::Value> {
    let state = state.as_ref();
    if !state.has_provider_catalog_data_reader() || !state.has_request_candidate_data_reader() {
        return None;
    }

    let provider = state
        .read_provider_catalog_providers_by_ids(&[provider_id.to_string()])
        .await
        .ok()
        .and_then(|mut providers| providers.drain(..).next())?;
    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let since_unix_secs = now_unix_secs.saturating_sub(lookback_hours * 3600);

    let mut endpoints = state
        .list_provider_catalog_endpoints_by_provider_ids(std::slice::from_ref(&provider.id))
        .await
        .ok()
        .unwrap_or_default();
    endpoints.sort_by(|left, right| {
        left.api_format
            .cmp(&right.api_format)
            .then_with(|| left.id.cmp(&right.id))
    });

    if endpoints.is_empty() {
        return Some(json!({
            "provider_id": provider.id,
            "provider_name": provider.name,
            "generated_at": unix_secs_to_rfc3339(now_unix_secs),
            "endpoints": [],
        }));
    }

    let endpoint_ids = endpoints
        .iter()
        .map(|endpoint| endpoint.id.clone())
        .collect::<Vec<_>>();
    let fetch_limit = per_endpoint_limit
        .saturating_mul(endpoint_ids.len())
        .max(per_endpoint_limit);
    let attempts = state
        .list_finalized_request_candidates_by_endpoint_ids_since(
            &endpoint_ids,
            since_unix_secs,
            fetch_limit,
        )
        .await
        .ok()
        .unwrap_or_default();

    let mut attempts_by_endpoint = BTreeMap::<String, Vec<StoredRequestCandidate>>::new();
    for candidate in attempts {
        let Some(endpoint_id) = candidate.endpoint_id.clone() else {
            continue;
        };
        attempts_by_endpoint
            .entry(endpoint_id)
            .or_default()
            .push(candidate);
    }

    for candidates in attempts_by_endpoint.values_mut() {
        candidates.sort_by(|left, right| {
            right
                .created_at_unix_secs
                .cmp(&left.created_at_unix_secs)
                .then_with(|| right.id.cmp(&left.id))
        });
        candidates.truncate(per_endpoint_limit);
        candidates.sort_by(|left, right| {
            request_candidate_event_unix_secs(left)
                .cmp(&request_candidate_event_unix_secs(right))
                .then_with(|| left.id.cmp(&right.id))
        });
    }

    let endpoints = endpoints
        .into_iter()
        .map(|endpoint| {
            let candidates = attempts_by_endpoint.remove(&endpoint.id).unwrap_or_default();
            let success_count = candidates
                .iter()
                .filter(|candidate| candidate.status == RequestCandidateStatus::Success)
                .count();
            let failed_count = candidates
                .iter()
                .filter(|candidate| candidate.status == RequestCandidateStatus::Failed)
                .count();
            let skipped_count = candidates
                .iter()
                .filter(|candidate| candidate.status == RequestCandidateStatus::Skipped)
                .count();
            let total_attempts = candidates.len();
            let success_rate = if total_attempts > 0 {
                success_count as f64 / total_attempts as f64
            } else {
                1.0
            };
            let last_event_at = candidates
                .last()
                .and_then(|candidate| unix_secs_to_rfc3339(request_candidate_event_unix_secs(candidate)));
            let events = candidates
                .into_iter()
                .filter_map(|candidate| {
                    Some(json!({
                        "timestamp": unix_secs_to_rfc3339(request_candidate_event_unix_secs(&candidate))?,
                        "status": request_candidate_status_label(candidate.status),
                        "status_code": candidate.status_code,
                        "latency_ms": candidate.latency_ms,
                        "error_type": candidate.error_type,
                        "error_message": candidate.error_message,
                    }))
                })
                .collect::<Vec<_>>();

            json!({
                "endpoint_id": endpoint.id,
                "api_format": endpoint.api_format,
                "is_active": endpoint.is_active,
                "total_attempts": total_attempts,
                "success_count": success_count,
                "failed_count": failed_count,
                "skipped_count": skipped_count,
                "success_rate": success_rate,
                "last_event_at": last_event_at,
                "events": events,
            })
        })
        .collect::<Vec<_>>();

    Some(json!({
        "provider_id": provider.id,
        "provider_name": provider.name,
        "generated_at": unix_secs_to_rfc3339(now_unix_secs),
        "endpoints": endpoints,
    }))
}
